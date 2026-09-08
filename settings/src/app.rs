//! The settings application: the model the loop folds, its messages, and
//! `update`.
//!
//! `view` lives here too (see below); every page contributes one `pub fn
//! view(&SettingsModel) -> View<Msg>` from its own module.

use std::rc::Rc;

use icedtea_ui::layout::Align;
use icedtea_ui::view::builders::{
    StackExt, StackPagesExt, box_, button, label, stack, stack_page, stack_switcher,
};
use icedtea_ui::view::{Cmd, View};
use icedtea_ui::widgets::types::Orientation;

use crate::compositor_reload::ReloadOutcome;
use crate::pages;
use crate::pages::PageId;
use crate::pages::displays::state::DisplaysState;

/// Everything the settings window shows, and nothing it does not.
///
/// `model` is `model.rs`'s untouched working/saved pair: dirty is
/// `working != saved`, computed on every `view`, never a flag. The GTK
/// `Ctx`/`Page`/`populating` machinery has no equivalent here — a programmatic
/// widget write cannot happen on an Elm loop, so there is nothing to guard.
/// Which palette entry the colour picker is editing.
///
/// The picker is a panel the Appearance page renders under the palette rows
/// (contract deviation P2-D11); this is the only state it needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorSlot {
    Background,
    Foreground,
    Accent,
}

pub struct SettingsModel {
    pub model: crate::model::Model,
    pub db_path: std::path::PathBuf,
    pub page: PageId,
    pub status: String,
    /// What is permanently unavailable because a worker thread could not be
    /// started, or empty when they all did.
    ///
    /// A *standing* fact, not a status line: worker availability is decided
    /// once, at spawn, and never changes. Seeding `status` with it (which is
    /// what this replaced) put it behind the first user edit — the very fold
    /// that makes Apply sensitive and so the moment it matters most.
    pub worker_warning: String,
    /// The action whose binding is being captured, if any (P3 arms it).
    pub capturing: Option<String>,
    /// Conflicting bindings, recomputed from `working` after every edit.
    pub conflicts: Vec<(icedtea_config::KeyCombo, Vec<String>)>,
    pub wallpaper_text: String,
    pub wallpaper_error: Option<String>,
    pub portal_available: bool,
    /// A file chooser is open. A second Browse click while one is up served
    /// a second, unprompted dialog once the first closed — the same latch
    /// `displays_in_flight` is for the Displays page.
    pub browse_in_flight: bool,
    /// The palette entry whose picker panel is open, if any.
    pub color_picker: Option<ColorSlot>,
    pub displays: DisplaysState,
    pub displays_status: String,
    pub displays_in_flight: bool,
    pub outputs_available: bool,
    /// The second Wayland connection, when there is one. `Cmd::Task` reaches
    /// Test/Apply through it (P4); `None` means output management is absent.
    pub outputs: Option<crate::outputs::pump::OutputsPump>,
    /// Worker senders, so `update` can `Cmd::Task` onto them.
    pub workers: crate::ipc::WorkerHandles,
}

impl SettingsModel {
    /// Load the working copy from `db_path`.
    #[must_use]
    pub fn new(db_path: std::path::PathBuf, workers: crate::ipc::WorkerHandles) -> Self {
        let model = crate::model::Model::load(&db_path);
        let wallpaper_text = model
            .working
            .appearance
            .wallpaper
            .clone()
            .unwrap_or_default();
        let conflicts = crate::model::duplicate_bindings(&model.working);
        SettingsModel {
            model,
            db_path,
            page: PageId::Appearance,
            worker_warning: worker_warning(&workers),
            status: String::new(),
            capturing: None,
            conflicts,
            wallpaper_text,
            wallpaper_error: None,
            portal_available: workers.portal_available(),
            browse_in_flight: false,
            color_picker: None,
            displays: DisplaysState::new(),
            displays_status: String::new(),
            displays_in_flight: false,
            outputs_available: false,
            outputs: None,
            workers,
        }
    }

    /// `working != saved`, the one dirty rule.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.model.is_dirty()
    }

    /// Attach the outputs connection, if the window got one.
    #[must_use]
    pub fn with_outputs(mut self, pump: Option<crate::outputs::pump::OutputsPump>) -> Self {
        // `manager_present()`, not `is_some()`: a connection with no
        // `zwlr_output_manager_v1` queues its `ManagerUnavailable` during
        // `attach`, before the loop exists, and nothing on a quiet socket will
        // ever wake the fd to deliver it. Starting optimistically available
        // would leave the page permanently lying. (`main` also seeds the loop
        // with `pump.drain()` so the initial enumeration itself is not lost.)
        self.outputs_available = pump
            .as_ref()
            .is_some_and(crate::outputs::pump::OutputsPump::manager_present);
        self.outputs = pump;
        self
    }
}

/// Everything that can change the model.
///
/// `Msg` crosses a thread boundary on the inbox, so it is `Send` and every
/// payload is owned or `Arc` — never `Rc` (M5-D2). P2, P3 and P4 append their
/// pages' variants (deviation P1-D1).
#[derive(Clone, Debug)]
pub enum Msg {
    /// A `StackSwitcher` button, by index.
    PageSelected(usize),
    Apply,
    Revert,
    /// From the reload worker, through the inbox.
    ///
    /// `shipped` is the snapshot the worker actually wrote — never the
    /// current working copy. An edit folded while the write was in flight
    /// used to be marked saved without ever reaching disk.
    Applied {
        shipped: std::sync::Arc<icedtea_config::Config>,
        result: Result<ReloadOutcome, String>,
    },
    /// The filesystem worker's answer to a Revert read of the config store:
    /// `Ok` with the loaded config (or defaults, for a genuinely absent store),
    /// or `Err` with a message when the store is present but unreadable
    /// (corrupt/IO/locked) — in which case the saved copy is kept, not wiped.
    ConfigLoaded(Result<std::sync::Arc<icedtea_config::Config>, String>),
    /// The compositor emitted `ConfigReloaded`.
    ConfigReloaded,

    // --- Appearance (P2) --------------------------------------------------
    /// A `drop_down` selection, by index into `model::BAR_POSITIONS`.
    BarPositionSelected(usize),
    BarHeightChanged(f64),
    CornerRadiusChanged(f64),
    /// A palette swatch's picker panel opened or closed (contract P2-D11).
    ColorPickerOpened(ColorSlot),
    ColorPickerClosed,
    /// A colour picked in a swatch's panel, packed as `ColorDialogC::pack`
    /// (M3 P5-D23).
    BackgroundPicked(f64),
    ForegroundPicked(f64),
    AccentPicked(f64),
    /// The Behavior page's snap-gap `spin_button` (contract deviation
    /// P2-D10: the control is on Behavior, the field stays on Appearance).
    SnapGapChanged(f64),

    // --- Behavior (P2) --------------------------------------------------
    RaiseOnFocusToggled(bool),
    HideBarOnFullscreenToggled(bool),
    SnapEnabledToggled(bool),

    /// One protocol message from the outputs connection, via `App::on_fd`.
    /// `Arc`, not `Rc`: `Msg` is `Send` (M5-D2).
    Outputs(std::sync::Arc<crate::outputs::OutputsMsg>),
    /// The wallpaper `Entry` changed, by the user typing or by `Revert`
    /// resetting it. Always authoritative (contract §2.6): validated the
    /// same way as [`Msg::WallpaperChosen`].
    WallpaperEdited(String),
    /// The Browse button was clicked.
    WallpaperBrowse,
    /// The portal worker's file chooser returned a path (contract §2.2).
    WallpaperChosen(std::path::PathBuf),
    /// The portal worker could not produce a path; the status line shows why
    /// (contract §2.2). This is the *unavailable* case, and it latches
    /// `portal_available` off.
    WallpaperPickerFailed(String),
    /// The user dismissed the file chooser. A working portal, a normal
    /// action: the status line says so and Browse stays live (amendment
    /// P2-D16).
    WallpaperPickerCancelled,
    /// The field's clear control.
    WallpaperCleared,
    /// The filesystem worker's verdict on `text`, which is applied only
    /// while `text` is still what the field holds (a later keystroke wins).
    WallpaperValidated {
        text: String,
        result: Result<String, String>,
    },

    // --- Workspaces (P3) --------------------------------------------------
    /// A workspace `Entry` changed, by row index.
    WorkspaceRenamed(usize, String),
    /// The Workspaces page's Add button.
    WorkspaceAdded,
    /// A row's Remove button, by row index.
    WorkspaceRemoved(usize),

    // --- Keybindings (P3) --------------------------------------------------
    /// The Keybindings page's per-row Set button, by action string. Its arm
    /// sets `m.capturing` to that action, superseding any row already pending
    /// capture (contract §2.7).
    CaptureArmed(String),
    /// The window-root capture handler resolved the pressed key and decided
    /// it wasn't Escape (contract §2.7). Its arm applies the combo to
    /// `m.model.working.keybindings` via `apply_capture` while a row is armed,
    /// then disarms the row and recomputes duplicate-binding conflicts.
    KeyCaptured {
        keysym: u32,
        mods: crate::model::CaptureMods,
    },
    /// The window-root capture handler saw Escape while a row was armed. Its
    /// arm clears `m.capturing`.
    CaptureCancelled,

    // --- Displays (P4) -----------------------------------------------------
    /// A press on the Displays canvas, at canvas-space `(x, y)`. Its arm starts
    /// a [`crate::pages::displays::state::Drag`] via `canvas::drag_began` and
    /// repopulates the control panel (contract §2.1's `HeadDragBegan`/
    /// `HeadDragged`/`HeadDragEnded` trio).
    HeadDragBegan(f64, f64),
    /// A pointer motion over the canvas, at canvas-space `(x, y)`.
    HeadDragged(f64, f64),
    /// The pointer released over the canvas, at canvas-space `(x, y)`.
    HeadDragEnded(f64, f64),
    /// The control panel's Enabled switch. Its arm writes the selected head's
    /// `enabled` flag via `pages::displays::controls::set_enabled`.
    HeadEnabledToggled(bool),
    /// The control panel's Resolution dropdown, by index into
    /// `DisplaysState::res_options`.
    ResolutionSelected(usize),
    /// The control panel's Refresh dropdown, by index into
    /// `DisplaysState::refresh_options`.
    RefreshSelected(usize),
    /// The control panel's Scale spinner.
    HeadScaleChanged(f64),
    /// The control panel's Transform dropdown, by index into
    /// `TRANSFORM_VALUES`.
    TransformSelected(usize),
    /// The Displays footer's Test button.
    DisplaysTest,
    /// The Displays footer's Revert button.
    DisplaysRevert,
    /// The Displays footer's Apply button.
    DisplaysApply,
}

const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<Msg>();
};

/// The standing warning for whatever could not start, naming the feature
/// rather than the thread.
fn worker_warning(workers: &crate::ipc::WorkerHandles) -> String {
    let mut missing: Vec<&str> = Vec::new();
    if !workers.reload_available() {
        missing.push("applying changes");
    }
    if !workers.fs_available() {
        missing.push("reverting and wallpaper checks");
    }
    if !workers.portal_available() {
        missing.push("the file chooser");
    }
    if missing.is_empty() {
        return String::new();
    }
    format!(
        "Some background work could not start; {} unavailable",
        missing.join(", ")
    )
}

/// Fold a candidate wallpaper path into the model.
///
/// One code path for the `Entry` and for the portal's answer: the portal is
/// another process and its reply is no more trusted than a typed string.
fn set_wallpaper(m: &mut SettingsModel, text: String) -> Cmd<Msg> {
    m.wallpaper_text = text;
    if m.wallpaper_text.trim().is_empty() {
        m.wallpaper_error = None;
        m.model.working.appearance.wallpaper = None;
        return Cmd::None;
    }
    // The verdict needs a `stat(2)`, and `update` does not block (spec D8):
    // a wallpaper on a network mount stalled the whole window inside the
    // fold, once per keystroke. The check leaves on the filesystem worker
    // and comes back as `Msg::WallpaperValidated`; until it does the field
    // keeps whatever it last said, which is what an `Entry` mid-edit shows
    // anyway.
    let handles = m.workers.clone();
    let text = m.wallpaper_text.clone();
    Cmd::Task(Rc::new(move || handles.validate_wallpaper(text.clone())))
}

/// Fold one message into the model.
///
/// Never blocks and never calls D-Bus: outbound work leaves through
/// `Cmd::Task`, which `App` runs on the loop thread after the fold.
pub fn update(m: &mut SettingsModel, msg: Msg) -> Cmd<Msg> {
    // One line per fold, for the harness gates. A no-op without
    // $ICEDTEA_PROBE_REPORT.
    crate::probe::report(&format!("msg {msg:?}"));
    if clears_status(&msg) {
        m.status.clear();
    }
    let cmd = match msg {
        Msg::PageSelected(index) => {
            m.page = PageId::from_index(index);
            // Leaving the Keybindings page disarms a capture — the GTK
            // `connect_unmap`/`EventControllerFocus` reset, in one line.
            m.capturing = None;
            Cmd::None
        }
        Msg::Apply => {
            let cfg = m.model.working.clone();
            let db_path = m.db_path.clone();
            let workers = m.workers.clone();
            m.status = "Applying\u{2026}".to_string();
            Cmd::Task(Rc::new(move || workers.apply(cfg.clone(), db_path.clone())))
        }
        Msg::Revert => {
            // Reading the store means opening `redb`, which `update` must not
            // do on the loop thread (spec D8): the read leaves on the
            // filesystem worker and comes back as `Msg::ConfigLoaded`.
            let handles = m.workers.clone();
            let db_path = m.db_path.clone();
            Cmd::Task(Rc::new(move || handles.load_config(db_path.clone())))
        }
        Msg::ConfigLoaded(Ok(cfg)) => {
            m.model.working = (*cfg).clone();
            m.model.saved = (*cfg).clone();
            m.wallpaper_text = m
                .model
                .working
                .appearance
                .wallpaper
                .clone()
                .unwrap_or_default();
            m.wallpaper_error = None;
            m.conflicts = crate::model::duplicate_bindings(&m.model.working);
            m.status = String::new();
            // Displays is deliberately not reverted: it owns its own Revert
            // and never routes through this working copy (main.rs:165-174).
            Cmd::None
        }
        Msg::ConfigLoaded(Err(_)) => {
            // The store is present but unreadable (corrupt/IO/locked).
            // `load_reportable` distinguishes this from an absent store, so we
            // must NOT adopt factory defaults here: leave `working` and `saved`
            // untouched (a corrupt read is not a clean revert) and surface the
            // failure instead of clearing the status line.
            m.status = "Could not read your saved settings".to_string();
            Cmd::None
        }
        Msg::Applied { shipped, result } => {
            match result {
                // `saved` is re-baselined from the snapshot the worker really
                // wrote, not from the current working copy: an edit folded
                // while the write was in flight stays dirty, because it is.
                Ok(ReloadOutcome::Reloaded) => {
                    m.model.saved = (*shipped).clone();
                    m.status = "Applied".to_string();
                }
                Ok(ReloadOutcome::CompositorAbsent) => {
                    m.model.saved = (*shipped).clone();
                    m.status = "Saved; will apply when the compositor starts".to_string();
                }
                // The write failed: the working copy stays dirty on purpose.
                Err(err) => m.status = format!("Failed to save: {err}"),
            }
            Cmd::None
        }
        Msg::ConfigReloaded => {
            m.status = "Compositor reloaded its configuration".to_string();
            Cmd::None
        }
        Msg::BarPositionSelected(index) => {
            if let Some(position) = crate::model::BAR_POSITIONS.get(index) {
                m.model.working.appearance.bar_position = (*position).to_string();
            }
            Cmd::None
        }
        Msg::BarHeightChanged(v) => {
            m.model.working.appearance.bar_height = crate::pages::appearance::spin_px(v);
            Cmd::None
        }
        Msg::CornerRadiusChanged(v) => {
            m.model.working.appearance.corner_radius = crate::pages::appearance::spin_px(v);
            Cmd::None
        }
        Msg::ColorPickerOpened(slot) => {
            m.color_picker = Some(slot);
            Cmd::None
        }
        Msg::ColorPickerClosed => {
            m.color_picker = None;
            Cmd::None
        }
        Msg::BackgroundPicked(packed) => {
            m.model.working.appearance.palette.background =
                crate::pages::appearance::packed_to_hex(packed);
            m.color_picker = None;
            Cmd::None
        }
        Msg::ForegroundPicked(packed) => {
            m.model.working.appearance.palette.foreground =
                crate::pages::appearance::packed_to_hex(packed);
            m.color_picker = None;
            Cmd::None
        }
        Msg::AccentPicked(packed) => {
            m.model.working.appearance.palette.accent =
                crate::pages::appearance::packed_to_hex(packed);
            m.color_picker = None;
            Cmd::None
        }
        Msg::SnapGapChanged(v) => {
            m.model.working.appearance.snap_gap = crate::pages::appearance::spin_px(v);
            Cmd::None
        }
        Msg::RaiseOnFocusToggled(on) => {
            m.model.working.behavior.raise_on_focus = on;
            Cmd::None
        }
        Msg::HideBarOnFullscreenToggled(on) => {
            m.model.working.behavior.hide_bar_on_fullscreen = on;
            Cmd::None
        }
        Msg::SnapEnabledToggled(on) => {
            m.model.working.behavior.snap_enabled = on;
            Cmd::None
        }
        Msg::Outputs(update) => {
            // The socket going dead needs one thing `on_outputs` (which only
            // ever touches the model) cannot express: a dead fd is
            // *permanently* readable, so the toolkit reports readiness and
            // never retires a foreign fd itself (`ui/src/window/mod.rs` —
            // "the toolkit never decides on its own that a foreign fd is
            // dead. It reports, and the owner calls `Window::unwatch`"). Left
            // watched, every poll wake would dispatch-error, warn, re-emit
            // `Disconnected` and re-render, forever. `Cmd::Unwatch` (P0-D7)
            // is the way out; the pump also latches shut so the interval
            // before this lands stays quiet. `tests/outputs_pump.rs` gates
            // on this Cmd, so it stays here rather than folding into the
            // pure model fold.
            let unwatch = matches!(&*update, crate::outputs::OutputsMsg::Disconnected)
                .then(|| {
                    m.outputs
                        .as_ref()
                        .map(crate::outputs::pump::OutputsPump::watch)
                })
                .flatten();
            crate::pages::displays::on_outputs(m, update.as_ref());
            match unwatch {
                Some(watch) => Cmd::Unwatch(watch),
                None => Cmd::None,
            }
        }
        Msg::WallpaperEdited(text) => set_wallpaper(m, text),
        Msg::WallpaperChosen(path) => {
            m.browse_in_flight = false;
            set_wallpaper(m, path.display().to_string())
        }
        Msg::WallpaperCleared => set_wallpaper(m, String::new()),
        Msg::WallpaperValidated { text, result } => {
            // A later keystroke has already superseded this answer.
            if text == m.wallpaper_text {
                match result {
                    Ok(path) => {
                        m.wallpaper_error = None;
                        m.model.working.appearance.wallpaper = Some(path);
                    }
                    Err(reason) => m.wallpaper_error = Some(reason),
                }
            }
            Cmd::None
        }
        Msg::WallpaperBrowse => {
            if !m.portal_available {
                m.status = "No file portal available; type a path instead".to_string();
                Cmd::None
            } else if m.browse_in_flight {
                // The chooser is already up. Serving this click after it
                // closes is a second, unprompted dialog nobody asked for --
                // and `parent_window` is "", so neither is modal.
                Cmd::None
            } else {
                m.browse_in_flight = true;
                let handles = m.workers.clone();
                let current = m
                    .model
                    .working
                    .appearance
                    .wallpaper
                    .as_ref()
                    .map(std::path::PathBuf::from);
                Cmd::Task(Rc::new(move || {
                    handles.choose_wallpaper(current.clone());
                }))
            }
        }
        Msg::WallpaperPickerFailed(reason) => {
            m.status = reason;
            m.portal_available = false;
            m.browse_in_flight = false;
            Cmd::None
        }
        // No latch: the portal answered, and answered normally. Greying
        // Browse for the session because someone pressed Escape once is the
        // bug amendment P2-D16 records; contract §2.6's latch text is about
        // a portal that is not there.
        Msg::WallpaperPickerCancelled => {
            m.status = "Wallpaper selection cancelled".to_string();
            m.browse_in_flight = false;
            Cmd::None
        }
        Msg::WorkspaceRenamed(index, text) => {
            pages::workspaces::rename_workspace(&mut m.model.working, index, &text);
            Cmd::None
        }
        Msg::WorkspaceAdded => {
            pages::workspaces::add_workspace(&mut m.model.working);
            Cmd::None
        }
        Msg::WorkspaceRemoved(index) => {
            pages::workspaces::remove_workspace(&mut m.model.working, index);
            Cmd::None
        }
        // --- Keybindings (P3) -------------------------------------------
        Msg::CaptureArmed(action) => {
            // Starting a new capture supersedes any row already pending --
            // the view renders "Press a key…" from `capturing` alone, so the
            // superseded row's button goes back to "Set" on the same frame.
            m.capturing = Some(action);
            Cmd::None
        }
        Msg::CaptureCancelled => {
            m.capturing = None;
            Cmd::None
        }
        // --- Displays (P4) -----------------------------------------------
        Msg::HeadDragBegan(x, y) => {
            crate::pages::displays::canvas::drag_began(&mut m.displays, x, y);
            crate::pages::displays::controls::repopulate(&mut m.displays);
            Cmd::None
        }
        Msg::HeadDragged(x, y) => {
            crate::pages::displays::canvas::dragged(&mut m.displays, x, y);
            Cmd::None
        }
        Msg::HeadDragEnded(x, y) => {
            crate::pages::displays::canvas::drag_ended(&mut m.displays, x, y);
            Cmd::None
        }
        Msg::HeadEnabledToggled(on) => {
            crate::pages::displays::controls::set_enabled(&mut m.displays, on);
            Cmd::None
        }
        Msg::ResolutionSelected(i) => {
            crate::pages::displays::controls::pick_resolution(&mut m.displays, i);
            Cmd::None
        }
        Msg::RefreshSelected(i) => {
            crate::pages::displays::controls::pick_refresh(&mut m.displays, i);
            Cmd::None
        }
        Msg::HeadScaleChanged(v) => {
            crate::pages::displays::controls::set_scale(&mut m.displays, v);
            Cmd::None
        }
        Msg::TransformSelected(i) => {
            crate::pages::displays::controls::pick_transform(&mut m.displays, i);
            Cmd::None
        }
        Msg::DisplaysTest => {
            crate::pages::displays::submit(m, true);
            Cmd::None
        }
        Msg::DisplaysApply => {
            crate::pages::displays::submit(m, false);
            Cmd::None
        }
        Msg::DisplaysRevert => {
            crate::pages::displays::revert(m);
            Cmd::None
        }
        Msg::KeyCaptured { keysym, mods } => {
            // `false` means `combo_from_keysym` declined -- a lone modifier
            // press. Stay armed and wait for the real key (contract §2.7
            // rule 5), the GTK behaviour verbatim. Nothing armed is inert:
            // `capture_key` already declines an unarmed press (deviation
            // P3-D1); this is the second gate, for a message that reached
            // the queue some other way.
            if let Some(action) = m.capturing.clone()
                && pages::keybindings::apply_capture(&mut m.model.working, &action, keysym, mods)
            {
                m.capturing = None;
                m.conflicts = crate::model::duplicate_bindings(&m.model.working);
            }
            Cmd::None
        }
    };
    crate::probe::report(&format!("page {}", m.page.name()));
    crate::probe::report(&format!("status {}", footer_text(m)));
    for line in state_report_lines(m) {
        crate::probe::report(&line);
    }
    cmd
}

/// Whether folding `msg` is a user edit, and so clears the status line.
///
/// The status is what the app has to *say*; the dirty hint is a standing
/// state. Letting dirty win outright (what the GTK `update_footer` closure
/// did, and what this used to copy) meant "Failed to save: …" — which
/// deliberately leaves the model dirty — could never be displayed at all,
/// nor "Applying…", nor the picker's cancelled/unavailable lines. So the
/// status wins while it is fresh, and the user's next edit is what makes it
/// stale. Exhaustive on purpose: a new `Msg` has to decide.
fn clears_status(msg: &Msg) -> bool {
    match msg {
        Msg::BarPositionSelected(_)
        | Msg::BarHeightChanged(_)
        | Msg::CornerRadiusChanged(_)
        | Msg::BackgroundPicked(_)
        | Msg::ForegroundPicked(_)
        | Msg::AccentPicked(_)
        | Msg::SnapGapChanged(_)
        | Msg::RaiseOnFocusToggled(_)
        | Msg::HideBarOnFullscreenToggled(_)
        | Msg::SnapEnabledToggled(_)
        | Msg::WallpaperEdited(_)
        | Msg::WallpaperCleared
        | Msg::WorkspaceRenamed(..)
        | Msg::WorkspaceAdded
        | Msg::WorkspaceRemoved(_)
        | Msg::KeyCaptured { .. } => true,
        Msg::PageSelected(_)
        | Msg::Apply
        | Msg::Revert
        | Msg::Applied { .. }
        | Msg::ConfigLoaded(_)
        | Msg::ConfigReloaded
        | Msg::ColorPickerOpened(_)
        | Msg::ColorPickerClosed
        | Msg::Outputs(_)
        | Msg::WallpaperBrowse
        | Msg::WallpaperChosen(_)
        | Msg::WallpaperValidated { .. }
        | Msg::WallpaperPickerFailed(_)
        | Msg::WallpaperPickerCancelled
        | Msg::CaptureArmed(_)
        | Msg::CaptureCancelled
        | Msg::HeadDragBegan(..)
        | Msg::HeadDragged(..)
        | Msg::HeadDragEnded(..)
        | Msg::HeadEnabledToggled(_)
        | Msg::ResolutionSelected(_)
        | Msg::RefreshSelected(_)
        | Msg::HeadScaleChanged(_)
        | Msg::TransformSelected(_)
        | Msg::DisplaysTest
        | Msg::DisplaysRevert
        | Msg::DisplaysApply => false,
    }
}

/// The `state <key> <value…>` lines the probe report carries (plan P4-D10).
///
/// M5-D9's `probe`/`alloc` lines carry geometry only, and the Displays drag
/// gate has to assert the *model's* rectangle. These four lines are what it
/// reads; they are written by the same once-per-changed-frame writer as the
/// other two kinds.
#[must_use]
pub fn state_report_lines(m: &SettingsModel) -> Vec<String> {
    let position = m
        .displays
        .selected
        .and_then(|i| m.displays.edits.get(i))
        .and_then(|e| e.position)
        .map_or_else(|| "none".to_string(), |(x, y)| format!("{x} {y}"));
    let selected = m
        .displays
        .selected
        .map_or_else(|| "none".to_string(), |i| i.to_string());
    vec![
        format!("state displays.position {position}"),
        format!("state displays.selected {selected}"),
        format!("state displays.dirty {}", m.displays.dirty),
        format!("state displays.in_flight {}", m.displays_in_flight),
    ]
}

/// What the footer's status label shows, in priority order: whatever the app
/// last said, then the standing degraded-worker warning, then the dirty hint.
///
/// The warning outranks "Unsaved changes" only when a **save-path** worker is
/// down (the reload or fs worker), because that is the reason the unsaved
/// changes cannot be saved; no fold clears it — it is not a status line but a
/// fact about this process. A dead *portal* worker (the file chooser) does not
/// affect saving, so it must not mask the dirty hint: when only the portal
/// worker is down, "Unsaved changes" shows, and the portal warning sits below
/// it (shown when the model is clean).
#[must_use]
pub fn footer_text(m: &SettingsModel) -> &str {
    if !m.status.is_empty() {
        return &m.status;
    }
    let save_path_down = !m.workers.reload_available() || !m.workers.fs_available();
    if save_path_down && !m.worker_warning.is_empty() {
        return &m.worker_warning;
    }
    if m.is_dirty() {
        return "Unsaved changes";
    }
    if !m.worker_warning.is_empty() {
        return &m.worker_warning;
    }
    ""
}

/// The whole window, rebuilt from the model on every frame.
#[must_use]
pub fn view(m: &SettingsModel) -> View<Msg> {
    box_(
        Orientation::Vertical,
        [
            nav(m),
            stack([
                stack_page("appearance", "Appearance", pages::appearance::view(m)),
                stack_page("behavior", "Behavior", pages::behavior::view(m)),
                stack_page("workspaces", "Workspaces", pages::workspaces::view(m)),
                stack_page("keybindings", "Keybindings", pages::keybindings::view(m)),
                stack_page("displays", "Displays", pages::displays::view(m)),
            ])
            .visible_child(m.page.name())
            .id("pages")
            .vexpand(true),
            footer(m),
        ],
    )
    .id("root")
    .on_key({
        // Armed from the model each frame (deviation P3-D1): an unarmed
        // handler must decline, or `GenericC::on_event` sets `cx.handled`
        // for every key press and M3's P4-D19 whole-dispatch stop swallows
        // the keystroke a focused `Entry` was waiting for.
        let armed = m.capturing.is_some();
        move |ev| pages::keybindings::capture_key(armed, ev)
    })
}

/// The page switcher. `Stack` + `StackSwitcher`, not `StackSidebar` (spec D7:
/// the sidebar's eviction defect is out of M5's scope).
fn nav(m: &SettingsModel) -> View<Msg> {
    // Without this the switcher builds with `selected = 0` whatever page the
    // stack is showing: `$ICEDTEA_SETTINGS_PAGE=behavior` opened on Behavior
    // with "Appearance" checked, and clicking the lit button was swallowed by
    // `StackSwitcherC::on_event`'s `index == self.selected` early return.
    stack_switcher(pages::page_infos())
        .selected(m.page.index())
        .id("nav")
        .on_selected(Msg::PageSelected)
}

/// Status line plus Revert and Apply. Every value here is computed from the
/// model — there is no dirty flag and no `Rc<dyn Fn()>` to call.
fn footer(m: &SettingsModel) -> View<Msg> {
    let dirty = m.is_dirty();
    box_(
        Orientation::Horizontal,
        [
            label(footer_text(m))
                .id("status")
                .hexpand(true)
                .halign(Align::Start),
            // Dirty is not enough: a worker that never started accepts the
            // request and never answers, so the button would stay live and
            // the footer would park on "Applying…" for the session. Revert
            // reads the store on the filesystem worker; Apply writes it on
            // the reload worker.
            button("Revert")
                .id("revert")
                .sensitive(dirty && m.workers.fs_available())
                .on_click(Msg::Revert),
            button("Apply")
                .id("apply")
                .sensitive(dirty && m.workers.reload_available())
                .on_click(Msg::Apply),
        ],
    )
    .id("footer")
    .margin(8, 8, 8, 8)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::{ColorSlot, Msg, SettingsModel, footer_text, update, view};
    use crate::compositor_reload::ReloadOutcome;
    use crate::pages::PageId;
    use icedtea_ui::view::Cmd;

    /// `Msg` crosses a thread boundary on the inbox (M5-D2), so it must be
    /// `Send` — which is what forbids `Rc` in a payload.
    #[test]
    fn msg_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<Msg>();
    }

    /// A model over a real (empty) config store, plus the worker queues its
    /// `Cmd::Task`s land on. The `TempDir` must be held for the db path to
    /// stay valid.
    fn model() -> (SettingsModel, tempfile::TempDir, TestWorkers) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("config.redb");
        let (workers, reload, portal, fs) = crate::ipc::handles_for_test();
        (
            SettingsModel::new(db, workers),
            dir,
            TestWorkers { reload, portal, fs },
        )
    }

    /// The worker queues a [`test_model`] hands its `Cmd::Task`s to.
    ///
    /// No thread and no D-Bus: `ipc::spawn` (which this used to call) opens a
    /// `ConfigReloaded` subscription on the developer's *live* session bus
    /// and blocking-calls `ReloadConfig` on whatever owns
    /// `org.icedtea.Compositor` there — a unit test that forced a real
    /// compositor to reload (finding 15). `handles_for_test` (P1-D8) is the
    /// sanctioned shape; [`TestWorkers::settle`] plays the queued filesystem
    /// work back by hand, synchronously, so a test can still assert on what
    /// the answer does to the model.
    pub(crate) struct TestWorkers {
        pub reload: crossbeam_channel::Receiver<crate::ipc::reload::ReloadRequest>,
        pub portal: crossbeam_channel::Receiver<crate::ipc::portal::PortalRequest>,
        pub fs: crossbeam_channel::Receiver<crate::ipc::fs::FsRequest>,
    }

    impl TestWorkers {
        /// Run every queued filesystem request the way the worker would, and
        /// fold its answer — the loop's `Cmd::Task` plus inbox round trip,
        /// collapsed into one synchronous call.
        pub fn settle(&self, m: &mut SettingsModel) {
            while let Ok(request) = self.fs.try_recv() {
                let msg = match request {
                    crate::ipc::fs::FsRequest::Shutdown => continue,
                    crate::ipc::fs::FsRequest::ValidateWallpaper { text } => {
                        Msg::WallpaperValidated {
                            text: text.clone(),
                            result: crate::pages::appearance::validate_wallpaper(&text)
                                .map(|path| path.display().to_string()),
                        }
                    }
                    crate::ipc::fs::FsRequest::LoadConfig { db_path } => Msg::ConfigLoaded(
                        icedtea_config::load_reportable(&db_path).map(std::sync::Arc::new),
                    ),
                };
                update(m, msg);
            }
        }
    }

    /// A model with worker *queues* and no window, and the receivers behind
    /// them. The queues must be held: dropping them makes every `Cmd::Task`'s
    /// send fail.
    pub(crate) fn test_model() -> (SettingsModel, TestWorkers) {
        let (workers, reload, portal, fs) = crate::ipc::handles_for_test();
        let cfg = icedtea_config::default_config();
        let model = SettingsModel {
            model: crate::model::Model {
                working: cfg.clone(),
                saved: cfg,
            },
            db_path: std::path::PathBuf::from("/nonexistent/icedtea-test.redb"),
            page: crate::pages::PageId::Appearance,
            status: String::new(),
            worker_warning: String::new(),
            capturing: None,
            conflicts: Vec::new(),
            wallpaper_text: String::new(),
            wallpaper_error: None,
            portal_available: true,
            browse_in_flight: false,
            color_picker: None,
            displays: crate::pages::displays::state::DisplaysState::new(),
            displays_status: String::new(),
            displays_in_flight: false,
            outputs_available: false,
            outputs: None,
            workers,
        };
        (model, TestWorkers { reload, portal, fs })
    }

    #[test]
    fn a_bar_position_pick_writes_the_domain_value_not_the_index() {
        let (mut m, _workers) = test_model();
        update(&mut m, Msg::BarPositionSelected(1));
        assert_eq!(m.model.working.appearance.bar_position, "bottom");
        update(&mut m, Msg::BarPositionSelected(0));
        assert_eq!(m.model.working.appearance.bar_position, "top");
    }

    #[test]
    fn an_out_of_domain_bar_position_index_is_ignored() {
        let (mut m, _workers) = test_model();
        let before = m.model.working.appearance.bar_position.clone();
        update(&mut m, Msg::BarPositionSelected(99));
        assert_eq!(
            m.model.working.appearance.bar_position, before,
            "an index BAR_POSITIONS does not have leaves the model alone"
        );
    }

    #[test]
    fn the_pixel_spin_buttons_write_clamped_integers() {
        let (mut m, _workers) = test_model();
        update(&mut m, Msg::BarHeightChanged(31.6));
        update(&mut m, Msg::CornerRadiusChanged(-4.0));
        assert_eq!(m.model.working.appearance.bar_height, 32);
        assert_eq!(m.model.working.appearance.corner_radius, 0);
        assert!(m.model.is_dirty(), "an edit makes the model dirty");
    }

    #[test]
    fn a_colour_pick_writes_hex_and_closes_the_picker() {
        let (mut m, _workers) = test_model();
        update(&mut m, Msg::ColorPickerOpened(ColorSlot::Accent));
        assert_eq!(m.color_picker, Some(ColorSlot::Accent));
        let packed = crate::pages::appearance::hex_to_packed("#89b4fa");
        update(&mut m, Msg::AccentPicked(packed));
        assert_eq!(m.model.working.appearance.palette.accent, "#89b4fa");
        assert_eq!(m.color_picker, None, "picking closes the picker");
    }

    #[test]
    fn the_three_colour_slots_are_independent() {
        let (mut m, _workers) = test_model();
        update(
            &mut m,
            Msg::BackgroundPicked(crate::pages::appearance::hex_to_packed("#1e1e2e")),
        );
        update(
            &mut m,
            Msg::ForegroundPicked(crate::pages::appearance::hex_to_packed("#cdd6f4")),
        );
        let palette = &m.model.working.appearance.palette;
        assert_eq!(palette.background, "#1e1e2e");
        assert_eq!(palette.foreground, "#cdd6f4");
        assert_eq!(
            palette.accent,
            icedtea_config::default_config().appearance.palette.accent,
            "the untouched slot is untouched"
        );
    }

    #[test]
    fn closing_the_picker_leaves_the_model_alone() {
        let (mut m, _workers) = test_model();
        update(&mut m, Msg::ColorPickerOpened(ColorSlot::Background));
        update(&mut m, Msg::ColorPickerClosed);
        assert_eq!(m.color_picker, None);
        assert!(!m.model.is_dirty(), "opening and closing edits nothing");
    }

    /// Mutation check: make `Msg::PageSelected` ignore its index; this fails
    /// on the `Workspaces` assertion. Restore.
    #[test]
    fn selecting_a_page_moves_the_model_and_cancels_a_capture() {
        let (mut m, _dir, _workers) = model();
        m.capturing = Some("close".to_string());
        update(&mut m, Msg::PageSelected(2));
        assert_eq!(m.page, PageId::Workspaces);
        assert_eq!(m.capturing, None, "a page switch cancels an armed capture");
    }

    /// Dirty is computed from working != saved, never a flag (spec §5.1).
    #[test]
    fn dirty_is_computed_and_revert_clears_it() {
        let (mut m, _dir, workers) = model();
        assert!(!m.is_dirty(), "a freshly loaded model is clean");
        m.model.working.appearance.palette.accent = "#ff00aa".to_string();
        assert!(m.is_dirty());
        // Revert reads `redb`, which `update` must not do on the loop thread
        // (spec D8): it leaves through `Cmd::Task` and comes back as
        // `Msg::ConfigLoaded`, which `settle` plays back here.
        let cmd = update(&mut m, Msg::Revert);
        assert!(
            matches!(cmd, Cmd::Task(_)),
            "the store is read on the worker, never inside update"
        );
        run_task(&cmd);
        workers.settle(&mut m);
        assert!(!m.is_dirty(), "Revert reloads both halves from disk");
        assert_eq!(m.status, "");
    }

    /// The three Apply outcomes are the three `main.rs:136-158` shipped.
    #[test]
    fn the_three_apply_outcomes_set_status_and_the_saved_snapshot() {
        let (mut m, _dir, _workers) = model();
        m.model.working.appearance.palette.accent = "#ff00aa".to_string();

        let shipped = |m: &SettingsModel| std::sync::Arc::new(m.model.working.clone());

        let snapshot = shipped(&m);
        update(
            &mut m,
            Msg::Applied {
                shipped: snapshot,
                result: Ok(ReloadOutcome::Reloaded),
            },
        );
        assert!(
            !m.is_dirty(),
            "a reload snapshots what was written into saved"
        );
        assert_eq!(m.status, "Applied");

        m.model.working.appearance.palette.accent = "#00ff00".to_string();
        let snapshot = shipped(&m);
        update(
            &mut m,
            Msg::Applied {
                shipped: snapshot,
                result: Ok(ReloadOutcome::CompositorAbsent),
            },
        );
        assert!(!m.is_dirty());
        assert_eq!(m.status, "Saved; will apply when the compositor starts");

        m.model.working.appearance.palette.accent = "#0000ff".to_string();
        let snapshot = shipped(&m);
        update(
            &mut m,
            Msg::Applied {
                shipped: snapshot,
                result: Err("disk on fire".to_string()),
            },
        );
        assert!(m.is_dirty(), "a failed write leaves the edit unsaved");
        assert_eq!(m.status, "Failed to save: disk on fire");
        assert_eq!(
            footer_text(&m),
            "Failed to save: disk on fire",
            "a failure the model stays dirty for must still be readable"
        );
    }

    /// An edit made while an Apply is in flight is still unsaved when the
    /// answer lands: `saved` is re-baselined from the snapshot the worker
    /// really wrote, never from the working copy as it is by then.
    ///
    /// Mutation check: `m.model.saved = m.model.working.clone()` in the
    /// `Applied` arm again (what it did) and both assertions fail — the app
    /// reports "Applied" over an edit that never reached disk.
    #[test]
    fn an_edit_folded_during_an_apply_is_not_marked_saved() {
        let (mut m, _dir, workers) = model();
        m.model.working.appearance.palette.accent = "#ff00aa".to_string();

        let cmd = update(&mut m, Msg::Apply);
        run_task(&cmd);
        let shipped = match workers.reload.try_recv() {
            Ok(crate::ipc::reload::ReloadRequest::Apply { cfg, .. }) => std::sync::Arc::from(cfg),
            other => panic!("expected a queued Apply, got one: {}", other.is_ok()),
        };

        // The user toggles something while the worker is inside its write.
        let before = m.model.working.behavior.raise_on_focus;
        update(&mut m, Msg::RaiseOnFocusToggled(!before));

        update(
            &mut m,
            Msg::Applied {
                shipped,
                result: Ok(ReloadOutcome::Reloaded),
            },
        );
        assert_eq!(
            m.model.saved.appearance.palette.accent, "#ff00aa",
            "what was written is what is baselined"
        );
        assert!(
            m.is_dirty(),
            "the toggle folded during the write never reached disk, so it is unsaved"
        );
    }

    /// The status line is what the app has to say, and it outranks the
    /// standing dirty hint until the user's next edit makes it stale.
    ///
    /// Mutation check: give `footer_text` its old "dirty wins outright" body
    /// and the first assertion fails — which is how "Failed to save: …" (an
    /// outcome that deliberately keeps the model dirty), "Applying…" and
    /// every picker message were unreadable, in the window *and* in the probe
    /// report the gates read.
    #[test]
    fn a_status_outranks_the_dirty_hint_until_the_next_edit() {
        let (mut m, _dir, _workers) = model();
        m.model.working.appearance.palette.accent = "#ff00aa".to_string();
        update(&mut m, Msg::Apply);
        assert!(m.is_dirty());
        assert_eq!(footer_text(&m), "Applying\u{2026}");

        update(&mut m, Msg::CornerRadiusChanged(4.0));
        assert_eq!(
            footer_text(&m),
            "Unsaved changes",
            "the next edit is what makes a status stale"
        );
    }

    /// A second Browse click while a chooser is already open is dropped, not
    /// queued: served afterwards it is a second dialog nobody asked for.
    ///
    /// Mutation check: drop the `browse_in_flight` guard and the second click
    /// returns a `Cmd::Task` too. Restore.
    #[test]
    fn a_browse_while_the_chooser_is_open_is_not_a_second_dialog() {
        let (mut m, _dir, workers) = model();
        let first = update(&mut m, Msg::WallpaperBrowse);
        assert!(matches!(first, Cmd::Task(_)));
        run_task(&first);
        assert!(m.browse_in_flight);
        assert!(
            matches!(update(&mut m, Msg::WallpaperBrowse), Cmd::None),
            "the latch holds while a chooser is up"
        );
        assert!(workers.portal.try_recv().is_ok(), "one request went out");
        assert!(
            workers.portal.try_recv().is_err(),
            "and only one: a second dialog is never queued behind the first"
        );

        update(&mut m, Msg::WallpaperPickerCancelled);
        assert!(!m.browse_in_flight, "an answer releases the latch");
        assert!(matches!(update(&mut m, Msg::WallpaperBrowse), Cmd::Task(_)));
    }

    /// A worker that could not start is said so, rather than leaving live
    /// controls whose clicks nothing will ever answer.
    ///
    /// Mutation check: seed `status` with `String::new()` unconditionally
    /// again and this fails.
    #[test]
    fn a_worker_that_never_started_is_reported_in_the_footer() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut m = SettingsModel::new(
            dir.path().join("config.redb"),
            crate::ipc::dead_handles_for_test(),
        );
        assert!(
            footer_text(&m).contains("could not start"),
            "got {:?}",
            footer_text(&m)
        );
        assert!(
            !m.portal_available,
            "and Browse is insensitive rather than inert"
        );

        // The warning must survive the fold that makes Apply *matter*.
        // Seeding `status` with it (what this replaced) meant the first edit
        // — which is exactly what makes the model dirty — cleared it.
        //
        // Mutation check: seed `m.status` from `worker_warning` in
        // `SettingsModel::new` again and leave `footer_text` reading only
        // `status`; this assertion fails with "Unsaved changes".
        update(&mut m, Msg::CornerRadiusChanged(4.0));
        assert!(m.is_dirty());
        assert!(
            footer_text(&m).contains("could not start"),
            "the warning outlives the edit that makes it matter, got {:?}",
            footer_text(&m)
        );
    }

    /// Apply and Revert are insensitive when the worker that would serve them
    /// never started, however dirty the model is: a click on either accepts a
    /// request nothing will ever answer and parks the footer on "Applying…"
    /// for the session.
    ///
    /// Mutation check: put `.sensitive(dirty)` back on either button and its
    /// assertion here fails.
    #[test]
    fn a_dead_worker_leaves_its_button_insensitive_however_dirty_the_model_is() {
        use icedtea_ui::css::node::PseudoStates;

        let dir = tempfile::tempdir().expect("tempdir");
        let mut m = SettingsModel::new(
            dir.path().join("config.redb"),
            crate::ipc::dead_handles_for_test(),
        );
        m.model.working.appearance.palette.accent = "#ff00aa".to_string();
        assert!(m.is_dirty(), "the precondition the buttons key off");

        let probe = probe_of(m);
        let disabled = |id: &str| {
            probe
                .root()
                .descendants()
                .find(|node| node.id().is_some_and(|found| found.as_str() == id))
                .unwrap_or_else(|| panic!("#{id} is in the tree"))
                .states()
                .contains(PseudoStates::DISABLED)
        };
        assert!(disabled("apply"), "Apply needs the reload worker");
        assert!(disabled("revert"), "Revert needs the filesystem worker");
        assert!(
            disabled("appearance_wallpaper_browse"),
            "Browse needs the portal worker"
        );
    }

    /// The page switcher is told which page the stack is showing.
    ///
    /// Mutation check: drop `nav`'s `.selected(m.page.index())` and the
    /// switcher checks "Appearance" while Behavior is on screen — and
    /// clicking the lit button is then swallowed by `StackSwitcherC::
    /// on_event`'s `index == self.selected` early return.
    #[test]
    fn the_switcher_checks_the_page_the_stack_is_showing() {
        let (mut m, _dir, _workers) = model();
        m.page = PageId::Behavior;
        let index = m.page.index();
        let probe = probe_of(m);

        let nav = probe
            .root()
            .descendants()
            .find(|node| node.id().is_some_and(|id| id.as_str() == "nav"))
            .expect("the switcher is in the tree");
        let checked: Vec<usize> = nav
            .children()
            .into_iter()
            .enumerate()
            .filter(|(_, button)| {
                button
                    .states()
                    .contains(icedtea_ui::css::node::PseudoStates::CHECKED)
            })
            .map(|(i, _)| i)
            .collect();
        assert_eq!(checked, vec![index], "exactly the shown page is checked");
    }

    /// `update` never blocks and never calls D-Bus: Apply hands the work to
    /// the worker through `Cmd::Task` (spec D8).
    ///
    /// Mutation check: make the `Apply` arm return `Cmd::None`; this fails.
    #[test]
    fn apply_queues_a_task_and_says_so() {
        let (mut m, _dir, _workers) = model();
        m.model.working.appearance.palette.accent = "#ff00aa".to_string();
        let cmd = update(&mut m, Msg::Apply);
        assert!(
            matches!(cmd, icedtea_ui::view::Cmd::Task(_)),
            "Apply must go out through Cmd::Task, never inside update"
        );
        assert_eq!(m.status, "Applying\u{2026}");
    }

    #[test]
    fn a_config_reloaded_signal_only_notes_itself() {
        let (mut m, _dir, _workers) = model();
        m.model.working.appearance.palette.accent = "#ff00aa".to_string();
        update(&mut m, Msg::ConfigReloaded);
        assert!(
            m.is_dirty(),
            "an external reload must not touch the working copy"
        );
        assert_eq!(m.status, "Compositor reloaded its configuration");
    }

    /// Run a `Cmd::Task`'s closure, the way `App::drain` does after the fold.
    fn run_task(cmd: &Cmd<Msg>) {
        match cmd {
            Cmd::Task(task) => task(),
            other => panic!("expected a Cmd::Task, got {other:?}"),
        }
    }

    /// Every node the tree lays out, by id.
    fn node_ids(probe: &icedtea_ui::view::app::Probe<Msg>) -> Vec<String> {
        let mut out = Vec::new();
        for node in probe.root().descendants() {
            if let Some(id) = node.id() {
                out.push(id.as_str().to_string());
            }
        }
        out
    }

    fn probe_of(m: SettingsModel) -> icedtea_ui::view::app::Probe<Msg> {
        icedtea_ui::view::App::new(m, update, view)
            .probe(
                (480, 420),
                icedtea_ui::app::compile_theme(&icedtea_ui::app::ThemeSource::Bundled),
                icedtea_ui::text::FontDatabase::probe_only(),
                icedtea_ui::icons::IconTheme::with_name_and_roots("hicolor", vec![]),
                std::rc::Rc::new(icedtea_ui::anim::ManualClock::new()),
            )
            .expect("the settings tree lays out")
    }

    /// The frame every page hangs off. Mutation check: drop `footer(m)` from
    /// `view`; this test fails on `apply`. Restore.
    #[test]
    fn the_root_carries_the_nav_the_stack_and_the_footer() {
        let (m, _dir, _workers) = model();
        let probe = probe_of(m);
        let ids = node_ids(&probe);
        for wanted in [
            "root", "nav", "pages", "footer", "status", "revert", "apply",
        ] {
            assert!(
                ids.contains(&wanted.to_string()),
                "missing #{wanted} in {ids:?}"
            );
        }
    }

    /// Only the selected page's body is in the tree the stack shows, and the
    /// selection follows the model.
    #[test]
    fn the_stack_shows_the_selected_page() {
        let (m, _dir, _workers) = model();
        assert_eq!(m.page, PageId::Appearance);
        let ids = node_ids(&probe_of(m));
        assert!(ids.contains(&"appearance".to_string()));

        let (mut m2, _dir2, _workers2) = model();
        m2.page = PageId::Displays;
        let ids = node_ids(&probe_of(m2));
        assert!(ids.contains(&"displays".to_string()));
    }

    /// The footer is computed, not pushed: `Unsaved changes` is what
    /// `is_dirty()` says, and Revert/Apply are insensitive while clean —
    /// `main.rs:73-84`'s `update_footer` closure, with the Rc<dyn Fn()> gone.
    ///
    /// Mutation check: make `footer` always render `m.status`; this fails.
    #[test]
    fn the_footer_reports_dirtiness_without_a_flag() {
        let (mut m, _dir, _workers) = model();
        let clean = footer_text(&m);
        assert_eq!(clean, "");
        m.model.working.appearance.palette.accent = "#ff00aa".to_string();
        assert_eq!(footer_text(&m), "Unsaved changes");
        m.status = "Applied".to_string();
        m.model.saved = m.model.working.clone();
        assert_eq!(footer_text(&m), "Applied");
    }

    use crate::outputs::{Head, Mode, OutputsMsg};
    use std::sync::Arc;

    fn head(name: &str) -> Head {
        let mode = Mode {
            width: 1920,
            height: 1080,
            refresh_mhz: 60_000,
            preferred: true,
        };
        Head {
            name: name.to_string(),
            description: format!("{name} test head"),
            enabled: true,
            modes: vec![mode],
            current_mode: Some(mode),
            x: 0,
            y: 0,
            scale: 1.0,
            transform: 0,
        }
    }

    /// The glib source's `match` on OutputsMsg becomes six `update` arms
    /// (contract §2.5). Mutation check: drop the `outputs_available = true`
    /// in the `HeadsChanged` arm; this test fails. Restore.
    #[test]
    fn heads_changed_reconciles_and_marks_output_management_available() {
        let (mut m, _dir, _workers) = model();
        update(
            &mut m,
            Msg::Outputs(Arc::new(OutputsMsg::HeadsChanged(vec![head("DP-1")]))),
        );
        assert!(m.outputs_available);
        assert_eq!(m.displays.heads.len(), 1);
        assert_eq!(m.displays.edits.len(), 1);
        assert_eq!(m.displays.selected, Some(0));
    }

    /// Neither arm can unwatch anything with no pump attached — the id lives
    /// on the pump. `Disconnected`'s `Cmd::Unwatch` is gated on a real pump in
    /// `tests/outputs_pump.rs`.
    #[test]
    fn losing_the_manager_or_the_connection_takes_the_page_out_of_service() {
        for msg in [OutputsMsg::ManagerUnavailable, OutputsMsg::Disconnected] {
            let (mut m, _dir, _workers) = model();
            m.outputs_available = true;
            m.displays_in_flight = true;
            let cmd = update(&mut m, Msg::Outputs(Arc::new(msg)));
            assert!(!m.outputs_available);
            assert!(
                !m.displays_in_flight,
                "an in-flight request can never complete now"
            );
            assert!(matches!(cmd, icedtea_ui::view::Cmd::None));
        }
    }

    /// A reply's own `is_test` tag decides what it means — never a shared
    /// in-flight flag, which an overlapped Test/Apply pair would misread.
    #[test]
    fn an_apply_reply_clears_in_flight_by_its_own_is_test_tag() {
        let (mut m, _dir, _workers) = model();
        m.displays.dirty = true;
        m.displays_in_flight = true;
        update(
            &mut m,
            Msg::Outputs(Arc::new(OutputsMsg::ApplySucceeded { is_test: true })),
        );
        assert!(!m.displays_in_flight);
        assert!(
            m.displays.dirty,
            "a successful *test* keeps the pending edits"
        );
        assert_eq!(m.displays_status, "Test succeeded");

        m.displays_in_flight = true;
        update(
            &mut m,
            Msg::Outputs(Arc::new(OutputsMsg::ApplySucceeded { is_test: false })),
        );
        assert!(!m.displays_in_flight);
        assert!(
            !m.displays.dirty,
            "a successful apply is no longer pending work"
        );
        assert_eq!(m.displays_status, "Applied");
    }

    fn a_real_image(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, b"pixels").expect("write the image");
        path
    }

    #[test]
    fn a_valid_typed_path_reaches_the_model_and_clears_the_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let image = a_real_image(dir.path(), "wall.png");
        let (mut m, workers) = test_model();
        m.wallpaper_error = Some("stale".to_string());

        // The `stat(2)` runs on the filesystem worker (spec D8), so the
        // verdict lands as a second message; `settle` is that round trip.
        let cmd = update(&mut m, Msg::WallpaperEdited(image.display().to_string()));
        run_task(&cmd);
        workers.settle(&mut m);

        assert_eq!(m.wallpaper_text, image.display().to_string());
        assert_eq!(m.wallpaper_error, None);
        assert_eq!(
            m.model.working.appearance.wallpaper,
            Some(image.display().to_string())
        );
    }

    #[test]
    fn an_invalid_typed_path_shows_an_error_and_never_touches_the_model() {
        let (mut m, workers) = test_model();
        let before = m.model.working.appearance.wallpaper.clone();

        let cmd = update(
            &mut m,
            Msg::WallpaperEdited("/nonexistent/icedtea/wall.png".to_string()),
        );
        run_task(&cmd);
        workers.settle(&mut m);

        assert_eq!(m.wallpaper_text, "/nonexistent/icedtea/wall.png");
        assert!(m.wallpaper_error.is_some(), "the field shows why");
        assert_eq!(
            m.model.working.appearance.wallpaper, before,
            "an invalid path is never written to the working copy"
        );
    }

    #[test]
    fn clearing_the_field_clears_the_wallpaper() {
        let dir = tempfile::tempdir().expect("tempdir");
        let image = a_real_image(dir.path(), "wall.png");
        let (mut m, workers) = test_model();
        let cmd = update(&mut m, Msg::WallpaperEdited(image.display().to_string()));
        run_task(&cmd);
        workers.settle(&mut m);

        update(&mut m, Msg::WallpaperEdited("   ".to_string()));
        workers.settle(&mut m);
        assert_eq!(m.model.working.appearance.wallpaper, None);
        assert_eq!(m.wallpaper_error, None, "empty is not an error");

        let cmd = update(&mut m, Msg::WallpaperEdited(image.display().to_string()));
        run_task(&cmd);
        workers.settle(&mut m);
        update(&mut m, Msg::WallpaperCleared);
        workers.settle(&mut m);
        assert!(m.wallpaper_text.is_empty());
        assert_eq!(m.model.working.appearance.wallpaper, None);
        assert_eq!(m.wallpaper_error, None);
    }

    #[test]
    fn a_portal_answer_goes_through_the_same_validation() {
        let (mut m, workers) = test_model();
        let cmd = update(
            &mut m,
            Msg::WallpaperChosen(std::path::PathBuf::from("/nonexistent/portal/wall.png")),
        );
        run_task(&cmd);
        workers.settle(&mut m);
        assert!(
            m.wallpaper_error.is_some(),
            "the portal is not trusted more than the keyboard"
        );
        assert_eq!(m.model.working.appearance.wallpaper, None);
    }

    #[test]
    fn a_failed_picker_disables_browse_without_touching_the_model() {
        let (mut m, _workers) = test_model();
        let before = m.model.working.clone();

        update(
            &mut m,
            Msg::WallpaperPickerFailed("No file portal available".to_string()),
        );

        assert!(!m.portal_available, "Browse is greyed for the session");
        assert_eq!(m.status, "No file portal available");
        assert_eq!(m.model.working, before, "the working copy is untouched");
        assert!(!m.model.is_dirty());
    }

    /// Amendment P2-D16: Escape in a working chooser is not a portal failure.
    ///
    /// Mutation check: give `Msg::WallpaperPickerCancelled` the same body as
    /// `WallpaperPickerFailed` (what the code did before the amendment, via
    /// `response_to_outcome`'s `Err`) and the `portal_available` assertion
    /// fails. Restore.
    #[test]
    fn a_cancelled_picker_says_so_and_leaves_browse_live() {
        let (mut m, _workers) = test_model();
        let before = m.model.working.clone();

        update(&mut m, Msg::WallpaperPickerCancelled);

        assert!(
            m.portal_available,
            "cancelling once must not grey Browse for the session"
        );
        assert_eq!(m.status, "Wallpaper selection cancelled");
        assert_eq!(m.model.working, before, "the working copy is untouched");
        assert!(!m.model.is_dirty());
    }

    #[test]
    fn browsing_without_a_portal_is_a_status_line_not_a_task() {
        let (mut m, _workers) = test_model();
        m.portal_available = false;
        let cmd = update(&mut m, Msg::WallpaperBrowse);
        assert!(
            matches!(cmd, Cmd::None),
            "no Cmd::Task is issued once the portal is known missing"
        );
        assert!(!m.status.is_empty());
    }

    #[test]
    fn browsing_with_a_portal_issues_a_task() {
        let (mut m, _workers) = test_model();
        let cmd = update(&mut m, Msg::WallpaperBrowse);
        assert!(
            matches!(cmd, Cmd::Task(_)),
            "the outbound call runs on the worker, never inside update"
        );
    }

    #[test]
    fn each_behavior_switch_writes_its_own_field() {
        let (mut m, _workers) = test_model();
        let defaults = icedtea_config::default_config().behavior;

        update(&mut m, Msg::RaiseOnFocusToggled(!defaults.raise_on_focus));
        assert_eq!(
            m.model.working.behavior.raise_on_focus,
            !defaults.raise_on_focus
        );
        assert_eq!(
            m.model.working.behavior.hide_bar_on_fullscreen, defaults.hide_bar_on_fullscreen,
            "the other two switches are untouched"
        );
        assert_eq!(m.model.working.behavior.snap_enabled, defaults.snap_enabled);

        update(
            &mut m,
            Msg::HideBarOnFullscreenToggled(!defaults.hide_bar_on_fullscreen),
        );
        update(&mut m, Msg::SnapEnabledToggled(!defaults.snap_enabled));
        assert_eq!(
            m.model.working.behavior.hide_bar_on_fullscreen,
            !defaults.hide_bar_on_fullscreen
        );
        assert_eq!(
            m.model.working.behavior.snap_enabled,
            !defaults.snap_enabled
        );
    }

    #[test]
    fn toggling_a_switch_makes_the_model_dirty_and_toggling_back_makes_it_clean() {
        let (mut m, _workers) = test_model();
        let before = m.model.working.behavior.raise_on_focus;
        update(&mut m, Msg::RaiseOnFocusToggled(!before));
        assert!(m.model.is_dirty());
        update(&mut m, Msg::RaiseOnFocusToggled(before));
        assert!(
            !m.model.is_dirty(),
            "dirty is computed from working != saved, never a latched flag"
        );
    }

    /// Mutation check: drop the `displays.position` line from
    /// `state_report_lines`; the drag interaction gate (Task 11) has nothing
    /// to read and this fails. Restore.
    ///
    /// Reconciliation (Task 8): the brief's `SettingsModel::for_test()` is not
    /// a name this crate exposes; `test_model()` (P1-D8's `pub(crate)`
    /// constructor, already used throughout this module and by
    /// `pages::displays`'s own tests) is the equivalent it does, returning the
    /// model alongside the worker queues.
    #[test]
    fn the_state_report_lines_carry_the_displays_model() {
        let (mut m, _workers) = test_model();
        m.displays.heads = Vec::new();
        m.displays.edits = Vec::new();
        m.displays.selected = None;
        let none = super::state_report_lines(&m);
        assert!(none.iter().any(|l| l == "state displays.selected none"));
        assert!(none.iter().any(|l| l == "state displays.dirty false"));
        assert!(none.iter().any(|l| l == "state displays.in_flight false"));
        assert!(
            none.iter().any(|l| l == "state displays.position none"),
            "no selection reports `none`, not a coordinate; got {none:?}"
        );

        let head = crate::outputs::Head {
            name: "DP-1".to_string(),
            description: "DP-1".to_string(),
            enabled: true,
            modes: Vec::new(),
            current_mode: None,
            x: 0,
            y: 0,
            scale: 1.0,
            transform: 0,
        };
        m.displays.edits = vec![crate::pages::displays::state::baseline_edit(&head)];
        m.displays.heads = vec![head];
        m.displays.selected = Some(0);
        m.displays.edits[0].position = Some((1920, -180));
        m.displays.dirty = true;
        let lines = super::state_report_lines(&m);
        assert!(
            lines
                .iter()
                .any(|l| l == "state displays.position 1920 -180")
        );
        assert!(lines.iter().any(|l| l == "state displays.selected 0"));
        assert!(lines.iter().any(|l| l == "state displays.dirty true"));
    }

    #[test]
    fn the_snap_gap_spin_writes_appearance_snap_gap_clamped() {
        let (mut m, _workers) = test_model();
        update(&mut m, Msg::SnapGapChanged(11.5));
        assert_eq!(
            m.model.working.appearance.snap_gap, 12,
            "snap_gap lives on Appearance in the config even though the control is on Behavior"
        );
        update(&mut m, Msg::SnapGapChanged(-1.0));
        assert_eq!(m.model.working.appearance.snap_gap, 0);
    }
}
