//! The pure taskbar model: the window list, workspaces, and active workspace,
//! folded from `org.icedtea.Compositor` traffic. No GTK — the render half (Task 9)
//! diffs this into widgets.

use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{Box as GtkBox, Button, GestureClick, Orientation};
use icedtea_contract::{Snapshot, WindowInfo, WindowUpdate, WorkspaceInfo};

use crate::compositor_client::CompositorCommands;

#[derive(Default, Debug)]
pub struct TaskbarModel {
    pub windows: Vec<WindowInfo>,
    pub workspaces: Vec<WorkspaceInfo>,
    pub active_workspace: u32,
}

/// The subset of `org.icedtea.Compositor` traffic the taskbar folds in. The worker
/// (`compositor_client`) translates D-Bus signals + the seed snapshot into these.
#[derive(Debug)]
pub enum CompositorUpdate {
    Snapshot(Snapshot),
    Opened(WindowInfo),
    Closed(u32),
    Updated { id: u32, update: WindowUpdate },
    WorkspaceSet { id: u32, active: bool },
    WorkspaceList(Vec<WorkspaceInfo>),
}

impl TaskbarModel {
    pub fn apply(&mut self, u: CompositorUpdate) {
        match u {
            CompositorUpdate::Snapshot(s) => {
                self.windows = s.windows;
                self.workspaces = s.workspaces;
                self.active_workspace = s.active_workspace;
            }
            CompositorUpdate::Opened(w) => match self.windows.iter_mut().find(|x| x.id == w.id) {
                Some(existing) => *existing = w,
                None => self.windows.push(w),
            },
            CompositorUpdate::Closed(id) => self.windows.retain(|w| w.id.0 != id),
            CompositorUpdate::Updated { id, update } => {
                if let Some(w) = self.windows.iter_mut().find(|x| x.id.0 == id) {
                    merge(w, update);
                }
            }
            CompositorUpdate::WorkspaceSet { id, active } => {
                if active {
                    self.active_workspace = id;
                }
            }
            CompositorUpdate::WorkspaceList(ws) => self.workspaces = ws,
        }
    }
}

fn merge(w: &mut WindowInfo, u: WindowUpdate) {
    if let Some(t) = u.title {
        w.title = t;
    }
    if let Some(g) = u.geometry {
        w.geometry = g;
    }
    if let Some(ws) = u.workspace {
        w.workspace = ws;
    }
    if let Some(m) = u.maximized {
        w.maximized = m;
    }
    if let Some(m) = u.minimized {
        w.minimized = m;
    }
    if let Some(f) = u.fullscreen {
        w.fullscreen = f;
    }
    if let Some(f) = u.focused {
        w.focused = f;
    }
    // `mapped` has no field on WindowInfo; the taskbar ignores it.
}

/// Rebuild the bar `container` from `model`. Clear-and-rebuild is fine for the
/// small window counts a taskbar shows; a diffing pass is a later refinement.
/// Layout: a `#workspaces` box then a `#windows` box, so tests (and CSS) can
/// address each half.
pub fn render(model: &TaskbarModel, container: &GtkBox, wm: &Rc<dyn CompositorCommands>) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }

    let workspaces = GtkBox::new(Orientation::Horizontal, 2);
    workspaces.set_widget_name("workspaces");
    for ws in &model.workspaces {
        let label = if ws.name.is_empty() { (ws.id + 1).to_string() } else { ws.name.clone() };
        let button = Button::with_label(&label);
        if ws.id == model.active_workspace {
            button.add_css_class("active");
        }
        let wm = wm.clone();
        let id = ws.id;
        button.connect_clicked(move |_| wm.set_workspace(id));
        workspaces.append(&button);
    }
    container.append(&workspaces);

    let windows = GtkBox::new(Orientation::Horizontal, 2);
    windows.set_widget_name("windows");
    for w in &model.windows {
        let label = if w.title.is_empty() { w.app_id.clone() } else { w.title.clone() };
        let button = Button::with_label(&label);
        button.set_widget_name("window-button");
        if w.focused {
            button.add_css_class("focused");
        }
        let id = w.id.0;
        let wm_focus = wm.clone();
        button.connect_clicked(move |_| wm_focus.focus_window(id));
        // Middle-click closes.
        let gesture = GestureClick::new();
        gesture.set_button(2);
        let wm_close = wm.clone();
        gesture.connect_pressed(move |_, _, _, _| wm_close.close_window(id));
        button.add_controller(gesture);
        windows.append(&button);
    }
    container.append(&windows);
}

#[cfg(test)]
mod tests {
    use super::*;
    use icedtea_contract::{Rectangle, WindowId};

    fn win(id: u32, app: &str) -> WindowInfo {
        WindowInfo {
            id: WindowId(id),
            app_id: app.into(),
            title: app.into(),
            pid: 0,
            workspace: 0,
            geometry: Rectangle { x: 0, y: 0, width: 1, height: 1 },
            maximized: false,
            minimized: false,
            fullscreen: false,
            focused: false,
        }
    }

    fn title_update(t: &str) -> WindowUpdate {
        WindowUpdate { title: Some(t.into()), ..Default::default() }
    }

    #[test]
    fn opened_then_closed_tracks_the_window_set() {
        let mut m = TaskbarModel::default();
        m.apply(CompositorUpdate::Opened(win(1, "a")));
        m.apply(CompositorUpdate::Opened(win(2, "b")));
        assert_eq!(m.windows.len(), 2);
        m.apply(CompositorUpdate::Closed(1));
        assert_eq!(m.windows.iter().map(|w| w.id.0).collect::<Vec<_>>(), vec![2]);
    }

    #[test]
    fn opened_with_a_known_id_replaces_rather_than_duplicates() {
        let mut m = TaskbarModel::default();
        m.apply(CompositorUpdate::Opened(win(1, "a")));
        m.apply(CompositorUpdate::Opened(win(1, "a-again")));
        assert_eq!(m.windows.len(), 1);
        assert_eq!(m.windows[0].app_id, "a-again");
    }

    #[test]
    fn updated_merges_title() {
        let mut m = TaskbarModel::default();
        m.apply(CompositorUpdate::Opened(win(1, "a")));
        m.apply(CompositorUpdate::Updated { id: 1, update: title_update("renamed") });
        assert_eq!(m.windows[0].title, "renamed");
    }

    #[test]
    fn workspace_set_tracks_active_only_when_active() {
        let mut m = TaskbarModel::default();
        m.apply(CompositorUpdate::WorkspaceSet { id: 3, active: true });
        assert_eq!(m.active_workspace, 3);
        m.apply(CompositorUpdate::WorkspaceSet { id: 5, active: false });
        assert_eq!(m.active_workspace, 3, "an inactive set must not move the active workspace");
    }
}
