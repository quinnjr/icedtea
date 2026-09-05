//! The Keybindings page: a scrollable list, one row per action, showing its
//! current `KeyCombo` and a **Set** button that puts the row into capture
//! mode (next key press becomes the new binding).
//!
//! The action set is the fixed set the compositor always recognizes
//! (`close`, `fullscreen`, ...) plus `workspace:N`/`move_to_workspace:N`
//! generated for `1..=workspace_names.len()` -- so this page's row set
//! tracks the Workspaces page's row count.

use icedtea_config::KeyCombo;
use icedtea_ui::window::keyboard::{KeyEvent, Mods};

use crate::app::Msg;
use crate::model::CaptureMods;

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

/// The window-root `on_key` handler.
///
/// `armed` is `m.capturing.is_some()`, read off the model by `app::view`
/// each frame. It is a parameter and not a `capturing.is_none()` check
/// inside `update` (which is what contract §2.7 rule 4 originally said)
/// because `GenericC::on_event`'s `Event::Key` arm sets `cx.handled = true`
/// for *any* message a `KeyPressed` handler returns, and M3 deviation
/// P4-D19 makes a handled event end the whole dispatch -- so a handler that
/// answers every key press swallows every keystroke in the window,
/// including the ones a focused `Entry` needs. Deviation P3-D1.
///
/// `None` means "not ours, let it through".
///
/// The keysym is resolved exactly the way `compositor/src/input.rs:109-123`
/// matches: the raw, level-0 sym, falling back to the modified one only
/// when the keycode produces no base sym at all. `icedtea_config::keys::
/// key_name_to_keysym` always encodes the unshifted keysym (`"KEY_q"` ->
/// `0x71`), so a capture that stored `0x51` (`XK_Q`) from a `SUPER+SHIFT+q`
/// press would produce a binding the compositor can never fire.
#[must_use]
pub fn capture_key(armed: bool, ev: &KeyEvent) -> Option<Msg> {
    if !armed || !ev.pressed || ev.repeat {
        return None;
    }
    let keysym = normalise_keysym(ev.base.raw(), ev.keysym.raw());
    if keysym == xkbcommon::xkb::keysyms::KEY_Escape {
        return Some(Msg::CaptureCancelled);
    }
    Some(Msg::KeyCaptured {
        keysym,
        mods: CaptureMods {
            ctrl: ev.mods.contains(Mods::CTRL),
            alt: ev.mods.contains(Mods::ALT),
            shift: ev.mods.contains(Mods::SHIFT),
            logo: ev.mods.contains(Mods::LOGO),
        },
    })
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

    use icedtea_ui::window::keyboard::{KeyEvent, Mods};
    use xkbcommon::xkb::{Keysym, keysyms};

    /// A synthesised press. `base` is what `Keymap::translate` stamps from
    /// `Keymap::base_keysym` (M5-D7); `keysym` is the level-adjusted sym the
    /// same translate produced.
    fn press(keycode: u32, keysym: u32, base: u32, mods: Mods) -> KeyEvent {
        KeyEvent {
            keycode,
            keysym: Keysym::from(keysym),
            base: Keysym::from(base),
            utf8: None,
            mods,
            consumed: Mods::empty(),
            pressed: true,
            repeat: false,
            serial: 1,
            time_ms: 1,
        }
    }

    /// The SHIP-BLOCKER the GTK test guarded (findings #1/#2), re-expressed
    /// on the toolkit: capturing with Shift held stores the *unshifted*
    /// keysym, because that is the only form `key_name_to_keysym` -- and so
    /// the compositor's `match_action` -- can ever match.
    ///
    /// Mutation check: make `capture_key` pass `ev.keysym.raw()` where it
    /// passes `ev.base.raw()`; this test fails with `KEY_A` (0x41). Restore.
    #[test]
    fn a_shifted_press_captures_the_base_keysym() {
        // evdev KEY_A = 30. xkb's is 38; the KeyEvent carries the evdev code.
        let ev = press(30, keysyms::KEY_A, keysyms::KEY_a, Mods::SHIFT);
        let msg = capture_key(true, &ev).expect("an armed press captures");
        match msg {
            Msg::KeyCaptured { keysym, mods } => {
                assert_eq!(keysym, keysyms::KEY_a, "the base, unshifted sym is stored");
                assert_eq!(
                    mods,
                    crate::model::CaptureMods {
                        shift: true,
                        ..Default::default()
                    }
                );
            }
            other => panic!("expected KeyCaptured, got {other:?}"),
        }
    }

    /// A keycode whose base sym is `NoSymbol` falls back to the modified sym
    /// rather than storing 0 -- `normalise_keysym`'s second clause, reached
    /// through `capture_key`.
    ///
    /// Mutation check: make `capture_key` return `ev.base.raw()`
    /// unconditionally; this fails with 0. Restore.
    #[test]
    fn a_keycode_with_no_base_sym_falls_back_to_the_modified_one() {
        let ev = press(999, keysyms::KEY_F5, keysyms::KEY_NoSymbol, Mods::empty());
        match capture_key(true, &ev).expect("still captures") {
            Msg::KeyCaptured { keysym, .. } => assert_eq!(keysym, keysyms::KEY_F5),
            other => panic!("expected KeyCaptured, got {other:?}"),
        }
    }

    /// Escape cancels rather than binding -- checked against the *normalised*
    /// sym, so an exotic layout that reaches Escape only at a shifted level
    /// still cancels.
    ///
    /// Mutation check: drop the Escape arm; this fails with `KeyCaptured`.
    /// Restore.
    #[test]
    fn escape_cancels_the_capture() {
        let ev = press(1, keysyms::KEY_Escape, keysyms::KEY_Escape, Mods::empty());
        assert!(matches!(
            capture_key(true, &ev),
            Some(Msg::CaptureCancelled)
        ));
    }

    /// Nothing armed: the handler must decline, or `GenericC`'s
    /// `Event::Key` arm sets `cx.handled` (M3 P4-D19: that ends the whole
    /// dispatch) and every keystroke in the window is swallowed -- typing
    /// into a workspace-name Entry included. This is deviation P3-D1's whole
    /// reason for existing.
    ///
    /// Mutation check: drop the `!armed` early return; this fails. Restore.
    #[test]
    fn an_unarmed_press_is_declined_so_the_focused_widget_still_sees_it() {
        let ev = press(30, keysyms::KEY_a, keysyms::KEY_a, Mods::empty());
        assert!(capture_key(false, &ev).is_none());
    }

    /// A release and an auto-repeat are both declined: a capture binds on
    /// the press, and a held key must not rebind the row again and again.
    ///
    /// Mutation check: drop the `!ev.repeat` clause; the repeat assertion
    /// fails. Restore.
    #[test]
    fn releases_and_repeats_are_declined() {
        let mut release = press(30, keysyms::KEY_a, keysyms::KEY_a, Mods::empty());
        release.pressed = false;
        assert!(capture_key(true, &release).is_none());

        let mut repeat = press(30, keysyms::KEY_a, keysyms::KEY_a, Mods::empty());
        repeat.repeat = true;
        assert!(capture_key(true, &repeat).is_none());
    }

    /// Modifiers come from `mods` (every effectively-active modifier), not
    /// from `consumed` (the ones this sym spent reaching its level). A
    /// Super+Shift+q press consumes Shift to reach `Q`, and a capture that
    /// read `consumed` would drop Super entirely.
    ///
    /// Mutation check: read `ev.consumed` instead of `ev.mods`; this fails
    /// with `logo: false`. Restore.
    #[test]
    fn modifiers_are_read_from_the_effective_state() {
        let mut ev = press(16, keysyms::KEY_Q, keysyms::KEY_q, Mods::SHIFT | Mods::LOGO);
        ev.consumed = Mods::SHIFT;
        match capture_key(true, &ev).expect("captures") {
            Msg::KeyCaptured { keysym, mods } => {
                assert_eq!(keysym, keysyms::KEY_q);
                assert_eq!(
                    mods,
                    crate::model::CaptureMods {
                        shift: true,
                        logo: true,
                        ..Default::default()
                    }
                );
            }
            other => panic!("expected KeyCaptured, got {other:?}"),
        }
    }

    /// Caps Lock and Num Lock are not binding modifiers -- `CaptureMods` has
    /// four fields and `MODIFIER_TOKENS` four tokens, so a lock state must
    /// not leak into a stored combo.
    ///
    /// Mutation check: add `Mods::CAPS` to the `shift` expression; this
    /// fails. Restore.
    #[test]
    fn lock_states_are_not_binding_modifiers() {
        let ev = press(30, keysyms::KEY_A, keysyms::KEY_a, Mods::CAPS | Mods::NUM);
        match capture_key(true, &ev).expect("captures") {
            Msg::KeyCaptured { mods, .. } => {
                assert_eq!(mods, crate::model::CaptureMods::default());
            }
            other => panic!("expected KeyCaptured, got {other:?}"),
        }
    }
}
