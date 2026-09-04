//! The settings application: the model the loop folds, its messages, and
//! `update`.
//!
//! `view` lives here too (see below); every page contributes one `pub fn
//! view(&SettingsModel) -> View<Msg>` from its own module.

use std::rc::Rc;

use icedtea_ui::view::Cmd;

use crate::compositor_reload::ReloadOutcome;
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
            workers,
        }
    }

    /// `working != saved`, the one dirty rule.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.model.is_dirty()
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
    match msg {
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
    }
}

#[cfg(test)]
mod tests {
    use super::{Msg, SettingsModel, update};
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
}
