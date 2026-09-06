# Pure-Rust GTK-themed UI — M5 Part 3: settings Workspaces + Keybindings pages, key capture — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Port `icedtea-settings`' Workspaces and Keybindings pages off GTK4 onto `icedtea-ui`'s Elm loop — an editable workspace list, a scrollable action/binding list, and layout-agnostic key capture driven by `KeyEvent::base` — with the twelve inherited pure-core tests still green and the GTK capture test re-expressed as a `VirtualKeyboardClient`-driven harness proof.

**Architecture:** Both pages become pure `view(&SettingsModel) -> View<Msg>` functions plus small pure mutation helpers in their own page modules; `app::update`'s seven Workspaces/Keybindings arms are one-line delegations to those helpers. Key capture is `SettingsModel::capturing: Option<String>` plus one window-root `on_key` closure that is *armed from the model each frame*: `pages::keybindings::capture_key(armed, ev)` normalises `ev.base`/`ev.keysym` exactly the way `compositor/src/input.rs` matches, and returns `Msg::KeyCaptured`/`Msg::CaptureCancelled`/`None`. Correctness is proven three ways: unit tests on the pure helpers, a harness test that injects a real `Shift`+`a` through `zwp_virtual_keyboard_v1` and reads the stored combo back out of the app's probe report, and rest-state screencopy gates over both pages in light, dark and high-contrast.

**Tech Stack:** Rust edition 2024 (rust-version 1.94), `icedtea-ui` (M3 toolkit + M5 P0 additions), `icedtea-config`, `xkbcommon` 0.9, `wayland-client` 0.31, `wayland-protocols(-wlr)` 0.3, `icedtea-harness` (headless `wlr` 0.20.29 compositor, `VirtualPointerClient`, `VirtualKeyboardClient`, `ScreencopyClient`), `tempfile` 3.

**Spec:** `docs/superpowers/specs/2026-09-03-pure-rust-gtk-m5-app-migrations-design.md` (§5.4 "P3 Workspaces / Keybindings", §7 testing gates) together with the frozen contract `docs/superpowers/plans/2026-09-03-m5-part0-contract.md` (§2.1 extraction, §2.2 `SettingsModel`/`Msg`, §2.3 `view` decomposition and normative ids, §2.7 P3, §2.9 the settings sheet, §5's P3 boundary). Parent program spec: `docs/superpowers/specs/2026-08-20-pure-rust-gtk-ui-design.md`. Predecessor contract, still binding: `docs/superpowers/plans/2026-08-27-m3-part0-contract.md` (§3 public interfaces, §10 deviations P3-A…P8-D75, §11 execution notes E1–E21).

---

## Contract deviations

Every item below deviates from `2026-09-03-m5-part0-contract.md`. Each is carried out by P3 and must be appended to that file's §6 in the shape *Carried out by / Added / Contract says / As shipped / Ruling* when it lands (P6 collects them; P3 writes them).

- **P3-D1 — `capture_key` takes an `armed: bool`.** Contract §2.7 declares `pub fn capture_key(ev: &KeyEvent) -> Option<Msg>` and §2.3 wires it as `.on_key(|ev| pages::keybindings::capture_key(ev))`, with rule 4 saying `update` simply ignores `Msg::KeyCaptured` when `capturing.is_none()`. That cannot ship: `GenericC::on_event`'s `Event::Key` arm sets `cx.handled = true` for *any* message a `KeyPressed` handler returns (`ui/src/view/controller.rs:508-513`), and M3 P4-D19 makes a handled event end the whole dispatch — so a root handler that answers every key press swallows every keystroke in the window. As shipped: `pub fn capture_key(armed: bool, ev: &KeyEvent) -> Option<crate::app::Msg>`, returning `None` whenever `!armed`, wired as `.on_key({ let armed = m.capturing.is_some(); move |ev| pages::keybindings::capture_key(armed, ev) })`. Ruling: the arming gate belongs in the handler, where `handled` is decided, not only in `update`.
- **P3-D2 — P3 edits `settings/src/app.rs`.** Contract §5's P3 *Owns* list names only the two page modules, `style.css`'s `.conflict` rule and `tests/keybindings.rs`, yet §2.2 puts the whole `update` match and §2.3 the whole root `view` in `app.rs`. P3's edits there are bounded to: the seven Workspaces/Keybindings `Msg` arms, the `Msg::PageSelected` arm's `capturing` reset (§2.7 rule 4), the root `.on_key` wiring (P3-D1), and swapping P1's two page stubs for the real `pages::{workspaces,keybindings}::view`. No other line of `app.rs` is touched.
- **P3-D3 — the app probe report gains a `binding` line.** Contract §2.7 says the re-expressed capture test "asserts through the app's probe report that the stored combo is `{ modifiers: ["SHIFT"], key: "KEY_a" }`", but M5-D9 defines only `probe <label> <x> <y>` and `alloc <id> <x> <y> <w> <h>` lines, neither of which carries a combo. As shipped: when `$ICEDTEA_PROBE_REPORT` is set, `pages::keybindings::apply_capture` appends one line `binding <action> <modifiers joined by '+', or '-'> <key>` (e.g. `binding close SHIFT KEY_a`) per stored binding. Ruling: a test-only report line in a P3-owned module beats either a coordinate-only assertion or a second D-Bus round trip; the raw `KeyCombo` field spellings are written, not `format_combo`'s display form, so the assertion pins the serialization the compositor matches on.
- **P3-D4 — the Workspaces page gates live in `settings/tests/keybindings.rs`.** Contract §5 gives P3 exactly one new test file; §2.7 names `workspaces_page_paints_every_probe_point_at_rest` without naming a file. Both pages' harness tests and gates therefore share that one integration binary. Ruling: one file keeps the harness scaffolding (compositor, theme, spawned binary, report parsing) single-sourced without creating a `settings/tests/support/` module that P2 may also be creating in parallel.
- **P3-D5 — six pure helpers not named in the contract.** P3 adds `pages::workspaces::{can_remove, add_workspace, remove_workspace, rename_workspace}` and `pages::keybindings::{apply_capture, report_binding}`. Contract §2.7 states their rules as prose inside `update`. Ruling: `App::run_offscreen` returns `Frames` and no model, so a rule that lives only inside an `update` arm has no unit test; moving each rule into a `pub fn` in a P3-owned page module makes every one of them directly testable and leaves `update` as one-line delegations. Contract §Rules already allows a part to "add private items freely"; these are recorded because they are `pub`.
- **P3-D6 — `$ICEDTEA_UI_THEME` selects `icedtea-settings`' base theme.** No contract section says how the settings binary picks a theme, yet §2.7 and §7 require rest-state gates in light, dark and high contrast. As shipped: `icedtea-settings` reads `$ICEDTEA_UI_THEME` as a path to a **complete** base theme used whole — `ui/src/bin/window-probe.rs:41`'s exact convention — with `settings/style.css` still layered over it as the app's own origin (§2.9). The gates write `icedtea_ui::BUNDLED_ADWAITA_{LIGHT,DARK,HC}` to a temp file and point the variable at it. **P2 lands this first** (its own deviation P2-D7, Task 9), in this exact spelling; Task 8 verifies it and changes nothing. Only if neither P1 nor P2 shipped it does P3 add it. (Consistency-check ruling E3.)
- **P3-D7 — two ids beyond §2.3's normative list.** §2.3 lists `workspaces.{list, add}` + `ws_name_<i>`/`ws_remove_<i>` and `keybindings.{list}` + `kb_row_<action>`/`kb_set_<action>`. P3 also sets `ws_row_<i>` (the `list_box_row`, so the rest-state gate can address a row as a unit) and `kb_combo_<action>` (the combo label, so a future conflict-styling gate can address it). Both are additive; no listed id changes.

---

## Global Constraints

Copied from the spec and contract. Every task's requirements implicitly include this section.

- **No GTK anywhere in a migrated app.** `settings/` must not reference `gtk4`, `gtk4-layer-shell`, `glib`, `gio`, `gobject`, `gdk`, `pango` or `cairo` — not in `Cargo.toml`, not in a `use`, not in a doc comment's code block. `grep -rn 'gtk4\|glib::\|gio::\|gdk::\|pango::\|cairo::' settings/src settings/tests` must be empty for every file P3 touches.
- **Pinned versions:** `wayland-client` 0.31, `wayland-protocols`/`wayland-protocols-wlr` 0.3, `zbus` at the workspace version, `rustix` 1, `skia-rs-safe` 0.4.0, `taffy` 0.14, `xkbcommon` 0.9, `wlr` 0.20.29. Edition 2024, `rust-version = "1.94"`. P3 adds no dependency to any `Cargo.toml`.
- **Gates, per crate, all green before every commit:**
  `cargo test -p icedtea-settings`; `cargo clippy -p icedtea-settings --all-targets -- -D warnings`; `cargo fmt --all --check`.
  Before the final commit also: `cargo clippy -p icedtea-ui --all-targets -- -D warnings` and `cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings`.
- **All M1/M2/M3 gates stay green, unchanged:** `themed_button_offscreen`, `adwaita_coverage`, `gtk4_property_reference`, `layer_shell_screencopy`, `transition_screencopy`, `window_events`, `counter_app`, `node_trees`, `widget_pixels`, `reconcile_props`, `gallery_gate` (light/dark/hc), `interaction_gate` (16/16). P3 changes no file under `ui/` — the one enumerated `ui/` exception in M5 belongs to P4 (`drop_down.rs`, contract §7).
- **Parts execute in order P0 → P1 → P2 → P3 → P4 → P5 → P6.** P3's base is P2's head, so P3 may consume everything P0, P1 and P2 produced. It may not consume anything from P4, P5 or P6.
- **Settings swaps GTK out in place (spec D3).** By the time P3 runs, `settings/` already has no GTK; P3 must not reintroduce a coexistence path, a feature flag or a second binary.
- **`settings/tests/live_apply.rs` keeps spawning the real `icedtea-compositor` binary** (inherited decision 11). P3 does not touch it, does not move it onto `icedtea_harness::Compositor`, and does not read from it.
- **Untrusted input never panics.** Keymaps, key events, config files and portal replies are all untrusted: a malformed value is dropped, logged once, and replaced by the initial/fallback.
- **Every load-bearing test records its mutation check** — break the code the test claims to cover, confirm the test fails, restore — written into the test's doc comment.
- **M6 scope is out of bounds even opportunistically:** IME/`text-input-v3`, drag-and-drop, `accesskit`, emoji, portals beyond `FileChooser`, `StackSidebar`'s eviction defect, the M2-inherited background-layer `currentColor` gap, the compositor's `sh -c` spawn action.
- **Commit trailer**, on every commit in this part:
  ```
  Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
  ```

---

## File Structure

| File | Disposition | Responsibility after P3 |
|---|---|---|
| `settings/src/pages/workspaces.rs` | Modify | The whole Workspaces page: the four pure config mutators (`can_remove`, `rename_workspace`, `add_workspace`, `remove_workspace`) over P1's extracted `next_workspace_name`/`prune_orphaned_workspace_bindings`, and `pub fn view(&SettingsModel) -> View<Msg>`. The GTK `build`, `Row`, `RebuildSlot` and `set_remove_sensitivity` are gone. The six inherited unit tests stay byte-identical. |
| `settings/src/pages/keybindings.rs` | Modify | The whole Keybindings page: `capture_key`, `apply_capture`, `report_binding`/`write_binding_line`, and `pub fn view(&SettingsModel) -> View<Msg>`, over P1's extracted `FIXED_ACTIONS`, `action_list`, `format_combo`, `normalise_keysym`, `should_capture`, `take_capture_reset`. The GTK `build`, `Row`, `sync_rows`, `reset_capture`, `unshifted_keysym` and `install_conflict_css` are gone. The six inherited unit tests stay byte-identical. |
| `settings/src/app.rs` | Modify (bounded, P3-D2) | Seven `update` arms, `Msg::PageSelected`'s capture reset, the root `.on_key` arming, and the two page-`view` call sites. |
| `settings/style.css` | Modify | Gains the one `.conflict` rule (contract §2.7 rule 6, §2.9). |
| `settings/tests/keybindings.rs` | Create | Every P3 harness test and gate: the `VirtualKeyboardClient` capture proof, the three capture-lifecycle tests, and both pages' rest-state screencopy gates in light/dark/hc. Self-contained scaffolding (P3-D4). |
| `settings/tests/keybindings_gtk.rs` | Delete | Re-expressed by `settings/tests/keybindings.rs` (contract §4.2). |

Nothing else in the workspace is created, modified or deleted by P3.

**Normative widget ids for these two pages** (contract §2.3 plus P3-D7). Gates address widgets by these and never by hard-coded coordinates:

```
workspaces_list                    the ListBox
ws_row_<i>                         one list_box_row per workspace, i = 0-based index
ws_name_<i>                        that row's Entry
ws_remove_<i>                      that row's Remove Button
workspaces_add                     the Add Button

keybindings_list                   the ScrolledWindow
kb_row_<action>                    one row Box per action, e.g. kb_row_close, kb_row_workspace:1
kb_combo_<action>                  that row's combo Label
kb_set_<action>                    that row's Set Button
```

Action strings contain `:` (`spawn:terminal`, `workspace:1`). That is deliberate: ids are used only as probe-report labels and `Window::allocation` keys, never as CSS selectors, so no selector parser ever sees them.

---

## Interfaces this part consumes

Produced by P0, P1 and P2; used verbatim. Listed here because a task's implementer sees only their own task.

```rust
// --- P0, ui/ ------------------------------------------------------------
// icedtea_ui::window::keyboard
pub struct KeyEvent {
    pub keycode: u32,
    pub keysym: xkbcommon::xkb::Keysym,
    pub base: xkbcommon::xkb::Keysym,   // M5-D7: group 0, level 0
    pub utf8: Option<String>,
    pub mods: Mods,
    pub consumed: Mods,
    pub pressed: bool,
    pub repeat: bool,
    pub serial: u32,
    pub time_ms: u32,
}
bitflags! { pub struct Mods: u8 { const SHIFT; const CTRL; const ALT; const LOGO; const CAPS; const NUM; } }
impl Keymap { pub fn base_keysym(&self, keycode: u32) -> xkbcommon::xkb::Keysym; }

// icedtea_ui::window
pub struct ProbePoint { pub label: String, pub x: i32, pub y: i32 }
impl Window {
    pub fn probe_points(&self) -> Vec<ProbePoint>;
    pub fn allocation(&self, id: &str) -> Option<icedtea_ui::layout::Allocation>;
}

// icedtea_ui::view
impl<M: 'static, Msg: Clone + 'static> App<M, Msg> {
    pub fn new(model: M,
               update: impl FnMut(&mut M, Msg) -> Cmd<Msg> + 'static,
               view: impl Fn(&M) -> View<Msg> + 'static) -> Self;   // M5-D4
}

// --- P1, settings/ ------------------------------------------------------
// settings/src/app.rs
pub struct SettingsModel {
    pub model: crate::model::Model,          // .working / .saved / .is_dirty() / .revert()
    pub db_path: std::path::PathBuf,
    pub page: crate::pages::PageId,
    pub status: String,
    pub capturing: Option<String>,
    pub conflicts: Vec<(icedtea_config::KeyCombo, Vec<String>)>,
    pub wallpaper_text: String,
    pub wallpaper_error: Option<String>,
    pub portal_available: bool,
    pub displays: crate::pages::displays::state::DisplaysState,
    pub displays_status: String,
    pub displays_in_flight: bool,
    pub outputs_available: bool,
    pub workers: crate::ipc::WorkerHandles,
}
#[derive(Clone, Debug)]
pub enum Msg { /* … */
    PageSelected(usize),
    WorkspaceRenamed(usize, String), WorkspaceRemoved(usize), WorkspaceAdded,
    CaptureArmed(String), CaptureCancelled,
    KeyCaptured { keysym: u32, mods: crate::model::CaptureMods },
}
pub fn view(m: &SettingsModel) -> View<Msg>;
// update: impl FnMut(&mut SettingsModel, Msg) -> Cmd<Msg>, one match, one arm group per page.

// settings/src/pages/mod.rs
pub enum PageId { Appearance, Behavior, Workspaces, Keybindings, Displays }
impl PageId { pub const ALL: [PageId; 5]; pub fn name(self) -> &'static str;
              pub fn title(self) -> &'static str; pub fn from_index(i: usize) -> PageId; }

// settings/src/pages/workspaces.rs  (extracted by P1, six tests moved with them)
pub fn next_workspace_name(existing: &[String]) -> String;
pub fn prune_orphaned_workspace_bindings(cfg: &mut icedtea_config::Config);

// settings/src/pages/keybindings.rs (extracted by P1, six tests moved with them)
pub const FIXED_ACTIONS: [&str; 10];
pub fn action_list(workspace_count: usize) -> Vec<String>;
pub fn format_combo(combo: &icedtea_config::KeyCombo) -> String;
pub fn should_capture(page_visible: bool, capturing: bool) -> bool;
pub fn take_capture_reset(capturing: &mut Option<String>) -> Option<String>;
pub fn normalise_keysym(base: u32, modified: u32) -> u32;

// settings/src/model.rs  (untouched since before M5)
pub struct CaptureMods { pub ctrl: bool, pub alt: bool, pub shift: bool, pub logo: bool }
pub fn combo_from_keysym(keysym: u32, modifiers: CaptureMods) -> Option<icedtea_config::KeyCombo>;
pub fn duplicate_bindings(cfg: &icedtea_config::Config) -> Vec<(icedtea_config::KeyCombo, Vec<String>)>;

// the probe report  (written by icedtea-ui's `App::run` — P0-D4/P1-D4, not by
// settings/src/main.rs; the settings binary only has to be run with the
// variable set)
// When $ICEDTEA_PROBE_REPORT is set, appends, once per frame in which the tree changed:
//   probe <label> <x> <y>      one per laid-out node, root-first (the first line of a
//                              batch is always `probe root <x> <y>`)
//   alloc <id> <x> <y> <w> <h> one per node carrying View::id
// Coordinates are window-surface coordinates.
// $ICEDTEA_UI_THEME names a complete base theme file (P3-D6); settings/style.css is
// layered over it and both are handed to Window::open's `sheet` argument.
```

---

## Task 1: Workspaces — the four pure config mutators

**Files:**
- Modify: `settings/src/pages/workspaces.rs`
- Test: `settings/src/pages/workspaces.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `pages::workspaces::next_workspace_name`, `pages::workspaces::prune_orphaned_workspace_bindings` (P1); `icedtea_config::Config`.
- Produces:
  ```rust
  pub fn can_remove(cfg: &icedtea_config::Config) -> bool;
  pub fn rename_workspace(cfg: &mut icedtea_config::Config, index: usize, name: &str);
  pub fn add_workspace(cfg: &mut icedtea_config::Config);
  pub fn remove_workspace(cfg: &mut icedtea_config::Config, index: usize);
  ```

- [ ] **Step 1: Write the failing tests**

Append to the existing `#[cfg(test)] mod tests` in `settings/src/pages/workspaces.rs`, immediately after `prune_orphaned_workspace_bindings_is_a_no_op_when_nothing_is_orphaned`. Do **not** touch the six tests already there.

```rust
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
        assert!(!can_remove(&cfg), "the last workspace must not be removable");
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
        assert_eq!(cfg.workspace_names, vec!["a".to_string(), "beta".to_string()]);
        assert_eq!(cfg.keybindings, bindings_before, "a rename touches no binding");
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
            icedtea_config::KeyCombo { modifiers: vec![], key: "KEY_3".to_string() },
        );
        cfg.keybindings.insert(
            "close".to_string(),
            icedtea_config::KeyCombo { modifiers: vec![], key: "KEY_q".to_string() },
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
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p icedtea-settings --lib pages::workspaces`
Expected: FAIL — `cannot find function 'can_remove' in this scope`, and the same for `rename_workspace`, `add_workspace`, `remove_workspace`.

- [ ] **Step 3: Write the implementation and delete the GTK page**

Replace `settings/src/pages/workspaces.rs`'s module header and everything from `use std::cell::RefCell;` down to the end of `pub fn build(...)` with the following. The `#[cfg(test)] mod tests` block at the bottom of the file — the six inherited tests plus Step 1's six — stays exactly where it is.

```rust
//! The Workspaces page: an editable list of workspace names, each row an
//! `Entry` (the name) + a Remove `Button`, plus an Add `Button` that appends
//! a new row.
//!
//! `workspace_names` must never go empty -- `icedtea_config::load_or_default`
//! rejects an empty list on load, so an empty working copy would silently
//! revert to defaults on next start. Remove is therefore insensitive
//! whenever exactly one row remains ([`can_remove`]).
//!
//! The Keybindings page generates a `workspace:N`/`move_to_workspace:N` row
//! pair per workspace, so adding/removing a row here changes the *set* of
//! actions that page shows. On the Elm loop there is nothing to notify: the
//! Keybindings page reads `working.workspace_names.len()` on every `view`,
//! so it cannot go stale, and the GTK `RebuildSlot`/`on_change`
//! forward-reference is gone.

use icedtea_config::Config;
use icedtea_ui::layout::Align;
use icedtea_ui::view::View;
use icedtea_ui::view::builders::{BoxExt, box_, list_box, list_box_row};
use icedtea_ui::widgets::Orientation;
use icedtea_ui::widgets::button::button;
use icedtea_ui::widgets::entry::entry;

use crate::app::{Msg, SettingsModel};

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
```

Keep P1's `next_workspace_name` and `prune_orphaned_workspace_bindings` (with their doc comments) exactly where they are in the file, below the block above. `Align` is imported for Task 2 and is unused until then, so add `#[allow(unused_imports)]`? No — instead, **omit the `Align`, `View`, `builders`, `Orientation`, `button`, `entry`, `Msg`, `SettingsModel` imports in this step** and add them in Task 2 together with the code that uses them. This step's imports are exactly:

```rust
use icedtea_config::Config;
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p icedtea-settings --lib pages::workspaces`
Expected: PASS, 12 tests (the six inherited plus the six new).

Run: `cargo clippy -p icedtea-settings --all-targets -- -D warnings`
Expected: PASS. (`settings/src/app.rs` still calls P1's stub for this page, so nothing else breaks yet.)

- [ ] **Step 5: Commit**

```bash
git add settings/src/pages/workspaces.rs
git commit -m "$(cat <<'EOF'
refactor(settings): the Workspaces page's config mutators, GTK-free

can_remove/rename_workspace/add_workspace/remove_workspace carry every rule
the GTK Remove/Add handlers held: the last row is never removable, an
out-of-range index is dropped rather than panicking, Add reuses the smallest
free name, and Remove prunes the workspace:N bindings it orphans. The GTK
build()/Row/RebuildSlot/set_remove_sensitivity go with them.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Workspaces — `view`

**Files:**
- Modify: `settings/src/pages/workspaces.rs`
- Test: `settings/src/pages/workspaces.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: Task 1's `can_remove`; `crate::app::{Msg, SettingsModel}` (P1); `icedtea_ui::view::builders::{box_, list_box, list_box_row, BoxExt}`, `icedtea_ui::widgets::{Orientation, button::button, entry::entry}`, `icedtea_ui::layout::Align`, `icedtea_ui::view::View`.
- Produces: `pub fn view(m: &crate::app::SettingsModel) -> icedtea_ui::view::View<crate::app::Msg>` — the Workspaces page body, rooted at a vertical `box_`, carrying the ids listed in File Structure.

The page's shape, from contract §2.7:

```
box(vertical, spacing 8, margin 16)
├── listbox                          #workspaces_list, vexpand
│   ├── listboxrow                   #ws_row_0, key 0
│   │   └── box(horizontal, spacing 8)
│   │       ├── entry                #ws_name_0, hexpand, on_change -> WorkspaceRenamed(0, _)
│   │       └── button "Remove"      #ws_remove_0, sensitive(can_remove), on_click -> WorkspaceRemoved(0)
│   ┊
└── button "Add workspace"           #workspaces_add, halign Start, on_click -> WorkspaceAdded
```

Every row carries `View::key(i)`: rows are positional and renameable, so the model's own ordering is the identity (contract §2.3, "without keys a rename would rebuild the `Entry` and lose the caret").

- [ ] **Step 1: Write the failing tests**

Append to `settings/src/pages/workspaces.rs`'s `#[cfg(test)] mod tests`:

```rust
    use icedtea_ui::view::{EventKind, Kind, PropName};

    /// A `SettingsModel` whose working config has `names` as its workspaces
    /// and nothing else disturbed. Built through the same `SettingsModel::
    /// new` the binary uses, so the view under test sees a real model.
    fn model_with(names: &[&str]) -> crate::app::SettingsModel {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("config.redb");
        let (inbox, tx) = icedtea_ui::view::Inbox::new().expect("inbox");
        // The inbox is dropped at the end of the test; the workers notice and
        // return. Keeping it alive here is what stops `spawn` from racing a
        // closed channel while the test runs.
        let workers = crate::ipc::spawn(tx);
        let mut m = crate::app::SettingsModel::new(db_path, workers, None);
        m.model.working.workspace_names = names.iter().map(|s| (*s).to_string()).collect();
        drop(inbox);
        m
    }

    /// The id/key/handler contract the gates and the reconciler depend on:
    /// one keyed row per workspace, each with its own ids, and the row's
    /// Entry emitting `WorkspaceRenamed` with *its own* index.
    ///
    /// Mutation check: drop `.key(i)` from the row; `keys_are_the_row_index`
    /// below fails. Drop the `move |s|` capture of `i` and hard-code `0`;
    /// this test fails on the second row. Restore both.
    #[test]
    fn each_workspace_row_carries_its_own_ids_and_index() {
        let m = model_with(&["alpha", "beta"]);
        let page = view(&m);
        let list = &page.children[0];
        assert_eq!(list.kind, Kind::ListBox);
        assert_eq!(list.props.str(PropName::Id), Some("workspaces_list"));
        assert_eq!(list.children.len(), 2, "one row per workspace name");

        for (i, expected) in ["alpha", "beta"].iter().enumerate() {
            let row = &list.children[i];
            assert_eq!(row.kind, Kind::ListBoxRow);
            assert_eq!(row.props.str(PropName::Id), Some(format!("ws_row_{i}").as_str()));
            let row_box = &row.children[0];
            let name = &row_box.children[0];
            assert_eq!(name.kind, Kind::Entry);
            assert_eq!(name.props.str(PropName::Id), Some(format!("ws_name_{i}").as_str()));
            assert_eq!(name.props.str(PropName::Text), Some(*expected));

            let renamed = name
                .handlers
                .fire_text(EventKind::Change, "typed")
                .expect("the name Entry emits a Change message");
            assert!(
                matches!(&renamed, Msg::WorkspaceRenamed(index, text) if *index == i && text == "typed"),
                "row {i} emitted {renamed:?}"
            );

            let remove = &row_box.children[1];
            assert_eq!(remove.props.str(PropName::Id), Some(format!("ws_remove_{i}").as_str()));
            let removed = remove
                .handlers
                .fire_unit(EventKind::Click)
                .expect("Remove emits a Click message");
            assert!(
                matches!(&removed, Msg::WorkspaceRemoved(index) if *index == i),
                "row {i} Remove emitted {removed:?}"
            );
        }
    }

    /// Keyed by row index, so a rename diffs the Entry's text in place
    /// instead of rebuilding the widget and dropping the caret.
    ///
    /// Mutation check: remove `.key(i)`; this fails with `None`. Restore.
    #[test]
    fn workspace_rows_are_keyed_by_index() {
        let m = model_with(&["alpha", "beta", "gamma"]);
        let page = view(&m);
        let keys: Vec<_> = page.children[0].children.iter().map(|row| row.key.clone()).collect();
        assert_eq!(
            keys,
            vec![
                Some(icedtea_ui::view::Key::Index(0)),
                Some(icedtea_ui::view::Key::Index(1)),
                Some(icedtea_ui::view::Key::Index(2)),
            ]
        );
    }

    /// Remove is insensitive at the last row -- the computed form of the GTK
    /// `set_remove_sensitivity` call.
    ///
    /// Mutation check: pass `true` instead of `can_remove(..)`; this fails.
    /// Restore.
    #[test]
    fn the_last_workspaces_remove_button_is_insensitive() {
        let one = model_with(&["only"]);
        let page = view(&one);
        let remove = &page.children[0].children[0].children[0].children[1];
        assert_eq!(remove.props.get(PropName::Sensitive), Some(&icedtea_ui::view::Prop::Bool(false)));

        let two = model_with(&["one", "two"]);
        let page = view(&two);
        for row in &page.children[0].children {
            let remove = &row.children[0].children[1];
            assert_eq!(remove.props.get(PropName::Sensitive), Some(&icedtea_ui::view::Prop::Bool(true)));
        }
    }

    /// Add is the page's second child and emits `WorkspaceAdded`.
    ///
    /// Mutation check: change the message to `Msg::WorkspaceRemoved(0)`;
    /// this fails. Restore.
    #[test]
    fn the_add_button_emits_workspace_added() {
        let m = model_with(&["alpha"]);
        let page = view(&m);
        let add = &page.children[1];
        assert_eq!(add.props.str(PropName::Id), Some("workspaces_add"));
        assert!(matches!(
            add.handlers.fire_unit(EventKind::Click),
            Some(Msg::WorkspaceAdded)
        ));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p icedtea-settings --lib pages::workspaces`
Expected: FAIL — `cannot find function 'view' in this scope`.

- [ ] **Step 3: Write the implementation**

Add the imports and `view` to `settings/src/pages/workspaces.rs`, above the `#[cfg(test)]` block:

```rust
use icedtea_ui::layout::Align;
use icedtea_ui::view::View;
use icedtea_ui::view::builders::{BoxExt, box_, list_box, list_box_row};
use icedtea_ui::widgets::Orientation;
use icedtea_ui::widgets::button::button;
use icedtea_ui::widgets::entry::entry;

use crate::app::{Msg, SettingsModel};

/// The Workspaces page body.
///
/// Pure: it reads `m.model.working.workspace_names` and nothing else, and it
/// never writes. Every row is keyed by its index so a rename diffs the
/// `Entry`'s text rather than rebuilding the widget and losing the caret
/// (contract §2.3).
#[must_use]
pub fn view(m: &SettingsModel) -> View<Msg> {
    let removable = can_remove(&m.model.working);
    let rows: Vec<View<Msg>> = m
        .model
        .working
        .workspace_names
        .iter()
        .enumerate()
        .map(|(i, name)| {
            list_box_row(
                box_(
                    Orientation::Horizontal,
                    [
                        entry(name)
                            .id(&format!("ws_name_{i}"))
                            .hexpand(true)
                            .on_change(move |text| Msg::WorkspaceRenamed(i, text.to_string())),
                        button("Remove")
                            .id(&format!("ws_remove_{i}"))
                            .sensitive(removable)
                            .on_click(Msg::WorkspaceRemoved(i)),
                    ],
                )
                .spacing(8),
            )
            .key(i)
            .id(&format!("ws_row_{i}"))
        })
        .collect();

    box_(
        Orientation::Vertical,
        [
            list_box(rows).id("workspaces_list").vexpand(true),
            button("Add workspace")
                .id("workspaces_add")
                .halign(Align::Start)
                .on_click(Msg::WorkspaceAdded),
        ],
    )
    .spacing(8)
    .margin(16, 16, 16, 16)
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p icedtea-settings --lib pages::workspaces`
Expected: PASS, 16 tests.

Run: `cargo clippy -p icedtea-settings --all-targets -- -D warnings`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add settings/src/pages/workspaces.rs
git commit -m "$(cat <<'EOF'
feat(settings): the Workspaces page view

A keyed listbox of Entry + Remove rows over an Add button, with the ids the
M5 gates address (workspaces_list, ws_row_<i>, ws_name_<i>, ws_remove_<i>,
workspaces_add). Rows are keyed by index so a rename diffs the Entry in
place instead of rebuilding it and dropping the caret.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: `app::update` — the three Workspaces arms and the page wiring

**Files:**
- Modify: `settings/src/app.rs` (bounded per P3-D2)
- Test: `settings/src/pages/workspaces.rs` (`#[cfg(test)] mod tests`) — the arms are one-line delegations, so their rules are already covered by Task 1; this task's test proves the *delegation*.

**Interfaces:**
- Consumes: Task 1's `can_remove`/`rename_workspace`/`add_workspace`/`remove_workspace`, Task 2's `view`, P1's `update`/`view` skeleton and `Msg`.
- Produces: `app::update` handles `Msg::WorkspaceRenamed`, `Msg::WorkspaceRemoved`, `Msg::WorkspaceAdded`; `app::view`'s `stack_page("workspaces", …)` renders `pages::workspaces::view(m)`.

- [ ] **Step 1: Write the failing test**

Append to `settings/src/pages/workspaces.rs`'s `#[cfg(test)] mod tests`:

```rust
    /// The three Workspaces `update` arms are delegations, and this pins
    /// that they delegate: folding the three messages through the real
    /// `app::update` must produce exactly what calling the mutators does.
    ///
    /// Mutation check: make the `Msg::WorkspaceRemoved` arm call
    /// `cfg.workspace_names.remove(i)` directly (no prune); the
    /// `workspace:3` assertion fails. Restore.
    #[test]
    fn the_update_arms_delegate_to_the_workspace_mutators() {
        let mut m = model_with(&["a", "b", "c"]);
        m.model.working.keybindings.insert(
            "workspace:3".to_string(),
            icedtea_config::KeyCombo { modifiers: vec![], key: "KEY_3".to_string() },
        );

        let _ = crate::app::update(&mut m, Msg::WorkspaceRenamed(1, "beta".to_string()));
        assert_eq!(m.model.working.workspace_names[1], "beta");

        let _ = crate::app::update(&mut m, Msg::WorkspaceAdded);
        assert_eq!(
            m.model.working.workspace_names,
            vec!["a".to_string(), "beta".to_string(), "c".to_string(), "1".to_string()]
        );

        let _ = crate::app::update(&mut m, Msg::WorkspaceRemoved(3));
        let _ = crate::app::update(&mut m, Msg::WorkspaceRemoved(2));
        assert_eq!(m.model.working.workspace_names, vec!["a".to_string(), "beta".to_string()]);
        assert!(
            !m.model.working.keybindings.contains_key("workspace:3"),
            "removing down to two workspaces prunes workspace:3"
        );
    }
```

`crate::app::update` is P1's free function; if P1 shipped it as a closure factory instead, call it through whatever P1's `pub fn update(&mut SettingsModel, Msg) -> Cmd<Msg>` entry point is — contract §2.2 names it `update` and requires it be callable as `impl FnMut(&mut SettingsModel, Msg) -> Cmd<Msg>`.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p icedtea-settings --lib pages::workspaces::tests::the_update_arms_delegate_to_the_workspace_mutators`
Expected: FAIL — P1's stub arms leave `workspace_names` untouched, so `assert_eq!(m.model.working.workspace_names[1], "beta")` fails with `"b"`.

- [ ] **Step 3: Write the implementation**

In `settings/src/app.rs`, replace the three stub arms in `update`'s match with:

```rust
        // --- Workspaces (P3) --------------------------------------------
        Msg::WorkspaceRenamed(index, name) => {
            crate::pages::workspaces::rename_workspace(&mut m.model.working, index, &name);
            Cmd::None
        }
        Msg::WorkspaceAdded => {
            crate::pages::workspaces::add_workspace(&mut m.model.working);
            Cmd::None
        }
        Msg::WorkspaceRemoved(index) => {
            crate::pages::workspaces::remove_workspace(&mut m.model.working, index);
            Cmd::None
        }
```

and in `view`, replace P1's Workspaces stub with the real page:

```rust
        stack_page("workspaces", "Workspaces", pages::workspaces::view(m)),
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p icedtea-settings --lib pages::workspaces`
Expected: PASS, 17 tests.

Run: `cargo clippy -p icedtea-settings --all-targets -- -D warnings && cargo fmt --all --check`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add settings/src/app.rs settings/src/pages/workspaces.rs
git commit -m "$(cat <<'EOF'
feat(settings): wire the Workspaces page into update and view

Three one-line update arms delegating to the page's own mutators, and the
stack page rendering the real view instead of P1's stub. Removing a
workspace prunes the workspace:N bindings it orphans, proven through the
real update fold.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Keybindings — `capture_key`

**Files:**
- Modify: `settings/src/pages/keybindings.rs`
- Test: `settings/src/pages/keybindings.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: P1's `normalise_keysym(base: u32, modified: u32) -> u32`; `icedtea_ui::window::keyboard::{KeyEvent, Mods}`; `crate::model::CaptureMods`; `crate::app::Msg`.
- Produces:
  ```rust
  pub fn capture_key(armed: bool, ev: &icedtea_ui::window::keyboard::KeyEvent)
      -> Option<crate::app::Msg>;
  ```

Rules, from contract §2.7 as amended by P3-D1:

1. `None` unless `armed && ev.pressed && !ev.repeat`.
2. Resolve the sym first: `normalise_keysym(ev.base.raw(), ev.keysym.raw())` — the raw/latin keysym, falling back to the modified one only when the keycode produces no base sym at all. This is `compositor/src/input.rs:109-123`'s rule verbatim, and it is required because `icedtea_config::keys::key_name_to_keysym` always encodes the unshifted keysym.
3. That sym being `XK_Escape` → `Msg::CaptureCancelled`.
4. Otherwise `Msg::KeyCaptured { keysym: sym, mods }`, with `mods` read off `ev.mods` (the effective state) and never off `ev.consumed`.

- [ ] **Step 1: Write the failing tests**

Append to `settings/src/pages/keybindings.rs`'s `#[cfg(test)] mod tests`:

```rust
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
                    crate::model::CaptureMods { shift: true, ..Default::default() }
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
        assert!(matches!(capture_key(true, &ev), Some(Msg::CaptureCancelled)));
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
        assert_eq!(capture_key(false, &ev), None);
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
        assert_eq!(capture_key(true, &release), None);

        let mut repeat = press(30, keysyms::KEY_a, keysyms::KEY_a, Mods::empty());
        repeat.repeat = true;
        assert_eq!(capture_key(true, &repeat), None);
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
                    crate::model::CaptureMods { shift: true, logo: true, ..Default::default() }
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
```

`Msg` must be comparable for the `assert_eq!(capture_key(..), None)` calls: `Option<Msg>` compares by `PartialEq`, which contract §2.2's `#[derive(Clone, Debug)]` does not provide. Use `assert!(capture_key(false, &ev).is_none())` instead of `assert_eq!` in the three places above that compare against `None`, and keep the `match` form everywhere else. Apply that substitution while writing the tests.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p icedtea-settings --lib pages::keybindings`
Expected: FAIL — `cannot find function 'capture_key' in this scope`.

- [ ] **Step 3: Write the implementation**

Add to `settings/src/pages/keybindings.rs`, keeping P1's extracted functions and their doc comments where they are:

```rust
use icedtea_ui::window::keyboard::{KeyEvent, Mods};

use crate::app::Msg;
use crate::model::CaptureMods;

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
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p icedtea-settings --lib pages::keybindings`
Expected: PASS, 13 tests (the six inherited plus Step 1's seven).

- [ ] **Step 5: Commit**

```bash
git add settings/src/pages/keybindings.rs
git commit -m "$(cat <<'EOF'
feat(settings): capture_key, the window-root binding capture handler

Normalises through KeyEvent::base exactly the way compositor/src/input.rs
matches, so a Shift-held capture stores the unshifted key name the config
format encodes. Takes an `armed` flag (deviation P3-D1): a KeyPressed
handler that answers every press sets cx.handled, and P4-D19 makes that end
the whole dispatch, which would swallow every keystroke in the window.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Keybindings — `apply_capture` and the `binding` report line

**Files:**
- Modify: `settings/src/pages/keybindings.rs`
- Test: `settings/src/pages/keybindings.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `crate::model::{CaptureMods, combo_from_keysym}`; `icedtea_config::{Config, KeyCombo}`.
- Produces:
  ```rust
  pub fn apply_capture(cfg: &mut icedtea_config::Config, action: &str,
                       keysym: u32, mods: crate::model::CaptureMods) -> bool;
  pub fn report_binding(action: &str, combo: &icedtea_config::KeyCombo);
  fn write_binding_line(path: &std::path::Path, action: &str,
                        combo: &icedtea_config::KeyCombo) -> std::io::Result<()>;
  ```

`apply_capture` returns `true` when a combo was stored (the caller clears `capturing`) and `false` when `combo_from_keysym` declined — a lone modifier press — in which case the capture **stays armed** and waits for the real key, which is the GTK behaviour verbatim (contract §2.7 rule 5).

`report_binding` is deviation P3-D3: when `$ICEDTEA_PROBE_REPORT` is set it appends one line

```
binding <action> <modifiers joined by '+', or '-' when there are none> <key>
```

e.g. `binding close SHIFT KEY_a`, `binding fullscreen SUPER+SHIFT KEY_q`, `binding reload - KEY_F5`. The raw `KeyCombo` field spellings are written, not `format_combo`'s display form, so the harness assertion pins the serialization the compositor matches on.

- [ ] **Step 1: Write the failing tests**

Append to `settings/src/pages/keybindings.rs`'s `#[cfg(test)] mod tests`:

```rust
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
            CaptureMods { shift: true, ..Default::default() },
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
            CaptureMods { logo: true, ..Default::default() },
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
        assert!(apply_capture(&mut cfg, "close", xkbcommon::xkb::keysyms::KEY_a, CaptureMods::default()));
        assert!(apply_capture(&mut cfg, "quit", xkbcommon::xkb::keysyms::KEY_a, CaptureMods::default()));
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
            &icedtea_config::KeyCombo { modifiers: vec![], key: "KEY_F5".to_string() },
        )
        .expect("write");

        let text = std::fs::read_to_string(&path).expect("read back");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines, vec!["binding close SUPER+SHIFT KEY_q", "binding reload - KEY_F5"]);
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
            &icedtea_config::KeyCombo { modifiers: vec![], key: "KEY_a".to_string() },
        )
        .expect("write");
        let text = std::fs::read_to_string(&path).expect("read back");
        assert_eq!(text, "probe root 10 20\nbinding close - KEY_a\n");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p icedtea-settings --lib pages::keybindings`
Expected: FAIL — `cannot find function 'apply_capture' in this scope` and `cannot find function 'write_binding_line' in this scope`.

- [ ] **Step 3: Write the implementation**

Add to `settings/src/pages/keybindings.rs`:

```rust
use std::path::Path;

use icedtea_config::{Config, KeyCombo};

use crate::model::combo_from_keysym;

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
pub fn apply_capture(cfg: &mut Config, action: &str, keysym: u32, mods: CaptureMods) -> bool {
    let Some(combo) = combo_from_keysym(keysym, mods) else {
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
    if let Err(err) = write_binding_line(Path::new(&path), action, combo) {
        tracing::warn!(%err, "cannot append a binding line to $ICEDTEA_PROBE_REPORT");
    }
}

/// `binding <action> <modifiers|-> <key>`, appended.
///
/// Appends rather than truncates: the same file carries the app's own
/// `probe`/`alloc` batches. The raw `KeyCombo` field spellings are written,
/// not [`format_combo`]'s display form, so a reader pins the exact
/// serialization the compositor's matcher compares against.
fn write_binding_line(path: &Path, action: &str, combo: &KeyCombo) -> std::io::Result<()> {
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
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p icedtea-settings --lib pages::keybindings`
Expected: PASS, 18 tests.

Run: `cargo clippy -p icedtea-settings --all-targets -- -D warnings`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add settings/src/pages/keybindings.rs
git commit -m "$(cat <<'EOF'
feat(settings): apply_capture, and a binding line in the probe report

apply_capture stores what combo_from_keysym builds and reports whether the
capture finished: a lone modifier press leaves it armed and writes nothing,
the GTK behaviour verbatim. Conflicts are stored, never refused.

The binding report line is deviation P3-D3: M5-D9's probe/alloc lines cannot
express a KeyCombo, so the harness capture test has nothing to read back
without it. Raw KeyCombo spellings, appended, never truncating.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: Keybindings — `view`, and the `.conflict` rule in the settings sheet

**Files:**
- Modify: `settings/src/pages/keybindings.rs`
- Modify: `settings/style.css`
- Test: `settings/src/pages/keybindings.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: P1's `action_list`, `format_combo`; `SettingsModel.{model, capturing, conflicts}`; `icedtea_ui::view::builders::{box_, scrolled_window, BoxExt}`, `icedtea_ui::widgets::{Orientation, button::button, label::{label, LabelExt}}`.
- Produces: `pub fn view(m: &crate::app::SettingsModel) -> icedtea_ui::view::View<crate::app::Msg>`.

The page's shape, from contract §2.7:

```
scrolledwindow                       #keybindings_list, vexpand, hexpand
└── box(vertical, spacing 4, margin 16)
    ├── box(horizontal, spacing 12)  #kb_row_close, key "close"
    │   ├── label "close"                       width_chars 24, xalign 0
    │   ├── label "SUPER+q" [.conflict]         #kb_combo_close, width_chars 20, xalign 0, hexpand
    │   └── button "Set" | "Press a key…"       #kb_set_close, on_click -> CaptureArmed("close")
    ┊
```

Rows are keyed by the action string. The combo label reads `format_combo` when the action is bound and `"(unbound)"` when it is not — `sync_rows`' exact two branches. It carries the `conflict` class when `m.conflicts` names the action, replacing the GTK `install_conflict_css` provider with a sheet rule.

- [ ] **Step 1: Write the failing tests**

Append to `settings/src/pages/keybindings.rs`'s `#[cfg(test)] mod tests`:

```rust
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
        let (inbox, tx) = icedtea_ui::view::Inbox::new().expect("inbox");
        let workers = crate::ipc::spawn(tx);
        let mut m = crate::app::SettingsModel::new(db_path, workers, None);
        m.model.working.workspace_names = (1..=count).map(|n| n.to_string()).collect();
        drop(inbox);
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
            assert_eq!(row.props.str(PropName::Id), Some(format!("kb_row_{action}").as_str()));
            assert_eq!(row.key, Some(icedtea_ui::view::Key::Name(action.as_str().into())));
            assert_eq!(row.children[0].props.str(PropName::Label), Some(action.as_str()));
            assert_eq!(
                row.children[1].props.str(PropName::Id),
                Some(format!("kb_combo_{action}").as_str())
            );

            let set = &row.children[2];
            assert_eq!(set.props.str(PropName::Id), Some(format!("kb_set_{action}").as_str()));
            let armed = set.handlers.fire_unit(EventKind::Click).expect("Set emits a Click");
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
        let combo = icedtea_config::KeyCombo { modifiers: vec![], key: "KEY_a".to_string() };
        m.model.working.keybindings.insert("close".to_string(), combo.clone());
        m.model.working.keybindings.insert("quit".to_string(), combo.clone());
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

        let plain_color = cascade(&sheet, &plain, &mut cx).winner(CssProp::Color).cloned();
        let flagged_color = cascade(&sheet, &flagged, &mut cx).winner(CssProp::Color).cloned();
        assert!(flagged_color.is_some(), "the .conflict rule declares a colour");
        assert_ne!(
            plain_color, flagged_color,
            "a conflicting label must not resolve to the ordinary label colour"
        );
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p icedtea-settings --lib pages::keybindings`
Expected: FAIL — `cannot find function 'view' in this scope`, and `the_settings_sheet_colours_a_conflicting_binding` fails on `flagged_color.is_some()` because `settings/style.css` has no `.conflict` rule yet.

- [ ] **Step 3: Write the implementation**

Append to `settings/style.css` (keeping whatever P1 put there):

```css
/* A keybinding bound to more than one action. Advisory only: conflicts
   never block Apply (M5 spec §5.4). Replaces the GTK
   `install_conflict_css` provider, which loaded the same declaration at
   STYLE_PROVIDER_PRIORITY_APPLICATION. */
.conflict {
    color: #f38ba8;
    font-weight: bold;
}
```

Add to `settings/src/pages/keybindings.rs`:

```rust
use icedtea_ui::view::View;
use icedtea_ui::view::builders::{BoxExt, box_, scrolled_window};
use icedtea_ui::widgets::Orientation;
use icedtea_ui::widgets::button::button;
use icedtea_ui::widgets::label::{LabelExt, label};

use crate::app::SettingsModel;

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
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p icedtea-settings --lib pages::keybindings`
Expected: PASS, 23 tests.

Run: `cargo clippy -p icedtea-settings --all-targets -- -D warnings && cargo fmt --all --check`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add settings/src/pages/keybindings.rs settings/style.css
git commit -m "$(cat <<'EOF'
feat(settings): the Keybindings page view and the .conflict sheet rule

A scrolled list of action/combo/Set rows keyed by action, so growing the
workspace count inserts rows without re-arming the others. The combo label
shows format_combo or (unbound) and carries the `conflict` class when
duplicate_bindings names it -- the sheet rule that replaces the GTK
install_conflict_css provider.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: `app::update` — the three Keybindings arms, the capture reset, the armed root handler

**Files:**
- Modify: `settings/src/app.rs` (bounded per P3-D2)
- Test: `settings/src/pages/keybindings.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: Task 4's `capture_key`, Task 5's `apply_capture`, Task 6's `view`; `crate::model::duplicate_bindings`; P1's `Msg::PageSelected` arm and root `view`.
- Produces: `app::update` handles `Msg::CaptureArmed`, `Msg::CaptureCancelled`, `Msg::KeyCaptured`, and clears `m.capturing` on `Msg::PageSelected`; `app::view`'s root carries the armed `on_key` and renders `pages::keybindings::view(m)`.

- [ ] **Step 1: Write the failing tests**

Append to `settings/src/pages/keybindings.rs`'s `#[cfg(test)] mod tests`:

```rust
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
                mods: CaptureMods { shift: true, ..Default::default() },
            },
        );
        assert!(m.capturing.is_none(), "a stored capture disarms the row");
        let combo = m.model.working.keybindings.get("close").expect("close is bound");
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
                mods: CaptureMods { logo: true, ..Default::default() },
            },
        );
        assert_eq!(m.capturing.as_deref(), Some("close"), "still waiting for the real key");
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
            icedtea_config::KeyCombo { modifiers: vec![], key: "KEY_a".to_string() },
        );
        let _ = crate::app::update(&mut m, Msg::CaptureArmed("close".to_string()));
        let _ = crate::app::update(
            &mut m,
            Msg::KeyCaptured { keysym: xkbcommon::xkb::keysyms::KEY_a, mods: CaptureMods::default() },
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
        let ev = press(30, xkbcommon::xkb::keysyms::KEY_a, xkbcommon::xkb::keysyms::KEY_a, Mods::empty());

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
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p icedtea-settings --lib pages::keybindings`
Expected: FAIL — `a_capture_arms_stores_and_disarms` fails at `assert_eq!(m.capturing.as_deref(), Some("close"))` (P1's stub arms do nothing), and `the_root_key_handler_is_armed_from_the_model` fails at the armed assertion with `None`.

- [ ] **Step 3: Write the implementation**

In `settings/src/app.rs`, replace the three stub arms in `update`'s match with:

```rust
        // --- Keybindings (P3) -------------------------------------------
        Msg::CaptureArmed(action) => {
            // Starting a new capture supersedes any row already pending --
            // the view renders "Press a key…" from `capturing` alone, so the
            // superseded row's button goes back to "Set" on the same frame.
            m.capturing = Some(action);
            Cmd::None
        }
        Msg::CaptureCancelled => {
            m.capturing = None;
            Cmd::None
        }
        Msg::KeyCaptured { keysym, mods } => {
            // Nothing armed: inert. `capture_key` already declines an
            // unarmed press (deviation P3-D1); this is the second gate, for
            // a message that reached the queue some other way.
            let Some(action) = m.capturing.clone() else {
                return Cmd::None;
            };
            // `false` means `combo_from_keysym` declined -- a lone modifier
            // press. Stay armed and wait for the real key (contract §2.7
            // rule 5), the GTK behaviour verbatim.
            if crate::pages::keybindings::apply_capture(
                &mut m.model.working,
                &action,
                keysym,
                mods,
            ) {
                m.capturing = None;
                m.conflicts = crate::model::duplicate_bindings(&m.model.working);
            }
            Cmd::None
        }
```

If `update`'s body is a `match` expression rather than a series of `return`s, replace the `let Some(action) = … else { return Cmd::None; };` with:

```rust
            let Some(action) = m.capturing.clone() else {
                return Cmd::None;
            };
```
only when `update` is a function that may `return`; otherwise write the arm as

```rust
        Msg::KeyCaptured { keysym, mods } => {
            if let Some(action) = m.capturing.clone()
                && crate::pages::keybindings::apply_capture(
                    &mut m.model.working, &action, keysym, mods,
                )
            {
                m.capturing = None;
                m.conflicts = crate::model::duplicate_bindings(&m.model.working);
            }
            Cmd::None
        }
```

Use whichever form matches P1's `update`; the second form is expression-shaped and always compiles.

In the same file, add the capture reset to the navigation arm — P1's arm keeps its own body, this adds one line:

```rust
        Msg::PageSelected(index) => {
            m.page = crate::pages::PageId::from_index(index);
            // Leaving the Keybindings page disarms any pending capture: the
            // GTK `connect_unmap`/`EventControllerFocus` reset, now one
            // line. Without it an armed capture survives the switch and the
            // window-root handler rebinds on the next keystroke typed
            // anywhere in the app.
            m.capturing = None;
            Cmd::None
        }
```

And in `view`, swap the Keybindings stub for the real page and arm the root handler:

```rust
        stack_page("keybindings", "Keybindings", pages::keybindings::view(m)),
```

```rust
    .id("root")
    .on_key({
        // Armed from the model each frame (deviation P3-D1): an unarmed
        // handler must decline, or `GenericC::on_event` sets `cx.handled`
        // for every key press and M3's P4-D19 whole-dispatch stop swallows
        // the keystroke a focused `Entry` was waiting for.
        let armed = m.capturing.is_some();
        move |ev| pages::keybindings::capture_key(armed, ev)
    })
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p icedtea-settings --lib pages::keybindings`
Expected: PASS, 30 tests.

Run: `cargo test -p icedtea-settings --lib`
Expected: PASS — including `model.rs`'s 10, `displays_canvas.rs`'s 8, `displays/state.rs`'s 12, `compositor_reload.rs`'s 1, and both pages' inherited 6 + 6.

Run: `cargo clippy -p icedtea-settings --all-targets -- -D warnings && cargo fmt --all --check`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add settings/src/app.rs settings/src/pages/keybindings.rs
git commit -m "$(cat <<'EOF'
feat(settings): wire binding capture into update and the root key handler

Arm, capture, cancel and page-switch-disarm, with the conflicts list
recomputed after every stored binding. The root on_key closure is armed from
m.capturing each frame (deviation P3-D1) so an idle window declines
keystrokes instead of swallowing them through P4-D19's whole-dispatch stop.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: The harness scaffolding and the Shift+`a` capture proof

**Files:**
- Create: `settings/tests/keybindings.rs`
- Test: `settings/tests/keybindings.rs`

**Interfaces:**
- Consumes: `icedtea_harness::{Compositor, ScreencopyClient, VirtualKeyboardClient, VirtualPointerClient}`; `icedtea_ui::BUNDLED_ADWAITA_{LIGHT,DARK,HC}`; `icedtea_ui::wayland::BTN_LEFT`; P1's `$ICEDTEA_PROBE_REPORT` writer and `$ICEDTEA_UI_THEME` reading (P3-D6); Task 5's `binding` line (P3-D3); Task 6's ids.
- Produces (private to this file, consumed by Tasks 9 and 10):
  ```rust
  const TITLE_BAR_HEIGHT: i32;
  const KEY_LEFTSHIFT: u32; const KEY_A: u32; const KEY_B: u32; const KEY_ESC: u32;
  struct Reaper(std::process::Child);
  struct Settings { compositor: Compositor, _reaper: Reaper, report: tempfile::NamedTempFile,
                    _theme: tempfile::NamedTempFile, _config: tempfile::TempDir,
                    origin: (i32, i32), output: (u32, u32),
                    pointer: VirtualPointerClient, keyboard: VirtualKeyboardClient }
  impl Settings {
      fn spawn(theme_css: &str) -> Settings;
      fn lines(&self) -> Vec<String>;
      fn batch(&self) -> Vec<String>;
      fn wait_for_line(&self, prefix: &str) -> Option<String>;
      fn wait_for_alloc(&self, id: &str) -> (i32, i32, i32, i32);
      fn alloc(&self, id: &str) -> Option<(i32, i32, i32, i32)>;
      fn probes(&self) -> Vec<(String, i32, i32)>;
      fn to_screen(&self, x: i32, y: i32) -> (i32, i32);
      fn click_id(&mut self, id: &str);
      fn select_page(&mut self, index: usize);
      fn click(&mut self, x: i32, y: i32);
      fn capture(&mut self) -> icedtea_harness::CapturedFrame;
  }
  ```

**How the page is reached.** No settings CLI flag and no new env var is invented: the gate clicks the `StackSwitcher`, which is what a user does and what proves navigation works. `StackSwitcherC::rebuild` (`ui/src/widgets/stack_switcher.rs`) appends one `button.text-button` node per page in `PageId::ALL` order, each with a `label` child. Those buttons are controller-owned subnodes with real allocations, so `Window::probe_points` emits one `button<N>` point per page inside `#nav`'s allocation. `select_page(i)` reads `alloc nav …`, takes the probe points whose label starts with `button` and whose centre lies inside that rectangle, sorts them by `x`, asserts there are exactly five, and clicks the `i`-th. Nothing is hard-coded.

**How focus reaches the client.** The virtual keyboard is spawned **before** the settings binary, so the seat already advertises its keyboard capability when the client binds `wl_keyboard`; the compositor's `pointer_button` press path focuses the window under the pointer (`compositor/src/state.rs:7150-7158`), and `select_page` clicks the window before any key is sent. Inside the window, `GenericC::on_event`'s `Event::PointerDown` arm sets the focus on the clicked node's nearest generic ancestor (`ui/src/view/controller.rs:473`) — `ButtonC` does not take focus itself — so clicking a Set button focuses its row box, and the next key press bubbles from there up to the root's `on_key`.

- [ ] **Step 1: Write the failing test**

Create `settings/tests/keybindings.rs`:

```rust
//! P3's harness tests and rest-state gates for the Workspaces and
//! Keybindings pages.
//!
//! One integration binary for both pages (deviation P3-D4): contract §5
//! gives P3 exactly one new test file, and sharing it keeps the scaffolding
//! -- compositor, theme, spawned binary, probe-report parsing -- single
//! sourced without creating a `settings/tests/support/` module P2 may also
//! be creating.
//!
//! Everything here drives the **real** `icedtea-settings` binary under
//! `icedtea_harness::Compositor`. `settings/tests/live_apply.rs` is the one
//! test that must spawn the real `icedtea-compositor` instead (inherited
//! decision 11); nothing in this file touches it.

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use icedtea_harness::{
    CapturedFrame, Compositor, ScreencopyClient, VirtualKeyboardClient, VirtualPointerClient,
};

/// The compositor's server-side title bar (`compositor/src/decoration.rs`),
/// so window-surface coordinates can be mapped onto the output.
const TITLE_BAR_HEIGHT: i32 = 28;

/// Linux evdev keycodes.
const KEY_ESC: u32 = 1;
const KEY_LEFTSHIFT: u32 = 42;
const KEY_A: u32 = 30;
const KEY_B: u32 = 48;

/// How long a report line may take to appear. Generous: a debug-build
/// settings binary boots a redb store, a second Wayland connection and two
/// worker threads before its first frame. A complexity bound, not a timing
/// pin.
const REPORT_TIMEOUT: Duration = Duration::from_secs(20);
const POLL: Duration = Duration::from_millis(40);

/// `PageId::ALL`'s indices, as `select_page` takes them.
const PAGE_WORKSPACES: usize = 2;
const PAGE_KEYBINDINGS: usize = 3;

/// Kills the settings process when dropped, including on a test panic.
struct Reaper(Child);

impl Drop for Reaper {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// One compositor, one settings process, one pointer and one keyboard.
struct Settings {
    compositor: Compositor,
    screencopy: ScreencopyClient,
    _reaper: Reaper,
    report: tempfile::NamedTempFile,
    _theme: tempfile::NamedTempFile,
    _config: tempfile::TempDir,
    /// The window-surface origin on the output.
    origin: (i32, i32),
    output: (u32, u32),
    pointer: VirtualPointerClient,
    keyboard: VirtualKeyboardClient,
}

impl Settings {
    /// Boot everything and wait for the first painted frame.
    ///
    /// The keyboard is spawned **before** the client: the headless seat
    /// advertises its keyboard capability only once a device exists on it,
    /// so a client that binds `wl_seat` first never binds `wl_keyboard` and
    /// can never receive `enter` (`ui/tests/window_events.rs`'s
    /// `a_virtual_keyboard_types_into_the_entry` documents the same
    /// ordering).
    fn spawn(theme_css: &str) -> Settings {
        let compositor = Compositor::spawn();
        let socket = compositor.socket_path().to_string_lossy().to_string();
        let (out_w, out_h) = compositor.output_size();
        assert!(
            out_w >= 640 && out_h >= 520,
            "a 480x420 settings window plus its title bar needs at least a \
             640x520 output, got {out_w}x{out_h}"
        );

        let keyboard = VirtualKeyboardClient::spawn(&socket);
        let pointer = VirtualPointerClient::spawn(&socket);

        let mut theme = tempfile::NamedTempFile::new().expect("theme file");
        std::io::Write::write_all(&mut theme, theme_css.as_bytes()).expect("write the theme");
        std::io::Write::flush(&mut theme).expect("flush the theme");

        let config = tempfile::tempdir().expect("config dir");
        let report = tempfile::NamedTempFile::new().expect("report file");

        let reaper = Reaper(
            Command::new(env!("CARGO_BIN_EXE_icedtea-settings"))
                .env("WAYLAND_DISPLAY", &socket)
                .env("XDG_CONFIG_HOME", config.path())
                .env("ICEDTEA_UI_THEME", theme.path())
                .env("ICEDTEA_PROBE_REPORT", report.path())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("failed to spawn icedtea-settings"),
        );

        let mut settings = Settings {
            compositor,
            screencopy: ScreencopyClient::spawn(&socket),
            _reaper: reaper,
            report,
            _theme: theme,
            _config: config,
            origin: (0, 0),
            output: (out_w as u32, out_h as u32),
            pointer,
            keyboard,
        };

        settings
            .wait_for_line("probe root ")
            .expect("icedtea-settings never wrote a probe batch");
        let window = settings
            .compositor
            .snapshot()
            .windows
            .into_iter()
            .find(|w| w.app_id == "org.icedtea.Settings")
            .expect("the settings window is in the compositor's model");
        settings.origin = (window.geometry.x, window.geometry.y + TITLE_BAR_HEIGHT);
        settings
    }

    /// Every line written so far.
    fn lines(&self) -> Vec<String> {
        std::fs::read_to_string(self.report.path())
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// The most recent frame's batch: everything from the last
    /// `probe root …` line onwards. Every batch begins with that line
    /// because `Window::probe_points` walks the tree root-first (M5-D9).
    fn batch(&self) -> Vec<String> {
        let lines = self.lines();
        let start = lines
            .iter()
            .rposition(|l| l.starts_with("probe root "))
            .unwrap_or(0);
        lines[start..].to_vec()
    }

    /// Poll until a line starting with `prefix` appears anywhere in the
    /// report.
    fn wait_for_line(&self, prefix: &str) -> Option<String> {
        let started = Instant::now();
        while started.elapsed() < REPORT_TIMEOUT {
            if let Some(line) = self.lines().into_iter().find(|l| l.starts_with(prefix)) {
                return Some(line);
            }
            std::thread::sleep(POLL);
        }
        None
    }

    /// `alloc <id> x y w h` from the latest batch, if present.
    fn alloc(&self, id: &str) -> Option<(i32, i32, i32, i32)> {
        let prefix = format!("alloc {id} ");
        let line = self.batch().into_iter().find(|l| l.starts_with(&prefix))?;
        let f: Vec<i32> = line
            .split_whitespace()
            .skip(2)
            .filter_map(|v| v.parse().ok())
            .collect();
        (f.len() == 4).then(|| (f[0], f[1], f[2], f[3]))
    }

    /// Poll until `id` is in the latest batch, then return its rectangle.
    fn wait_for_alloc(&self, id: &str) -> (i32, i32, i32, i32) {
        let started = Instant::now();
        while started.elapsed() < REPORT_TIMEOUT {
            if let Some(rect) = self.alloc(id) {
                return rect;
            }
            std::thread::sleep(POLL);
        }
        panic!(
            "#{id} never appeared in a probe batch; last batch was:\n{}",
            self.batch().join("\n")
        );
    }

    /// `(label, x, y)` for every probe point in the latest batch.
    fn probes(&self) -> Vec<(String, i32, i32)> {
        self.batch()
            .into_iter()
            .filter_map(|line| {
                let mut f = line.split_whitespace();
                (f.next()? == "probe").then_some(())?;
                let label = f.next()?.to_string();
                let x = f.next()?.parse().ok()?;
                let y = f.next()?.parse().ok()?;
                Some((label, x, y))
            })
            .collect()
    }

    /// Window-surface coordinates -> output coordinates.
    fn to_screen(&self, x: i32, y: i32) -> (i32, i32) {
        (self.origin.0 + x, self.origin.1 + y)
    }

    /// Move, press and release the left button at an output coordinate.
    ///
    /// The eight settling round trips are `ui/tests/support/mod.rs`'s
    /// `Driver::move_to` rule: a button sent back to back with the motion
    /// that first put the pointer on a surface can reach the seat before
    /// focus is assigned, and `wlr_seat_pointer_notify_button` drops a
    /// button with no focused surface silently.
    fn click(&mut self, x: i32, y: i32) {
        self.pointer.motion_absolute(
            f64::from(x),
            f64::from(y),
            self.output.0,
            self.output.1,
        );
        self.pointer.frame();
        self.pointer.pump();
        for _ in 0..8 {
            std::thread::sleep(Duration::from_millis(25));
            self.pointer.pump();
        }
        self.pointer.button(icedtea_ui::wayland::BTN_LEFT, true);
        self.pointer.frame();
        self.pointer.pump();
        std::thread::sleep(Duration::from_millis(25));
        self.pointer.button(icedtea_ui::wayland::BTN_LEFT, false);
        self.pointer.frame();
        self.pointer.pump();
    }

    /// Click the centre of the widget with this id.
    fn click_id(&mut self, id: &str) {
        let (x, y, w, h) = self.wait_for_alloc(id);
        let (sx, sy) = self.to_screen(x + w / 2, y + h / 2);
        self.click(sx, sy);
    }

    /// Click the `index`-th `StackSwitcher` button, in `PageId::ALL` order.
    ///
    /// Derived, never hard-coded: the switcher's five `button.text-button`
    /// subnodes are the probe points labelled `button<N>` whose centres fall
    /// inside `#nav`'s allocation, and their `label` children are labelled
    /// `label<N>` so they never collide with the filter.
    fn select_page(&mut self, index: usize) {
        let (nx, ny, nw, nh) = self.wait_for_alloc("nav");
        let mut buttons: Vec<(i32, i32)> = self
            .probes()
            .into_iter()
            .filter(|(label, x, y)| {
                label.starts_with("button")
                    && *x >= nx
                    && *x < nx + nw
                    && *y >= ny
                    && *y < ny + nh
            })
            .map(|(_, x, y)| (x, y))
            .collect();
        buttons.sort_unstable();
        assert_eq!(
            buttons.len(),
            5,
            "the switcher must offer one button per PageId; found {buttons:?} in \
             nav ({nx},{ny},{nw},{nh})"
        );
        let (bx, by) = buttons[index];
        let (sx, sy) = self.to_screen(bx, by);
        self.click(sx, sy);
    }

    /// One screencopy frame.
    fn capture(&mut self) -> CapturedFrame {
        self.screencopy.capture()
    }
}

/// The GTK test this replaces (`settings/tests/keybindings_gtk.rs`) proved
/// the SHIP-BLOCKER shift-normalisation fix through GDK's own keymap. This
/// proves the same property through the real toolkit, the real compositor
/// and a real `zwp_virtual_keyboard_v1` press: capturing `Shift`+`a` must
/// store the *unshifted* `KEY_a`, because `icedtea_config::keys::
/// key_name_to_keysym` only ever encodes the unshifted keysym and a stored
/// `KEY_A` is a binding the compositor's `match_action` can never fire.
///
/// Mutation check: in `pages::keybindings::capture_key`, pass
/// `ev.keysym.raw()` where it passes `ev.base.raw()`; this test fails with
/// `binding close SHIFT KEY_A`. Restore. (Equivalently: make
/// `normalise_keysym` return `modified` unconditionally -- contract §5's
/// named P3 mutation check -- and this test fails the same way.)
#[test]
fn shift_a_while_capturing_records_base_a_with_shift() {
    let mut s = Settings::spawn(icedtea_ui::BUNDLED_ADWAITA_LIGHT);
    s.select_page(PAGE_KEYBINDINGS);
    s.wait_for_alloc("keybindings_list");

    // Arm the `close` row. Clicking the Set button also focuses its row box
    // (`GenericC::on_event`'s PointerDown arm), which is what puts a focus
    // owner in the tree so the next key press is dispatched at all.
    s.click_id("kb_set_close");

    s.keyboard.key_down(KEY_LEFTSHIFT);
    s.keyboard.key_press(KEY_A);
    s.keyboard.key_up(KEY_LEFTSHIFT);
    s.keyboard.pump();

    let line = s
        .wait_for_line("binding close ")
        .unwrap_or_else(|| panic!("no capture was recorded; report:\n{}", s.lines().join("\n")));
    assert_eq!(
        line, "binding close SHIFT KEY_a",
        "a Shift-held capture must store the unshifted key name"
    );
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p icedtea-settings --test keybindings shift_a_while_capturing -- --nocapture`
Expected: FAIL — the old `settings/tests/keybindings_gtk.rs` still exists and does not build against the GTK-free crate, so the run fails at compile time with `unresolved import 'gtk4'` in that file. Delete-order note: `keybindings_gtk.rs` is removed in Task 11, so to see *this* test's own red first, run it with the GTK file temporarily renamed:

```bash
mv settings/tests/keybindings_gtk.rs /tmp/keybindings_gtk.rs.bak
cargo test -p icedtea-settings --test keybindings shift_a_while_capturing -- --nocapture
mv /tmp/keybindings_gtk.rs.bak settings/tests/keybindings_gtk.rs
```

With it out of the way the expected failure is the real one: `no capture was recorded; report: …` if the wiring is wrong, or a passing run if Tasks 4–7 are correct. **This test is expected to pass immediately** — it is the integration proof of code that already exists, not a driver for new code. Confirm that it is non-vacuous by applying its own mutation check now: change `ev.base.raw()` to `ev.keysym.raw()` in `capture_key`, re-run, see `binding close SHIFT KEY_A`, restore.

- [ ] **Step 3: Confirm the scaffolding assumptions**

Three P1-produced behaviours this file depends on. Verify each against the code P1 shipped, and fix the *test* (never the app) if a spelling differs:

1. `ICEDTEA_PROBE_REPORT` produces `probe <label> <x> <y>` and `alloc <id> <x> <y> <w> <h>` lines, each batch beginning with `probe root `. Check with:
   ```bash
   grep -n 'probe \|alloc ' settings/src/main.rs settings/src/app.rs
   ```
2. `ICEDTEA_UI_THEME` names a complete base theme file (deviation P3-D6). Check with:
   ```bash
   grep -n 'ICEDTEA_UI_THEME' settings/src/main.rs
   ```
   If P1 did not read it, add the reading to `settings/src/main.rs` in exactly `ui/src/bin/window-probe.rs:41`'s shape — `std::env::var("ICEDTEA_UI_THEME").map_or(ThemeSource::Bundled, |v| ThemeSource::File(v.into()))` — with `settings/style.css` still layered over the result, and record P3-D6 as landed.
3. The window's `app_id` is `org.icedtea.Settings` (contract §2). Check with:
   ```bash
   grep -n 'org.icedtea.Settings' settings/src/main.rs
   ```

- [ ] **Step 4: Run the test to verify it passes**

Run:
```bash
mv settings/tests/keybindings_gtk.rs /tmp/keybindings_gtk.rs.bak
cargo test -p icedtea-settings --test keybindings -- --test-threads=1 --nocapture
mv /tmp/keybindings_gtk.rs.bak settings/tests/keybindings_gtk.rs
```
Expected: PASS, 1 test. `--test-threads=1` because each test spawns its own headless compositor and screencopy client; running them in parallel on a loaded machine multiplies the boot time past `REPORT_TIMEOUT`.

- [ ] **Step 5: Commit**

```bash
git add settings/tests/keybindings.rs
git commit -m "$(cat <<'EOF'
test(settings): the Shift+a capture proof, on the real toolkit

Replaces keybindings_gtk.rs's GDK keymap query with an end-to-end proof: a
real zwp_virtual_keyboard_v1 Shift+a press against the real icedtea-settings
binary under the harness compositor, read back through the probe report's
binding line (deviation P3-D3). The page is reached by clicking the
StackSwitcher, with every coordinate derived from the probe report.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
EOF
)"
```

---

## Task 9: The capture lifecycle, end to end

**Files:**
- Modify: `settings/tests/keybindings.rs`
- Test: `settings/tests/keybindings.rs`

**Interfaces:**
- Consumes: Task 8's `Settings`, `PAGE_KEYBINDINGS`, `PAGE_WORKSPACES`, the key constants.
- Produces: three integration tests — `escape_cancels_a_capture`, `a_lone_modifier_press_leaves_the_capture_armed_end_to_end`, `switching_pages_cancels_a_capture_end_to_end` — plus `adding_and_removing_a_workspace_reaches_the_model`.

Each of the first three asserts an **absence** (nothing was bound) and then a **presence** (the very next real capture does bind), so none of them can pass vacuously.

- [ ] **Step 1: Write the failing tests**

Append to `settings/tests/keybindings.rs`:

```rust
/// Escape while armed cancels: nothing is bound, and the row is disarmed --
/// proven by the positive control, a subsequent `b` that binds only after
/// the row is armed again.
///
/// Mutation check: drop `capture_key`'s Escape arm; the first assertion
/// fails, because Escape itself gets stored as the binding. Restore.
#[test]
fn escape_cancels_a_capture() {
    let mut s = Settings::spawn(icedtea_ui::BUNDLED_ADWAITA_LIGHT);
    s.select_page(PAGE_KEYBINDINGS);
    s.wait_for_alloc("keybindings_list");

    s.click_id("kb_set_close");
    s.keyboard.key_press(KEY_ESC);
    s.keyboard.pump();

    // The capture was cancelled, so the next key must not bind anything.
    s.keyboard.key_press(KEY_A);
    s.keyboard.pump();
    std::thread::sleep(Duration::from_millis(400));
    assert!(
        !s.lines().iter().any(|l| l.starts_with("binding ")),
        "Escape must cancel; report:\n{}",
        s.lines().join("\n")
    );

    // Positive control: re-arm and the very next key does bind.
    s.click_id("kb_set_close");
    s.keyboard.key_press(KEY_B);
    s.keyboard.pump();
    let line = s
        .wait_for_line("binding close ")
        .unwrap_or_else(|| panic!("re-arming did not work; report:\n{}", s.lines().join("\n")));
    assert_eq!(line, "binding close - KEY_b");
}

/// A lone Shift press mid-capture leaves the row armed and binds nothing --
/// the GTK behaviour verbatim -- and the real key that follows binds
/// normally, without a second click on Set.
///
/// Mutation check: make `apply_capture` return `true` when
/// `combo_from_keysym` returned `None`; the second assertion fails, because
/// the row disarms and the following `a` binds nothing. Restore.
#[test]
fn a_lone_modifier_press_leaves_the_capture_armed_end_to_end() {
    let mut s = Settings::spawn(icedtea_ui::BUNDLED_ADWAITA_LIGHT);
    s.select_page(PAGE_KEYBINDINGS);
    s.wait_for_alloc("keybindings_list");

    s.click_id("kb_set_close");
    s.keyboard.key_press(KEY_LEFTSHIFT);
    s.keyboard.pump();
    std::thread::sleep(Duration::from_millis(400));
    assert!(
        !s.lines().iter().any(|l| l.starts_with("binding ")),
        "a bare Shift must not become a binding; report:\n{}",
        s.lines().join("\n")
    );

    // Still armed: no second click on Set.
    s.keyboard.key_press(KEY_A);
    s.keyboard.pump();
    let line = s
        .wait_for_line("binding close ")
        .unwrap_or_else(|| panic!("the capture did not stay armed; report:\n{}", s.lines().join("\n")));
    assert_eq!(line, "binding close - KEY_a");
}

/// Leaving the page disarms, so a keystroke typed after the switch cannot
/// silently rebind the row a user left armed -- the GTK `connect_unmap` /
/// `EventControllerFocus` reset.
///
/// Mutation check: drop `m.capturing = None` from `Msg::PageSelected`; the
/// first assertion fails with `binding close - KEY_a`. Restore.
#[test]
fn switching_pages_cancels_a_capture_end_to_end() {
    let mut s = Settings::spawn(icedtea_ui::BUNDLED_ADWAITA_LIGHT);
    s.select_page(PAGE_KEYBINDINGS);
    s.wait_for_alloc("keybindings_list");

    s.click_id("kb_set_close");
    s.select_page(PAGE_WORKSPACES);
    s.wait_for_alloc("workspaces_add");

    s.keyboard.key_press(KEY_A);
    s.keyboard.pump();
    std::thread::sleep(Duration::from_millis(400));
    assert!(
        !s.lines().iter().any(|l| l.starts_with("binding ")),
        "a page switch must disarm the capture; report:\n{}",
        s.lines().join("\n")
    );

    // Positive control: back on the page, arming still works.
    s.select_page(PAGE_KEYBINDINGS);
    s.wait_for_alloc("keybindings_list");
    s.click_id("kb_set_close");
    s.keyboard.key_press(KEY_B);
    s.keyboard.pump();
    let line = s
        .wait_for_line("binding close ")
        .unwrap_or_else(|| panic!("re-arming after the switch failed; report:\n{}", s.lines().join("\n")));
    assert_eq!(line, "binding close - KEY_b");
}

/// The Workspaces page's Add and Remove reach the model: Add grows the row
/// set by one (a new `ws_row_<n>` appears in the probe batch) and Remove
/// shrinks it back. Read through the probe report's `alloc` lines, which is
/// the only thing about a running app this test can see.
///
/// Mutation check: make `remove_workspace` return before removing; the
/// second assertion fails. Restore.
#[test]
fn adding_and_removing_a_workspace_reaches_the_model() {
    let mut s = Settings::spawn(icedtea_ui::BUNDLED_ADWAITA_LIGHT);
    s.select_page(PAGE_WORKSPACES);
    s.wait_for_alloc("workspaces_add");

    let rows_at_rest = s
        .batch()
        .iter()
        .filter(|l| l.starts_with("alloc ws_row_"))
        .count();
    assert!(rows_at_rest >= 1, "the default config has at least one workspace");

    s.click_id("workspaces_add");
    s.wait_for_alloc(&format!("ws_row_{rows_at_rest}"));

    s.click_id(&format!("ws_remove_{rows_at_rest}"));
    let started = Instant::now();
    while started.elapsed() < REPORT_TIMEOUT {
        let rows = s
            .batch()
            .iter()
            .filter(|l| l.starts_with("alloc ws_row_"))
            .count();
        if rows == rows_at_rest {
            return;
        }
        std::thread::sleep(POLL);
    }
    panic!(
        "the row count never returned to {rows_at_rest}; last batch:\n{}",
        s.batch().join("\n")
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run, with the GTK file out of the way as in Task 8:
```bash
mv settings/tests/keybindings_gtk.rs /tmp/keybindings_gtk.rs.bak
cargo test -p icedtea-settings --test keybindings -- --test-threads=1
mv /tmp/keybindings_gtk.rs.bak settings/tests/keybindings_gtk.rs
```
Expected: the four new tests are integration proofs of code Tasks 4–7 already wrote, so a correct implementation passes them on the first run. Prove each is non-vacuous by applying its own recorded mutation check, one at a time, confirming the named assertion goes red, and restoring.

- [ ] **Step 3: Apply the four mutation checks**

For each test, in order, apply the mutation named in its doc comment, run only that test, confirm it fails with the named message, restore, and confirm it passes again:

```bash
cargo test -p icedtea-settings --test keybindings escape_cancels_a_capture -- --test-threads=1
cargo test -p icedtea-settings --test keybindings a_lone_modifier_press_leaves -- --test-threads=1
cargo test -p icedtea-settings --test keybindings switching_pages_cancels -- --test-threads=1
cargo test -p icedtea-settings --test keybindings adding_and_removing_a_workspace -- --test-threads=1
```

- [ ] **Step 4: Run the whole file to verify it passes**

Run:
```bash
mv settings/tests/keybindings_gtk.rs /tmp/keybindings_gtk.rs.bak
cargo test -p icedtea-settings --test keybindings -- --test-threads=1
mv /tmp/keybindings_gtk.rs.bak settings/tests/keybindings_gtk.rs
```
Expected: PASS, 5 tests.

- [ ] **Step 5: Commit**

```bash
git add settings/tests/keybindings.rs
git commit -m "$(cat <<'EOF'
test(settings): the capture lifecycle and workspace edits, end to end

Escape cancels, a lone modifier stays armed, a page switch disarms, and Add
and Remove reach the model. Each absence assertion is paired with a positive
control so none can pass vacuously.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
EOF
)"
```

---

## Task 10: Rest-state screencopy gates for both pages, light/dark/hc

**Files:**
- Modify: `settings/tests/keybindings.rs`
- Test: `settings/tests/keybindings.rs`

**Interfaces:**
- Consumes: Task 8's `Settings` (`capture`, `batch`, `probes`, `to_screen`, `select_page`, `wait_for_alloc`).
- Produces: `workspaces_page_paints_every_probe_point_at_rest` and `keybindings_page_paints_every_probe_point_at_rest`, each run in light, dark and high contrast (contract §2.7, spec §7).

**What the gate asserts.** For the page's own id-bearing widgets that are fully inside the window's client rectangle: every one of them paints — some pixel inside its border box differs from the page background. And separately, every probe point in the batch that lies inside the client rectangle is inside the captured frame. There are **no `KNOWN_BLANK` exemptions for app widgets** (spec §7): every id on the page must paint.

The clipping filter is real, not a loophole: the Keybindings page is a `ScrolledWindow` over 19 rows in a 420 px-tall window, so most rows are laid out below the viewport and their allocations legitimately fall outside it. The gate therefore also asserts a **floor** on how many ids it checked, and names three ids that must always be among them, so a layout regression that pushed everything off-screen fails instead of trivially passing.

- [ ] **Step 1: Write the failing tests**

Append to `settings/tests/keybindings.rs`:

```rust
/// Whether anything inside `rect` (output coordinates) differs from
/// `background`.
///
/// A full scan of the border box, the rule `ui/tests/support/mod.rs`'s
/// `paints_something` settled on: a sparse grid misses a widget that drew
/// only a 1px border, and "painted nothing" reported for something that
/// plainly painted is worse than a slower gate.
fn paints_something(
    frame: &CapturedFrame,
    rect: (i32, i32, i32, i32),
    background: (u8, u8, u8),
) -> bool {
    let (x, y, w, h) = rect;
    if w <= 0 || h <= 0 {
        return false;
    }
    (y..y + h).any(|py| {
        (x..x + w).any(|px| {
            pixel_at(frame, px, py).is_some_and(|got| got != background)
        })
    })
}

/// The pixel at an output coordinate, or `None` when it is outside.
fn pixel_at(frame: &CapturedFrame, x: i32, y: i32) -> Option<(u8, u8, u8)> {
    if x < 0 || y < 0 {
        return None;
    }
    icedtea_harness::pixel_at(frame, x as u32, y as u32)
}

/// Run one page's rest-state gate in `theme_css`.
///
/// `page` is the switcher index; `prefixes` are the id prefixes that belong
/// to the page; `required` are ids that must always be visible; `floor` is
/// the smallest number of ids a healthy layout leaves fully inside the
/// window.
fn rest_state_gate(
    theme_css: &str,
    theme_name: &str,
    page: usize,
    prefixes: &[&str],
    required: &[&str],
    floor: usize,
) {
    let mut s = Settings::spawn(theme_css);
    s.select_page(page);
    s.wait_for_alloc(required[0]);
    // One more settle so the page's first frame is the one on screen, not
    // the switcher's own transition frame.
    std::thread::sleep(Duration::from_millis(300));

    let (win_w, win_h) = {
        let root = s.wait_for_alloc("root");
        (root.2, root.3)
    };
    let frame = s.capture();
    // The page background: two pixels in from the client area's top-left
    // corner, which is the root box's own fill in every bundled theme.
    let (bx, by) = s.to_screen(2, 2);
    let background = pixel_at(&frame, bx, by)
        .expect("the client area's corner is inside the captured frame");

    let batch = s.batch();
    let mut checked: Vec<String> = Vec::new();
    let mut blank: Vec<String> = Vec::new();
    for line in &batch {
        let Some(rest) = line.strip_prefix("alloc ") else {
            continue;
        };
        let mut fields = rest.split_whitespace();
        let Some(id) = fields.next() else { continue };
        if !prefixes.iter().any(|p| id.starts_with(p)) {
            continue;
        }
        let nums: Vec<i32> = fields.filter_map(|v| v.parse().ok()).collect();
        if nums.len() != 4 {
            continue;
        }
        let (x, y, w, h) = (nums[0], nums[1], nums[2], nums[3]);
        // Skip what the ScrolledWindow legitimately clips away.
        if x < 0 || y < 0 || x + w > win_w || y + h > win_h {
            continue;
        }
        let (sx, sy) = s.to_screen(x, y);
        if !paints_something(&frame, (sx, sy, w, h), background) {
            blank.push(format!("#{id} ({x},{y},{w},{h})"));
        }
        checked.push(id.to_string());
    }

    assert!(
        blank.is_empty(),
        "these widgets painted nothing in the {theme_name} theme: {}",
        blank.join(", ")
    );
    assert!(
        checked.len() >= floor,
        "only {} of the page's widgets were inside the window in the {theme_name} \
         theme (expected at least {floor}): {checked:?}",
        checked.len()
    );
    for id in required {
        assert!(
            checked.iter().any(|c| c == id),
            "#{id} must be visible at rest in the {theme_name} theme; checked {checked:?}"
        );
    }

    // Every probe point the window reported is addressable: inside the
    // client rectangle and inside the frame.
    for (label, x, y) in s.probes() {
        if x < 0 || y < 0 || x >= win_w || y >= win_h {
            continue;
        }
        let (sx, sy) = s.to_screen(x, y);
        assert!(
            pixel_at(&frame, sx, sy).is_some(),
            "probe point {label} at ({x},{y}) is outside the captured frame in the \
             {theme_name} theme"
        );
    }
}

/// Ids that belong to the Workspaces page.
const WORKSPACES_PREFIXES: &[&str] = &["workspaces_", "ws_row_", "ws_name_", "ws_remove_"];
/// Ids that belong to the Keybindings page.
const KEYBINDINGS_PREFIXES: &[&str] = &["keybindings_", "kb_row_", "kb_combo_", "kb_set_"];

/// Every Workspaces widget paints at rest.
///
/// Mutation check: give `ws_name_<i>` `.visible(false)`; the `#ws_name_0
/// must be visible` assertion fails. Restore.
#[test]
fn workspaces_page_paints_every_probe_point_at_rest_in_the_light_theme() {
    rest_state_gate(
        icedtea_ui::BUNDLED_ADWAITA_LIGHT,
        "light",
        PAGE_WORKSPACES,
        WORKSPACES_PREFIXES,
        &["workspaces_list", "workspaces_add", "ws_name_0", "ws_remove_0"],
        6,
    );
}

#[test]
fn workspaces_page_paints_every_probe_point_at_rest_in_the_dark_theme() {
    rest_state_gate(
        icedtea_ui::BUNDLED_ADWAITA_DARK,
        "dark",
        PAGE_WORKSPACES,
        WORKSPACES_PREFIXES,
        &["workspaces_list", "workspaces_add", "ws_name_0", "ws_remove_0"],
        6,
    );
}

#[test]
fn workspaces_page_paints_every_probe_point_at_rest_in_the_high_contrast_theme() {
    rest_state_gate(
        icedtea_ui::BUNDLED_ADWAITA_HC,
        "high contrast",
        PAGE_WORKSPACES,
        WORKSPACES_PREFIXES,
        &["workspaces_list", "workspaces_add", "ws_name_0", "ws_remove_0"],
        6,
    );
}

/// Every visible Keybindings widget paints at rest. The page scrolls, so
/// rows below the viewport are excluded by the clipping filter and the
/// floor plus the required ids keep the gate honest.
///
/// Mutation check: render the combo label with an empty string; `#kb_combo_
/// close` paints nothing and the gate fails. Restore.
#[test]
fn keybindings_page_paints_every_probe_point_at_rest_in_the_light_theme() {
    rest_state_gate(
        icedtea_ui::BUNDLED_ADWAITA_LIGHT,
        "light",
        PAGE_KEYBINDINGS,
        KEYBINDINGS_PREFIXES,
        &["keybindings_list", "kb_row_close", "kb_combo_close", "kb_set_close"],
        8,
    );
}

#[test]
fn keybindings_page_paints_every_probe_point_at_rest_in_the_dark_theme() {
    rest_state_gate(
        icedtea_ui::BUNDLED_ADWAITA_DARK,
        "dark",
        PAGE_KEYBINDINGS,
        KEYBINDINGS_PREFIXES,
        &["keybindings_list", "kb_row_close", "kb_combo_close", "kb_set_close"],
        8,
    );
}

#[test]
fn keybindings_page_paints_every_probe_point_at_rest_in_the_high_contrast_theme() {
    rest_state_gate(
        icedtea_ui::BUNDLED_ADWAITA_HC,
        "high contrast",
        PAGE_KEYBINDINGS,
        KEYBINDINGS_PREFIXES,
        &["keybindings_list", "kb_row_close", "kb_combo_close", "kb_set_close"],
        8,
    );
}
```

`icedtea_harness::pixel_at` is the harness's own frame reader; if the harness exposes it under a different path (check `grep -n 'pub fn pixel_at' harness/src/lib.rs`), replace the one-line `pixel_at` wrapper's body with the correct call and change nothing else. If the harness has no such helper, implement the wrapper directly against `CapturedFrame`'s public width/height/pixel accessors — the frame is `Bgr888`, 3 bytes per pixel (memory note: "pixman screencopy format is Bgr888 3bpp not Xrgb"), so the body is:

```rust
    let (w, h) = (frame.width as i32, frame.height as i32);
    if x >= w || y >= h {
        return None;
    }
    let i = ((y * w + x) * 3) as usize;
    let d = &frame.data;
    Some((d[i + 2], d[i + 1], d[i]))
```

- [ ] **Step 2: Run the gates to verify they fail**

Run:
```bash
mv settings/tests/keybindings_gtk.rs /tmp/keybindings_gtk.rs.bak
cargo test -p icedtea-settings --test keybindings _at_rest_ -- --test-threads=1
mv /tmp/keybindings_gtk.rs.bak settings/tests/keybindings_gtk.rs
```
Expected: FAIL at first with a compile error until `pixel_at`'s body matches the harness's actual API; after that, either PASS (the pages paint) or a named list of blank widgets. Any blank widget is a real defect in Task 2's or Task 6's view — fix the view, never the gate's threshold.

- [ ] **Step 3: Apply the two mutation checks**

```bash
# Workspaces: add .visible(false) to the ws_name_<i> entry in
# settings/src/pages/workspaces.rs::view, run, see
# "#ws_name_0 must be visible at rest", restore.
cargo test -p icedtea-settings --test keybindings workspaces_page_paints -- --test-threads=1

# Keybindings: render `label("")` instead of `label(&text)` for the combo in
# settings/src/pages/keybindings.rs::view, run, see
# "these widgets painted nothing … #kb_combo_close", restore.
cargo test -p icedtea-settings --test keybindings keybindings_page_paints -- --test-threads=1
```

- [ ] **Step 4: Run the whole file to verify it passes**

Run:
```bash
mv settings/tests/keybindings_gtk.rs /tmp/keybindings_gtk.rs.bak
cargo test -p icedtea-settings --test keybindings -- --test-threads=1
mv /tmp/keybindings_gtk.rs.bak settings/tests/keybindings_gtk.rs
```
Expected: PASS, 11 tests.

- [ ] **Step 5: Commit**

```bash
git add settings/tests/keybindings.rs
git commit -m "$(cat <<'EOF'
test(settings): rest-state screencopy gates for both P3 pages

Every id-bearing widget of the Workspaces and Keybindings pages that is
fully inside the window paints something, in light, dark and high contrast,
with no KNOWN_BLANK exemptions. The Keybindings page scrolls, so clipped
rows are excluded by a derived filter guarded by a count floor and four
always-visible ids, and every reported probe point must be inside the
captured frame.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
EOF
)"
```

---

## Task 11: Delete the GTK test, record the deviations, run the part gate

**Files:**
- Delete: `settings/tests/keybindings_gtk.rs`
- Modify: `docs/superpowers/plans/2026-09-03-m5-part0-contract.md` (§6 only)

**Interfaces:**
- Consumes: everything Tasks 1–10 produced.
- Produces: a GTK-free `settings/tests/`, seven `P3-D*` amendments in the contract's §6 ledger, and a green P3 gate set.

- [ ] **Step 1: Write the failing check**

Run:
```bash
grep -rn 'gtk4\|glib::\|gio::\|gdk::\|pango::\|cairo::' settings/src settings/tests
```
Expected: FAIL — `settings/tests/keybindings_gtk.rs` matches on `gtk4::glib::translate::IntoGlib`, `gtk4::prelude::*`, `gtk4::init()` and `unshifted_keysym`.

- [ ] **Step 2: Delete the GTK test**

```bash
git rm settings/tests/keybindings_gtk.rs
```

Its property is not lost: `settings/tests/keybindings.rs::shift_a_while_capturing_records_base_a_with_shift` proves the same SHIP-BLOCKER through the real toolkit and a real virtual keyboard, and `pages::keybindings::tests::a_shifted_press_captures_the_base_keysym` proves it at the unit level. `pages::keybindings::unshifted_keysym` — the function the deleted file imported — was removed by P1 (contract §4.2) and replaced by `normalise_keysym`.

- [ ] **Step 3: Record the deviations**

Append to `docs/superpowers/plans/2026-09-03-m5-part0-contract.md`'s §6, after whatever P1 and P2 left there, one block per deviation in the M3 §10 shape. Copy each of this plan's seven **Contract deviations** entries verbatim into that shape:

```markdown
### P3-D1 — `capture_key` takes an `armed: bool`

**Carried out by:** P3 (`settings/src/pages/keybindings.rs`, `settings/src/app.rs`).

**Contract says** (§2.7, §2.3): `pub fn capture_key(ev: &KeyEvent) -> Option<Msg>`,
wired as `.on_key(|ev| pages::keybindings::capture_key(ev))`, with `update`
ignoring `Msg::KeyCaptured` when `capturing.is_none()`.

**As shipped:** `pub fn capture_key(armed: bool, ev: &KeyEvent) -> Option<Msg>`,
returning `None` whenever `!armed`; the root wires
`.on_key({ let armed = m.capturing.is_some(); move |ev| capture_key(armed, ev) })`.

**Ruling:** `GenericC::on_event`'s `Event::Key` arm sets `cx.handled = true` for
any message a `KeyPressed` handler returns (`ui/src/view/controller.rs:508-513`),
and M3 P4-D19 makes a handled event end the whole dispatch — so a root handler
that answers every press swallows every keystroke in the window, a focused
`Entry`'s included. The arming gate belongs where `handled` is decided.
`update` keeps its own `capturing.is_none()` guard as a second gate.

### P3-D2 — P3 edits `settings/src/app.rs`

**Carried out by:** P3.

**Contract says** (§5, P3 *Owns*): the two page modules, `style.css`'s
`.conflict` rule, `tests/keybindings.rs`, and the deletion of
`tests/keybindings_gtk.rs`.

**As shipped:** plus a bounded edit to `settings/src/app.rs` — the seven
Workspaces/Keybindings `update` arms, `Msg::PageSelected`'s `capturing` reset,
the root `.on_key` arming, and the two `stack_page` call sites.

**Ruling:** §2.2 puts the whole `update` match and §2.3 the whole root `view`
in `app.rs`, so no part can implement a page's behaviour without touching it.
The same applies to P2 and P4; the *Owns* lists name new files, not the shared
ones every page part necessarily edits.

### P3-D3 — the app probe report gains a `binding` line

**Carried out by:** P3 (`settings/src/pages/keybindings.rs`).

**Contract says** (§2.7): the re-expressed capture test "asserts through the
app's probe report that the stored combo is `{ modifiers: ["SHIFT"], key:
"KEY_a" }`". §M5-D9 defines only `probe <label> <x> <y>` and
`alloc <id> <x> <y> <w> <h>`.

**As shipped:** when `$ICEDTEA_PROBE_REPORT` is set, `apply_capture` appends
`binding <action> <modifiers joined by '+', or '-'> <key>`.

**Ruling:** neither existing line can express a `KeyCombo`. The raw field
spellings are written rather than `format_combo`'s display form, so the
assertion pins the serialization `compositor/src/input.rs` matches on.
Production is unaffected: the variable is unset and the writer is a no-op.

### P3-D4 — both pages' gates live in `settings/tests/keybindings.rs`

**Carried out by:** P3.

**Contract says** (§5): P3 owns `tests/keybindings.rs (new)`. §2.7 names
`workspaces_page_paints_every_probe_point_at_rest` without a file.

**As shipped:** one integration binary carries both pages' harness tests and
gates.

**Ruling:** one file keeps the scaffolding single-sourced without creating a
`settings/tests/support/` module P2 may also be creating in parallel.

### P3-D5 — six pure helpers not named in the contract

**Carried out by:** P3.

**Contract says** (§2.7): the Workspaces mutation rules and the capture-store
rule as prose inside `update`.

**As shipped:** `pages::workspaces::{can_remove, add_workspace,
remove_workspace, rename_workspace}` and `pages::keybindings::{apply_capture,
report_binding}`, with `update`'s arms as one-line delegations.

**Ruling:** `App::run_offscreen` returns `Frames` and no model, so a rule that
lives only inside an `update` arm has no unit test. Moving each rule into a
`pub fn` in a P3-owned page module makes all of them directly testable.

### P3-D6 — `$ICEDTEA_UI_THEME` selects `icedtea-settings`' base theme

**Carried out by:** P1 if it shipped the reading, otherwise P3
(`settings/src/main.rs`).

**Contract says:** nothing about how settings picks a theme; §2.7 and spec §7
require rest-state gates in light, dark and high contrast.

**As shipped:** `$ICEDTEA_UI_THEME` names a **complete** base theme file used
whole — `ui/src/bin/window-probe.rs:41`'s convention — with `settings/style.css`
still layered over it (§2.9).

**Ruling:** reusing the toolkit's own existing convention beats inventing a
settings-specific `--theme` flag, and it is the only hook the gates need.

### P3-D7 — two ids beyond §2.3's normative list

**Carried out by:** P3.

**Contract says** (§2.3): `workspaces.{list, add}` + `ws_name_<i>`/
`ws_remove_<i>`; `keybindings.{list}` + `kb_row_<action>`/`kb_set_<action>`.

**As shipped:** plus `ws_row_<i>` (the `list_box_row`, so the rest-state gate
can address a row as a unit) and `kb_combo_<action>` (the combo label).

**Ruling:** additive; no listed id changes, and both are addressed by the P3
gates.
```

- [ ] **Step 4: Run the part gate**

```bash
cargo fmt --all --check
cargo clippy -p icedtea-settings --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings
cargo test -p icedtea-settings -- --test-threads=1
grep -rn 'gtk4\|glib::\|gio::\|gdk::\|pango::\|cairo::' settings/src settings/tests
cargo tree -p icedtea-settings -e normal -i gtk4
```

Expected:
- `fmt`/`clippy`: PASS, all four invocations.
- `cargo test -p icedtea-settings`: PASS. Lib tests include `model.rs`'s 10, `displays_canvas.rs`'s 8, `displays/state.rs`'s 12, `compositor_reload.rs`'s 1, `pages::workspaces`'s **17** (6 inherited + 6 from Task 1 + 4 from Task 2 + 1 from Task 3) and `pages::keybindings`'s **30** (6 inherited + 7 from Task 4 + 5 from Task 5 + 5 from Task 6 + 7 from Task 7). Integration tests include `keybindings` (11), `outputs_client`, and `live_apply` (which spawns the real `icedtea-compositor` binary and is untouched).
- The `grep`: **no output**.
- `cargo tree … -i gtk4`: `error: package ID specification 'gtk4' did not match any packages`.

Then confirm the inherited toolkit gates are still green — P3 changed no file under `ui/`, so this is a regression check, not a fix opportunity:

```bash
cargo test -p icedtea-ui --test gallery_gate -- --test-threads=1
cargo test -p icedtea-ui --test interaction_gate -- --test-threads=1
cargo test -p icedtea-ui --test window_events -- --test-threads=1
cargo test -p icedtea-ui --test widget_pixels
cargo test -p icedtea-ui --test node_trees
cargo test -p icedtea-ui --test reconcile_props
cargo test -p icedtea-ui --test counter_app
```
Expected: PASS, `interaction_gate` at 16/16.

- [ ] **Step 5: Commit**

```bash
git add -A settings/tests docs/superpowers/plans/2026-09-03-m5-part0-contract.md
git commit -m "$(cat <<'EOF'
test(settings): delete keybindings_gtk.rs and record P3's deviations

The GTK capture test's property is carried by the toolkit test
(shift_a_while_capturing_records_base_a_with_shift) and its unit twin
(a_shifted_press_captures_the_base_keysym). settings/src and settings/tests
now match no gtk4/glib/gio/gdk/pango/cairo reference at all.

Contract §6 gains P3-D1..P3-D7: the armed capture_key, the bounded app.rs
edit, the binding report line, the single P3 test file, the six pure
helpers, the ICEDTEA_UI_THEME reading, and the two extra widget ids.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
EOF
)"
```

---

## Self-Review

### 1. Spec coverage

| Spec / contract requirement | Where | Covered by |
|---|---|---|
| Workspaces: `ListBox` of rows (`Entry` + Remove), Add | spec §5.4, contract §2.7 | Task 2 |
| Remove insensitive at the last row (`set_remove_sensitivity`, computed) | contract §2.7 | Tasks 1, 2 (`can_remove`, `the_last_workspaces_remove_button_is_insensitive`) |
| `Msg::WorkspaceAdded` pushes `next_workspace_name` | contract §2.7 | Tasks 1, 3 |
| `Msg::WorkspaceRemoved(i)` removes and prunes orphaned bindings | contract §2.7 | Tasks 1, 3 (`remove_workspace_prunes_the_bindings_it_orphans`, `the_update_arms_delegate_to_the_workspace_mutators`) |
| Rows keyed so a rename does not rebuild the `Entry` | contract §2.3, §2.7 | Task 2 (`workspace_rows_are_keyed_by_index`) |
| `RebuildSlot`/`on_workspaces_changed` disappears; Keybindings reads the count each `view` | contract §2.7 | Task 6 (`view` recomputes `action_list` from `working.workspace_names.len()`) |
| Keybindings: `ScrolledWindow` of action rows, one per `action_list` | contract §2.7 | Task 6 |
| Row shows `format_combo`, `(unbound)`, and the `conflict` class | contract §2.7 | Task 6 |
| Set button flips to "Press a key…" for the armed row | contract §2.7 | Task 6 |
| Capture is `model.capturing` + one window-root `on_key` using `Keymap::base_keysym` | spec §5.4, contract §2.7 | Tasks 4, 7 |
| Capture rule 1: `None` unless pressed and not repeat | contract §2.7 | Task 4 (`releases_and_repeats_are_declined`) |
| Capture rule 2: Escape cancels | contract §2.7 | Tasks 4, 9 |
| Capture rule 3: mods from `ev.mods`, never `ev.consumed` | contract §2.7 | Task 4 (`modifiers_are_read_from_the_effective_state`) |
| Capture rule 3: `normalise_keysym(ev.base, ev.keysym)` | contract §2.7, M5-D7 | Tasks 4, 8 |
| Capture rule 4: inert when nothing armed; `PageSelected` clears | contract §2.7 | Tasks 7, 9 |
| Capture rule 5: a lone modifier leaves the capture armed | contract §2.7 | Tasks 5, 7, 9 |
| Capture rule 6: conflicts never block Apply; `.conflict` sheet rule replaces `install_conflict_css` | contract §2.7, §2.9 | Tasks 5, 6 |
| Normative ids the gates address | contract §2.3 | Tasks 2, 6; listed in File Structure |
| Kept verbatim: `workspaces.rs`'s 6 and `keybindings.rs`'s 6 | contract §2.7 | Tasks 1, 4 (never edited); count proven in Task 11 |
| `keybindings_gtk.rs` → `tests/keybindings.rs::shift_a_while_capturing_records_base_a_with_shift`, `VirtualKeyboardClient`-driven | contract §2.7, spec §7 | Task 8 |
| New: `a_lone_modifier_press_leaves_the_capture_armed` | contract §2.7 | Tasks 5, 7, 9 |
| New: `escape_cancels_a_capture` | contract §2.7 | Tasks 4, 9 |
| New: `switching_pages_cancels_a_capture` | contract §2.7 | Tasks 7, 9 |
| New: `removing_a_workspace_prunes_its_bindings` | contract §2.7 | Task 1 (`remove_workspace_prunes_the_bindings_it_orphans`) + Task 3 through the real fold |
| Rest-state gates for both pages, light/dark/hc, no app-widget exemptions | contract §2.7, spec §7 | Task 10 |
| Mutation check on `normalise_keysym` (return `modified` unconditionally ⇒ test fails) | contract §5, P3 *Gate* | Task 8's mutation check (recorded on `shift_a_while_capturing_records_base_a_with_shift`) |
| `settings/style.css` gains the `.conflict` rule | contract §2.9 | Task 6 |
| Delete `settings/tests/keybindings_gtk.rs` | contract §4.2 | Task 11 |
| Must not touch other pages, `ui/`, `model.rs`, `compositor_reload.rs`, `outputs/protocol.rs`, `live_apply.rs`, `outputs_client.rs` | contract §5 | File Structure names every file P3 touches; none of those is in it |
| Every load-bearing test records a mutation check | contract §5 cross-cutting | Every test's doc comment; Tasks 9 and 10 execute theirs as explicit steps |
| No M6 scope | spec §11 | No IME, DnD, `accesskit`, emoji, portal or `StackSidebar` work appears in any task |

No gap found.

### 2. Placeholder scan

Searched for `TBD`, `TODO`, `implement later`, `fill in details`, "appropriate error handling", "handle edge cases", "write tests for the above", "similar to Task N". None present. Every code step carries the literal code. Every type named in a later task is defined in an earlier one or in the *Interfaces this part consumes* block. The two places this plan says "if P1 shipped it differently" (Task 3's `update` entry point, Task 8 Step 3's three scaffolding checks, Task 10's `pixel_at` body) are not deferred work: each names the exact check to run, the exact alternative to write, and the exact file, and each resolves before its own step ends.

### 3. Type consistency vs the contract

- `capture_key` is `(armed: bool, ev: &KeyEvent) -> Option<Msg>` in Task 4's implementation, Task 4's tests, Task 7's root wiring and P3-D1 — one shape, deviating from the contract exactly once and recorded.
- `apply_capture(&mut Config, &str, u32, CaptureMods) -> bool` is identical in Task 5's implementation, Task 5's and Task 7's tests, Task 7's `update` arm and P3-D5.
- `can_remove(&Config) -> bool`, `rename_workspace(&mut Config, usize, &str)`, `add_workspace(&mut Config)`, `remove_workspace(&mut Config, usize)` are identical in Tasks 1, 2, 3 and P3-D5.
- `view(m: &SettingsModel) -> View<Msg>` is the signature contract §2.3 fixes for both page modules, and both Tasks 2 and 6 use it verbatim.
- `Msg` variants used — `WorkspaceRenamed(usize, String)`, `WorkspaceRemoved(usize)`, `WorkspaceAdded`, `CaptureArmed(String)`, `CaptureCancelled`, `KeyCaptured { keysym: u32, mods: CaptureMods }`, `PageSelected(usize)` — match contract §2.2 field-for-field. No new variant is introduced.
- Ids: `workspaces_list`, `workspaces_add`, `ws_row_<i>`, `ws_name_<i>`, `ws_remove_<i>`, `keybindings_list`, `kb_row_<action>`, `kb_combo_<action>`, `kb_set_<action>` — spelled identically in File Structure, Task 2, Task 6, Task 8's `click_id` calls, Task 9 and Task 10's prefix/required lists.
- The report line is `binding <action> <mods> <key>` in Task 5's implementation, Task 5's unit test, Tasks 8 and 9's assertions and P3-D3 — one grammar, one separator (`+`), one empty marker (`-`).
- `Msg` derives `Clone, Debug` and **not** `PartialEq` (contract §2.2), which is why every test in this plan matches with `matches!`/`match` and uses `.is_none()` rather than `assert_eq!(…, None)`. Task 4's Step 1 states that substitution explicitly.
