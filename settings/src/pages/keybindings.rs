//! The Keybindings page: a scrollable list, one row per action, showing its
//! current `KeyCombo` and a **Set** button that puts the row into capture
//! mode (next key press becomes the new binding).
//!
//! The action set is the fixed set the compositor always recognizes
//! (`close`, `fullscreen`, ...) plus `workspace:N`/`move_to_workspace:N`
//! generated for `1..=workspace_names.len()` -- so this page's row set
//! tracks the Workspaces page's row count.

use icedtea_config::KeyCombo;
use icedtea_ui::view::View;
use icedtea_ui::view::builders::{BoxExt, box_, scrolled_window};
use icedtea_ui::widgets::Orientation;
use icedtea_ui::widgets::button::button;
use icedtea_ui::widgets::label::{LabelExt, label};
use icedtea_ui::window::keyboard::{KeyEvent, Mods};

use crate::app::{Msg, SettingsModel};
use crate::model::CaptureMods;

/// The Keybindings page body.
///
/// Pure: it reads `m.model.working.keybindings`, `m.capturing` and
/// `m.conflicts`, and never writes. The action set is recomputed from
/// `working.workspace_names.len()` on every call, which is why the GTK
/// `RebuildSlot` forward-reference between this page and the Workspaces
/// page disappears -- this page cannot go stale.
///
/// Rows are keyed by action, so adding a workspace inserts two rows without
/// rebuilding (and re-arming) any of the others.
#[must_use]
pub fn view(m: &SettingsModel) -> View<Msg> {
    let cfg = &m.model.working;
    let rows: Vec<View<Msg>> = action_list(cfg.workspace_names.len())
        .into_iter()
        .map(|action| {
            let text = cfg
                .keybindings
                .get(&action)
                .map_or_else(|| "(unbound)".to_string(), format_combo);
            let colliding = m
                .conflicts
                .iter()
                .any(|(_, actions)| actions.contains(&action));
            let mut combo = label(&text)
                .id(&format!("kb_combo_{action}"))
                .width_chars(20)
                .xalign(0.0)
                .hexpand(true);
            if colliding {
                combo = combo.classes(&["conflict"]);
            }
            let armed = m.capturing.as_deref() == Some(action.as_str());
            let set_label = if armed { "Press a key…" } else { "Set" };
            let armed_action = action.clone();
            box_(
                Orientation::Horizontal,
                [
                    label(&action).width_chars(24).xalign(0.0),
                    combo,
                    button(set_label)
                        .id(&format!("kb_set_{action}"))
                        .on_click(Msg::CaptureArmed(armed_action)),
                ],
            )
            .spacing(12)
            .key(action.clone())
            .id(&format!("kb_row_{action}"))
        })
        .collect();

    scrolled_window(
        box_(Orientation::Vertical, rows)
            .spacing(4)
            .margin(16, 16, 16, 16),
    )
    .id("keybindings_list")
    .vexpand(true)
    .hexpand(true)
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

/// Store a captured binding.
///
/// Returns `true` when a combo was stored -- the caller clears
/// `m.capturing` and recomputes `m.conflicts` -- and `false` when
/// [`combo_from_keysym`] declined the sym, which is what a lone
/// Shift/Ctrl/Alt/Super press produces. In that case the capture stays
/// armed and waits for the real key: the GTK handler's behaviour verbatim
/// (contract §2.7 rule 5).
///
/// A conflicting combo is stored like any other. Conflicts are advisory
/// styling only and never block Apply.
pub fn apply_capture(
    cfg: &mut icedtea_config::Config,
    action: &str,
    keysym: u32,
    mods: CaptureMods,
) -> bool {
    let Some(combo) = crate::model::combo_from_keysym(keysym, mods) else {
        return false;
    };
    report_binding(action, &combo);
    cfg.keybindings.insert(action.to_string(), combo);
    true
}

/// Append one `binding` line to `$ICEDTEA_PROBE_REPORT`, when it is set.
///
/// Deviation P3-D3. M5-D9's app-side report carries `probe` and `alloc`
/// lines, neither of which can express a `KeyCombo`, so the harness capture
/// test has nothing to assert against without this. A no-op in production,
/// where the variable is unset; an I/O failure is logged once and dropped,
/// never propagated into `update`.
pub fn report_binding(action: &str, combo: &KeyCombo) {
    let Some(path) = std::env::var_os("ICEDTEA_PROBE_REPORT") else {
        return;
    };
    if let Err(err) = write_binding_line(std::path::Path::new(&path), action, combo) {
        tracing::warn!(%err, "cannot append a binding line to $ICEDTEA_PROBE_REPORT");
    }
}

/// `binding <action> <modifiers|-> <key>`, appended.
///
/// Appends rather than truncates: the same file carries the app's own
/// `probe`/`alloc` batches. The raw `KeyCombo` field spellings are written,
/// not [`format_combo`]'s display form, so a reader pins the exact
/// serialization the compositor's matcher compares against.
fn write_binding_line(
    path: &std::path::Path,
    action: &str,
    combo: &KeyCombo,
) -> std::io::Result<()> {
    use std::io::Write as _;
    let mods = if combo.modifiers.is_empty() {
        "-".to_string()
    } else {
        combo.modifiers.join("+")
    };
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "binding {action} {mods} {}", combo.key)
}

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

    /// A real capture stores the combo `combo_from_keysym` builds and
    /// reports that the capture is finished.
    ///
    /// Mutation check: return `false` unconditionally; this fails. Restore.
    #[test]
    fn apply_capture_stores_the_combo_and_reports_done() {
        let mut cfg = icedtea_config::default_config();
        let stored = apply_capture(
            &mut cfg,
            "close",
            xkbcommon::xkb::keysyms::KEY_a,
            CaptureMods {
                shift: true,
                ..Default::default()
            },
        );
        assert!(stored, "a real key finishes the capture");
        let combo = cfg.keybindings.get("close").expect("close is bound");
        assert_eq!(combo.key, "KEY_a");
        assert_eq!(combo.modifiers, vec!["SHIFT".to_string()]);
    }

    /// A lone modifier press leaves the capture armed and writes nothing:
    /// holding Super while reaching for the real key must not bind bare
    /// Super, and must not clobber the existing binding either.
    ///
    /// Mutation check: make `apply_capture` insert whatever
    /// `combo_from_keysym` returns without the `else return false`; this
    /// fails to compile, which is the point -- instead, make it return
    /// `true` on `None`; the `still_armed` assertion fails. Restore.
    #[test]
    fn a_lone_modifier_leaves_the_capture_armed() {
        let mut cfg = icedtea_config::default_config();
        let before = cfg.keybindings.clone();
        let stored = apply_capture(
            &mut cfg,
            "close",
            xkbcommon::xkb::keysyms::KEY_Super_L,
            CaptureMods {
                logo: true,
                ..Default::default()
            },
        );
        assert!(!stored, "still armed: wait for the real key");
        assert_eq!(cfg.keybindings, before, "nothing was written");
    }

    /// A conflict is stored, not refused: two actions may share a combo and
    /// the page only flags it (contract §2.7 rule 6, "conflicts never block
    /// Apply").
    ///
    /// Mutation check: make `apply_capture` return `false` when
    /// `duplicate_bindings` would grow; this fails. Restore.
    #[test]
    fn apply_capture_allows_a_conflicting_binding() {
        let mut cfg = icedtea_config::default_config();
        assert!(apply_capture(
            &mut cfg,
            "close",
            xkbcommon::xkb::keysyms::KEY_a,
            CaptureMods::default()
        ));
        assert!(apply_capture(
            &mut cfg,
            "quit",
            xkbcommon::xkb::keysyms::KEY_a,
            CaptureMods::default()
        ));
        let conflicts = crate::model::duplicate_bindings(&cfg);
        let flagged: Vec<_> = conflicts
            .iter()
            .flat_map(|(_, actions)| actions.iter().cloned())
            .collect();
        assert!(flagged.contains(&"close".to_string()));
        assert!(flagged.contains(&"quit".to_string()));
    }

    /// The probe-report line format the harness gate parses (deviation
    /// P3-D3): whitespace-separated, raw KeyCombo spellings, `-` for no
    /// modifiers.
    ///
    /// Mutation check: write `format_combo(combo)` instead of the raw key;
    /// the `KEY_a` assertion fails with `a`. Restore.
    #[test]
    fn a_binding_report_line_carries_the_raw_combo_spelling() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("report");

        write_binding_line(
            &path,
            "close",
            &icedtea_config::KeyCombo {
                modifiers: vec!["SUPER".to_string(), "SHIFT".to_string()],
                key: "KEY_q".to_string(),
            },
        )
        .expect("write");
        write_binding_line(
            &path,
            "reload",
            &icedtea_config::KeyCombo {
                modifiers: vec![],
                key: "KEY_F5".to_string(),
            },
        )
        .expect("write");

        let text = std::fs::read_to_string(&path).expect("read back");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(
            lines,
            vec!["binding close SUPER+SHIFT KEY_q", "binding reload - KEY_F5"]
        );
    }

    /// Writing appends; it never truncates. The report file also carries the
    /// app's `probe`/`alloc` batches, and a capture that truncated them
    /// would blind every gate in this part.
    ///
    /// Mutation check: use `File::create` instead of `OpenOptions::append`;
    /// this fails -- the earlier line is gone. Restore.
    #[test]
    fn a_binding_report_line_appends_to_what_is_already_there() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("report");
        std::fs::write(&path, "probe root 10 20\n").expect("seed");
        write_binding_line(
            &path,
            "close",
            &icedtea_config::KeyCombo {
                modifiers: vec![],
                key: "KEY_a".to_string(),
            },
        )
        .expect("write");
        let text = std::fs::read_to_string(&path).expect("read back");
        assert_eq!(text, "probe root 10 20\nbinding close - KEY_a\n");
    }

    use icedtea_ui::css::cascade::{CompiledSheet, cascade};
    use icedtea_ui::css::node::Node;
    use icedtea_ui::css::registry::Prop as CssProp;
    use icedtea_ui::css::select::MatchCx;
    use icedtea_ui::view::{EventKind, Kind, Prop, PropName};

    /// A `SettingsModel` on a throwaway db, with `workspace_names` set so
    /// `action_list`'s generated rows are predictable.
    fn model_with_workspaces(count: usize) -> crate::app::SettingsModel {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("config.redb");
        // `handles_for_test`, never the real `ipc::spawn`: that one opens a
        // `ConfigReloaded` subscription on the developer's live session bus
        // (finding 15). Nothing here issues a `Cmd::Task`.
        let (workers, _rx, _portal_rx, _fs_rx) = crate::ipc::handles_for_test();
        let mut m = crate::app::SettingsModel::new(db_path, workers);
        m.model.working.workspace_names = (1..=count).map(|n| n.to_string()).collect();
        m
    }

    /// One keyed row per `action_list` entry, with the three ids the gates
    /// address and the `CaptureArmed` message the Set button emits.
    ///
    /// Mutation check: key the rows by index instead of by action; the key
    /// assertion fails. Restore.
    #[test]
    fn one_keyed_row_per_action_with_its_own_ids() {
        let m = model_with_workspaces(2);
        let page = view(&m);
        assert_eq!(page.kind, Kind::ScrolledWindow);
        assert_eq!(page.props.str(PropName::Id), Some("keybindings_list"));

        let rows = &page.children[0].children;
        let actions = action_list(2);
        assert_eq!(rows.len(), actions.len(), "one row per action");

        for (row, action) in rows.iter().zip(actions.iter()) {
            assert_eq!(
                row.props.str(PropName::Id),
                Some(format!("kb_row_{action}").as_str())
            );
            assert_eq!(
                row.key,
                Some(icedtea_ui::view::Key::Name(action.as_str().into()))
            );
            assert_eq!(
                row.children[0].props.str(PropName::Label),
                Some(action.as_str())
            );
            assert_eq!(
                row.children[1].props.str(PropName::Id),
                Some(format!("kb_combo_{action}").as_str())
            );

            let set = &row.children[2];
            assert_eq!(
                set.props.str(PropName::Id),
                Some(format!("kb_set_{action}").as_str())
            );
            let armed = set
                .handlers
                .fire_unit(EventKind::Click)
                .expect("Set emits a Click");
            assert!(
                matches!(&armed, Msg::CaptureArmed(a) if a == action),
                "row {action} emitted {armed:?}"
            );
        }
    }

    /// The combo label shows `format_combo` when bound and `(unbound)` when
    /// not -- `sync_rows`' two branches, now computed.
    ///
    /// Mutation check: drop the `(unbound)` branch and render an empty
    /// string; this fails. Restore.
    #[test]
    fn the_combo_label_shows_the_binding_or_unbound() {
        let mut m = model_with_workspaces(1);
        m.model.working.keybindings.insert(
            "close".to_string(),
            icedtea_config::KeyCombo {
                modifiers: vec!["SUPER".to_string()],
                key: "KEY_q".to_string(),
            },
        );
        m.model.working.keybindings.remove("quit");

        let page = view(&m);
        let rows = &page.children[0].children;
        let combo_of = |action: &str| -> String {
            rows.iter()
                .find(|r| r.props.str(PropName::Id) == Some(format!("kb_row_{action}").as_str()))
                .and_then(|r| r.children[1].props.str(PropName::Label))
                .expect("a combo label")
                .to_string()
        };
        assert_eq!(combo_of("close"), "SUPER+q");
        assert_eq!(combo_of("quit"), "(unbound)");
    }

    /// The armed row's Set button says so; every other row still says "Set".
    ///
    /// Mutation check: always render "Set"; this fails. Restore.
    #[test]
    fn the_armed_rows_button_prompts_for_a_key() {
        let mut m = model_with_workspaces(1);
        m.capturing = Some("fullscreen".to_string());
        let page = view(&m);
        let rows = &page.children[0].children;
        let label_of = |action: &str| -> String {
            rows.iter()
                .find(|r| r.props.str(PropName::Id) == Some(format!("kb_row_{action}").as_str()))
                .and_then(|r| r.children[2].props.str(PropName::Label))
                .expect("a Set button label")
                .to_string()
        };
        assert_eq!(label_of("fullscreen"), "Press a key…");
        assert_eq!(label_of("close"), "Set");
    }

    /// A conflicting binding's combo label carries the `conflict` class; a
    /// clean one does not. This is what replaces `install_conflict_css`.
    ///
    /// Mutation check: add the class unconditionally; the `close` assertion
    /// fails. Restore.
    #[test]
    fn a_conflicting_row_carries_the_conflict_class() {
        let mut m = model_with_workspaces(1);
        let combo = icedtea_config::KeyCombo {
            modifiers: vec![],
            key: "KEY_a".to_string(),
        };
        m.model
            .working
            .keybindings
            .insert("close".to_string(), combo.clone());
        m.model
            .working
            .keybindings
            .insert("quit".to_string(), combo.clone());
        m.conflicts = crate::model::duplicate_bindings(&m.model.working);

        let page = view(&m);
        let rows = &page.children[0].children;
        let classes_of = |action: &str| -> Vec<String> {
            rows.iter()
                .find(|r| r.props.str(PropName::Id) == Some(format!("kb_row_{action}").as_str()))
                .and_then(|r| match r.children[1].props.get(PropName::Classes) {
                    Some(Prop::Classes(list)) => Some(list.iter().map(|c| c.to_string()).collect()),
                    _ => None,
                })
                .unwrap_or_default()
        };
        assert!(classes_of("close").contains(&"conflict".to_string()));
        assert!(classes_of("quit").contains(&"conflict".to_string()));
        assert!(
            !classes_of("reload").contains(&"conflict".to_string()),
            "an unconflicted row is not flagged"
        );
    }

    /// The settings sheet actually styles that class: a `label.conflict`
    /// wins a `color` declaration a plain `label` does not, once
    /// `settings/style.css` is layered over the Adwaita stack the way
    /// `main.rs` layers it.
    ///
    /// Mutation check: delete the `.conflict` rule from `settings/style.css`;
    /// this fails with equal winners. Restore.
    #[test]
    fn the_settings_sheet_colours_a_conflicting_binding() {
        let css = format!(
            "{}\n{}",
            icedtea_ui::BUNDLED_ADWAITA_LIGHT,
            include_str!("../../style.css")
        );
        let sheet = CompiledSheet::compile(&css);
        let mut cx = MatchCx::new();
        let window = Node::with_classes("window", &["background"]);
        let plain = Node::new("label");
        let flagged = Node::with_classes("label", &["conflict"]);
        window.append_child(&plain);
        window.append_child(&flagged);

        let plain_color = cascade(&sheet, &plain, &mut cx)
            .winner(CssProp::Color)
            .cloned();
        let flagged_color = cascade(&sheet, &flagged, &mut cx)
            .winner(CssProp::Color)
            .cloned();
        assert!(
            flagged_color.is_some(),
            "the .conflict rule declares a colour"
        );
        assert_ne!(
            plain_color, flagged_color,
            "a conflicting label must not resolve to the ordinary label colour"
        );
    }

    /// The capture lifecycle through the real `update` fold: arm, capture,
    /// disarm, and the conflicts list recomputed on the way out.
    ///
    /// Mutation check: drop the `m.capturing = None` from the
    /// `Msg::KeyCaptured` arm; the `is_none` assertion fails. Restore.
    #[test]
    fn a_capture_arms_stores_and_disarms() {
        let mut m = model_with_workspaces(1);
        let _ = crate::app::update(&mut m, Msg::CaptureArmed("close".to_string()));
        assert_eq!(m.capturing.as_deref(), Some("close"));

        let _ = crate::app::update(
            &mut m,
            Msg::KeyCaptured {
                keysym: xkbcommon::xkb::keysyms::KEY_a,
                mods: CaptureMods {
                    shift: true,
                    ..Default::default()
                },
            },
        );
        assert!(m.capturing.is_none(), "a stored capture disarms the row");
        let combo = m
            .model
            .working
            .keybindings
            .get("close")
            .expect("close is bound");
        assert_eq!(combo.key, "KEY_a");
        assert_eq!(combo.modifiers, vec!["SHIFT".to_string()]);
    }

    /// A lone modifier press keeps the row armed and writes nothing.
    ///
    /// Mutation check: clear `m.capturing` regardless of `apply_capture`'s
    /// answer; this fails. Restore.
    #[test]
    fn a_lone_modifier_press_leaves_the_capture_armed() {
        let mut m = model_with_workspaces(1);
        let before = m.model.working.keybindings.clone();
        let _ = crate::app::update(&mut m, Msg::CaptureArmed("close".to_string()));
        let _ = crate::app::update(
            &mut m,
            Msg::KeyCaptured {
                keysym: xkbcommon::xkb::keysyms::KEY_Super_L,
                mods: CaptureMods {
                    logo: true,
                    ..Default::default()
                },
            },
        );
        assert_eq!(
            m.capturing.as_deref(),
            Some("close"),
            "still waiting for the real key"
        );
        assert_eq!(m.model.working.keybindings, before);
    }

    /// `Msg::KeyCaptured` with nothing armed is inert -- the belt to
    /// `capture_key`'s braces (contract §2.7 rule 4).
    ///
    /// Mutation check: drop the `capturing.is_none()` guard; this fails
    /// with a spurious binding on whatever action was last armed. Restore.
    #[test]
    fn an_unarmed_key_captured_message_changes_nothing() {
        let mut m = model_with_workspaces(1);
        let before = m.model.working.keybindings.clone();
        let _ = crate::app::update(
            &mut m,
            Msg::KeyCaptured {
                keysym: xkbcommon::xkb::keysyms::KEY_a,
                mods: CaptureMods::default(),
            },
        );
        assert_eq!(m.model.working.keybindings, before);
    }

    /// Escape disarms without writing.
    ///
    /// Mutation check: make the arm a no-op; this fails. Restore.
    #[test]
    fn cancelling_disarms_without_writing() {
        let mut m = model_with_workspaces(1);
        let before = m.model.working.keybindings.clone();
        let _ = crate::app::update(&mut m, Msg::CaptureArmed("close".to_string()));
        let _ = crate::app::update(&mut m, Msg::CaptureCancelled);
        assert!(m.capturing.is_none());
        assert_eq!(m.model.working.keybindings, before);
    }

    /// Leaving the page disarms: the GTK `connect_unmap` /
    /// `EventControllerFocus` reset, now one line in the nav arm. Without
    /// it an armed capture would survive a page switch and rebind on the
    /// next keystroke typed anywhere.
    ///
    /// Mutation check: drop `m.capturing = None` from the
    /// `Msg::PageSelected` arm; this fails. Restore.
    #[test]
    fn switching_pages_cancels_a_capture() {
        let mut m = model_with_workspaces(1);
        let _ = crate::app::update(&mut m, Msg::CaptureArmed("close".to_string()));
        let _ = crate::app::update(&mut m, Msg::PageSelected(2));
        assert!(m.capturing.is_none(), "a page switch disarms");
        assert_eq!(m.page, crate::pages::PageId::Workspaces);
    }

    /// The conflicts list is recomputed after a capture, so the next
    /// `view` flags both halves of a fresh collision.
    ///
    /// Mutation check: drop the `duplicate_bindings` recompute; this fails
    /// with an empty list. Restore.
    #[test]
    fn a_capture_recomputes_the_conflicts_list() {
        let mut m = model_with_workspaces(1);
        m.model.working.keybindings.insert(
            "quit".to_string(),
            icedtea_config::KeyCombo {
                modifiers: vec![],
                key: "KEY_a".to_string(),
            },
        );
        let _ = crate::app::update(&mut m, Msg::CaptureArmed("close".to_string()));
        let _ = crate::app::update(
            &mut m,
            Msg::KeyCaptured {
                keysym: xkbcommon::xkb::keysyms::KEY_a,
                mods: CaptureMods::default(),
            },
        );
        let flagged: Vec<String> = m
            .conflicts
            .iter()
            .flat_map(|(_, actions)| actions.iter().cloned())
            .collect();
        assert!(flagged.contains(&"close".to_string()));
        assert!(flagged.contains(&"quit".to_string()));
    }

    /// The root handler is armed from the model: with nothing armed it
    /// declines every key, so a focused `Entry` keeps receiving text; with a
    /// capture armed it answers. Firing the root's `KeyPressed` handler
    /// directly is exactly what `GenericC::on_event` does.
    ///
    /// Mutation check: hard-code `true` for `armed` in `app::view`; the
    /// first assertion fails. Restore.
    #[test]
    fn the_root_key_handler_is_armed_from_the_model() {
        let mut m = model_with_workspaces(1);
        let ev = press(
            30,
            xkbcommon::xkb::keysyms::KEY_a,
            xkbcommon::xkb::keysyms::KEY_a,
            Mods::empty(),
        );

        let idle = crate::app::view(&m);
        assert!(
            idle.handlers.fire_key(EventKind::KeyPressed, &ev).is_none(),
            "an idle window must not swallow keystrokes"
        );

        m.capturing = Some("close".to_string());
        let armed = crate::app::view(&m);
        assert!(matches!(
            armed.handlers.fire_key(EventKind::KeyPressed, &ev),
            Some(Msg::KeyCaptured { .. })
        ));
    }
}
