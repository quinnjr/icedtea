# B1 App Launcher Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the Win7/10-style app launcher (search, pinned, All-apps, tiles, power row) as a dedicated layer-shell surface in the shell process, launched via a new allowlist `SpawnApp` D-Bus method.

**Architecture:** A pure launcher core (`shell/src/launcher/`, no Wayland) owns the `.desktop` index, fuzzy matcher, and pin/tile/recency stores; a new `Launcher` surface + Start button render it with existing toolkit widgets; the compositor gains one `SpawnApp(app_id)` method that resolves against its own index copy and execs through the existing `parse_spawn_argv` path (never raw argv over the bus).

**Tech Stack:** Rust, `wayland-client` + `wayland-protocols-wlr` layer-shell, `icedtea-ui` toolkit (SearchEntry, buttons, lists), zbus D-Bus, redb config DB, `x11rb`-free (no X11 here).

**Spec:** `../specs/2026-09-12-b1-app-launcher-design.md` (full Win7/10 shape; bottom default w/ top option; `SpawnApp(app_id)` ruling over raw-argv passthrough — see `compositor/src/state.rs` `apply_action` doc which forbids wiring `spawn` to D-Bus).

## Global Constraints

- Worktree `.worktrees/b1-launcher`, branch `feature/b1-app-launcher` (already created off `develop` tip `8f1d450`).
- Never `unwrap`/`expect` inside Wayland callback or D-Bus handler paths that run on foreign threads — record state, assert after the run (repo C-frame rule).
- No raw argv crosses D-Bus, ever (`apply_action` hardening rationale). `SpawnApp` takes an app-id; the compositor resolves it against its own index.
- TDD, bite-sized commits, scoped review per task; whole-branch review before merge.
- Gates per task: `cargo test --workspace -- --test-threads=1`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --all --check`.
- Known pre-existing failures (not yours): 3 `icedtea-ui` `gallery_gate` render tests (image/window_controls/expander paint nothing); unused-`ids` warning in `shell/src/panel.rs` m8 test.

---

## File structure

- `shell/src/launcher/mod.rs` — pure core façade: `DesktopIndex::scan(dirs)`, `Matcher::rank(query)`, `PinStore`/`TileStore`/`RecencyStore` ops. No Wayland imports.
- `shell/src/launcher/entry.rs` — `DesktopEntry { id, name, exec, icon, categories, keywords }` + `.desktop` file parser (Name/Exec/Icon/Categories/Keywords/NoDisplay/OnlyShowIn/NotShowIn, `Exec` field-code handling, locale `Name[xx]` fallback to `Name`).
- `shell/src/launcher/config_ext.rs` — `LauncherConfig { pinned: Vec<String>, tile_groups: Vec<TileGroup>, recency: HashMap<String, (u64, u64)> }` + serde defaults merged into `config::Config` beside `appearance` (see `config/src/lib.rs:42`), defaults in `config/src/defaults.rs` (follows `bar_position: "bottom"` at `defaults.rs:52`).
- `shell/src/launcher_view.rs` — `LauncherModel`, `LauncherMsg`, `view()` (search box, pinned rail, All-apps list, tiles pane, power row), open/close/focus behavior.
- `shell/src/panel.rs` — Start button (bar left end) + toggle wiring; `spec()` reads `bar_position` (see Task 3).
- `contract/src/types.rs` — `Appearance` already carries `bar_position: String`; no contract change (launcher reads config locally; snapshot untouched).
- `compositor/src/dbus.rs` — `DbCommand::SpawnApp { app_id: String, reply: Sender<bool> }` (follows `GetState` round-trip shape at `dbus.rs:92`) + `spawn_app(&self, app_id: String) -> bool` interface method (follows `focus_window` at `dbus.rs:361`, wire member `SpawnApp`).
- `compositor/src/state.rs` — `handle_command` arm resolving `app_id` against a server-side index copy, exec via existing `parse_spawn_argv` (`state.rs:1156`) + `Command::new(program).args(args).spawn()`.
- `compositor/tests/launcher_spawn.rs` (new) + `shell` unit/render tests — e2e pins.

---

### Task 1: Pure launcher core — parse, index, match, stores

**Files:**
- Create: `shell/src/launcher/mod.rs`, `shell/src/launcher/entry.rs`
- Test: `shell/src/launcher/mod.rs` (`#[cfg(test)]` unit tests inline, repo shell-test style)

**Interfaces:**
- Consumes: XDG dirs (`/usr/share/applications`, `~/.local/share/applications`), `config::Config` (read-only).
- Produces: `DesktopEntry`, `DesktopIndex::scan(&[PathBuf]) -> Vec<DesktopEntry>` (skips `NoDisplay=true`, honors `OnlyShowIn`/`NotShowIn` against `{"icedtea"}`), `Matcher::rank(&self, query: &str) -> Vec<&DesktopEntry>` (substring + word-boundary + prefix bonuses, recency tiebreak input), `PinStore::{pin,unpin,is_pinned}`, `TileStore::{groups,assign}`, `RecencyStore::record`.

- [ ] **Step 1: Write failing tests** for parse + rank:
```rust
#[test]
fn parse_skips_nodisplay_and_honors_onlyshowin() {
    let e = parse_entry("[Desktop Entry]\nName=Foo\nExec=foo\nNoDisplay=true\n").unwrap();
    assert!(e.nodisplay);
    let idx = DesktopIndex::from_entries(vec![e]);
    assert!(idx.apps().is_empty());
}
#[test]
fn rank_prefers_prefix_over_substring() {
    let idx = DesktopIndex::from_entries(vec![
        entry("x Firefox", "firefox"), entry("y Fire tools", "firetools"),
    ]);
    assert_eq!(rank(&idx, "fire")[0].name, "x Firefox");
}
```
- [ ] **Step 2: Run** `cargo test -p icedtea-shell launcher` — FAIL (no such module)
- [ ] **Step 3: Implement** `entry.rs` parser (line `Key=Value`, `[xx]` locale fallback, `%f/%F/%u/%U/%i/%c/%k` field codes: strip for display, keep raw `Exec` for launch), `mod.rs` index + matcher + three stores (HashMap/Vec, capped recency at 200 entries with prune).
- [ ] **Step 4: Run** — PASS, full `cargo test -p icedtea-shell` green
- [ ] **Step 5: Commit** `feat(shell): pure launcher core (parse, index, match, stores)`

### Task 2: Config schema — launcher stores beside appearance

**Files:**
- Modify: `config/src/lib.rs` (`Config` struct ~line 42), `config/src/defaults.rs` (beside `bar_position` line 52)
- Test: `config` crate tests (follow existing defaults tests)

**Interfaces:**
- Consumes: Task 1 `LauncherConfig` shape.
- Produces: `Config.launcher: LauncherConfig` with serde defaults (`pinned: []`, `tile_groups: []`, `recency: {}`), persisted through the existing redb path untouched.

- [ ] **Step 1: Write failing test** asserting `Config::default().launcher.pinned.is_empty()` and a round-trip with one pinned id + one tile group.
- [ ] **Step 2: Run** `cargo test -p icedtea-config` — FAIL (no such field)
- [ ] **Step 3: Implement** struct + defaults (mirror `Behavior` at `lib.rs:35` exactly: derive list + field style).
- [ ] **Step 4: Run** — PASS
- [ ] **Step 5: Commit** `feat(config): launcher stores schema (pinned, tiles, recency)`

### Task 3: Bar position wiring + Start button

**Files:**
- Modify: `shell/src/panel.rs` (`spec()` at line ~57, `view()` bar children)
- Test: `shell/tests/` panel rest-state gates (existing) + new anchor assertion

**Interfaces:**
- Consumes: `Appearance.bar_position` (`"bottom"` default, `"top"` option; settings UI already writes it — `settings/src/app.rs:400`).
- Produces: `spec()` anchors Left+Right+Bottom (exclusive-zone `BAR_HEIGHT`) when bottom, Left+Right+Top when top; Start button as first bar child emitting toggle.

- [ ] **Step 1: Verify-first** — grep that nothing else reads `bar_position` for layout (`dbus.rs:661` and `defaults.rs:52` are default seeds, not layout). Then write failing test: `spec()` with bottom config anchors Bottom-not-Top and vice versa.
- [ ] **Step 2: Run** — FAIL (spec hardcodes Top; see `panel.rs:39` doc comment about the VM-console short-viewport caveat — keep exclusive-zone behavior identical, only the anchored edge changes).
- [ ] **Step 3: Implement** anchor switch + Start button (toolkit `button("start")`, class `.start`, toggles launcher open state in `PanelModel`).
- [ ] **Step 4: Run** panel gates + new test — PASS
- [ ] **Step 5: Commit** `feat(shell): bar_position honored + Start button`

### Task 4: SpawnApp D-Bus method (allowlist, no raw argv)

**Files:**
- Modify: `shell/src/compositor_client.rs` (trait + proxy, follows `focus_window` at lines 159–189), `compositor/src/dbus.rs` (`DbCommand::SpawnApp` beside `Quit` line 91; interface method beside `focus_window` line 361), `compositor/src/state.rs` (`handle_command` arm)
- Test: `compositor/tests/launcher_spawn.rs` (new)

**Interfaces:**
- Consumes: Task 1 entry shape (server keeps its own index copy built the same way), existing `parse_spawn_argv` (`state.rs:1156`).
- Produces: `fn spawn_app(&self, app_id: &str) -> bool` (trait + `CompositorProxy` via `call_method(..., "SpawnApp", &(app_id,))`); compositor resolves id → entry → `Command::new(program).args(args).spawn()`; unknown id / unparseable Exec → `false` + warn log, never spawn.

- [ ] **Step 1: Write failing e2e** `spawn_app_unknown_id_returns_false_and_spawns_nothing` (bind a harness client? No — call the interface on a headless compositor with a bogus id, assert `false`; then seed a real `.desktop` dir, assert `true`).
- [ ] **Step 2: Run** — FAIL (no such method)
- [ ] **Step 3: Implement** trait + mock recording (follow `MockWm` pattern for `CompositorCommands`), proxy call, interface method, `DbCommand` + arm. The arm MUST NOT accept argv (hardening rationale at `state.rs:4919` — cite it in a comment).
- [ ] **Step 4: Run** new e2e + workspace suite slice — PASS
- [ ] **Step 5: Commit** `feat(compositor): allowlist SpawnApp D-Bus method`

### Task 5: Launcher surface + view + keyboard model

**Files:**
- Create: `shell/src/launcher_view.rs`
- Modify: `shell/src/panel.rs` (surface registry beside panel `spec()`), `shell/src/main.rs` or `lib.rs` (second surface boot — follow panel boot)
- Test: offscreen render tests (new `shell/tests/launcher.rs`)

**Interfaces:**
- Consumes: Tasks 1–3 (`DesktopIndex`, stores, `spec()` anchor logic), `SearchEntry` (`search_entry(&str)` + `on_search`, `ui/src/widgets/search_entry.rs:35`, IME-capable).
- Produces: bottom-left (or top-left) `Layer::Top` surface, `KeyboardInteractivity::Exclusive` (verify variant name against `wayland-protocols-wlr` first — panel uses `None` at `panel.rs:64`), search→filter→Enter-to-launch, arrows across panes, Escape/focus-loss close, right-click pin/unpin, `.suggested-action` top hit, own rest gates.

- [ ] **Step 1: Write failing render tests** rest state per theme (Adwaita light/dark pattern from settings page gates) + open→search→dismiss transitions.
- [ ] **Step 2: Run** — FAIL
- [ ] **Step 3: Implement** model/msgs/view (search box, pinned rail, All-apps list w/ letter jump, tiles pane, power row UI shell with no-op handlers — Task 6 wires them), surface boot + Start toggle + close paths.
- [ ] **Step 4: Run** — PASS
- [ ] **Step 5: Commit** `feat(shell): launcher surface, view, keyboard model`

### Task 6: Power row backends + full e2e + gates

**Files:**
- Modify: `shell/src/launcher_view.rs` (power handlers), `shell/src/launcher/mod.rs` (argv builders, pure + unit-tested)
- Test: `compositor/tests/launcher_spawn.rs` (extend), `shell/tests/launcher.rs` (extend)

**Interfaces:**
- Consumes: Tasks 4–5. Produces: working power row; proven spawn path.

- [ ] **Step 1: Write failing tests** — pure argv builders: `power_argv(Lock) == ["loginctl","lock-session"]`, suspend/poweroff/reboot likewise, logout → compositor quit path (no argv); each shell-out failure surfaces as a launcher status line (test the status-message mapping, not the exec).
- [ ] **Step 2: Run** — FAIL
- [ ] **Step 3: Implement** builders + handlers (`Command::new().args().status()`, failure → status line; every call site commented `// A3-logind: replace with logind client`). Launch path: Enter → `spawn_app(entry.id)` → on `true`, record recency + close.
- [ ] **Step 4: Write failing e2e** open-from-Start → type → launch asserts spawn reached compositor (observability: `DbCommand::SpawnApp` reply `true` + a `MockWm`-recorded call) and recency recorded; then gut the `spawn_app` call and confirm FAIL (deletion test), restore.
- [ ] **Step 5: Run** full gates — PASS
- [ ] **Step 6: Commit** `feat(shell): power row backends + launcher e2e`

## Self-Review

- Spec §Architecture → Tasks 3+5 (surface/process); §Position config → Task 3; §Index+stores → Tasks 1+2; §Layout+keyboard → Task 5; §Spawn+power → Tasks 4+6; §Testing → Tasks 1/5/6 (unit, render, e2e+deletion); §Out of scope respected (no file providers, no tile DnD, no adaptive ordering, no A3, no separate process).
- No placeholders; every step names files, exact names (`parse_spawn_argv`, `SpawnApp`, `SearchEntryExt::on_search`, `bar_position`), and expected outputs.
- Type consistency: `DesktopEntry/LauncherConfig/PinStore/TileStore/RecencyStore`, `spawn_app(&str)->bool`, `DbCommand::SpawnApp{app_id,reply}`, `bar_position: String` threaded unchanged.
