//! The pure taskbar model: the window list, workspaces, and active workspace,
//! folded from `org.icedtea.WM` traffic. No GTK — the render half (Task 9)
//! diffs this into widgets.

use icedtea_contract::{Snapshot, WindowInfo, WindowUpdate, WorkspaceInfo};

#[derive(Default, Debug)]
pub struct TaskbarModel {
    pub windows: Vec<WindowInfo>,
    pub workspaces: Vec<WorkspaceInfo>,
    pub active_workspace: u32,
}

/// The subset of `org.icedtea.WM` traffic the taskbar folds in. The worker
/// (`wm_client`) translates D-Bus signals + the seed snapshot into these.
#[derive(Debug)]
pub enum WmUpdate {
    Snapshot(Snapshot),
    Opened(WindowInfo),
    Closed(u32),
    Updated { id: u32, update: WindowUpdate },
    WorkspaceSet { id: u32, active: bool },
    WorkspaceList(Vec<WorkspaceInfo>),
}

impl TaskbarModel {
    pub fn apply(&mut self, u: WmUpdate) {
        match u {
            WmUpdate::Snapshot(s) => {
                self.windows = s.windows;
                self.workspaces = s.workspaces;
                self.active_workspace = s.active_workspace;
            }
            WmUpdate::Opened(w) => match self.windows.iter_mut().find(|x| x.id == w.id) {
                Some(existing) => *existing = w,
                None => self.windows.push(w),
            },
            WmUpdate::Closed(id) => self.windows.retain(|w| w.id.0 != id),
            WmUpdate::Updated { id, update } => {
                if let Some(w) = self.windows.iter_mut().find(|x| x.id.0 == id) {
                    merge(w, update);
                }
            }
            WmUpdate::WorkspaceSet { id, active } => {
                if active {
                    self.active_workspace = id;
                }
            }
            WmUpdate::WorkspaceList(ws) => self.workspaces = ws,
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
        m.apply(WmUpdate::Opened(win(1, "a")));
        m.apply(WmUpdate::Opened(win(2, "b")));
        assert_eq!(m.windows.len(), 2);
        m.apply(WmUpdate::Closed(1));
        assert_eq!(m.windows.iter().map(|w| w.id.0).collect::<Vec<_>>(), vec![2]);
    }

    #[test]
    fn opened_with_a_known_id_replaces_rather_than_duplicates() {
        let mut m = TaskbarModel::default();
        m.apply(WmUpdate::Opened(win(1, "a")));
        m.apply(WmUpdate::Opened(win(1, "a-again")));
        assert_eq!(m.windows.len(), 1);
        assert_eq!(m.windows[0].app_id, "a-again");
    }

    #[test]
    fn updated_merges_title() {
        let mut m = TaskbarModel::default();
        m.apply(WmUpdate::Opened(win(1, "a")));
        m.apply(WmUpdate::Updated { id: 1, update: title_update("renamed") });
        assert_eq!(m.windows[0].title, "renamed");
    }

    #[test]
    fn workspace_set_tracks_active_only_when_active() {
        let mut m = TaskbarModel::default();
        m.apply(WmUpdate::WorkspaceSet { id: 3, active: true });
        assert_eq!(m.active_workspace, 3);
        m.apply(WmUpdate::WorkspaceSet { id: 5, active: false });
        assert_eq!(m.active_workspace, 3, "an inactive set must not move the active workspace");
    }
}
