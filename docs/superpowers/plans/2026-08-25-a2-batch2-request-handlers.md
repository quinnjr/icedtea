# A2 Batch 2 — Request-Handler Protocols — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Add three request-driven Wayland compat protocols — **cursor-shape** (named cursors from the theme), **xdg-activation** (focus-steal prevention / request-attention), and **gamma-control** (night-light backend) — by publishing one additive `wlr` release and implementing their compositor handlers.

**Architecture:** Unlike Batch 1's passive globals, each of these needs the `wlr` crate to expose an event (a wlroots signal listener fanned out to a new handler-trait method) AND the compositor to act on it. cursor-shape reuses the crate's existing `wlr_cursor` + `wlr_xcursor_manager` (created with the seat) to set a named xcursor; xdg-activation runs a focus-steal policy and, when it declines to steal focus, raises an additive `attention` bit on the `org.icedtea.Compositor` window state; gamma-control applies a per-output LUT in the existing `OutputHandler::frame` commit path.

**Tech Stack:** Rust 2024; `wlr` safe wrapper over wlroots 0.20; harness clients bind from `wayland-protocols`.

**Spec:** `docs/superpowers/specs/2026-08-20-icedtea-compat-protocols-design.md` (milestone **M-A2.2**, Batch 2 section + Decisions 4/5/7 + Verification #4). This plan implements Batch 2 only; Batch 3 (foreign-toplevel-management) is a separate later plan. Batch 1 (M-A2.1, passive globals) is already merged (develop @ d624ff7, wlr 0.20.24).

## Global Constraints

- **Version allocated at publish time — do NOT hard-code.** Next free `0.20.x` after 0.20.24 is expected **0.20.25**; confirm unclaimed on crates.io (`cargo search wlr`) and coordinate with any parallel wloots-sys sessions (`SendMessage`) immediately before publishing.
- **Every `cargo publish` and every branch merge is a hard-stop for owner consent.** Present ready → wait for explicit go. Never auto-publish/merge/push. crates.io auth via libsecret — `cargo publish` with no `--token`.
- **Additive-only within `0.20.x`** — no existing `wlr` item's name/signature changes. The new handler-trait methods are DEFAULTED so existing implementors compile unchanged.
- **The `contract` attention bit is additive** (a new `bool`/`Option<bool>` field). The epic (#18, coordination rule 2) prefers contract edits to follow #14's wire-break; #14 is not done, so this Batch adds its field additively and the locked-wire test is regenerated. If #14 lands first, rebase onto it.
- **Two repos, two branches:** wlr work on `feature/wlr-a2.2` off `develop` (wloots-sys); icedtea work on `feature/a2-batch2` off `develop`. During compositor dev a `[patch.crates-io] wlr = { path = … }` may point at the local wlr worktree; it MUST be dropped and the pin bumped to the published crate before the compositor branch merges (as Batch 1 / XWayland M5 did).
- **Commit trailer:** `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`.
- **wlr gates (freeze commit):** `cargo test -p wlr` + `cargo clippy --all-targets -- -D warnings` + `RUSTDOCFLAGS="-D warnings" cargo doc -p wlr --no-deps` + `cargo fmt --all --check` + coverage audit.
- **icedtea gates:** `cargo test --workspace` + `cargo clippy --workspace --all-targets`. (NOTE: if the sandbox `getrandom` flake recurs — large test binaries panicking at libtest startup with `UnexpectedEof` — it is environmental, not code; retry / run in a healthy env. See the A2 memory.)

## Spike results (M-A2.0 continuation — recorded)

- All three C globals (`wlr_cursor_shape_manager_v1_create`, `wlr_xdg_activation_v1_create`, `wlr_gamma_control_manager_v1_create`) are already bound in `wlr-sys` (confirmed in Batch 1). So are the helpers this plan uses (`wlr_cursor_shape_v1_name`, `wlr_cursor_set_xcursor`, `wlr_output_get_gamma_size`, gamma-LUT/state setters, `wlr_gamma_control_v1_apply`/`_send_failed`). **No `wlr-sys` change.**
- **cursor-shape reuses existing infra:** `RuntimeInner` already holds `cursor: RefCell<Option<NonNull<wlr_cursor>>>` and `xcursor: RefCell<Option<NonNull<wlr_xcursor_manager>>>` (runtime.rs ~453-457), created in `create_seat`. The handler sets a named xcursor on that cursor.
- **The compositor owns NO cursor-image path today** (only `cursor_position` reads) — cursor-shape is the compositor's first cursor-image writer, done entirely through the new `wlr` helper.
- **Output commit** uses a fresh `wlr_output_state` per `Output::commit` (output.rs ~144); gamma applies through that state.
- **Attention bit slots into `contract::WindowInfo`** (after `focused`) and `contract::WindowUpdate` (as `Option<bool>`), types.rs:24/52.
- **Focus path:** `WindowManager::focus` emits `WindowUpdated`; xdg-activation's "steal focus" branch calls it, the "don't steal" branch emits an `attention` update.

## File Structure

**wloots-sys (`feature/wlr-a2.2`):**
- `crates/wlr/src/runtime.rs` — three `create_*` + listener installs + helpers (`set_cursor_shape`/named-xcursor, gamma size/apply, activation-token accessors); `RuntimeInner` cells.
- `crates/wlr/src/handler.rs` — three new DEFAULTED handler-trait methods: `CursorShapeHandler`/`request_set_shape`, `ActivationHandler`/`request_activate`, `GammaHandler`/`set_gamma` (place on an existing handler trait if that matches the crate's convention, else new defaulted supertrait — mirror how M4.4 added `session_lock_changed` additively).
- `crates/wlr/src/backend.rs` — the `extern "C"` listener callbacks (mirror the pointer-constraints/screencopy callbacks).
- `crates/wlr/coverage/{waived,wrapped}.toml`, `README.md`, `Cargo.toml`.

**icedtea (`feature/a2-batch2`):**
- `contract/src/types.rs` — `attention` field on `WindowInfo` + `WindowUpdate`; regen the locked-wire test.
- `compositor/src/lib.rs` — three boot globals (non-fatal, `tracing::error!` house style).
- `compositor/src/state.rs` — the three handler impls.
- `compositor/Cargo.toml`, `harness/Cargo.toml` — wlr pin.
- `harness/src/lib.rs` + `compositor/tests/compat_protocols.rs` (extend) — client bindings + tests.

---

### Task 1: wlr — cursor-shape (create + request_set_shape + set-named-cursor helper)

**Files:** `crates/wlr/src/runtime.rs`, `handler.rs`, `backend.rs`, `RuntimeInner` cell. Test: `crates/wlr/tests/seat.rs`.

**Interfaces:**
- Produces: `Runtime::create_cursor_shape_manager(&Display) -> Result<()>`; a DEFAULTED handler method `request_set_shape(&mut self, device_kind, serial: u32, shape: CursorShape)` (new `CursorShape` enum mirroring `wp_cursor_shape_device_v1.shape`); `Runtime::set_cursor_shape(shape: CursorShape)` or `set_named_cursor(&str)` that calls `wlr_cursor_set_xcursor(cursor, xcursor_mgr, wlr_cursor_shape_v1_name(shape))` on the existing seat cursor.

- [ ] **Step 1:** Add `cursor_shape_manager` cell to `RuntimeInner`. Write `create_cursor_shape_manager` (mirror `create_relative_pointer_manager`: double-create guard, `wlr_cursor_shape_manager_v1_create(display, version)` — use max supported version — `NonNull`, cell), then install a `request_set_shape` signal listener (mirror an existing signal-installing `create_*`, e.g. pointer-constraints) that fans out to the handler method. Reconcile the exact device-type the event carries (pointer vs tablet-tool) against the wlroots header.
- [ ] **Step 2:** Write the `set_named_cursor`/`set_cursor_shape` helper reaching `self.inner.cursor` + `self.inner.xcursor` (both already exist). `wlr_cursor_shape_v1_name(shape)` gives the xcursor name; `wlr_cursor_set_xcursor(cursor, mgr, name)` sets it.
- [ ] **Step 3:** Failing test: double-create returns `Err(Error::Operation)`. Then PASS. `cargo test -p wlr --test seat cursor_shape`.
- [ ] **Step 4:** Gates (`cargo test -p wlr`, clippy). Commit `feat(wlr): A2 batch-2 cursor-shape manager + request_set_shape`.

### Task 2: wlr — xdg-activation (create + request_activate with token seat/serial)

**Files:** as Task 1.

**Interfaces:**
- Produces: `Runtime::create_xdg_activation_manager(&Display) -> Result<()>`; DEFAULTED `request_activate(&mut self, surface_of_target, token: ActivationToken)` where `ActivationToken` exposes the recorded seat + serial + requesting surface (wlroots validates issuance internally; the compositor receives only validated requests and applies its own focus-steal policy).

- [ ] **Step 1:** Add cell + `create_xdg_activation_manager` (`wlr_xdg_activation_v1_create(display)`), install the `request_activate` listener → handler. Expose from the event: the target surface (map to the compositor's window id the same way other surface-keyed events do) and the token's `seat`/`serial`/`surface` fields (read from `wlr_xdg_activation_token_v1`).
- [ ] **Step 2:** Failing double-create test → PASS. `cargo test -p wlr --test seat xdg_activation`.
- [ ] **Step 3:** Gates. Commit `feat(wlr): A2 batch-2 xdg-activation manager + request_activate`.

### Task 3: wlr — gamma-control (create + set_gamma + Output gamma apply/size/failed)

**Files:** as Task 1, plus `crates/wlr/src/output.rs`.

**Interfaces:**
- Produces: `Runtime::create_gamma_control_manager(&Display) -> Result<()>`; DEFAULTED `set_gamma(&mut self, output_id: OutputId, ramp: GammaRamp)` (or the wlroots-recommended shape: the handler stashes the pending control and the crate applies it on next commit); `Output::gamma_size(&self) -> usize` (`wlr_output_get_gamma_size`); `Output::set_gamma(size, r, g, b) -> Result<()>` applying `wlr_output_state_set_gamma_lut` in the commit; a way to signal `wlr_gamma_control_v1_send_failed` on a rejected commit; clear-on-controller-destroy (wlroots forwards the destroy).

- [ ] **Step 1:** Add cell + `create_gamma_control_manager` (`wlr_gamma_control_manager_v1_create(display)`), install `set_gamma` listener → handler carrying the target output id + the ramp (or the `wlr_gamma_control_v1` to `apply` into a pending state). Add `Output::gamma_size` and the gamma-LUT apply into `Output::commit`'s `wlr_output_state` path (output.rs ~144). Expose the failed-signal path.
- [ ] **Step 2:** Failing double-create test → PASS; a `gamma_size` smoke test on a headless output if feasible. `cargo test -p wlr --test seat gamma`.
- [ ] **Step 3:** Gates. Commit `feat(wlr): A2 batch-2 gamma-control manager + output gamma apply`.

### Task 4: wlr — coverage ledger, README, version bump, full gates

- [ ] **Step 1:** `cargo test -p wlr --test coverage_audit` → move each newly-used batch-2 symbol from `waived.toml` to `wrapped.toml` (`module`/`item` naming as Batch 1). Re-run → PASS.
- [ ] **Step 2:** README `## 0.20.25 — A2 batch-2 request handlers` section (cursor-shape, xdg-activation, gamma-control; "all additive; new handler methods defaulted"). Bump `crates/wlr/Cargo.toml` to the confirmed next `0.20.x`.
- [ ] **Step 3:** Full wlr gate suite (test + clippy + doc + fmt). Commit `release(wlr): <version> — A2 batch-2 request handlers`.

### Task 5: Publish wlr + merge to develop — HARD-STOP

- [ ] Confirm version free + coordinate. `cargo publish -p wlr --dry-run`. **STOP — present for owner consent.** On go: `cargo publish -p wlr`; ff `feature/wlr-a2.2` → `develop`; push.

### Task 6: contract — additive `attention` bit

**Files:** `contract/src/types.rs` (+ the locked-wire test, wherever it lives — search `wire` / `signature` in `contract/`).

- [ ] **Step 1:** Add `pub attention: bool` to `WindowInfo` (after `focused`) and `pub attention: Option<bool>` to `WindowUpdate`. Update every `WindowInfo`/`WindowUpdate` constructor in the workspace (compositor mainly) to set it (default `false`/`None`).
- [ ] **Step 2:** Regenerate the locked-wire-signature test's expected value for the new field. Run the contract tests. Commit `feat(contract): additive attention bit on WindowInfo/WindowUpdate`.

### Task 7: compositor — consume wlr + boot globals + cursor-shape handler

**Files:** `compositor/Cargo.toml`, `harness/Cargo.toml`, `compositor/src/lib.rs`, `compositor/src/state.rs`.

- [ ] **Step 1:** Bump wlr pin → published version (both compositor occurrences + harness); drop any dev `[patch]`; build.
- [ ] **Step 2:** Add three non-fatal boot globals in `lib.rs` (house `tracing::error!` style), beside the Batch-1 ones.
- [ ] **Step 3:** Implement `request_set_shape`: guard on the requesting device being the focused pointer/seat (a background client must not change the cursor), then call the wlr `set_named_cursor` helper for the shape; revert to the default cursor on focus-leave. Build + existing suite + clippy. Commit `feat(compositor): cursor-shape handler sets the named seat cursor`.

### Task 8: compositor — xdg-activation focus-steal policy + attention bit

**Files:** `compositor/src/state.rs`.

- [ ] **Step 1:** Implement `request_activate`: if the token carries a seat + serial from a still-recent user interaction on a currently-focused client → raise + focus the target via `WindowManager::focus` (emits `WindowUpdated`). Otherwise set the target window's `attention = true` and emit a `WindowUpdate { attention: Some(true) }`. Clear `attention` when the window next gains focus. Reuse the existing focus/raise path — no new focus machinery.
- [ ] **Step 2:** Existing suite + clippy. Commit `feat(compositor): xdg-activation focus-steal policy + attention flag`.

### Task 9: compositor — gamma-control handler

**Files:** `compositor/src/lib.rs`, `compositor/src/state.rs`.

> **Ruling (Task 1-3 review, wlr 0.20.25 as published):** wlroots 0.20 has NO `wlr_output_state_set_gamma_lut`; `Runtime::create_gamma_control_manager` wires `wlr_scene_set_gamma_control_manager_v1(scene, mgr)`, and the scene applies ramps (fitted to `wlr_output_get_gamma_size`) on its own output commits and sends `failed` on a rejected commit. There is no `Output::set_gamma` and `OutputHandler::gamma_control_changed(OutputId)` is NOTIFICATION-ONLY. The manager requires `init_graphics` to have run (it has — `lib.rs` calls it before the globals block).

- [ ] **Step 1:** Create the manager at boot beside the Batch-1 globals (non-fatal, house `tracing::error!` style). Implement `gamma_control_changed` as a `tracing::debug!` trace of the output — no stash, no apply, no commit-path change (the scene owns all of that).
- [ ] **Step 2:** Existing suite + clippy. Commit `feat(compositor): gamma-control via scene integration` (may be folded into Task 7's boot-globals commit).

### Task 10: harness + tests

**Files:** `harness/src/lib.rs`, `compositor/tests/compat_protocols.rs`.

- [ ] **Step 1:** Advertised-globals assertions for `wp_cursor_shape_manager_v1`, `xdg_activation_v1`, `zwlr_gamma_control_manager_v1` (mirror Batch 1).
- [ ] **Step 2:** cursor-shape: a client with pointer focus calls `set_shape`; assert the seat's active cursor became the mapped named cursor and reverts when focus leaves (guards the background-client case). Add whatever harness accessor reads the current seat cursor name.
- [ ] **Step 3:** xdg-activation (both branches, non-vacuous): activation carrying a fresh interaction serial ⇒ target gains focus; a stale/token-less activation ⇒ target does NOT steal focus but its `attention` bit is set (assert via the `org.icedtea.Compositor` snapshot/WindowUpdate the test reads).
- [ ] **Step 4:** gamma-control (scene-applied, headless): the headless output reports `gamma_size == 0`, so a client's `get_gamma_control` on it must receive a protocol-conformant `failed` (wlroots sends it when the size is 0) — assert that event arrives (not a hang / not `gamma_size`). Reconcile against the actual wlroots behavior observed in the test; if headless instead advertises a size, set a ramp of that size and assert the control stays alive (no `failed`) through a subsequent frame.
- [ ] **Step 5:** `cargo test -p icedtea-compositor --test compat_protocols` + clippy. Commit `test(compositor): A2 batch-2 cursor-shape/xdg-activation/gamma`.

### Task 11: final review + merge window — HARD-STOP

- [ ] Full gates both repos on the freeze commits; confirm wlr already merged (Task 5). Whole-branch review on the most capable model (focus: the focus-steal policy's correctness, cursor revert-on-focus-leave, gamma commit-failure handling, the additive contract change). **STOP — present the `feature/a2-batch2 → develop` merge window** for owner go.

## Self-Review

- **Spec coverage:** cursor-shape ✓(T1/T7/T10), xdg-activation ✓(T2/T6/T8/T10), gamma-control ✓(T3/T9/T10). Decision 4 (reuse xcursor) ✓T1; Decision 5 (focus-steal policy + attention) ✓T8; Decision 7 (gamma = night-light backend only, no scheduler UI) ✓ (out of scope). Verification #4 (cursor path) resolved by the spike (wlr owns the cursor).
- **Type consistency:** each `create_*` returns `Result<()>` mirroring Batch 1; handler methods are defaulted (additive). The `attention` field name is identical across `WindowInfo`/`WindowUpdate`/compositor emit sites.
- **Reconciliation points flagged for implementers** (as Batch 1 did): exact `CursorShape` enum + device type from the cursor-shape header; the `ActivationToken` field accessors on `wlr_xdg_activation_token_v1`; the gamma listener/apply shape (handler-stash-then-commit vs direct apply) — pick whichever matches the installed wlroots 0.20 headers.
- **Placeholders:** none — every task names the C symbols, the handler shapes, and the compositor logic; exact signatures are reconciled against real source at implementation, per the established Batch-1 workflow.
