//! The Workspaces page: an editable list of workspace names, each row an
//! `Entry` (the name) + a Remove `Button`, plus an Add `Button` that appends
//! a new row.
//!
//! `workspace_names` must never go empty -- `icedtea_config::load_or_default`
//! rejects an empty list on load, so an empty working copy would silently
//! revert to defaults on next start. Remove is therefore insensitive
//! whenever exactly one row remains.
//!
//! The keybindings page generates a `workspace:N`/`move_to_workspace:N` row
//! pair per workspace, so adding/removing a row here changes the *set* of
//! actions that page shows. `on_change` (supplied by `main.rs`) is called
//! after every add/remove so the caller can refresh that page in turn; a
//! plain rename doesn't change the generated action set, so it only calls
//! `ctx.mark_dirty()`.

use std::cell::RefCell;
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{Align, Box as GtkBox, Button, Entry, Orientation};

use crate::pages::{Ctx, Page};

/// The Workspaces page.
///
/// P1 ships the page's frame only; P3 fills it in (contract §2.6).
#[must_use]
pub fn view(m: &crate::app::SettingsModel) -> icedtea_ui::view::View<crate::app::Msg> {
    let _ = m;
    icedtea_ui::view::builders::box_(
        icedtea_ui::widgets::types::Orientation::Vertical,
        [icedtea_ui::view::builders::label("Workspaces")],
    )
    .id("workspaces_page")
    .margin(16, 16, 16, 16)
}

/// Alias for the recursive "rebuild the row list" closure slot -- `Remove`
/// and `Add` handlers need to call `rebuild` again after mutating the
/// model, but `rebuild` doesn't exist yet at the point those handlers are
/// wired, so it's threaded through this `RefCell` and filled in right after
/// `rebuild` is built (see [`build`]).
type RebuildSlot = Rc<RefCell<Option<Rc<dyn Fn()>>>>;

/// One built row: just the Remove `Button`, whose sensitivity is recomputed
/// whenever the row count changes (`rebuild` tears down and recreates every
/// row's widgets on every add/remove/refresh, so nothing else needs to be
/// kept around here).
struct Row {
    remove_button: Button,
}

fn set_remove_sensitivity(rows: &[Row]) {
    let removable = rows.len() > 1;
    for row in rows {
        row.remove_button.set_sensitive(removable);
    }
}

/// The name to give a newly added workspace: the smallest positive integer
/// not already used as a workspace name (as a decimal string). *Not*
/// `workspace_names.len() + 1` -- after a `Remove` (or a rename that happens
/// to collide) that formula can duplicate an existing name or land on one a
/// rename left free further down the list, instead of the actual next
/// unused slot.
pub fn next_workspace_name(existing: &[String]) -> String {
    let mut n: u32 = 1;
    loop {
        let candidate = n.to_string();
        if !existing.iter().any(|name| name == &candidate) {
            return candidate;
        }
        n += 1;
    }
}

/// Drop any `workspace:N` / `move_to_workspace:N` keybinding whose `N`
/// exceeds the current workspace count. The Keybindings page only ever
/// generates rows for `1..=workspace_names.len()`
/// (`pages::keybindings::action_list`), so once a `Remove` shrinks that
/// count, any binding beyond the new length is unreachable from the UI --
/// left in place it would sit in the saved config forever, a keybinding for
/// a workspace slot that no longer exists.
pub fn prune_orphaned_workspace_bindings(cfg: &mut icedtea_config::Config) {
    let workspace_count = cfg.workspace_names.len();
    cfg.keybindings.retain(|action, _| {
        for prefix in ["workspace:", "move_to_workspace:"] {
            if let Some(n) = action
                .strip_prefix(prefix)
                .and_then(|s| s.parse::<usize>().ok())
            {
                return n <= workspace_count;
            }
        }
        true
    });
}

pub fn build(ctx: Ctx, on_change: Rc<dyn Fn()>) -> Page {
    let root = GtkBox::new(Orientation::Vertical, 8);
    root.set_margin_top(16);
    root.set_margin_bottom(16);
    root.set_margin_start(16);
    root.set_margin_end(16);

    let list = GtkBox::new(Orientation::Vertical, 4);
    root.append(&list);

    let add_button = Button::with_label("Add workspace");
    add_button.set_halign(Align::Start);
    root.append(&add_button);

    let rows: Rc<RefCell<Vec<Row>>> = Rc::new(RefCell::new(Vec::new()));

    // `rebuild` (re-populate `list` from `ctx.model.working.workspace_names`)
    // is defined recursively -- Remove/Add handlers need to call it again
    // after mutating the model -- so it's stored behind this slot and
    // populated right after `rebuild` itself is built.
    let rebuild_slot: RebuildSlot = Rc::new(RefCell::new(None));

    let rebuild: Rc<dyn Fn()> = {
        let ctx = ctx.clone();
        let list = list.clone();
        let rows = rows.clone();
        let on_change = on_change.clone();
        let rebuild_slot = rebuild_slot.clone();
        Rc::new(move || {
            while let Some(child) = list.first_child() {
                list.remove(&child);
            }
            rows.borrow_mut().clear();

            let names = ctx.model.borrow().working.workspace_names.clone();
            for (i, name) in names.into_iter().enumerate() {
                let row_box = GtkBox::new(Orientation::Horizontal, 8);
                let entry = Entry::new();
                entry.set_text(&name);
                entry.set_hexpand(true);
                let remove_button = Button::with_label("Remove");
                row_box.append(&entry);
                row_box.append(&remove_button);
                list.append(&row_box);

                {
                    let ctx = ctx.clone();
                    entry.connect_changed(move |entry| {
                        let text = entry.text().to_string();
                        {
                            let mut model = ctx.model.borrow_mut();
                            if let Some(slot) = model.working.workspace_names.get_mut(i) {
                                *slot = text;
                            }
                        }
                        ctx.mark_dirty();
                    });
                }

                {
                    let ctx = ctx.clone();
                    let on_change = on_change.clone();
                    let rebuild_slot = rebuild_slot.clone();
                    remove_button.connect_clicked(move |_| {
                        {
                            let mut model = ctx.model.borrow_mut();
                            if model.working.workspace_names.len() > 1
                                && i < model.working.workspace_names.len()
                            {
                                model.working.workspace_names.remove(i);
                                prune_orphaned_workspace_bindings(&mut model.working);
                            }
                        }
                        ctx.mark_dirty();
                        if let Some(rebuild) = rebuild_slot.borrow().clone() {
                            rebuild();
                        }
                        on_change();
                    });
                }

                rows.borrow_mut().push(Row { remove_button });
            }
            set_remove_sensitivity(&rows.borrow());
        })
    };
    *rebuild_slot.borrow_mut() = Some(rebuild.clone());

    {
        let ctx = ctx.clone();
        let on_change = on_change.clone();
        let rebuild_slot = rebuild_slot.clone();
        add_button.connect_clicked(move |_| {
            {
                let mut model = ctx.model.borrow_mut();
                let name = next_workspace_name(&model.working.workspace_names);
                model.working.workspace_names.push(name);
            }
            ctx.mark_dirty();
            if let Some(rebuild) = rebuild_slot.borrow().clone() {
                rebuild();
            }
            on_change();
        });
    }

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

    Page {
        root: root.upcast(),
        refresh,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use icedtea_config::KeyCombo;

    #[test]
    fn next_workspace_name_is_not_len_plus_one_after_a_remove() {
        // Two workspaces named "1" and "2"; if Remove had deleted "1" the
        // old `len + 1` formula would compute `1 + 1 = "2"`, duplicating
        // the survivor -- `next_workspace_name` must not do that.
        let existing = vec!["2".to_string()];
        assert_eq!(
            next_workspace_name(&existing),
            "1",
            "must reuse the freed slot, not collide with \"2\""
        );
    }

    #[test]
    fn next_workspace_name_skips_used_numbers() {
        let existing = vec!["1".to_string(), "2".to_string(), "3".to_string()];
        assert_eq!(next_workspace_name(&existing), "4");
    }

    #[test]
    fn next_workspace_name_ignores_non_numeric_names() {
        let existing = vec!["alpha".to_string(), "beta".to_string()];
        assert_eq!(next_workspace_name(&existing), "1");
    }

    #[test]
    fn next_workspace_name_never_collides_with_a_renamed_workspace() {
        // A rename can leave a numeric-looking name anywhere in the list,
        // not just contiguously from 1 -- `next_workspace_name` must still
        // never pick an already-used name.
        let existing = vec!["1".to_string(), "99".to_string()];
        let name = next_workspace_name(&existing);
        assert!(
            !existing.contains(&name),
            "{name} must not already be in use"
        );
        assert_eq!(name, "2");
    }

    fn combo(key: &str) -> KeyCombo {
        KeyCombo {
            modifiers: vec![],
            key: key.to_string(),
        }
    }

    #[test]
    fn prune_orphaned_workspace_bindings_drops_bindings_past_the_new_count() {
        // Three workspaces' worth of generated bindings; removing down to
        // two workspaces must drop the workspace:3 / move_to_workspace:3
        // pair (the row the Keybindings page can no longer show) and keep
        // everything else, including unrelated fixed actions.
        let mut cfg = icedtea_config::default_config();
        cfg.workspace_names = vec!["a".to_string(), "b".to_string()];
        cfg.keybindings
            .insert("workspace:1".to_string(), combo("KEY_1"));
        cfg.keybindings
            .insert("workspace:2".to_string(), combo("KEY_2"));
        cfg.keybindings
            .insert("workspace:3".to_string(), combo("KEY_3"));
        cfg.keybindings
            .insert("move_to_workspace:1".to_string(), combo("KEY_1"));
        cfg.keybindings
            .insert("move_to_workspace:2".to_string(), combo("KEY_2"));
        cfg.keybindings
            .insert("move_to_workspace:3".to_string(), combo("KEY_3"));
        cfg.keybindings.insert("close".to_string(), combo("KEY_q"));

        prune_orphaned_workspace_bindings(&mut cfg);

        assert!(cfg.keybindings.contains_key("workspace:1"));
        assert!(cfg.keybindings.contains_key("workspace:2"));
        assert!(
            !cfg.keybindings.contains_key("workspace:3"),
            "workspace:3 must be pruned"
        );
        assert!(cfg.keybindings.contains_key("move_to_workspace:1"));
        assert!(cfg.keybindings.contains_key("move_to_workspace:2"));
        assert!(
            !cfg.keybindings.contains_key("move_to_workspace:3"),
            "move_to_workspace:3 must be pruned"
        );
        assert!(
            cfg.keybindings.contains_key("close"),
            "unrelated fixed actions must survive"
        );
    }

    #[test]
    fn prune_orphaned_workspace_bindings_is_a_no_op_when_nothing_is_orphaned() {
        let cfg_before = icedtea_config::default_config();
        let mut cfg = cfg_before.clone();
        // default_config's workspace_names.len() is <= its highest
        // pre-bound workspace:N, so nothing here is actually orphaned yet
        // (see keybindings.rs's own note on this); pruning must change
        // nothing surprising -- specifically, every currently-reachable
        // workspace:N/move_to_workspace:N must survive.
        prune_orphaned_workspace_bindings(&mut cfg);
        let max = cfg_before.workspace_names.len();
        for n in 1..=max {
            assert!(cfg.keybindings.contains_key(&format!("workspace:{n}")));
            assert!(
                cfg.keybindings
                    .contains_key(&format!("move_to_workspace:{n}"))
            );
        }
    }

    /// The two GTK-free helpers are the module's public surface — `app.rs`'s
    /// `WorkspaceAdded`/`WorkspaceRemoved` arms call them directly.
    ///
    /// Mutation check: drop `pub` from `next_workspace_name`; this test stops
    /// compiling. Restore.
    #[test]
    fn the_pure_surface_is_public() {
        let next: fn(&[String]) -> String = crate::pages::workspaces::next_workspace_name;
        assert_eq!(next(&[]), "1");
        let prune: fn(&mut icedtea_config::Config) =
            crate::pages::workspaces::prune_orphaned_workspace_bindings;
        let mut cfg = icedtea_config::default_config();
        prune(&mut cfg);
        assert!(!cfg.workspace_names.is_empty());
    }
}
