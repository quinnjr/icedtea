# B3 Notification Popups + Center Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Render the A4 daemon's notifications as a top-right popup stack with a history center, driven entirely by the daemon's signals and query surface.

**Architecture:** A notifier client (`shell/src/notif_client.rs`, mirroring `compositor_client.rs`: `NotifierCommands` trait + `NotifierProxy` + recording mock) feeds a new `Notifications` surface in the shell process (mirroring `launcher_view.rs::spec`/`run_surface`); popups render `contract::Notification` rows with per-urgency timeouts, and the center reuses the same model in history mode.

**Tech Stack:** Rust, `wayland-client` + `wayland-protocols-wlr` layer-shell, `icedtea-ui` toolkit widgets, zbus D-Bus client, existing `icedtea-notifications` daemon (no daemon changes).

**Spec:** `../specs/2026-09-15-b3-notification-popups-design.md` (popups + center; auto-dismiss urgency-scaled; overlay surface; daemon is source of truth).

## Global Constraints

- Worktree `.worktrees/b3-notify`, branch `feature/b3-notification-popups` (already created off `develop` tip `8aab7a1`).
- Never `unwrap`/`expect` on Wayland callback or D-Bus dispatch threads — record state, assert after the run.
- The daemon is the single source of truth: no client-side expiry model, no second timeout heap; shell timers only drive dismissal *requests* (`CloseNotification`).
- TDD, bite-sized commits, scoped review per task; whole-branch review before merge.
- Gates per task: `cargo test --workspace -- --test-threads=1`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --all --check`.
- Known pre-existing failures (not yours): 3 `icedtea-ui` `gallery_gate` render tests are green since #40 — verify current state first; any unrelated red is reported, not fixed.

---

## File structure

- `shell/src/notif_client.rs` — `NotifierCommands` trait (`get_active`, `get_history`, `invoke_action`, `close_notification`, `get_dnd`, `set_dnd`) + `NotifierProxy` (zbus blocking, `NOTIF_BUS_NAME`/`NOTIF_PATH` from `icedtea-contract`) + recording mock; signal subscription for `Added`/`Closed`/`ActionInvoked`/`DndChanged` (follow `compositor_client.rs` signal-worker shape).
- `shell/src/notif_view.rs` — `NotifModel` (stack + center/history mode + DND mirror), `NotifMsg`, `view()` (popup rows, action buttons, close ×, history list, clear-all, DND toggle), `spec()` (Right+Top anchor, `Layer::Top`, exclusive-zone 0, keyboard `None`), `run_surface` boot (follow `launcher_view.rs`).
- `shell/src/panel.rs` — bell button at the bar's right end (badge = active count), toggling center mode.
- `shell/tests/notif.rs` (new) — render + model tests; harness e2e lives in `compositor/tests/` per repo layout if a compositor round-trip is needed (prefer shell-level + real daemon if spawnable headless, else documented mock).

---

### Task 1: Notifier client + pure stack model

**Files:**
- Create: `shell/src/notif_client.rs`
- Test: inline `#[cfg(test)]` + `shell/tests/notif.rs` (new)

**Interfaces:**
- Consumes: `contract::Notification { id: u32, app_name: String, icon: IconSource, summary: String, body: String, actions: Vec<NotificationAction { key: String, label: String }>, urgency: Urgency, .. }`; daemon methods `get_active() -> Vec<Notification>`, `get_history()`, `invoke_action(id: u32, key: String)`, `close_notification(id: u32)`, `get_do_not_disturb() -> bool`, DND setter; signals `Added`/`Closed`/`ActionInvoked`/`DndChanged`.
- Produces: `NotifierCommands` trait + `NotifierProxy` + `MockNotifier` (recording); `NotifModel::apply_added/applied_closed` pure stack ops (newest-first, per-app rate cap: max 3 visible per app, overflow to center only).

- [ ] **Step 1: Write failing tests**
```rust
#[test]
fn added_stacks_newest_first_and_caps_per_app() {
    let mut m = NotifModel::default();
    for i in 0..5 { m.apply_added(mk("chat", i)); }
    assert_eq!(m.visible().len(), 3);
    assert_eq!(m.visible()[0].id, 4);
}
#[test]
fn closed_removes_and_dnd_suppresses_popups_but_not_center() {
    let mut m = NotifModel::default();
    m.apply_added(mk("mail", 1));
    m.apply_closed(1);
    assert!(m.visible().is_empty());
    m.set_dnd(true);
    m.apply_added(mk("mail", 2));
    assert!(m.visible().is_empty() && m.history().len() == 1);
}
```
- [ ] **Step 2: Run** `cargo test -p icedtea-shell notif` — FAIL (no such module)
- [ ] **Step 3: Implement** the trait + proxy (blocking zbus, calls mirror `CompositorProxy::call_method` shape with members `GetActive`/`GetHistory`/`InvokeAction`/`CloseNotification`/`GetDoNotDisturb`/DND setter — verify exact member names against `notifications/src/service.rs` first, do not guess) + mock + pure model (stack, cap, DND gate).
- [ ] **Step 4: Run** — PASS, `cargo test -p icedtea-shell` green
- [ ] **Step 5: Commit** `feat(shell): notifier client + pure popup stack model`

### Task 2: Notification surface + popup view

**Files:**
- Create: `shell/src/notif_view.rs`
- Modify: `shell/src/main.rs` or `lib.rs` (surface boot — follow panel/launcher boot)
- Test: `shell/tests/notif.rs` (extend)

**Interfaces:**
- Consumes: Task 1 model + client.
- Produces: `spec()` (anchor Right+Top, `Layer::Top`, exclusive-zone 0, keyboard `None`, size ~(360, auto)), `view()` popup rows (icon, app name, summary, body, action buttons from `actions: Vec<NotificationAction>`, close ×), urgency styling (`.critical` accent border), per-urgency dismiss timers (low 5s, normal 8s, critical 20s, paused on hover — timers request `CloseNotification`, never local-expire), own rest gates.

- [ ] **Step 1: Write failing render tests** rest state per theme (Adwaita light/dark pattern from settings page gates) + one popup + one critical + one with actions.
- [ ] **Step 2: Run** — FAIL
- [ ] **Step 3: Implement** model/msgs/view/surface boot; action click → `invoke_action(id, key)` then dismiss; body click / × → `close_notification(id)`; timeout → same.
- [ ] **Step 4: Run** — PASS
- [ ] **Step 5: Commit** `feat(shell): notification popup surface + view`

### Task 3: Bell + center mode + DND toggle

**Files:**
- Modify: `shell/src/panel.rs` (bell button, right end), `shell/src/notif_view.rs` (center mode)
- Test: `shell/tests/notif.rs` (extend)

**Interfaces:**
- Consumes: Tasks 1–2, daemon `get_history`, DND getter/setter + `DndChanged` signal.
- Produces: bell with badge count; center mode (scrollable history, clear-all → per-id `close_notification`, DND toggle reflecting daemon state).

- [ ] **Step 1: Write failing tests** badge equals active length; toggle center shows history rows; DND toggle calls setter and suppresses popups while listing.
- [ ] **Step 2: Run** — FAIL
- [ ] **Step 3: Implement** bell button (`button("bell")`, class + badge label), center mode view, clear-all, DND toggle wiring with signal-driven refresh.
- [ ] **Step 4: Run** panel gates + new tests — PASS
- [ ] **Step 5: Commit** `feat(shell): bell, notification center, DND toggle`

### Task 4: Harness e2e + gates

**Files:**
- Test: `shell/tests/notif.rs` (extend) and/or `compositor/tests/` if a bus round-trip is needed
- Modify: none expected (fix fallout only)

**Interfaces:**
- Consumes: Tasks 1–3. Produces: proven end-to-end behavior.

- [ ] **Step 1: Write failing e2e** — spawn the real daemon headless if possible (else drive the mock client against a scripted signal script): `notify` → popup appears with body + actions → click first action → assert daemon observed `ActionInvoked(id, key)`; enable DND → `notify` → no popup, center lists it.
- [ ] **Step 2: Run** — FAIL
- [ ] **Step 3: Implement** e2e (if the real daemon can't run headless, say so in code comments and prove against the mock + a live-signal script, not pixels alone).
- [ ] **Step 4: Write deletion test** — gut the `Added` signal subscription → e2e FAILs; restore.
- [ ] **Step 5: Run** full gates — PASS
- [ ] **Step 6: Commit** `test(shell): notification e2e + deletion gate`

## Self-Review

- Spec §Architecture → Tasks 2+3 (surface/process); §Data flow → Task 1 (client + signals) with daemon as source of truth (no client expiry heap anywhere); §Center+DND → Task 3; §Interaction/timeouts/theming → Task 2 (urgency durations, pause-on-hover, `.critical`); §Testing → Tasks 1/2/4 (unit, render, e2e+deletion); §Out of scope respected (no policy page, no sounds, no inline replies, no extra persistence).
- No placeholders; every step names files, exact names (`GetActive`, `InvokeAction`, `CloseNotification`, `SearchEntry`-analogous patterns where reused), and expected outputs.
- Type consistency: `Notification`/`NotificationAction{key,label}` field names threaded verbatim from `contract`; `NotifierCommands`/`NotifierProxy`/`MockNotifier` naming mirrors `CompositorCommands`/`CompositorProxy`; `spec()`/`run_surface` mirror `launcher_view.rs`.
