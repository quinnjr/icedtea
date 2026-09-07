//! The panel: one `App<PanelModel, Msg>` on one layer surface, carrying the
//! taskbar and the clipboard popover.
//!
//! `view` is pure and keyed — workspace id, window id, history entry id — so
//! the reconciler keeps hover and focus identity across an update. The GTK
//! panel cleared and rebuilt its containers instead; that was a workaround for
//! having no reconciler, not a design (spec D9).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use icedtea_contract::ClipEntry;
use icedtea_ui::layout::Rect;
use icedtea_ui::view::builders::box_;
use icedtea_ui::view::{Cmd, View};
use icedtea_ui::widgets::Orientation;
use icedtea_ui::window::popup::PopupKey;

use crate::clip_client::ClipCommands;
use crate::clipboard::{ClipUpdate, ClipboardModel};
use crate::compositor_client::CompositorCommands;
use crate::taskbar::{CompositorUpdate, TaskbarModel};

/// The bar's committed height, and its exclusive zone.
///
/// layer-shell's `auto_exclusive_zone_enable()` has no `LayerSpec`
/// equivalent — the field is a concrete `i32` — so the panel names the height
/// `set_size_request(-1, 28)` used to force (contract §6 P5-D6).
pub const BAR_HEIGHT: i32 = 28;

/// The clipboard popover's surface size.
pub const POPOVER_SIZE: (u32, u32) = (320, 280);

pub struct PanelModel {
    /// Verbatim from `taskbar.rs`; `apply`/`merge` and their 5 tests unchanged.
    pub taskbar: TaskbarModel,
    /// Verbatim from `clipboard.rs`; `apply` and its 1 test unchanged.
    pub clipboard: ClipboardModel,
    /// The single source of truth for the clipboard popover.
    pub open_popover: Option<PopupKey>,
    /// Kept behind the traits so the tests can pass mocks (spec D8). `Rc`, not
    /// `Arc`: they never cross a thread — `update` runs on the loop thread and
    /// so does `Cmd::Task`.
    pub wm: Rc<dyn CompositorCommands>,
    pub clip: Rc<dyn ClipCommands>,
    pub bar_height: i32,
    /// The `clip` button's border box, republished once a frame by the
    /// `App::on_frame` hook. `Cmd::OpenPopup`'s anchor rect: `update` has no
    /// `&Window`, and `PopupAnchorPoint::Node` needs a `Node` a handler cannot
    /// hand it.
    pub clip_rect: Rc<Cell<Option<Rect>>>,
    /// A shared mirror of `clipboard.entries`, refreshed by `update` on every
    /// history change. The popover's body is a `Cmd::OpenPopup` payload the
    /// loop re-runs each frame (contract §6 P5-D2) and therefore cannot borrow
    /// the model; this is what it reads instead.
    pub history: Rc<RefCell<Vec<ClipEntry>>>,
}

impl PanelModel {
    #[must_use]
    pub fn new(wm: Rc<dyn CompositorCommands>, clip: Rc<dyn ClipCommands>) -> PanelModel {
        PanelModel {
            taskbar: TaskbarModel::default(),
            clipboard: ClipboardModel::default(),
            open_popover: None,
            wm,
            clip,
            bar_height: BAR_HEIGHT,
            clip_rect: Rc::new(Cell::new(None)),
            history: Rc::new(RefCell::new(Vec::new())),
        }
    }
}

#[derive(Clone, Debug)]
pub enum Msg {
    /// One compositor update, from the inbox. `Arc`, not `Rc`: `Msg` is `Send`.
    Compositor(Arc<CompositorUpdate>),
    /// One clipboard history update, from the inbox.
    Clip(Arc<ClipUpdate>),

    WorkspaceClicked(u32),
    WindowClicked(u32),
    /// Every pointer release on a window button; `update` acts only on
    /// `BTN_MIDDLE`. Left releases arrive here too and are ignored — the focus
    /// path is `WindowClicked`, fired by `EventKind::Click`.
    WindowPointerUp {
        id: u32,
        button: u32,
    },

    ClipButtonClicked,
    PopoverOpened(PopupKey),
    PopoverDismissed(PopupKey),
    ClipActivated(u64),
    ClipPinToggled {
        id: u64,
        pinned: bool,
    },
    ClipRemoved(u64),
    ClipCleared,
}

/// Contract §3.1: the inbox carries `Msg` across a thread, so it must be `Send`.
const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<Msg>();
};

/// The command surface when there is no session bus.
///
/// The GTK panel's `wire_taskbar`/`wire_clipboard` returned early on a proxy
/// failure and left that half of the bar inert. One `App` cannot return early
/// from half of itself, so the inertness moves behind the traits: the bar still
/// opens, still paints and still reserves its exclusive zone, and every command
/// is dropped with one warning.
#[derive(Debug, Default)]
pub struct Offline;

impl CompositorCommands for Offline {
    fn focus_window(&self, id: u32) {
        tracing::warn!(id, "no session bus; focus_window dropped");
    }
    fn close_window(&self, id: u32) {
        tracing::warn!(id, "no session bus; close_window dropped");
    }
    fn set_workspace(&self, id: u32) {
        tracing::warn!(id, "no session bus; set_workspace dropped");
    }
}

impl ClipCommands for Offline {
    fn activate(&self, id: u64) {
        tracing::warn!(id, "no session bus; activate dropped");
    }
    fn pin(&self, id: u64, on: bool) {
        tracing::warn!(id, on, "no session bus; pin dropped");
    }
    fn remove(&self, id: u64) {
        tracing::warn!(id, "no session bus; remove dropped");
    }
    fn clear(&self) {
        tracing::warn!("no session bus; clear dropped");
    }
}

/// Fold one message. Never blocks and never calls D-Bus: every outbound
/// command leaves as a `Cmd::Task`, which the loop runs after the fold.
pub fn update(m: &mut PanelModel, msg: Msg) -> Cmd<Msg> {
    match msg {
        Msg::Compositor(u) => {
            m.taskbar.apply(Arc::unwrap_or_clone(u));
            Cmd::None
        }
        Msg::Clip(u) => {
            m.clipboard.apply(Arc::unwrap_or_clone(u));
            *m.history.borrow_mut() = m.clipboard.entries.clone();
            Cmd::None
        }
        Msg::WorkspaceClicked(_)
        | Msg::WindowClicked(_)
        | Msg::WindowPointerUp { .. }
        | Msg::ClipButtonClicked
        | Msg::PopoverOpened(_)
        | Msg::PopoverDismissed(_)
        | Msg::ClipActivated(_)
        | Msg::ClipPinToggled { .. }
        | Msg::ClipRemoved(_)
        | Msg::ClipCleared => Cmd::None,
    }
}

/// The whole bar. `#bar` is what `style.css`'s first selector names.
pub fn view(_m: &PanelModel) -> View<Msg> {
    box_(Orientation::Horizontal, Vec::<View<Msg>>::new()).id("bar")
}

#[cfg(test)]
mod tests {
    use super::*;
    use icedtea_contract::{Rectangle, Snapshot, WindowId, WindowInfo, WorkspaceInfo};

    /// A recording `CompositorCommands`. `RefCell`, not `Mutex`: these unit
    /// tests are single-threaded, and so is `update`. The cross-thread mock
    /// the harness tests need lives in `shell/tests/support/mod.rs`.
    #[derive(Default)]
    pub(crate) struct MockWm {
        pub(crate) calls: RefCell<Vec<(String, u32)>>,
    }

    impl CompositorCommands for MockWm {
        fn focus_window(&self, id: u32) {
            self.calls.borrow_mut().push(("focus".into(), id));
        }
        fn close_window(&self, id: u32) {
            self.calls.borrow_mut().push(("close".into(), id));
        }
        fn set_workspace(&self, id: u32) {
            self.calls.borrow_mut().push(("workspace".into(), id));
        }
    }

    #[derive(Default)]
    pub(crate) struct MockClip {
        pub(crate) calls: RefCell<Vec<(String, u64)>>,
    }

    impl ClipCommands for MockClip {
        fn activate(&self, id: u64) {
            self.calls.borrow_mut().push(("activate".into(), id));
        }
        fn pin(&self, id: u64, _on: bool) {
            self.calls.borrow_mut().push(("pin".into(), id));
        }
        fn remove(&self, id: u64) {
            self.calls.borrow_mut().push(("remove".into(), id));
        }
        fn clear(&self) {
            self.calls.borrow_mut().push(("clear".into(), 0));
        }
    }

    pub(crate) fn win(id: u32, title: &str) -> WindowInfo {
        WindowInfo {
            id: WindowId(id),
            app_id: "app".into(),
            title: title.into(),
            pid: 0,
            workspace: 0,
            geometry: Rectangle {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            },
            maximized: false,
            minimized: false,
            fullscreen: false,
            focused: false,
            attention: false,
        }
    }

    pub(crate) fn snapshot(windows: Vec<WindowInfo>, workspaces: Vec<WorkspaceInfo>) -> Snapshot {
        Snapshot {
            seq: 1,
            windows,
            workspaces,
            active_workspace: 0,
        }
    }

    /// The mocks, the model constructor and the two recording vectors, in one
    /// place, so every test below reads the same way.
    pub(crate) fn panel() -> (PanelModel, Rc<MockWm>, Rc<MockClip>) {
        let wm = Rc::new(MockWm::default());
        let clip = Rc::new(MockClip::default());
        let model = PanelModel::new(wm.clone(), clip.clone());
        (model, wm, clip)
    }

    #[test]
    fn a_fresh_panel_renders_the_bar() {
        let (m, _, _) = panel();
        let v = view(&m);
        assert_eq!(
            v.props.str(icedtea_ui::view::PropName::Id),
            Some("bar"),
            "style.css's first selector is #bar"
        );
    }

    #[test]
    fn a_compositor_snapshot_reaches_the_taskbar_model() {
        let (mut m, _, _) = panel();
        let _ = update(
            &mut m,
            Msg::Compositor(Arc::new(CompositorUpdate::Snapshot(snapshot(
                vec![win(1, "One"), win(2, "Two")],
                vec![WorkspaceInfo {
                    id: 0,
                    name: String::new(),
                }],
            )))),
        );
        assert_eq!(
            m.taskbar.windows.iter().map(|w| w.id.0).collect::<Vec<_>>(),
            vec![1, 2]
        );
    }

    #[test]
    fn a_history_update_reaches_both_the_model_and_the_popover_mirror() {
        let (mut m, _, _) = panel();
        let _ = update(
            &mut m,
            Msg::Clip(Arc::new(ClipUpdate::History(vec![ClipEntry {
                id: 10,
                kind: icedtea_contract::ClipKind::Text,
                preview: "copied text".into(),
                mime: "text/plain".into(),
                pinned: false,
                source_app: None,
            }]))),
        );
        assert_eq!(m.clipboard.entries.len(), 1);
        assert_eq!(
            m.history.borrow().iter().map(|e| e.id).collect::<Vec<_>>(),
            vec![10],
            "the popover's mirror must follow the model"
        );
    }
}
