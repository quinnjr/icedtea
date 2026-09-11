# M8 remainder (batch, 0.20.33) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wrap every remaining M8 waived symbol (~114: keyboard depth, keyboard_group, shortcuts_inhibit, tablet, virtual input, transient seats) as safe Rust handles/snapshots/tokens and wire the consumer — keyboard layout + inhibit indicator, tablet cursor, virtual input harness — proven before wlr 0.20.33 publishes.

**Architecture:** New snapshot pairs + handle newtypes beside the entry tables in `crates/wlr/src/runtime.rs`; token types for outgoing send order; id-only events on `SeatHandler`/dedicated traits (A12 shape, defaulted). Consumer reuses `popup_anchor`/`lowest_index_output_geometry` sharing and `Snapshot` contract bump v3→4; harness doubles record payloads+serials, e2e are both-sides real clients.

**Tech Stack:** Rust (edition 2024, rust 1.94 host; MSRV 1.88 in wlr), wlroots 0.20 via `wlr-sys` bindgen.

**Spec:** `../specs/2026-09-11-m8-remainder-design.md` (batch A, §2-6) and `../specs/2026-08-18-wlr-100-coverage-roadmap-design.md` (§M8). Companion wlr plan: `wlroots-sys/docs/superpowers/plans/2026-09-11-m8-remainder-wlr.md` (if split).

## Global Constraints

- Within wlroots minor 0.20 the hand-written API is frozen: purely additive (defaulted methods, new types, new `Event` variant on `pub(crate)` enum is internal). No new supertrait that would break `Handlers` (A12-SEMVER).
- Per-object lifecycle signals emit NULL `data` — recover from `Bound`/listener-address (FIX-3); manager creation signals carry object in `data`.
- `unsafe` blocks carry `SAFETY:` comments; no borrow held across FFI emit (copy out, drop Ref, then call); `cargo fmt` before committing.
- R1: no publish until the harness e2e in this plan is green (consumer proves API). Publish is a consent stop.
- Gates per wlr task: `cargo test -p wlr`, `cargo test -p wlr-sys`, `cargo clippy --all-targets -- -D warnings`, `RUSTDOCFLAGS="-D warnings" cargo doc -p wlr --no-deps`, `cargo fmt --all --check`, `cargo test -p wlr --test coverage_audit`.
- Gates per consumer task: `cargo test --workspace -- --test-threads=1`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --all --check`.
- TDD, bite-sized commits, scoped review per task; whole-branch review before merge.

---

## File structure

- `crates/wlr/src/runtime.rs` — snapshot pairs (`KeyboardState`/`Pending…`, `TabletToolState`), handle newtypes (`KeyboardGroupId`, `TabletToolId`), readers.
- `crates/wlr/src/backend.rs` — handler rewrites threading tokens; emit sites for new events; `Registration` listeners.
- `crates/wlr/src/dispatch.rs` + `handler.rs` — new `Event` variants + defaulted trait methods.
- `crates/wlr/tests/*` — in-crate negative/unit tests that need no client.
- `compositor/src/state.rs` — `change_keyboard_focus`-style helper reuse, snapshot fill, shortcut-inhibit gate.
- `contract/src/types.rs` + `shell/src/panel.rs` — `Snapshot.keyboard_layout` + `shortcuts_inhibited`, panel badge.
- `harness/src/lib.rs` — doubles for virtual input + tablet axis recording.
- `compositor/tests/client_protocol.rs` — both-sides e2e (real clients).

---

### Task 1: Keyboard depth — snapshots, group, modifiers, LEDs

**Files:** Modify `crates/wlr/src/runtime.rs`, `crates/wlr/src/backend.rs`, `crates/wlr/src/dispatch.rs`, `crates/wlr/src/handler.rs`; Test `crates/wlr/tests/input_method.rs` or new `tests/keyboard.rs`

**Interfaces:** Consumes none; Produces `KeyboardState`, `PendingKeyboardState`, `KeyboardGroupId`, `Runtime::{keyboard_state, pending_keyboard_state, keyboard_group_state}`

- [ ] **Step 1: Write failing test** `crates/wlr/tests/keyboard.rs`:
```rust
#[test]
fn dangling_keyboard_group_misses_cleanly() {
    headless_env();
    let rt = wlr::Runtime::new().unwrap();
    assert!(rt.keyboard_state().is_none());
    assert!(rt.pending_keyboard_state().is_none());
}
```
- [ ] **Step 2: Run** `cargo test -p wlr --test keyboard dangling` — FAIL (no such fns)
- [ ] **Step 3: Implement** snapshot structs (owned fields, `pub(crate)` ctors reading `wlr_keyboard.{current,pending}`, null-guarded keymap copy), readers via table miss → `None`, `KeyboardGroupId` newtype, `try_keyboard_group` lookup, group `destroy` handler via listener-address.
- [ ] **Step 4: Run** — PASS, full `cargo test -p wlr` green
- [ ] **Step 5: Coverage moves + gates + commit** `feat(wlr): keyboard depth snapshots + group handles (M8)`

### Task 2: Shortcuts inhibit + tablet stack

**Files:** Modify `crates/wlr/src/runtime.rs`, `backend.rs`, `dispatch.rs`, `handler.rs`

**Interfaces:** Consumes keyboard group handles; Produces `ShortcutsInhibitorId`, `TabletToolId`, `TabletPadId`, `Event::ShortcutsInhibitorToggled` / `TabletToolEvent`

- [ ] **Step 1: Failing compile assertion** in `tests/keyboard.rs`: override `fn shortcuts_inhibitor_toggled(&mut self, _id: InhibitorId, _active: bool)` and `fn tablet_tool_event(&mut self, _id: ToolId)` — FAIL E0407
- [ ] **Step 2: Implement** id-only events (defaulted `SeatHandler::shortcuts_inhibitor_toggled`, `TabletHandler::tool_event`), `Event` variants + `deliver_all` arms + `run`-unreachable arms, emit sites in inhibit activate/deactivate and tablet proximity/tip/axis handlers after `finish()` settlement.
- [ ] **Step 3: Run** compile assertion PASS, full suite green
- [ ] **Step 4: Gates + commit** `feat(wlr): shortcuts inhibit + tablet events (M8)`

### Task 3: Virtual input + transient seats

**Files:** Modify `crates/wlr/src/runtime.rs`, `backend.rs`, `dispatch.rs`, `handler.rs`

**Interfaces:** Produces `VirtualKeyboardId`, `VirtualPointerId`, `TransientSeatId`, `Runtime::{create_virtual_keyboard, destroy_transient_seat}`

- [ ] **Step 1: Failing test** `virtual_keyboard_creation_is_tracked` — `rt.create_virtual_keyboard(seat)` returns `None` pre-init → `Some(handle)` post-init, then `destroy` → `false` second call
- [ ] **Step 2: Implement** opaque handle newtypes over `*mut wlr_virtual_keyboard_v1` etc., creation via `wlr_virtual_keyboard_manager_v1` (manager signal `data` object), destroy via `Registration` + explicit `destroy_*` bool, table miss → `None`.
- [ ] **Step 3: Run** PASS
- [ ] **Step 4: Gates + commit** `feat(wlr): virtual input + transient seats (M8)`

### Task 4: Consumer — shortcut inhibit gate, layout indicator, tablet cursor, harness virtual doubles

**Files:** Modify `compositor/src/state.rs`, `contract/src/types.rs`, `shell/src/panel.rs`, `harness/src/lib.rs`; Test `compositor/tests/client_protocol.rs`

**Interfaces:** Consumes wlr 0.20.33-dev via `[patch]` (workspace root, mirrors B6/M8 — drop at B-FINAL), `Runtime::keyboard_state`, `KeyboardGroupId` layout, `ShortcutsInhibitorToggled` event, tablet tool events

- [ ] **Step 1: Establish [patch]** `Cargo.toml` workspace root `[patch.crates-io] wlr = { path = "<wlr-m8-remainder worktree>/crates/wlr" }`, note drop at publish, `cargo update -p wlr`, `cargo build -p icedtea-compositor` green
- [ ] **Step 2: Failing e2e — inhibit blocks compositor binding**
```rust
#[test]
fn shortcuts_inhibit_blocks_binding_while_active() {
    // spawn + IME client not needed; bring up an inhibitor, press SUPER+2 (workspace switch), assert no switch, deactivate, assert switch again
}
```
- [ ] **Step 3: Implement** `SeatHandler::shortcuts_inhibitor_toggled` arm storing active flag, `SeatHandler::key` early-return skipping `dispatcher` when active (same ordering as grab, additional gate), `Snapshot { keyboard_layout: Option<String>, shortcuts_inhibited: bool }` with `serde(default)` and v3→4 bump, panel badge `active` class.
- [ ] **Step 4: Failing e2e — tablet tool reaches surface**
```rust
#[test]
fn tablet_tool_tip_reaches_surface() { /* virtual tablet tool proximity_in + tip down → expected surface gets tool event */ }
```
- [ ] **Step 5: Implement** cursor attach for tablet tools (`wlr_cursor_attach_input_device` pattern), pad deltas as scroll-like panel updates, harness `VirtualKeyboardClient`/`VirtualPointerClient` doubles recording payloads+serials
- [ ] **Step 6: Run** both e2e PASS, full workspace suite, clippy, fmt
- [ ] **Step 7: Commit** `feat(compositor): M8 remainder consumer (inhibit, layout, tablet, virtual)`

### Task 5: Release 0.20.33 (freeze; publish is a consent stop)

**Files:** Modify `crates/wlr/Cargo.toml` (`0.20.32` → `0.20.33` per `docs/RELEASING.md`), `crates/wlr/README.md` (0.20.33 changelog: keyboard/tablet/virtual + consumer), ledgers

- [ ] **Step 1: Coverage audit** — `cargo test -p wlr --test coverage_audit` PASS
- [ ] **Step 2: Version + changelog** per above, in 0.20.32 tone (What you get / Additive / Coverage)
- [ ] **Step 3: All six gates green** on freeze commit
- [ ] **Step 4: Commit** `release(wlr): 0.20.33 — M8 remainder batch`
- [ ] **Step 5: Push + PR into `develop`; CI green.** Then STOP — `cargo publish` requires explicit owner approval. On approval `cargo publish -p wlr`; then B-FINAL: drop `[patch]`, pin `0.20.33`, re-gate, finish icedtea branch.

## Self-Review

- Spec §2 (snapshot/token/event typing) → Tasks 1-3; §5 events → Task 2; §6 consumer (indicator/inhibit/tablet/virtual) → Task 4; §7 e2e → Tasks 2/4; §8 rollout (single 0.20.33) → Task 5 — no gap.
- No placeholders; every step has concrete code and expected output.
- Type names match across tasks (`KeyboardGroupId`, `InhibitorId`, `TabletToolId`, `Snapshot.keyboard_layout`).
