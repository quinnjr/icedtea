use serde::{Deserialize, Serialize};
use zvariant::Type;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Type)]
#[zvariant(signature = "u")]
pub struct WindowId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[zvariant(signature = "(iiii)")]
pub struct Rectangle {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Rectangle {
    pub fn contains(self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct WindowInfo {
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct WorkspaceInfo {
    pub id: u32,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct Snapshot {
    pub seq: u64,
    pub windows: Vec<WindowInfo>,
    pub workspaces: Vec<WorkspaceInfo>,
    pub active_workspace: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Type)]
pub struct WindowUpdate {
    pub title: Option<String>,
    pub geometry: Option<Rectangle>,
    pub workspace: Option<u32>,
    pub maximized: Option<bool>,
    pub minimized: Option<bool>,
    pub fullscreen: Option<bool>,
    pub focused: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct AltTabState {
    pub active: bool,
    pub entries: Vec<WindowId>,
    pub index: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct Palette {
    pub background: String,
    pub foreground: String,
    pub accent: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct Appearance {
    pub bar_position: String,
    pub bar_height: i32,
    pub corner_radius: i32,
    pub snap_gap: i32,
    pub palette: Palette,
    pub wallpaper: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_window() -> WindowInfo {
        WindowInfo {
            id: WindowId(1),
            app_id: "org.gnome.Calculator".into(),
            title: "Calculator".into(),
            pid: 1234,
            workspace: 0,
            geometry: Rectangle { x: 100, y: 100, width: 400, height: 300 },
            maximized: false,
            minimized: false,
            fullscreen: false,
            focused: true,
        }
    }

    fn sample_snapshot() -> Snapshot {
        Snapshot {
            seq: 7,
            windows: vec![sample_window()],
            workspaces: vec![
                WorkspaceInfo { id: 0, name: "1".into() },
                WorkspaceInfo { id: 1, name: "2".into() },
            ],
            active_workspace: 0,
        }
    }

    #[test]
    fn snapshot_json_round_trip() {
        let s = sample_snapshot();
        let json = serde_json::to_string(&s).unwrap();
        let back: Snapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn window_info_zvariant_round_trip() {
        let w = sample_window();
        let ctxt = zvariant::serialized::Context::new_dbus(zvariant::LE, 0);
        let encoded = zvariant::to_bytes(ctxt, &w).unwrap();
        let decoded: WindowInfo = encoded.deserialize().unwrap().0;
        assert_eq!(w, decoded);
    }

    #[test]
    fn window_update_default_is_all_none() {
        let u = WindowUpdate::default();
        assert!(u.title.is_none() && u.geometry.is_none() && u.workspace.is_none());
        assert!(u.maximized.is_none() && u.minimized.is_none() && u.fullscreen.is_none());
        assert!(u.focused.is_none());
    }
}
