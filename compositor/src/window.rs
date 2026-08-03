use std::collections::BTreeMap;

use icedtea_contract as contract;
use contract::{Event, Rectangle, Snapshot, WindowId, WindowInfo, WindowUpdate, WorkspaceInfo};

#[derive(Debug, Clone)]
pub struct Window {
    pub id: WindowId,
    pub app_id: String,
    pub title: String,
    pub pid: u32,
    pub workspace: u32,
    pub geometry: Rectangle,
    pub maximized: bool,
    pub minimized: bool,
    pub fullscreen: bool,
    pub focused: bool,
}

#[derive(Debug, Clone)]
pub struct Workspace {
    pub id: u32,
    pub name: String,
    pub focused_window: Option<WindowId>,
}

pub struct WindowManager {
    windows: BTreeMap<WindowId, Window>,
    workspaces: Vec<Workspace>,
    active_workspace: u32,
    next_id: u32,
    seq: u64,
    /// Pending events drained by the compositor each frame.
    pub pending_events: Vec<Event>,
}

impl WindowManager {
    pub fn new(workspace_names: Vec<String>) -> Self {
        let workspaces = workspace_names
            .iter()
            .enumerate()
            .map(|(i, name)| Workspace { id: i as u32, name: name.clone(), focused_window: None })
            .collect();
        Self {
            windows: BTreeMap::new(),
            workspaces,
            active_workspace: 0,
            next_id: 1,
            seq: 0,
            pending_events: Vec::new(),
        }
    }

    fn bump(&mut self) {
        self.seq += 1;
    }

    fn emit(&mut self, event: Event) {
        self.bump();
        self.pending_events.push(event);
    }

    pub fn add_window(&mut self, app_id: &str, title: &str, pid: u32) -> WindowId {
        let id = WindowId(self.next_id);
        self.next_id += 1;
        let window = Window {
            id,
            app_id: app_id.to_string(),
            title: title.to_string(),
            pid,
            workspace: self.active_workspace,
            geometry: Rectangle { x: 0, y: 0, width: 0, height: 0 },
            maximized: false,
            minimized: false,
            fullscreen: false,
            focused: false,
        };
        self.windows.insert(id, window.clone());
        self.emit(Event::WindowOpened(self.to_info(&self.windows[&id])));
        self.focus(id);
        id
    }

    pub fn remove_window(&mut self, id: WindowId) -> Option<Window> {
        let window = self.windows.remove(&id)?;
        let workspace = window.workspace;
        let was_focused = self.workspace_mut(workspace).focused_window == Some(id);
        if was_focused {
            self.workspace_mut(workspace).focused_window = None;
        }
        self.emit(Event::WindowClosed(id));
        Some(window)
    }

    pub fn get(&self, id: WindowId) -> Option<&Window> {
        self.windows.get(&id)
    }

    pub fn set_title(&mut self, id: WindowId, title: String) -> Option<()> {
        let w = self.windows.get_mut(&id)?;
        w.title = title.clone();
        self.emit(Event::WindowUpdated { id, update: WindowUpdate { title: Some(title), ..Default::default() } });
        Some(())
    }

    pub fn set_geometry(&mut self, id: WindowId, geometry: Rectangle) -> Option<()> {
        let w = self.windows.get_mut(&id)?;
        w.geometry = geometry;
        self.emit(Event::WindowUpdated { id, update: WindowUpdate { geometry: Some(geometry), ..Default::default() } });
        Some(())
    }

    pub fn set_maximized(&mut self, id: WindowId, value: bool) -> Option<()> {
        let w = self.windows.get_mut(&id)?;
        w.maximized = value;
        self.emit(Event::WindowUpdated { id, update: WindowUpdate { maximized: Some(value), ..Default::default() } });
        Some(())
    }

    pub fn set_minimized(&mut self, id: WindowId, value: bool) -> Option<()> {
        let w = self.windows.get_mut(&id)?;
        w.minimized = value;
        self.emit(Event::WindowUpdated { id, update: WindowUpdate { minimized: Some(value), ..Default::default() } });
        Some(())
    }

    pub fn set_fullscreen(&mut self, id: WindowId, value: bool) -> Option<()> {
        let w = self.windows.get_mut(&id)?;
        w.fullscreen = value;
        self.emit(Event::WindowUpdated { id, update: WindowUpdate { fullscreen: Some(value), ..Default::default() } });
        Some(())
    }

    pub fn set_workspace(&mut self, id: WindowId, workspace: u32) -> Option<()> {
        if !self.workspace_exists(workspace) {
            return None;
        }
        let w = self.windows.get_mut(&id)?;
        w.workspace = workspace;
        w.focused = false;
        self.emit(Event::WindowUpdated { id, update: WindowUpdate { workspace: Some(workspace), focused: Some(false), ..Default::default() } });
        Some(())
    }

    pub fn focus(&mut self, id: WindowId) -> Option<()> {
        let w = self.windows.get_mut(&id)?;
        if w.minimized {
            w.minimized = false;
        }
        let (ws, was_focused) = {
            let w = self.windows.get(&id)?;
            (w.workspace, w.focused)
        };
        if was_focused {
            return Some(());
        }
        // Clear prior focus on the same workspace.
        let old = self.workspace_mut(ws).focused_window.replace(id);
        match old {
            Some(old_id) if old_id != id => {
                if let Some(old_w) = self.windows.get_mut(&old_id) {
                    old_w.focused = false;
                    self.emit(Event::WindowUpdated { id: old_id, update: WindowUpdate { focused: Some(false), ..Default::default() } });
                }
            }
            _ => {}
        }
        let w = self.windows.get_mut(&id)?;
        w.focused = true;
        self.emit(Event::WindowUpdated { id, update: WindowUpdate { focused: Some(true), ..Default::default() } });
        Some(())
    }

    pub fn focused_window(&self) -> Option<&Window> {
        self.workspace(self.active_workspace)
            .and_then(|ws| ws.focused_window)
            .and_then(|id| self.windows.get(&id))
    }

    pub fn set_active_workspace(&mut self, id: u32) -> bool {
        if !self.workspace_exists(id) {
            return false;
        }
        self.active_workspace = id;
        self.emit(Event::WorkspaceSet { id, active: true });
        true
    }

    pub fn active_workspace(&self) -> u32 {
        self.active_workspace
    }

    pub fn windows(&self) -> impl Iterator<Item = &Window> {
        self.windows.values()
    }

    pub fn windows_in_workspace(&self, ws: u32) -> Vec<&Window> {
        self.windows.values().filter(|w| w.workspace == ws).collect()
    }

    pub fn to_info(&self, w: &Window) -> WindowInfo {
        WindowInfo {
            id: w.id,
            app_id: w.app_id.clone(),
            title: w.title.clone(),
            pid: w.pid,
            workspace: w.workspace,
            geometry: w.geometry,
            maximized: w.maximized,
            minimized: w.minimized,
            fullscreen: w.fullscreen,
            focused: w.focused,
        }
    }

    pub fn workspace_info(&self) -> Vec<WorkspaceInfo> {
        self.workspaces.iter().map(|w| WorkspaceInfo { id: w.id, name: w.name.clone() }).collect()
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            seq: self.seq,
            windows: self.windows.values().map(|w| self.to_info(w)).collect(),
            workspaces: self.workspace_info(),
            active_workspace: self.active_workspace,
        }
    }

    pub fn alt_tab_entries(&self) -> Vec<WindowId> {
        self.windows_in_workspace(self.active_workspace)
            .into_iter()
            .filter(|w| !w.minimized)
            .map(|w| w.id)
            .collect()
    }

    fn workspace(&self, id: u32) -> Option<&Workspace> {
        self.workspaces.get(id as usize)
    }

    fn workspace_mut(&mut self, id: u32) -> &mut Workspace {
        &mut self.workspaces[id as usize]
    }

    fn workspace_exists(&self, id: u32) -> bool {
        (id as usize) < self.workspaces.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mgr() -> WindowManager {
        WindowManager::new(vec!["1".into(), "2".into()])
    }

    #[test]
    fn add_focuses_window_and_emits_opened() {
        let mut m = mgr();
        let id = m.add_window("app", "title", 1);
        assert!(m.get(id).unwrap().focused);
        assert!(matches!(m.pending_events.first(), Some(Event::WindowOpened(_))));
    }

    #[test]
    fn focus_unfocuses_previous() {
        let mut m = mgr();
        let a = m.add_window("a", "a", 1);
        let b = m.add_window("b", "b", 2);
        assert!(!m.get(a).unwrap().focused);
        assert!(m.get(b).unwrap().focused);
    }

    #[test]
    fn move_to_workspace_keeps_focus_valid() {
        let mut m = mgr();
        let a = m.add_window("a", "a", 1);
        m.set_workspace(a, 1).unwrap();
        assert_eq!(m.get(a).unwrap().workspace, 1);
        assert!(!m.get(a).unwrap().focused);
    }

    #[test]
    fn remove_clears_focus_to_none() {
        let mut m = mgr();
        let a = m.add_window("a", "a", 1);
        m.remove_window(a).unwrap();
        assert!(m.focused_window().is_none());
    }

    #[test]
    fn snapshot_matches_state() {
        let mut m = mgr();
        let id = m.add_window("app", "t", 7);
        let snap = m.snapshot();
        assert_eq!(snap.active_workspace, 0);
        assert_eq!(snap.windows.len(), 1);
        assert_eq!(snap.windows[0].id, id);
        assert_eq!(snap.windows[0].pid, 7);
        assert_eq!(snap.workspaces.len(), 2);
    }

    #[test]
    fn alt_tab_skips_minimized() {
        let mut m = mgr();
        let a = m.add_window("a", "a", 1);
        let b = m.add_window("b", "b", 2);
        m.set_minimized(a, true).unwrap();
        assert_eq!(m.alt_tab_entries(), vec![b]);
    }
}
