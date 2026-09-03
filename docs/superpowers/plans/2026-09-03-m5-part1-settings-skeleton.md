# Pure-Rust GTK-themed UI — M5 Part 1: settings skeleton: pure-core extraction, displays.rs split, dependency swap, toplevel window, nav + footer, reload worker — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn `icedtea-settings` into a pure-Rust `icedtea-ui` application — every GTK-free
core extracted to a sibling module with its tests, `displays.rs` split into state/view halves,
`gtk4`/`gio`/`glib` gone from the crate, and one `App<SettingsModel, Msg>` on a
`Role::Toplevel` window whose navigation, footer, reload worker and outputs pump all work —
with the five pages still empty for P2–P4 to fill.

**Architecture:** The crate splits along the line the M5 spec draws: `model.rs`,
`compositor_reload.rs`, `outputs/protocol.rs` and `pages/displays_canvas.rs` are untouched
GTK-free cores; the logic that was trapped inside GTK `build()` closures moves to
`pages/{workspaces,keybindings,appearance}.rs` and `pages/displays/state.rs` with its tests
following it verbatim; and everything that was a widget tree becomes an Elm loop —
`SettingsModel` wrapping `model::Model`, a `Send` `Msg`, a non-blocking `update`, and a pure
`view(&SettingsModel) -> View<Msg>` of `stack_switcher` + `stack` + footer. Two external event
sources reach that loop through P0's ingress: a `std::thread` reload worker posting
`Msg::Applied`/`Msg::ConfigReloaded` on an `Inbox`, and the second `wayland-client` connection
(`zwlr_output_management_v1`) whose `EventQueue` fd is registered with `Window::watch_fd` and
mapped to `Msg::Outputs` by `App::on_fd`.

**Tech Stack:** Rust edition 2024 (rust-version 1.94), `icedtea-ui` (M3 + P0), `icedtea-config`,
`icedtea-contract`, `wayland-client 0.31`, `wayland-protocols-wlr 0.3`, `rustix 1`,
`crossbeam-channel` (workspace), `async-channel 2` (kept: `outputs/protocol.rs` is unchanged),
`zbus` (workspace), `redb` (workspace), `tracing`/`tracing-subscriber`,
`icedtea-harness` + `tempfile` (dev).

**Spec:** `docs/superpowers/specs/2026-09-03-pure-rust-gtk-m5-app-migrations-design.md`
(§2 decisions D1–D9 and the eleven inherited decisions; §5.1–§5.3 shape and IPC; §7 gates)
and the frozen contract `docs/superpowers/plans/2026-09-03-m5-part0-contract.md`
(§2.1 extraction table, §2.2 model/`Msg`, §2.3 view decomposition and widget ids, §2.4 reload
worker, §2.5 outputs pump, §5's P1 boundary). Predecessor contract:
`docs/superpowers/plans/2026-08-27-m3-part0-contract.md` (§3 public interfaces, §10 P3-A…P8-D75,
§11 E1–E21).

---

## Contract deviations

Every item below is a departure from `2026-09-03-m5-part0-contract.md` discovered while
writing this plan. Each is numbered in the contract's own `P1-D<n>` scheme, is carried out by
the task named, and **must be appended to contract §6 by the task that lands it** in the M3 §10
shape (*Carried out by / Added / Contract says / As shipped / Ruling*).

- **P1-D1 — `Msg` is built up per part, not all at P1.** Contract §2.2 lists one frozen `Msg`
  enum spanning P1–P4. P1 defines only the six variants its own view and workers can produce
  (`PageSelected`, `Apply`, `Revert`, `Applied`, `ConfigReloaded`, `Outputs`); P2/P3/P4 each
  append their page's variants when they add the view that fires them. Ruling: an `update` arm
  with no view able to fire it cannot be driven by a test, so writing one at P1 would be
  untested code by construction; §2.2's list stays normative as the **union** across P1–P4 and
  no variant's name, payload type or `Send`-ness changes. (Task 6a.)
- **P1-D2 — `SettingsModel` gains `outputs: Option<crate::outputs::pump::OutputsPump>`.**
  Contract §2.2's struct listing omits it, but §2.5's own wiring snippet passes `pump.clone()`
  into `SettingsModel::new`, and P4's Test/Apply must reach the pump from `update` through
  `Cmd::Task`. `OutputsPump` is `Clone` (its fields are an `Rc`, an `async_channel::Receiver`
  and a `WatchId`).
  **The constructor's shape changes with it.** §2.5's snippet reads
  `SettingsModel::new(db_path, workers, pump.clone())` — three positional arguments. P1 ships
  `SettingsModel::new(db_path: PathBuf, workers: WorkerHandles) -> Self` (two arguments,
  `outputs: None`) plus `#[must_use] pub fn with_outputs(self, pump: Option<OutputsPump>) -> Self`,
  so `main.rs` reads `SettingsModel::new(db, workers).with_outputs(pump.clone())`. Ruling: the
  two forms carry the same three values into the same three fields, and the builder is what lets
  every `SettingsModel` unit test (Task 6) construct a model without a Wayland connection, which
  a 3-arity constructor taking `Option<OutputsPump>` would also permit but would force every
  test and every future caller to spell `None` for a field they do not use. §2.5's wiring is
  otherwise honoured verbatim, and `with_outputs` is the only place `outputs_available` is set
  from the pump's presence. A reader diffing §2.5's snippet against Task 10's `main` should
  expect exactly this one substitution and nothing else. (Task 8 adds the field and the builder;
  Task 10 wires it.)
- **P1-D3 — `PAGES` is realised as `PageId::ALL` plus `pages::page_infos()`.** The contract's
  module map (§0) names a `PAGES` item in `pages/mod.rs`. `StackPageInfo` holds `Rc<str>`, which
  cannot appear in a `const`, so the switcher's page list is a function returning
  `Rc<[StackPageInfo]>` built from `PageId::ALL`. (Task 5.)
- **P1-D4a — `Inbox::try_recv` is added to `ui/src/view/inbox.rs` by P1, as an enumerated
  exception to §5's "P1 must not touch `ui/`".** M5-D2 gives `Inbox` only `new` and `sender`,
  so nothing can observe what a worker posted without a running `App`, and the reload worker's
  round trip (contract §2.4) would be untestable off the compositor. Added:
  `pub fn try_recv(&self) -> Option<Msg>` — drains one wake byte and one queued message,
  `None` when the channel is empty. Purely additive; `App::run`'s drain is unchanged. (Task 6b.)
- **P1-D4 — `App::with_probe_report` is added to `ui/src/view/app.rs` by P1, as an enumerated
  exception to §5's "P1 must not touch `ui/`".** M5-D9 promises that a settings build writes
  `probe`/`alloc` lines to `$ICEDTEA_PROBE_REPORT` "once per frame in which the tree changed",
  but `App::run` owns the `Window` for the whole loop and exposes no per-frame hook, so
  `Window::probe_points`/`Window::allocation` are unreachable from the app crate. This is the
  same shape as the already-discharged P4 `drop_down.rs` exception (§7): the API is knowable
  only from an app, so the app's part lands it. **P0's head does ship it** — P0's own deviation P0-D4 lands
  `#[must_use] pub fn with_probe_report(self, path: std::path::PathBuf) -> Self` plus an
  automatic `$ICEDTEA_PROBE_REPORT` pickup inside `App::run` (P0 Task 18), precisely so P1 and
  P5 do not each write a report writer. Task 9 therefore reduces to: assert the signature and
  the emitted `probe`/`alloc` line format match, then wire it; **P1 implements nothing under
  `ui/` for this item** and P1-D4 is recorded as "consumed from P0, not landed by P1". If the
  signature differs, stop and report rather than adding a second writer. `Inbox::try_recv`
  (P1-D4a) is unaffected: P0 ships no public `try_recv`, so that one is genuinely P1's.
  (Task 9; consistency-check ruling E1.)
- **P1-D5 — the probe report is append-only, in `frame <n>` blocks.**
  `ui/tests/support/mod.rs::parse_allocation_line` takes exactly five whitespace fields, so the
  contract's literal `alloc <id> <x> <y> <w> <h>` line (six fields) parses as `None`. P1 keeps
  the contract's line text and has readers strip the leading `alloc ` before parsing; each
  report block is appended (never truncated) behind a `frame <n>` marker so an app's own
  `msg <…>` lines and the geometry blocks can share one file. (Task 9.)
- **P1-D6 — P1 adds `settings/tests/support/mod.rs` and `settings/tests/skeleton.rs`.**
  §5's "Owns (settings)" list for P1 enumerates `src/` files only, but its gate names a harness
  test (`the_first_toplevel_app_run_paints_and_navigates`) that needs a home and a settings-local
  copy of the report/screencopy helpers (`ui/tests/support` is not a published module). Task 8
  creates the file with its first helper; Task 11 completes it. (Tasks 8 and 11.)
- **P1-D7 — `WorkerHandles` ships with the `reload` half only; P2 lands `portal`/`choose_wallpaper`.**
  Contract §2.4 is headed "IPC: the worker thread and inbox wiring **(P1)**" and its frozen
  struct carries two fields and two methods:

  ```rust
  #[derive(Clone)]
  pub struct WorkerHandles {
      reload: crossbeam_channel::Sender<ReloadRequest>,
      portal: crossbeam_channel::Sender<PortalRequest>,
  }
  impl WorkerHandles {
      pub fn apply(&self, cfg: icedtea_config::Config, db_path: std::path::PathBuf);
      pub fn choose_wallpaper(&self, current: Option<std::path::PathBuf>);
  }
  pub fn spawn(tx: InboxSender<crate::app::Msg>) -> WorkerHandles;
  ```

  P1 ships only `reload`/`apply`, and `ipc::spawn` starts only the reload worker.
  **Contract says:** the signature above, under a `(P1)` header, and §5's cross-cutting rule
  "no part changes a signature in this contract without a §6 amendment".
  **As shipped (P1):** `WorkerHandles { reload }` with `apply` alone; no `portal` field, no
  `choose_wallpaper`, no `ipc/portal.rs`, no `PortalRequest`.
  **Ruling:** the narrowing is a *part-boundary* split, not a design change. §5's own
  implemented-vs-consumed table assigns "§2.6 portal worker | P2 | P2", §5's P2 block owns
  `src/ipc/portal.rs` (new), and §2.6 is where `PortalRequest`, the `OpenFile` flow and
  `Msg::WallpaperBrowse`/`WallpaperChosen`/`WallpaperPickerFailed` are specified — none of which
  exist at P1, so a P1 `choose_wallpaper` could neither be called by a view nor answered by a
  worker nor driven by a test. This is the same reasoning as P1-D1's incremental `Msg`, and it is
  recorded here for the same reason: §2.4's header says `(P1)`, so a P2 author reading only §2.4
  would otherwise expect the whole struct to have shipped.
  **Binding on P2:** P2 adds the `portal` field and `choose_wallpaper` **exactly as §2.4 spells
  them** — same names, same argument types, same "never blocks; the answer arrives as
  `Msg::WallpaperChosen`/`Msg::WallpaperPickerFailed`" contract — and extends `ipc::spawn` to
  start the portal worker alongside the reload worker. Nothing P1 ships blocks that: both
  `WorkerHandles`'s fields and `spawn`'s body are private, `spawn`'s signature is already §2.4's
  final one, and `handles_for_test` (P1's own addition, below) is the only other constructor.
  P2 extends `handles_for_test` to return the portal receiver too, in the same tuple shape.
  §2.4's signature is therefore the **union** across P1–P2; no name, argument type or blocking
  guarantee in it changes. (Task 6a ships the P1 half; P2 completes it.)
- **P1-D8 — `ipc::handles_for_test` is added, beyond contract §2.4's listing.** §2.4 gives
  `ipc` exactly `WorkerHandles`, its methods and `spawn`. `spawn` starts real OS threads and a
  real D-Bus subscription, which no `SettingsModel` unit test wants; `handles_for_test() ->
  (WorkerHandles, Receiver<reload::ReloadRequest>)` hands back the same handle type wired to a
  receiver the caller keeps, with no thread behind it. Purely additive, `#[must_use]`, and used
  only by tests. P2 extends its return tuple with the portal receiver when it lands P1-D7's
  other half. (Task 6a.)

---

## Global Constraints

Copied from the spec and the contract. Every task's requirements implicitly include this section.

- **Pinned crates, exact:** `wayland-client 0.31`, `wayland-protocols-wlr 0.3` (`features = ["client"]`),
  `zbus` = the workspace version (`zbus.workspace = true`, resolves to 5.18.0), `rustix 1`,
  `skia-rs-safe 0.4.0` (through `icedtea-ui` only), `taffy 0.14` (through `icedtea-ui` only),
  `xkbcommon 0.9` (through `icedtea-ui` only), `wlr 0.20.29` (through `icedtea-harness` only),
  `async-channel 2`, `crossbeam-channel` = `crossbeam-channel.workspace = true`, `redb.workspace = true`,
  `tempfile 3` (dev). No new dependency may be added to `settings/Cargo.toml` beyond the ones
  §2.1 of the contract lists.
- **No `gtk4`, `gtk4-layer-shell`, `glib`, `gio`, `gdk`, `pango` or `cairo`** in a migrated app —
  neither as a dependency nor as a path in source. From Task 11 onward,
  `grep -rn 'gtk4\|glib::\|gio::\|gdk::\|pango::\|cairo::' settings/src settings/tests` must be empty.
- **Edition 2024, `rust-version` 1.94**, both inherited via `edition.workspace`/`rust-version.workspace`.
- **Gates, per crate, after every task that touches it:**
  `cargo test -p icedtea-settings`, `cargo clippy -p icedtea-settings --all-targets -- -D warnings`,
  and for any task touching `ui/`: `cargo test -p icedtea-ui`,
  `cargo clippy -p icedtea-ui --all-targets -- -D warnings` **and**
  `cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings`.
  `cargo fmt --all --check` before every commit.
- **All M1/M2/M3 gates stay green, unchanged:** `themed_button_offscreen`, `adwaita_coverage`,
  `gtk4_property_reference`, `layer_shell_screencopy`, `transition_screencopy`, `window_events`,
  `counter_app`, `node_trees`, `widget_pixels`, `reconcile_props`, `gallery_gate`,
  `interaction_gate` (16/16).
- **Parts execute in order P0 → P1 → … → P6**, each based on the previous part's head, so this
  plan may consume P0's Produces (M5-D1 `watch_fd`/`FdReady`, M5-D2 `Inbox`/`with_inbox`/`on_fd`,
  M5-D3 `Cmd::Task`, M5-D4 `App::new` closures, M5-D9 `Window::probe_points`/`allocation`,
  M5-D10 the `ui` dependency additions) as already shipped.
- **Settings swaps `gtk4` out IN PLACE (spec D3).** There is no second binary, no feature flag
  and no sibling crate: at Task 11's commit the GTK settings binary stops existing. All M5 work
  happens in this worktree and merges as one PR.
- **`settings/tests/live_apply.rs` keeps spawning the REAL `icedtea-compositor` binary**
  (inherited decision 11). No task may point it at `icedtea_harness::Compositor`, and no task may
  edit it, `settings/tests/outputs_client.rs`, `settings/src/model.rs`,
  `settings/src/compositor_reload.rs` or `settings/src/outputs/protocol.rs`.
- **M6 scope is out of bounds even opportunistically:** IME/`text-input-v3`, drag-and-drop,
  `accesskit`, emoji, portals beyond `FileChooser`, `StackSidebar`'s eviction defect, the
  M2-inherited background-layer `currentColor` gap, the compositor's `sh -c` spawn.
- **Untrusted input never panics:** config files, keymaps, output-management heads, D-Bus signal
  bodies. A malformed value is dropped, logged once, and replaced by the initial/fallback.
- **Every load-bearing test records a mutation check** — break the code the test claims to cover,
  confirm the test fails, restore — written into the test's own doc comment.
- **Commit trailer, on every commit:**

  ```
  Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
  ```

---

## File Structure

| Path | Status after P1 | Responsibility |
|---|---|---|
| `settings/Cargo.toml` | modified (T4, T6, T10) | `icedtea-ui` + `crossbeam-channel` in; `gtk4`/`gio`/`glib` out |
| `settings/style.css` | created (T10) | app sheet layered as a user-origin overlay (`.conflict` rule is P3's) |
| `settings/src/lib.rs` | modified (T6a, T9, T10) | module tree: `app`, `compositor_reload`, `ipc`, `model`, `outputs`, `pages`, `probe` |
| `settings/src/main.rs` | rewritten (T10) | thin binary: sheet → `Window::open` → `Inbox` → workers → pump → `App::run` |
| `settings/src/app.rs` | created (T6a), extended (T7, T8, T9) | `SettingsModel`, `Msg`, `update`, `view`, `nav`, `footer` |
| `settings/src/probe.rs` | created (T9) | `$ICEDTEA_PROBE_REPORT` line appender for the app's own state lines |
| `settings/src/ipc/mod.rs` | created (T6a: `WorkerHandles`, `handles_for_test`; T6b: `spawn`) | `WorkerHandles`, `spawn` |
| `settings/src/ipc/reload.rs` | created (T6a: `ReloadRequest`; T6b: the worker) | `ReloadRequest` + the reload/`ConfigReloaded` worker thread |
| `settings/src/outputs/mod.rs` | modified (T8, T10) | drops `client`, adds `pump` |
| `settings/src/outputs/pump.rs` | created (T8) | `OutputsPump`: `watch_fd`/`on_fd` glue for the second Wayland connection |
| `settings/src/outputs/client.rs` | **deleted** (T10) | the `gio::Socket`/`glib` source |
| `settings/src/outputs/protocol.rs` | untouched | the GTK-free protocol core |
| `settings/src/model.rs` | untouched | working/saved pair, capture translation, validation |
| `settings/src/compositor_reload.rs` | untouched | blocking `ReloadConfig` client |
| `settings/src/pages/mod.rs` | rewritten (T5, T10) | `PageId`, `page_infos`, module list (`Ctx`/`Page` deleted at T11) |
| `settings/src/pages/appearance.rs` | modified (T4, T7, T10) | colour conversions; GTK `build()` deleted at T11 |
| `settings/src/pages/behavior.rs` | modified (T7, T10) | GTK `build()` deleted; `view` stub for P2 |
| `settings/src/pages/workspaces.rs` | modified (T3, T7, T10) | `next_workspace_name`, `prune_orphaned_workspace_bindings` |
| `settings/src/pages/keybindings.rs` | modified (T2, T7, T10) | `action_list`, `format_combo`, `should_capture`, `take_capture_reset`, `normalise_keysym` |
| `settings/src/pages/displays_canvas.rs` | untouched | `Rect`/`View`/`compute_view`/`hit_test`/`snap`, 8 tests |
| `settings/src/pages/displays.rs` | **deleted** (T1) | split below |
| `settings/src/pages/displays/mod.rs` | created (T1), rewritten (T7, T10) | page module root; GTK `build()` until T11, then the `view` stub for P4 |
| `settings/src/pages/displays/state.rs` | created (T1) | `DisplaysState`, `Drag`, `reconcile`, rect/mode helpers, 12 tests |
| `settings/tests/support/mod.rs` | created (T8), completed (T11) | settings-local harness helpers: spawn, probe report, screencopy |
| `settings/tests/skeleton.rs` | created (T11) | `the_first_toplevel_app_run_paints_and_navigates` + rest-state paint |
| `settings/tests/outputs_client.rs` | untouched | drives `OutputsConnection` directly |
| `settings/tests/live_apply.rs` | untouched | spawns the real `icedtea-compositor` |
| `settings/tests/appearance_gtk.rs` | untouched by P1 | P2 deletes it |
| `settings/tests/keybindings_gtk.rs` | untouched by P1 | P3 deletes it |
| `ui/src/view/app.rs` | modified (T9) | `App::with_probe_report` (deviation P1-D4) |
| `ui/README.md` | modified (T9) | one subsection documenting the probe-report writer |
| `ui/src/view/inbox.rs` | modified (T6b) | `Inbox::try_recv` (deviation P1-D4a) |
| `ui/tests/ingress.rs` | modified (T6b, T9) | `try_recv` and probe-report tests |
| `settings/tests/outputs_pump.rs` | created (T8) | the pump against the harness compositor |
| `settings/tests/no_gtk.rs` | created (T10) | the dependency-swap gate |

---

## Task 1: Split `pages/displays.rs` into `pages/displays/{mod,state}.rs`

The spec's risk table names this first: "`displays.rs` ported without a split → the split is
P1's job, before any page ports". This task moves the 1,312-line file's GTK-free half
(lines 24–293: constants, `DisplaysState`, `baseline_edit`, `default_mode_for`,
`same_connector_set`, `Reconciled`, `reconcile`, `distinct_resolutions`, `refreshes_for`,
`format_refresh`, `enabled_rects`, `all_rects`) and its **twelve** module tests into
`pages/displays/state.rs`, byte-for-byte, and leaves the still-GTK `build()` in
`pages/displays/mod.rs`. Nothing about the crate's behaviour changes; `gtk4` is still a
dependency after this task.

**Files:**
- Create: `settings/src/pages/displays/state.rs`
- Create: `settings/src/pages/displays/mod.rs`
- Delete: `settings/src/pages/displays.rs`

**Interfaces:**
- Consumes: `crate::outputs::{Head, HeadEdit, Mode, ModeRequest}` and
  `crate::pages::displays_canvas::{Rect, View, compute_view, head_rect, hit_test, snap}`
  (both unchanged).
- Produces (all `pub`, in `crate::pages::displays::state`):

```rust
pub const CANVAS_MARGIN: f64 = 16.0;
pub const SNAP_THRESHOLD: f64 = 40.0;
pub const TRANSFORM_LABELS: [&str; 8];
pub const TRANSFORM_VALUES: [i32; 8];

pub struct DisplaysState {
    pub heads: Vec<Head>,
    pub edits: Vec<HeadEdit>,
    pub selected: Option<usize>,
    pub dirty: bool,
    pub view: crate::pages::displays_canvas::View,
    pub drag: Option<Drag>,
    pub res_options: Vec<(i32, i32)>,
    pub refresh_options: Vec<i32>,
}
impl DisplaysState { pub fn new() -> Self; }
impl Default for DisplaysState { fn default() -> Self; }

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Drag { pub head: usize, pub origin: (f64, f64), pub start: (i32, i32) }

pub struct Reconciled {
    pub edits: Vec<HeadEdit>,
    pub selected: Option<usize>,
    pub compatible: bool,
    pub dropped: bool,
}

pub fn baseline_edit(h: &Head) -> HeadEdit;
pub fn default_mode_for(head: &Head) -> Option<ModeRequest>;
pub fn same_connector_set(a: &[Head], b: &[Head]) -> bool;
pub fn reconcile(old_heads: &[Head], old_edits: &[HeadEdit], old_selected: Option<usize>,
                 dirty: bool, new_heads: &[Head]) -> Reconciled;
pub fn distinct_resolutions(modes: &[Mode]) -> Vec<(i32, i32)>;
pub fn refreshes_for(modes: &[Mode], w: i32, h: i32) -> Vec<i32>;
pub fn format_refresh(mhz: i32) -> String;
pub fn enabled_rects(st: &DisplaysState) -> (Vec<usize>, Vec<Rect>);
pub fn all_rects(st: &DisplaysState) -> (Vec<usize>, Vec<Rect>, Vec<bool>);
```

- [ ] **Step 1: Record the test count the move must preserve**

Run, and write the number down — it is the assertion this task is judged on:

```bash
cd /home/joseph/Projects/icedtea/.claude/worktrees/m5
cargo test -p icedtea-settings --lib pages::displays:: -- --list | tail -1
```

Expected: `12 tests, 0 benchmarks` (`distinct_resolutions_dedups_and_orders_by_area`,
`refreshes_for_filters_by_resolution_highest_first`, `baseline_edit_mirrors_head`,
`same_connector_set_is_order_independent`, `reconcile_preserves_edits_when_the_set_is_unchanged`,
`reconcile_resets_and_reports_dropped_on_a_set_change`,
`reconcile_rebaselines_untouched_heads_from_external_changes`,
`default_mode_for_prefers_current_then_preferred_then_first`,
`all_rects_includes_disabled_heads_for_selection`,
`all_rects_sizes_a_mode_less_head_with_a_fallback`,
`transform_dropdown_exposes_all_eight_variants`,
`reconcile_set_change_without_pending_edits_is_not_dropped`).

- [ ] **Step 2: Write the failing test that pins the new module path**

Append to `settings/src/pages/displays/state.rs` — the file does not exist yet, so create it
containing only this test module for now:

```rust
//! The GTK-free core of the Displays page.

#[cfg(test)]
mod move_proof {
    /// The twelve module tests of the old `pages/displays.rs` now live here,
    /// and every item they exercise is reachable at this path.
    ///
    /// Mutation check: rename `state::reconcile` to `state::reconcile2`; this
    /// test stops compiling. Restore.
    #[test]
    fn the_pure_core_is_reachable_at_pages_displays_state() {
        let heads: Vec<crate::outputs::Head> = Vec::new();
        let r = super::reconcile(&heads, &[], None, false, &heads);
        assert!(r.edits.is_empty());
        assert_eq!(r.selected, None);
        assert!(r.compatible, "an empty-to-empty set change is compatible");
        assert!(!r.dropped);
        assert_eq!(super::CANVAS_MARGIN, 16.0);
        assert_eq!(super::SNAP_THRESHOLD, 40.0);
        assert_eq!(super::TRANSFORM_VALUES, [0, 1, 2, 3, 4, 5, 6, 7]);
    }
}
```

- [ ] **Step 3: Run it to verify it fails**

Run: `cargo test -p icedtea-settings --lib pages::displays::state`
Expected: FAIL — `error[E0583]: file not found for module 'state'` (the module is not declared
yet), or once declared, `cannot find function 'reconcile' in module 'super'`.

- [ ] **Step 4: Create the directory module and move the pure core into it**

```bash
cd /home/joseph/Projects/icedtea/.claude/worktrees/m5/settings/src/pages
mkdir -p displays
git mv displays.rs displays/mod.rs
```

Now cut lines 24–293 of `displays/mod.rs` (from the `/// Canvas padding` doc comment down to
the closing brace of `all_rects`) plus the whole `#[cfg(test)] mod tests { … }` block at the
end of the file, and paste them into `displays/state.rs` under the header below. **Do not
retype them and do not reformat them** — they move byte-for-byte, and the twelve tests move
unmodified.

`settings/src/pages/displays/state.rs` header, above the moved block:

```rust
//! The GTK-free core of the Displays page: the pending-edit state the page
//! mutates, the hotplug/property-update reconciliation, and the mode/rect
//! helpers the canvas and the control panel both read.
//!
//! Moved verbatim out of the old `pages/displays.rs` (M5 contract §2.1) with
//! its twelve tests. Nothing here knows about a toolkit: `Head`/`HeadEdit`
//! come from `crate::outputs`, `Rect`/`View` from `displays_canvas`.

use crate::outputs::{Head, HeadEdit, Mode, ModeRequest};
use crate::pages::displays_canvas::{Rect, View, compute_view, head_rect};
```

Three edits to the moved block, and only these three:

1. Every moved item becomes `pub` (`pub const CANVAS_MARGIN`, `pub struct DisplaysState`,
   `pub fn baseline_edit`, …), and every field of `DisplaysState` and `Reconciled` becomes `pub`.
2. `DisplaysState::drag` changes type from `Option<(usize, i32, i32)>` to `Option<Drag>`, and
   the new struct is declared next to it:

```rust
/// An in-flight canvas drag. The GTK page held this implicitly inside a
/// `GestureDrag`; on the Elm loop it is model state, so P4's
/// `HeadDragBegan`/`HeadDragged`/`HeadDragEnded` arms have somewhere to put it.
///
/// `origin` is the canvas-space press point, `start` the dragged head's layout
/// position when the press landed — the drag is always computed from the press,
/// never accumulated, so a dropped motion event cannot make the head drift.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Drag {
    /// Index into `DisplaysState::heads`/`edits`.
    pub head: usize,
    pub origin: (f64, f64),
    pub start: (i32, i32),
}
```

3. `DisplaysState::new` gains the `Default` impl clippy requires for a public `new`:

```rust
impl Default for DisplaysState {
    fn default() -> Self {
        DisplaysState::new()
    }
}
```

In `displays/mod.rs`, replace the removed block with a re-import so `build()` compiles unchanged:

```rust
pub mod state;

use state::{
    CANVAS_MARGIN, DisplaysState, SNAP_THRESHOLD, TRANSFORM_LABELS, TRANSFORM_VALUES, all_rects,
    baseline_edit, default_mode_for, distinct_resolutions, enabled_rects, format_refresh,
    refreshes_for, reconcile,
};
```

and update `build()`'s two `st.drag` sites to the new type (they are the only places the tuple
was constructed or read):

```rust
// drag-begin, where the old code wrote `st.drag = Some((index, start_x, start_y));`
st.drag = Some(state::Drag {
    head: index,
    origin: (start_x_canvas, start_y_canvas),
    start: (start_x, start_y),
});

// drag-update, where the old code read `if let Some((index, sx, sy)) = st.drag`
if let Some(state::Drag { head: index, start: (sx, sy), .. }) = st.drag
```

- [ ] **Step 5: Run the moved tests**

```bash
cargo test -p icedtea-settings --lib pages::displays:: -- --list | tail -1
cargo test -p icedtea-settings
```

Expected: the list still reports **13 tests** for `pages::displays::` — the twelve moved ones
(now `pages::displays::state::tests::*`) plus `move_proof::the_pure_core_is_reachable_at_pages_displays_state`;
the full suite passes.

- [ ] **Step 6: Format, lint, commit**

```bash
cd /home/joseph/Projects/icedtea/.claude/worktrees/m5
cargo fmt --all
cargo fmt --all --check
cargo clippy -p icedtea-settings --all-targets -- -D warnings
git add settings/src/pages/displays.rs settings/src/pages/displays/
git commit -m "$(cat <<'MSG'
refactor(settings): split displays.rs into displays/{mod,state}.rs

The 1,312-line page interleaved drag maths, protocol wiring and control
repopulation in one build() closure. Its GTK-free half — DisplaysState,
reconcile, the mode and rect helpers — moves verbatim into
pages/displays/state.rs with all twelve of its tests, so P4 can port the
view onto icedtea-ui without carrying the GTK page's shape with it.

The one substantive change: DisplaysState::drag becomes Option<Drag>, a
named struct carrying the press origin and the dragged head's start
position, because the Elm loop has no GestureDrag to hold that implicitly.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
MSG
)"
```

---

## Task 2: Publish the Keybindings pure core and add `normalise_keysym`

`keybindings.rs`'s GTK-free helpers are private module items today; the M5 view layer calls
them from `app.rs` and from its own `view`, so they become `pub`. The GDK-bound
`unshifted_keysym` — the note's "single highest-risk item in this crate" — is replaced by the
toolkit-free `normalise_keysym(base, modified)`, which encodes the exact rule
`compositor/src/input.rs:109-123` matches with. `gtk4` is still a dependency after this task;
the GTK `build()` calls the new function.

**Files:**
- Modify: `settings/src/pages/keybindings.rs`

**Interfaces:**
- Consumes: `icedtea_config::KeyCombo` (unchanged).
- Produces (all `pub`, in `crate::pages::keybindings`):

```rust
pub const FIXED_ACTIONS: [&str; 10];
pub const SNAP_RESTORE: &str;
pub fn action_list(workspace_count: usize) -> Vec<String>;
pub fn format_combo(combo: &icedtea_config::KeyCombo) -> String;
pub fn should_capture(page_visible: bool, capturing: bool) -> bool;
pub fn take_capture_reset(capturing: &mut Option<String>) -> Option<String>;
pub fn normalise_keysym(base: u32, modified: u32) -> u32;
```

- [ ] **Step 1: Write the failing tests**

Append inside `settings/src/pages/keybindings.rs`'s existing `#[cfg(test)] mod tests`:

```rust
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
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p icedtea-settings --lib pages::keybindings`
Expected: FAIL — `cannot find function 'normalise_keysym' in this scope`, and
`function 'action_list' is private`.

- [ ] **Step 3: Implement**

In `settings/src/pages/keybindings.rs`, mark the pure items `pub`:

```rust
pub const FIXED_ACTIONS: [&str; 10] = [ /* unchanged list */ ];
pub const SNAP_RESTORE: &str = "snap:restore";

pub fn action_list(workspace_count: usize) -> Vec<String> { /* unchanged body */ }
pub fn format_combo(combo: &KeyCombo) -> String { /* unchanged body */ }
pub fn should_capture(page_visible: bool, capturing: bool) -> bool { /* unchanged body */ }
pub fn take_capture_reset(capturing: &mut Option<String>) -> Option<String> { /* unchanged body */ }
```

and replace `unshifted_keysym` entirely with:

```rust
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
///
/// Replaces the GDK-bound `unshifted_keysym(keycode, fallback)`: there is no
/// `gdk::Display` to ask any more, and none is needed — the toolkit stamps
/// `base` on every `KeyEvent`.
#[must_use]
pub fn normalise_keysym(base: u32, modified: u32) -> u32 {
    if base == 0 { modified } else { base }
}
```

The GTK `build()`'s single call site changes from

```rust
let keysym = unshifted_keysym(key.keycode(), keyval.into_glib());
```

to

```rust
let base = gdk::Display::default()
    .and_then(|display| display.translate_key(key.keycode(), gdk::ModifierType::empty(), 0))
    .map(|(k, _group, _level, _consumed)| k.into_glib())
    .unwrap_or(0);
let keysym = normalise_keysym(base, keyval.into_glib());
```

(the GDK lookup dies with `build()` at Task 11; only the *decision* moves out now).

- [ ] **Step 4: Run the tests**

Run: `cargo test -p icedtea-settings --lib pages::keybindings`
Expected: PASS — 10 tests (the 6 that moved with the file plus the 4 new ones).

- [ ] **Step 5: Commit**

```bash
cd /home/joseph/Projects/icedtea/.claude/worktrees/m5
cargo fmt --all && cargo fmt --all --check
cargo clippy -p icedtea-settings --all-targets -- -D warnings
git add settings/src/pages/keybindings.rs
git commit -m "$(cat <<'MSG'
refactor(settings): publish the keybindings core, replace unshifted_keysym

action_list/format_combo/should_capture/take_capture_reset become pub: the
M5 view layer calls them from app.rs, and a private helper would leave the
action set re-implemented there.

unshifted_keysym asked gdk::Display to translate a keycode at group 0,
level 0. icedtea-ui stamps that sym onto every KeyEvent as `base`
(M5-D7), so the GDK round trip becomes a two-argument decision:
normalise_keysym(base, modified) takes the base sym and falls back to the
modified one only when there is no base sym at all — the exact rule
compositor/src/input.rs matches with.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
MSG
)"
```

---

## Task 3: Publish the Workspaces pure core

Two functions and six tests; the smallest of the extractions. `gtk4` is still a dependency
after this task.

**Files:**
- Modify: `settings/src/pages/workspaces.rs`

**Interfaces:**
- Consumes: `icedtea_config::Config` (unchanged).
- Produces (`pub`, in `crate::pages::workspaces`):

```rust
pub fn next_workspace_name(existing: &[String]) -> String;
pub fn prune_orphaned_workspace_bindings(cfg: &mut icedtea_config::Config);
```

- [ ] **Step 1: Write the failing test**

Append inside `settings/src/pages/workspaces.rs`'s existing `#[cfg(test)] mod tests`:

```rust
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
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p icedtea-settings --lib pages::workspaces`
Expected: FAIL — `function 'next_workspace_name' is private`.

- [ ] **Step 3: Implement**

In `settings/src/pages/workspaces.rs`, change the two signatures (bodies and doc comments
unchanged):

```rust
pub fn next_workspace_name(existing: &[String]) -> String {
```

```rust
pub fn prune_orphaned_workspace_bindings(cfg: &mut icedtea_config::Config) {
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p icedtea-settings --lib pages::workspaces`
Expected: PASS — 7 tests (the 6 existing plus `the_pure_surface_is_public`).

- [ ] **Step 5: Commit**

```bash
cd /home/joseph/Projects/icedtea/.claude/worktrees/m5
cargo fmt --all && cargo fmt --all --check
cargo clippy -p icedtea-settings --all-targets -- -D warnings
git add settings/src/pages/workspaces.rs
git commit -m "$(cat <<'MSG'
refactor(settings): publish the workspaces pure core

next_workspace_name and prune_orphaned_workspace_bindings become pub so
app.rs's WorkspaceAdded/WorkspaceRemoved arms can call them once the GTK
build() is gone. Bodies and their six tests are untouched.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
MSG
)"
```

---

## Task 4: Add the `icedtea-ui` dependency and re-type the Appearance colour helpers

`icedtea-ui` and `crossbeam-channel` join `settings/Cargo.toml`; `gtk4`/`gio`/`glib` stay for
now (Task 11 removes them). `appearance.rs`'s two GDK-typed colour functions become four
toolkit-typed ones, and the GTK `build()` keeps working through a two-line adapter at each of
its three colour-button call sites.

**Files:**
- Modify: `settings/Cargo.toml`
- Modify: `settings/src/pages/appearance.rs`

**Interfaces:**
- Consumes: `icedtea_ui::css::value::Rgba` (`{ r, g, b, a }`, all `f32`, `0..=1`),
  `icedtea_ui::widgets::color_dialog::ColorDialogC::{pack, unpack}`
  (`pack(Rgba) -> f64` / `unpack(f64) -> Rgba`, M3 P5-D23), `crate::model::valid_hex`.
- Produces (all `pub`, in `crate::pages::appearance`):

```rust
pub fn hex_to_rgba(s: &str) -> icedtea_ui::css::value::Rgba;
pub fn rgba_to_hex(c: icedtea_ui::css::value::Rgba) -> String;
pub fn hex_to_packed(s: &str) -> f64;
pub fn packed_to_hex(packed: f64) -> String;
```

- [ ] **Step 1: Write the failing tests**

Replace `settings/src/pages/appearance.rs`'s `#[cfg(test)] mod tests` body with:

```rust
#[cfg(test)]
mod tests {
    use super::{hex_to_packed, hex_to_rgba, packed_to_hex, rgba_to_hex};

    /// Mutation check: make `hex_to_rgba` divide by 256.0 instead of 255.0;
    /// this round trip fails on `#ffffff`. Restore.
    #[test]
    fn hex_rgba_round_trips() {
        for hex in ["#1e1e2e", "#cdd6f4", "#89b4fa", "#000000", "#ffffff"] {
            let rgba = hex_to_rgba(hex);
            assert_eq!(rgba_to_hex(rgba), hex);
        }
    }

    #[test]
    fn invalid_hex_falls_back_to_black_without_panicking() {
        let black = hex_to_rgba("not-a-color");
        assert_eq!(rgba_to_hex(black), "#000000");
        assert_eq!(black.a, 1.0, "the fallback is opaque black, not transparent");
    }

    /// A `ColorDialogButton` carries its colour as a packed f64 (M3 P5-D23),
    /// so the page's hex strings have to survive that packing exactly.
    ///
    /// Mutation check: swap `pack`/`unpack` in `packed_to_hex`; this fails.
    #[test]
    fn packed_round_trips_through_the_color_dialog_packing() {
        for hex in ["#1e1e2e", "#cdd6f4", "#89b4fa", "#000000", "#ffffff"] {
            assert_eq!(packed_to_hex(hex_to_packed(hex)), hex);
        }
    }

    #[test]
    fn a_nonsense_packed_value_is_black_not_a_panic() {
        assert_eq!(packed_to_hex(f64::NAN), "#000000");
        assert_eq!(packed_to_hex(-1.0), "#000000");
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p icedtea-settings --lib pages::appearance`
Expected: FAIL — `unresolved import 'icedtea_ui'` / `cannot find function 'hex_to_packed'`.

- [ ] **Step 3: Add the dependencies**

In `settings/Cargo.toml`, add to `[dependencies]` (leave every existing line in place):

```toml
icedtea-ui = { path = "../ui" }
crossbeam-channel.workspace = true
```

- [ ] **Step 4: Implement the four conversions**

In `settings/src/pages/appearance.rs`, replace the two GDK functions with:

```rust
use icedtea_ui::css::value::Rgba;
use icedtea_ui::widgets::color_dialog::ColorDialogC;

/// Parse a `#RRGGBB` string into an opaque toolkit colour; falls back to
/// opaque black for anything `valid_hex` rejects (should not happen for
/// values this page itself wrote, but keeps `view` panic-free against a
/// hand-edited db).
///
/// Same parse as the GDK version it replaces — `model::valid_hex` then
/// `u8::from_str_radix` per channel — with `icedtea_ui`'s colour type in
/// place of `gdk::RGBA`.
#[must_use]
pub fn hex_to_rgba(s: &str) -> Rgba {
    if !valid_hex(s) {
        return Rgba { r: 0.0, g: 0.0, b: 0.0, a: 1.0 };
    }
    let digits = &s[1..];
    let byte = |i: usize| u8::from_str_radix(&digits[i..i + 2], 16).unwrap_or(0);
    Rgba {
        r: f32::from(byte(0)) / 255.0,
        g: f32::from(byte(2)) / 255.0,
        b: f32::from(byte(4)) / 255.0,
        a: 1.0,
    }
}

/// Format a colour's channels (alpha ignored — the palette has no
/// transparency concept) as `#RRGGBB`.
#[must_use]
pub fn rgba_to_hex(c: Rgba) -> String {
    let chan = |v: f32| {
        if v.is_nan() { 0 } else { (v.clamp(0.0, 1.0) * 255.0).round() as u8 }
    };
    format!("#{:02x}{:02x}{:02x}", chan(c.r), chan(c.g), chan(c.b))
}

/// The value a `color_dialog_button` carries for `s` (M3 P5-D23: the colour
/// rides `PropName::Value` as a packed `f64`).
#[must_use]
pub fn hex_to_packed(s: &str) -> f64 {
    ColorDialogC::pack(hex_to_rgba(s))
}

/// The hex a `color_dialog_button`'s `on_value_changed` payload means.
/// `ColorDialogC::unpack` already yields opaque black for a non-finite or
/// out-of-range value, so this never panics on a hostile model.
#[must_use]
pub fn packed_to_hex(packed: f64) -> String {
    rgba_to_hex(ColorDialogC::unpack(packed))
}
```

Then fix the three GTK call sites inside `build()` — each currently does
`button.set_rgba(&hex_to_rgba(&hex))` and reads back `rgba_to_hex(&button.rgba())`:

```rust
// writing a colour into a gtk4::ColorDialogButton
let c = hex_to_rgba(&hex);
button.set_rgba(&RGBA::new(c.r, c.g, c.b, c.a));

// reading one back out
let g = button.rgba();
let hex = rgba_to_hex(Rgba { r: g.red(), g: g.green(), b: g.blue(), a: g.alpha() });
```

(these adapters die with `build()` at Task 11).

- [ ] **Step 5: Run the tests**

Run: `cargo test -p icedtea-settings --lib pages::appearance`
Expected: PASS — 4 tests.

- [ ] **Step 6: Commit**

```bash
cd /home/joseph/Projects/icedtea/.claude/worktrees/m5
cargo fmt --all && cargo fmt --all --check
cargo clippy -p icedtea-settings --all-targets -- -D warnings
git add settings/Cargo.toml settings/src/pages/appearance.rs
git commit -m "$(cat <<'MSG'
refactor(settings): depend on icedtea-ui, re-type the colour helpers

hex_to_rgba/rgba_to_hex kept their algorithm but were typed through
gdk::RGBA. They now speak icedtea_ui::css::value::Rgba, and gain the two
packed-f64 conversions a ColorDialogButton actually carries (M3 P5-D23).
The GTK build() bridges the two colour types at its three call sites until
it is deleted.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
MSG
)"
```

---

## Task 5: `PageId` and the switcher's page list

`pages/mod.rs` gains the page identity the model, the switcher and the stack all agree on.
`Ctx`/`Page` stay until Task 11.

**Files:**
- Modify: `settings/src/pages/mod.rs`

**Interfaces:**
- Consumes: `icedtea_ui::widgets::StackPageInfo { name: Rc<str>, title: Rc<str>, icon: Option<IconRef>, needs_attention: bool }`.
- Produces (in `crate::pages`):

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageId { Appearance, Behavior, Workspaces, Keybindings, Displays }
impl PageId {
    pub const ALL: [PageId; 5];
    #[must_use] pub fn name(self) -> &'static str;
    #[must_use] pub fn title(self) -> &'static str;
    #[must_use] pub fn from_index(i: usize) -> PageId;
    #[must_use] pub fn index(self) -> usize;
}
#[must_use] pub fn page_infos() -> std::rc::Rc<[icedtea_ui::widgets::StackPageInfo]>;
```

- [ ] **Step 1: Write the failing tests**

Append to `settings/src/pages/mod.rs`:

```rust
#[cfg(test)]
mod page_id_tests {
    use super::{PageId, page_infos};

    /// The switcher hands back the index of the button pressed, and the
    /// stack selects by name — so the two orders must be the same one.
    ///
    /// Mutation check: reverse `PageId::ALL`; this test fails on
    /// `from_index(0)`. Restore.
    #[test]
    fn indices_names_and_titles_line_up() {
        assert_eq!(PageId::ALL.len(), 5);
        for (i, page) in PageId::ALL.iter().copied().enumerate() {
            assert_eq!(PageId::from_index(i), page);
            assert_eq!(page.index(), i);
        }
        assert_eq!(PageId::from_index(0), PageId::Appearance);
        assert_eq!(PageId::Appearance.name(), "appearance");
        assert_eq!(PageId::Appearance.title(), "Appearance");
        assert_eq!(PageId::Displays.name(), "displays");
        assert_eq!(PageId::Displays.title(), "Displays");
    }

    /// A `StackSwitcher` selection index arrives from a widget, so an
    /// out-of-range one is untrusted input: it must clamp, never panic.
    #[test]
    fn an_out_of_range_index_clamps_to_the_last_page() {
        assert_eq!(PageId::from_index(99), PageId::Displays);
        assert_eq!(PageId::from_index(usize::MAX), PageId::Displays);
    }

    #[test]
    fn page_infos_mirrors_page_id_all() {
        let infos = page_infos();
        assert_eq!(infos.len(), 5);
        for (info, page) in infos.iter().zip(PageId::ALL) {
            assert_eq!(&*info.name, page.name());
            assert_eq!(&*info.title, page.title());
            assert!(!info.needs_attention);
        }
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p icedtea-settings --lib pages::page_id_tests`
Expected: FAIL — `cannot find type 'PageId' in this scope`.

- [ ] **Step 3: Implement**

Add to `settings/src/pages/mod.rs`, above the existing `Ctx`:

```rust
/// The five settings pages, in switcher order.
///
/// One identity for three consumers that must not disagree: the model's
/// `page` field, the `StackSwitcher`'s selection index, and the `Stack`'s
/// `visible-child-name`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageId {
    Appearance,
    Behavior,
    Workspaces,
    Keybindings,
    Displays,
}

impl PageId {
    /// Every page, in the order the switcher shows them.
    pub const ALL: [PageId; 5] = [
        PageId::Appearance,
        PageId::Behavior,
        PageId::Workspaces,
        PageId::Keybindings,
        PageId::Displays,
    ];

    /// The `stack_page` name this page is selected by.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            PageId::Appearance => "appearance",
            PageId::Behavior => "behavior",
            PageId::Workspaces => "workspaces",
            PageId::Keybindings => "keybindings",
            PageId::Displays => "displays",
        }
    }

    /// The switcher button's label.
    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            PageId::Appearance => "Appearance",
            PageId::Behavior => "Behavior",
            PageId::Workspaces => "Workspaces",
            PageId::Keybindings => "Keybindings",
            PageId::Displays => "Displays",
        }
    }

    /// The page a `StackSwitcher` selection index names. An index past the
    /// end clamps to the last page: the index arrives from a widget, and the
    /// cross-cutting rule is that untrusted input never panics.
    #[must_use]
    pub fn from_index(i: usize) -> PageId {
        PageId::ALL[i.min(PageId::ALL.len() - 1)]
    }

    /// This page's position in [`PageId::ALL`].
    #[must_use]
    pub fn index(self) -> usize {
        PageId::ALL
            .iter()
            .position(|p| *p == self)
            .unwrap_or_default()
    }
}

/// The page list a `stack_switcher` takes.
///
/// A function rather than the contract's `PAGES` constant (deviation P1-D3):
/// `StackPageInfo` holds `Rc<str>`, which cannot appear in a `const`.
#[must_use]
pub fn page_infos() -> std::rc::Rc<[icedtea_ui::widgets::StackPageInfo]> {
    PageId::ALL
        .iter()
        .map(|page| icedtea_ui::widgets::StackPageInfo {
            name: std::rc::Rc::from(page.name()),
            title: std::rc::Rc::from(page.title()),
            icon: None,
            needs_attention: false,
        })
        .collect()
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p icedtea-settings --lib pages::page_id_tests`
Expected: PASS — 3 tests.

- [ ] **Step 5: Commit**

```bash
cd /home/joseph/Projects/icedtea/.claude/worktrees/m5
cargo fmt --all && cargo fmt --all --check
cargo clippy -p icedtea-settings --all-targets -- -D warnings
git add settings/src/pages/mod.rs
git commit -m "$(cat <<'MSG'
feat(settings): add PageId and the switcher's page list

One identity for the model's selected page, the StackSwitcher's selection
index and the Stack's visible-child-name, so the three cannot drift.
from_index clamps rather than panicking: the index comes off a widget.

page_infos() is a function, not the contract's PAGES constant, because
StackPageInfo holds Rc<str> (deviation P1-D3, recorded in contract §6).

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
MSG
)"
```

---

## Task 6: `SettingsModel`, `Msg`, `update`, and the reload worker

The heart of the migration: the model the Elm loop folds, the message type it folds (`Send`, no
`Rc`), the `update` arms for navigation and the footer, and the worker thread that owns the
blocking `ReloadClient` and the `ConfigReloaded` subscription.

**This task is the plan's one oversize outlier — about 530 code lines against a ~400-line
guideline and a 33–363 range across the other ten tasks — so it ships as two commits,
Task 6a and Task 6b**, each independently compiling, tested and reviewable. The naive split
(model in one commit, IPC in the other) does not compile in either order, because
`SettingsModel` holds `WorkerHandles` while `ipc::spawn` and `reload::spawn` send
`crate::app::Msg`. The cut that *is* acyclic runs through the threads, not through the files:

- **Task 6a — the model and the channel half (~300 code lines).** `app.rs` in full
  (`SettingsModel`, `Msg`, `update`), `ipc/mod.rs`'s `WorkerHandles`/`apply`/`handles_for_test`
  and `ipc/reload.rs`'s `ReloadRequest`. Nothing here mentions `crate::app::Msg` from `ipc`, and
  nothing spawns a thread, so the cycle never forms. Six `app::tests` prove it.
- **Task 6b — the worker threads (~230 code lines).** `reload::spawn` (the blocking
  `ReloadClient` loop and the `ConfigReloaded` watcher), `ipc::spawn`, `Inbox::try_recv`
  (deviation P1-D4a) and the one round-trip test that needs all three. `ipc/mod.rs` and
  `ipc/reload.rs` gain items; no signature written in 6a changes.

Every interface below is the union across the two; the "Produces" block is unchanged from what
a single Task 6 would have shipped, and Task 7 sees no difference.


**Files:**
- Create: `settings/src/app.rs`
- Create: `settings/src/ipc/mod.rs`
- Create: `settings/src/ipc/reload.rs`
- Modify: `settings/src/lib.rs`

**Interfaces:**
- Consumes: `icedtea_ui::view::{Cmd, Inbox, InboxSender}` (M5-D2, M5-D3);
  `crate::model::Model` (`load`, `is_dirty`, `revert`, fields `working`/`saved`);
  `crate::compositor_reload::{ReloadClient, ReloadOutcome, apply_and_reload}`;
  `crate::pages::PageId`; `crate::pages::displays::state::DisplaysState`;
  `icedtea_contract::{COMPOSITOR_BUS_NAME, COMPOSITOR_PATH}`.
- Produces:

```rust
// settings/src/app.rs
pub struct SettingsModel {
    pub model: crate::model::Model,
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
impl SettingsModel {
    pub fn new(db_path: std::path::PathBuf, workers: crate::ipc::WorkerHandles) -> Self;
    #[must_use] pub fn is_dirty(&self) -> bool;
}

#[derive(Clone, Debug)]
pub enum Msg {
    PageSelected(usize),
    Apply,
    Revert,
    Applied(Result<crate::compositor_reload::ReloadOutcome, String>),
    ConfigReloaded,
}

pub fn update(m: &mut SettingsModel, msg: Msg) -> icedtea_ui::view::Cmd<Msg>;

// settings/src/ipc/mod.rs
// P1 half only: `portal` / `choose_wallpaper` are P2's (deviation P1-D7).
#[derive(Clone)]
pub struct WorkerHandles { /* private senders: `reload` at P1, `portal` added by P2 */ }
impl WorkerHandles {
    pub fn apply(&self, cfg: icedtea_config::Config, db_path: std::path::PathBuf);
    // P2 adds, verbatim from contract §2.4:
    // pub fn choose_wallpaper(&self, current: Option<std::path::PathBuf>);
}
/// Signature is already §2.4's final one; P2 only adds the portal worker to its body.
pub fn spawn(tx: icedtea_ui::view::InboxSender<crate::app::Msg>) -> WorkerHandles;
/// Additive, tests only (deviation P1-D8); P2 extends the tuple with the portal receiver.
#[must_use] pub fn handles_for_test() -> (WorkerHandles, crossbeam_channel::Receiver<crate::ipc::reload::ReloadRequest>);

// settings/src/ipc/reload.rs
pub enum ReloadRequest { Apply { cfg: icedtea_config::Config, db_path: std::path::PathBuf } }
pub fn spawn(rx: crossbeam_channel::Receiver<ReloadRequest>,
             tx: icedtea_ui::view::InboxSender<crate::app::Msg>) -> std::thread::JoinHandle<()>;
```


---

### Task 6a — the model, the messages, `update`, and the handles

- [ ] **Step 1: Write the failing tests**


Create `settings/src/app.rs` containing only its test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::{Msg, SettingsModel, update};
    use crate::compositor_reload::ReloadOutcome;
    use crate::pages::PageId;

    /// `Msg` crosses a thread boundary on the inbox (M5-D2), so it must be
    /// `Send` — which is what forbids `Rc` in a payload.
    #[test]
    fn msg_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<Msg>();
    }

    fn model() -> (SettingsModel, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("config.redb");
        let (workers, _rx) = crate::ipc::handles_for_test();
        (SettingsModel::new(db, workers), dir)
    }

    /// Mutation check: make `Msg::PageSelected` ignore its index; this fails
    /// on the `Workspaces` assertion. Restore.
    #[test]
    fn selecting_a_page_moves_the_model_and_cancels_a_capture() {
        let (mut m, _dir) = model();
        m.capturing = Some("close".to_string());
        update(&mut m, Msg::PageSelected(2));
        assert_eq!(m.page, PageId::Workspaces);
        assert_eq!(m.capturing, None, "a page switch cancels an armed capture");
    }

    /// Dirty is computed from working != saved, never a flag (spec §5.1).
    #[test]
    fn dirty_is_computed_and_revert_clears_it() {
        let (mut m, _dir) = model();
        assert!(!m.is_dirty(), "a freshly loaded model is clean");
        m.model.working.appearance.accent = "#ff00aa".to_string();
        assert!(m.is_dirty());
        update(&mut m, Msg::Revert);
        assert!(!m.is_dirty(), "Revert reloads both halves from disk");
        assert_eq!(m.status, "");
    }

    /// The three Apply outcomes are the three `main.rs:136-158` shipped.
    #[test]
    fn the_three_apply_outcomes_set_status_and_the_saved_snapshot() {
        let (mut m, _dir) = model();
        m.model.working.appearance.accent = "#ff00aa".to_string();

        update(&mut m, Msg::Applied(Ok(ReloadOutcome::Reloaded)));
        assert!(!m.is_dirty(), "a reload snapshots working into saved");
        assert_eq!(m.status, "Applied");

        m.model.working.appearance.accent = "#00ff00".to_string();
        update(&mut m, Msg::Applied(Ok(ReloadOutcome::CompositorAbsent)));
        assert!(!m.is_dirty());
        assert_eq!(m.status, "Saved; will apply when the compositor starts");

        m.model.working.appearance.accent = "#0000ff".to_string();
        update(&mut m, Msg::Applied(Err("disk on fire".to_string())));
        assert!(m.is_dirty(), "a failed write leaves the edit unsaved");
        assert_eq!(m.status, "Failed to save: disk on fire");
    }

    /// `update` never blocks and never calls D-Bus: Apply hands the work to
    /// the worker through `Cmd::Task` (spec D8).
    ///
    /// Mutation check: make the `Apply` arm return `Cmd::None`; this fails.
    #[test]
    fn apply_queues_a_task_and_says_so() {
        let (mut m, _dir) = model();
        m.model.working.appearance.accent = "#ff00aa".to_string();
        let cmd = update(&mut m, Msg::Apply);
        assert!(
            matches!(cmd, icedtea_ui::view::Cmd::Task(_)),
            "Apply must go out through Cmd::Task, never inside update"
        );
        assert_eq!(m.status, "Applying\u{2026}");
    }

    #[test]
    fn a_config_reloaded_signal_only_notes_itself() {
        let (mut m, _dir) = model();
        m.model.working.appearance.accent = "#ff00aa".to_string();
        update(&mut m, Msg::ConfigReloaded);
        assert!(m.is_dirty(), "an external reload must not touch the working copy");
        assert_eq!(m.status, "Compositor reloaded its configuration");
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p icedtea-settings --lib app::`
Expected: FAIL — `file not found for module 'app'` until `lib.rs` declares it, then
`cannot find function 'update' in this scope`.

- [ ] **Step 3: Implement the request type and `WorkerHandles`**

`settings/src/ipc/reload.rs`, in full for now — the request type only; its worker thread is
Task 6b's, and this file is why 6a compiles without one:

```rust
//! The reload worker: one OS thread owning the blocking `ReloadClient` for
//! outbound `ReloadConfig` calls and a second, async connection subscribed to
//! the compositor's `ConfigReloaded` signal.
//!
//! `update` never blocks and never calls D-Bus (spec D8), so every save+reload
//! arrives here as a [`ReloadRequest`] and every answer leaves as a
//! `Msg::Applied` on the inbox.

/// What the loop thread asks the worker for.
pub enum ReloadRequest {
    /// Write `cfg` to `db_path`, then best-effort `ReloadConfig`.
    Apply {
        cfg: icedtea_config::Config,
        db_path: std::path::PathBuf,
    },
}
```

`settings/src/ipc/mod.rs`, above its test module — everything except `spawn`, which is
Task 6b's:

```rust
//! Worker threads and the handles `update` reaches them through.
//!
//! Everything outbound is fire-and-forget: `update` pushes onto a channel and
//! returns, and the answer arrives as a `Msg` on the app's inbox.

pub mod reload;

/// Everything `update` needs to reach a worker. Cloneable; it lives on the
/// loop thread, so nothing here has to be `Send`.
///
/// P1 ships the reload half. Contract §2.4 also specifies
/// `portal: crossbeam_channel::Sender<PortalRequest>` and
/// `choose_wallpaper(&self, current: Option<PathBuf>)`; those land with the
/// portal worker in P2 (§2.6, and §5's table assigns "§2.6 portal worker | P2"),
/// recorded as deviation P1-D7. The field is private and `spawn`'s signature is
/// already final, so adding it is a P2-local edit.
#[derive(Clone)]
pub struct WorkerHandles {
    reload: crossbeam_channel::Sender<reload::ReloadRequest>,
    // P2: portal: crossbeam_channel::Sender<portal::PortalRequest>,
}

impl WorkerHandles {
    /// Queue a save+reload. Never blocks: the answer arrives as
    /// `Msg::Applied` on the inbox. A closed channel (the worker died) is
    /// logged once and dropped — the window stays usable.
    pub fn apply(&self, cfg: icedtea_config::Config, db_path: std::path::PathBuf) {
        if self
            .reload
            .send(reload::ReloadRequest::Apply { cfg, db_path })
            .is_err()
        {
            tracing::warn!("the reload worker is gone; Apply was dropped");
        }
    }
}

/// Handles wired to a receiver the caller keeps, with no worker thread behind
/// them — what a `SettingsModel` unit test constructs (deviation P1-D8).
///
/// P2 extends the returned tuple with the portal receiver, in this same shape.
#[must_use]
pub fn handles_for_test() -> (
    WorkerHandles,
    crossbeam_channel::Receiver<reload::ReloadRequest>,
) {
    let (reload_tx, reload_rx) = crossbeam_channel::unbounded();
    (WorkerHandles { reload: reload_tx }, reload_rx)
}
```

Add `crossbeam-channel.workspace = true` to `settings/Cargo.toml`'s `[dependencies]` in this
step if Task 4 has not already.

- [ ] **Step 4: Implement the model, the messages and `update`**

`settings/src/app.rs`, above its test module:


`settings/src/app.rs`, above its test module:

```rust
//! The settings application: the model the loop folds, its messages, and
//! `update`.
//!
//! `view` lives here too (see below); every page contributes one `pub fn
//! view(&SettingsModel) -> View<Msg>` from its own module.

use std::rc::Rc;

use icedtea_ui::view::{Cmd, View};

use crate::compositor_reload::ReloadOutcome;
use crate::pages::PageId;
use crate::pages::displays::state::DisplaysState;

/// Everything the settings window shows, and nothing it does not.
///
/// `model` is `model.rs`'s untouched working/saved pair: dirty is
/// `working != saved`, computed on every `view`, never a flag. The GTK
/// `Ctx`/`Page`/`populating` machinery has no equivalent here — a programmatic
/// widget write cannot happen on an Elm loop, so there is nothing to guard.
pub struct SettingsModel {
    pub model: crate::model::Model,
    pub db_path: std::path::PathBuf,
    pub page: PageId,
    pub status: String,
    /// The action whose binding is being captured, if any (P3 arms it).
    pub capturing: Option<String>,
    /// Conflicting bindings, recomputed from `working` after every edit.
    pub conflicts: Vec<(icedtea_config::KeyCombo, Vec<String>)>,
    pub wallpaper_text: String,
    pub wallpaper_error: Option<String>,
    pub portal_available: bool,
    pub displays: DisplaysState,
    pub displays_status: String,
    pub displays_in_flight: bool,
    pub outputs_available: bool,
    /// Worker senders, so `update` can `Cmd::Task` onto them.
    pub workers: crate::ipc::WorkerHandles,
}

impl SettingsModel {
    /// Load the working copy from `db_path`.
    #[must_use]
    pub fn new(db_path: std::path::PathBuf, workers: crate::ipc::WorkerHandles) -> Self {
        let model = crate::model::Model::load(&db_path);
        let wallpaper_text = model.working.appearance.wallpaper.clone().unwrap_or_default();
        let conflicts = crate::model::duplicate_bindings(&model.working);
        SettingsModel {
            model,
            db_path,
            page: PageId::Appearance,
            status: String::new(),
            capturing: None,
            conflicts,
            wallpaper_text,
            wallpaper_error: None,
            portal_available: true,
            displays: DisplaysState::new(),
            displays_status: String::new(),
            displays_in_flight: false,
            outputs_available: false,
            workers,
        }
    }

    /// `working != saved`, the one dirty rule.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.model.is_dirty()
    }
}

/// Everything that can change the model.
///
/// `Msg` crosses a thread boundary on the inbox, so it is `Send` and every
/// payload is owned or `Arc` — never `Rc` (M5-D2). P2, P3 and P4 append their
/// pages' variants (deviation P1-D1).
#[derive(Clone, Debug)]
pub enum Msg {
    /// A `StackSwitcher` button, by index.
    PageSelected(usize),
    Apply,
    Revert,
    /// From the reload worker, through the inbox.
    Applied(Result<ReloadOutcome, String>),
    /// The compositor emitted `ConfigReloaded`.
    ConfigReloaded,
}

const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<Msg>();
};

/// Fold one message into the model.
///
/// Never blocks and never calls D-Bus: outbound work leaves through
/// `Cmd::Task`, which `App` runs on the loop thread after the fold.
pub fn update(m: &mut SettingsModel, msg: Msg) -> Cmd<Msg> {
    match msg {
        Msg::PageSelected(index) => {
            m.page = PageId::from_index(index);
            // Leaving the Keybindings page disarms a capture — the GTK
            // `connect_unmap`/`EventControllerFocus` reset, in one line.
            m.capturing = None;
            Cmd::None
        }
        Msg::Apply => {
            let cfg = m.model.working.clone();
            let db_path = m.db_path.clone();
            let workers = m.workers.clone();
            m.status = "Applying\u{2026}".to_string();
            Cmd::Task(Rc::new(move || workers.apply(cfg.clone(), db_path.clone())))
        }
        Msg::Revert => {
            m.model.revert(&m.db_path);
            m.wallpaper_text = m.model.working.appearance.wallpaper.clone().unwrap_or_default();
            m.wallpaper_error = None;
            m.conflicts = crate::model::duplicate_bindings(&m.model.working);
            m.status = String::new();
            // Displays is deliberately not reverted: it owns its own Revert
            // and never routes through this working copy (main.rs:165-174).
            Cmd::None
        }
        Msg::Applied(Ok(ReloadOutcome::Reloaded)) => {
            m.model.saved = m.model.working.clone();
            m.status = "Applied".to_string();
            Cmd::None
        }
        Msg::Applied(Ok(ReloadOutcome::CompositorAbsent)) => {
            m.model.saved = m.model.working.clone();
            m.status = "Saved; will apply when the compositor starts".to_string();
            Cmd::None
        }
        Msg::Applied(Err(err)) => {
            // The write failed: the working copy stays dirty on purpose.
            m.status = format!("Failed to save: {err}");
            Cmd::None
        }
        Msg::ConfigReloaded => {
            m.status = "Compositor reloaded its configuration".to_string();
            Cmd::None
        }
    }
}
```

`settings/src/lib.rs` declares the new modules:

```rust
pub mod app;
pub mod compositor_reload;
pub mod ipc;
pub mod model;
pub mod outputs;
pub mod pages;
```

- [ ] **Step 5: Run the tests**

```bash
cd /home/joseph/Projects/icedtea/.claude/worktrees/m5
cargo test -p icedtea-settings --lib app::
cargo test -p icedtea-settings
```

Expected: PASS — 6 `app::tests`, whole suite green. `ipc` has no test of its own yet: its
round trip is Task 6b's, because there is no worker thread to round-trip through.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all && cargo fmt --all --check
cargo clippy -p icedtea-settings --all-targets -- -D warnings
git add settings/Cargo.toml settings/src/lib.rs settings/src/app.rs settings/src/ipc/
git commit -m "$(cat <<'MSG'
feat(settings): the model, its messages, update, and the worker handles

SettingsModel wraps model.rs's untouched working/saved pair and adds the UI
state the GTK Ctx/Page machinery used to scatter across five pages. Msg is
Send with no Rc payloads, which is what will let a worker thread post onto
the app's inbox in the next commit.

update never blocks and never calls D-Bus: Apply returns Cmd::Task onto a
crossbeam channel that WorkerHandles owns. The three Apply outcomes and
their saved-snapshot rules are main.rs:136-158's, unchanged, and Revert
deliberately leaves Displays alone (main.rs:165-174).

WorkerHandles ships its reload half only; the portal sender and
choose_wallpaper that contract §2.4 lists beside it land with the portal
worker in P2 (deviation P1-D7, contract §6). handles_for_test is additive
and test-only (deviation P1-D8).

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
MSG
)"
```

---

### Task 6b — the worker threads, `ipc::spawn` and `Inbox::try_recv`

- [ ] **Step 1: Write the failing test**

Add to `settings/src/ipc/mod.rs` (Task 6a created it) its test module:

```rust
#[cfg(test)]
mod tests {
    use std::time::Duration;

    /// The whole worker round trip with no compositor on the bus: the request
    /// goes out on a channel, the config lands on disk, and `Msg::Applied`
    /// comes back through the inbox — `ReloadOutcome::CompositorAbsent`
    /// unless something really is listening.
    ///
    /// Mutation check: drop the `tx.send(...)` in `reload::spawn`'s request
    /// arm; this test times out and fails. Restore.
    #[test]
    fn the_reload_worker_writes_the_config_and_answers_on_the_inbox() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("config.redb");
        let (inbox, tx) = icedtea_ui::view::Inbox::<crate::app::Msg>::new().expect("inbox");
        let handles = super::spawn(tx);

        let mut cfg = icedtea_config::default_config();
        cfg.appearance.accent = "#ff00aa".to_string();
        handles.apply(cfg, db.clone());

        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut applied = None;
        while std::time::Instant::now() < deadline && applied.is_none() {
            applied = inbox.try_recv();
            std::thread::sleep(Duration::from_millis(20));
        }
        match applied {
            Some(crate::app::Msg::Applied(Ok(_))) => {}
            other => panic!("expected Msg::Applied(Ok(..)), got {other:?}"),
        }
        let on_disk = icedtea_config::load_or_default(&db);
        assert_eq!(on_disk.appearance.accent, "#ff00aa");
    }
}
```


- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p icedtea-settings --lib ipc::`
Expected: FAIL — `cannot find function 'spawn' in module 'super'`, and
`no method named 'try_recv' found for struct 'Inbox'`.

- [ ] **Step 3: Land `Inbox::try_recv` in `ui/` (deviation P1-D4a)**


First check whether P0 already shipped it:

```bash
cd /home/joseph/Projects/icedtea/.claude/worktrees/m5
grep -n "fn try_recv" ui/src/view/inbox.rs || echo "not present — add it"
```

If it is absent, add to `ui/src/view/inbox.rs`, inside `impl<Msg: Send + 'static> Inbox<Msg>`:

```rust
    /// Take one queued message, if any, without waiting.
    ///
    /// The counterpart of the drain [`App::run`](crate::view::App::run) does
    /// once per frame, exposed so a worker thread's round trip can be tested
    /// without a compositor. Consumes one wake byte per message taken, so a
    /// caller that drains the inbox to empty leaves the pipe empty too and
    /// the loop does not wake for messages that are already gone.
    #[must_use]
    pub fn try_recv(&self) -> Option<Msg> {
        let msg = self.rx.try_recv().ok()?;
        let mut byte = [0u8; 1];
        // A short read or `WouldBlock` is fine: the channel is the queue, the
        // pipe is only the wakeup, and one spurious wake costs one empty frame.
        let _ = rustix::io::read(&self.read, &mut byte[..]);
        Some(msg)
    }
```

and, in `ui/tests/ingress.rs`:

```rust
/// Mutation check: make `try_recv` return `None` unconditionally; this test
/// fails. Restore.
#[test]
fn try_recv_takes_messages_in_send_order_and_then_reports_empty() {
    let (inbox, tx) = icedtea_ui::view::Inbox::<u32>::new().expect("inbox");
    tx.send(1).expect("send 1");
    tx.send(2).expect("send 2");
    assert_eq!(inbox.try_recv(), Some(1));
    assert_eq!(inbox.try_recv(), Some(2));
    assert_eq!(inbox.try_recv(), None);
}
```

Run: `cargo test -p icedtea-ui --test ingress try_recv`
Expected: PASS.

- [ ] **Step 4: Implement the reload worker**

`settings/src/ipc/reload.rs` — Task 6a wrote its module doc and `ReloadRequest`; this step
adds the imports, the two consts, `spawn` and `watch_config_reloaded`. The file in full
afterwards:

```rust
//! The reload worker: one OS thread owning the blocking `ReloadClient` for
//! outbound `ReloadConfig` calls and a second, async connection subscribed to
//! the compositor's `ConfigReloaded` signal.
//!
//! `update` never blocks and never calls D-Bus (spec D8), so every save+reload
//! arrives here as a [`ReloadRequest`] and every answer leaves as a
//! `Msg::Applied` on the inbox.

use icedtea_ui::view::InboxSender;

use crate::app::Msg;
use crate::compositor_reload::{ReloadClient, apply_and_reload};

/// What the loop thread asks the worker for.
pub enum ReloadRequest {
    /// Write `cfg` to `db_path`, then best-effort `ReloadConfig`.
    Apply {
        cfg: icedtea_config::Config,
        db_path: std::path::PathBuf,
    },
}

const COMPOSITOR_IFACE: &str = "org.icedtea.Compositor";
/// The signal member name is **not** PascalCase — zbus exposes `#[interface]`
/// *methods* in PascalCase (`ReloadConfig`) but leaves signal names alone
/// (contract §0, and `shell/src/clip_client.rs:37,41` proves both halves).
const CONFIG_RELOADED: &str = "ConfigReloaded";

/// Start the worker. It returns when every `Inbox` is dropped (the app exited)
/// or the request channel closes.
///
/// Unlike shell's worker it does **not** `process::exit` on a D-Bus failure:
/// settings is a foreground app with no `Restart=always` unit, and a dead
/// reload worker must leave the window usable.
pub fn spawn(
    rx: crossbeam_channel::Receiver<ReloadRequest>,
    tx: InboxSender<Msg>,
) -> std::thread::JoinHandle<()> {
    let signal_tx = tx.clone();
    // The signal subscription is its own thread so a long blocking
    // `ReloadConfig` cannot delay a `ConfigReloaded` and vice versa.
    std::thread::Builder::new()
        .name("settings-config-reloaded".to_string())
        .spawn(move || watch_config_reloaded(&signal_tx))
        .map_or_else(
            |err| tracing::warn!(%err, "no ConfigReloaded watcher thread"),
            |handle| drop(handle),
        );

    std::thread::Builder::new()
        .name("settings-reload".to_string())
        .spawn(move || {
            let client = ReloadClient::new();
            while let Ok(request) = rx.recv() {
                let ReloadRequest::Apply { cfg, db_path } = request;
                let answer = apply_and_reload(&cfg, &db_path, &client)
                    .map_err(|err| err.to_string());
                if tx.send(Msg::Applied(answer)).is_err() {
                    // Every Inbox is gone: the app exited.
                    return;
                }
            }
        })
        .expect("spawning the settings reload worker")
}

/// Subscribe to `org.icedtea.Compositor`'s `ConfigReloaded` and post one
/// `Msg::ConfigReloaded` per signal.
///
/// Every failure — no session bus, no compositor, a stream error — ends the
/// watcher quietly: the app stays usable without it. Nothing here panics on a
/// malformed signal body, because the body is never deserialised.
fn watch_config_reloaded(tx: &InboxSender<Msg>) {
    let outcome = zbus::block_on(async {
        let conn = zbus::Connection::session().await?;
        let rule = zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .interface(COMPOSITOR_IFACE)?
            .path(icedtea_contract::COMPOSITOR_PATH)?
            .build();
        let mut stream = zbus::MessageStream::for_match_rule(rule, &conn, None).await?;
        use futures_util::StreamExt as _;
        while let Some(Ok(msg)) = stream.next().await {
            let is_reloaded = msg
                .header()
                .member()
                .is_some_and(|member| member.as_str() == CONFIG_RELOADED);
            if is_reloaded && tx.send(Msg::ConfigReloaded).is_err() {
                break;
            }
        }
        Ok::<(), zbus::Error>(())
    });
    if let Err(err) = outcome {
        tracing::info!(%err, "no ConfigReloaded subscription; settings runs without it");
    }
}
```

`futures-util` is already a dependency of the workspace's D-Bus code
(`shell/Cargo.toml`); add `futures-util = "0.3"` to `settings/Cargo.toml`'s `[dependencies]`
in this step — it is the one dependency §2.1's table does not list, and it is required by
`MessageStream`'s `StreamExt::next`. Record it in the commit message.

- [ ] **Step 5: Add `ipc::spawn`**

`settings/src/ipc/mod.rs` gains, below `WorkerHandles`'s `impl` — Task 6a's items are
unchanged:

```rust
use icedtea_ui::view::InboxSender;

/// Start every worker against `tx` and return their handles.
///
/// P1 starts one: the reload worker. P2 starts the portal worker here too, from
/// the same `tx` (deviation P1-D7); this signature is contract §2.4's final one
/// and does not change when it does.
#[must_use]
pub fn spawn(tx: InboxSender<crate::app::Msg>) -> WorkerHandles {
    let (reload_tx, reload_rx) = crossbeam_channel::unbounded();
    // The join handle is deliberately dropped: the worker's lifetime is the
    // process's, and it stops itself when the channel or the inbox closes.
    drop(reload::spawn(reload_rx, tx));
    WorkerHandles { reload: reload_tx }
}
```

- [ ] **Step 6: Run the tests**

```bash
cd /home/joseph/Projects/icedtea/.claude/worktrees/m5
cargo test -p icedtea-settings --lib ipc::
cargo test -p icedtea-settings
cargo test -p icedtea-ui --test ingress
```

Expected: PASS — 6 `app::tests`, 1 `ipc::tests`, whole suite green.

- [ ] **Step 7: Commit**


```bash
cargo fmt --all && cargo fmt --all --check
cargo clippy -p icedtea-settings --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings
cargo test -p icedtea-ui --test ingress
git add settings/Cargo.toml settings/src/lib.rs settings/src/app.rs settings/src/ipc/ ui/src/view/inbox.rs ui/tests/ingress.rs
git commit -m "$(cat <<'MSG'
feat(settings): the reload worker and its inbox round trip

SettingsModel wraps model.rs's untouched working/saved pair and adds the UI
state the GTK Ctx/Page machinery used to scatter across five pages. Msg is
Send with no Rc payloads, which is what lets a worker thread post onto the
app's inbox.

update never blocks and never calls D-Bus: Apply returns Cmd::Task onto a
crossbeam channel, and the worker thread — one for the blocking
ReloadConfig call, one for the ConfigReloaded subscription — answers with
Msg::Applied / Msg::ConfigReloaded. The three Apply outcomes and their
saved-snapshot rules are main.rs:136-158's, unchanged.

Adds futures-util, which zbus's MessageStream needs for StreamExt::next,
and Inbox::try_recv in icedtea-ui (deviation P1-D4a, contract §6) so the
worker round trip is testable without a compositor.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
MSG
)"
```

---

## Task 7: `view` — root, navigation, stack and footer

The pure `view(&SettingsModel) -> View<Msg>` the contract §2.3 spells: a vertical box of
`stack_switcher` + `stack` of five `stack_page`s + footer. Each page contributes a
`pub fn view(&SettingsModel) -> View<Msg>` from its own module; P1's five are one labelled
placeholder line each, replaced wholesale by P2/P3/P4 — they are real, painting views, not
stubs, so the rest-state gate is meaningful from this task onward.

**Files:**
- Modify: `settings/src/app.rs`
- Modify: `settings/src/pages/{appearance,behavior,workspaces,keybindings}.rs`
- Modify: `settings/src/pages/displays/mod.rs`

**Interfaces:**
- Consumes: `icedtea_ui::view::builders::{box_, stack, stack_page, stack_switcher, StackExt}`,
  `icedtea_ui::widgets::{button, label}`, `icedtea_ui::layout::Orientation`,
  `icedtea_ui::view::Align`, `crate::pages::{PageId, page_infos}`, `SettingsModel`, `Msg`.
- Produces:

```rust
// settings/src/app.rs
pub fn view(m: &SettingsModel) -> View<Msg>;
fn nav(m: &SettingsModel) -> View<Msg>;
fn footer(m: &SettingsModel) -> View<Msg>;

// one per page module, all `pub`
pub fn crate::pages::appearance::view(m: &crate::app::SettingsModel) -> View<crate::app::Msg>;
pub fn crate::pages::behavior::view(m: &crate::app::SettingsModel) -> View<crate::app::Msg>;
pub fn crate::pages::workspaces::view(m: &crate::app::SettingsModel) -> View<crate::app::Msg>;
pub fn crate::pages::keybindings::view(m: &crate::app::SettingsModel) -> View<crate::app::Msg>;
pub fn crate::pages::displays::view(m: &crate::app::SettingsModel) -> View<crate::app::Msg>;
```

**Widget ids (normative for every P1 gate):** `root`, `nav`, `pages`, `footer`, `status`,
`revert`, `apply`, and one per page body: `appearance_page`, `behavior_page`,
`workspaces_page`, `keybindings_page`, `displays_page`.

- [ ] **Step 1: Write the failing tests**

Append to `settings/src/app.rs`'s `mod tests`:

```rust
    use icedtea_ui::css::node::Node;

    /// Every node the tree lays out, by id.
    fn ids(probe: &icedtea_ui::view::app::Probe<Msg>) -> Vec<String> {
        let mut out = Vec::new();
        for node in probe.root().descendants() {
            if let Some(id) = node.id() {
                out.push(id.to_string());
            }
        }
        out
    }

    fn probe_of(m: SettingsModel) -> icedtea_ui::view::app::Probe<Msg> {
        icedtea_ui::view::App::new(m, update, view)
            .probe(
                (480, 420),
                icedtea_ui::app::compile_theme(&icedtea_ui::app::ThemeSource::Bundled),
                icedtea_ui::text::FontDatabase::probe_only(),
                icedtea_ui::icons::IconTheme::default(),
                std::rc::Rc::new(icedtea_ui::anim::ManualClock::new()),
            )
            .expect("the settings tree lays out")
    }

    /// The frame every page hangs off. Mutation check: drop `footer(m)` from
    /// `view`; this test fails on `apply`. Restore.
    #[test]
    fn the_root_carries_the_nav_the_stack_and_the_footer() {
        let (m, _dir) = model();
        let probe = probe_of(m);
        let ids = ids(&probe);
        for wanted in ["root", "nav", "pages", "footer", "status", "revert", "apply"] {
            assert!(ids.contains(&wanted.to_string()), "missing #{wanted} in {ids:?}");
        }
    }

    /// Only the selected page's body is in the tree the stack shows, and the
    /// selection follows the model.
    #[test]
    fn the_stack_shows_the_selected_page() {
        let (mut m, _dir) = model();
        assert_eq!(m.page, PageId::Appearance);
        let ids = ids(&probe_of(m));
        assert!(ids.contains(&"appearance_page".to_string()));

        let (mut m2, _dir2) = model();
        m2.page = PageId::Displays;
        let ids = ids(&probe_of(m2));
        assert!(ids.contains(&"displays_page".to_string()));
        let _ = &mut m;
    }

    /// The footer is computed, not pushed: `Unsaved changes` is what
    /// `is_dirty()` says, and Revert/Apply are insensitive while clean —
    /// `main.rs:73-84`'s `update_footer` closure, with the Rc<dyn Fn()> gone.
    ///
    /// Mutation check: make `footer` always render `m.status`; this fails.
    #[test]
    fn the_footer_reports_dirtiness_without_a_flag() {
        let (mut m, _dir) = model();
        let clean = footer_text(&m);
        assert_eq!(clean, "");
        m.model.working.appearance.accent = "#ff00aa".to_string();
        assert_eq!(footer_text(&m), "Unsaved changes");
        m.status = "Applied".to_string();
        m.model.saved = m.model.working.clone();
        assert_eq!(footer_text(&m), "Applied");
    }
```

and the helper the last test reads (in `app.rs`, not in `mod tests`, because `view` builds it):

```rust
/// What the footer's status label shows: the dirty indicator wins over the
/// last status line, exactly as the GTK `update_footer` closure did.
#[must_use]
pub fn footer_text(m: &SettingsModel) -> &str {
    if m.is_dirty() { "Unsaved changes" } else { &m.status }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p icedtea-settings --lib app::tests`
Expected: FAIL — `cannot find function 'view' in this scope`.

- [ ] **Step 3: Implement the five page views**

In `settings/src/pages/appearance.rs` (and the same shape in `behavior.rs`,
`workspaces.rs`, `keybindings.rs` and `displays/mod.rs`, changing the id, the label and the
module doc line):

```rust
/// The Appearance page.
///
/// P1 ships the page's frame only; P2 fills it in (contract §2.6).
#[must_use]
pub fn view(m: &crate::app::SettingsModel) -> icedtea_ui::view::View<crate::app::Msg> {
    let _ = m;
    icedtea_ui::view::builders::box_(
        icedtea_ui::layout::Orientation::Vertical,
        [icedtea_ui::widgets::label("Appearance")],
    )
    .id("appearance_page")
    .margin(16, 16, 16, 16)
}
```

The four others, verbatim except for these three values:

| module | id | label | filled by |
|---|---|---|---|
| `pages/behavior.rs` | `behavior_page` | `"Behavior"` | P2 |
| `pages/workspaces.rs` | `workspaces_page` | `"Workspaces"` | P3 |
| `pages/keybindings.rs` | `keybindings_page` | `"Keybindings"` | P3 |
| `pages/displays/mod.rs` | `displays_page` | `"Displays"` | P4 |

- [ ] **Step 4: Implement `view`, `nav` and `footer`**

Append to `settings/src/app.rs`:

```rust
use icedtea_ui::layout::Orientation;
use icedtea_ui::view::Align;
use icedtea_ui::view::builders::{StackExt, box_, stack, stack_page, stack_switcher};
use icedtea_ui::widgets::{button, label};

use crate::pages;

/// The whole window, rebuilt from the model on every frame.
#[must_use]
pub fn view(m: &SettingsModel) -> View<Msg> {
    box_(
        Orientation::Vertical,
        [
            nav(m),
            stack([
                stack_page("appearance", "Appearance", pages::appearance::view(m)),
                stack_page("behavior", "Behavior", pages::behavior::view(m)),
                stack_page("workspaces", "Workspaces", pages::workspaces::view(m)),
                stack_page("keybindings", "Keybindings", pages::keybindings::view(m)),
                stack_page("displays", "Displays", pages::displays::view(m)),
            ])
            .visible_child(m.page.name())
            .id("pages")
            .vexpand(true),
            footer(m),
        ],
    )
    .id("root")
}

/// The page switcher. `Stack` + `StackSwitcher`, not `StackSidebar` (spec D7:
/// the sidebar's eviction defect is out of M5's scope).
fn nav(m: &SettingsModel) -> View<Msg> {
    let _ = m;
    stack_switcher(pages::page_infos())
        .id("nav")
        .on_selected(Msg::PageSelected)
}

/// Status line plus Revert and Apply. Every value here is computed from the
/// model — there is no dirty flag and no `Rc<dyn Fn()>` to call.
fn footer(m: &SettingsModel) -> View<Msg> {
    let dirty = m.is_dirty();
    box_(
        Orientation::Horizontal,
        [
            label(footer_text(m))
                .id("status")
                .hexpand(true)
                .halign(Align::Start),
            button("Revert").id("revert").sensitive(dirty).on_click(Msg::Revert),
            button("Apply").id("apply").sensitive(dirty).on_click(Msg::Apply),
        ],
    )
    .id("footer")
    .margin(8, 8, 8, 8)
}
```

- [ ] **Step 5: Run the tests**

Run: `cargo test -p icedtea-settings --lib app::tests`
Expected: PASS — 9 tests.

- [ ] **Step 6: Commit**

```bash
cd /home/joseph/Projects/icedtea/.claude/worktrees/m5
cargo fmt --all && cargo fmt --all --check
cargo clippy -p icedtea-settings --all-targets -- -D warnings
git add settings/src/app.rs settings/src/pages/
git commit -m "$(cat <<'MSG'
feat(settings): the view — nav, stack and footer

view(&SettingsModel) is pure: a vertical box of the StackSwitcher, the
five-page Stack selected by model.page, and a footer whose status text and
button sensitivity are computed from working != saved. The GTK
Ctx/Page/populating dirty contract has no equivalent and is not ported.

Each page contributes its own pub view(); P1's five are the page frames
P2/P3/P4 fill in, each carrying the id its gates address it by.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
MSG
)"
```

---

## Task 8: the outputs pump — `watch_fd` + `on_fd` for the second Wayland connection

The Displays page's `zwlr_output_management_v1` connection is a second `wayland-client`
`EventQueue` that used to be pumped by a `gio::Socket` glib source. It now registers its
readiness fd with `Window::watch_fd` and turns readiness into `Msg::Outputs` through
`App::on_fd`. `outputs/protocol.rs` is not edited.

**Files:**
- Create: `settings/src/outputs/pump.rs`
- Modify: `settings/src/outputs/mod.rs` (declare `pump`; `client` stays until Task 10)
- Modify: `settings/src/app.rs` (`Msg::Outputs` + its `update` arm + the model field)

**Interfaces:**
- Consumes: `icedtea_ui::window::{Interest, WatchId, Window}` (M5-D1);
  `crate::outputs::{HeadEdit, OutputsConnection, OutputsError, OutputsMsg}`;
  `crate::pages::displays::state::{DisplaysState, reconcile}`; `rustix::io::dup`.
- Produces:

```rust
// settings/src/outputs/pump.rs
#[derive(Clone)]
pub struct OutputsPump { /* Rc<RefCell<OutputsConnection>>, async_channel::Receiver<OutputsMsg>, WatchId */ }
impl OutputsPump {
    pub fn attach(window: &mut icedtea_ui::window::Window) -> std::io::Result<Option<OutputsPump>>;
    #[must_use] pub fn watch(&self) -> icedtea_ui::window::WatchId;
    #[must_use] pub fn drain(&self) -> Vec<crate::app::Msg>;
    pub fn test_configuration(&self, edits: &[crate::outputs::HeadEdit]);
    pub fn build_and_send_configuration(&self, edits: &[crate::outputs::HeadEdit]);
    pub fn flush(&self);
}

// settings/src/app.rs — added to `Msg` and `SettingsModel`
Msg::Outputs(std::sync::Arc<crate::outputs::OutputsMsg>)
SettingsModel::outputs: Option<crate::outputs::pump::OutputsPump>   // deviation P1-D2
impl SettingsModel { pub fn with_outputs(self, pump: Option<crate::outputs::pump::OutputsPump>) -> Self; }
```

- [ ] **Step 1: Write the failing tests**

Append to `settings/src/app.rs`'s `mod tests`:

```rust
    use crate::outputs::{Head, Mode, OutputsMsg};
    use std::sync::Arc;

    fn head(name: &str) -> Head {
        let mode = Mode { width: 1920, height: 1080, refresh_mhz: 60_000, preferred: true };
        Head {
            name: name.to_string(),
            description: format!("{name} test head"),
            enabled: true,
            modes: vec![mode],
            current_mode: Some(mode),
            x: 0,
            y: 0,
            scale: 1.0,
            transform: 0,
        }
    }

    /// The glib source's `match` on OutputsMsg becomes six `update` arms
    /// (contract §2.5). Mutation check: drop the `outputs_available = true`
    /// in the `HeadsChanged` arm; this test fails. Restore.
    #[test]
    fn heads_changed_reconciles_and_marks_output_management_available() {
        let (mut m, _dir) = model();
        update(&mut m, Msg::Outputs(Arc::new(OutputsMsg::HeadsChanged(vec![head("DP-1")]))));
        assert!(m.outputs_available);
        assert_eq!(m.displays.heads.len(), 1);
        assert_eq!(m.displays.edits.len(), 1);
        assert_eq!(m.displays.selected, Some(0));
    }

    #[test]
    fn losing_the_manager_or_the_connection_takes_the_page_out_of_service() {
        for msg in [OutputsMsg::ManagerUnavailable, OutputsMsg::Disconnected] {
            let (mut m, _dir) = model();
            m.outputs_available = true;
            m.displays_in_flight = true;
            update(&mut m, Msg::Outputs(Arc::new(msg)));
            assert!(!m.outputs_available);
            assert!(!m.displays_in_flight, "an in-flight request can never complete now");
        }
    }

    /// A reply's own `is_test` tag decides what it means — never a shared
    /// in-flight flag, which an overlapped Test/Apply pair would misread.
    #[test]
    fn an_apply_reply_clears_in_flight_by_its_own_is_test_tag() {
        let (mut m, _dir) = model();
        m.displays.dirty = true;
        m.displays_in_flight = true;
        update(&mut m, Msg::Outputs(Arc::new(OutputsMsg::ApplySucceeded { is_test: true })));
        assert!(!m.displays_in_flight);
        assert!(m.displays.dirty, "a successful *test* keeps the pending edits");
        assert_eq!(m.displays_status, "Test succeeded");

        m.displays_in_flight = true;
        update(&mut m, Msg::Outputs(Arc::new(OutputsMsg::ApplySucceeded { is_test: false })));
        assert!(!m.displays_in_flight);
        assert!(!m.displays.dirty, "a successful apply is no longer pending work");
        assert_eq!(m.displays_status, "Applied");
    }
```

and create `settings/tests/outputs_pump.rs`:

```rust
//! The outputs pump against the harness compositor: the second Wayland
//! connection's fd is registered with the window, and readiness turns into
//! `Msg::Outputs`.

mod support;

use icedtea_settings::app::Msg;
use icedtea_settings::outputs::pump::OutputsPump;

/// Mutation check: make `OutputsPump::drain` return `Vec::new()`
/// unconditionally; this test fails. Restore.
#[test]
fn the_pump_registers_its_fd_and_drains_protocol_messages() {
    let comp = icedtea_harness::Compositor::spawn();
    let mut window = support::open_test_window(&comp);
    let before = window.watches().len();

    let pump = OutputsPump::attach(&mut window)
        .expect("attach never fails once the connection is up")
        .expect("the harness advertises zwlr_output_manager_v1");

    assert_eq!(
        window.watches().len(),
        before + 1,
        "the pump's queue fd must be in the window's poll set"
    );
    assert!(window.watches().contains(&pump.watch()));

    // The connection roundtripped during `attach`, so an initial
    // HeadsChanged is already queued.
    let msgs = pump.drain();
    assert!(
        msgs.iter().any(|m| matches!(m, Msg::Outputs(u) if matches!(
            **u,
            icedtea_settings::outputs::OutputsMsg::HeadsChanged(_)
        ))),
        "expected an initial HeadsChanged, got {msgs:?}"
    );
}
```

`settings/tests/support/mod.rs` is completed in Task 11; for this task create it with just the
one helper it needs:

```rust
//! Harness helpers shared by the settings integration tests.

/// A 480x420 toplevel window on `comp`'s socket, with the bundled Adwaita
/// sheet — the same surface `icedtea-settings` opens.
pub fn open_test_window(comp: &icedtea_harness::Compositor) -> icedtea_ui::window::Window {
    // SAFETY-free: the harness owns the socket for the life of `comp`, and
    // `Window::open` reads `$WAYLAND_DISPLAY` once, here, before any thread
    // that could race it exists.
    unsafe { std::env::set_var("WAYLAND_DISPLAY", &comp.socket) };
    icedtea_ui::window::Window::open(
        icedtea_ui::window::SurfaceSpec {
            role: icedtea_ui::window::Role::Toplevel,
            size: (480, 420),
            title: "icedtea Settings".to_string(),
            app_id: "org.icedtea.Settings".to_string(),
        },
        icedtea_ui::app::compile_theme(&icedtea_ui::app::ThemeSource::Bundled),
        icedtea_ui::text::FontDatabase::new(),
    )
    .expect("open a toplevel on the harness")
}
```

- [ ] **Step 2: Run to verify failure**

```bash
cargo test -p icedtea-settings --lib app::tests
cargo test -p icedtea-settings --test outputs_pump
```
Expected: FAIL — `no variant named 'Outputs' found for enum 'Msg'`, and
`unresolved import 'icedtea_settings::outputs::pump'`.

- [ ] **Step 3: Implement the pump**

`settings/src/outputs/pump.rs`:

```rust
//! The Displays page's second `wayland-client` connection, pumped by the
//! toolkit's poll loop instead of a glib source.
//!
//! `Window::watch_fd` takes a dup of the `EventQueue`'s readiness fd (M5-D1),
//! and `App::on_fd` calls [`OutputsPump::drain`] whenever it fires. Nothing
//! here blocks: `drain` dispatches what is already buffered and reads only if
//! that dispatched nothing, exactly as `protocol.rs:367-379` does.

use std::cell::RefCell;
use std::rc::Rc;

use icedtea_ui::window::{Interest, WatchId, Window};

use crate::app::Msg;
use crate::outputs::{HeadEdit, OutputsConnection, OutputsMsg};

/// Owns the second connection and its watch.
#[derive(Clone)]
pub struct OutputsPump {
    conn: Rc<RefCell<OutputsConnection>>,
    rx: async_channel::Receiver<OutputsMsg>,
    watch: WatchId,
}

impl OutputsPump {
    /// Connect, register the queue fd with `window`, and return the pump.
    ///
    /// A connect failure — no compositor, no output-management global, fd
    /// exhaustion — is **not** fatal: it returns `Ok(None)` and the page shows
    /// its "output management unavailable" state, the exact degradation the
    /// deleted `OutputsClient` had.
    ///
    /// # Errors
    ///
    /// Never today: every failure path yields `Ok(None)`. The signature keeps
    /// the `io::Result` the contract froze so a future registration failure has
    /// somewhere to go.
    pub fn attach(window: &mut Window) -> std::io::Result<Option<OutputsPump>> {
        let (tx, rx) = async_channel::unbounded::<OutputsMsg>();
        let conn = match OutputsConnection::connect_to_env(tx) {
            Ok(conn) => conn,
            Err(err) => {
                tracing::warn!(%err, "no outputs connection; the Displays page is unavailable");
                return Ok(None);
            }
        };
        // Dup so the window owns its own fd and the original stays with the
        // EventQueue — the same dup `outputs/client.rs:61` did for gio::Socket.
        let fd = match rustix::io::dup(std::os::fd::AsFd::as_fd(conn.queue())) {
            Ok(fd) => fd,
            Err(err) => {
                tracing::warn!(%err, "cannot dup the outputs queue fd; page unavailable");
                conn.notify_disconnected();
                return Ok(None);
            }
        };
        let watch = window.watch_fd(fd, Interest::Read);
        Ok(Some(OutputsPump {
            conn: Rc::new(RefCell::new(conn)),
            rx,
            watch,
        }))
    }

    /// The watch `App::on_fd` is keyed on.
    #[must_use]
    pub fn watch(&self) -> WatchId {
        self.watch
    }

    /// The `App::on_fd` body: dispatch, then fold every queued protocol
    /// message into a `Msg`. Never blocks, never `blocking_dispatch`.
    #[must_use]
    pub fn drain(&self) -> Vec<Msg> {
        {
            let mut conn = self.conn.borrow_mut();
            let dispatched = conn.dispatch_pending();
            let outcome = match dispatched {
                Ok(0) => conn.read_and_dispatch(),
                other => other,
            };
            if let Err(err) = outcome {
                tracing::warn!(%err, "the outputs connection died");
                conn.notify_disconnected();
            }
        }
        let mut msgs = Vec::new();
        while let Ok(msg) = self.rx.try_recv() {
            msgs.push(Msg::Outputs(std::sync::Arc::new(msg)));
        }
        msgs
    }

    /// Preview a configuration. Fire-and-forget, from `Cmd::Task`; the reply
    /// arrives as `OutputsMsg::ApplySucceeded { is_test: true }` or its
    /// failure/cancel siblings.
    pub fn test_configuration(&self, edits: &[HeadEdit]) {
        if let Err(err) = self.conn.borrow_mut().test_configuration(edits) {
            tracing::warn!(?err, "test configuration could not be sent");
        }
        self.flush();
    }

    /// Apply a configuration. Same fire-and-forget shape as
    /// [`OutputsPump::test_configuration`].
    pub fn build_and_send_configuration(&self, edits: &[HeadEdit]) {
        if let Err(err) = self.conn.borrow_mut().build_and_send_configuration(edits) {
            tracing::warn!(?err, "configuration could not be sent");
        }
        self.flush();
    }

    /// Push queued requests out. The toolkit's loop flushes its own
    /// connection, never this one.
    pub fn flush(&self) {
        if let Err(err) = self.conn.borrow().flush() {
            tracing::warn!(%err, "flushing the outputs connection failed");
        }
    }
}
```

`settings/src/outputs/mod.rs` gains `pub mod pump;` beside `pub mod client;`.

- [ ] **Step 4: Implement `Msg::Outputs`, the model field, and the `update` arm**

In `settings/src/app.rs`, add to `Msg`:

```rust
    /// One protocol message from the outputs connection, via `App::on_fd`.
    /// `Arc`, not `Rc`: `Msg` is `Send` (M5-D2).
    Outputs(std::sync::Arc<crate::outputs::OutputsMsg>),
```

to `SettingsModel` (deviation P1-D2):

```rust
    /// The second Wayland connection, when there is one. `Cmd::Task` reaches
    /// Test/Apply through it (P4); `None` means output management is absent.
    pub outputs: Option<crate::outputs::pump::OutputsPump>,
```

with `outputs: None` in `SettingsModel::new` and the builder the binary uses:

```rust
    /// Attach the outputs connection, if the window got one.
    #[must_use]
    pub fn with_outputs(mut self, pump: Option<crate::outputs::pump::OutputsPump>) -> Self {
        self.outputs_available = pump.is_some();
        self.outputs = pump;
        self
    }
```

and the `update` arm, which is the glib source's `match`
(`settings/src/pages/displays.rs:836-1023`) rewritten as a fold:

```rust
        Msg::Outputs(update) => {
            match &*update {
                crate::outputs::OutputsMsg::HeadsChanged(heads) => {
                    let r = crate::pages::displays::state::reconcile(
                        &m.displays.heads,
                        &m.displays.edits,
                        m.displays.selected,
                        m.displays.dirty,
                        heads,
                    );
                    m.displays.heads = heads.clone();
                    m.displays.edits = r.edits;
                    m.displays.selected = r.selected;
                    // A drag was indexed against the *old* head list; the new
                    // one may be shorter or reordered (finding #2).
                    m.displays.drag = None;
                    if !r.compatible {
                        // A genuine set change re-baselines, so nothing is
                        // unsaved any more.
                        m.displays.dirty = false;
                    }
                    m.outputs_available = true;
                    m.displays_status = if r.dropped {
                        "Displays changed \u{2014} pending edits discarded".to_string()
                    } else {
                        String::new()
                    };
                }
                crate::outputs::OutputsMsg::ApplySucceeded { is_test } => {
                    m.displays_in_flight = false;
                    if *is_test {
                        // A preview succeeded: keep the edits so the user can
                        // commit them.
                        m.displays_status = "Test succeeded".to_string();
                    } else {
                        m.displays.dirty = false;
                        m.displays_status = "Applied".to_string();
                    }
                }
                crate::outputs::OutputsMsg::ApplyFailed { is_test } => {
                    m.displays_in_flight = false;
                    if *is_test {
                        m.displays_status = "Test rejected by the compositor".to_string();
                    } else {
                        // Re-baseline: the compositor kept its own layout.
                        m.displays.edits = m
                            .displays
                            .heads
                            .iter()
                            .map(crate::pages::displays::state::baseline_edit)
                            .collect();
                        m.displays.dirty = false;
                        m.displays_status =
                            "Configuration rejected by the compositor".to_string();
                    }
                }
                crate::outputs::OutputsMsg::ApplyCancelled => {
                    m.displays_in_flight = false;
                    m.displays_status = "Configuration superseded \u{2014} re-reading".to_string();
                }
                crate::outputs::OutputsMsg::ManagerUnavailable
                | crate::outputs::OutputsMsg::Disconnected => {
                    m.displays_in_flight = false;
                    m.outputs_available = false;
                    m.displays_status = String::new();
                }
            }
            Cmd::None
        }
```

- [ ] **Step 5: Run the tests**

```bash
cargo test -p icedtea-settings --lib app::tests
cargo test -p icedtea-settings --test outputs_pump
```
Expected: PASS — 12 unit tests, 1 harness test.

- [ ] **Step 6: Commit**

```bash
cd /home/joseph/Projects/icedtea/.claude/worktrees/m5
cargo fmt --all && cargo fmt --all --check
cargo clippy -p icedtea-settings --all-targets -- -D warnings
git add settings/src/outputs/ settings/src/app.rs settings/tests/
git commit -m "$(cat <<'MSG'
feat(settings): pump the outputs connection through watch_fd/on_fd

The zwlr_output_management_v1 connection used to be dispatched by a
gio::Socket glib source. OutputsPump dups its EventQueue fd into
Window::watch_fd (M5-D1) and turns readiness into Msg::Outputs, and the
glib source's match becomes six update arms — the same reconcile call, the
same is_test-tagged reply handling, the same "unavailable" degradation on a
missing manager or a dead connection. outputs/protocol.rs is untouched.

SettingsModel gains an `outputs` field the contract's §2.2 listing omits
but its own §2.5 wiring requires (deviation P1-D2, contract §6).

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
MSG
)"
```

---

## Task 9: `App::with_probe_report` and the settings state log

M5-D9 gives a harness test a way to find a live app's widgets. `Window::probe_points`/
`Window::allocation` read the `Window`'s own layout tree, which is the right thing for the raw
`window-probe` path — but under `App::run` the layout lives in the loop's `Runtime`, and the
app crate cannot reach either. So the writer lands in `ui/src/view/app.rs` as an enumerated
exception (deviation P1-D4), computed from the tree the frame just laid out, and settings
appends its own `msg`/state lines to the same file through `settings/src/probe.rs`.

**Files:**
- Modify: `ui/src/view/app.rs`
- Modify: `ui/README.md`
- Create: `settings/src/probe.rs`
- Modify: `settings/src/lib.rs`, `settings/src/app.rs`

**Interfaces:**
- Consumes: `icedtea_ui::layout::{Allocation, LayoutTree}`, `icedtea_ui::css::node::Node`.
- Produces:

```rust
// ui/src/view/app.rs
impl<M: 'static, Msg: Clone + 'static> App<M, Msg> {
    #[must_use] pub fn with_probe_report(self, path: std::path::PathBuf) -> Self;
}

// settings/src/probe.rs
#[must_use] pub fn report_path() -> Option<std::path::PathBuf>;
pub fn report(line: &str);
```

**Report format (deviation P1-D5), append-only:**

```
frame <n>
probe <label> <x> <y>
alloc <id> <x> <y> <w> <h>
```

`<label>` is the node's id if it has one, else its CSS node name with a repeat index —
`gallery::probe_points_of`'s rule. `<x> <y>` are the border box's floored centre. A reader
parses a `probe` line with `parse_probe_line` directly (four fields: `probe`, label, x, y) and
an `alloc` line by stripping the leading `alloc ` first (`parse_allocation_line` takes exactly
five). Settings appends its own single-token-prefixed state lines (`page <name>`,
`status <text>`, `msg <variant>`) to the same file; every writer appends, nothing truncates.

- [ ] **Step 1: Write the failing tests**

Check first whether P0 already shipped the builder:

```bash
cd /home/joseph/Projects/icedtea/.claude/worktrees/m5
grep -n "fn with_probe_report" ui/src/view/app.rs || echo "not present — add it"
```

Append to `ui/tests/ingress.rs`:

```rust
/// An app running offscreen still publishes its geometry, so a settings or
/// shell gate can locate widgets without hard-coding coordinates.
///
/// Mutation check: make `with_probe_report` store `None`; this test fails
/// with an empty report. Restore.
#[test]
fn an_app_with_a_probe_report_publishes_probe_and_alloc_lines() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("report");
    let app = icedtea_ui::view::App::new(
        0u32,
        |_m: &mut u32, _msg: u32| icedtea_ui::view::Cmd::None,
        |_m: &u32| {
            icedtea_ui::view::builders::box_(
                icedtea_ui::layout::Orientation::Vertical,
                [icedtea_ui::widgets::button("Apply").id("apply")],
            )
            .id("root")
        },
    )
    .with_probe_report(path.clone());
    let clock = std::rc::Rc::new(icedtea_ui::anim::ManualClock::new());
    app.run_offscreen((200, 100), clock, vec![icedtea_ui::view::app::ScriptStep::Capture])
        .expect("offscreen run");

    let text = std::fs::read_to_string(&path).expect("the report exists");
    assert!(text.lines().any(|l| l == "frame 0"), "no frame marker in {text:?}");
    assert!(
        text.lines().any(|l| l.starts_with("probe apply ")),
        "no probe point for #apply in {text:?}"
    );
    let alloc = text
        .lines()
        .find(|l| l.starts_with("alloc apply "))
        .expect("no alloc line for #apply");
    let fields: Vec<&str> = alloc.split_whitespace().collect();
    assert_eq!(fields.len(), 6, "alloc lines carry id, x, y, w, h");
    let width: f32 = fields[4].parse().expect("width parses");
    assert!(width > 0.0, "#apply should have been laid out, got {alloc}");
}

/// A frame that changes nothing appends nothing: the report is a change log,
/// not a per-frame dump, so a test can wait for a specific state.
#[test]
fn an_unchanged_tree_does_not_append_another_block() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("report");
    let app = icedtea_ui::view::App::new(
        0u32,
        |_m: &mut u32, _msg: u32| icedtea_ui::view::Cmd::None,
        |_m: &u32| icedtea_ui::widgets::label("steady").id("root"),
    )
    .with_probe_report(path.clone());
    let clock = std::rc::Rc::new(icedtea_ui::anim::ManualClock::new());
    app.run_offscreen(
        (200, 100),
        clock,
        vec![
            icedtea_ui::view::app::ScriptStep::Capture,
            icedtea_ui::view::app::ScriptStep::Capture,
            icedtea_ui::view::app::ScriptStep::Capture,
        ],
    )
    .expect("offscreen run");
    let text = std::fs::read_to_string(&path).expect("the report exists");
    assert_eq!(
        text.lines().filter(|l| l.starts_with("frame ")).count(),
        1,
        "three identical frames must publish one block, got {text:?}"
    );
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p icedtea-ui --test ingress probe_report`
Expected: FAIL — `no method named 'with_probe_report' found for struct 'App'`.

- [ ] **Step 3: Implement the writer in `ui/src/view/app.rs`**

Add the field to `App` (`probe_report: Option<std::path::PathBuf>`, `None` in `App::new`), the
builder, and the two free functions:

```rust
    /// Publish this app's live widget geometry to `path`, for harness tests.
    ///
    /// One block per frame whose laid-out tree changed, appended:
    ///
    /// ```text
    /// frame <n>
    /// probe <label> <x> <y>
    /// alloc <id> <x> <y> <w> <h>
    /// ```
    ///
    /// `<label>` is a node's id if it has one, else its CSS node name with a
    /// repeat index — `gallery::probe_points_of`'s rule — and `<x> <y>` the
    /// floored centre of its border box. Both `run` and `run_offscreen`
    /// publish, so a gate can address widgets by id with or without a
    /// compositor. An I/O failure is logged once and never fails the frame:
    /// this is diagnostics, not behaviour.
    #[must_use]
    pub fn with_probe_report(mut self, path: std::path::PathBuf) -> Self {
        self.probe_report = Some(path);
        self
    }
```

```rust
/// The report block for one laid-out tree, or an empty string if nothing is
/// laid out yet.
fn probe_block(root: &Node, layout: &LayoutTree) -> String {
    use std::collections::BTreeMap;
    use std::fmt::Write as _;

    let descendants: Vec<Node> = std::iter::once(root.clone())
        .chain(root.descendants())
        .collect();
    let mut counts: BTreeMap<Rc<str>, usize> = BTreeMap::new();
    for node in &descendants {
        *counts.entry(node.name()).or_default() += 1;
    }
    let mut seen: BTreeMap<Rc<str>, usize> = BTreeMap::new();
    let mut probes = String::new();
    let mut allocs = String::new();
    for node in &descendants {
        let name = node.name();
        let index = seen.entry(name.clone()).or_default();
        let positional = if counts.get(&name).copied().unwrap_or(0) > 1 {
            format!("{name}{index}")
        } else {
            name.to_string()
        };
        *index += 1;
        let Some(alloc) = layout.allocation(node) else {
            continue;
        };
        let id = node.id().map(|id| id.to_string());
        let label = id.clone().unwrap_or(positional);
        let r = alloc.border_box;
        let _ = writeln!(
            probes,
            "probe {label} {} {}",
            (r.x + r.width / 2.0).floor() as i32,
            (r.y + r.height / 2.0).floor() as i32
        );
        if let Some(id) = id {
            let _ = writeln!(allocs, "alloc {id} {} {} {} {}", r.x, r.y, r.width, r.height);
        }
    }
    format!("{probes}{allocs}")
}

/// Append `block` under a frame marker, if it differs from the last one.
fn publish_probe_block(
    path: &std::path::Path,
    frame: &mut u64,
    last: &mut Option<String>,
    block: String,
) {
    if block.is_empty() || last.as_deref() == Some(block.as_str()) {
        return;
    }
    use std::io::Write as _;
    let written = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut file| {
            write!(file, "frame {frame}\n{block}")?;
            file.flush()
        });
    match written {
        Ok(()) => {
            *frame += 1;
            *last = Some(block);
        }
        Err(err) => tracing::warn!(%err, "cannot write the probe report"),
    }
}
```

In both `run` and `run_offscreen`, immediately after their `restyle_and_layout(...)?;` call and
before painting, add:

```rust
            if let Some(path) = &probe_report {
                publish_probe_block(
                    path,
                    &mut probe_frame,
                    &mut probe_last,
                    probe_block(&rt.root, &rt.layout),
                );
            }
```

with `let probe_report = self.probe_report.take();`, `let mut probe_frame = 0u64;` and
`let mut probe_last: Option<String> = None;` declared beside the loop's other locals.

Append to `ui/README.md`, under *Reactive framework (`view/`)*:

```markdown
### Publishing a live window's geometry

`App::with_probe_report(path)` appends one block per frame whose laid-out tree
changed:

```text
frame <n>
probe <label> <x> <y>
alloc <id> <x> <y> <w> <h>
```

`<label>` is the node's `id` when it has one, else its CSS node name with a
repeat index; `<x> <y>` is the floored centre of its border box. Both `run` and
`run_offscreen` publish, so a harness gate addresses widgets by id instead of
hard-coding coordinates. The file is only ever appended to, so an application
may write its own state lines to the same path.
```

- [ ] **Step 4: Run the ui tests**

```bash
cargo test -p icedtea-ui --test ingress
cargo test -p icedtea-ui
```
Expected: PASS, including every M3 gate unchanged.

- [ ] **Step 5: Add the settings state log**

`settings/src/probe.rs`:

```rust
//! The app's own lines in `$ICEDTEA_PROBE_REPORT`.
//!
//! `App::with_probe_report` publishes geometry; this publishes state — `page
//! <name>`, `status <text>`, `msg <variant>` — into the same append-only file,
//! so a harness gate can wait for a state and then click a widget it located
//! by id in the same report.

/// The report path, when the environment names one.
#[must_use]
pub fn report_path() -> Option<std::path::PathBuf> {
    std::env::var_os("ICEDTEA_PROBE_REPORT").map(std::path::PathBuf::from)
}

/// Append one line. A missing variable or an I/O failure is a no-op: this is
/// diagnostics, and a settings build with no report must behave identically.
pub fn report(line: &str) {
    let Some(path) = report_path() else {
        return;
    };
    use std::io::Write as _;
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{line}");
        let _ = file.flush();
    }
}
```

`settings/src/lib.rs` gains `pub mod probe;`, and `update`'s head gains one line so every fold
is observable:

```rust
pub fn update(m: &mut SettingsModel, msg: Msg) -> Cmd<Msg> {
    // One line per fold, for the harness gates. A no-op without
    // $ICEDTEA_PROBE_REPORT.
    crate::probe::report(&format!("msg {msg:?}"));
    let cmd = match msg { /* … the arms, unchanged … */ };
    crate::probe::report(&format!("page {}", m.page.name()));
    crate::probe::report(&format!("status {}", footer_text(m)));
    cmd
}
```

- [ ] **Step 6: Run the settings tests**

Run: `cargo test -p icedtea-settings`
Expected: PASS — unchanged counts (the report is inert without the variable).

- [ ] **Step 7: Commit**

```bash
cd /home/joseph/Projects/icedtea/.claude/worktrees/m5
cargo fmt --all && cargo fmt --all --check
cargo clippy -p icedtea-ui --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings
cargo clippy -p icedtea-settings --all-targets -- -D warnings
git add ui/src/view/app.rs ui/tests/ingress.rs ui/README.md settings/src/probe.rs settings/src/lib.rs settings/src/app.rs
git commit -m "$(cat <<'MSG'
feat(ui): App::with_probe_report, and settings' state log

M5-D9 put probing on Window, but under App::run the laid-out tree lives in
the loop's Runtime and the app crate can reach neither. with_probe_report
publishes the same probe/alloc lines from inside the loop — one appended
block per frame whose tree changed — so a settings or shell gate can locate
widgets by id instead of hard-coding coordinates. Recorded as deviation
P1-D4/P1-D5 in the M5 contract §6.

settings/src/probe.rs appends its own page/status/msg lines to the same
file, which is why the format is append-only rather than rewritten.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
MSG
)"
```

---

## Task 10: the flip — new `main.rs`, GTK amputation, `settings/style.css`

The commit the spec's D3 describes: the GTK settings binary stops existing. `main.rs` becomes
the wiring §2.5 spells; every `build()`, `Ctx`, `Page`, `Row`, `install_conflict_css`,
`sync_rows`, `reset_capture`, `set_remove_sensitivity`, `labeled`/`labeled_row`/`set_dropdown`
and `outputs/client.rs` is deleted; `gtk4`, `gio` and `glib` leave `Cargo.toml`.

**Files:**
- Rewrite: `settings/src/main.rs`
- Create: `settings/style.css`
- Modify: `settings/Cargo.toml`, `settings/src/pages/mod.rs`, `settings/src/outputs/mod.rs`,
  `settings/src/pages/{appearance,behavior,workspaces,keybindings}.rs`,
  `settings/src/pages/displays/mod.rs`
- Delete: `settings/src/outputs/client.rs`

**Interfaces:**
- Consumes: `icedtea_ui::app::{ThemeEnv, load_layered_stylesheet}`,
  `icedtea_ui::css::cascade::CompiledSheet`, `icedtea_ui::css::parse::parse_stylesheet_with_base`,
  `icedtea_ui::window::{Role, SurfaceSpec, Window}`, `icedtea_ui::view::{App, Inbox}`,
  `icedtea_ui::text::FontDatabase`, `crate::ipc::spawn`, `crate::outputs::pump::OutputsPump`,
  `crate::app::{SettingsModel, update, view}`, `icedtea_config::default_db_path`.
- Produces: `settings/src/main.rs::main`, and `fn sheet() -> CompiledSheet` beside it.

- [ ] **Step 1: Write the failing test**

Create `settings/tests/no_gtk.rs`:

```rust
//! The dependency-swap gate: the settings crate builds and runs with no GTK
//! stack anywhere in its normal dependency tree or its source.
//!
//! Mutation check: put `gtk4 = "0.11"` back in settings/Cargo.toml; this test
//! fails. Restore.

use std::process::Command;

fn workspace_root() -> std::path::PathBuf {
    let mut path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path
}

#[test]
fn no_gtk_stack_in_the_settings_tree_or_sources() {
    for package in ["gtk4", "gtk4-layer-shell", "glib", "gio", "gdk4", "pango", "cairo-rs"] {
        let out = Command::new(env!("CARGO"))
            .current_dir(workspace_root())
            .args(["tree", "-p", "icedtea-settings", "-e", "normal", "-i", package])
            .output()
            .expect("cargo tree runs");
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            !out.status.success() || text.contains("nothing to print"),
            "{package} is still in icedtea-settings' normal tree:\n{text}"
        );
    }

    for dir in ["settings/src", "settings/tests"] {
        let out = Command::new("grep")
            .current_dir(workspace_root())
            .args(["-rn", "gtk4\\|glib::\\|gio::\\|gdk::\\|pango::\\|cairo::", dir])
            .output()
            .expect("grep runs");
        let hits = String::from_utf8_lossy(&out.stdout);
        assert!(hits.trim().is_empty(), "GTK paths remain in {dir}:\n{hits}");
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p icedtea-settings --test no_gtk`
Expected: FAIL — `gtk4 is still in icedtea-settings' normal tree`.

- [ ] **Step 3: Delete the GTK view code**

```bash
cd /home/joseph/Projects/icedtea/.claude/worktrees/m5/settings
git rm src/outputs/client.rs
```

Then, by hand:

- `src/outputs/mod.rs`: drop `pub mod client;` and `pub use client::OutputsClient;`, keep the
  `protocol` re-exports, and update the module doc's second bullet to describe `pump` instead of
  the glib source.
- `src/pages/mod.rs`: delete `Ctx`, `Ctx::mark_dirty`, `Page`, `Page::refresh` and the `gtk4`
  imports. What remains is the module list, `PageId`, `page_infos` and the tests.
- `src/pages/appearance.rs`: delete `build`, `labeled_row`, the GTK imports and the two colour
  adapters. What remains is the four conversions, `view`, and the tests.
- `src/pages/behavior.rs`: delete `build` and `labeled_row`. What remains is `view`.
- `src/pages/workspaces.rs`: delete `Row`, `set_remove_sensitivity`, `RebuildSlot` and `build`.
  What remains is the two pure functions, `view`, and the tests.
- `src/pages/keybindings.rs`: delete `Row`, `sync_rows`, `reset_capture`, `install_conflict_css`,
  `CONFLICT_CSS_CLASS` and `build`. What remains is the pure surface, `view`, and the tests.
- `src/pages/displays/mod.rs`: delete `build`, `labeled` and `set_dropdown` and every GTK import;
  what remains is `pub mod state;`, the `use` of `state`, and `view`.
- `Cargo.toml`: delete the `gtk4`, `gio` and `glib` lines and the stale comment above
  `wayland-client` (it explains a gtk4 re-export that no longer exists).

- [ ] **Step 4: Write the new binary and the app sheet**

`settings/style.css`:

```css
/* icedtea-settings' own sheet, layered over the GTK theme as a user-origin
 * overlay — the mechanism the user's ~/.config/gtk-4.0/gtk.css uses.
 *
 * P3 adds the `.conflict` rule for duplicate keybindings. */

#footer {
  border-top: 1px solid alpha(currentColor, 0.15);
}
```

`settings/src/main.rs`:

```rust
//! `icedtea-settings` — a config editor for icedtea, built on `icedtea-ui`.
//!
//! One `App<SettingsModel, Msg>` on one `Role::Toplevel` window. Two external
//! event sources reach the loop through the toolkit's ingress: the reload
//! worker posts on the inbox, and the Displays page's second Wayland
//! connection is registered with `Window::watch_fd` and mapped to messages by
//! `App::on_fd`.

use icedtea_config::default_db_path;
use icedtea_settings::app::{SettingsModel, update, view};
use icedtea_settings::outputs::pump::OutputsPump;
use icedtea_settings::{ipc, probe};
use icedtea_ui::app::{ThemeEnv, load_layered_stylesheet};
use icedtea_ui::css::cascade::CompiledSheet;
use icedtea_ui::css::parse::parse_stylesheet_with_base;
use icedtea_ui::text::FontDatabase;
use icedtea_ui::view::{App, Inbox};
use icedtea_ui::window::{Role, SurfaceSpec, Window};

const APP_ID: &str = "org.icedtea.Settings";
const TITLE: &str = "icedtea Settings";
const SIZE: (u32, u32) = (480, 420);
const STYLE: &str = include_str!("../style.css");

/// The GTK theme stack with this app's own sheet layered on top, compiled
/// under the media environment the theme implies.
fn sheet() -> CompiledSheet {
    let env = ThemeEnv::from_env();
    let mut stylesheet = load_layered_stylesheet(&env);
    stylesheet.append_layer(parse_stylesheet_with_base(STYLE, None));
    CompiledSheet::compile_with_env(&stylesheet, &env.media_env())
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let spec = SurfaceSpec {
        role: Role::Toplevel,
        size: SIZE,
        title: TITLE.to_string(),
        app_id: APP_ID.to_string(),
    };
    let mut window = match Window::open(spec, sheet(), FontDatabase::new()) {
        Ok(window) => window,
        Err(err) => {
            tracing::error!(?err, "cannot open the settings window");
            std::process::exit(1);
        }
    };

    let (inbox, tx) = match Inbox::new() {
        Ok(pair) => pair,
        Err(err) => {
            tracing::error!(%err, "cannot create the inbox");
            std::process::exit(1);
        }
    };
    let workers = ipc::spawn(tx);

    // A missing output-management global is not fatal: the Displays page
    // shows its unavailable state and everything else works.
    let pump = OutputsPump::attach(&mut window).unwrap_or_else(|err| {
        tracing::warn!(%err, "no outputs pump");
        None
    });
    let watch = pump.as_ref().map(OutputsPump::watch);

    let model = SettingsModel::new(default_db_path(), workers).with_outputs(pump.clone());
    let mut app = App::new(model, update, view).with_inbox(inbox);
    if let (Some(id), Some(pump)) = (watch, pump) {
        app = app.on_fd(id, move || pump.drain());
    }
    if let Some(path) = probe::report_path() {
        app = app.with_probe_report(path);
    }
    if let Err(err) = app.run(window) {
        tracing::error!(?err, "the settings loop stopped");
        std::process::exit(1);
    }
}
```

- [ ] **Step 5: Run the gate and the whole suite**

```bash
cd /home/joseph/Projects/icedtea/.claude/worktrees/m5
cargo test -p icedtea-settings --test no_gtk
cargo test -p icedtea-settings
cargo tree -p icedtea-settings -e normal | grep -Ei '\b(gtk4|glib|gio|gdk|pango|cairo)\b' || echo "clean"
```

Expected: `no_gtk` passes; the full settings suite passes; the `cargo tree` grep prints `clean`.
`settings/tests/appearance_gtk.rs` and `settings/tests/keybindings_gtk.rs` still compile against
`gtk4` as **dev**-dependencies — leave both, and leave `gtk4` in `[dev-dependencies]`, until P2
and P3 delete them; the gate above checks the **normal** tree only.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all && cargo fmt --all --check
cargo clippy -p icedtea-settings --all-targets -- -D warnings
git add settings/Cargo.toml settings/style.css settings/src/ settings/tests/no_gtk.rs
git commit -m "$(cat <<'MSG'
feat(settings)!: drop gtk4/gio/glib and run on icedtea-ui

The flip the M5 spec's D3 describes: settings swaps its toolkit in place, so
the GTK binary stops existing here rather than coexisting behind a feature
flag. main.rs is now the wiring the contract §2.5 spells — sheet, window,
inbox, workers, outputs pump, App::run — and every build(), Ctx, Page, Row,
CssProvider and the glib outputs source are deleted.

settings/style.css is the app's own sheet, layered over the GTK theme stack
as a user-origin overlay; P3 adds the .conflict rule to it.

The pages are frames until P2-P4 fill them, but every gate below them —
model, keybindings, workspaces, displays state, outputs protocol, the live
compositor round trip — is untouched and green.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
MSG
)"
```

---

## Task 11: the first `Role::Toplevel` `App::run`, under the harness

The named risk in the spec's table: `App::run` has only ever driven the gallery's `Role::Layer`
surface. This gate runs the real settings binary against `icedtea_harness::Compositor`, proves
it paints, and proves the switcher and the footer respond.

**Files:**
- Modify: `settings/tests/support/mod.rs`
- Create: `settings/tests/skeleton.rs`

**Interfaces:**
- Consumes: `icedtea_harness::{Compositor, ScreencopyClient, VirtualPointerClient}`,
  `icedtea_ui::wayland::BTN_LEFT`, `App::with_probe_report`'s line format,
  `settings::probe`'s `page`/`status` lines.
- Produces (in `settings/tests/support/mod.rs`):

```rust
pub fn open_test_window(comp: &icedtea_harness::Compositor) -> icedtea_ui::window::Window;
pub struct SettingsProc { /* Child + report path + TempDir */ }
pub fn spawn_settings(comp: &icedtea_harness::Compositor) -> SettingsProc;
impl SettingsProc {
    pub fn lines(&self) -> Vec<String>;
    pub fn wait_line(&self, prefix: &str, timeout: std::time::Duration) -> Option<String>;
    pub fn point(&self, id: &str) -> Option<(i32, i32)>;
    pub fn allocation(&self, id: &str) -> Option<(f32, f32, f32, f32)>;
}
pub fn click(pointer: &mut icedtea_harness::VirtualPointerClient, x: i32, y: i32);
pub fn pixel_at(frame: &icedtea_harness::CapturedFrame, x: u32, y: u32) -> Option<(u8, u8, u8)>;
pub fn paints_something(frame: &icedtea_harness::CapturedFrame, background: (u8, u8, u8)) -> bool;
```

- [ ] **Step 1: Write the failing test**

Create `settings/tests/skeleton.rs`:

```rust
//! The settings window under the harness compositor.
//!
//! This is `App::run`'s first `Role::Toplevel` surface (M5 spec §10's named
//! risk): every earlier `App::run` drove the gallery's layer surface.

mod support;

use std::time::Duration;

const SETTLE: Duration = Duration::from_secs(20);

/// The window opens, paints, and both the switcher and the footer respond.
///
/// Mutation check: remove `.on_selected(Msg::PageSelected)` from `nav`; the
/// `page displays` wait fails. Restore.
#[test]
fn the_first_toplevel_app_run_paints_and_navigates() {
    let comp = icedtea_harness::Compositor::spawn();
    let mut shot = icedtea_harness::ScreencopyClient::spawn(&comp.socket);
    let background = {
        let frame = shot.capture();
        support::pixel_at(&frame, 1, 1).expect("the harness background is readable")
    };
    let app = support::spawn_settings(&comp);

    // 1. It painted: the first geometry block names the ids `view` builds.
    assert!(
        app.wait_line("probe root ", SETTLE).is_some(),
        "no probe block: the window never laid out\n{:?}",
        app.lines()
    );
    for id in ["nav", "pages", "footer", "status", "revert", "apply", "appearance_page"] {
        assert!(
            app.point(id).is_some(),
            "#{id} is missing from the report\n{:?}",
            app.lines()
        );
    }
    let frame = shot.capture();
    assert!(
        support::paints_something(&frame, background),
        "the settings window painted nothing over the harness background"
    );

    // 2. The switcher navigates: click the fifth switcher button.
    let mut pointer = icedtea_harness::VirtualPointerClient::spawn(&comp.socket);
    let (nx, ny) = app.point("nav").expect("#nav is laid out");
    let (_, _, nav_w, _) = app.allocation("nav").expect("#nav has an allocation");
    // Five equal-width linked buttons; the fifth's centre is at 90% of the width.
    let displays_x = nx - (nav_w as i32) / 2 + (nav_w as i32 * 9) / 10;
    support::click(&mut pointer, displays_x, ny);
    assert!(
        app.wait_line("page displays", SETTLE).is_some(),
        "the switcher did not select the Displays page\n{:?}",
        app.lines()
    );
    assert!(
        app.point("displays_page").is_some(),
        "the Displays page body never entered the tree"
    );

    // 3. The footer responds: Apply is insensitive while clean, and clicking
    //    it changes nothing — the model is not dirty.
    let (ax, ay) = app.point("apply").expect("#apply is laid out");
    support::click(&mut pointer, ax, ay);
    assert!(
        app.wait_line("status Applying", Duration::from_secs(2)).is_none(),
        "an insensitive Apply must not start a save\n{:?}",
        app.lines()
    );
}
```

and add the rest of `settings/tests/support/mod.rs` (`open_test_window` is already there from
Task 8):

```rust
use std::io::Read as _;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// A running `icedtea-settings` against a harness compositor, with its probe
/// report on disk.
pub struct SettingsProc {
    child: Child,
    report: std::path::PathBuf,
    #[allow(dead_code, reason = "kept alive so the temp dir outlives the child")]
    dir: tempfile::TempDir,
}

impl Drop for SettingsProc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The directory cargo put this test binary in, which is also where it put
/// `icedtea-settings`.
fn target_profile_dir() -> std::path::PathBuf {
    let mut path = std::env::current_exe().expect("current_exe");
    path.pop(); // deps/
    path.pop(); // debug/ or release/
    path
}

/// Spawn the real settings binary on `comp`'s socket, with a private config db
/// and a probe report.
pub fn spawn_settings(comp: &icedtea_harness::Compositor) -> SettingsProc {
    let dir = tempfile::tempdir().expect("tempdir");
    let report = dir.path().join("report");
    let child = Command::new(target_profile_dir().join("icedtea-settings"))
        .env("WAYLAND_DISPLAY", &comp.socket)
        .env("XDG_CONFIG_HOME", dir.path())
        .env("XDG_DATA_HOME", dir.path())
        .env("ICEDTEA_PROBE_REPORT", &report)
        .env("RUST_LOG", "warn")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn icedtea-settings");
    SettingsProc { child, report, dir }
}

impl SettingsProc {
    /// Every line reported so far.
    pub fn lines(&self) -> Vec<String> {
        let mut text = String::new();
        if let Ok(mut file) = std::fs::File::open(&self.report) {
            let _ = file.read_to_string(&mut text);
        }
        text.lines().map(str::to_owned).collect()
    }

    /// Poll the report until a line starting with `prefix` appears.
    pub fn wait_line(&self, prefix: &str, timeout: Duration) -> Option<String> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Some(line) = self.lines().into_iter().find(|l| l.starts_with(prefix)) {
                return Some(line);
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        None
    }

    /// The most recent `probe <id> <x> <y>` line's coordinates.
    pub fn point(&self, id: &str) -> Option<(i32, i32)> {
        let prefix = format!("probe {id} ");
        let line = self.lines().into_iter().rev().find(|l| l.starts_with(&prefix))?;
        let mut fields = line.split_whitespace().skip(2);
        Some((fields.next()?.parse().ok()?, fields.next()?.parse().ok()?))
    }

    /// The most recent `alloc <id> <x> <y> <w> <h>` line's box.
    pub fn allocation(&self, id: &str) -> Option<(f32, f32, f32, f32)> {
        let prefix = format!("alloc {id} ");
        let line = self.lines().into_iter().rev().find(|l| l.starts_with(&prefix))?;
        let mut f = line.split_whitespace().skip(2);
        Some((
            f.next()?.parse().ok()?,
            f.next()?.parse().ok()?,
            f.next()?.parse().ok()?,
            f.next()?.parse().ok()?,
        ))
    }
}

/// Move, press and release at `(x, y)` in output coordinates, with the settle
/// loop `ui/tests/support`'s `Driver::click` uses (surface focus is assigned
/// asynchronously; a press that races it lands nowhere).
pub fn click(pointer: &mut icedtea_harness::VirtualPointerClient, x: i32, y: i32) {
    for _ in 0..8 {
        pointer.motion_absolute(f64::from(x), f64::from(y), 1920, 1080);
        pointer.frame();
        pointer.pump();
        std::thread::sleep(Duration::from_millis(25));
    }
    pointer.button(icedtea_ui::wayland::BTN_LEFT, true);
    pointer.frame();
    pointer.pump();
    std::thread::sleep(Duration::from_millis(25));
    pointer.button(icedtea_ui::wayland::BTN_LEFT, false);
    pointer.frame();
    pointer.pump();
}

/// One pixel of a screencopy frame, as `ui/tests/support::pixel_at` reads it.
pub fn pixel_at(frame: &icedtea_harness::CapturedFrame, x: u32, y: u32) -> Option<(u8, u8, u8)> {
    icedtea_ui::shm::pixel_rgb(frame.format, &frame.bytes, frame.stride, x, y)
}

/// Whether `frame` contains any pixel that is not the compositor's wallpaper.
///
/// Sampled on a 4px grid: a screencopy of a 1920x1080 output is 2M pixels and
/// the question is only "did anything paint at all".
pub fn paints_something(frame: &icedtea_harness::CapturedFrame, background: (u8, u8, u8)) -> bool {
    (0..frame.height).step_by(4).any(|y| {
        (0..frame.width)
            .step_by(4)
            .any(|x| pixel_at(frame, x, y).is_some_and(|px| px != background))
    })
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p icedtea-settings --test skeleton`
Expected: FAIL — `cannot find function 'spawn_settings' in module 'support'` before the helpers
land, then the assertion `no probe block: the window never laid out` if the binary does not open
a toplevel.

- [ ] **Step 3: Make it pass**

No new production code should be needed — Tasks 6–10 built everything the test drives. Work any
failure as a real bug, in this order:

1. `probe root` never appears ⇒ the window never laid out. Check `Window::open` succeeded
   against the harness (`RUST_LOG=debug` and `.stderr(Stdio::inherit())` in `spawn_settings`),
   and that `main` passes `probe::report_path()` into `with_probe_report`.
2. `page displays` never appears ⇒ the click missed. Print `app.allocation("nav")` and the
   computed `displays_x`; the switcher's five buttons are linked and equal-width, so the fifth
   centre is `nav.x + nav.w * 0.9`.
3. The screencopy assertion fails ⇒ the toplevel painted but the harness composited nothing;
   capture twice with a 100 ms gap before asserting.

- [ ] **Step 4: Run the full gate set**

```bash
cd /home/joseph/Projects/icedtea/.claude/worktrees/m5
cargo test -p icedtea-settings
cargo test -p icedtea-ui
cargo test -p icedtea-ui --test gallery_gate
cargo test -p icedtea-ui --test interaction_gate
cargo test -p icedtea-ui --test window_events
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p icedtea-ui --all-targets --no-default-features -- -D warnings
cargo fmt --all --check
```

Expected: everything green; `interaction_gate` still 16/16.

- [ ] **Step 5: Commit**

```bash
git add settings/tests/
git commit -m "$(cat <<'MSG'
test(settings): gate the first Role::Toplevel App::run

App has only ever driven the gallery's layer surface, so the M5 spec calls
a toplevel App::run out as a risk of its own. This runs the real settings
binary against the harness compositor and asserts the three things that
could each have been silently broken: it lays out (its probe report names
every id view builds), it paints (screencopy differs from the wallpaper),
and it responds (a switcher click reaches Msg::PageSelected and puts the
Displays page body in the tree, while an insensitive Apply starts nothing).

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
MSG
)"
```

---

## Self-Review

### 1. Spec coverage

Every P1 obligation from the spec and from contract §5's "P1 — settings skeleton and pure-core
extraction" block, against the task that discharges it.

| Requirement (source) | Task |
|---|---|
| Spec §5.2 / contract §2.1: `workspaces::{next_workspace_name, prune_orphaned_workspace_bindings}` extracted with their tests | 3 |
| Spec §5.2 / contract §2.1: `keybindings::{format_combo, action_list, should_capture, take_capture_reset}` extracted, `unshifted_keysym` replaced by `normalise_keysym` | 2 |
| Contract §2.1: `appearance::{hex_to_rgba, rgba_to_hex, hex_to_packed, packed_to_hex}` re-typed onto `icedtea_ui::css::value::Rgba` with their tests | 4 |
| Spec §5.2 / contract §2.1: `displays.rs` split into control-panel / message / canvas modules — P1's half: `displays/{mod,state}.rs`, twelve tests moved verbatim, `Drag` added | 1 |
| Contract §2.1: `displays_canvas.rs`, `model.rs`, `compositor_reload.rs`, `outputs/protocol.rs` moved/kept with zero edits | 1, 10 (untouched; the no-GTK gate proves nothing crept in) |
| Contract §2.1: `settings/Cargo.toml` after P1 — `icedtea-ui` + `crossbeam-channel` in, `gtk4`/`gio`/`glib` out, `async-channel` kept | 4 (in), 10 (out) |
| Contract §2.1: `outputs/client.rs` deleted with `OutputsClient` | 10 |
| Contract §2.2: `SettingsModel`, `Msg` (`Send`, static assertion), `update` with the three Apply outcomes and Revert's displays exclusion | 6 |
| Contract §2.2: `Ctx`, `Page`, `Page::refresh`, `populating`, the `keybindings_page_slot` forward reference deleted; the populate guard not ported | 10 (deletions), recorded in the commit body and contract §6 by P6 |
| Contract §2.3: `view`, `nav`, `footer`, `PageId`, `page_infos`, per-page `view` functions, the normative widget ids | 5, 7 |
| Contract §2.4: `ipc::{WorkerHandles, spawn}`, `ipc::reload::{ReloadRequest, spawn}`, `Msg::Apply` as `Cmd::Task`, the `ConfigReloaded` subscription, no `process::exit` | 6 (reload half; `WorkerHandles::portal`/`choose_wallpaper` are P2's — deviation **P1-D7**) |
| Contract §2.5: `OutputsPump::{attach, watch, drain, test_configuration, build_and_send_configuration, flush}`, the `OutputsMsg` → `update` mapping, `Ok(None)` degradation | 8 |
| Contract §2.9 / spec §5.1: `settings/style.css` compiled and layered as a user-origin overlay, passed to `Window::open` | 10 |
| M5-D9 / contract §5: widget ids are addressable from a harness gate | 9 (the writer), 11 (the gate) |
| Contract §5 gate: "every moved test green **unmodified** (8 + 10 + 1 + 12 + 6 + 6)" | 1 (12), 2 (6), 3 (6), 4 (2 re-typed + 2 new), untouched: `displays_canvas` 8, `model` 10, `compositor_reload` 1 |
| Contract §5 gate: `the_first_toplevel_app_run_paints_and_navigates` | 11 |
| Contract §5 gate: `cargo tree` shows no `gtk4`/`gio`/`glib` | 10 |
| Spec §11 / contract §5: `live_apply.rs` untouched and still spawning the real compositor binary | no task edits it; Task 10's `no_gtk` gate checks the **normal** tree only, so its `gtk4` dev-dependency survives for P2/P3 |

**Gap check.** Contract §2.6–§2.8 (Appearance/Behavior/Workspaces/Keybindings/Displays page
bodies, the portal worker, the `DropDown` list height) are P2/P3/P4's by §5 and are deliberately
absent here; each page ships the frame and the id its later part fills. `settings/style.css`'s
`.conflict` rule is P3's by §5, so P1's sheet carries only the footer separator.

One of those gaps reaches back into a **§2.4 signature**, which is headed `(P1)`, so it is
enumerated as a deviation rather than left to the gap-check prose: `WorkerHandles`'s `portal`
field and `choose_wallpaper` method (and `ipc/portal.rs`, `PortalRequest`) ship at P2 with the
portal worker they exist to reach — **deviation P1-D7**, above, which also records what P2 must
add and the fact that `spawn`'s signature is already final. Two further §2.4/§2.5 shape notes are
likewise enumerated rather than implied: **P1-D8** (`ipc::handles_for_test`, additive, tests only)
and the second half of **P1-D2** (`SettingsModel::new`'s arity: §2.5's 3-argument snippet ships as
a 2-argument constructor plus `with_outputs`). Nothing else in §2.1–§2.5 is deferred: every other
item in those sections is discharged by a task in the table above.

### 2. Placeholder scan

Searched the plan for `TBD`, `TODO`, `implement later`, `fill in details`, `similar to Task`,
`add appropriate`, `handle edge cases`, `etc.` in a code position, and for code steps with no
code block. Findings and fixes:

- The five per-page `view` functions in Task 7 are the one place a reader could see a stub. They
  are not: each is complete, compiles, lays out, paints a label and carries the id its gates
  address. The table names which later part replaces which body, so no task is left to guess.
- Task 11 Step 3 ("Make it pass") deliberately contains no new production code, because Tasks
  6–10 built everything the gate drives; it lists the three concrete failure modes with the
  concrete diagnosis for each rather than saying "debug it".
- Task 1's Step 4 says "cut lines 24–293 and paste them" instead of reprinting 270 lines. That is
  the requirement, not a shortcut: the twelve tests only prove the move if the code is not
  retyped, and the three permitted edits are each spelled out with their exact replacement text.
- Every `use` path in the plan is a real item verified against the checkout
  (`icedtea_ui::css::value::Rgba`, `icedtea_ui::widgets::StackPageInfo`,
  `icedtea_ui::view::builders::{box_, stack, stack_page, stack_switcher, StackExt}`,
  `icedtea_ui::widgets::{button, label, spin_button, switch, drop_down, entry}`,
  `icedtea_ui::shm::pixel_rgb`, `icedtea_harness::{Compositor, ScreencopyClient,
  VirtualPointerClient, CapturedFrame}`, `crossbeam_channel::unbounded`).

### 3. Type consistency

- `Msg` — six variants, defined once in Task 6 and extended once in Task 8 (`Outputs`), used with
  the same names and payloads in Tasks 7, 8, 9, 10 and 11. `Msg::Applied` carries
  `Result<ReloadOutcome, String>` everywhere (worker, `update`, tests); `Msg::Outputs` carries
  `Arc<OutputsMsg>` everywhere, never `Rc`, which is what the `assert_send::<Msg>()` static
  assertion in Task 6 enforces.
- `SettingsModel::new(db_path: PathBuf, workers: WorkerHandles)` in Task 6 is exactly what Task 8's
  `with_outputs` chains onto and what Task 10's `main` calls. It is two arguments where contract
  §2.5's snippet writes three (`…, pump.clone()`); that substitution is deviation P1-D2 and is
  the only difference between §2.5's wiring and Task 10's `main`.
- `WorkerHandles::apply(cfg: Config, db_path: PathBuf)` in Task 6 matches the `Cmd::Task` closure
  in `update`'s `Apply` arm and `ipc::reload::ReloadRequest::Apply { cfg, db_path }`. The sibling
  `choose_wallpaper(current: Option<PathBuf>)` that contract §2.4 lists beside it has no caller
  and no worker at P1 and is P2's (deviation P1-D7); its signature is not restated or altered
  anywhere in this plan.
- `OutputsPump::{attach -> io::Result<Option<Self>>, watch -> WatchId, drain -> Vec<Msg>}` in
  Task 8 matches Task 10's `main` (`attach`, `map(OutputsPump::watch)`, `on_fd(id, move || pump.drain())`)
  and Task 8's own harness test.
- `PageId::{ALL, name, title, from_index, index}` in Task 5 is what Task 6's `PageSelected` arm,
  Task 7's `view`/`nav` and Task 9's `page <name>` report line all use; `page_infos()` returns
  `Rc<[StackPageInfo]>`, which is exactly `stack_switcher`'s parameter type.
- `footer_text(&SettingsModel) -> &str` is defined once (Task 7) and used by `footer` and by
  Task 9's status line, so the gate and the label can never disagree.
- `state::{DisplaysState, Drag, reconcile, baseline_edit}` from Task 1 are used with those exact
  names in Task 6's model, Task 8's `update` arm and (later) P4.
- The report format is written by Task 9 (`probe <label> <x> <y>`, `alloc <id> <x> <y> <w> <h>`,
  `frame <n>`) and parsed by Task 11's `point`/`allocation`/`wait_line` with matching field
  counts and offsets (`skip(2)` past the keyword and the id).
