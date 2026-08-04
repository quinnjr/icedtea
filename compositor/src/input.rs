use std::collections::HashMap;

use icedtea_contract::{Rectangle, WindowId};

use crate::layout::{snap_zone_for_point, SnapZone};

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Modifiers: u32 {
        const SUPER = 1;
        const CTRL = 2;
        const ALT = 4;
        const SHIFT = 8;
    }
}

pub fn key_name_to_keysym(name: &str) -> u32 {
    match name {
        "KEY_Return" => 0xff0d,
        "KEY_Tab" => 0xff09,
        "KEY_Left" => 0xff51,
        "KEY_Up" => 0xff52,
        "KEY_Right" => 0xff53,
        "KEY_Down" => 0xff54,
        "KEY_q" => 0x71,
        "KEY_f" => 0x66,
        "KEY_r" => 0x72,
        _ => {
            if let Some(Ok(digit)) = name.strip_prefix("KEY_").map(|rest| rest.parse::<u32>()) {
                return 0x30 + digit; // KEY_1..KEY_9
            }
            tracing::warn!("unknown key name {name}");
            0
        }
    }
}

pub fn keysym_to_key_name(keysym: u32) -> String {
    // Inverse of key_name_to_keysym; used for DBus/debug output.
    for (name, sym) in [
        ("KEY_Return", 0xff0d),
        ("KEY_Tab", 0xff09),
        ("KEY_Left", 0xff51),
        ("KEY_Up", 0xff52),
        ("KEY_Right", 0xff53),
        ("KEY_Down", 0xff54),
        ("KEY_q", 0x71),
        ("KEY_f", 0x66),
        ("KEY_r", 0x72),
    ] {
        if sym == keysym {
            return name.to_string();
        }
    }
    if (0x31..=0x39).contains(&keysym) {
        return format!("KEY_{}", keysym - 0x30);
    }
    format!("KEY_{keysym}")
}

// NOTE (brief deviation): the brief's sample `match_action` body has a
// `use crate::config_combo::KeyCombo;` inside the function even though the
// parameter is already spelled with the fully-qualified
// `crate::config_combo::KeyCombo` path -- that import is never referenced by
// its short name and is dead weight that `-D warnings` (unused_imports)
// rejects. Dropped it; behavior is unchanged.
pub fn match_action(
    bindings: &HashMap<String, crate::config_combo::KeyCombo>,
    mods: Modifiers,
    keysym: u32,
) -> Option<String> {
    for (action, combo) in bindings {
        let wanted = combo.modifiers.iter().fold(Modifiers::empty(), |acc, m| {
            acc | match m.as_str() {
                "SUPER" => Modifiers::SUPER,
                "CTRL" => Modifiers::CTRL,
                "ALT" => Modifiers::ALT,
                "SHIFT" => Modifiers::SHIFT,
                _ => Modifiers::empty(),
            }
        });
        if wanted == mods && key_name_to_keysym(&combo.key) == keysym {
            return Some(action.clone());
        }
    }
    None
}

pub struct AltTabMachine {
    entries: Vec<WindowId>,
    index: usize,
    active: bool,
}

impl Default for AltTabMachine {
    fn default() -> Self {
        Self::new()
    }
}

impl AltTabMachine {
    pub fn new() -> Self {
        Self { entries: Vec::new(), index: 0, active: false }
    }

    pub fn start(&mut self, entries: Vec<WindowId>) {
        self.entries = entries;
        self.index = 0;
        self.active = true;
    }

    pub fn step(&mut self, next: bool) -> usize {
        if self.entries.is_empty() {
            return self.index;
        }
        if next {
            self.index = (self.index + 1) % self.entries.len();
        } else {
            self.index = if self.index == 0 { self.entries.len() - 1 } else { self.index - 1 };
        }
        self.index
    }

    pub fn end(&mut self) -> Option<WindowId> {
        self.active = false;
        self.entries.get(self.index).copied()
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn index(&self) -> usize {
        self.index
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragResult {
    Moved,
    Snapped(SnapZone),
    Restored,
}

pub struct DragMachine {
    window_id: Option<WindowId>,
    grab_offset: (i32, i32),
    preview_zone: Option<SnapZone>,
}

impl Default for DragMachine {
    fn default() -> Self {
        Self::new()
    }
}

impl DragMachine {
    pub fn new() -> Self {
        Self { window_id: None, grab_offset: (0, 0), preview_zone: None }
    }

    pub fn begin(&mut self, window_id: WindowId, grab_offset: (i32, i32)) {
        self.window_id = Some(window_id);
        self.grab_offset = grab_offset;
        self.preview_zone = None;
    }

    pub fn motion(&mut self, pointer: (i32, i32), output: Rectangle, threshold: i32) {
        self.preview_zone = snap_zone_for_point(output, pointer, threshold);
    }

    pub fn preview_zone(&self) -> Option<SnapZone> {
        self.preview_zone
    }

    // Alias named per the brief's Interfaces section (`drag_result()` for
    // preview state), kept alongside `preview_zone()` -- the Step 1 sample
    // code and its tests call the latter, so both names are exposed rather
    // than picking one and breaking the other half of the brief.
    pub fn drag_result(&self) -> Option<SnapZone> {
        self.preview_zone
    }

    pub fn grab_offset(&self) -> (i32, i32) {
        self.grab_offset
    }

    pub fn end(&mut self) -> DragResult {
        match (self.window_id.take(), self.preview_zone.take()) {
            (Some(_), Some(zone)) => DragResult::Snapped(zone),
            (Some(_), None) => DragResult::Moved,
            _ => DragResult::Restored,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config_combo::KeyCombo;

    fn bindings() -> HashMap<String, KeyCombo> {
        let mut m = HashMap::new();
        m.insert("close".into(), KeyCombo { modifiers: vec!["SUPER".into()], key: "KEY_q".into() });
        m.insert(
            "cycle:alt_tab".into(),
            KeyCombo { modifiers: vec!["SUPER".into()], key: "KEY_Tab".into() },
        );
        m
    }

    #[test]
    fn exact_modifier_match() {
        assert_eq!(match_action(&bindings(), Modifiers::SUPER, 0x71), Some("close".into()));
        assert_eq!(match_action(&bindings(), Modifiers::SUPER | Modifiers::SHIFT, 0x71), None);
    }

    #[test]
    fn wrong_keysym_does_not_match() {
        assert_eq!(match_action(&bindings(), Modifiers::SUPER, 0x66), None);
    }

    #[test]
    fn alt_tab_wraps_around() {
        let mut m = AltTabMachine::new();
        m.start(vec![WindowId(1), WindowId(2), WindowId(3)]);
        assert_eq!(m.step(true), 1);
        assert_eq!(m.step(true), 2);
        assert_eq!(m.step(true), 0);
        assert_eq!(m.step(false), 2);
        assert_eq!(m.end(), Some(WindowId(3)));
        assert!(!m.is_active());
    }

    #[test]
    fn drag_to_edge_snaps() {
        let output = Rectangle { x: 0, y: 0, width: 1000, height: 800 };
        let mut m = DragMachine::new();
        m.begin(WindowId(1), (10, 10));
        m.motion((2, 400), output, 8);
        assert_eq!(m.preview_zone(), Some(SnapZone::Left));
        assert!(matches!(m.end(), DragResult::Snapped(SnapZone::Left)));
    }

    #[test]
    fn drag_center_moves() {
        let output = Rectangle { x: 0, y: 0, width: 1000, height: 800 };
        let mut m = DragMachine::new();
        m.begin(WindowId(1), (10, 10));
        m.motion((500, 400), output, 8);
        assert_eq!(m.preview_zone(), None);
        assert!(matches!(m.end(), DragResult::Moved));
    }
}
