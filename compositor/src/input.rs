use std::collections::HashMap;

use icedtea_contract::{Rectangle, WindowId};
use smithay::input::keyboard::xkb;

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

/// The modifier tokens a `KeyCombo` may name.
pub const MODIFIER_TOKENS: [&str; 4] = ["SUPER", "CTRL", "ALT", "SHIFT"];

/// Resolve a binding's key name (`"KEY_<xkb keysym name>"`, e.g. `"KEY_q"`,
/// `"KEY_Return"`, `"KEY_F5"`, `"KEY_bracketleft"`) to its keysym, or `0`
/// when the name names no keysym at all.
///
/// Review finding I4: this used to be a hand-written table of nine keys plus
/// the digits `1..=9`; *everything* else -- every letter but `q`/`f`/`r`,
/// every function key, every punctuation key -- resolved to `0`, so a
/// perfectly ordinary custom binding like `SUPER+a` could never fire. It
/// also logged a `tracing::warn!` on the miss, and `match_action` calls this
/// once per binding per key press, so a single unresolvable binding produced
/// a warn line on *every* keystroke. Resolution now goes through xkb (the
/// same keysym database the input path itself uses, so names and runtime
/// keysyms cannot drift apart), and the diagnostics moved to
/// `validate_keybindings`, which runs once at config load/reload.
pub fn key_name_to_keysym(name: &str) -> u32 {
    let bare = name.strip_prefix("KEY_").unwrap_or(name);
    let sym = xkb::keysym_from_name(bare, xkb::KEYSYM_NO_FLAGS);
    if sym.raw() != xkb::keysyms::KEY_NoSymbol {
        return sym.raw();
    }
    // Second pass, per xkbcommon's own recommendation: a case-insensitive
    // lookup catches `"KEY_TAB"`/`"KEY_return"`-style spellings. Resolving
    // only on this pass is accepted silently -- `validate_keybindings` calls
    // this same function and reports a binding only when *both* passes come
    // back `KEY_NoSymbol`, i.e. when the name matches no keysym at all.
    xkb::keysym_from_name(bare, xkb::KEYSYM_CASE_INSENSITIVE).raw()
}

/// Inverse of [`key_name_to_keysym`]; used for D-Bus/debug output.
pub fn keysym_to_key_name(keysym: u32) -> String {
    let name = xkb::keysym_get_name(xkb::Keysym::new(keysym));
    if name.is_empty() {
        return format!("KEY_{keysym}");
    }
    format!("KEY_{name}")
}

/// Check every configured binding once, returning a human-readable problem
/// description per unusable binding: an unrecognized modifier token, or a
/// key name that names no keysym.
///
/// Review finding I4: `match_action` used to warn from inside the per-event
/// lookup, so a typo'd binding spammed the log on every single key press.
/// `State` calls this at config load and on every reload instead, so each
/// problem is reported exactly once per config.
pub fn validate_keybindings(bindings: &HashMap<String, crate::config_combo::KeyCombo>) -> Vec<String> {
    let mut problems: Vec<String> = Vec::new();
    for (action, combo) in bindings {
        for m in &combo.modifiers {
            if !MODIFIER_TOKENS.contains(&m.as_str()) {
                problems.push(format!(
                    "binding for action {action:?} names unrecognized modifier {m:?} \
                     (expected one of {MODIFIER_TOKENS:?}); it can never fire"
                ));
            }
        }
        if key_name_to_keysym(&combo.key) == xkb::keysyms::KEY_NoSymbol {
            problems.push(format!(
                "binding for action {action:?} names unknown key {:?}; it can never fire",
                combo.key
            ));
        }
    }
    problems.sort();
    problems
}

/// Log whatever [`validate_keybindings`] found, once.
pub fn warn_about_keybindings(bindings: &HashMap<String, crate::config_combo::KeyCombo>) {
    for problem in validate_keybindings(bindings) {
        tracing::warn!("{problem}");
    }
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
                _ => {
                    // Review finding I4: the `tracing::warn!` that used to
                    // live here ran once per binding per key press.
                    // Diagnostics now come from `validate_keybindings`, at
                    // config load/reload time.
                    //
                    // Route an unrecognized token (typo, wrong case, etc.)
                    // to a sentinel bit outside the four defined flags
                    // instead of `Modifiers::empty()`. Runtime `mods` values
                    // are always built from just SUPER/CTRL/ALT/SHIFT, so
                    // this permanently prevents `wanted == mods` from
                    // matching -- a mistyped modifier makes the binding
                    // unreachable (loud, once wired to real input logging),
                    // rather than silently downgrading it to "no modifier
                    // required" and matching a bare keypress or the wrong
                    // combo.
                    Modifiers::from_bits_retain(u32::MAX)
                }
            }
        });
        let wanted_sym = key_name_to_keysym(&combo.key);
        // An unresolvable key name must never match: without this guard a
        // typo'd binding would fire for any key press whose keysym also
        // failed to resolve (both being `NoSymbol`).
        if wanted_sym == xkb::keysyms::KEY_NoSymbol {
            continue;
        }
        if wanted == mods && wanted_sym == keysym {
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

    /// The entry list captured by `start()`, stable for the session's
    /// duration (callers should read this instead of recomputing a fresh
    /// window list on every step, so `index` always stays in bounds).
    pub fn entries(&self) -> &[WindowId] {
        &self.entries
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

    /// The window currently being dragged, if a drag is in progress.
    /// `end()`/`cancel()` consume `window_id` internally (they need to reset
    /// it) without returning it, so callers that need "which window was this
    /// drag for" (to apply the move/snap to the right window) must read it
    /// before calling `end()`.
    pub fn window_id(&self) -> Option<WindowId> {
        self.window_id
    }

    /// The pointer-to-window-origin offset captured at `begin()`, used to
    /// compute the window's new top-left from the pointer position on a
    /// plain (non-snapped) move.
    pub fn grab_offset(&self) -> (i32, i32) {
        self.grab_offset
    }

    // Alias named per the brief's Interfaces section (`drag_result()` for
    // preview state), kept alongside `preview_zone()` -- the Step 1 sample
    // code and its tests call the latter, so both names are exposed rather
    // than picking one and breaking the other half of the brief.
    pub fn drag_result(&self) -> Option<SnapZone> {
        self.preview_zone
    }

    pub fn end(&mut self) -> DragResult {
        match (self.window_id.take(), self.preview_zone.take()) {
            (Some(_), Some(zone)) => DragResult::Snapped(zone),
            (Some(_), None) => DragResult::Moved,
            _ => DragResult::Restored,
        }
    }

    // Reachable "abort mid-drag" transition (e.g. Escape during a drag):
    // discards any in-flight window/preview state and always reports
    // `Restored`, regardless of whether a snap preview was staged. Without
    // this, `end()` can only produce `Restored` when `begin()` was never
    // called (or a prior `end()`/`cancel()` already consumed the drag), so
    // a drag that genuinely started had no way to signal "put it back."
    pub fn cancel(&mut self) -> DragResult {
        self.window_id = None;
        self.preview_zone = None;
        DragResult::Restored
    }
}

/// Which window edges an interactive resize is dragging.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ResizeEdges {
    pub top: bool,
    pub bottom: bool,
    pub left: bool,
    pub right: bool,
}

impl ResizeEdges {
    pub fn is_empty(self) -> bool {
        !(self.top || self.bottom || self.left || self.right)
    }
}

/// Smallest interactive-resize result, so a client can't be configured to a
/// zero (or negative) size by dragging past the opposite edge.
pub const MIN_WINDOW_SIZE: (i32, i32) = (160, 80);

/// Pointer-driven interactive resize, the counterpart of [`DragMachine`].
/// Started by the xdg `resize_request` handler (the client asks for it while
/// already holding the button down) and stepped by the same pointer
/// motion/release path the drag machine uses.
pub struct ResizeMachine {
    window_id: Option<WindowId>,
    edges: ResizeEdges,
    start_geometry: Rectangle,
    start_pointer: (i32, i32),
}

impl Default for ResizeMachine {
    fn default() -> Self {
        Self::new()
    }
}

impl ResizeMachine {
    pub fn new() -> Self {
        Self {
            window_id: None,
            edges: ResizeEdges::default(),
            start_geometry: Rectangle { x: 0, y: 0, width: 0, height: 0 },
            start_pointer: (0, 0),
        }
    }

    pub fn begin(&mut self, window_id: WindowId, edges: ResizeEdges, geometry: Rectangle, pointer: (i32, i32)) {
        self.window_id = Some(window_id);
        self.edges = edges;
        self.start_geometry = geometry;
        self.start_pointer = pointer;
    }

    pub fn window_id(&self) -> Option<WindowId> {
        self.window_id
    }

    /// The geometry the window should have with the pointer at `pointer`,
    /// clamped to [`MIN_WINDOW_SIZE`]. `None` when no resize is in progress.
    pub fn geometry_for(&self, pointer: (i32, i32)) -> Option<Rectangle> {
        self.window_id?;
        let (dx, dy) = (pointer.0 - self.start_pointer.0, pointer.1 - self.start_pointer.1);
        let g = self.start_geometry;
        let (mut x, mut y, mut width, mut height) = (g.x, g.y, g.width, g.height);
        if self.edges.right {
            width = (g.width + dx).max(MIN_WINDOW_SIZE.0);
        }
        if self.edges.left {
            width = (g.width - dx).max(MIN_WINDOW_SIZE.0);
            x = g.x + g.width - width;
        }
        if self.edges.bottom {
            height = (g.height + dy).max(MIN_WINDOW_SIZE.1);
        }
        if self.edges.top {
            height = (g.height - dy).max(MIN_WINDOW_SIZE.1);
            y = g.y + g.height - height;
        }
        Some(Rectangle { x, y, width, height })
    }

    /// End the resize, returning the window it applied to (if any).
    pub fn end(&mut self) -> Option<WindowId> {
        self.edges = ResizeEdges::default();
        self.window_id.take()
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
    fn unknown_modifier_token_never_matches() {
        let mut m = HashMap::new();
        // A typo'd/mis-cased modifier ("Sooper" instead of "SUPER") must not
        // silently fold to `Modifiers::empty()` -- that would make this
        // binding fire on a bare `KEY_q` with no modifiers at all, or on
        // any other combo that happens to share `wanted == mods`.
        m.insert("typo".into(), KeyCombo { modifiers: vec!["Sooper".into()], key: "KEY_q".into() });
        assert_eq!(match_action(&m, Modifiers::empty(), 0x71), None);
        assert_eq!(match_action(&m, Modifiers::SUPER, 0x71), None);
    }

    // --- Final-review fix-round tests (I4) ---

    /// I4: the nine-key hand table is gone -- every xkb keysym name is
    /// bindable now -- while the names the old table did know keep resolving
    /// to exactly the same keysyms (so no existing config silently changes
    /// meaning).
    #[test]
    fn key_names_resolve_through_xkb_without_regressing_the_old_table() {
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
            ("KEY_1", 0x31),
            ("KEY_9", 0x39),
        ] {
            assert_eq!(key_name_to_keysym(name), sym, "{name} must keep its keysym");
        }
        // Newly bindable: the other 23 letters, function keys, punctuation.
        assert_eq!(key_name_to_keysym("KEY_a"), 0x61);
        assert_eq!(key_name_to_keysym("KEY_z"), 0x7a);
        assert_eq!(key_name_to_keysym("KEY_F5"), 0xffc2);
        assert_eq!(key_name_to_keysym("KEY_space"), 0x20);
        assert_eq!(key_name_to_keysym("KEY_bracketleft"), 0x5b);
        // Still unresolvable, still reported as `NoSymbol`.
        assert_eq!(key_name_to_keysym("KEY_definitely_not_a_key"), xkb::keysyms::KEY_NoSymbol);
    }

    #[test]
    fn key_name_round_trips_through_keysym_to_key_name() {
        for name in ["KEY_Return", "KEY_Tab", "KEY_q", "KEY_a", "KEY_F5", "KEY_1", "KEY_space"] {
            assert_eq!(keysym_to_key_name(key_name_to_keysym(name)), name);
        }
    }

    /// I4: a letter binding that could never fire before now dispatches.
    #[test]
    fn letter_bindings_can_match() {
        let mut m = HashMap::new();
        m.insert("spawn:menu".into(), KeyCombo { modifiers: vec!["SUPER".into()], key: "KEY_a".into() });
        assert_eq!(match_action(&m, Modifiers::SUPER, 0x61), Some("spawn:menu".into()));
    }

    /// I4: problems are reported once, by an explicit validation pass, not
    /// from inside the per-key-press lookup.
    #[test]
    fn validate_keybindings_reports_bad_modifier_and_bad_key() {
        let mut m = HashMap::new();
        m.insert("good".into(), KeyCombo { modifiers: vec!["SUPER".into()], key: "KEY_q".into() });
        m.insert("bad_mod".into(), KeyCombo { modifiers: vec!["Sooper".into()], key: "KEY_q".into() });
        m.insert("bad_key".into(), KeyCombo { modifiers: vec!["SUPER".into()], key: "KEY_nope".into() });
        let problems = validate_keybindings(&m);
        assert_eq!(problems.len(), 2, "one problem per unusable binding: {problems:?}");
        assert!(problems.iter().any(|p| p.contains("bad_mod") && p.contains("Sooper")));
        assert!(problems.iter().any(|p| p.contains("bad_key") && p.contains("KEY_nope")));
        assert!(validate_keybindings(&bindings()).is_empty(), "the default-shaped bindings are clean");
    }

    /// I4: an unresolvable key name must not match a key press whose keysym
    /// also failed to resolve (both `NoSymbol`).
    #[test]
    fn unresolvable_key_name_never_matches() {
        let mut m = HashMap::new();
        m.insert("typo".into(), KeyCombo { modifiers: vec!["SUPER".into()], key: "KEY_nope".into() });
        assert_eq!(match_action(&m, Modifiers::SUPER, xkb::keysyms::KEY_NoSymbol), None);
    }

    // --- Interactive resize (C1 / ledger item 15: xdg `resize_request`) ---

    #[test]
    fn resize_from_bottom_right_grows_without_moving_the_origin() {
        let mut m = ResizeMachine::new();
        let geo = Rectangle { x: 100, y: 100, width: 400, height: 300 };
        m.begin(WindowId(1), ResizeEdges { bottom: true, right: true, ..Default::default() }, geo, (500, 400));
        assert_eq!(
            m.geometry_for((550, 450)),
            Some(Rectangle { x: 100, y: 100, width: 450, height: 350 })
        );
        assert_eq!(m.end(), Some(WindowId(1)));
        assert_eq!(m.geometry_for((550, 450)), None, "no resize in progress after end()");
    }

    #[test]
    fn resize_from_top_left_moves_the_origin_and_clamps_to_minimum() {
        let mut m = ResizeMachine::new();
        let geo = Rectangle { x: 100, y: 100, width: 400, height: 300 };
        m.begin(WindowId(1), ResizeEdges { top: true, left: true, ..Default::default() }, geo, (100, 100));
        assert_eq!(m.geometry_for((150, 140)), Some(Rectangle { x: 150, y: 140, width: 350, height: 260 }));
        // Dragging past the opposite edge clamps instead of inverting.
        let clamped = m.geometry_for((10_000, 10_000)).unwrap();
        assert_eq!((clamped.width, clamped.height), MIN_WINDOW_SIZE);
        assert_eq!(clamped.x + clamped.width, geo.x + geo.width, "the far edge stays put");
        assert_eq!(clamped.y + clamped.height, geo.y + geo.height);
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

    #[test]
    fn drag_cancel_restores() {
        let output = Rectangle { x: 0, y: 0, width: 1000, height: 800 };
        let mut m = DragMachine::new();
        m.begin(WindowId(1), (10, 10));
        m.motion((2, 400), output, 8);
        assert_eq!(m.preview_zone(), Some(SnapZone::Left));
        assert!(matches!(m.cancel(), DragResult::Restored));
        // Cancel is one-shot too: the staged snap preview is discarded, not
        // just skipped once.
        assert_eq!(m.preview_zone(), None);
        assert!(matches!(m.end(), DragResult::Restored));
    }
}
