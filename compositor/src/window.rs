use std::collections::BTreeMap;

use icedtea_contract as contract;
use contract::{Event, Rectangle, SeqEvent, Snapshot, WindowId, WindowInfo, WindowUpdate, WorkspaceInfo};

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
    /// Whether this window's client currently has a mapped buffer attached.
    /// `true` from `add_window` (the model row is only ever created on a
    /// real map) until an `unmapped` (or, for a model-only row, a caller
    /// through `set_mapped`) says otherwise. Distinct from `minimized`:
    /// minimizing is a compositor-driven, user-visible state a client never
    /// sees reflected in its own protocol state, while unmapping is the
    /// client's own act of detaching its buffer (and can reverse itself by
    /// mapping again, which is why the row survives rather than being
    /// removed). Gates visibility (`is_visible`), alt-tab candidacy
    /// (`alt_tab_entries`), and focus candidacy (`focus`,
    /// `focus_mru_in_workspace`) the same way `minimized` already does.
    pub mapped: bool,
    /// Client's negotiated xdg-decoration mode: Some(true) for ClientSide, Some(false) for ServerSide, None if unset.
    pub client_decorations_requested: Option<bool>,
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
    /// Pending events drained by the compositor each frame, each tagged with
    /// the `seq` value its mutation advanced the counter to (review finding
    /// I2 -- see `contract::SeqEvent`).
    pub pending_events: Vec<SeqEvent>,
    /// Focus history: most-recently-focused windows (head = most recent).
    /// Invariant: every WindowId in self.windows is in focus_mru (guaranteed because
    /// add_window always calls focus, and remove_window prunes from focus_mru).
    focus_mru: Vec<WindowId>,
}

impl WindowManager {
    pub fn new(workspace_names: Vec<String>) -> Self {
        // Review finding M2: `workspace_mut` indexes `self.workspaces`
        // directly, so a manager built with an empty name list panicked on
        // the first `add_window`. Production callers are guarded
        // (`load_or_default` rejects an empty list), but `State::new` accepts
        // an arbitrary `Config`, so fall back to a single workspace here
        // rather than leaving a reachable index panic.
        let workspace_names =
            if workspace_names.is_empty() { vec!["1".to_string()] } else { workspace_names };
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
            focus_mru: Vec::new(),
        }
    }

    fn bump(&mut self) {
        self.seq += 1;
    }

    fn emit(&mut self, event: Event) {
        self.bump();
        let seq = self.seq;
        self.pending_events.push(SeqEvent { seq, event });
    }

    /// Queue an event that didn't come from one of this type's own mutators
    /// (e.g. `AltTabState`, driven by `State::alt_tab`), advancing the same
    /// sequence counter every other event uses.
    pub fn push_event(&mut self, event: Event) {
        self.emit(event);
    }

    pub fn add_window(&mut self, app_id: &str, title: &str, pid: u32, geometry: Rectangle) -> WindowId {
        let id = WindowId(self.next_id);
        self.next_id += 1;
        let window = Window {
            id,
            app_id: app_id.to_string(),
            title: title.to_string(),
            pid,
            workspace: self.active_workspace,
            geometry,
            maximized: false,
            minimized: false,
            fullscreen: false,
            focused: false,
            mapped: true,
            client_decorations_requested: None,
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
        // Prune from focus MRU to prevent unbounded growth.
        self.focus_mru.retain(|&wid| wid != id);
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

    pub fn toggle_maximized(&mut self, id: WindowId) -> Option<()> {
        let w = self.windows.get_mut(&id)?;
        let new_value = !w.maximized;
        w.maximized = new_value;
        self.emit(Event::WindowUpdated { id, update: WindowUpdate { maximized: Some(new_value), ..Default::default() } });
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

    pub fn set_client_decorations_requested(&mut self, id: WindowId, value: Option<bool>) -> Option<()> {
        let w = self.windows.get_mut(&id)?;
        w.client_decorations_requested = value;
        Some(())
    }

    /// Flip `id`'s mapped state. `None` (no emission) on an unknown id or a
    /// value that already matches -- the same "no-op on no change" shape
    /// every other setter here follows. `Some(())` otherwise, having emitted
    /// exactly one `WindowUpdated` (the milestone's one sanctioned addition
    /// to the exactly-one-event-per-mutation invariant).
    ///
    /// The row itself is never touched otherwise: unlike `remove_window`,
    /// this leaves geometry, title, and every other field exactly as they
    /// were, because an unmap is not a destroy (a client can map the same
    /// toplevel again). What changes is only what `is_visible`,
    /// `alt_tab_entries`, and `focus`/`focus_mru_in_workspace` are willing to
    /// do with the row while it's unmapped.
    pub fn set_mapped(&mut self, id: WindowId, mapped: bool) -> Option<()> {
        let w = self.windows.get_mut(&id)?;
        if w.mapped == mapped {
            return None;
        }
        w.mapped = mapped;
        self.emit(Event::WindowUpdated { id, update: WindowUpdate::default() });
        Some(())
    }

    pub fn set_workspace(&mut self, id: WindowId, workspace: u32) -> Option<()> {
        if !self.workspace_exists(workspace) {
            return None;
        }
        let w = self.windows.get_mut(&id)?;
        let old_workspace = w.workspace;
        let was_focused = w.focused;
        w.workspace = workspace;
        w.focused = false;
        // Clear focus pointer from origin workspace if this window was focused there.
        if was_focused && self.workspace_mut(old_workspace).focused_window == Some(id) {
            self.workspace_mut(old_workspace).focused_window = None;
        }
        self.emit(Event::WindowUpdated { id, update: WindowUpdate { workspace: Some(workspace), focused: Some(false), ..Default::default() } });
        Some(())
    }

    pub fn focus(&mut self, id: WindowId) -> Option<()> {
        let (ws, was_focused, was_minimized, mapped) = {
            let w = self.windows.get(&id)?;
            (w.workspace, w.focused, w.minimized, w.mapped)
        };
        // An unmapped window has no client to activate and must never be
        // handed the keyboard: refuse outright rather than remap-safely
        // no-op (decided -- see the task-14 brief's Step 2).
        if !mapped {
            return None;
        }
        if was_focused {
            // If already focused but minimized, unminimize and emit event.
            if was_minimized {
                let w = self.windows.get_mut(&id)?;
                w.minimized = false;
                self.emit(Event::WindowUpdated { id, update: WindowUpdate { minimized: Some(false), ..Default::default() } });
            }
            return Some(());
        }
        // Unminimize if needed.
        if was_minimized {
            let w = self.windows.get_mut(&id)?;
            w.minimized = false;
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
        // Emit combined event if we unminimized during focus change.
        if was_minimized {
            self.emit(Event::WindowUpdated { id, update: WindowUpdate { focused: Some(true), minimized: Some(false), ..Default::default() } });
        } else {
            self.emit(Event::WindowUpdated { id, update: WindowUpdate { focused: Some(true), ..Default::default() } });
        }
        // Update focus MRU.
        self.focus_mru.retain(|&wid| wid != id);
        self.focus_mru.insert(0, id);
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
        // Return windows ordered by focus MRU (most recent first).
        // Invariant: every window in self.windows is in focus_mru, so filter_map is safe.
        self.focus_mru.iter().filter_map(move |id| self.windows.get(id))
    }

    pub fn windows_in_workspace(&self, ws: u32) -> Vec<&Window> {
        self.windows.values().filter(|w| w.workspace == ws).collect()
    }

    /// The windows that should actually be drawn and hit-tested right now:
    /// the active workspace's non-minimized windows, topmost (most recently
    /// focused) first.
    ///
    /// Review finding I1: rendering and click-to-focus both consumed the raw
    /// `windows()` MRU list -- every window on *every* workspace, minimized
    /// ones included -- so switching workspaces changed nothing on screen and
    /// a click could focus (or close) a window belonging to an inactive
    /// workspace.
    pub fn visible_windows(&self) -> Vec<&Window> {
        self.windows().filter(|w| self.is_visible(w)).collect()
    }

    /// Whether `w` belongs to the active workspace, isn't minimized, and is
    /// mapped (see `visible_windows`).
    pub fn is_visible(&self, w: &Window) -> bool {
        w.workspace == self.active_workspace && !w.minimized && w.mapped
    }

    /// Same predicate as `is_visible`, by id (`false` for an unknown id).
    pub fn is_visible_id(&self, id: WindowId) -> bool {
        self.get(id).is_some_and(|w| self.is_visible(w))
    }

    /// The topmost visible window containing `point` (output logical
    /// coordinates), i.e. what a click at that point acts on. Used by the
    /// backend's click-to-focus path (review finding I1).
    pub fn window_at(&self, point: (i32, i32)) -> Option<&Window> {
        self.visible_windows().into_iter().find(|w| w.geometry.contains(point.0, point.1))
    }

    /// Focus the most-recently-focused non-minimized window on `ws`, if any.
    /// Returns the id focused, or `None` when the workspace has nothing
    /// focusable (in which case its focus pointer is left cleared).
    ///
    /// Review finding I6: `set_workspace` cleared the *origin* workspace's
    /// focus pointer but never gave the destination one, so
    /// `MoveToWorkspace` (which then switches to that workspace) left
    /// `focused_window()` as `None` and the very next `close`/`fullscreen`/
    /// `snap` action silently no-opped.
    pub fn focus_mru_in_workspace(&mut self, ws: u32) -> Option<WindowId> {
        let candidate = self
            .focus_mru
            .iter()
            .copied()
            .find(|id| self.windows.get(id).is_some_and(|w| w.workspace == ws && !w.minimized && w.mapped))?;
        self.focus(candidate)?;
        Some(candidate)
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
            .filter(|w| !w.minimized && w.mapped)
            .map(|w| w.id)
            .collect()
    }

    /// The id the next `add_window` call will assign.
    pub fn next_id(&self) -> u32 {
        self.next_id
    }

    /// Raise the id counter to at least `min_next_id`, never lowering it.
    /// Used by `State::apply_config` (Task 11 review #3): that method
    /// discards all windows and rebuilds a fresh `WindowManager`, but must
    /// not let the fresh instance start reissuing ids from 1 -- a shell that
    /// hasn't yet processed the `WindowClosed` events for the old windows
    /// could otherwise see a brand-new window claim an id it still believes
    /// is live.
    pub fn raise_id_floor(&mut self, min_next_id: u32) {
        if min_next_id > self.next_id {
            self.next_id = min_next_id;
        }
    }

    /// The current snapshot sequence number (`Snapshot::seq`).
    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// Raise the sequence counter to at least `min_seq`, never lowering it.
    /// Same rationale as `raise_id_floor`: a fresh `WindowManager` built by
    /// `State::apply_config` starts `seq` back at 0, which would make
    /// `snapshot().seq` go backwards across a reload -- a subscriber
    /// comparing sequence numbers to detect missed updates would wrongly
    /// conclude nothing changed (or that time ran backwards).
    pub fn raise_seq_floor(&mut self, min_seq: u64) {
        if min_seq > self.seq {
            self.seq = min_seq;
        }
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

    const GEO: Rectangle = Rectangle { x: 0, y: 0, width: 640, height: 400 };

    #[test]
    fn add_focuses_window_and_emits_opened() {
        let mut m = mgr();
        let id = m.add_window("app", "title", 1, GEO);
        assert!(m.get(id).unwrap().focused);
        assert!(matches!(m.pending_events.first(), Some(SeqEvent { event: Event::WindowOpened(_), .. })));
    }

    #[test]
    fn focus_unfocuses_previous() {
        let mut m = mgr();
        let a = m.add_window("a", "a", 1, GEO);
        let b = m.add_window("b", "b", 2, GEO);
        assert!(!m.get(a).unwrap().focused);
        assert!(m.get(b).unwrap().focused);
    }

    #[test]
    fn move_to_workspace_keeps_focus_valid() {
        let mut m = mgr();
        let a = m.add_window("a", "a", 1, GEO);
        m.set_workspace(a, 1).unwrap();
        assert_eq!(m.get(a).unwrap().workspace, 1);
        assert!(!m.get(a).unwrap().focused);
        // Verify focused_window() is cleared after move (stale pointer check).
        assert!(m.focused_window().is_none());
    }

    #[test]
    fn remove_clears_focus_to_none() {
        let mut m = mgr();
        let a = m.add_window("a", "a", 1, GEO);
        m.remove_window(a).unwrap();
        assert!(m.focused_window().is_none());
    }

    #[test]
    fn snapshot_matches_state() {
        let mut m = mgr();
        let id = m.add_window("app", "t", 7, GEO);
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
        let a = m.add_window("a", "a", 1, GEO);
        let b = m.add_window("b", "b", 2, GEO);
        m.set_minimized(a, true).unwrap();
        assert_eq!(m.alt_tab_entries(), vec![b]);
    }

    #[test]
    fn focus_minimized_already_focused_emits_event() {
        let mut m = mgr();
        let a = m.add_window("a", "a", 1, GEO);
        // Minimize while focused.
        m.set_minimized(a, true).unwrap();
        m.pending_events.clear();
        // Focus again (already focused, but minimized).
        m.focus(a).unwrap();
        // Should emit an event for the minimized state change.
        assert!(!m.pending_events.is_empty());
        assert!(matches!(m.pending_events.first(), Some(SeqEvent { event: Event::WindowUpdated { .. }, .. })));
        assert!(!m.get(a).unwrap().minimized);
    }

    #[test]
    fn windows_ordered_by_mru() {
        let mut m = mgr();
        let a = m.add_window("a", "a", 1, GEO);
        let b = m.add_window("b", "b", 2, GEO);
        let c = m.add_window("c", "c", 3, GEO);
        // Current order (MRU first): c, b, a
        let ids: Vec<_> = m.windows().map(|w| w.id).collect();
        assert_eq!(ids, vec![c, b, a]);
        // Focus a, should move to front.
        m.focus(a).unwrap();
        let ids: Vec<_> = m.windows().map(|w| w.id).collect();
        assert_eq!(ids, vec![a, c, b]);
    }

    #[test]
    fn remove_window_prunes_focus_mru() {
        let mut m = mgr();
        let a = m.add_window("a", "a", 1, GEO);
        let b = m.add_window("b", "b", 2, GEO);
        let c = m.add_window("c", "c", 3, GEO);
        // MRU order: c, b, a
        assert_eq!(m.windows().map(|w| w.id).collect::<Vec<_>>(), vec![c, b, a]);
        // Remove b (middle of MRU).
        m.remove_window(b).unwrap();
        // Should have c, a in MRU order.
        assert_eq!(m.windows().map(|w| w.id).collect::<Vec<_>>(), vec![c, a]);
        // Add new window d, should be MRU first.
        let d = m.add_window("d", "d", 4, GEO);
        assert_eq!(m.windows().map(|w| w.id).collect::<Vec<_>>(), vec![d, c, a]);
        // Verify b is not in the state at all.
        assert!(m.get(b).is_none());
    }

    // --- Final-review fix-round tests ---

    /// M2: a manager built from a config with no workspace names must not
    /// leave a reachable index panic in `workspace_mut`.
    #[test]
    fn empty_workspace_list_falls_back_to_one_workspace() {
        let mut m = WindowManager::new(vec![]);
        assert_eq!(m.workspace_info().len(), 1);
        let id = m.add_window("a", "a", 1, GEO);
        assert!(m.get(id).unwrap().focused);
    }

    /// I1: only the active workspace's non-minimized windows are drawn and
    /// hit-tested, topmost (MRU) first.
    #[test]
    fn visible_windows_filters_by_workspace_and_minimized() {
        let mut m = mgr();
        let a = m.add_window("a", "a", 1, GEO);
        let b = m.add_window("b", "b", 2, GEO);
        let c = m.add_window("c", "c", 3, GEO);
        m.set_workspace(c, 1).unwrap();
        m.set_minimized(b, true).unwrap();

        assert_eq!(m.visible_windows().iter().map(|w| w.id).collect::<Vec<_>>(), vec![a]);
        assert!(m.is_visible_id(a));
        assert!(!m.is_visible_id(b), "minimized windows are not drawn or clickable");
        assert!(!m.is_visible_id(c), "another workspace's windows are not drawn or clickable");

        m.set_active_workspace(1);
        assert_eq!(m.visible_windows().iter().map(|w| w.id).collect::<Vec<_>>(), vec![c]);
    }

    /// I1: a click must never land on a window from an inactive workspace,
    /// even when its geometry contains the point.
    #[test]
    fn window_at_only_hits_visible_windows() {
        let mut m = mgr();
        let a = m.add_window("a", "a", 1, GEO);
        let b = m.add_window("b", "b", 2, GEO);
        // Both cover (10, 10); `b` is MRU head, so it wins while visible.
        assert_eq!(m.window_at((10, 10)).map(|w| w.id), Some(b));
        m.set_workspace(b, 1).unwrap();
        assert_eq!(m.window_at((10, 10)).map(|w| w.id), Some(a));
        m.set_minimized(a, true).unwrap();
        assert!(m.window_at((10, 10)).is_none());
        assert!(m.window_at((10_000, 10_000)).is_none());
    }

    /// I6: after a window is moved away, the workspace it lands on has a
    /// focusable head that the next action can act on.
    #[test]
    fn focus_mru_in_workspace_picks_head_and_skips_minimized() {
        let mut m = mgr();
        let a = m.add_window("a", "a", 1, GEO);
        let b = m.add_window("b", "b", 2, GEO);
        m.set_workspace(b, 1).unwrap();
        // Workspace 1 now holds only `b`, unfocused.
        assert_eq!(m.focus_mru_in_workspace(1), Some(b));
        assert!(m.get(b).unwrap().focused);

        // Minimized windows aren't focus candidates; an empty workspace
        // reports `None` rather than focusing something on another one.
        m.set_minimized(b, true).unwrap();
        assert_eq!(m.focus_mru_in_workspace(1), None);
        assert_eq!(m.focus_mru_in_workspace(0), Some(a));
    }

    /// I2: every queued event carries the seq its mutation produced, and
    /// those seqs are strictly increasing.
    #[test]
    fn pending_events_carry_increasing_seq() {
        let mut m = mgr();
        let a = m.add_window("a", "a", 1, GEO);
        m.set_title(a, "new".into()).unwrap();
        let seqs: Vec<u64> = m.pending_events.iter().map(|e| e.seq).collect();
        assert!(seqs.len() >= 2);
        assert!(seqs.windows(2).all(|w| w[1] > w[0]), "seqs must strictly increase: {seqs:?}");
        assert_eq!(*seqs.last().unwrap(), m.seq(), "the last queued event carries the current seq");
    }

    // --- Task 14: the model-level "unmapped" concept ---

    /// Unmapping a window keeps its row (title, geometry, etc. all
    /// untouched) but pulls it out of visibility and alt-tab candidacy;
    /// remapping restores both. The value not changing is a no-op.
    #[test]
    fn an_unmapped_window_leaves_visibility_and_alt_tab_but_keeps_its_row() {
        let mut m = WindowManager::new(vec!["1".into()]);
        let a = m.add_window("a", "a", 1, Rectangle { x: 0, y: 0, width: 10, height: 10 });
        let b = m.add_window("b", "b", 1, Rectangle { x: 0, y: 0, width: 10, height: 10 });
        assert_eq!(m.set_mapped(b, false), Some(()));
        assert!(!m.get(b).expect("row kept").mapped);
        assert!(!m.alt_tab_entries().contains(&b));
        assert!(m.visible_windows().iter().all(|w| w.id != b));
        assert_eq!(m.set_mapped(b, false), None, "unchanged value is a no-op");
        assert_eq!(m.set_mapped(b, true), Some(()));
        assert!(m.alt_tab_entries().contains(&b));
        let _ = a;
    }

    /// The milestone's one sanctioned addition to the exactly-one-event-
    /// per-mutation invariant: `set_mapped` emits exactly one `WindowUpdated`.
    #[test]
    fn set_mapped_emits_exactly_one_window_updated() {
        let mut m = WindowManager::new(vec!["1".into()]);
        let a = m.add_window("a", "a", 1, Rectangle { x: 0, y: 0, width: 10, height: 10 });
        let before = m.seq();
        m.set_mapped(a, false);
        assert_eq!(m.seq(), before + 1, "exactly one emission");
    }

    /// Step 2's focus-refusal decision: focusing an unmapped window is
    /// refused outright, not a remap-safe no-op.
    #[test]
    fn focus_refuses_an_unmapped_window() {
        let mut m = WindowManager::new(vec!["1".into()]);
        let a = m.add_window("a", "a", 1, GEO);
        m.set_mapped(a, false).unwrap();
        assert_eq!(m.focus(a), None, "an unmapped window must never gain focus");
    }
}
