//! The Keybindings page: a scrollable list, one row per action, showing its
//! current `KeyCombo` and a **Set** button that puts the row into capture
//! mode (next key press becomes the new binding).
//!
//! The action set is the fixed set the compositor always recognizes
//! (`close`, `fullscreen`, ...) plus `workspace:N`/`move_to_workspace:N`
//! generated for `1..=workspace_names.len()` -- so this page's row set
//! tracks the Workspaces page's row count.

use icedtea_config::KeyCombo;

/// The Keybindings page.
///
/// P1 ships the page's frame only; P3 fills it in (contract §2.6).
#[must_use]
pub fn view(m: &crate::app::SettingsModel) -> icedtea_ui::view::View<crate::app::Msg> {
    let _ = m;
    icedtea_ui::view::builders::box_(
        icedtea_ui::widgets::types::Orientation::Vertical,
        [icedtea_ui::view::builders::label("Keybindings")],
    )
    .id("keybindings_page")
    .margin(16, 16, 16, 16)
}

/// The fixed part of the action set -- always present regardless of
/// workspace count. Kept in the same order the compositor's own defaults
/// insert them (`icedtea_config::defaults::default_config`) purely so the
/// row order is stable/predictable, not because order carries meaning here.
pub const FIXED_ACTIONS: [&str; 10] = [
    "close",
    "fullscreen",
    "reload",
    "quit",
    "cycle:alt_tab",
    "spawn:terminal",
    "snap:left",
    "snap:right",
    "snap:up",
    "snap:down",
];
pub const SNAP_RESTORE: &str = "snap:restore";

/// The full action set for a config with `workspace_count` workspaces: the
/// fixed actions plus a generated `workspace:N`/`move_to_workspace:N` pair
/// for every `1..=workspace_count`.
pub fn action_list(workspace_count: usize) -> Vec<String> {
    let mut actions: Vec<String> = FIXED_ACTIONS.iter().map(|s| s.to_string()).collect();
    actions.push(SNAP_RESTORE.to_string());
    for n in 1..=workspace_count {
        actions.push(format!("workspace:{n}"));
        actions.push(format!("move_to_workspace:{n}"));
    }
    actions
}

/// Render a `KeyCombo` the way the brief specifies: `SUPER+SHIFT+q` --
/// modifiers in the order they're stored (always `MODIFIER_TOKENS` order,
/// per `combo_from_keysym`) followed by the key name with its `KEY_` prefix
/// stripped.
pub fn format_combo(combo: &KeyCombo) -> String {
    let key = combo.key.strip_prefix("KEY_").unwrap_or(&combo.key);
    let mut parts = combo.modifiers.clone();
    parts.push(key.to_string());
    parts.join("+")
}

/// Whether the shared window key controller should consume a key press as a
/// binding capture. It must only act when the Keybindings page is the
/// visible child (`page_visible`) AND a capture is actually armed
/// (`capturing`). Extracted so the gate is unit-testable without a live
/// window. (#4)
pub fn should_capture(page_visible: bool, capturing: bool) -> bool {
    page_visible && capturing
}

/// Clear an armed capture and report which action's row must have its Set
/// button restored (`None` when nothing was armed). Pure so the reset decision
/// is unit-testable; the caller performs the widget label restore. (#4)
pub fn take_capture_reset(capturing: &mut Option<String>) -> Option<String> {
    capturing.take()
}

/// Pick the keysym a capture stores, given the two the key event carries.
///
/// `base` is the group-0/level-0 sym (`icedtea_ui::window::keyboard::KeyEvent::base`,
/// M5-D7) and `modified` the shift/caps/group-adjusted one (`KeyEvent::keysym`).
/// The rule is `compositor/src/input.rs:109-123`'s, verbatim: take the base sym,
/// and fall back to the modified one **only** when the keycode produces no base
/// sym at all (`XKB_KEY_NoSymbol`, which is 0).
///
/// This is required because `icedtea_config::keys::key_name_to_keysym` always
/// encodes the unshifted keysym (`"KEY_q"` -> `0x71`), so a capture that stored
/// `0x51` (`XK_Q`) from a `SUPER+SHIFT+q` press would produce a binding
/// `match_action` can never fire.
#[must_use]
pub fn normalise_keysym(base: u32, modified: u32) -> u32 {
    if base == 0 { modified } else { base }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_list_is_fixed_plus_generated_workspace_pairs() {
        let actions = action_list(2);
        for fixed in FIXED_ACTIONS {
            assert!(
                actions.contains(&fixed.to_string()),
                "{fixed} must be present"
            );
        }
        assert!(actions.contains(&SNAP_RESTORE.to_string()));
        assert!(actions.contains(&"workspace:1".to_string()));
        assert!(actions.contains(&"workspace:2".to_string()));
        assert!(actions.contains(&"move_to_workspace:1".to_string()));
        assert!(actions.contains(&"move_to_workspace:2".to_string()));
        assert!(!actions.contains(&"workspace:3".to_string()));
        assert_eq!(actions.len(), FIXED_ACTIONS.len() + 1 + 2 * 2);
    }

    #[test]
    fn action_list_matches_default_config_bindings_for_its_workspace_count() {
        // `default_config()` pre-binds workspace:1..=9 regardless of how
        // many workspace *names* it ships (4) -- those extra bindings are
        // simply unreachable until the Workspaces page grows the name list
        // that far, which is by design (see the brief: rows are generated
        // for `1..=workspace_names.len()`). So this only checks that every
        // action for the actual default workspace count is covered, not
        // that every pre-bound key shows up as a row.
        let cfg = icedtea_config::default_config();
        let actions = action_list(cfg.workspace_names.len());
        for n in 1..=cfg.workspace_names.len() {
            let ws = format!("workspace:{n}");
            let mv = format!("move_to_workspace:{n}");
            assert!(
                cfg.keybindings.contains_key(&ws),
                "default_config must bind {ws}"
            );
            assert!(
                cfg.keybindings.contains_key(&mv),
                "default_config must bind {mv}"
            );
            assert!(actions.contains(&ws));
            assert!(actions.contains(&mv));
        }
        for action in FIXED_ACTIONS.iter().chain(std::iter::once(&SNAP_RESTORE)) {
            assert!(
                cfg.keybindings.contains_key(*action),
                "default binding {action:?} missing from default_config"
            );
        }
    }

    #[test]
    fn format_combo_renders_super_shift_key() {
        let combo = KeyCombo {
            modifiers: vec!["SUPER".to_string(), "SHIFT".to_string()],
            key: "KEY_q".to_string(),
        };
        assert_eq!(format_combo(&combo), "SUPER+SHIFT+q");
    }

    #[test]
    fn format_combo_with_no_modifiers() {
        let combo = KeyCombo {
            modifiers: vec![],
            key: "KEY_F5".to_string(),
        };
        assert_eq!(format_combo(&combo), "F5");
    }

    #[test]
    fn should_capture_only_when_page_visible_and_armed() {
        // Both conditions required: an armed capture on a hidden page (the user
        // switched away without completing) must NOT consume keystrokes.
        assert!(should_capture(true, true));
        assert!(
            !should_capture(false, true),
            "armed but page hidden: must not hijack"
        );
        assert!(
            !should_capture(true, false),
            "visible but nothing armed: pass through"
        );
        assert!(!should_capture(false, false));
    }

    #[test]
    fn take_capture_reset_reports_and_clears_the_armed_action() {
        let mut armed = Some("close".to_string());
        assert_eq!(take_capture_reset(&mut armed), Some("close".to_string()));
        assert_eq!(armed, None, "capture must be cleared after a reset");
        // A second reset is a no-op once nothing is armed.
        assert_eq!(take_capture_reset(&mut armed), None);
    }

    /// `KEY_NoSymbol` is 0; a keycode the keymap does not map reports it as
    /// `base`, and the capture must then fall back to the modified sym rather
    /// than storing 0.
    ///
    /// Mutation check: make `normalise_keysym` return `modified`
    /// unconditionally; `normalise_keysym_prefers_the_base_sym` fails. Restore.
    #[test]
    fn normalise_keysym_prefers_the_base_sym() {
        // SUPER+SHIFT+q: GDK/xkb report the modified sym XK_Q (0x51); the
        // compositor only ever matches the unshifted XK_q (0x71), because
        // `icedtea_config::keys::key_name_to_keysym("KEY_q")` encodes 0x71.
        assert_eq!(normalise_keysym(0x71, 0x51), 0x71);
    }

    #[test]
    fn normalise_keysym_falls_back_when_there_is_no_base_sym() {
        assert_eq!(normalise_keysym(0, 0x51), 0x51);
    }

    #[test]
    fn normalise_keysym_is_identity_for_an_unmodified_key() {
        assert_eq!(normalise_keysym(0x71, 0x71), 0x71);
    }

    /// The pure surface the M5 view layer calls is `pub` — a private helper
    /// would leave `app.rs` re-implementing the action set.
    #[test]
    fn the_pure_surface_is_public() {
        fn takes_fn(_: fn(usize) -> Vec<String>) {}
        takes_fn(crate::pages::keybindings::action_list);
        assert!(crate::pages::keybindings::should_capture(true, true));
        assert_eq!(crate::pages::keybindings::FIXED_ACTIONS.len(), 10);
    }
}
