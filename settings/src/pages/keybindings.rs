//! The Keybindings page: a scrollable list, one row per action, showing its
//! current `KeyCombo` and a **Set** button that puts the row into capture
//! mode (next key press becomes the new binding).
//!
//! The action set is the fixed set the compositor always recognizes
//! (`close`, `fullscreen`, ...) plus `workspace:N`/`move_to_workspace:N`
//! generated for `1..=workspace_names.len()` -- so this page's row set
//! tracks the Workspaces page's row count (`main.rs` refreshes this page
//! whenever that one adds/removes a workspace).

use std::cell::RefCell;
use std::rc::Rc;

use gtk4::glib::translate::IntoGlib;
use gtk4::prelude::*;
use gtk4::{
    gdk, glib, Box as GtkBox, Button, EventControllerFocus, EventControllerKey, Label, Orientation,
    PropagationPhase, ScrolledWindow,
};

use icedtea_config::KeyCombo;

use crate::model::{combo_from_keysym, duplicate_bindings, CaptureMods};
use crate::pages::{Ctx, Page};

/// The fixed part of the action set -- always present regardless of
/// workspace count. Kept in the same order the compositor's own defaults
/// insert them (`icedtea_config::defaults::default_config`) purely so the
/// row order is stable/predictable, not because order carries meaning here.
const FIXED_ACTIONS: [&str; 10] = [
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
const SNAP_RESTORE: &str = "snap:restore";

/// The full action set for a config with `workspace_count` workspaces: the
/// fixed actions plus a generated `workspace:N`/`move_to_workspace:N` pair
/// for every `1..=workspace_count`.
fn action_list(workspace_count: usize) -> Vec<String> {
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
fn format_combo(combo: &KeyCombo) -> String {
    let key = combo.key.strip_prefix("KEY_").unwrap_or(&combo.key);
    let mut parts = combo.modifiers.clone();
    parts.push(key.to_string());
    parts.join("+")
}

const CONFLICT_CSS_CLASS: &str = "keybinding-conflict";

/// One built row: the action name (the model key), the combo label to keep
/// in sync, and the Set button (whose label flips to "Press a key..." while
/// this row is capturing).
struct Row {
    action: String,
    combo_label: Label,
    set_button: Button,
}

/// Refresh every row's combo label from `cfg` and re-run
/// `duplicate_bindings` to add/remove the conflict CSS class -- called after
/// any binding change (a capture completing) and by `refresh()`.
fn sync_rows(rows: &[Row], cfg: &icedtea_config::Config) {
    let conflicts = duplicate_bindings(cfg);
    for row in rows {
        if let Some(combo) = cfg.keybindings.get(&row.action) {
            row.combo_label.set_label(&format_combo(combo));
        } else {
            row.combo_label.set_label("(unbound)");
        }
        let colliding = conflicts.iter().any(|(_, actions)| actions.contains(&row.action));
        if colliding {
            row.combo_label.add_css_class(CONFLICT_CSS_CLASS);
        } else {
            row.combo_label.remove_css_class(CONFLICT_CSS_CLASS);
        }
    }
}

/// Whether the shared window key controller should consume a key press as a
/// binding capture. The controller lives on the *window* in the capture phase,
/// so it fires for keystrokes typed on *any* Stack page; it must only act when
/// the Keybindings page is the visible child (`page_visible`) AND a capture is
/// actually armed (`capturing`). Extracted so the gate is unit-testable without
/// a live GTK display. (#4)
fn should_capture(page_visible: bool, capturing: bool) -> bool {
    page_visible && capturing
}

/// Clear an armed capture and report which action's row must have its Set
/// button restored (`None` when nothing was armed). Pure so the reset decision
/// is unit-testable; the caller performs the widget label restore. (#4)
fn take_capture_reset(capturing: &mut Option<String>) -> Option<String> {
    capturing.take()
}

/// Cancel any armed capture and restore the mid-capture row's Set button label.
/// Wired to leaving the Keybindings page (root `unmap`, i.e. the Stack switching
/// to another child) and to focus leaving the page, so an armed capture can
/// never survive a page switch and hijack keystrokes typed elsewhere. (#4)
fn reset_capture(capturing: &Rc<RefCell<Option<String>>>, rows: &Rc<RefCell<Vec<Row>>>) {
    let reset = take_capture_reset(&mut capturing.borrow_mut());
    if let Some(action) = reset
        && let Some(row) = rows.borrow().iter().find(|r| r.action == action)
    {
        row.set_button.set_label("Set");
    }
}

/// Resolve the group-0/level-0 keysym for a captured hardware `keycode` --
/// the layout-agnostic, un-shifted keysym the compositor's own matcher
/// compares against (`compositor/src/input.rs` matches on the keysym as
/// delivered with *no* level/group adjustment applied by us -- it relies on
/// bindings being stored in their base form). `EventControllerKey`'s
/// `keyval` is *already* shift/caps-adjusted by GDK (e.g. Shift+q ->
/// `KEY_Q`, Shift+1 -> `KEY_exclam`), so capturing straight from `keyval`
/// would silently store a binding that never matches a real, unshifted key
/// press. Asking the display to translate the same `keycode` with an empty
/// modifier state and group 0 gives back the level-0 keysym regardless of
/// what Shift/Caps-Lock/group was actually active during capture.
///
/// Falls back to `fallback_keysym` (the raw event `keyval`) if the display
/// or its keymap can't translate the keycode -- should not happen for a
/// real key-press event, but keeps capture panic-free against an exotic or
/// absent keymap.
pub fn unshifted_keysym(keycode: u32, fallback_keysym: u32) -> u32 {
    gdk::Display::default()
        .and_then(|display| display.translate_key(keycode, gdk::ModifierType::empty(), 0))
        .map(|(key, _group, _level, _consumed)| key.into_glib())
        .unwrap_or(fallback_keysym)
}

/// Install the CSS provider for [`CONFLICT_CSS_CLASS`] on the default
/// display, once. Purely advisory styling (a red label) -- see the brief's
/// Step 4: conflicts never block Apply.
fn install_conflict_css() {
    let Some(display) = gdk::Display::default() else { return };
    let provider = gtk4::CssProvider::new();
    provider.load_from_data(&format!(".{CONFLICT_CSS_CLASS} {{ color: #f38ba8; font-weight: bold; }}"));
    gtk4::style_context_add_provider_for_display(&display, &provider, gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION);
}

pub fn build(ctx: Ctx) -> Page {
    install_conflict_css();

    let scroller = ScrolledWindow::new();
    scroller.set_vexpand(true);
    scroller.set_hexpand(true);

    let list = GtkBox::new(Orientation::Vertical, 4);
    list.set_margin_top(16);
    list.set_margin_bottom(16);
    list.set_margin_start(16);
    list.set_margin_end(16);
    scroller.set_child(Some(&list));

    let rows: Rc<RefCell<Vec<Row>>> = Rc::new(RefCell::new(Vec::new()));

    // The action currently in capture mode (set by a row's Set button,
    // cleared on a successful capture or Esc). `None` means the shared key
    // controller below ignores key presses entirely, so normal window
    // interaction (Tab-focus, etc.) is unaffected outside capture mode.
    let capturing: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));

    let key_controller = EventControllerKey::new();
    key_controller.set_propagation_phase(PropagationPhase::Capture);
    {
        // Capture only the pieces of `Ctx` the handler actually needs (the
        // shared model + the dirty plumbing) rather than the whole `Ctx`, whose
        // `window` field is the very window this controller is attached to.
        // Cloning `Ctx` here would form a window -> controller -> Ctx -> window
        // strong reference cycle that leaks the window; these `Rc`s never point
        // back at the window, so the cycle is broken. (#4)
        let model = ctx.model.clone();
        let on_dirty = ctx.on_dirty.clone();
        let populating = ctx.populating.clone();
        let rows = rows.clone();
        let capturing = capturing.clone();
        let scroller = scroller.clone();
        key_controller.connect_key_pressed(move |_controller, keyval, keycode, state| {
            // The controller is window-scoped in the capture phase, so gate on
            // the page being visible: never touch keystrokes typed on another
            // Stack page even if a capture was somehow left armed. (#4)
            if !should_capture(scroller.is_mapped(), capturing.borrow().is_some()) {
                return glib::Propagation::Proceed;
            }
            let Some(action) = capturing.borrow().clone() else {
                return glib::Propagation::Proceed;
            };

            if keyval == gdk::Key::Escape {
                *capturing.borrow_mut() = None;
                if let Some(row) = rows.borrow().iter().find(|r| r.action == action) {
                    row.set_button.set_label("Set");
                }
                return glib::Propagation::Stop;
            }

            let mods = CaptureMods {
                ctrl: state.contains(gdk::ModifierType::CONTROL_MASK),
                alt: state.contains(gdk::ModifierType::ALT_MASK),
                shift: state.contains(gdk::ModifierType::SHIFT_MASK),
                logo: state.contains(gdk::ModifierType::SUPER_MASK),
            };

            // If `combo_from_keysym` returns `None` (a lone modifier press),
            // fall through leaving `capturing` set -- stay in capture mode
            // and wait for the "real" key.
            let keysym = unshifted_keysym(keycode, keyval.into_glib());
            if let Some(combo) = combo_from_keysym(keysym, mods) {
                model.borrow_mut().working.keybindings.insert(action.clone(), combo);
                if !populating.get() {
                    (on_dirty)();
                }
                *capturing.borrow_mut() = None;
                let rows_ref = rows.borrow();
                if let Some(row) = rows_ref.iter().find(|r| r.action == action) {
                    row.set_button.set_label("Set");
                }
                sync_rows(&rows_ref, &model.borrow().working);
            }
            glib::Propagation::Stop
        });
    }
    ctx.window.add_controller(key_controller);

    // Cancel an armed capture whenever the page stops being the visible Stack
    // child (GtkStack unmaps the non-visible child, so the root's `unmap` fires
    // on a page switch away) or when focus leaves the page entirely. Without
    // this, a capture armed here would stay armed across a page switch and the
    // window-scoped controller would silently rebind + swallow keys typed on
    // another page. (#4)
    {
        let capturing = capturing.clone();
        let rows = rows.clone();
        scroller.connect_unmap(move |_| reset_capture(&capturing, &rows));
    }
    {
        let focus = EventControllerFocus::new();
        let capturing = capturing.clone();
        let rows = rows.clone();
        focus.connect_leave(move |_| reset_capture(&capturing, &rows));
        scroller.add_controller(focus);
    }

    // `rebuild` tears down and recreates every row from
    // `ctx.model.working` -- used both for the initial populate and for
    // `refresh()` (Revert, and workspace-count changes forwarded from the
    // Workspaces page).
    let rebuild: Rc<dyn Fn()> = {
        let ctx = ctx.clone();
        let list = list.clone();
        let rows = rows.clone();
        let capturing = capturing.clone();
        Rc::new(move || {
            *capturing.borrow_mut() = None;
            while let Some(child) = list.first_child() {
                list.remove(&child);
            }
            rows.borrow_mut().clear();

            let cfg = ctx.model.borrow().working.clone();
            let actions = action_list(cfg.workspace_names.len());

            for action in actions {
                let row_box = GtkBox::new(Orientation::Horizontal, 12);
                let action_label = Label::new(Some(&action));
                action_label.set_width_chars(24);
                action_label.set_xalign(0.0);

                let combo_label = Label::new(None);
                combo_label.set_width_chars(20);
                combo_label.set_xalign(0.0);
                combo_label.set_hexpand(true);

                let set_button = Button::with_label("Set");

                row_box.append(&action_label);
                row_box.append(&combo_label);
                row_box.append(&set_button);
                list.append(&row_box);

                {
                    let capturing = capturing.clone();
                    let rows = rows.clone();
                    let action = action.clone();
                    set_button.connect_clicked(move |button| {
                        // Starting a new capture supersedes any row already
                        // pending -- reset its Set button back to "Set" so
                        // it doesn't stay stuck on "Press a key..." forever
                        // once this row steals capture focus.
                        if let Some(previous) = capturing.borrow_mut().replace(action.clone())
                            && previous != action
                            && let Some(row) = rows.borrow().iter().find(|r| r.action == previous)
                        {
                            row.set_button.set_label("Set");
                        }
                        button.set_label("Press a key…");
                        button.grab_focus();
                    });
                }

                rows.borrow_mut().push(Row { action, combo_label, set_button });
            }

            sync_rows(&rows.borrow(), &cfg);
        })
    };

    let refresh: Rc<dyn Fn()> = {
        let ctx = ctx.clone();
        let rebuild = rebuild.clone();
        Rc::new(move || {
            ctx.populating.set(true);
            rebuild();
            ctx.populating.set(false);
        })
    };
    refresh();

    Page { root: scroller.upcast(), refresh }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_list_is_fixed_plus_generated_workspace_pairs() {
        let actions = action_list(2);
        for fixed in FIXED_ACTIONS {
            assert!(actions.contains(&fixed.to_string()), "{fixed} must be present");
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
            assert!(cfg.keybindings.contains_key(&ws), "default_config must bind {ws}");
            assert!(cfg.keybindings.contains_key(&mv), "default_config must bind {mv}");
            assert!(actions.contains(&ws));
            assert!(actions.contains(&mv));
        }
        for action in FIXED_ACTIONS.iter().chain(std::iter::once(&SNAP_RESTORE)) {
            assert!(cfg.keybindings.contains_key(*action), "default binding {action:?} missing from default_config");
        }
    }

    #[test]
    fn format_combo_renders_super_shift_key() {
        let combo = KeyCombo { modifiers: vec!["SUPER".to_string(), "SHIFT".to_string()], key: "KEY_q".to_string() };
        assert_eq!(format_combo(&combo), "SUPER+SHIFT+q");
    }

    #[test]
    fn format_combo_with_no_modifiers() {
        let combo = KeyCombo { modifiers: vec![], key: "KEY_F5".to_string() };
        assert_eq!(format_combo(&combo), "F5");
    }

    #[test]
    fn should_capture_only_when_page_visible_and_armed() {
        // Both conditions required: an armed capture on a hidden page (the user
        // switched away without completing) must NOT consume keystrokes.
        assert!(should_capture(true, true));
        assert!(!should_capture(false, true), "armed but page hidden: must not hijack");
        assert!(!should_capture(true, false), "visible but nothing armed: pass through");
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
}
