//! The settings application: the model the loop folds, its messages, and
//! `update`.
//!
//! `view` lives here too (see below); every page contributes one `pub fn
//! view(&SettingsModel) -> View<Msg>` from its own module.

use std::rc::Rc;

use icedtea_ui::layout::Align;
use icedtea_ui::view::builders::{
    StackExt, box_, button, label, stack, stack_page, stack_switcher,
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
pub struct SettingsModel {
    pub model: crate::model::Model,
    pub db_path: std::path::PathBuf,
    pub page: PageId,
    pub status: String,
    /// The action whose binding is being captured, if any (P3 arms it).
    pub capturing: Option<String>,
    /// Conflicting bindings, recomputed from `working` after every edit.
    pub conflicts: Vec<(icedtea_config::KeyCombo, Vec<String>)>,
    pub wallpaper_text: String,
    pub wallpaper_error: Option<String>,
    pub portal_available: bool,
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
            status: String::new(),
            capturing: None,
            conflicts,
            wallpaper_text,
            wallpaper_error: None,
            portal_available: true,
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
    Applied(Result<ReloadOutcome, String>),
    /// The compositor emitted `ConfigReloaded`.
    ConfigReloaded,
    /// One protocol message from the outputs connection, via `App::on_fd`.
    /// `Arc`, not `Rc`: `Msg` is `Send` (M5-D2).
    Outputs(std::sync::Arc<crate::outputs::OutputsMsg>),
    /// The portal worker's file chooser returned a path (contract §2.2).
    WallpaperChosen(std::path::PathBuf),
    /// The portal worker could not produce a path; the status line shows why
    /// (contract §2.2).
    WallpaperPickerFailed(String),
}

const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<Msg>();
};

/// Fold one message into the model.
///
/// Never blocks and never calls D-Bus: outbound work leaves through
/// `Cmd::Task`, which `App` runs on the loop thread after the fold.
pub fn update(m: &mut SettingsModel, msg: Msg) -> Cmd<Msg> {
    // One line per fold, for the harness gates. A no-op without
    // $ICEDTEA_PROBE_REPORT.
    crate::probe::report(&format!("msg {msg:?}"));
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
            m.model.revert(&m.db_path);
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
        Msg::Applied(Ok(ReloadOutcome::Reloaded)) => {
            m.model.saved = m.model.working.clone();
            m.status = "Applied".to_string();
            Cmd::None
        }
        Msg::Applied(Ok(ReloadOutcome::CompositorAbsent)) => {
            m.model.saved = m.model.working.clone();
            m.status = "Saved; will apply when the compositor starts".to_string();
            Cmd::None
        }
        Msg::Applied(Err(err)) => {
            // The write failed: the working copy stays dirty on purpose.
            m.status = format!("Failed to save: {err}");
            Cmd::None
        }
        Msg::ConfigReloaded => {
            m.status = "Compositor reloaded its configuration".to_string();
            Cmd::None
        }
        Msg::Outputs(update) => match &*update {
            crate::outputs::OutputsMsg::HeadsChanged(heads) => {
                let r = crate::pages::displays::state::reconcile(
                    &m.displays.heads,
                    &m.displays.edits,
                    m.displays.selected,
                    m.displays.dirty,
                    heads,
                );
                m.displays.heads = heads.clone();
                m.displays.edits = r.edits;
                m.displays.selected = r.selected;
                // A drag was indexed against the *old* head list; the new
                // one may be shorter or reordered (finding #2).
                m.displays.drag = None;
                if !r.compatible {
                    // A genuine set change re-baselines, so nothing is
                    // unsaved any more.
                    m.displays.dirty = false;
                }
                m.outputs_available = true;
                m.displays_status = if r.dropped {
                    "Displays changed \u{2014} pending edits discarded".to_string()
                } else {
                    String::new()
                };
                Cmd::None
            }
            crate::outputs::OutputsMsg::ApplySucceeded { is_test } => {
                m.displays_in_flight = false;
                if *is_test {
                    // A preview succeeded: keep the edits so the user can
                    // commit them.
                    m.displays_status = "Test succeeded".to_string();
                } else {
                    m.displays.dirty = false;
                    m.displays_status = "Applied".to_string();
                }
                Cmd::None
            }
            crate::outputs::OutputsMsg::ApplyFailed { is_test } => {
                m.displays_in_flight = false;
                if *is_test {
                    m.displays_status = "Test rejected by the compositor".to_string();
                } else {
                    // Re-baseline: the compositor kept its own layout.
                    m.displays.edits = m
                        .displays
                        .heads
                        .iter()
                        .map(crate::pages::displays::state::baseline_edit)
                        .collect();
                    m.displays.dirty = false;
                    m.displays_status = "Configuration rejected by the compositor".to_string();
                }
                Cmd::None
            }
            crate::outputs::OutputsMsg::ApplyCancelled => {
                m.displays_in_flight = false;
                m.displays_status = "Configuration superseded \u{2014} re-reading".to_string();
                Cmd::None
            }
            crate::outputs::OutputsMsg::ManagerUnavailable => {
                // The connection is alive; only the global is missing.
                // Keep watching the fd — nothing is spinning.
                m.displays_in_flight = false;
                m.outputs_available = false;
                m.displays_status = String::new();
                Cmd::None
            }
            crate::outputs::OutputsMsg::Disconnected => {
                m.displays_in_flight = false;
                m.outputs_available = false;
                m.displays_status = String::new();
                // The socket is dead, and a dead fd is *permanently*
                // readable: the toolkit reports readiness and never
                // retires a foreign fd itself (`ui/src/window/mod.rs` —
                // "the toolkit never decides on its own that a foreign fd
                // is dead. It reports, and the owner calls
                // `Window::unwatch`"). Left watched, every poll wake would
                // dispatch-error, warn, re-emit `Disconnected` and
                // re-render, forever. `Cmd::Unwatch` (P0-D7) is the way
                // out; the pump also latches shut so the interval before
                // this lands stays quiet.
                match m
                    .outputs
                    .as_ref()
                    .map(crate::outputs::pump::OutputsPump::watch)
                {
                    Some(watch) => Cmd::Unwatch(watch),
                    None => Cmd::None,
                }
            }
        },
        Msg::WallpaperChosen(path) => {
            m.wallpaper_text = path.display().to_string();
            m.wallpaper_error = None;
            Cmd::None
        }
        Msg::WallpaperPickerFailed(reason) => {
            m.status = reason;
            Cmd::None
        }
    };
    crate::probe::report(&format!("page {}", m.page.name()));
    crate::probe::report(&format!("status {}", footer_text(m)));
    cmd
}

/// What the footer's status label shows: the dirty indicator wins over the
/// last status line, exactly as the GTK `update_footer` closure did.
#[must_use]
pub fn footer_text(m: &SettingsModel) -> &str {
    if m.is_dirty() {
        "Unsaved changes"
    } else {
        &m.status
    }
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
}

/// The page switcher. `Stack` + `StackSwitcher`, not `StackSidebar` (spec D7:
/// the sidebar's eviction defect is out of M5's scope).
fn nav(m: &SettingsModel) -> View<Msg> {
    let _ = m;
    stack_switcher(pages::page_infos())
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
            button("Revert")
                .id("revert")
                .sensitive(dirty)
                .on_click(Msg::Revert),
            button("Apply")
                .id("apply")
                .sensitive(dirty)
                .on_click(Msg::Apply),
        ],
    )
    .id("footer")
    .margin(8, 8, 8, 8)
}

#[cfg(test)]
mod tests {
    use super::{Msg, SettingsModel, footer_text, update, view};
    use crate::compositor_reload::ReloadOutcome;
    use crate::pages::PageId;

    /// `Msg` crosses a thread boundary on the inbox (M5-D2), so it must be
    /// `Send` — which is what forbids `Rc` in a payload.
    #[test]
    fn msg_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<Msg>();
    }

    fn model() -> (SettingsModel, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("config.redb");
        let (workers, _rx) = crate::ipc::handles_for_test();
        (SettingsModel::new(db, workers), dir)
    }

    /// Mutation check: make `Msg::PageSelected` ignore its index; this fails
    /// on the `Workspaces` assertion. Restore.
    #[test]
    fn selecting_a_page_moves_the_model_and_cancels_a_capture() {
        let (mut m, _dir) = model();
        m.capturing = Some("close".to_string());
        update(&mut m, Msg::PageSelected(2));
        assert_eq!(m.page, PageId::Workspaces);
        assert_eq!(m.capturing, None, "a page switch cancels an armed capture");
    }

    /// Dirty is computed from working != saved, never a flag (spec §5.1).
    #[test]
    fn dirty_is_computed_and_revert_clears_it() {
        let (mut m, _dir) = model();
        assert!(!m.is_dirty(), "a freshly loaded model is clean");
        m.model.working.appearance.palette.accent = "#ff00aa".to_string();
        assert!(m.is_dirty());
        update(&mut m, Msg::Revert);
        assert!(!m.is_dirty(), "Revert reloads both halves from disk");
        assert_eq!(m.status, "");
    }

    /// The three Apply outcomes are the three `main.rs:136-158` shipped.
    #[test]
    fn the_three_apply_outcomes_set_status_and_the_saved_snapshot() {
        let (mut m, _dir) = model();
        m.model.working.appearance.palette.accent = "#ff00aa".to_string();

        update(&mut m, Msg::Applied(Ok(ReloadOutcome::Reloaded)));
        assert!(!m.is_dirty(), "a reload snapshots working into saved");
        assert_eq!(m.status, "Applied");

        m.model.working.appearance.palette.accent = "#00ff00".to_string();
        update(&mut m, Msg::Applied(Ok(ReloadOutcome::CompositorAbsent)));
        assert!(!m.is_dirty());
        assert_eq!(m.status, "Saved; will apply when the compositor starts");

        m.model.working.appearance.palette.accent = "#0000ff".to_string();
        update(&mut m, Msg::Applied(Err("disk on fire".to_string())));
        assert!(m.is_dirty(), "a failed write leaves the edit unsaved");
        assert_eq!(m.status, "Failed to save: disk on fire");
    }

    /// `update` never blocks and never calls D-Bus: Apply hands the work to
    /// the worker through `Cmd::Task` (spec D8).
    ///
    /// Mutation check: make the `Apply` arm return `Cmd::None`; this fails.
    #[test]
    fn apply_queues_a_task_and_says_so() {
        let (mut m, _dir) = model();
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
        let (mut m, _dir) = model();
        m.model.working.appearance.palette.accent = "#ff00aa".to_string();
        update(&mut m, Msg::ConfigReloaded);
        assert!(
            m.is_dirty(),
            "an external reload must not touch the working copy"
        );
        assert_eq!(m.status, "Compositor reloaded its configuration");
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
        let (m, _dir) = model();
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
        let (m, _dir) = model();
        assert_eq!(m.page, PageId::Appearance);
        let ids = node_ids(&probe_of(m));
        assert!(ids.contains(&"appearance_page".to_string()));

        let (mut m2, _dir2) = model();
        m2.page = PageId::Displays;
        let ids = node_ids(&probe_of(m2));
        assert!(ids.contains(&"displays_page".to_string()));
    }

    /// The footer is computed, not pushed: `Unsaved changes` is what
    /// `is_dirty()` says, and Revert/Apply are insensitive while clean —
    /// `main.rs:73-84`'s `update_footer` closure, with the Rc<dyn Fn()> gone.
    ///
    /// Mutation check: make `footer` always render `m.status`; this fails.
    #[test]
    fn the_footer_reports_dirtiness_without_a_flag() {
        let (mut m, _dir) = model();
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
        let (mut m, _dir) = model();
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
            let (mut m, _dir) = model();
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
        let (mut m, _dir) = model();
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
}
