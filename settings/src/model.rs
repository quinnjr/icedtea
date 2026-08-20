//! GTK-free core of the settings app: working-copy state, the compositor's
//! `apply` path, keybinding-capture translation, and validation helpers.
//!
//! Nothing here touches GTK -- the (GTK) view converts `gdk::ModifierType`
//! into [`CaptureMods`] and calls into this module, so the logic here can be
//! unit-tested without a display server.

use std::path::Path;

use icedtea_config::{keysym_to_key_name, load_or_default, open, Config, KeyCombo, MODIFIER_TOKENS};

/// The working-copy state the settings UI edits: `working` is what the
/// widgets are bound to, `saved` is a snapshot of what's actually on disk
/// (as of the last successful load/apply/revert), and [`Model::is_dirty`]
/// compares the two so the view can show an unsaved-changes indicator.
#[derive(Debug, Clone)]
pub struct Model {
    pub working: Config,
    pub saved: Config,
}

impl Model {
    /// Load `db_path` (falling back to defaults per
    /// `icedtea_config::load_or_default`) into a fresh working copy whose
    /// `saved` snapshot matches, so a freshly-loaded `Model` is never dirty.
    pub fn load(db_path: &Path) -> Self {
        let cfg = load_or_default(db_path);
        Self { working: cfg.clone(), saved: cfg }
    }

    /// Whether the working copy has diverged from the last-known-saved
    /// snapshot.
    pub fn is_dirty(&self) -> bool {
        self.working != self.saved
    }

    /// Discard any unsaved edits: reload `db_path` into both `working` and
    /// `saved`, so `is_dirty()` is `false` immediately after.
    pub fn revert(&mut self, db_path: &Path) {
        let cfg = load_or_default(db_path);
        self.working = cfg.clone();
        self.saved = cfg;
    }
}

/// Persist `cfg` to `db_path`. On success the caller is expected to snapshot
/// `saved = working` (this function only writes -- it doesn't own `Model`,
/// so it can't do that itself).
///
/// The store is opened through [`icedtea_config::open`] (not
/// `redb::Database::create` directly) so its `catch_unwind` guard + parent
/// `create_dir_all` apply: a corrupt config file returns `Err` here ("Failed
/// to save") instead of tripping an internal redb `assert!` and panicking the
/// GTK app.
///
/// `cfg.displays` is deliberately **not** written from the caller's working
/// copy. The Displays page never routes through this working-copy `Config`:
/// it applies layouts out-of-band over `zwlr_output_management_v1`, and the
/// compositor persists them to the `displays` table on its own side. The
/// `Config` the settings app loaded at startup carries a snapshot of `displays`
/// that is already stale the moment the compositor writes a new layout, so
/// saving the whole working copy here would clobber the compositor-owned
/// layout on any unrelated Apply (an accent-color change, say). To keep the
/// persisted displays authoritative we reload the current on-disk `displays`
/// immediately before saving and write those, leaving every other section to
/// the caller's edits.
pub fn apply(cfg: &Config, db_path: &Path) -> Result<(), redb::Error> {
    let on_disk = load_or_default(db_path);
    let mut cfg = cfg.clone();
    cfg.displays = on_disk.displays;
    let db = open(db_path)?;
    cfg.save(&db)
}

/// GTK-free stand-in for `gdk::ModifierType` -- the (GTK) capture widget
/// converts the modifier state of the key-press event it just captured into
/// this before handing off to [`combo_from_keysym`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CaptureMods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub logo: bool,
}

/// The xkb key names of keys that are themselves modifiers (both physical
/// keys of every modifier icedtea recognizes, plus the other well-known
/// xkb modifier/lock keysyms) -- a lone press of one of these must never
/// become a binding on its own, or every attempt to hold e.g. Super while
/// picking the "real" key would spuriously bind to bare Super first.
///
/// This checks the *name* `keysym_to_key_name` resolves to (rather than
/// depending on `xkbcommon` directly for its keysym constants) so this
/// module stays within the `icedtea_config` surface the plan calls out
/// (`keysym_to_key_name`, `MODIFIER_TOKENS`) instead of picking up a second,
/// independent way to talk about keysyms.
fn is_bare_modifier(key_name: &str) -> bool {
    matches!(
        key_name.strip_prefix("KEY_").unwrap_or(key_name),
        "Shift_L"
            | "Shift_R"
            | "Shift_Lock"
            | "Control_L"
            | "Control_R"
            | "Caps_Lock"
            | "Meta_L"
            | "Meta_R"
            | "Alt_L"
            | "Alt_R"
            | "Super_L"
            | "Super_R"
            | "Hyper_L"
            | "Hyper_R"
            | "Num_Lock"
            | "ISO_Level3_Shift"
            | "ISO_Level3_Latch"
            | "ISO_Level3_Lock"
            | "ISO_Level5_Shift"
            | "ISO_Level5_Latch"
            | "ISO_Level5_Lock"
    )
}

/// Translate a captured keysym + modifier state into a `KeyCombo`, in the
/// exact serialization the compositor's matcher expects (see
/// `icedtea_config::keys`): `modifiers` uses the `MODIFIER_TOKENS` spelling,
/// `key` is `keysym_to_key_name(keysym)`.
///
/// Returns `None` when `keysym` itself names a bare modifier key (see
/// [`is_bare_modifier`]) -- a lone Shift/Ctrl/Alt/Super press mid-capture
/// should never become a binding.
pub fn combo_from_keysym(keysym: u32, modifiers: CaptureMods) -> Option<KeyCombo> {
    let key = keysym_to_key_name(keysym);
    if is_bare_modifier(&key) {
        return None;
    }

    let mut mods = Vec::new();
    if modifiers.logo {
        mods.push("SUPER".to_string());
    }
    if modifiers.ctrl {
        mods.push("CTRL".to_string());
    }
    if modifiers.alt {
        mods.push("ALT".to_string());
    }
    if modifiers.shift {
        mods.push("SHIFT".to_string());
    }
    debug_assert!(mods.iter().all(|m| MODIFIER_TOKENS.contains(&m.as_str())));

    Some(KeyCombo { modifiers: mods, key })
}

/// The order-independent identity of a combo: its key name plus its modifier
/// tokens as a *set* (sorted, deduplicated). The compositor's matcher folds a
/// binding's modifier tokens into a `Modifiers` bitflag set
/// (`compositor/src/input.rs`), so `[SUPER, SHIFT]` and `[SHIFT, SUPER]` fire
/// the *same* binding. Grouping duplicates by this key — rather than by
/// `KeyCombo`'s order-sensitive derived `PartialEq` — matches that semantics,
/// so a hand-edited config with reordered modifiers is still caught as a
/// conflict.
fn combo_identity(combo: &KeyCombo) -> (String, Vec<String>) {
    let mut mods = combo.modifiers.clone();
    mods.sort();
    mods.dedup();
    (combo.key.clone(), mods)
}

/// Every `KeyCombo` in `cfg.keybindings` bound to more than one action,
/// paired with the actions that share it. Two bindings count as the same
/// combo when they name the same key and the same *set* of modifiers
/// (order-independent), matching how the compositor's matcher compares them —
/// see [`combo_identity`].
pub fn duplicate_bindings(cfg: &Config) -> Vec<(KeyCombo, Vec<String>)> {
    let mut groups: Vec<(KeyCombo, Vec<String>)> = Vec::new();
    for (action, combo) in &cfg.keybindings {
        match groups.iter_mut().find(|(c, _)| combo_identity(c) == combo_identity(combo)) {
            Some((_, actions)) => actions.push(action.clone()),
            None => groups.push((combo.clone(), vec![action.clone()])),
        }
    }
    groups.retain(|(_, actions)| actions.len() > 1);
    groups
}

/// Whether `s` is a `#RRGGBB` hex color: a leading `#` followed by exactly
/// six ASCII hex digits.
pub fn valid_hex(s: &str) -> bool {
    let Some(digits) = s.strip_prefix('#') else {
        return false;
    };
    digits.len() == 6 && digits.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The bar's allowed `position` values.
pub const BAR_POSITIONS: [&str; 2] = ["top", "bottom"];

#[cfg(test)]
mod tests {
    use super::*;
    use icedtea_config::{key_name_to_keysym, Behavior, KeyCombo};
    use tempfile::tempdir;

    fn non_default_config() -> Config {
        let mut cfg = icedtea_config::default_config();
        cfg.appearance.bar_position = "bottom".to_string();
        cfg.behavior = Behavior {
            raise_on_focus: !cfg.behavior.raise_on_focus,
            hide_bar_on_fullscreen: !cfg.behavior.hide_bar_on_fullscreen,
            snap_enabled: !cfg.behavior.snap_enabled,
        };
        cfg.workspace_names = vec!["alpha".to_string(), "beta".to_string()];
        cfg.keybindings.insert(
            "custom_action".to_string(),
            KeyCombo { modifiers: vec!["SUPER".to_string(), "SHIFT".to_string()], key: "KEY_x".to_string() },
        );
        cfg
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings-cfg.redb");
        let cfg = non_default_config();

        apply(&cfg, &path).unwrap();
        let loaded = load_or_default(&path);

        assert_eq!(loaded, cfg);
    }

    /// Proof that `combo_from_keysym` preserves whatever keysym it's
    /// *given*: whatever it serializes as `KeyCombo::key` must resolve back
    /// to the *same* keysym through `key_name_to_keysym` -- the exact
    /// function the compositor's own matcher uses.
    ///
    /// This is deliberately scoped to `combo_from_keysym` alone -- it feeds
    /// in an already-unshifted keysym (as every one of these `KEY_*` names
    /// is), so it says nothing about whether the *caller* handed in the
    /// right keysym in the first place. That's a separate concern this
    /// module can't cover: shift/caps-lock normalization needs a live GDK
    /// keymap, which is exactly why this crate is GTK-free (see the module
    /// doc). The actual non-vacuous proof that a Shift-held capture still
    /// resolves to the unshifted key lives in
    /// `pages::keybindings::tests::unshifted_keysym_normalizes_shift_and_capslock_to_the_base_key`,
    /// which drives a real `gdk::Display` against a harness compositor.
    #[test]
    fn combo_round_trips_to_the_compositor_format() {
        let mods = CaptureMods { ctrl: true, alt: false, shift: true, logo: true };
        for name in ["KEY_q", "KEY_Return", "KEY_F5"] {
            let sym = key_name_to_keysym(name);
            assert_ne!(sym, 0, "{name} must resolve to a real keysym for this test to be non-vacuous");

            let combo = combo_from_keysym(sym, mods).unwrap_or_else(|| panic!("{name} must bind"));

            assert_eq!(
                key_name_to_keysym(&combo.key),
                sym,
                "combo_from_keysym({name}) must serialize a key name that resolves back to the same keysym"
            );
            for token in &combo.modifiers {
                assert!(MODIFIER_TOKENS.contains(&token.as_str()), "unexpected modifier token {token:?}");
            }
            assert_eq!(combo.modifiers, vec!["SUPER", "CTRL", "SHIFT"]);
        }
    }

    #[test]
    fn lone_modifier_does_not_bind() {
        let shift_l = key_name_to_keysym("KEY_Shift_L");
        assert_ne!(shift_l, 0);
        assert!(combo_from_keysym(shift_l, CaptureMods { shift: true, ..Default::default() }).is_none());

        let super_l = key_name_to_keysym("KEY_Super_L");
        assert_ne!(super_l, 0);
        assert!(combo_from_keysym(super_l, CaptureMods { logo: true, ..Default::default() }).is_none());
    }

    #[test]
    fn duplicate_bindings_finds_a_planted_conflict() {
        let mut cfg = icedtea_config::default_config();
        let shared = KeyCombo { modifiers: vec!["SUPER".to_string()], key: "KEY_z".to_string() };
        cfg.keybindings.insert("action_one".to_string(), shared.clone());
        cfg.keybindings.insert("action_two".to_string(), shared.clone());

        let dupes = duplicate_bindings(&cfg);
        assert_eq!(dupes.len(), 1);
        let (combo, mut actions) = dupes.into_iter().next().unwrap();
        assert_eq!(combo, shared);
        actions.sort();
        assert_eq!(actions, vec!["action_one".to_string(), "action_two".to_string()]);
    }

    #[test]
    fn duplicate_bindings_is_empty_for_defaults() {
        let cfg = icedtea_config::default_config();
        assert!(duplicate_bindings(&cfg).is_empty(), "default keybindings must not collide");
    }

    #[test]
    fn duplicate_bindings_catches_reordered_modifiers() {
        // The compositor matches modifiers as an order-independent set, so
        // these two bindings fire the same combo and MUST be reported as a
        // conflict even though their modifier order (and thus derived
        // `PartialEq`) differs.
        let mut cfg = icedtea_config::default_config();
        cfg.keybindings.insert(
            "a".to_string(),
            KeyCombo { modifiers: vec!["SUPER".into(), "SHIFT".into()], key: "KEY_z".into() },
        );
        cfg.keybindings.insert(
            "b".to_string(),
            KeyCombo { modifiers: vec!["SHIFT".into(), "SUPER".into()], key: "KEY_z".into() },
        );
        let dupes = duplicate_bindings(&cfg);
        assert_eq!(dupes.len(), 1, "reordered-modifier duplicate must be caught");
        let (_, actions) = &dupes[0];
        assert_eq!(actions.len(), 2);
    }

    #[test]
    fn valid_hex_accepts_and_rejects() {
        assert!(valid_hex("#000000"));
        assert!(valid_hex("#ffFFff"));
        assert!(valid_hex("#1e1e2e"));

        assert!(!valid_hex("000000"));
        assert!(!valid_hex("#00000"));
        assert!(!valid_hex("#0000000"));
        assert!(!valid_hex("#gggggg"));
        assert!(!valid_hex(""));
        assert!(!valid_hex("#"));
    }

    #[test]
    fn bar_positions_domain() {
        assert_eq!(BAR_POSITIONS, ["top", "bottom"]);
        assert!(BAR_POSITIONS.contains(&"top"));
        assert!(BAR_POSITIONS.contains(&"bottom"));
        assert!(!BAR_POSITIONS.contains(&"left"));
    }

    /// A non-display Apply (here: an accent-color edit) must never clobber the
    /// displays the compositor persisted out-of-band. `apply` reloads the
    /// on-disk `displays` and writes those, so the layout on disk survives even
    /// though the caller's working copy carried a stale (empty) `displays`.
    #[test]
    fn apply_preserves_on_disk_displays() {
        use icedtea_config::DisplayConfig;

        let dir = tempdir().unwrap();
        let path = dir.path().join("displays-cfg.redb");

        // The compositor persisted a layout on its side.
        let persisted = vec![DisplayConfig {
            name: "DP-1".into(),
            enabled: true,
            width: 2560,
            height: 1440,
            refresh_mhz: 144000,
            x: 0,
            y: 0,
            scale: 1.0,
            transform: 0,
        }];
        // The compositor persists displays through its own `Config::save`, not
        // through `model::apply` (which never writes displays), so seed the
        // store that way.
        let mut on_disk = icedtea_config::default_config();
        on_disk.displays = persisted.clone();
        {
            let db = open(&path).unwrap();
            on_disk.save(&db).unwrap();
        }

        // The settings app loaded its Config at startup, then edits a
        // non-display field. Simulate a *stale* working copy whose displays no
        // longer match disk (here: empty, as a fresh default would be).
        let mut working = icedtea_config::default_config();
        working.displays.clear();
        working.appearance.palette.accent = "#ff00aa".to_string();
        apply(&working, &path).unwrap();

        // The non-display edit landed, but the compositor-owned displays are
        // intact -- not overwritten by the working copy's stale empty list.
        let reloaded = load_or_default(&path);
        assert_eq!(reloaded.appearance.palette.accent, "#ff00aa");
        assert_eq!(reloaded.displays, persisted, "Apply must not clobber persisted displays");
    }

    #[test]
    fn model_dirty_tracking_and_revert() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("model-cfg.redb");
        let cfg = icedtea_config::default_config();
        apply(&cfg, &path).unwrap();

        let mut model = Model::load(&path);
        assert!(!model.is_dirty());

        model.working.behavior.snap_enabled = !model.working.behavior.snap_enabled;
        assert!(model.is_dirty());

        apply(&model.working, &path).unwrap();
        model.saved = model.working.clone();
        assert!(!model.is_dirty());

        model.working.behavior.raise_on_focus = !model.working.behavior.raise_on_focus;
        assert!(model.is_dirty());
        model.revert(&path);
        assert!(!model.is_dirty());
        assert_eq!(model.working, model.saved);
    }
}
