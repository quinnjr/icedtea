# icedtea Displays (zwlr_output_management_v1 + drag canvas) Implementation Plan

> **For agentic workers:** execute via subagent-driven-development / ultracode. Tasks are phased; **T8 (publish) and T9 (merge) are hard stops for user consent.** The `/code-review` fixes are already landed (commit `6e4d717`) — this plan covers the Displays feature only.

**Goal:** Advertise `zwlr_output_management_v1` from the compositor and add a settings-app Displays page (visual drag canvas + per-monitor resolution/refresh/scale/transform/enabled), with the compositor persisting applied layouts to redb and re-applying on connect/hotplug.

**Architecture:** Compositor advertises wlroots' `wlr_output_manager_v1`; it handles `apply`/`test` **synchronously** in the wlr crate's C handler (commit output state + manual position), then emits an owned-data notification so the compositor re-derives geometry and persists to `config.displays`. The settings app is a `zwlr_output_management_v1` **client** over a second `wayland-client` connection dispatched on a `gio::Socket` glib source (GDK untouched). Persistence keyed by connector **name**.

**Tech Stack:** Rust; the user's `wlr` crate over wlroots 0.20 (→ publish **0.20.22**); redb; GTK4 0.11 (gtk4/gio/glib 0.22); `wayland-client` 0.31 + `wayland-protocols-wlr` 0.3 (`client`).

**Spec:** `docs/superpowers/specs/2026-08-19-icedtea-displays-and-review-fixes-design.md`

## Global Constraints

- **`wlr` is pinned from crates.io** (`links = "wlroots"` forbids a path dep in the published graph). During development, build the compositor against the local wlr via a **`[patch."crates.io"] wlr = { path = "<wlroots-sys worktree>/crates/wlr" }`** in the icedtea root `Cargo.toml`. The patch is **removed** at T8 when `wlr = "0.20.22"` is published and the dep bumped.
- **Additive-only** wlr changes within 0.20.x. New public symbols (`Output::{modes,set_mode,set_scale,set_transform,disable}`, `Mode`, `Transform`, `Runtime::{create_output_manager,update_output_manager_state}`, the defaulted `OutputHandler::output_configuration_applied`) must be *wrapped, not waived* in the M5 symbol audit.
- **The `wlr` 0.20.22 publish (T8) and the branch merge (T9) are hard stops** requiring explicit user consent. The publish touches the shared wlroots-sys repo — coordinate with the M5 session via SendMessage first.
- Commit trailer on every commit: `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`.
- Empty/missing `displays` config ⇒ today's behavior exactly. No `SCHEMA_VERSION` bump.
- No `unsafe` in icedtea's own crates (keep it inside gio/glib/wlr); wlr-crate FFI follows the existing MaybeUninit+`wlr_output_state_finish`-on-every-path idiom.

---

## Phase 1 — config (icedtea-local, builds today, no wlr dep)

### Task 1: `displays: Vec<DisplayConfig>` config section

**Files:**
- Modify: `contract/src/types.rs` (define `DisplayConfig`), `contract/src/lib.rs` (export)
- Modify: `config/src/lib.rs` (field + save + load), `config/src/schema.rs` (table const), `config/src/defaults.rs` (default)
- Modify: `compositor/src/state.rs` test call sites that build a full `Config { .. }` literal (~4724/4761/4810) if any don't use `..default_config()`
- Test: `config/src/lib.rs` `#[cfg(test)]`

**Interfaces — Produces:**
```rust
// contract/src/types.rs
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DisplayConfig {
    pub name: String,      // connector, match key, e.g. "DP-1"
    pub enabled: bool,
    pub width: i32, pub height: i32,  // 0×0 => preferred
    pub refresh_mhz: i32,             // 0 => backend picks
    pub x: i32, pub y: i32,
    pub scale: f64,        // 1.0 default
    pub transform: i32,    // wl_output.transform value; 0 = normal
}
// config: pub displays: Vec<DisplayConfig> on Config
```

**Steps:**
- [ ] Define `DisplayConfig` in `contract/src/types.rs`; export via `contract/src/lib.rs`; `pub use contract::DisplayConfig;` in `config/src/lib.rs` next to `pub use contract::Appearance;`.
- [ ] `schema.rs`: `pub const DB_DISPLAYS: TableDefinition<&'static str,&'static [u8]> = TableDefinition::new("displays"); pub const KEY_DISPLAYS: &str = "displays";`
- [ ] `Config` struct: add `pub displays: Vec<DisplayConfig>` after `workspace_names`.
- [ ] `load_or_default`: after the workspace_names block, add the `if let Ok(t)=open_table(DB_DISPLAYS) && let Some(v)=read_json::<Vec<DisplayConfig>>(&t,KEY_DISPLAYS) { cfg.displays = v; }` chain (**no** `!is_empty()` guard — an empty vec is a legitimate "no displays configured" state).
- [ ] `Config::save`: after the workspaces block, open `DB_DISPLAYS`, `serde_json::to_vec(&self.displays)`, insert under `KEY_DISPLAYS`.
- [ ] `defaults.rs`: `displays: vec![],`.
- [ ] Fix any full-`Config`-literal build sites (compositor tests) — add `displays: vec![]` or convert to `..default_config()`.
- [ ] Tests: default is empty; save+load round-trips a populated `Vec<DisplayConfig>`; a DB with no `DB_DISPLAYS` table loads `displays == vec![]`.
- [ ] `cargo test -p icedtea-config` green; `cargo build -p icedtea-config -p icedtea-compositor` green.

---

## Phase 2 — wlr crate (in a dedicated wlroots-sys worktree; compositor builds against it via `[patch]`)

**Setup (controller, before T2):** create a worktree off wlroots-sys `develop` (isolated from the M5 worktree), e.g. `git -C /home/joseph/Projects/wlroots-sys worktree add .claude/worktrees/wlr-output-mgmt -b feature/wlr-output-management develop`; add the `[patch."crates.io"] wlr = { path = ".../crates/wlr" }` to the icedtea root `Cargo.toml` pointing at that worktree.

### Task 2: `Output` state setters + `modes()` + `Mode`/`Transform` types

**Files:** Modify `crates/wlr/src/output.rs`, `crates/wlr/src/lib.rs` (exports); Test: output.rs `#[cfg(test)]` via `ScratchOutput`.

**Interfaces — Produces:**
```rust
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub struct Mode { pub width: i32, pub height: i32, pub refresh_mhz: i32, pub preferred: bool }
pub enum Transform { Normal, _90, _180, _270, Flipped, Flipped90, Flipped180, Flipped270 }
impl Output<'_> {
    pub fn modes(&self) -> Vec<Mode>;
    pub fn set_mode(&self, width: i32, height: i32, refresh_mhz: i32) -> Result<()>; // set_custom_mode
    pub fn set_scale(&self, scale: f32) -> Result<()>;
    pub fn set_transform(&self, t: Transform) -> Result<()>;
    pub fn disable(&self) -> Result<()>;
}
```

**Steps:**
- [ ] `modes()`: `wl_list_for_each!(&raw mut (*self.as_ptr()).modes, sys::wlr_output_mode, link)`, map each to `Mode`, collect. Pure read.
- [ ] Setters: each copies `enable_with_preferred_mode()`'s scaffold (MaybeUninit → `wlr_output_state_init` → the one setter → `wlr_output_commit_state` → **`wlr_output_state_finish` on every path** → `Result`). `set_mode` uses `wlr_output_state_set_custom_mode(w,h,refresh)` (headless/nested lack fixed modes — document the caveat). `disable` uses `set_enabled(false)`.
- [ ] `Transform` + `From<Transform> for sys::wl_output_transform` (locally-bindgen'd `WL_OUTPUT_TRANSFORM_*`); export `Mode`/`Transform` from lib.rs.
- [ ] Test: `ScratchOutput` with a `wl_list_init`'d modes list + one linked `wlr_output_mode`; assert `modes()` returns it. Mutation-verify.
- [ ] `cargo test -p wlr` green.

### Task 3: `Runtime::create_output_manager` + state field

**Files:** Modify `crates/wlr/src/runtime.rs`.
**Interfaces — Produces:** `pub fn create_output_manager(&self, display: &Display) -> Result<()>`; `pub(crate) fn output_manager_ptr(&self) -> Option<NonNull<sys::wlr_output_manager_v1>>`.

**Steps:**
- [ ] Add `output_manager: RefCell<Option<NonNull<sys::wlr_output_manager_v1>>>` to `RuntimeInner` (~L167), init `None` in the constructor.
- [ ] Copy `create_pointer_constraints_manager` verbatim, swapping to `wlr_output_manager_v1_create` / `Error::Create("wlr_output_manager_v1_create")` / double-call guard.
- [ ] `cargo build -p wlr` green.

### Task 4: apply/test handlers + head notification + advertise-state sync

**Files:** Modify `crates/wlr/src/backend.rs` (extern-C handlers + wiring), `crates/wlr/src/handler.rs` (defaulted `OutputHandler` method), `crates/wlr/src/runtime.rs` (`update_output_manager_state`); Test: `compositor/tests/client_protocol.rs` (advertised global).

**Interfaces — Produces:**
```rust
// handler.rs — additive, defaulted (non-breaking, mirrors SeatHandler::session_lock_changed)
pub struct AppliedHead { pub name: Option<String>, pub enabled: bool,
    pub width: i32, pub height: i32, pub refresh_mhz: i32,
    pub x: i32, pub y: i32, pub scale: f32, pub transform: Transform }
trait OutputHandler {
    fn output_configuration_applied(&mut self, _heads: Vec<AppliedHead>) {}
}
// runtime.rs
impl Runtime { pub fn update_output_manager_state(&self) { /* rebuild config from layout, wlr_output_manager_v1_set_configuration */ } }
```

**Steps:**
- [ ] Wire `events.apply`→`on_output_manager_apply::<S>` and `events.test`→`on_output_manager_test::<S>` via `Registration::link_bare` in the backend manager-setup block (after L1518), guarded by `output_manager_ptr()`.
- [ ] `on_output_manager_apply`: `bound_of(l)`→session/runtime; `NonNull(data as *mut wlr_output_configuration_v1)` guard (non-null case). **Collect head ptrs first** (`wl_list_for_each!` over `.heads`). Per head: build `wlr_output_state`, `wlr_output_head_v1_state_apply(&head.state, &mut st)`, `wlr_output_commit_state` (test path: `wlr_output_test_state`), finish state; track all-ok; on apply, set position via `runtime.set_output_position(id, head.state.x, head.state.y)` (state_apply does **not** do position). Then `send_succeeded` XOR `send_failed`, then **`wlr_output_configuration_v1_destroy(config)`** (owned). Fully synchronous.
- [ ] After a successful apply, build `Vec<AppliedHead>` (owned data — name via `output.name()`, resulting mode/pos/scale/transform) and invoke `output_configuration_applied` on the handler. **No borrowed config crosses the boundary.**
- [ ] `update_output_manager_state()`: create a fresh `wlr_output_configuration_v1` from the current layout heads and `wlr_output_manager_v1_set_configuration`.
- [ ] `client_protocol.rs`: `output_manager_global_is_advertised` asserting `advertised_globals` contains `"zwlr_output_manager_v1"` (copy the session_lock test).
- [ ] `cargo test -p wlr` + the compositor advertised-global test (via patch) green. Mutation-verify.

---

## Phase 3 — compositor (builds against patched wlr)

### Task 5: advertise manager, apply persisted config, handle applies, persist

**Files:** Modify `compositor/src/lib.rs` (create_output_manager), `compositor/src/state.rs` (OutputSurface name, new_output lookup, `OutputHandler::output_configuration_applied`, persist); Test: `compositor/tests/` harness multi-output.

**Steps:**
- [ ] `OutputSurface` (state.rs:45): add `pub name: String`; populate in `new_output` from `output.name()` (also keep an `output_names: HashMap<u32,String>` if a destroy-time reverse lookup is needed).
- [ ] `lib.rs::run()`: `if let Err(err) = runtime.create_output_manager(&display) { tracing::error!(%err, "output-management unavailable"); }` in the manager block, before the fatal `create_seat`.
- [ ] `new_output`: after `init_output`, look up `self.config.displays` by `output.name()`. Present+enabled → `set_mode`/`set_scale`/`set_transform` then `set_output_position`; **fall back to `enable_with_preferred_mode()` + auto on any setter error** (never leave it dark). Present+disabled → `disable()`. Absent → current path. Then the existing geometry/scene sequence (output_layout_box → create_output → sync_wallpaper_nodes → background rect → arrange_layers → resolve_orphaned_layers → schedule_frame). Call `runtime.update_output_manager_state()` at the end.
- [ ] `impl OutputHandler for State`: `output_configuration_applied(heads)`: for each head map `name`→index, re-derive geometry (`output_layout_box`) and re-run the create_output/arrange/sync sequence; windows on a disabled/shrunken output migrate via existing `migrate_windows_from`. Then **persist**: upsert each head into `config.displays` and write via the existing off-loop redb pattern (`spawn_config_reload`/`drain_config_reload` sibling — a `spawn_config_save`), never blocking the handler on redb I/O.
- [ ] `destroyed`: call `update_output_manager_state()` after removal.
- [ ] Tests (harness, two headless outputs): drive an `OutputConfiguration` positioning B right-of A at a chosen mode → assert layout boxes/usable areas + that `DB_DISPLAYS` persisted; disabled-output path → windows migrate off; invalid config → `send_failed`, no state change; restart with the persisted DB → layout re-applied on `new_output`. Mutation-verify each.
- [ ] Full `cargo test -p icedtea-compositor` (via patch) green.

---

## Phase 4 — settings app Displays page (talks to compositor via the protocol; no direct wlr dep)

### Task 6: GTK-free `outputs/` protocol client + glib fd source

**Files:** Create `settings/src/outputs/mod.rs` (+ `client.rs`); Modify `settings/Cargo.toml`, `settings/src/lib.rs`; Test: `settings/tests/outputs_client.rs` (against harness).

**Steps:**
- [ ] `settings/Cargo.toml`: add `wayland-client = "0.31"`, `wayland-protocols-wlr = { version = "0.3", features = ["client"] }` (**no `unstable` feature — it doesn't exist**), and (for clarity) `gio = "0.22"`, `glib = "0.22"`, `rustix` (for a safe `dup`).
- [ ] `outputs/` (GTK-free): a second `wayland_client::Connection::connect_to_env()` + `EventQueue<OutputsState>` + `Dispatch` impls for `wl_registry`, `zwlr_output_manager_v1`, `zwlr_output_head_v1`, `zwlr_output_mode_v1`, `zwlr_output_configuration_v1`, `zwlr_output_configuration_head_v1`. Plain-data model `Head { name, description, modes: Vec<Mode>, current, pending, x, y, scale, transform, enabled }`. If the manager global is absent, expose that as a state (page shows "output management unavailable").
- [ ] glib integration: `rustix::io::dup` the queue's `BorrowedFd` → `OwnedFd` → `gio::Socket::from_fd` → `SocketExtManual::create_source(IOCondition::IN, None, name, Priority::DEFAULT, cb)` → `.attach(Some(&glib::MainContext::default()))`. Callback: `dispatch_pending`; then only if none pending, `prepare_read()?.read()?; dispatch_pending()`. **Never** `blocking_dispatch` on the GTK thread.
- [ ] Public surface for the view: `heads() -> &[Head]`, `build_and_send_configuration(edits)`, and an `async-channel` sender emitting `HeadsChanged` / `ApplySucceeded` / `ApplyFailed`.
- [ ] Test (`icedtea-harness` dev-dep): boot the harness compositor (once it advertises the manager via patch), enumerate heads, apply a config, assert succeeded — no GTK.

### Task 7: Displays page — drag canvas + controls

**Files:** Create `settings/src/pages/displays.rs`; Modify `settings/src/pages/mod.rs`, `settings/src/main.rs` (add to Stack); Test: `settings/src/pages/displays.rs` GTK-free geometry unit tests (+ a `displays_gtk.rs` integration binary if a live widget assertion is needed).

**Steps:**
- [ ] GTK-free geometry helpers (unit-tested): shared scale factor for the canvas, hit-testing a point to a head rect, edge-snapping a dragged rect to neighbors/origin → resulting `x,y`.
- [ ] `GtkDrawingArea` + drag gesture drawing each enabled head scaled; drag repositions with snapping; selection focuses the side controls.
- [ ] Side controls for the selected head: Enabled switch, Resolution dropdown (distinct w×h from `Head.modes`), Refresh dropdown (for the chosen w×h), Scale spin, Transform dropdown, read-only Position.
- [ ] Wire to `outputs/` via async-channel: `HeadsChanged` rebuilds the canvas; **Test** button → protocol test; **Apply** → `build_and_send_configuration`; `ApplyFailed` shows the rejection and reverts the canvas. No redb writes (compositor persists).
- [ ] Add the page to the Stack in `main.rs` (`add_titled(&displays.root, Some("displays"), "Displays")`); compositor-absent → controls disabled with the explanatory label.
- [ ] `cargo test -p icedtea-settings` + `cargo clippy -p icedtea-settings --all-targets -- -D warnings` green.

---

## Phase 5 — publish gate + finalize

### Task 8: publish `wlr` 0.20.22 + flip the dep  — **HARD STOP (user consent + M5 coordination)**
- [ ] STOP: present the publish window; confirm with the M5 session (SendMessage) that no conflicting develop merge/publish is in flight.
- [ ] Merge `feature/wlr-output-management` → wlroots-sys `develop` (its own review/merge on that repo), bump `wlr` version to 0.20.22, `cargo publish` (libsecret token — no `--token`).
- [ ] icedtea: bump `compositor/Cargo.toml` `wlr = "0.20.22"`; **remove the `[patch]`**; `cargo update -p wlr`; full `cargo test` workspace green against the published crate.

### Task 9: final whole-branch review + merge  — **HARD STOP (user consent)**
- [ ] Dispatch the final whole-branch review (most capable model) over `settings-app` (the fixes + all Displays work).
- [ ] One scoped fix pass for any findings; re-review.
- [ ] STOP: present the merge window (local `--no-ff` `settings-app` → develop). No further publish.

## Self-review notes
- Spec coverage: A1 name-matching→T1/T5; A2 config→T1; A3 wlr→T2/T3/T4; A4 compositor→T5; A5 (no D-Bus)→covered by protocol; A6 client→T6; A6 canvas→T7; A7 tests→per task; Part B fixes→already landed (6e4d717).
- Types consistent across tasks: `Mode`/`Transform`/`AppliedHead` (wlr), `DisplayConfig` (contract), `Head` (settings).
- Correction vs earlier assumptions baked in: no glib `unix_fd_add` (use `gio::Socket`), no `wayland-protocols-wlr` `unstable` feature, no `SCHEMA_VERSION` bump, manual position apply, synchronous config handling + destroy.
