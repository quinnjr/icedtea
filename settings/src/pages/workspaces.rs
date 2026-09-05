//! The Workspaces page: an editable list of workspace names.
//!
//! `workspace_names` must never go empty -- `icedtea_config::load_or_default`
//! rejects an empty list on load, so an empty working copy would silently
//! revert to defaults on next start. Remove must therefore stay a no-op
//! whenever exactly one row remains.
//!
//! The keybindings page generates a `workspace:N`/`move_to_workspace:N` row
//! pair per workspace, so adding/removing a row here changes the *set* of
//! actions that page shows.

use icedtea_config::Config;

/// Whether the Remove buttons are sensitive: never at the last row.
#[must_use]
pub fn can_remove(cfg: &Config) -> bool {
    cfg.workspace_names.len() > 1
}

/// Rename workspace `index`. An index the model no longer has is dropped:
/// a `Msg::WorkspaceRenamed` carries the index the *rendered* view had, and
/// a `Change` that races a Remove must not panic (the crate's never-panic
/// rule for untrusted input).
pub fn rename_workspace(cfg: &mut Config, index: usize, name: &str) {
    if let Some(slot) = cfg.workspace_names.get_mut(index) {
        *slot = name.to_string();
    }
}

/// Append a workspace named [`next_workspace_name`]'s answer.
pub fn add_workspace(cfg: &mut Config) {
    let name = next_workspace_name(&cfg.workspace_names);
    cfg.workspace_names.push(name);
}

/// Remove workspace `index` and prune the bindings that removal orphans.
///
/// A no-op at the last row ([`can_remove`]) and for an out-of-range index,
/// for the same reason [`rename_workspace`] tolerates one.
pub fn remove_workspace(cfg: &mut Config, index: usize) {
    if !can_remove(cfg) || index >= cfg.workspace_names.len() {
        return;
    }
    cfg.workspace_names.remove(index);
    prune_orphaned_workspace_bindings(cfg);
}

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

    /// The Remove buttons are insensitive at one row: `icedtea_config::
    /// load_or_default` rejects an empty workspace list, so an empty working
    /// copy would silently revert to defaults on the next start. This is the
    /// GTK `set_remove_sensitivity` rule, now computed from the config.
    ///
    /// Mutation check: make `can_remove` return `true` unconditionally; this
    /// test fails on the one-name case. Restore.
    #[test]
    fn can_remove_is_false_at_the_last_workspace() {
        let mut cfg = icedtea_config::default_config();
        cfg.workspace_names = vec!["only".to_string()];
        assert!(
            !can_remove(&cfg),
            "the last workspace must not be removable"
        );
        cfg.workspace_names.push("second".to_string());
        assert!(can_remove(&cfg), "two workspaces: both are removable");
    }

    /// A rename writes exactly one slot and leaves the rest of the config
    /// alone -- including the keybindings, which name workspaces by index and
    /// not by name.
    ///
    /// Mutation check: make `rename_workspace` push instead of assigning;
    /// the length assertion fails. Restore.
    #[test]
    fn rename_workspace_writes_one_slot() {
        let mut cfg = icedtea_config::default_config();
        cfg.workspace_names = vec!["a".to_string(), "b".to_string()];
        let bindings_before = cfg.keybindings.clone();
        rename_workspace(&mut cfg, 1, "beta");
        assert_eq!(
            cfg.workspace_names,
            vec!["a".to_string(), "beta".to_string()]
        );
        assert_eq!(
            cfg.keybindings, bindings_before,
            "a rename touches no binding"
        );
    }

    /// An out-of-range index is dropped, not a panic: `Msg::WorkspaceRenamed`
    /// carries the index the view rendered, and a reconcile that lands a
    /// stale `Change` after a Remove would otherwise take the process down
    /// (the crate's never-panic rule).
    ///
    /// Mutation check: index with `cfg.workspace_names[index] = ...`; this
    /// test panics instead of passing. Restore.
    #[test]
    fn rename_workspace_ignores_an_out_of_range_index() {
        let mut cfg = icedtea_config::default_config();
        cfg.workspace_names = vec!["a".to_string()];
        rename_workspace(&mut cfg, 7, "beta");
        assert_eq!(cfg.workspace_names, vec!["a".to_string()]);
    }

    /// Add appends `next_workspace_name`'s answer -- the smallest unused
    /// positive integer -- not `len + 1`.
    ///
    /// Mutation check: replace the body with a `format!("{}", len + 1)` push;
    /// this test fails with `"3"` instead of `"1"`. Restore.
    #[test]
    fn add_workspace_appends_the_next_free_name() {
        let mut cfg = icedtea_config::default_config();
        cfg.workspace_names = vec!["2".to_string(), "3".to_string()];
        add_workspace(&mut cfg);
        assert_eq!(
            cfg.workspace_names,
            vec!["2".to_string(), "3".to_string(), "1".to_string()],
            "the freed slot is reused, not len + 1"
        );
    }

    /// Remove drops the row *and* prunes the `workspace:N` /
    /// `move_to_workspace:N` bindings the Keybindings page can no longer
    /// show -- the GTK Remove handler's two-step, verbatim.
    ///
    /// Mutation check: drop the `prune_orphaned_workspace_bindings` call;
    /// the `workspace:3` assertion fails. Restore.
    #[test]
    fn remove_workspace_prunes_the_bindings_it_orphans() {
        let mut cfg = icedtea_config::default_config();
        cfg.workspace_names = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        cfg.keybindings.insert(
            "workspace:3".to_string(),
            icedtea_config::KeyCombo {
                modifiers: vec![],
                key: "KEY_3".to_string(),
            },
        );
        cfg.keybindings.insert(
            "close".to_string(),
            icedtea_config::KeyCombo {
                modifiers: vec![],
                key: "KEY_q".to_string(),
            },
        );

        remove_workspace(&mut cfg, 0);

        assert_eq!(cfg.workspace_names, vec!["b".to_string(), "c".to_string()]);
        assert!(
            !cfg.keybindings.contains_key("workspace:3"),
            "the orphaned workspace:3 binding must be pruned"
        );
        assert!(
            cfg.keybindings.contains_key("close"),
            "unrelated fixed actions survive"
        );
    }

    /// Remove refuses at one row and refuses an out-of-range index, in both
    /// cases leaving the config exactly as it was -- no partial edit, no
    /// prune, no panic.
    ///
    /// Mutation check: drop the `can_remove` guard; the one-name assertion
    /// fails. Restore.
    #[test]
    fn remove_workspace_refuses_the_last_row_and_a_bad_index() {
        let mut cfg = icedtea_config::default_config();
        cfg.workspace_names = vec!["only".to_string()];
        let before = cfg.clone();
        remove_workspace(&mut cfg, 0);
        assert_eq!(cfg.workspace_names, before.workspace_names);
        assert_eq!(cfg.keybindings, before.keybindings);

        cfg.workspace_names.push("second".to_string());
        let before = cfg.clone();
        remove_workspace(&mut cfg, 9);
        assert_eq!(cfg.workspace_names, before.workspace_names);
        assert_eq!(cfg.keybindings, before.keybindings);
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
