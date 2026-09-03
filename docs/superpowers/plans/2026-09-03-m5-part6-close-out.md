# Pure-Rust GTK-themed UI — M5 Part 6: close-out: dependency audit, deleted tests, full gate set, deviation record — Implementation Plan

> **Note for agentic workers:** this plan is written to be executed with the
> `superpowers:subagent-driven-development` skill — one subagent per task, each
> task self-contained (files, interfaces, failing test, implementation, gate
> command, commit). Do not batch tasks; do not skip the failing-test step. Every
> task ends on a green `cargo test -p <crate>` and a commit.

## Contract deviations

The binding contract is
`docs/superpowers/plans/2026-09-03-m5-part0-contract.md`. Every signature and
every command in this plan is copied from it verbatim except the six items
below. Each one is appended to the contract's §6 in Task 6, in the M3 §10 shape
(**Carried out by / Added / Contract says / As shipped / Ruling**).

1. **`P6-D1` — the close-out sweeps stale GTK *doc comments* out of files
   earlier parts were told not to touch.** §4.1 requires

   ```
   grep -rn 'gtk4\|glib::\|gio::\|gdk::\|pango::\|cairo::' settings/src settings/tests shell/src shell/tests
   ```

   to be empty. It cannot be, however perfect the migration: `settings/src/model.rs`
   (three lines), `settings/src/outputs/protocol.rs` (two) and
   `settings/src/outputs/mod.rs` (one) are listed in §0/§2.1 as **unchanged**,
   and every one of them still describes GTK machinery in prose — "the (GTK)
   view converts `gdk::ModifierType`", "no `gtk4`, `gio`, or `glib`", "a
   `gio::Socket` glib source". `shell/src/{compositor_client,clip_client}.rs`
   (P5 must-not-touch) say "the GTK thread" six times between them.
   **Ruling:** P6 rewrites those comment lines and nothing else — no signature,
   no expression, no test. It is the only edit this part makes to code files,
   it is behaviour-free by inspection (`git diff` shows only `//!`/`///`/`//`
   lines), and the audit test in Task 1 is what proves the sweep is complete.
   Stale documentation of a deleted dependency is exactly the residue a
   close-out exists to remove.

2. **`P6-D2` — the audits ship as tests, not only as shell commands.** §4.1
   states the dependency audit as three `cargo tree` invocations and two greps
   run once. A command run once at close-out proves nothing on the next commit.
   **As shipped:** `settings/tests/dependency_audit.rs` and
   `shell/tests/dependency_audit.rs` assert the same facts hermetically — they
   read `Cargo.toml`, the workspace `Cargo.lock` and the crate's own `.rs`
   files, and never invoke `cargo` (a nested `cargo` under `cargo test`
   contends for the build lock and can hang). §4.1's exact `cargo tree`
   commands are **still run once by hand**, in Task 1 Step 5 and Task 2 Step 5,
   as independent corroboration of the lockfile assertion; their output goes
   into the commit message.

3. **`P6-D3` — the workspace-wide lockfile assertion lives in the settings
   crate.** A workspace-scope test needs a crate to live in, and every crate
   that is not `settings/`, `shell/` or `ui/` is out of M5 scope (contract
   header). **Ruling:** `the_workspace_lockfile_has_no_gtk_stack_package` and
   `no_new_gtk_stack_package_slips_into_the_lockfile` live in
   `settings/tests/dependency_audit.rs`, documented there as workspace-scope
   rather than crate-scope. `shell/tests/dependency_audit.rs` is crate-scoped
   only, so the two files are not copies of each other.

4. **`P6-D4` — `cargo tree -i <absent-package>` fails; it does not print
   "package ID not found".** §4.1 says the invert query "must print `package ID
   not found`". The real behaviour, measured on this machine on 2026-09-03, is
   exit status **101** with

   ```
   error: package ID specification `gtk4` did not match any packages
   ```

   on stderr. **Ruling:** the plan pins the real string and the real exit
   status, and the corroboration step asserts the command *fails* rather than
   grepping its stdout.

5. **`P6-D5` — `cargo doc` is run with `RUSTDOCFLAGS="-D warnings"`.** §4.4
   writes `cargo doc --workspace --no-deps  # -D warnings`; the flag is not a
   `cargo doc` flag. **As shipped:** `RUSTDOCFLAGS="-D warnings" cargo doc
   --workspace --no-deps`, the M3 P8 precedent.

6. **`P6-D6` — §4.2's deleted files and deleted-in-place items get a ledger
   test each.** §4.2 lists six deleted files and thirteen deleted-in-place
   items with no proof mechanism, and the milestone rule this part is judged on
   is "every deletion is paired with the test that proves the replacement".
   **As shipped:** `settings/tests/deletion_ledger.rs` and
   `shell/tests/deletion_ledger.rs` assert, per crate: every deleted path is
   absent; every deleted identifier is absent by an exact pattern; every moved
   suite still carries its exact test count; and every replacement gate named
   by the contract exists by test-function name. The pairing is data in the
   test file (`DELETED_WITH_REPLACEMENT`), so it cannot drift from the prose.

---

## Goal

Close M5 by proving the three claims the milestone actually makes, and by
writing down what it cost.

1. **The GTK stack is gone.** Not "not imported any more" — *gone*: no `gtk4`,
   `gtk4-layer-shell`, `glib`, `gio`, `gdk4`, `pango`, `cairo` in either app's
   dependency tree, none of the twenty-four GTK-stack packages left in the
   workspace `Cargo.lock`, and no GTK name left in `settings/` or `shell/`
   sources — including the doc comments that outlived the code they describe.
2. **Nothing was deleted without a replacement.** Six files and thirteen
   in-place items went away across P1–P5. Each is paired, in a test, with the
   thing that now proves the behaviour it used to prove — or, for the one case
   where the property became true by construction, with the contract record
   that says so.
3. **The whole gate set is green at once.** Per-part gates prove a part; the
   milestone is proven only by running M1's, M2's, M3's and M5's gates together
   on one tree, once, and recording what they reported.

Then the ledger: contract §6 gets the eleven P0 amendments marked landed, the
six milestone-level records §4.3 enumerates, and P6's own six deviations; the
two spec status lines and `ui/README.md` stop lying about what ships; and the
branch stops at the owner's consent gate with a PR body that states the IME
regression in plain words.

**P6 fixes no behaviour.** Contract §5: "Must not touch: any behaviour — a gate
failure at close-out is fixed in the owning part's fix wave, not in the
close-out." If a gate fails in Task 5, this part stops and routes the failure
to the part that owns the file.

## Architecture

```
settings/tests/dependency_audit.rs   [P6, new]  crate-scope GTK audit + the two
                                                workspace-scope lockfile tests
                                                (P6-D3), all hermetic
settings/tests/deletion_ledger.rs    [P6, new]  §4.2's settings half: absent
                                                paths, absent identifiers, moved
                                                test counts, replacement gates
shell/tests/dependency_audit.rs      [P6, new]  crate-scope GTK audit + the
                                                stale-prose sweep guard
shell/tests/deletion_ledger.rs       [P6, new]  §4.2's shell half

settings/src/model.rs                [P6, comments only]  P6-D1
settings/src/outputs/mod.rs          [P6, comments only]  P6-D1
settings/src/outputs/protocol.rs     [P6, comments only]  P6-D1
shell/src/compositor_client.rs       [P6, comments only]  P6-D1
shell/src/clip_client.rs             [P6, comments only]  P6-D1
shell/tests/live_dbus.rs             [P6, comments only]  P6-D1

docs/…/plans/2026-09-03-m5-part0-contract.md   [P6]  §6, the whole ledger
docs/…/specs/2026-09-03-…-m5-…-design.md       [P6]  status line
docs/…/specs/2026-08-20-pure-rust-gtk-ui-design.md [P6]  status + item 5
ui/README.md                                   [P6]  the M5 sentence + gate list
docs/…/plans/2026-09-03-m5-pr-body.md          [P6, new]  the PR body, unpushed
```

Three properties shape every test in this part:

- **Hermetic.** No test spawns `cargo`, a compositor, or a bus. Each reads
  files under the workspace root, derived from `env!("CARGO_MANIFEST_DIR")`.
  That is what makes the audit cheap enough to keep forever instead of being a
  one-off close-out ritual.
- **Self-excluding.** The audit files quote the very tokens they forbid, so the
  source walker skips `dependency_audit.rs` and `deletion_ledger.rs` by file
  name. Nothing else is skipped.
- **Data, not prose.** Deleted paths, deleted identifiers, moved test counts
  and required gate names are `const` tables. A reviewer reads the table, not a
  paragraph, and the table is what fails.

## Tech Stack

| Concern | API |
|---|---|
| File walking | `std::fs::read_dir`, an explicit `Vec<PathBuf>` stack (no `walkdir` — dev-deps are `icedtea-harness` + `tempfile`, and P6 adds none) |
| Path roots | `env!("CARGO_MANIFEST_DIR")`, `Path::parent` for the workspace root |
| Manifest / lockfile reading | `std::fs::read_to_string` + `str::lines` + `str::strip_prefix` (no `toml` dependency: the two facts needed are line-shaped) |
| Test counting | `str::matches("#[test]").count()` |
| Assertions | `assert!` with `{offenders:#?}` so a failure names every offending file and line |
| Dependency corroboration | `cargo tree -e normal -i <pkg>`, run by hand (P6-D2) |
| Gate run | `cargo fmt`, `cargo clippy`, `cargo doc`, `cargo test` |

**Facts this part depends on, verified on this machine on 2026-09-03 against
`develop` @ `d9cee52`:**

- The workspace `Cargo.lock` carries exactly twenty-four GTK-stack packages:
  `cairo-rs`, `cairo-sys-rs`, `field-offset`, `gdk-pixbuf`, `gdk-pixbuf-sys`,
  `gdk4`, `gdk4-sys`, `gio`, `gio-sys`, `glib`, `glib-macros`, `glib-sys`,
  `gobject-sys`, `graphene-rs`, `graphene-sys`, `gsk4`, `gsk4-sys`, `gtk4`,
  `gtk4-layer-shell`, `gtk4-layer-shell-sys`, `gtk4-macros`, `gtk4-sys`,
  `pango`, `pango-sys`. Twenty-three of them match one of the prefixes
  `gtk|gdk|gsk|glib|gio|gobject|pango|cairo|graphene`; `field-offset` does not,
  which is why the explicit list exists beside the prefix rule. Nothing else in
  the lockfile matches those prefixes, so the prefix rule has no false positive
  to exempt.
- `icedtea-notifications`, `icedtea-clipboard`, `icedtea-compositor`,
  `icedtea-harness`, `icedtea-config` and `icedtea-contract` declare no GTK
  dependency: `settings/` and `shell/` are the only holders, so removing them
  empties the lockfile of all twenty-four.
- `cargo tree -p icedtea-shell -e normal -i pango` today prints the real
  `pango v0.22.8 → gdk4 → gsk4 → gtk4 → gtk4-layer-shell` chain; with the
  package absent the same command exits `101` with `error: package ID
  specification \`pango\` did not match any packages` (P6-D4).
- Today's `#[test]` counts, which the moved suites must still report after the
  extraction: `settings/src/pages/displays_canvas.rs` **8**,
  `settings/src/pages/displays.rs` **12** (moving to
  `settings/src/pages/displays/state.rs`), `settings/src/pages/workspaces.rs`
  **6**, `settings/src/pages/keybindings.rs` **6**,
  `settings/src/pages/appearance.rs` **2**, `settings/src/model.rs` **10**,
  `settings/src/compositor_reload.rs` **1**, `shell/src/taskbar.rs` **5**,
  `shell/src/clipboard.rs` **1**, `shell/src/compositor_client.rs` **1**.
- Today's `ui/tests` counts, the M1/M2/M3 baselines Task 5 checks against:
  `themed_button_offscreen` 4, `adwaita_coverage` 10, `gtk4_property_reference`
  4, `transition_screencopy` 1, `layer_shell_screencopy` 3, `window_events` 7,
  `counter_app` 4, `node_trees` 55, `reconcile_props` 3, `widget_pixels` 40,
  `icon_theme` 5, `gallery_gate` 8, `interaction_gate` 16.
- There is no `TODO`, `FIXME`, `todo!(` or `unimplemented!(` anywhere in
  `settings/src`, `settings/tests`, `shell/src` or `shell/tests` today, so the
  "no deferred work" assertion starts green and can only be broken by M5.
- `settings/tests/` has no `fixtures/` or `support/` subdirectory today; P1–P4
  may add one. The walker is recursive, so it covers whatever lands.

## Spec

- `docs/superpowers/specs/2026-09-03-pure-rust-gtk-m5-app-migrations-design.md`
  — §8 (close-out, all of it), §7 (the gate inventory this part runs in full),
  §2 (the eleven inherited decisions and D1–D9), §10 (the risk row "IME
  regression at deletion"), §11 (out of scope).
- `docs/superpowers/plans/2026-09-03-m5-part0-contract.md` — **§4 in full**
  (P6's normative interface: §4.1 the dependency audit, §4.2 the deleted files
  and items, §4.3 the deviations record, §4.4 the final gate list), §5 P6's
  owns/consumes/must-not-touch/gate row, §5's cross-cutting rules, §6 (where
  Task 6 writes).
- `docs/superpowers/plans/2026-08-27-m3-part0-contract.md` — §10 (the
  amendment shape Task 6 copies; P8-D69's `KNOWN_BLANK_AT_REST` record, which
  M5-D8 supersedes) and §11 (E1–E21, still binding).

## Global Constraints

- **Crate pins (do not bump, do not add others):** `wayland-client` 0.31,
  `wayland-protocols` 0.32, `wayland-protocols-wlr` 0.3, `zbus` 5 (workspace),
  `rustix` 1, `skia-rs-safe` 0.4.0, `taffy` 0.14, `xkbcommon` 0.9,
  `crossbeam-channel` 0.5 (workspace), `async-channel` 2, `redb` 3 (workspace),
  `tempfile` 3 (dev), `wlr` 0.20.29. **P6 adds no dependency at all** — not
  `toml`, not `walkdir`, not `regex`. Every audit is `std`.
- **No `gtk4`, `gtk4-layer-shell`, `glib`, `gio`, `gdk`, `gdk4`, `pango`,
  `cairo` or `gobject` in a migrated app** — this part is the thing that proves
  it.
- **Edition 2024, `rust-version = "1.94"`,** both inherited from
  `[workspace.package]`. Do not set them per crate.
- **Gates, every task:**
  `cargo test -p icedtea-settings` and/or `cargo test -p icedtea-shell` for the
  crate the task touched, plus
  `cargo clippy -p <crate> --all-targets -- -D warnings` and
  `cargo fmt --all --check`. Task 5 runs the full set of §4.4 including
  `cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings`.
- **`cargo fmt --all --check` is a hard gate.** Run `cargo fmt --all` before
  every commit.
- **Every M1/M2/M3 gate stays green:** `themed_button_offscreen`,
  `adwaita_coverage`, `gtk4_property_reference`, `layer_shell_screencopy`,
  `transition_screencopy`, `window_events`, `counter_app`, `node_trees`,
  `widget_pixels`, `reconcile_props`, `gallery_gate` (light/dark/hc) and
  `interaction_gate` (16/16). P6 edits none of their files.
- **Parts execute in order P0 → P1 → P2 → P3 → P4 → P5 → P6.** P6 may consume
  anything P0–P5 produced; everything it consumes has already landed on
  `rebuild/pure-rust-gtk-m5` when this part starts.
- **This part deletes**, and **every deletion is paired with the test that
  proves the replacement.** The pairing is the `DELETED_WITH_REPLACEMENT` table
  in each ledger file. The one deletion with no replacement test —
  `settings/tests/appearance_gtk.rs` — is paired instead with the contract §6
  record that explains why the property is true by construction, and the ledger
  says so in the same table.
- **P6 fixes no behaviour.** A failing gate is routed to the owning part.
  The single exception, enumerated: the comment-only sweep of P6-D1.
- **Untrusted input never panics.** In P6 that is the file walker and the two
  line parsers: a missing directory yields no files rather than a panic, and a
  `Cargo.lock` whose format changed fails a named assertion rather than
  producing a vacuous pass.
- **Every load-bearing test states its mutation check** in its doc comment: the
  exact edit that must make it fail, confirmed by running it, then reverted.
- **Commit trailer, every commit:**
  `Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>`
- **Do not merge.** The branch is presented for review; merging is a consent
  stop owned by the repository owner (Task 8).

---

## File Structure

| File | Status | Responsibility |
|---|---|---|
| `settings/tests/dependency_audit.rs` | create | Crate-scope GTK audit for `icedtea-settings` (manifest, sources, deferred work) plus the two workspace-scope `Cargo.lock` tests (P6-D3). Hermetic; no `cargo`. |
| `settings/tests/deletion_ledger.rs` | create | §4.2's settings half: two deleted files absent, the settings deleted-in-place identifiers absent, the six moved suites' `#[test]` counts, and every settings gate the contract names present by function name. |
| `shell/tests/dependency_audit.rs` | create | Crate-scope GTK audit for `icedtea-shell`, plus the stale-prose guard (`"GTK thread"`, `"GTK side"`, `"the render half"`). |
| `shell/tests/deletion_ledger.rs` | create | §4.2's shell half: `bridge.rs` and `shell_gtk.rs` absent, `render`/`connect_activation`/`pub use gtk4` gone, the three kept suites' counts, and the six shell gate names present. |
| `settings/src/model.rs` | modify | Comments only (P6-D1): three doc lines that describe a GTK view that no longer exists. |
| `settings/src/outputs/mod.rs` | modify | Comments only (P6-D1): the module doc's `client`/`gio::Socket` bullet. |
| `settings/src/outputs/protocol.rs` | modify | Comments only (P6-D1): the module doc's "separate from GDK's" / "no `gtk4`, `gio`, or `glib`" / "glib main-loop integration" sentences. |
| `shell/src/compositor_client.rs` | modify | Comments only (P6-D1): four "GTK thread"/"GTK side"/"GTK running" mentions. |
| `shell/src/clip_client.rs` | modify | Comments only (P6-D1): three "GTK thread"/"GTK side" mentions. |
| `shell/tests/live_dbus.rs` | modify | Comments only (P6-D1): one "GTK" mention in the header doc. |
| `docs/superpowers/plans/2026-09-03-m5-part0-contract.md` | modify | §6: the M5-D1…M5-D11 landed table, the six §4.3 milestone records, and P6-D1…P6-D6. |
| `docs/superpowers/specs/2026-09-03-pure-rust-gtk-m5-app-migrations-design.md` | modify | Status line. |
| `docs/superpowers/specs/2026-08-20-pure-rust-gtk-ui-design.md` | modify | Status line and decomposition item 5. |
| `ui/README.md` | modify | The "What this crate covers" M5 sentence and the gate list; P0 already wrote the API sections (M5-D11). |
| `docs/superpowers/plans/2026-09-03-m5-pr-body.md` | create | The PR body, including the IME regression in plain words. Written, not pushed. |

---

## Task 1: the settings dependency audit, and the stale-GTK doc-comment sweep

**Files:**
- Create: `settings/tests/dependency_audit.rs`
- Modify: `settings/src/model.rs` (comments only)
- Modify: `settings/src/outputs/mod.rs` (comments only)
- Modify: `settings/src/outputs/protocol.rs` (comments only)

**Interfaces:**

- Consumes: the migrated `icedtea-settings` crate as P1–P4 left it — in
  particular `settings/Cargo.toml` with `gtk4`, `gio` and `glib` removed
  (contract §2.1) and `settings/src/outputs/client.rs` deleted.
- Produces (test-only; nothing is `pub`, nothing is imported by another crate):

```rust
// settings/tests/dependency_audit.rs
const GTK_STACK_PACKAGES: &[&str];        // the 24 lockfile package names
const GTK_STACK_PREFIXES: &[&str];        // the 9 name prefixes
const GTK_TOKENS: &[&str];                // the source/manifest tokens
const DEFERRED_WORK_TOKENS: &[&str];      // todo!( / unimplemented!( / TODO / FIXME
const SELF_EXCLUDED: &[&str];             // the two audit file names

fn crate_dir() -> std::path::PathBuf;
fn workspace_dir() -> std::path::PathBuf;
fn rust_sources(root: &std::path::Path) -> Vec<std::path::PathBuf>;
fn offending_lines(paths: &[std::path::PathBuf], tokens: &[&str]) -> Vec<String>;
fn locked_package_names(lock: &str) -> Vec<&str>;

#[test] fn the_settings_manifest_declares_no_gtk_dependency();
#[test] fn no_settings_source_mentions_a_gtk_name();
#[test] fn no_settings_source_defers_work();
#[test] fn the_workspace_lockfile_has_no_gtk_stack_package();
#[test] fn no_new_gtk_stack_package_slips_into_the_lockfile();
```

- [ ] **Step 1: Write the failing test**

Create `settings/tests/dependency_audit.rs`:

```rust
//! M5 close-out: the dependency audit for `icedtea-settings` (contract §4.1),
//! plus the two workspace-scope lockfile assertions (contract deviation
//! P6-D3 — a workspace-scope test needs a crate to live in, and every crate
//! outside `settings/`, `shell/` and `ui/` is out of M5 scope).
//!
//! These tests are hermetic on purpose (P6-D2): they read `Cargo.toml`, the
//! workspace `Cargo.lock` and this crate's own `.rs` files, and never invoke
//! `cargo`. A nested `cargo` under `cargo test` contends for the build lock.
//! §4.1's `cargo tree` invert queries are still run once by hand at close-out
//! as independent corroboration; their output is in this commit's message.
//!
//! Mutation check for the whole file: add `gtk4 = "0.11"` back to
//! `settings/Cargo.toml` and run `cargo build -p icedtea-settings` to relock;
//! `the_settings_manifest_declares_no_gtk_dependency` and
//! `the_workspace_lockfile_has_no_gtk_stack_package` must both fail. Restore
//! the manifest and relock.

use std::path::{Path, PathBuf};

/// Every GTK-stack package the workspace `Cargo.lock` carried on `develop`
/// @ `d9cee52`, measured with
/// `awk -F'"' '/^name = /{print $2}' Cargo.lock` filtered by the prefixes
/// below plus `field-offset`. `settings/` and `shell/` were their only
/// holders, so a complete M5 removes all twenty-four.
const GTK_STACK_PACKAGES: &[&str] = &[
    "cairo-rs",
    "cairo-sys-rs",
    "field-offset",
    "gdk-pixbuf",
    "gdk-pixbuf-sys",
    "gdk4",
    "gdk4-sys",
    "gio",
    "gio-sys",
    "glib",
    "glib-macros",
    "glib-sys",
    "gobject-sys",
    "graphene-rs",
    "graphene-sys",
    "gsk4",
    "gsk4-sys",
    "gtk4",
    "gtk4-layer-shell",
    "gtk4-layer-shell-sys",
    "gtk4-macros",
    "gtk4-sys",
    "pango",
    "pango-sys",
];

/// Name prefixes no crate in this workspace may lock. Twenty-three of the
/// twenty-four packages above match one of these; nothing else in the
/// lockfile does, so there is no false positive to exempt. This catches a
/// *new* GTK-stack package (a `gtk4` minor bump adding `gdk4-wayland`, say)
/// that the explicit list above could not know about.
const GTK_STACK_PREFIXES: &[&str] = &[
    "gtk", "gdk", "gsk", "glib", "gio", "gobject", "pango", "cairo", "graphene",
];

/// Case-sensitive tokens that cannot appear innocently in this crate's
/// manifest or sources. Lower-case on purpose: prose that says "GTK-free" or
/// "the GTK app this replaced" is fine and stays; `gdk::ModifierType` in a
/// doc comment is not, because it documents a type this crate can no longer
/// name. `gio` is matched only in its import shapes, because "gio" is a
/// substring of ordinary words such as "region".
const GTK_TOKENS: &[&str] = &[
    "gtk4",
    "glib",
    "gdk",
    "pango",
    "cairo",
    "gobject",
    "gio::",
    "use gio",
    "gio =",
    "GtkBox",
    "ApplicationWindow",
    "CssProvider",
];

/// M5 defers nothing (global rule: no TODOs, no stubs, no "later" notes).
const DEFERRED_WORK_TOKENS: &[&str] = &["todo!(", "unimplemented!(", "TODO", "FIXME"];

/// The audit files quote the very tokens they forbid, so the walker skips
/// them by file name. Nothing else is skipped.
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

/// Every `.rs` file under `root`, depth-first, sorted, minus the audit files.
/// A `root` that does not exist yields an empty vector rather than a panic:
/// `settings/tests/support/` may or may not exist depending on what P1–P4
/// needed.
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

/// `"<path>:<line number>: <trimmed line>"` for every line of every file that
/// contains any of `tokens`.
fn offending_lines(paths: &[PathBuf], tokens: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    for path in paths {
        let text = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        for (i, line) in text.lines().enumerate() {
            if tokens.iter().any(|t| line.contains(t)) {
                out.push(format!("{}:{}: {}", path.display(), i + 1, line.trim()));
            }
        }
    }
    out
}

/// The `name = "…"` values of a `Cargo.lock`, in file order.
fn locked_package_names(lock: &str) -> Vec<&str> {
    lock.lines()
        .filter_map(|line| line.strip_prefix("name = "))
        .map(|value| value.trim().trim_matches('"'))
        .collect()
}

/// Mutation check: add `gtk4 = { version = "0.11" }` to `[dependencies]`;
/// this must fail naming that line. Restore.
#[test]
fn the_settings_manifest_declares_no_gtk_dependency() {
    let path = crate_dir().join("Cargo.toml");
    let manifest = std::fs::read_to_string(&path).expect("read settings/Cargo.toml");
    let mut offenders = Vec::new();
    for (i, line) in manifest.lines().enumerate() {
        // Strip TOML comments: the manifest may legitimately explain in prose
        // which GTK crate a dependency replaced.
        let code = line.split('#').next().unwrap_or("");
        if GTK_TOKENS.iter().any(|t| code.contains(t)) {
            offenders.push(format!("Cargo.toml:{}: {}", i + 1, line.trim()));
        }
    }
    assert!(
        offenders.is_empty(),
        "settings/Cargo.toml still declares the GTK stack: {offenders:#?}"
    );
}

/// Mutation check: re-add `//! converts `gdk::ModifierType`` to
/// `settings/src/model.rs`; this must fail naming that line. Restore.
#[test]
fn no_settings_source_mentions_a_gtk_name() {
    let mut paths = rust_sources(&crate_dir().join("src"));
    paths.extend(rust_sources(&crate_dir().join("tests")));
    assert!(
        paths.len() > 5,
        "the source walker found only {} files — it is looking in the wrong place",
        paths.len()
    );
    let offenders = offending_lines(&paths, GTK_TOKENS);
    assert!(
        offenders.is_empty(),
        "settings/ still names the GTK stack (code or doc comment): {offenders:#?}"
    );
}

/// Mutation check: add `// TODO: wire this up` to `settings/src/app.rs`;
/// this must fail naming that line. Restore.
#[test]
fn no_settings_source_defers_work() {
    let mut paths = rust_sources(&crate_dir().join("src"));
    paths.extend(rust_sources(&crate_dir().join("tests")));
    let offenders = offending_lines(&paths, DEFERRED_WORK_TOKENS);
    assert!(
        offenders.is_empty(),
        "settings/ defers work at the M5 close-out: {offenders:#?}"
    );
}

/// Workspace scope (P6-D3).
///
/// Mutation check: add `gtk4 = "0.11"` to `settings/Cargo.toml`, run
/// `cargo build -p icedtea-settings` to relock, and this must fail listing
/// every GTK package the relock pulled back in. Restore and relock.
#[test]
fn the_workspace_lockfile_has_no_gtk_stack_package() {
    let lock = std::fs::read_to_string(workspace_dir().join("Cargo.lock")).expect("read Cargo.lock");
    let names = locked_package_names(&lock);
    assert!(
        names.len() > 100,
        "the lockfile parse found only {} packages — the `name = \"…\"` format changed \
         and this test would pass vacuously",
        names.len()
    );
    let found: Vec<&str> = GTK_STACK_PACKAGES
        .iter()
        .copied()
        .filter(|p| names.contains(p))
        .collect();
    assert!(
        found.is_empty(),
        "Cargo.lock still locks the GTK stack: {found:?}"
    );
}

/// Workspace scope (P6-D3). Catches a GTK-stack package the explicit list
/// above could not know about.
///
/// Mutation check: add `"gtk-fake"` to `GTK_STACK_PREFIXES`' input by
/// inserting a `name = "gtk-fake"` line into a scratch copy of the lockfile
/// string; the assertion must fire. (Do not edit `Cargo.lock` itself.)
#[test]
fn no_new_gtk_stack_package_slips_into_the_lockfile() {
    let lock = std::fs::read_to_string(workspace_dir().join("Cargo.lock")).expect("read Cargo.lock");
    let found: Vec<&str> = locked_package_names(&lock)
        .into_iter()
        .filter(|name| {
            GTK_STACK_PREFIXES
                .iter()
                .any(|p| *name == *p || name.starts_with(&format!("{p}-")))
        })
        .collect();
    assert!(
        found.is_empty(),
        "a GTK-stack package is locked by the workspace: {found:?}"
    );
}
```

- [ ] **Step 2: Run the test to see it fail**

```bash
cargo test -p icedtea-settings --test dependency_audit
```

Expected: four tests pass and **`no_settings_source_mentions_a_gtk_name`
fails**, naming the doc-comment lines that survived P1–P4. On a tree where P1
left `outputs/mod.rs`'s module doc otherwise intact, that is six lines:

```
---- no_settings_source_mentions_a_gtk_name stdout ----
thread 'no_settings_source_mentions_a_gtk_name' panicked at settings/tests/dependency_audit.rs:
settings/ still names the GTK stack (code or doc comment): [
    ".../settings/src/model.rs:4: //! Nothing here touches GTK -- the (GTK) view converts `gdk::ModifierType`",
    ".../settings/src/model.rs:80: /// GTK-free stand-in for `gdk::ModifierType` -- the (GTK) capture widget",
    ".../settings/src/model.rs:262: /// which drives a real `gdk::Display` against a harness compositor.",
    ".../settings/src/outputs/mod.rs:9: //!   enumerate/apply cycle is testable against `icedtea-harness` with no GTK,",
    ".../settings/src/outputs/protocol.rs:6: //! logic — no `gtk4`, `gio`, or `glib` — so the whole enumerate/apply cycle is",
    ".../settings/src/outputs/protocol.rs:8: //! explicit roundtrips. The glib main-loop integration lives in",
]
```

The exact list depends on how much of `outputs/mod.rs`'s module doc P1 already
rewrote when it deleted `client.rs`. **Read the list from the failure**, not
from this plan, and fix every entry it names. If the failure also names a
`.rs` file under `settings/src/pages/` or `settings/src/app.rs`, that is **not**
a comment: stop, and route it to the owning part (P1–P4) as a missed migration.

- [ ] **Step 3: Implement the sweep**

Comment lines only. After this step, `git diff --stat` for the three files must
show only lines beginning with `//!`, `///` or `//` (Step 4 checks it).

In `settings/src/model.rs`, replace the module doc's third paragraph:

```rust
//! GTK-free core of the settings app: working-copy state, the compositor's
//! `apply` path, keybinding-capture translation, and validation helpers.
//!
//! Nothing here touches a toolkit -- the view converts a [`KeyEvent`]'s
//! modifier state into [`CaptureMods`] and calls into this module, so the
//! logic here can be unit-tested without a display server.
//!
//! [`KeyEvent`]: icedtea_ui::window::keyboard::KeyEvent
```

replace `CaptureMods`' doc:

```rust
/// Toolkit-free modifier set -- the capture handler converts the modifier
/// state of the key-press event it just captured into this before handing off
/// to [`combo_from_keysym`].
```

and replace the two lines inside `combo_round_trips_to_the_compositor_format`'s
doc comment that name GDK:

```rust
    /// keymap, which is exactly why this crate has no toolkit dependency (see
    /// the module doc). The actual non-vacuous proof that a Shift-held
    /// capture still resolves to the unshifted key lives in
    /// `settings/tests/keybindings.rs::shift_a_while_capturing_records_base_a_with_shift`,
    /// which drives a real `VirtualKeyboardClient` against a harness
    /// compositor.
```

In `settings/src/outputs/mod.rs`, the module doc's two bullets become one that
describes what actually ships (the second bullet's `client` module was deleted
in P1; if P1 already replaced it with a `pump` bullet, keep P1's text and only
remove the words `gio` and `glib` from the first bullet):

```rust
//! `zwlr_output_management_v1` client for the Displays page.
//!
//! Two layers:
//!
//! * [`protocol`] — the core owning a *second* `wayland-client` connection and
//!   the full `Dispatch` tree for the output-management protocol. Plain-data
//!   [`Head`]/[`Mode`] snapshots out, [`HeadEdit`] configurations in. Drivable
//!   with explicit roundtrips, so the whole enumerate/apply cycle is testable
//!   against `icedtea-harness` with no toolkit in sight.
//! * [`pump`] — the [`OutputsPump`] handle the Displays page holds. It
//!   registers the connection's queue fd with the window's poll set
//!   (`Window::watch_fd`) and drains it from `App::on_fd`, so the queue is
//!   pumped from the app's own loop without a dispatch thread and without ever
//!   blocking it.
```

In `settings/src/outputs/protocol.rs`, the module doc:

```rust
//! `zwlr_output_management_v1` client core.
//!
//! Owns a second `wayland-client` [`Connection`] (separate from the toolkit's)
//! and the [`EventQueue`] that drives it, plus the `Dispatch` glue for every
//! object in the output-management tree. Everything here is plain data and
//! pure protocol logic, so the whole enumerate/apply cycle is exercisable from
//! a libtest binary against `icedtea-harness` with nothing but explicit
//! roundtrips. The event-loop integration lives in [`super::pump`], layered on
//! top of this.
```

If P1's `outputs/mod.rs` names its module something other than `pump`, use the
name that shipped — the doc must match the tree, and `cargo doc` with
`-D warnings` (Task 5) fails on a broken intra-doc link either way.

- [ ] **Step 4: Run the test to verify it passes, and prove the sweep is
      comment-only**

```bash
cargo test -p icedtea-settings --test dependency_audit
git diff -U0 -- settings/src/model.rs settings/src/outputs/mod.rs settings/src/outputs/protocol.rs \
  | grep -E '^[+-]' | grep -vE '^(\+\+\+|---)' | grep -vE '^[+-]\s*(//!|///|//)' || echo "comment-only"
```

Expected: `test result: ok. 5 passed`, and `comment-only` from the second
command (no non-comment line changed on either side of the diff).

- [ ] **Step 5: Corroborate with §4.1's `cargo tree` queries (P6-D2, P6-D4)**

```bash
cargo tree -p icedtea-settings -e normal | head -40
for pkg in gtk4 gtk4-layer-shell glib gio gdk4 pango cairo-rs gobject-sys glib-sys gtk4-sys; do
  if cargo tree -p icedtea-settings -e normal -i "$pkg" >/dev/null 2>&1; then
    echo "FAIL: $pkg is in icedtea-settings' normal tree"
  else
    echo "absent: $pkg"
  fi
done
```

Expected: the first command shows `icedtea-config`, `icedtea-contract`,
`icedtea-ui`, `wayland-client`, `wayland-protocols-wlr`, `rustix`,
`async-channel`, `crossbeam-channel`, `zbus`, `redb`, `tracing`,
`tracing-subscriber` and no GTK name; the loop prints `absent:` ten times and
no `FAIL:` line. Each absent package makes `cargo tree -i` exit `101` with
`error: package ID specification \`<pkg>\` did not match any packages` — that
non-zero exit is the assertion (P6-D4). Paste the loop's output into the commit
message.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
cargo clippy -p icedtea-settings --all-targets -- -D warnings
git add settings/tests/dependency_audit.rs settings/src/model.rs \
        settings/src/outputs/mod.rs settings/src/outputs/protocol.rs
git commit -m "test(settings): audit the GTK stack out of icedtea-settings

Five hermetic assertions -- manifest, sources, deferred work, and the two
workspace-scope Cargo.lock checks -- replace contract §4.1's run-once shell
commands (P6-D2, P6-D3). The source scan flagged six stale doc comments in
model.rs, outputs/mod.rs and outputs/protocol.rs that still described the GTK
view and the gio::Socket glib source; those are rewritten here, comments only
(P6-D1).

cargo tree -p icedtea-settings -e normal -i <pkg> corroborates: absent for
gtk4, gtk4-layer-shell, glib, gio, gdk4, pango, cairo-rs, gobject-sys,
glib-sys and gtk4-sys.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

## Task 2: the shell dependency audit and the stale-prose sweep

**Files:**
- Create: `shell/tests/dependency_audit.rs`
- Modify: `shell/src/compositor_client.rs` (comments only)
- Modify: `shell/src/clip_client.rs` (comments only)
- Modify: `shell/tests/live_dbus.rs` (comments only)
- Modify: `shell/src/taskbar.rs` (comments only, if P5 left the line)

**Interfaces:**

- Consumes: the migrated `icedtea-shell` crate as P5 left it — `gtk4` and
  `gtk4-layer-shell` removed from `shell/Cargo.toml`, `shell/src/bridge.rs`
  deleted, `pub use gtk4;` gone from `shell/src/lib.rs` (contract §3.5).
- Produces (test-only):

```rust
// shell/tests/dependency_audit.rs
const GTK_TOKENS: &[&str];              // same twelve tokens as settings'
const STALE_GTK_PROSE: &[&str];         // "GTK thread", "GTK side", "GTK running",
                                        // "the render half", "GTK main loop"
const DEFERRED_WORK_TOKENS: &[&str];
const SELF_EXCLUDED: &[&str];

fn crate_dir() -> std::path::PathBuf;
fn rust_sources(root: &std::path::Path) -> Vec<std::path::PathBuf>;
fn offending_lines(paths: &[std::path::PathBuf], tokens: &[&str]) -> Vec<String>;

#[test] fn the_shell_manifest_declares_no_gtk_dependency();
#[test] fn no_shell_source_mentions_a_gtk_name();
#[test] fn no_shell_doc_comment_still_describes_a_gtk_process();
#[test] fn no_shell_source_defers_work();
```

The lockfile tests are **not** repeated here: they are workspace-scope and live
once, in settings' audit (P6-D3). This file is crate-scope only, which is why
it is not a copy of Task 1's.

- [ ] **Step 1: Write the failing test**

Create `shell/tests/dependency_audit.rs`:

```rust
//! M5 close-out: the dependency audit for `icedtea-shell` (contract §4.1).
//!
//! Crate scope only. The workspace-wide `Cargo.lock` assertions live once, in
//! `settings/tests/dependency_audit.rs` (contract deviation P6-D3).
//!
//! Hermetic (P6-D2): no `cargo` is invoked. §4.1's `cargo tree` invert queries
//! are run once by hand at close-out; their output is in this commit's
//! message.

use std::path::{Path, PathBuf};

/// Case-sensitive tokens that cannot appear innocently in this crate's
/// manifest or sources. Identical to settings' list, deliberately: two crates
/// audited by the same rule, each in its own file, beats a shared helper
/// module that neither crate owns.
const GTK_TOKENS: &[&str] = &[
    "gtk4",
    "glib",
    "gdk",
    "pango",
    "cairo",
    "gobject",
    "gio::",
    "use gio",
    "gio =",
    "GtkBox",
    "ApplicationWindow",
    "CssProvider",
];

/// Upper-case prose that describes a process shape this crate no longer has.
/// `compositor_client.rs` and `clip_client.rs` keep their worker bodies
/// verbatim (contract §3.5, P5 must-not-touch), so their doc comments still
/// say the worker "pushes to the GTK thread" -- there is no GTK thread any
/// more, there is the app's loop thread and its inbox. Prose that merely
/// mentions GTK historically ("the GTK app this replaced") is fine and is not
/// listed here.
const STALE_GTK_PROSE: &[&str] = &[
    "GTK thread",
    "GTK side",
    "GTK running",
    "GTK main loop",
    "the render half",
];

/// M5 defers nothing.
const DEFERRED_WORK_TOKENS: &[&str] = &["todo!(", "unimplemented!(", "TODO", "FIXME"];

/// The audit files quote the tokens they forbid.
const SELF_EXCLUDED: &[&str] = &["dependency_audit.rs", "deletion_ledger.rs"];

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Every `.rs` file under `root`, depth-first, sorted, minus the audit files.
/// A missing `root` yields an empty vector rather than a panic.
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

fn offending_lines(paths: &[PathBuf], tokens: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    for path in paths {
        let text = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        for (i, line) in text.lines().enumerate() {
            if tokens.iter().any(|t| line.contains(t)) {
                out.push(format!("{}:{}: {}", path.display(), i + 1, line.trim()));
            }
        }
    }
    out
}

fn shell_sources() -> Vec<PathBuf> {
    let mut paths = rust_sources(&crate_dir().join("src"));
    paths.extend(rust_sources(&crate_dir().join("tests")));
    assert!(
        paths.len() > 5,
        "the source walker found only {} files -- it is looking in the wrong place",
        paths.len()
    );
    paths
}

/// Mutation check: add `gtk4-layer-shell = "0.8"` to `[dependencies]`; this
/// must fail naming that line. Restore.
#[test]
fn the_shell_manifest_declares_no_gtk_dependency() {
    let manifest =
        std::fs::read_to_string(crate_dir().join("Cargo.toml")).expect("read shell/Cargo.toml");
    let mut offenders = Vec::new();
    for (i, line) in manifest.lines().enumerate() {
        let code = line.split('#').next().unwrap_or("");
        if GTK_TOKENS.iter().any(|t| code.contains(t)) {
            offenders.push(format!("Cargo.toml:{}: {}", i + 1, line.trim()));
        }
    }
    assert!(
        offenders.is_empty(),
        "shell/Cargo.toml still declares the GTK stack: {offenders:#?}"
    );
}

/// Mutation check: re-add `pub use gtk4;` to `shell/src/lib.rs`; this must
/// fail naming that line. Restore.
#[test]
fn no_shell_source_mentions_a_gtk_name() {
    let offenders = offending_lines(&shell_sources(), GTK_TOKENS);
    assert!(
        offenders.is_empty(),
        "shell/ still names the GTK stack (code or doc comment): {offenders:#?}"
    );
}

/// Mutation check: restore `//! `history_changed` and pushes it to the GTK
/// thread` at the top of `clip_client.rs`; this must fail naming that line.
/// Restore.
#[test]
fn no_shell_doc_comment_still_describes_a_gtk_process() {
    let offenders = offending_lines(&shell_sources(), STALE_GTK_PROSE);
    assert!(
        offenders.is_empty(),
        "shell/ documents a process shape it no longer has: {offenders:#?}"
    );
}

/// Mutation check: add `// TODO: reconnect` to `shell/src/panel.rs`; this must
/// fail naming that line. Restore.
#[test]
fn no_shell_source_defers_work() {
    let offenders = offending_lines(&shell_sources(), DEFERRED_WORK_TOKENS);
    assert!(
        offenders.is_empty(),
        "shell/ defers work at the M5 close-out: {offenders:#?}"
    );
}
```

- [ ] **Step 2: Run the test to see it fail**

```bash
cargo test -p icedtea-shell --test dependency_audit
```

Expected: three tests pass and **`no_shell_doc_comment_still_describes_a_gtk_process`
fails** with eight entries — four in `compositor_client.rs` (its module doc's
"pushes it to the GTK thread", the `spawn` doc's "leaves GTK running with a
permanently empty, permanently stale taskbar", the `break; // GTK side gone.`
comment and `CompositorProxy`'s "Issues … commands from the GTK thread"), three
in `clip_client.rs` (the same three shapes) and one in `shell/tests/live_dbus.rs`
("the coverage the taskbar/popover GTK …"). If `shell/src/taskbar.rs:2` still
says "the render half (Task 9)", that is a ninth.

If `no_shell_source_mentions_a_gtk_name` also fails, that is **not** a comment
problem: stop and route it to P5 as a missed migration.

- [ ] **Step 3: Implement the sweep**

Comment lines only, in the same four files. Replace each stale phrase with what
actually happens now — the worker thread pushes onto the app's inbox, and the
loop thread drains it:

In `shell/src/compositor_client.rs`:

```rust
//! panel's loop thread through its inbox; [`CompositorProxy`] issues commands
```

for the module doc's second line;

```rust
/// thread end leaves the panel running with a permanently empty, permanently
```

for `spawn`'s doc;

```rust
            break; // The panel exited.
```

for the `break` comment; and

```rust
/// Issues `org.icedtea.Compositor` commands from the panel's loop thread.
/// Method calls are
```

for `CompositorProxy`'s doc.

In `shell/src/clip_client.rs`, the same three shapes:

```rust
//! `history_changed` and pushes it onto the panel's inbox; [`ClipProxy`] issues
```

```rust
            break; // The panel exited.
```

```rust
/// Issues `org.icedtea.Clipboard` commands from the panel's loop thread.
```

In `shell/tests/live_dbus.rs`, the header doc:

```rust
//! `icedtea-clipboard` service. This is the coverage the taskbar/popover
//! panel tests cannot give: they drive mocks, this drives the real bus.
```

In `shell/src/taskbar.rs`, if the line survived P5:

```rust
//! folded from `org.icedtea.Compositor` traffic. No toolkit types here -- the
//! view half lives in `panel.rs`.
```

**Do not touch anything but comments in `compositor_client.rs` and
`clip_client.rs`** — contract §5 gives P5 their worker and proxy bodies, and
§3.5's `process::exit(1)` rule is asserted by P5's
`the_fatal_worker_path_still_exits`, which Task 4 checks is present.

- [ ] **Step 4: Run the test to verify it passes, and prove the sweep is
      comment-only**

```bash
cargo test -p icedtea-shell --test dependency_audit
git diff -U0 -- shell/src/compositor_client.rs shell/src/clip_client.rs \
                shell/tests/live_dbus.rs shell/src/taskbar.rs \
  | grep -E '^[+-]' | grep -vE '^(\+\+\+|---)' | grep -vE '^[+-]\s*(//!|///|//)' || echo "comment-only"
cargo test -p icedtea-shell --test live_dbus
```

Expected: `test result: ok. 4 passed`; `comment-only`; and `live_dbus` still
green (1 passed) — proof the comment sweep did not disturb the real-bus test.

- [ ] **Step 5: Corroborate with §4.1's `cargo tree` queries and the
      workspace-wide grep**

```bash
cargo tree -p icedtea-shell -e normal | head -30
for pkg in gtk4 gtk4-layer-shell gtk4-layer-shell-sys glib gio gdk4 pango cairo-rs gobject-sys gtk4-sys; do
  if cargo tree -p icedtea-shell -e normal -i "$pkg" >/dev/null 2>&1; then
    echo "FAIL: $pkg is in icedtea-shell' normal tree"
  else
    echo "absent: $pkg"
  fi
done
cargo tree -e normal --workspace | grep -Ei '\b(gtk4|gtk4-layer-shell|glib|gio|gdk|pango|cairo)\b' || echo "workspace clean"
```

Expected: the tree shows `icedtea-contract`, `icedtea-ui`, `zbus`,
`async-channel`, `futures-util`, `crossbeam-channel`, `tracing`,
`tracing-subscriber` and no GTK name; ten `absent:` lines and no `FAIL:`; and
`workspace clean`. Paste the loop's output and the `workspace clean` line into
the commit message.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
cargo clippy -p icedtea-shell --all-targets -- -D warnings
git add shell/tests/dependency_audit.rs shell/src/compositor_client.rs \
        shell/src/clip_client.rs shell/tests/live_dbus.rs shell/src/taskbar.rs
git commit -m "test(shell): audit the GTK stack out of icedtea-shell

Four hermetic assertions -- manifest, sources, stale GTK prose, deferred work.
The workspace-scope Cargo.lock checks stay in settings' audit (P6-D3), so this
file is crate-scope only.

The prose scan flagged eight doc comments in compositor_client.rs,
clip_client.rs and live_dbus.rs that still described a GTK main thread the
panel no longer has; rewritten here, comments only (P6-D1). The worker and
proxy bodies -- including the deliberate process::exit(1) on a post-connect
D-Bus error -- are untouched.

cargo tree -p icedtea-shell -e normal -i <pkg>: absent for all ten GTK
packages; cargo tree -e normal --workspace greps clean.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

## Task 3: the settings deletion ledger

**Files:**
- Create: `settings/tests/deletion_ledger.rs`

**Interfaces:**

- Consumes: contract §4.2's deleted-file and deleted-item lists; §2.1's moved
  test counts; §2.6/§2.7/§2.8's gate test names; P1's
  `the_first_toplevel_app_run_paints_and_navigates`.
- Produces (test-only):

```rust
// settings/tests/deletion_ledger.rs
const DELETED_WITH_REPLACEMENT: &[(&str, &str, &str)];  // (deleted path, replacement path, what proves it)
const DELETED_IDENTIFIERS: &[(&str, &str)];             // (exact pattern, where it used to live)
const MOVED_SUITES: &[(&str, usize)];                   // (path, exact #[test] count)
const REQUIRED_GATES: &[&str];                          // test fn names that must exist

fn crate_dir() -> std::path::PathBuf;
fn rust_sources(root: &std::path::Path) -> Vec<std::path::PathBuf>;
fn count_tests(path: &std::path::Path) -> usize;
fn all_settings_text() -> String;

#[test] fn every_deleted_settings_file_is_gone_and_its_replacement_exists();
#[test] fn every_deleted_settings_identifier_is_gone();
#[test] fn every_moved_settings_suite_kept_its_test_count();
#[test] fn every_settings_gate_the_contract_names_exists();
```

- [ ] **Step 1: Write the failing test**

Create `settings/tests/deletion_ledger.rs`:

```rust
//! M5 close-out: contract §4.2's settings half, as data.
//!
//! The milestone rule this part is judged on is "every deletion is paired with
//! the test that proves the replacement". The pairing is the
//! `DELETED_WITH_REPLACEMENT` table below, so it cannot drift from a
//! paragraph of prose (contract deviation P6-D6).
//!
//! Hermetic: paths and file contents only, no `cargo`, no compositor.

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
        "settings/tests/outputs_client.rs (kept verbatim) + displays_page_paints_every_probe_point_at_rest",
    ),
    (
        "settings/src/pages/displays.rs",
        "settings/src/pages/displays/state.rs",
        "the twelve moved tests in displays/state.rs + dragging_a_head_snaps_it_and_updates_the_model",
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
];

/// `(exact source pattern that must not appear anywhere in settings/, where
/// it used to live)`. Patterns are deliberately narrow: `Page` alone would
/// match `PageId` and `stack_page`, `Row` would match `list_box_row`.
const DELETED_IDENTIFIERS: &[(&str, &str)] = &[
    ("pub struct Ctx", "pages/mod.rs -- the shared model/window/on_dirty bundle"),
    ("pub struct Page {", "pages/mod.rs -- the root+refresh pair"),
    ("fn mark_dirty", "pages/mod.rs -- Ctx::mark_dirty"),
    ("populating", "pages/mod.rs -- the Rc<Cell<bool>> write-back guard"),
    ("fn unshifted_keysym", "pages/keybindings.rs -- replaced by normalise_keysym + KeyEvent::base"),
    ("fn install_conflict_css", "pages/keybindings.rs -- replaced by settings/style.css's .conflict rule"),
    ("fn sync_rows", "pages/keybindings.rs -- replaced by view(&model)"),
    ("fn reset_capture", "pages/keybindings.rs -- replaced by Msg::CaptureCancelled"),
    ("struct Row", "pages/workspaces.rs -- the per-row widget handle"),
    ("fn set_remove_sensitivity", "pages/workspaces.rs -- now .sensitive(names.len() > 1)"),
    ("pub fn build(", "every page -- replaced by pub fn view(&SettingsModel) -> View<Msg>"),
    ("keybindings_page_slot", "main.rs -- the forward-reference slot"),
];

/// `(path, its exact `#[test]` count)`. Contract §2.1: the pure cores move
/// **with their tests**, byte-for-byte. A move that drops a test is the
/// failure mode this catches; the counts were measured on `develop` @
/// `d9cee52` before the extraction.
const MOVED_SUITES: &[(&str, usize)] = &[
    ("src/pages/displays_canvas.rs", 8),
    ("src/pages/displays/state.rs", 12),
    ("src/pages/workspaces.rs", 6),
    ("src/pages/keybindings.rs", 6),
    ("src/pages/appearance.rs", 2),
    ("src/model.rs", 10),
    ("src/compositor_reload.rs", 1),
];

/// Every settings test the contract names as an M5 gate, by function name.
/// Contract §2.1 (P1), §2.6 (P2), §2.7 (P3), §2.8 (P4).
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
    "appearance_page_paints_every_probe_point_at_rest",
    "behavior_page_paints_every_probe_point_at_rest",
    // P3 -- Workspaces and Keybindings.
    "shift_a_while_capturing_records_base_a_with_shift",
    "a_lone_modifier_press_leaves_the_capture_armed",
    "escape_cancels_a_capture",
    "switching_pages_cancels_a_capture",
    "removing_a_workspace_prunes_its_bindings",
    "workspaces_page_paints_every_probe_point_at_rest",
    "keybindings_page_paints_every_probe_point_at_rest",
    // P4 -- Displays.
    "a_drag_that_hits_nothing_leaves_the_selection_alone",
    "a_drag_snaps_the_dragged_head_against_its_neighbour",
    "a_release_outside_the_canvas_ends_the_drag",
    "enabling_a_mode_less_head_gives_it_a_default_mode",
    "an_apply_reply_clears_in_flight_by_its_own_is_test_tag",
    "displays_page_paints_every_probe_point_at_rest",
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
        .map(|p| {
            std::fs::read_to_string(p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
        })
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
            problems.push(format!("{deleted} still exists (was to be replaced by {replacement})"));
        }
        if !root.join(replacement).exists() {
            problems.push(format!(
                "{deleted} was deleted but its replacement {replacement} does not exist \
                 (proof was to be: {proof})"
            ));
        }
    }
    assert!(problems.is_empty(), "the settings deletion ledger does not hold: {problems:#?}");
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
/// `settings/src/pages/displays/state.rs`; this must fail reporting 11 for 12.
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
```

- [ ] **Step 2: Run the test to see it fail**

Before the ledger can pass, it must be seen failing on a tree where a deletion
was not done. Prove it by restoring one deleted file:

```bash
git checkout develop -- settings/tests/appearance_gtk.rs
cargo test -p icedtea-settings --test deletion_ledger
```

Expected: three tests pass and **`every_deleted_settings_file_is_gone_and_its_replacement_exists`
fails** with

```
the settings deletion ledger does not hold: [
    "settings/tests/appearance_gtk.rs still exists (was to be replaced by docs/superpowers/plans/2026-09-03-m5-part0-contract.md)",
]
```

- [ ] **Step 3: Restore the deletion (there is no production code in this task)**

```bash
git rm -f settings/tests/appearance_gtk.rs
```

The deliverable here is the ledger, not a change to the crate. If any of the
four tests fails on the untouched tree, the failure belongs to the part that
owns the file: `outputs/client.rs`, `pages/displays.rs` and the moved counts to
**P1**; the P2/P3/P4 gate names to **P2/P3/P4**. Route it there; do not add the
missing test here, and do not weaken the table.

The one adjustment permitted in this task: if a gate shipped under a name that
differs from the contract's, **read the shipped name from the crate**, confirm
with `git log -S<name>` that the owning part renamed it deliberately, and
update `REQUIRED_GATES` in the same commit with a comment naming the part and
the commit. A missing gate is never resolved by deleting its row.

- [ ] **Step 4: Run the test to verify it passes**

```bash
cargo test -p icedtea-settings --test deletion_ledger
git status --porcelain settings/
```

Expected: `test result: ok. 4 passed`, and `git status` clean apart from the new
`settings/tests/deletion_ledger.rs`.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
cargo clippy -p icedtea-settings --all-targets -- -D warnings
git add settings/tests/deletion_ledger.rs
git commit -m "test(settings): pair every M5 deletion with the test that replaced it

Contract §4.2's settings half as four data tables (P6-D6): the two deleted
files and the two deleted test files with their replacements, twelve
GTK-era identifiers that must not survive, the seven moved suites with the
exact test counts they carried on develop (8/12/6/6/2/10/1), and the
twenty-six gate names P1-P4 owe.

appearance_gtk.rs is the single deletion with no replacement test: its
property is true by construction on an Elm loop, and the table pairs it with
the contract §6 record instead.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

## Task 4: the shell deletion ledger

**Files:**
- Create: `shell/tests/deletion_ledger.rs`

**Interfaces:**

- Consumes: contract §4.2's shell half, §3.6's kept-verbatim counts and
  re-expressed test names, §3.5's `process::exit(1)` rule.
- Produces (test-only):

```rust
// shell/tests/deletion_ledger.rs
const DELETED_WITH_REPLACEMENT: &[(&str, &str, &str)];
const DELETED_IDENTIFIERS: &[(&str, &str)];
const KEPT_SUITES: &[(&str, usize)];
const REQUIRED_GATES: &[&str];

fn crate_dir() -> std::path::PathBuf;
fn workspace_dir() -> std::path::PathBuf;
fn rust_sources(root: &std::path::Path) -> Vec<std::path::PathBuf>;
fn count_tests(path: &std::path::Path) -> usize;
fn all_shell_text() -> String;

#[test] fn every_deleted_shell_file_is_gone_and_its_replacement_exists();
#[test] fn every_deleted_shell_identifier_is_gone();
#[test] fn every_kept_shell_suite_kept_its_test_count();
#[test] fn every_shell_gate_the_contract_names_exists();
#[test] fn the_fatal_dbus_path_still_exits_the_process();
```

- [ ] **Step 1: Write the failing test**

Create `shell/tests/deletion_ledger.rs`:

```rust
//! M5 close-out: contract §4.2's shell half, as data (P6-D6).
//!
//! Hermetic: paths and file contents only.

use std::path::{Path, PathBuf};

/// `(deleted path, the path that replaced it, the test that proves the
/// replacement behaves)`.
const DELETED_WITH_REPLACEMENT: &[(&str, &str, &str)] = &[
    (
        "shell/src/bridge.rs",
        "shell/src/panel.rs",
        "a_window_opened_signal_through_the_inbox_adds_a_button (the inbox replaces glib::spawn_future_local)",
    ),
    (
        "shell/tests/shell_gtk.rs",
        "shell/tests/panel.rs",
        "a_panel_click_reaches_the_command_surface (same mocks, same (action, id) assertions)",
    ),
];

/// `(exact source pattern that must not appear anywhere in shell/, where it
/// used to live)`.
const DELETED_IDENTIFIERS: &[(&str, &str)] = &[
    ("pub use gtk4", "lib.rs -- the re-export shell_gtk.rs needed"),
    ("pub mod bridge", "lib.rs -- the deleted worker bridge module"),
    ("pub fn render", "taskbar.rs and clipboard.rs -- clear-and-rebuild, replaced by view()"),
    ("fn connect_activation", "clipboard.rs -- replaced by ListBox::on_item_activated"),
    ("GestureClick", "taskbar.rs -- replaced by on_click / on_pointer_up_with_button"),
    ("set_ellipsize", "clipboard.rs -- replaced by the label's CSS ellipsize"),
    ("taskbar_box", "main.rs -- the separate child box the rebuild needed"),
];

/// `(path, its exact `#[test]` count)`. Contract §3.6: these suites are kept
/// **verbatim**; a rewrite that drops one is the failure mode this catches.
/// Counts measured on `develop` @ `d9cee52`.
const KEPT_SUITES: &[(&str, usize)] = &[
    ("src/taskbar.rs", 5),
    ("src/clipboard.rs", 1),
    ("src/compositor_client.rs", 1),
    ("tests/live_dbus.rs", 1),
];

/// Every shell test the contract names as an M5 gate (§3.6), by function name.
const REQUIRED_GATES: &[&str] = &[
    "a_panel_click_reaches_the_command_surface",
    "middle_clicking_a_window_button_closes_it",
    "a_window_opened_signal_through_the_inbox_adds_a_button",
    "panel_paints_every_probe_point_at_rest",
    "the_clipboard_popover_opens_pastes_and_dismisses",
    "the_fatal_worker_path_still_exits",
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

fn count_tests(path: &Path) -> usize {
    std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
        .matches("#[test]")
        .count()
}

fn all_shell_text() -> String {
    let mut paths = rust_sources(&crate_dir().join("src"));
    paths.extend(rust_sources(&crate_dir().join("tests")));
    assert!(
        paths.len() > 5,
        "the source walker found only {} files -- it is looking in the wrong place",
        paths.len()
    );
    paths
        .iter()
        .map(|p| {
            std::fs::read_to_string(p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Mutation check: `git checkout develop -- shell/src/bridge.rs`; this must
/// fail naming that path. Delete it again.
#[test]
fn every_deleted_shell_file_is_gone_and_its_replacement_exists() {
    let root = workspace_dir();
    let mut problems = Vec::new();
    for (deleted, replacement, proof) in DELETED_WITH_REPLACEMENT {
        if root.join(deleted).exists() {
            problems.push(format!("{deleted} still exists (was to be replaced by {replacement})"));
        }
        if !root.join(replacement).exists() {
            problems.push(format!(
                "{deleted} was deleted but its replacement {replacement} does not exist \
                 (proof was to be: {proof})"
            ));
        }
    }
    assert!(problems.is_empty(), "the shell deletion ledger does not hold: {problems:#?}");
}

/// Mutation check: re-add `pub use gtk4;` to `shell/src/lib.rs`; this must
/// fail naming it. Restore.
#[test]
fn every_deleted_shell_identifier_is_gone() {
    let text = all_shell_text();
    let survivors: Vec<String> = DELETED_IDENTIFIERS
        .iter()
        .filter(|(pattern, _)| text.contains(pattern))
        .map(|(pattern, home)| format!("`{pattern}` ({home})"))
        .collect();
    assert!(
        survivors.is_empty(),
        "GTK-era shell items survived the migration: {survivors:#?}"
    );
}

/// Mutation check: delete one `#[test]` from `shell/src/taskbar.rs`; this must
/// fail reporting 4 for 5. Restore.
#[test]
fn every_kept_shell_suite_kept_its_test_count() {
    let mut problems = Vec::new();
    for (relative, expected) in KEPT_SUITES {
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
        "a kept-verbatim shell suite changed size: {problems:#?}"
    );
}

/// Mutation check: rename `panel_paints_every_probe_point_at_rest` in
/// `shell/tests/panel.rs`; this must fail naming it. Restore.
#[test]
fn every_shell_gate_the_contract_names_exists() {
    let text = all_shell_text();
    let missing: Vec<&str> = REQUIRED_GATES
        .iter()
        .copied()
        .filter(|name| !text.contains(&format!("fn {name}(")))
        .collect();
    assert!(
        missing.is_empty(),
        "the contract names these shell gates and they do not exist: {missing:#?}"
    );
}

/// Contract §3.5, verbatim and non-negotiable: a post-connect D-Bus error in
/// the compositor worker calls `std::process::exit(1)` so systemd's
/// `Restart=always` replaces the process with a matching build. Logging and
/// continuing would leave a permanently stale taskbar systemd never restarts.
/// P5 asserts the behaviour; this asserts nobody quietly softened the source.
///
/// Mutation check: replace the `std::process::exit(1)` in
/// `shell/src/compositor_client.rs` with `tracing::error!`; this must fail.
/// Restore.
#[test]
fn the_fatal_dbus_path_still_exits_the_process() {
    let source = std::fs::read_to_string(crate_dir().join("src/compositor_client.rs"))
        .expect("read shell/src/compositor_client.rs");
    assert!(
        source.contains("process::exit(1)"),
        "compositor_client.rs no longer exits on a fatal D-Bus error -- \
         contract §3.5 forbids softening this into a log line"
    );
    let unit = std::fs::read_to_string(workspace_dir().join("shell/systemd/icedtea-shell.service"))
        .expect("read the systemd unit");
    assert!(
        unit.contains("Restart=always"),
        "the systemd unit no longer restarts the shell, which is the other \
         half of the process::exit(1) contract"
    );
}
```

- [ ] **Step 2: Run the test to see it fail**

```bash
git checkout develop -- shell/src/bridge.rs
cargo test -p icedtea-shell --test deletion_ledger
```

Expected: four tests pass and **`every_deleted_shell_file_is_gone_and_its_replacement_exists`
fails** with

```
the shell deletion ledger does not hold: [
    "shell/src/bridge.rs still exists (was to be replaced by shell/src/panel.rs)",
]
```

- [ ] **Step 3: Restore the deletion (no production code in this task)**

```bash
git rm -f shell/src/bridge.rs
```

As in Task 3: a failure on the untouched tree belongs to **P5**, and is routed
there rather than fixed here. The one permitted adjustment is the same —
a gate that shipped under a different name is looked up in the crate, confirmed
with `git log -S<name>`, and the table row is updated with a comment naming the
commit.

If `the_fatal_dbus_path_still_exits_the_process` fails, that is a **contract
violation**, not a naming drift: stop the close-out and raise it.

- [ ] **Step 4: Run the test to verify it passes**

```bash
cargo test -p icedtea-shell --test deletion_ledger
git status --porcelain shell/
```

Expected: `test result: ok. 5 passed`; `git status` clean apart from the new
`shell/tests/deletion_ledger.rs`.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
cargo clippy -p icedtea-shell --all-targets -- -D warnings
git add shell/tests/deletion_ledger.rs
git commit -m "test(shell): pair every M5 deletion with the test that replaced it

Contract §4.2's shell half as data (P6-D6): bridge.rs and shell_gtk.rs with
their replacements, seven GTK-era identifiers that must not survive, the four
kept-verbatim suites with their exact counts (5/1/1/1), and the six gate names
P5 owes.

Plus a source-level guard on contract §3.5's non-negotiable rule: the
post-connect D-Bus failure path still calls process::exit(1), and the systemd
unit still says Restart=always.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

## Task 5: the full gate set, run once, on one tree

**Files:**
- No file changes. The deliverable is a recorded run.

**Interfaces:**
- Consumes: everything P0–P5 landed, plus Tasks 1–4's audits.
- Produces: the `GATE-RUN.md` block pasted into Task 6's contract §6 preamble
  and Task 8's PR body. Nothing is committed by this task except that record,
  which rides in Task 6's commit.

- [ ] **Step 1: Write the failing check**

There is no unit test for "every gate is green at once"; the check is the run
itself, and it must be seen failing first in a way that proves it is really
looking. Run it on a deliberately broken tree:

```bash
sed -i 's/pub const BTN_MIDDLE: u32 = 0x112;/pub const BTN_MIDDLE: u32 = 0x999;/' ui/src/window/pointer.rs
cargo test -p icedtea-shell --test panel 2>&1 | tail -5
git checkout -- ui/src/window/pointer.rs
```

Expected: `middle_clicking_a_window_button_closes_it` **fails** (the mock
records no `("close", 7)`), proving the shell gate is driven by the real button
code and not by a constant the test also owns. Restore the file before Step 2 —
`git status --porcelain` must be empty.

- [ ] **Step 2: Run the whole gate set and record what it says**

Run in this order, top to bottom, stopping at the first failure:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
cargo test --workspace
```

Then the named gates, individually, so their counts are on the record:

```bash
# M1 / M2
cargo test -p icedtea-ui --test themed_button_offscreen      # 4 passed
cargo test -p icedtea-ui --test adwaita_coverage             # 10 passed
cargo test -p icedtea-ui --test gtk4_property_reference      # 4 passed
cargo test -p icedtea-ui --test layer_shell_screencopy       # 3 passed
cargo test -p icedtea-ui --test transition_screencopy        # 1 passed
# M3
cargo test -p icedtea-ui --test window_events                # >= 7 passed (P0 added 2)
cargo test -p icedtea-ui --test counter_app                  # 4 passed
cargo test -p icedtea-ui --test node_trees                   # 55 passed
cargo test -p icedtea-ui --test widget_pixels                # >= 40 passed (P0 added 5)
cargo test -p icedtea-ui --test reconcile_props              # 3 passed
cargo test -p icedtea-ui --test gallery_gate                 # >= 8 passed, light/dark/hc
cargo test -p icedtea-ui --test interaction_gate             # >= 16 passed, 16/16 driven
# M5 new
cargo test -p icedtea-ui --test ingress
cargo test -p icedtea-settings
cargo test -p icedtea-shell
```

and the two suites the milestone must not have moved onto the harness
(inherited decision 11):

```bash
cargo test -p icedtea-settings --test live_apply     # spawns the real icedtea-compositor
cargo test -p icedtea-settings --test outputs_client
cargo test -p icedtea-shell --test live_dbus         # real icedtea-clipboard service
```

Expected: every command exits 0. Record, in a scratch `GATE-RUN.md` (not
committed on its own — Task 6 pastes it into contract §6's preamble and Task 8
into the PR body):

- the exact `test result:` line of each named gate;
- the `cargo test --workspace` totals per package;
- wall-clock time for `cargo test --workspace` (the screencopy gates cost
  minutes; the number belongs on the record so the next milestone can budget);
- the `cargo tree` corroboration lines from Tasks 1 and 2 Step 5.

**If anything fails, stop.** Contract §5: "a gate failure at close-out is fixed
in the owning part's fix wave, not in the close-out." Record the failure, name
the owning part from the contract's §5 file ownership, and hand it back. Do not
patch it here, and do not proceed to Task 6 with a red tree.

- [ ] **Step 3: Prove the M1/M2/M3 gate files were not edited to make this
      pass**

```bash
git diff --stat develop -- ui/tests/themed_button_offscreen.rs \
  ui/tests/adwaita_coverage.rs ui/tests/gtk4_property_reference.rs \
  ui/tests/layer_shell_screencopy.rs ui/tests/transition_screencopy.rs \
  ui/tests/node_trees.rs ui/tests/counter_app.rs ui/tests/reconcile_props.rs
```

Expected: **empty output**. Those eight are byte-identical to `develop`;
contract §5 gives P0 only `gallery_gate.rs`, `interaction_gate.rs`,
`widget_pixels.rs`, `window_events.rs` and `support/mod.rs` among the test
files, and `layer_shell_screencopy.rs` is explicitly must-not-touch. A non-empty
diff here is a finding, routed to the part that made it.

```bash
git diff --stat develop -- ui/tests/gallery_gate.rs ui/tests/interaction_gate.rs \
  ui/tests/widget_pixels.rs ui/tests/window_events.rs ui/tests/support/mod.rs
```

Expected: **non-empty**, and every hunk attributable to P0's M5-D1/D5/D7/D8/D9
work (or P4's single `drop_down.rs` regression test). Skim it; anything else is
a finding.

- [ ] **Step 4: Confirm `KNOWN_BLANK_AT_REST` is exactly the contracted six**

```bash
sed -n '/const KNOWN_BLANK_AT_REST/,/];/p' ui/tests/gallery_gate.rs
```

Expected exactly, per M5-D8:

```rust
const KNOWN_BLANK_AT_REST: &[&str] = &[
    // Zero-area allocation.
    "window_controls",
    "font_dialog",
    "popover_menu",
    "popover_menu_bar",
    "alert_dialog",
    // Real allocation, nothing drawn into it.
    "link_button",
];
```

If P0's generic layout fix shrank it further, that is welcome and is already a
§6 amendment of P0's — note the shipped list for Task 6 and Task 7's README
sentence, which must name whatever actually ships.

- [ ] **Step 5: No commit**

This task changes no file. Its output is the `GATE-RUN.md` scratch record that
Tasks 6 and 8 consume. Confirm the tree is clean before handing on:

```bash
git status --porcelain
```

Expected: empty (the scratch record lives outside the repo, e.g. in the
executor's scratchpad).

---

## Task 6: the M5 deviation record — contract §6

**Files:**
- Modify: `docs/superpowers/plans/2026-09-03-m5-part0-contract.md` (§6 only)

**Interfaces:**
- Consumes: contract §4.3's enumerated list; the eleven M5-D items from §1;
  every `P<n>-D<m>` amendment P1–P5 already appended during their fix waves;
  Task 5's `GATE-RUN.md`.
- Produces: no code. §6 stops being "*(Empty at freeze.)*".

- [ ] **Step 1: Write the failing check**

```bash
awk '/^## 6\. Amendments/,/^## 7\./' docs/superpowers/plans/2026-09-03-m5-part0-contract.md \
  | grep -c '^### ' || echo 0
grep -c 'Empty at freeze' docs/superpowers/plans/2026-09-03-m5-part0-contract.md
```

Expected before the edit: a count of §6 headings equal to however many
amendments P1–P5 appended (0 if none did), and `2` for the "Empty at freeze"
placeholders (§6 and §7 each carry one). After Step 3, the heading count is at
least `6 + <whatever P1–P5 added>` and the "Empty at freeze" count is `1`
(§7's, which the consistency checker owns, stays).

- [ ] **Step 2: Inventory what is already there**

```bash
awk '/^## 6\. Amendments/,/^## 7\./' docs/superpowers/plans/2026-09-03-m5-part0-contract.md
git log --oneline develop..HEAD -- ui/src ui/tests
git log --oneline develop..HEAD --format='%h %s' | tail -60
```

Read the existing `P0-D*` … `P5-D*` amendments; **do not renumber or reword
them**. Collect the commit hash of each P0 item from the `ui/` log — the landed
table in Step 3 names one commit per M5-D item.

- [ ] **Step 3: Write the record**

Replace §6's placeholder paragraph with the following, keeping any existing
`P0-D*` … `P5-D*` entries **below** the landed table and **above** the P6
entries, in part order. `P0-D1 … P0-D6` belong to that set — P0 appends them
itself while it runs, in the same numbering scheme as the app parts' — so do
not fold them into the M5-D landed table and do not renumber them.

````markdown
## 6. Amendments

*Frozen at the M5 close-out (P6). The eleven pre-declared M5-D items are recorded
as landed with their commits; each part's own discoveries (`P0-D*` … `P5-D*`)
follow in part order,
in the M3 §10 shape (**Carried out by / Added / Contract says / As shipped /
Ruling**); P6's six close-out deviations come last.*

### M5-D1 … M5-D11 — landed

| Item | What it added | Landed in |
|---|---|---|
| M5-D1 | `Window::watch_fd`/`unwatch`/`watches`, `WatchId`, `Interest`, `InputEvent::FdReady`, the N-fd `wait_bounded` | `<commit>` |
| M5-D2 | `Inbox`/`InboxSender`/`SendError`, `App::with_inbox`, `App::on_fd`, the drain order | `<commit>` |
| M5-D3 | `Cmd::Task(Rc<dyn Fn()>)`, executed in `drain` outside `update` | `<commit>` |
| M5-D4 | `App::new` takes `impl FnMut`/`impl Fn`; `fn` items still coerce | `<commit>` |
| M5-D5 | `EventKind::{PointerDown,PointerMotion,PointerUp}`, `Handler::PairButton`, six builders, `BTN_*` | `<commit>` |
| M5-D6 | `Prop::Draw` carries `&mut PaintCx` so a canvas can shape text | `<commit>` |
| M5-D7 | `Keymap::base_keysym`, `KeyEvent::base`, the normalisation rule | `<commit>` |
| M5-D8 | Rest paint for `ColorDialogButton`/`ColorDialog`/`CheckButton`/`Scrollbar`; `KNOWN_BLANK_AT_REST` 9 → 6 | `<commit>` |
| M5-D9 | `Window::probe_points`/`allocation`, `ProbePoint`, `$ICEDTEA_PROBE_REPORT` | `<commit>` |
| M5-D10 | `ui/Cargo.toml`: `rustix` gains `pipe`, `crossbeam-channel` added | `<commit>` |
| M5-D11 | `ui/README.md`'s five new sections and the `KNOWN_BLANK` correction | `<commit>` |

### P6-D1 — the close-out sweeps stale GTK doc comments out of must-not-touch files

**Carried out by:** P6 (`settings/src/model.rs`, `settings/src/outputs/mod.rs`,
`settings/src/outputs/protocol.rs`, `shell/src/compositor_client.rs`,
`shell/src/clip_client.rs`, `shell/tests/live_dbus.rs`, `shell/src/taskbar.rs`).
**Added:** 2026-09-03, at the close-out.

**Contract §4.1 says** the source grep over `settings/src settings/tests
shell/src shell/tests` "must also be empty", while §0/§2.1 list `model.rs`,
`outputs/{mod,protocol}.rs` as **unchanged** and §5 makes
`compositor_client.rs`/`clip_client.rs`'s bodies must-not-touch for P5. Both
cannot hold: those files describe GTK machinery in prose
(`gdk::ModifierType`, "a `gio::Socket` glib source", "pushes it to the GTK
thread") that no longer exists.

**As shipped:** P6 rewrites those comment lines and nothing else. The diff for
every one of the seven files contains only `//!`, `///` and `//` lines, checked
mechanically in Task 1 Step 4 and Task 2 Step 4.

**Ruling.** Stale documentation of a deleted dependency is exactly the residue a
close-out exists to remove, and a comment is not behaviour: no signature, no
expression and no test changed, and `live_dbus` was re-run green immediately
after. The must-not-touch rules protect behaviour, and they are intact.

### P6-D2 — the dependency audit ships as tests, not only as shell commands

**Carried out by:** P6 (`settings/tests/dependency_audit.rs`,
`shell/tests/dependency_audit.rs`). **Added:** 2026-09-03.

**Contract §4.1 states** the audit as `cargo tree` invocations and two `grep`s
run once.

**As shipped:** nine hermetic assertions across two files — manifest, sources,
stale prose, deferred work, and two workspace-scope `Cargo.lock` checks — that
never invoke `cargo`. §4.1's exact commands are still run once by hand and their
output is in the audit commits' messages.

**Ruling.** A command run once at close-out proves the tree on that day; a test
proves it on every commit after. Invoking `cargo` from inside `cargo test` was
rejected: it contends for the build lock and can hang the suite.

### P6-D3 — the workspace-scope lockfile assertions live in the settings crate

**Carried out by:** P6 (`settings/tests/dependency_audit.rs`).
**Added:** 2026-09-03.

**The contract has no home** for a workspace-scope test: `harness/`,
`contract/`, `config/`, `compositor/`, `clipboard/` and `notifications/` are all
out of M5 scope (contract header).

**As shipped:** `the_workspace_lockfile_has_no_gtk_stack_package` and
`no_new_gtk_stack_package_slips_into_the_lockfile` live in settings' audit file,
documented there as workspace-scope. Shell's audit is crate-scope only.

**Ruling.** One owner, arbitrary but recorded, beats two copies that can drift.
The lockfile lists twenty-four GTK-stack packages today and must list none;
settings was chosen because it held the larger GTK surface.

### P6-D4 — `cargo tree -i <absent>` fails; it does not print "package ID not found"

**Carried out by:** P6 (Task 1 Step 5, Task 2 Step 5). **Added:** 2026-09-03.

**Contract §4.1 says** the invert query "must print `package ID not found`".

**As shipped:** the corroboration asserts a **non-zero exit**. The real message,
measured on 2026-09-03, is exit `101` with ``error: package ID specification
`gtk4` did not match any packages``.

**Ruling.** Grepping for a string cargo never emits would have passed
vacuously — the failure mode a dependency audit exists to prevent.

### P6-D5 — `cargo doc` is run with `RUSTDOCFLAGS="-D warnings"`

**Carried out by:** P6 (Task 5). **Added:** 2026-09-03.

**Contract §4.4 writes** `cargo doc --workspace --no-deps  # -D warnings`.
`-D warnings` is not a `cargo doc` flag.

**As shipped:** `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps`,
the M3 P8 precedent.

### P6-D6 — §4.2's deletions get a ledger test each

**Carried out by:** P6 (`settings/tests/deletion_ledger.rs`,
`shell/tests/deletion_ledger.rs`). **Added:** 2026-09-03.

**Contract §4.2 lists** six deleted files and thirteen deleted-in-place items
with no proof mechanism, while the milestone rule is "every deletion is paired
with the test that proves the replacement".

**As shipped:** four data tables per crate — deleted-path→replacement→proof,
forbidden identifiers, moved/kept suite test counts, and required gate names.

**Ruling.** The pairing is data, so it cannot drift from prose. The one deletion
with no replacement test — `settings/tests/appearance_gtk.rs` — is paired
explicitly with P6-D7 below, so the table stays total rather than silently
short.

### P6-D7 — `appearance_gtk.rs`'s populate guard is not ported: it is true by construction

**Carried out by:** P2 (deletion), P6 (record). **Added:** 2026-09-03.

**`settings/tests/appearance_gtk.rs`'s
`populate_never_writes_a_widget_fallback_back_into_the_model`** guarded the GTK
`Ctx`/`Page`/`populating: Rc<Cell<bool>>` machinery
(`settings/src/pages/mod.rs:28-43`), which existed because a *programmatic*
widget write fires the same handler a user edit does.

**As shipped:** not ported. On an Elm loop there is no write-back at all —
`view` renders `model.working`, and a handler runs only from real input — so the
property holds by construction and there is nothing left to guard. `Ctx`,
`Page`, `Page::refresh`, `populating` and the `keybindings_page_slot`
forward-reference are deleted, and `settings/tests/deletion_ledger.rs`'s
`every_deleted_settings_identifier_is_gone` is what keeps them gone.

### P6-D8 — the IME regression: both apps' text entries lose input methods until M6

**Carried out by:** P6 (record); the whole milestone carries it.
**Added:** 2026-09-03.

**GTK4's `Entry` speaks `text-input-v3`** through GTK's IM context. The toolkit
does not: `text-input-v3` is M6 scope by policy (spec §2 decision 8, §11).

**As shipped:** settings' wallpaper path, workspace-name and keybinding fields,
and the clipboard popover's search field, accept direct key events only. Users
of an input method — CJK, Korean, any compose-heavy layout — lose composition in
these two apps at this merge and regain it at M6.

**Ruling.** Stated here **and in the PR body**, in plain words, rather than
discovered by a user. It is not softened by "most users are unaffected": it is a
regression, it is deliberate, and it is scheduled.

### P6-D9 — the `DropDown` fixed 240 px list is sized to content, capped

**Carried out by:** P4 (`ui/src/widgets/drop_down.rs`), P6 (record).
**Added:** 2026-09-03.

**M3 P7-D54 embeds** a `DropDown`'s list at a fixed 240 px
(`ui/src/widgets/drop_down.rs:350`), which clips inside settings' 420 px-tall
window with a switcher and two footers above and below.

**As shipped:** the retained list is `rows * DROP_DOWN_ROW_PX` clamped to
`DROP_DOWN_MAX_PX = 240` and scrolls beyond it, so a two-item list is 68 px.
`ui/tests/interaction_gate.rs::opening_a_drop_down_and_picking_an_item_updates_the_button`
stayed green unchanged, and
`settings/tests/displays.rs::a_drop_down_list_fits_inside_the_settings_window`
is the regression test.

**Ruling.** This is the one enumerated exception to "`ui/` belongs to P0"
(contract §7): the correct height is knowable only once a real settings page is
laid out at 420 px. It is not a precedent for further app-part edits under
`ui/`.

### P6-D10 — the M3 contract's `App::new` signature is superseded by M5-D4

**Carried out by:** P0, P6 (cross-reference). **Added:** 2026-09-03.

**M3 contract §4.7 (`2026-08-27-m3-part0-contract.md:1516-1521`) ships**
`App::new(model, update: fn(&mut M, Msg) -> Cmd<Msg>, view: fn(&M) ->
View<Msg>)`.

**As shipped:** M5-D4's `impl FnMut` / `impl Fn` widening. Both M5 apps need
captured state in `update` (worker senders; `Rc<dyn CompositorCommands>` so the
mocks can be swapped), and a bare `fn` item cannot capture. `fn` items still
coerce, so the gallery, `ui/tests/counter_app.rs` and every M3 call site compile
unchanged.

**Ruling.** Recorded here, and a one-line pointer is added to the M3 contract's
own §10 so a reader of the older document is not misled.

### P6-D11 — `auto_exclusive_zone_enable()` becomes a literal `exclusive_zone: 28`

**Carried out by:** P5, P6 (record). **Added:** 2026-09-03.

**`shell/src/main.rs:49-84` called** `window.auto_exclusive_zone_enable()`,
which asks gtk4-layer-shell to track the surface's own height.

**As shipped:** `LayerSpec::exclusive_zone` is a concrete `i32`, so the panel
passes `BAR_HEIGHT` (28) — the height `set_size_request(-1, 28)` forced anyway.

**Ruling.** The two are equivalent for a bar of fixed height, which this bar is.
A future variable-height bar needs a real `auto` mode in `LayerSpec`, and that
is toolkit work, not app work.

### P6-D12 — `async-channel` is retained in both app crates

**Carried out by:** P1, P5, P6 (record). **Added:** 2026-09-03.

**Decision 1 drops** `gtk4`/`gio`/`glib`/`gobject`/`pango`/`cairo`/`gdk` from a
migrated app.

**As shipped:** `async-channel` stays in both `settings/Cargo.toml` and
`shell/Cargo.toml`. It is not a GTK crate: `settings/src/outputs/protocol.rs`
keeps its `connect_to_env(tx: async_channel::Sender<OutputsMsg>)` signature
unchanged (spec §5.3), and shell's two D-Bus workers keep theirs (§3.5). The
inbox side uses `crossbeam-channel`.

**Ruling.** The rule bans the GNOME toolkit, not every channel crate the old
code happened to sit beside. Changing `protocol.rs`'s signature to remove a
dependency it does not have would have broken "reuse the pure core unchanged".
````

Fill every `<commit>` from Step 2's log. Then paste Task 5's `GATE-RUN.md`
`test result:` lines as a short "Gate run at close-out" block immediately after
the landed table.

- [ ] **Step 4: Run the check to verify it passes**

```bash
awk '/^## 6\. Amendments/,/^## 7\./' docs/superpowers/plans/2026-09-03-m5-part0-contract.md \
  | grep -c '^### '
grep -c 'Empty at freeze' docs/superpowers/plans/2026-09-03-m5-part0-contract.md
grep -c '<commit>' docs/superpowers/plans/2026-09-03-m5-part0-contract.md
```

Expected: at least `13` headings (the landed table's heading + twelve P6 entries,
plus every `P1-D*`…`P5-D*` already there); `1` remaining "Empty at freeze" (§7's);
and **`0`** unfilled `<commit>` placeholders.

- [ ] **Step 5: Add the M3 contract's one-line pointer (P6-D10)**

In `docs/superpowers/plans/2026-08-27-m3-part0-contract.md`, at the end of §10,
append:

```markdown
### P8-D76 — `App::new`'s signature is superseded by M5-D4

`§4.7`'s `App::new(model, update: fn(..), view: fn(..))` was widened to
`impl FnMut` / `impl Fn` by M5-D4
(`docs/superpowers/plans/2026-09-03-m5-part0-contract.md` §1). `fn` items still
coerce, so every M3 call site compiles unchanged; read that amendment before
this section's signature.
```

- [ ] **Step 6: Commit**

```bash
git add docs/superpowers/plans/2026-09-03-m5-part0-contract.md \
        docs/superpowers/plans/2026-08-27-m3-part0-contract.md
git commit -m "docs(m5): the close-out deviation record

Contract §6 stops being empty: the eleven P0 amendments marked landed with
their commits, the gate run's test-result lines, each part's own discoveries in
part order, and P6-D1..P6-D12 -- the six close-out deviations this part took
plus the six milestone records §4.3 enumerates (the populate guard true by
construction, the IME regression, the DropDown sizing, the App::new widening,
the literal exclusive zone, and async-channel's retention).

The M3 contract §10 gains a one-line pointer at App::new so a reader of the
older document is not misled.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

## Task 7: the status lines — two specs and `ui/README.md`

**Files:**
- Modify: `docs/superpowers/specs/2026-09-03-pure-rust-gtk-m5-app-migrations-design.md`
- Modify: `docs/superpowers/specs/2026-08-20-pure-rust-gtk-ui-design.md`
- Modify: `ui/README.md`

**Interfaces:**
- Consumes: Task 5's gate run; Task 6's §6; P0's README sections (M5-D11).
- Produces: no code.

- [ ] **Step 1: Write the failing check**

```bash
grep -q 'implemented on `rebuild/pure-rust-gtk-m5`' \
  docs/superpowers/specs/2026-09-03-pure-rust-gtk-m5-app-migrations-design.md \
  && echo "m5 status updated" || echo "m5 status NOT updated"
grep -q 'M5 implemented' docs/superpowers/specs/2026-08-20-pure-rust-gtk-ui-design.md \
  && echo "parent status updated" || echo "parent status NOT updated"
grep -q 'M5 rebuilds the two applications' ui/README.md \
  && echo "readme updated" || echo "readme NOT updated"
```

Expected: three `NOT updated` lines.

- [ ] **Step 2: Confirm the tree these lines will describe is green**

Task 5 must have passed. Re-confirm cheaply:

```bash
cargo test -p icedtea-settings --test dependency_audit --test deletion_ledger
cargo test -p icedtea-shell --test dependency_audit --test deletion_ledger
```

Expected: 9 passed in settings' two binaries, 9 in shell's.

- [ ] **Step 3: Write the documentation**

In `docs/superpowers/specs/2026-09-03-pure-rust-gtk-m5-app-migrations-design.md`,
replace the status line:

```markdown
**Status:** implemented on `rebuild/pure-rust-gtk-m5` (parts 0–6 of
`docs/superpowers/plans/2026-09-03-m5-part0-contract.md`); `gtk4`,
`gtk4-layer-shell`, `glib`, `gio`, `gdk4`, `pango` and `cairo` are gone from
the workspace lockfile; awaiting owner review before merge
```

In `docs/superpowers/specs/2026-08-20-pure-rust-gtk-ui-design.md`, replace the
status line:

```markdown
**Status:** M1 and M2 implemented and merged; M3 implemented on
`rebuild/pure-rust-gtk-m3` (widget toolkit, popups, windows, icons — the parent
plan's M4 folded in); M5 implemented on `rebuild/pure-rust-gtk-m5` (settings
and the shell panel migrated, the GTK stack dropped); M6 proposed
```

and rewrite decomposition item 5:

```markdown
5. **M5 — App migrations** *(implemented)*: `settings` and the `shell` panel
   rebuilt on the toolkit, each reusing its pure core verbatim, and `gtk4`,
   `gtk4-layer-shell`, `glib`, `gio`, `gdk4`, `pango` and `cairo` dropped from
   the workspace. The understand pass corrected the ordering the original plan
   assumed: the shell's taskbar and its clipboard popover share one layer-shell
   window and one process, so they are one migration; the clipboard *daemon* has
   no GTK and needed no work. Measured by five settings rest-state page gates,
   the panel gate, the interaction gates, and the dependency and deletion
   ledgers in each app crate.
```

In `ui/README.md`, in **What this crate covers**, append to the paragraph that
ends "…and icon theming.":

```markdown
**M5 rebuilds the two applications on it:** `icedtea-settings` (five pages, a
second Wayland connection for `zwlr_output_management_v1`, a portal file
chooser) and `icedtea-shell` (a layer-shell panel with a clipboard popover).
The toolkit additions M5 needed — external-event ingress (`Window::watch_fd`,
`Inbox`, `App::on_fd`, `Cmd::Task`), view-layer pointer events, base-level
keysyms, live-window probing and four widgets' rest paint — are documented in
their own sections below.
```

and in **The M3 gates**, replace the `KNOWN_BLANK_AT_REST` sentence — it is
wrong twice today ("nine entries", "seven collapse to a zero-area allocation")
and M5-D8 changed the number again:

```markdown
- `tests/gallery_gate.rs` walks the page in surface-height slices under the
  harness compositor and asserts every widget paints something in light, dark
  and high-contrast Adwaita — except the six entries listed in that file's
  `KNOWN_BLANK_AT_REST`, which are measured, not asserted on. Five of the six
  collapse to a zero-area allocation (`window_controls`, `font_dialog`,
  `popover_menu`, `popover_menu_bar`, `alert_dialog`); `link_button` gets a
  real allocation and draws nothing into it. The list was fifteen at the end of
  M3 and nine after its first fix wave; M5-D8 gave `color_dialog`,
  `color_dialog_button`, `check_button` and `scrollbar` a rest paint, so those
  are asserted on like everything else. Every entry, exempt or not, must still
  appear whole in some slice.
```

If Task 5 Step 4 found a shorter list, use the shipped list and adjust both
numbers — the README must name whatever actually ships.

Add one line to the gate command block in the same section:

```bash
cargo test -p icedtea-ui --test ingress            # M5 external-event ingress
```

- [ ] **Step 4: Run the check to verify it passes**

```bash
grep -q 'implemented on `rebuild/pure-rust-gtk-m5`' \
  docs/superpowers/specs/2026-09-03-pure-rust-gtk-m5-app-migrations-design.md \
  && echo "m5 status updated"
grep -q 'M5 implemented' docs/superpowers/specs/2026-08-20-pure-rust-gtk-ui-design.md \
  && echo "parent status updated"
grep -q 'M5 rebuilds the two applications' ui/README.md && echo "readme updated"
grep -c 'seven collapse' ui/README.md || echo "stale sentence gone"
cargo test -p icedtea-ui --test gallery_gate
```

Expected: three `updated` lines; `stale sentence gone`; and `gallery_gate`
green — its `the_readme_widget_table_lists_every_kind` and P0's
`the_readme_names_every_known_blank_widget` both read this file, so a README
edit that misnames a widget fails here.

- [ ] **Step 5: Commit**

```bash
git add docs/superpowers/specs/2026-09-03-pure-rust-gtk-m5-app-migrations-design.md \
        docs/superpowers/specs/2026-08-20-pure-rust-gtk-ui-design.md \
        ui/README.md
git commit -m "docs(m5): status lines for the two specs and the README

M5 is implemented on rebuild/pure-rust-gtk-m5; the parent spec's decomposition
item 5 records what the understand pass corrected (shell taskbar + clipboard
popover are one migration; the clipboard daemon needed no work).

The README's KNOWN_BLANK_AT_REST sentence was wrong twice -- nine entries, and
'seven collapse to zero-area' when it was six -- and M5-D8 changed the number
again. It now names the six that ship and says which kind of blank each is.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

## Task 8: the PR body, and the consent stop

**Files:**
- Create: `docs/superpowers/plans/2026-09-03-m5-pr-body.md`

**Interfaces:**
- Consumes: Tasks 1–7. Produces: no code.

- [ ] **Step 1: Write the failing check**

```bash
test -f docs/superpowers/plans/2026-09-03-m5-pr-body.md \
  && echo "pr body exists" || echo "pr body MISSING"
```

Expected: `pr body MISSING`.

- [ ] **Step 2: Re-run the close-out gate once more on the final tree**

Tasks 6 and 7 committed documentation after Task 5's run. Re-confirm nothing
moved:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
git status --porcelain
```

Expected: all green, working tree clean.

- [ ] **Step 3: Write the PR body**

Create `docs/superpowers/plans/2026-09-03-m5-pr-body.md`:

```markdown
# M5 — App migrations: settings and the shell panel off GTK

Rebuilds `icedtea-settings` and `icedtea-shell` on `icedtea-ui` and removes the
GNOME toolkit from the workspace. Spec:
`docs/superpowers/specs/2026-09-03-pure-rust-gtk-m5-app-migrations-design.md`.
Contract: `docs/superpowers/plans/2026-09-03-m5-part0-contract.md`.

## What ships

- **P0 — toolkit pre-flight (`ui/`).** External-event ingress
  (`Window::watch_fd`/`unwatch`, `InputEvent::FdReady`, an N-fd `wait_bounded`,
  `Inbox`/`InboxSender`, `App::with_inbox`, `App::on_fd`, `Cmd::Task`),
  `App::new` widened to closures, three view-layer pointer `EventKind`s with
  `Handler::PairButton`, `Prop::Draw` carrying `&mut PaintCx`,
  `Keymap::base_keysym`/`KeyEvent::base`, live-window probing
  (`Window::probe_points`/`allocation`), and rest paint for four widgets —
  `KNOWN_BLANK_AT_REST` drops from nine to six.
- **P1–P4 — settings.** One `App<SettingsModel, Msg>` on a `Role::Toplevel`
  window: `Stack` + `StackSwitcher` over Appearance, Behavior, Workspaces,
  Keybindings and Displays, a computed-dirty footer, a zbus reload worker and a
  portal `FileChooser` worker on the inbox, the outputs connection's queue fd on
  `watch_fd`, and the Displays canvas on a `DrawingArea` with the existing
  `displays_canvas` maths called verbatim.
- **P5 — the shell panel.** One `App<PanelModel, Msg>` on a `Role::Layer`
  surface, genuinely reactive (keyed by workspace id, window id and history
  entry id — clear-and-rebuild was a GTK workaround), with the clipboard
  popover as a real `xdg_popup` off the layer root.
- **P6 — close-out.** The dependency and deletion ledgers, the full gate set,
  and the deviation record.

## The GTK stack is gone

`cargo tree -e normal --workspace` names no `gtk4`, `gtk4-layer-shell`, `glib`,
`gio`, `gdk`, `pango` or `cairo`, and all twenty-four GTK-stack packages have
left `Cargo.lock`. Enforced from here on by
`settings/tests/dependency_audit.rs` and `shell/tests/dependency_audit.rs`.

Deleted: `settings/src/outputs/client.rs`, `settings/src/pages/displays.rs`
(split), `settings/tests/appearance_gtk.rs`, `settings/tests/keybindings_gtk.rs`,
`shell/src/bridge.rs`, `shell/tests/shell_gtk.rs`, and thirteen in-place items.
Each is paired with its replacement in `*/tests/deletion_ledger.rs`.

## Known regression: input methods

**Both apps lose IME support at this merge.** GTK4's `Entry` speaks
`text-input-v3` through GTK's IM context; the toolkit does not, and
`text-input-v3` is M6 scope by policy. Settings' wallpaper path, workspace-name
and keybinding fields and the clipboard popover's search field accept direct key
events only. Users of an input method lose composition in these two apps until
M6. Recorded as contract amendment P6-D8.

## Also deliberately out of scope

Drag-and-drop, `accesskit`, emoji, portals beyond `FileChooser`, a native file
dialog, the six remaining `KNOWN_BLANK_AT_REST` widgets, `StackSidebar`'s
eviction defect, the M2-inherited background-layer `currentColor` gap, and the
compositor's `sh -c` spawn action (pre-existing, flagged by the M3 security
review).

## Gates

`cargo fmt --all --check`; `cargo clippy --workspace --all-targets -D warnings`
and `-p icedtea-ui --no-default-features`;
`RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps`;
`cargo test --workspace`. M1/M2/M3's gates are green unchanged —
`themed_button_offscreen` 4, `adwaita_coverage` 10, `gtk4_property_reference` 4,
`layer_shell_screencopy` 3, `transition_screencopy` 1, `node_trees` 55,
`counter_app` 4, `reconcile_props` 3, `gallery_gate` light/dark/hc, and
`interaction_gate` 16/16 — and eight of their files are byte-identical to
`develop`. `settings/tests/live_apply.rs` still spawns the real
`icedtea-compositor` binary, and `shell/tests/live_dbus.rs` still drives the
real `icedtea-clipboard` service.

🤖 Generated with [Claude Code](https://claude.com/claude-code)

https://claude.ai/code/session_01EMhh94aEjJoKP53cq3G5C2
```

Paste Task 5's recorded `test result:` lines under **Gates**, replacing the
prose counts with the measured ones where they differ.

- [ ] **Step 4: Run the check to verify it passes**

```bash
test -f docs/superpowers/plans/2026-09-03-m5-pr-body.md && echo "pr body exists"
grep -q 'lose IME support' docs/superpowers/plans/2026-09-03-m5-pr-body.md \
  && echo "regression stated"
```

Expected: `pr body exists`, `regression stated`.

- [ ] **Step 5: Commit**

```bash
git add docs/superpowers/plans/2026-09-03-m5-pr-body.md
git commit -m "docs(m5): the PR body for the merge window

States the IME regression in plain words, as contract §4.3 item 3 requires:
both apps' text entries lose input-method support until M6.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

- [ ] **Step 6: Stop for consent**

M5 is complete on `rebuild/pure-rust-gtk-m5`. **Do not merge, do not push, do
not open the PR.** Present the branch as ready — the full gate set green, the
audits and ledgers passing, the deviation record written — and ask whether to
proceed with the push/PR/merge, or to wait while the owner runs `/simplify`,
`/code-review`, `/optimize` or `/lex-review` themselves. Spec §9 makes the merge
a consent stop, and the owner's global rule makes it one for every branch.

Report, in the hand-off:

- the `cargo test --workspace` totals and wall-clock time from Task 5;
- the number of contract §6 amendments, and that `<commit>` placeholders are 0;
- the one regression (IME) and where it is recorded;
- any gate failure routed back to an owning part during Task 5, and to which
  part.

---

## Self-Review

### 1. Spec and contract coverage

| Spec / contract requirement | Where |
|---|---|
| §8 / §4.1 `cargo tree -p icedtea-settings -p icedtea-shell` shows no GTK | Task 1 Step 5, Task 2 Step 5 (per-crate `-i` loops), Task 2 Step 5 (workspace `cargo tree \| grep`) |
| §4.1 the `-i` invert query per package (`gtk4`, `gtk4-layer-shell`, `glib`, `gio`, `gdk`, `gdk4`, `pango`, `cairo`, `cairo-rs`, `gobject`, `glib-sys`, `gtk4-sys`) | Tasks 1 and 2 Step 5 loops (ten packages each, covering all twelve names between them), corroborating the lockfile tests |
| §4.1 the workspace-wide `cargo tree \| grep` is empty | Task 2 Step 5 (`workspace clean`) |
| §4.1 the source grep over `settings/src settings/tests shell/src shell/tests` is empty | Task 1's `no_settings_source_mentions_a_gtk_name`, Task 2's `no_shell_source_mentions_a_gtk_name`; the residue it exposes is swept in the same tasks (P6-D1) |
| §4.2 the six deleted files | Task 3's and Task 4's `DELETED_WITH_REPLACEMENT` (4 + 2) |
| §4.2 the thirteen deleted-in-place items | Task 3's `DELETED_IDENTIFIERS` (12 patterns covering `Ctx`, `Page`, `mark_dirty`, `refresh`/`populating`, `unshifted_keysym`, `install_conflict_css`, `sync_rows`, `reset_capture`, `Row`, `set_remove_sensitivity`, every `build()`, the slot) + Task 4's (7 patterns covering `taskbar::render`, `clipboard::{render, connect_activation}`, `pub use gtk4`, `pub mod bridge`, and two GTK types) |
| "every deletion is paired with the test that proves the replacement" | The third column of both `DELETED_WITH_REPLACEMENT` tables; the one unpaired deletion is pointed at P6-D7 explicitly |
| §4.3 item 1 — M5-D1…M5-D11 marked landed with commits | Task 6 Step 3's landed table; Step 4 asserts zero `<commit>` placeholders |
| §4.3 item 2 — the populate guard | P6-D7 |
| §4.3 item 3 — the IME regression, "in the PR body as well as here" | P6-D8 and Task 8's PR body ("Known regression: input methods") |
| §4.3 item 4 — the `DropDown` clipping resolution | P6-D9 |
| §4.3 item 5 — the M3 §3 `App::new` amendment, cross-referenced from M3's §10 | P6-D10 and Task 6 Step 5's `P8-D76` pointer |
| §4.3 item 6 — `auto_exclusive_zone_enable()` → literal 28 | P6-D11 |
| §4.3 item 7 — `async-channel` retained | P6-D12 |
| §4.3 item 8 — anything the parts add, in the M3 §10 shape | Task 6 Step 2 inventories them and Step 3 preserves them, unrenumbered, between the landed table and P6's entries |
| §4.4 `cargo fmt --all --check` | Task 5 Step 2, Task 8 Step 2 |
| §4.4 `cargo clippy --workspace --all-targets -- -D warnings` | Task 5 Step 2, Task 8 Step 2 |
| §4.4 `cargo clippy -p icedtea-ui --all-targets --no-default-features` | Task 5 Step 2 |
| §4.4 `cargo doc --workspace --no-deps` with `-D warnings` | Task 5 Step 2 (P6-D5 pins the env var) |
| §4.4 `cargo test --workspace` | Task 5 Step 2, Task 8 Step 2 |
| §4.4 the named M1/M2 gates | Task 5 Step 2's first block, with counts |
| §4.4 the named M3 gates, `interaction_gate` 16/16 | Task 5 Step 2's second block; Step 3 proves eight gate files are byte-identical to `develop` |
| §4.4 M5 new: `ui/tests/ingress.rs`, settings' five rest gates + interaction gates, `shell/tests/panel.rs` + panel/popover gates | Task 5 Step 2's third block runs them; Tasks 3 and 4's `REQUIRED_GATES` assert all thirty-two by name |
| §4.4 the kept-verbatim suites (`outputs_client`, `live_apply` against the **real** compositor binary, `live_dbus`) | Task 5 Step 2's last block; inherited decision 11 named in the plan text |
| §4.4 "screencopy gates run the full set once at close-out" | Task 5 Step 2, and its wall-clock number is recorded |
| §5 P6 "Must not touch: any behaviour" | Stated in Global Constraints; Tasks 3 and 4 Step 3 route failures to the owning part; the single enumerated exception is P6-D1's comment sweep, proved comment-only by a mechanical diff filter in Tasks 1 and 2 Step 4 |
| §5 cross-cutting: mutation check per load-bearing test | Every `#[test]` in Tasks 1–4 carries one in its doc comment; Task 5 Step 1 is itself a mutation check of the shell gate |
| §5 cross-cutting: untrusted input never panics | `rust_sources` on a missing directory yields `[]`; `locked_package_names` on a changed format fails a named assertion rather than passing vacuously |
| Spec §9 "the merge is a consent stop" | Task 8 Step 6 |
| Spec §10 risk "IME regression at deletion — stated here and in the PR" | P6-D8 + PR body |
| Spec §11 out of scope | Named in the PR body's "Also deliberately out of scope"; no task touches any of it |

Gaps found and closed while writing: §4.1's source grep is unsatisfiable
without editing must-not-touch files (P6-D1); §4.1's `-i` expectation names a
string cargo never prints (P6-D4); §4.4's `-D warnings` is not a `cargo doc`
flag (P6-D5); §4.2 has no proof mechanism at all (P6-D6); and a workspace-scope
assertion has no crate to live in (P6-D3). All five are recorded as deviations
rather than silently worked around.

### 2. Placeholder scan

Searched for `TBD`, `TODO`, `FIXME`, `similar to Task`, `as above`, `and so
on`, `implement later`, `handle edge cases`, `add appropriate`, `...` inside
code blocks.

- No `TBD`/`FIXME`/`similar to Task N` anywhere. `TODO` appears only as a
  *forbidden token* inside `DEFERRED_WORK_TOKENS` and in the mutation-check
  comments that tell the executor to add one temporarily — never as an unfilled
  blank.
- One deliberate placeholder exists and is removed within the same task:
  `<commit>` in Task 6's landed table, filled in Step 3 from Step 2's `git log`,
  with Step 4 asserting `grep -c '<commit>'` is `0`.
- Task 5 has no production code. That is the deliverable being a recorded run,
  not a missing step: it has a failing check (Step 1 breaks `BTN_MIDDLE` and
  watches the shell gate fail), an exact command list, an exact expectation per
  command, and an explicit routing rule for a failure.
- Tasks 3 and 4 Step 3 say "no production code" and then say exactly what to do
  instead: restore the deletion, route a real failure to the owning part, and
  the one permitted table adjustment (a renamed gate, confirmed with
  `git log -S`) with its evidence requirement.
- Every ellipsis in this plan is inside prose or a quoted contract heading
  range (`M5-D1 … M5-D11`), never inside a code block standing in for code.

### 3. Type consistency against the contract

- **Nothing in this part is a public interface.** Every item Tasks 1–4 add is a
  private `const` or `fn` inside an integration-test binary; no crate imports
  them, so no contract signature is touched. That is why the "Produces" blocks
  list test-only items and why P6 needs no §6 signature amendment.
- `rust_sources`, `offending_lines`, `count_tests`, `crate_dir` and
  `workspace_dir` are declared once per file with identical signatures in all
  four files, and are called with exactly those signatures. `workspace_dir` is
  absent from `shell/tests/dependency_audit.rs` on purpose — that file is
  crate-scope (P6-D3) — and present in the other three, each of which uses it.
- The four table types are consistent per file: `&[(&str, &str, &str)]` for
  `DELETED_WITH_REPLACEMENT`, `&[(&str, &str)]` for `DELETED_IDENTIFIERS`,
  `&[(&str, usize)]` for `MOVED_SUITES`/`KEPT_SUITES`, `&[&str]` for
  `REQUIRED_GATES`, `GTK_TOKENS`, `GTK_STACK_PACKAGES`, `GTK_STACK_PREFIXES`,
  `STALE_GTK_PROSE`, `DEFERRED_WORK_TOKENS` and `SELF_EXCLUDED`. Every loop
  destructures the arity it declares.
- Paths: `DELETED_WITH_REPLACEMENT` entries are **workspace-relative** and are
  joined onto `workspace_dir()`; `MOVED_SUITES`/`KEPT_SUITES` entries are
  **crate-relative** and are joined onto `crate_dir()`. The two conventions are
  stated in each table's doc comment and used consistently.
- Names taken from the contract verbatim: the twenty-six settings gate names
  (§2.1, §2.6, §2.7, §2.8), the six shell gate names (§3.6), the six deleted
  paths (§4.2), the moved-suite counts (§2.1 and §3.6, cross-checked against a
  measurement of `develop` @ `d9cee52`: 8/12/6/6/2/10/1 and 5/1/1/1), the six
  `KNOWN_BLANK_AT_REST` entries (M5-D8), the `BTN_MIDDLE = 0x112` constant
  (M5-D5), and `COMPOSITOR_CONTRACT_VERSION`'s consumer
  `shell/src/compositor_client.rs` (§3.5).
- `settings/src/outputs/pump.rs` is named in Task 1 Step 3's rewritten module
  doc and in Task 3's `DELETED_WITH_REPLACEMENT`; the contract's §0 module map
  spells it `settings/src/outputs/pump.rs`, and Task 1 Step 3 says explicitly
  what to do if P1 shipped a different module name (use the shipped one — a
  broken intra-doc link fails `cargo doc -D warnings` in Task 5 either way).
- `ProbePoint`, `Window::probe_points`, `Inbox`, `Cmd::Task`, `Handler::PairButton`
  and `Keymap::base_keysym` are referenced only in prose and in the PR body's
  feature list; this part calls none of them, so there is no signature to drift.
