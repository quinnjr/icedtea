//! M5 close-out: contract §4.2's settings half, as data.
//!
//! The milestone rule this part is judged on is "every deletion is paired with
//! the test that proves the replacement". The pairing is the
//! `DELETED_WITH_REPLACEMENT` table below, so it cannot drift from a
//! paragraph of prose (contract deviation P6-D6).
//!
//! Hermetic: paths and file contents only, no `cargo`, no compositor.
//!
//! Reconciled against the tree at commit `36862f6` (task-3 report has the
//! full diff against the task-3 brief, which was written against
//! `develop @ d9cee52` before P1-P5 executed):
//!  - `settings/tests/no_gtk.rs` (deleted in this part's Task 1) is added to
//!    `DELETED_WITH_REPLACEMENT`, paired with `dependency_audit.rs`.
//!  - `DELETED_IDENTIFIERS`' `"populating"` pattern narrowed to `"populating:"`
//!    -- the bare word false-positives on `src/app.rs`'s own doc comment
//!    explaining the deletion ("the GTK `Ctx`/`Page`/`populating` machinery
//!    has no equivalent here").
//!  - `MOVED_SUITES` counts updated to what P1-P4 actually shipped:
//!    `displays/state.rs` 12 -> 13, `workspaces.rs` 6 -> 18, `keybindings.rs`
//!    6 -> 32, `appearance.rs` 2 -> 15. `displays_canvas.rs` (8),
//!    `model.rs` (10) and `compositor_reload.rs` (1) were unchanged.
//!  - Six `REQUIRED_GATES` entries shipped under a different name than the
//!    contract's singular one: P2/P3/P4 split each "paints at rest" gate into
//!    three theme variants (`..._in_the_light_theme` /
//!    `_dark_theme` / `_high_contrast_theme`); this table checks the light
//!    variant, which is sufficient proof the gate exists (each rename
//!    verified deliberate via `git log -S<name>`, cited per row below).
//!    `removing_a_workspace_prunes_its_bindings` shipped as
//!    `remove_workspace_prunes_the_bindings_it_orphans` (P3, commit
//!    `c6edb97`).

use std::path::{Path, PathBuf};

/// `(deleted path, the path that replaced it, the test that proves the
/// replacement behaves)`. Every path is workspace-relative.
///
/// `settings/tests/appearance_gtk.rs` is the one deletion with **no**
/// replacement test, by ruling: its
/// `populate_never_writes_a_widget_fallback_back_into_the_model` guarded a
/// property that an Elm loop makes true by construction (contract §2.2), and
/// the pairing is therefore the contract §6 record, not a test. It is listed
/// with that record as its replacement so the table stays total.
const DELETED_WITH_REPLACEMENT: &[(&str, &str, &str)] = &[
    (
        "settings/src/outputs/client.rs",
        "settings/src/outputs/pump.rs",
        "settings/tests/outputs_client.rs (kept verbatim) + \
         displays_page_paints_every_probe_point_at_rest_in_the_light_theme",
    ),
    (
        "settings/src/pages/displays.rs",
        "settings/src/pages/displays/state.rs",
        "the thirteen moved tests in displays/state.rs + \
         dragging_a_head_snaps_it_and_updates_the_model",
    ),
    (
        "settings/tests/appearance_gtk.rs",
        "docs/superpowers/plans/2026-09-03-m5-part0-contract.md",
        "no test: §6 records the property as true by construction on an Elm loop",
    ),
    (
        "settings/tests/keybindings_gtk.rs",
        "settings/tests/keybindings.rs",
        "shift_a_while_capturing_records_base_a_with_shift",
    ),
    (
        "settings/tests/no_gtk.rs",
        "settings/tests/dependency_audit.rs",
        "the_settings_manifest_declares_no_gtk_dependency + \
         the_workspace_lockfile_has_no_gtk_stack_package",
    ),
];

/// `(exact source pattern that must not appear anywhere in settings/, where
/// it used to live)`. Patterns are deliberately narrow: `Page` alone would
/// match `PageId` and `stack_page`, `Row` would match `list_box_row`.
///
/// `"populating:"` (not the bare word `"populating"`): `src/app.rs`'s own doc
/// comment on `SettingsModel` legitimately says "the GTK `Ctx`/`Page`/
/// `populating` machinery has no equivalent here" -- prose explaining the
/// deletion, not a survival of it. The field declaration form `populating:`
/// (and its only other real occurrence, a field access) is what the deleted
/// `Ctx` type actually carried, and does not appear in that sentence.
const DELETED_IDENTIFIERS: &[(&str, &str)] = &[
    (
        "pub struct Ctx",
        "pages/mod.rs -- the shared model/window/on_dirty bundle",
    ),
    ("pub struct Page {", "pages/mod.rs -- the root+refresh pair"),
    ("fn mark_dirty", "pages/mod.rs -- Ctx::mark_dirty"),
    (
        "populating:",
        "pages/mod.rs -- Ctx's Rc<Cell<bool>> write-back guard field",
    ),
    (
        "fn unshifted_keysym",
        "pages/keybindings.rs -- replaced by normalise_keysym + KeyEvent::base",
    ),
    (
        "fn install_conflict_css",
        "pages/keybindings.rs -- replaced by settings/style.css's .conflict rule",
    ),
    (
        "fn sync_rows",
        "pages/keybindings.rs -- replaced by view(&model)",
    ),
    (
        "fn reset_capture",
        "pages/keybindings.rs -- replaced by Msg::CaptureCancelled",
    ),
    (
        "struct Row",
        "pages/workspaces.rs -- the per-row widget handle",
    ),
    (
        "fn set_remove_sensitivity",
        "pages/workspaces.rs -- now .sensitive(names.len() > 1)",
    ),
    (
        "pub fn build(",
        "every page -- replaced by pub fn view(&SettingsModel) -> View<Msg>",
    ),
    (
        "keybindings_page_slot",
        "main.rs -- the forward-reference slot",
    ),
];

/// `(path, its exact `#[test]` count)`. Contract §2.1: the pure cores move
/// **with their tests**, byte-for-byte. A move that drops a test is the
/// failure mode this catches; the counts here are what P1-P4 shipped on the
/// current tree (`36862f6`), not the pre-extraction counts the brief was
/// written against -- three of the seven suites grew tests during their own
/// part's work (`workspaces.rs` 6 -> 18, `keybindings.rs` 6 -> 32,
/// `appearance.rs` 2 -> 15) and `displays/state.rs` grew by one (12 -> 13)
/// when it was split further into `canvas.rs`/`controls.rs`.
const MOVED_SUITES: &[(&str, usize)] = &[
    ("src/pages/displays_canvas.rs", 8),
    ("src/pages/displays/state.rs", 13),
    ("src/pages/workspaces.rs", 18),
    ("src/pages/keybindings.rs", 32),
    ("src/pages/appearance.rs", 15),
    ("src/model.rs", 10),
    ("src/compositor_reload.rs", 1),
];

/// Every settings test the contract names as an M5 gate, by function name.
/// Contract §2.1 (P1), §2.6 (P2), §2.7 (P3), §2.8 (P4).
///
/// Six names below are the light-theme variant of a gate the contract named
/// singular; P2/P3/P4 each split their "paints at rest" gate into
/// light/dark/high-contrast tests. Checking the light variant is sufficient
/// proof the gate exists; each rename was confirmed deliberate with
/// `git log -S<name>`.
const REQUIRED_GATES: &[&str] = &[
    // P1 -- the first `App::run` on a Role::Toplevel window (the named risk).
    "the_first_toplevel_app_run_paints_and_navigates",
    // P2 -- Appearance and Behavior.
    "hex_rgba_round_trips",
    "invalid_hex_falls_back_to_black_without_panicking",
    "packed_round_trips_through_the_color_dialog_packing",
    "wallpaper_validation_rejects_a_missing_file",
    "wallpaper_validation_rejects_an_unknown_extension",
    "a_failed_picker_disables_browse_without_touching_the_model",
    "portal_uri_decoding_handles_percent_escapes",
    // renamed by P2, commit 408d807 ("rest-state gates for Appearance and
    // Behavior, light/dark/hc"): one gate became three, per theme.
    "appearance_page_paints_every_probe_point_at_rest_in_the_light_theme",
    "behavior_page_paints_every_probe_point_at_rest_in_the_light_theme",
    // P3 -- Workspaces and Keybindings.
    "shift_a_while_capturing_records_base_a_with_shift",
    "a_lone_modifier_press_leaves_the_capture_armed",
    "escape_cancels_a_capture",
    "switching_pages_cancels_a_capture",
    // renamed by P3, commit c6edb97 ("the Workspaces page's config mutators,
    // GTK-free").
    "remove_workspace_prunes_the_bindings_it_orphans",
    // renamed by P3, commit 1cfc9dc ("rest-state screencopy gates for both
    // P3 pages"): one gate became three, per theme.
    "workspaces_page_paints_every_probe_point_at_rest_in_the_light_theme",
    "keybindings_page_paints_every_probe_point_at_rest_in_the_light_theme",
    // P4 -- Displays.
    "a_drag_that_hits_nothing_leaves_the_selection_alone",
    "a_drag_snaps_the_dragged_head_against_its_neighbour",
    "a_release_outside_the_canvas_ends_the_drag",
    "enabling_a_mode_less_head_gives_it_a_default_mode",
    "an_apply_reply_clears_in_flight_by_its_own_is_test_tag",
    // renamed by P4, commit 95dc8f9 ("the Displays rest-state gate,
    // light/dark/hc"): one gate became three, per theme.
    "displays_page_paints_every_probe_point_at_rest_in_the_light_theme",
    "dragging_a_head_snaps_it_and_updates_the_model",
    "a_colour_pick_changes_the_swatch",
    "apply_reaches_reload_config_on_the_mock",
    "a_drop_down_list_fits_inside_the_settings_window",
];

const SELF_EXCLUDED: &[&str] = &["dependency_audit.rs", "deletion_ledger.rs"];

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn workspace_dir() -> PathBuf {
    crate_dir()
        .parent()
        .expect("the crate directory has a parent")
        .to_path_buf()
}

fn rust_sources(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs")
                && !SELF_EXCLUDED.iter().any(|name| path.ends_with(name))
            {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// `#[test]` attributes in a file. A `#[test]` inside a string literal would
/// be counted too; no file in this crate has one, and a spurious count is a
/// loud failure rather than a silent pass.
fn count_tests(path: &Path) -> usize {
    std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
        .matches("#[test]")
        .count()
}

/// Every settings source and test, concatenated, for whole-crate pattern
/// searches.
fn all_settings_text() -> String {
    let mut paths = rust_sources(&crate_dir().join("src"));
    paths.extend(rust_sources(&crate_dir().join("tests")));
    assert!(
        paths.len() > 5,
        "the source walker found only {} files -- it is looking in the wrong place",
        paths.len()
    );
    paths
        .iter()
        .map(|p| std::fs::read_to_string(p).unwrap_or_else(|e| panic!("read {}: {e}", p.display())))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Mutation check: `git checkout develop -- settings/tests/appearance_gtk.rs`;
/// this must fail naming that path. Delete it again.
#[test]
fn every_deleted_settings_file_is_gone_and_its_replacement_exists() {
    let root = workspace_dir();
    let mut problems = Vec::new();
    for (deleted, replacement, proof) in DELETED_WITH_REPLACEMENT {
        if root.join(deleted).exists() {
            problems.push(format!(
                "{deleted} still exists (was to be replaced by {replacement})"
            ));
        }
        if !root.join(replacement).exists() {
            problems.push(format!(
                "{deleted} was deleted but its replacement {replacement} does not exist \
                 (proof was to be: {proof})"
            ));
        }
    }
    assert!(
        problems.is_empty(),
        "the settings deletion ledger does not hold: {problems:#?}"
    );
}

/// Mutation check: add `pub fn build(ctx: u8) {}` to
/// `settings/src/pages/behavior.rs`; this must fail naming `pub fn build(`.
/// Restore.
#[test]
fn every_deleted_settings_identifier_is_gone() {
    let text = all_settings_text();
    let survivors: Vec<String> = DELETED_IDENTIFIERS
        .iter()
        .filter(|(pattern, _)| text.contains(pattern))
        .map(|(pattern, home)| format!("`{pattern}` ({home})"))
        .collect();
    assert!(
        survivors.is_empty(),
        "GTK-era settings items survived the migration: {survivors:#?}"
    );
}

/// Mutation check: delete one `#[test]` function from
/// `settings/src/pages/displays/state.rs`; this must fail reporting 12 for 13.
/// Restore.
#[test]
fn every_moved_settings_suite_kept_its_test_count() {
    let mut problems = Vec::new();
    for (relative, expected) in MOVED_SUITES {
        let path = crate_dir().join(relative);
        if !path.exists() {
            problems.push(format!("{relative} does not exist"));
            continue;
        }
        let actual = count_tests(&path);
        if actual != *expected {
            problems.push(format!("{relative}: {actual} tests, expected {expected}"));
        }
    }
    assert!(
        problems.is_empty(),
        "a pure core moved without all of its tests: {problems:#?}"
    );
}

/// Mutation check: rename `escape_cancels_a_capture` in
/// `settings/tests/keybindings.rs`; this must fail naming it. Restore.
#[test]
fn every_settings_gate_the_contract_names_exists() {
    let text = all_settings_text();
    let missing: Vec<&str> = REQUIRED_GATES
        .iter()
        .copied()
        .filter(|name| !text.contains(&format!("fn {name}(")))
        .collect();
    assert!(
        missing.is_empty(),
        "the contract names these settings gates and they do not exist: {missing:#?}"
    );
}
