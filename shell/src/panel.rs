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
use icedtea_ui::view::builders::{box_, button};
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
        Msg::WorkspaceClicked(id) => {
            let wm = m.wm.clone();
            Cmd::Task(Rc::new(move || wm.set_workspace(id)))
        }
        Msg::WindowClicked(id) => {
            let wm = m.wm.clone();
            Cmd::Task(Rc::new(move || wm.focus_window(id)))
        }
        Msg::WindowPointerUp { .. }
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
pub fn view(m: &PanelModel) -> View<Msg> {
    box_(
        Orientation::Horizontal,
        [workspaces(m), windows(m), clip_button(m)],
    )
    .id("bar")
}

/// One button per workspace, keyed by workspace id.
///
/// Label, one-indexed fallback and the `active` class are `taskbar::render`'s
/// rules verbatim; what changed is that they are now computed from the model
/// on every `view` instead of being written into a widget on every rebuild.
fn workspaces(m: &PanelModel) -> View<Msg> {
    let buttons: Vec<View<Msg>> = m
        .taskbar
        .workspaces
        .iter()
        .map(|ws| {
            let id = ws.id;
            let label = if ws.name.is_empty() {
                (id + 1).to_string()
            } else {
                ws.name.clone()
            };
            let view = button(&label)
                .key(u64::from(id))
                .id(&format!("ws_{id}"))
                .on_click(Msg::WorkspaceClicked(id));
            if id == m.taskbar.active_workspace {
                view.class("active")
            } else {
                view
            }
        })
        .collect();
    box_(Orientation::Horizontal, buttons).id("workspaces")
}

/// One button per window, keyed by window id.
///
/// `hexpand` lives here, not on a wrapper: the GTK panel kept the taskbar in
/// its own child box only so the clipboard button survived
/// `taskbar::render`'s clear-and-rebuild. There is no rebuild to survive.
///
/// `attention` surfaces the compositor's refused-activation flag
/// (`State::request_activate`), which is the whole point of the bit.
fn windows(m: &PanelModel) -> View<Msg> {
    let buttons: Vec<View<Msg>> = m
        .taskbar
        .windows
        .iter()
        .map(|w| {
            let id = w.id.0;
            let label = if w.title.is_empty() {
                w.app_id.clone()
            } else {
                w.title.clone()
            };
            let mut view = button(&label)
                .key(u64::from(id))
                .id(&format!("window_{id}"))
                .on_click(Msg::WindowClicked(id));
            if w.focused {
                view = view.class("focused");
            }
            if w.attention {
                view = view.class("attention");
            }
            view
        })
        .collect();
    box_(Orientation::Horizontal, buttons)
        .id("windows")
        .hexpand(true)
}

/// The clipboard popover's trigger. `active` while the popover is open, the
/// same class `#workspaces button.active` uses for the same "this is the one"
/// meaning.
fn clip_button(m: &PanelModel) -> View<Msg> {
    let view = button("clip").id("clip").on_click(Msg::ClipButtonClicked);
    if m.open_popover.is_some() {
        view.class("active")
    } else {
        view
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use icedtea_contract::{Rectangle, Snapshot, WindowId, WindowInfo, WorkspaceInfo};
    use icedtea_ui::view::{EventKind, PropName};

    /// Run a `Cmd`'s side effects the way the loop does: `Cmd::Task` bodies,
    /// in order, after the fold. Anything else is inert here — `Cmd::OpenPopup`
    /// and `Cmd::ClosePopup` need a window, and the tests that care about them
    /// assert on the command's shape instead.
    fn run_tasks(cmd: Cmd<Msg>) {
        match cmd {
            Cmd::Task(f) => f(),
            Cmd::Batch(list) => {
                for c in list {
                    run_tasks(c);
                }
            }
            _ => {}
        }
    }

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

    /// Find the first descendant of `v` whose `id` prop is `id`.
    fn by_id<'a>(v: &'a View<Msg>, id: &str) -> Option<&'a View<Msg>> {
        if v.props.str(PropName::Id) == Some(id) {
            return Some(v);
        }
        v.children.iter().find_map(|c| by_id(c, id))
    }

    /// The `Label` prop of every direct child of the container `id` names.
    fn labels(v: &View<Msg>, id: &str) -> Vec<String> {
        by_id(v, id)
            .expect("container")
            .children
            .iter()
            .map(|c| c.props.str(PropName::Label).unwrap_or_default().to_string())
            .collect()
    }

    fn seeded() -> (PanelModel, Rc<MockWm>, Rc<MockClip>) {
        let (mut m, wm, clip) = panel();
        let _ = update(
            &mut m,
            Msg::Compositor(Arc::new(CompositorUpdate::Snapshot(snapshot(
                vec![win(1, "One"), win(2, "Two")],
                vec![
                    WorkspaceInfo {
                        id: 0,
                        name: String::new(),
                    },
                    WorkspaceInfo {
                        id: 1,
                        name: "web".into(),
                    },
                ],
            )))),
        );
        (m, wm, clip)
    }

    #[test]
    fn the_bar_holds_workspaces_windows_and_the_clip_button() {
        let (m, _, _) = seeded();
        let v = view(&m);
        assert_eq!(v.props.str(PropName::Id), Some("bar"));
        let ids: Vec<Option<&str>> = v
            .children
            .iter()
            .map(|c| c.props.str(PropName::Id))
            .collect();
        assert_eq!(
            ids,
            vec![Some("workspaces"), Some("windows"), Some("clip")],
            "contract §3.3's order: workspaces, windows, clip"
        );
    }

    #[test]
    fn a_window_button_is_labelled_by_title_then_app_id() {
        let (mut m, _, _) = seeded();
        assert_eq!(labels(&view(&m), "windows"), vec!["One", "Two"]);
        // A window with no title falls back to its app id, verbatim from
        // `taskbar::render`.
        let mut untitled = win(3, "");
        untitled.app_id = "org.example.Thing".into();
        let _ = update(
            &mut m,
            Msg::Compositor(Arc::new(CompositorUpdate::Opened(untitled))),
        );
        assert_eq!(
            labels(&view(&m), "windows"),
            vec!["One", "Two", "org.example.Thing"]
        );
    }

    #[test]
    fn a_workspace_button_is_labelled_by_name_then_one_indexed_id() {
        let (m, _, _) = seeded();
        assert_eq!(
            labels(&view(&m), "workspaces"),
            vec!["1", "web"],
            "an unnamed workspace shows `id + 1`; a named one shows its name"
        );
    }

    #[test]
    fn the_active_workspace_and_the_focused_window_carry_their_classes() {
        let (mut m, _, _) = seeded();
        let _ = update(
            &mut m,
            Msg::Compositor(Arc::new(CompositorUpdate::WorkspaceSet {
                id: 1,
                active: true,
            })),
        );
        let _ = update(
            &mut m,
            Msg::Compositor(Arc::new(CompositorUpdate::Updated {
                id: 2,
                update: icedtea_contract::WindowUpdate {
                    focused: Some(true),
                    attention: Some(true),
                    ..Default::default()
                },
            })),
        );
        let v = view(&m);
        let classes = |id: &str| -> Vec<String> {
            match by_id(&v, id).expect("button").props.get(PropName::Classes) {
                Some(icedtea_ui::view::Prop::Classes(list)) => {
                    list.iter().map(|c| c.to_string()).collect()
                }
                _ => Vec::new(),
            }
        };
        assert!(classes("ws_1").contains(&"active".to_string()));
        assert!(!classes("ws_0").contains(&"active".to_string()));
        assert!(classes("window_2").contains(&"focused".to_string()));
        assert!(classes("window_2").contains(&"attention".to_string()));
        assert!(classes("window_1").is_empty());
    }

    #[test]
    fn clicking_a_window_button_reaches_focus_window_on_the_command_surface() {
        let (m, wm, _) = seeded();
        let v = view(&m);
        let msg = by_id(&v, "window_1")
            .expect("window_1")
            .handlers
            .fire_unit(EventKind::Click)
            .expect("a click handler");
        assert!(matches!(msg, Msg::WindowClicked(1)));
        let mut m = m;
        let cmd = update(&mut m, msg);
        run_tasks(cmd);
        assert_eq!(wm.calls.borrow().as_slice(), [("focus".to_string(), 1)]);
    }

    #[test]
    fn clicking_a_workspace_button_reaches_set_workspace() {
        let (m, wm, _) = seeded();
        let v = view(&m);
        let msg = by_id(&v, "ws_1")
            .expect("ws_1")
            .handlers
            .fire_unit(EventKind::Click)
            .expect("a click handler");
        assert!(matches!(msg, Msg::WorkspaceClicked(1)));
        let mut m = m;
        let cmd = update(&mut m, msg);
        run_tasks(cmd);
        assert_eq!(wm.calls.borrow().as_slice(), [("workspace".to_string(), 1)]);
    }
}
