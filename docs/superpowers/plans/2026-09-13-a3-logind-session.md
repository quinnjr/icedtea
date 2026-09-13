# A3 logind session/seat integration — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `icedtea-session`, a standalone daemon that registers with `systemd-logind`, exposes `org.icedtea.Session`, locks before sleep (bounded), locks on idle, blocks logind's power keys, and promotes the compositor's lock state to a production D-Bus surface.

**Architecture:** One new workspace crate `session` (binary `icedtea-session`), shaped exactly like `clipboard`: a `#[interface]` D-Bus service, a `wayland-client` protocol client (`ext_idle_notifier_v1`), and `systemd --user` unit. Policy is pure (`session/src/policy.rs`); logind and the compositor are reached through trait seams so every decision is unit-testable without real D-Bus. The only compositor change is additive: `IsLocked()` on `org.icedtea.Compositor` and a `SessionLockChanged` signal.

**Tech Stack:** Rust, `zbus` 5 (blocking + `zbus::block_on` async signal streams, no tokio), `wayland-client` + `wayland-protocols` (`staging`, for `ext-idle-notify-v1`) + `wayland-protocols-wlr`, `icedtea-contract`, `icedtea-config`, `crossbeam-channel`, `tracing`.

**Spec:** `../specs/2026-08-20-icedtea-logind-session-design.md`

## Global Constraints

- Worktree `.worktrees/a3-logind`, branch `feature/a3-logind-session` (created off `develop` tip `af69365`).
- Naming (spec predates the rename): the compositor's bus is **`org.icedtea.Compositor`** (`contract::COMPOSITOR_BUS_NAME`, `COMPOSITOR_PATH`), interface type `CompositorInterface` — not `org.icedtea.WM`/`WmInterface`. Line numbers in the spec are as-of Aug 20 and **must be re-located by role**, not trusted.
- Every fallible D-Bus/process step that runs on a foreign thread records state and is asserted after the run — never `unwrap`/`expect` on a zbus dispatch or Wayland callback thread (repo C-frame rule).
- Locking is refused without a configured `locker_command` (spec Decision 2): idle→lock and lock-before-sleep **no-op with `tracing::warn!`**, never a screen nothing can unlock.
- The sleep delay inhibitor fd is released on **every** path (Drop guard), bounded at 3s — never block suspend past the budget (spec Decision 5; logind's `InhibitDelayMaxSec` default is 5s).
- Gates per task: `cargo test -p <crate>` (+ `--workspace` where the change is cross-crate), `cargo clippy --all-targets -- -D warnings`, `cargo fmt --all --check`.
- TDD, bite-sized commits, scoped review per task; whole-branch review before merge.

---

## File structure

- `config/src/lib.rs` + `config/src/defaults.rs` — `Power` struct and its default.
- `contract/src/event.rs` + `contract/src/lib.rs` — `Event::SessionLockChanged(bool)`.
- `compositor/src/dbus.rs` — `IsLocked()` on `CompositorInterface`; `event_signal_name` arm; emitter arm.
- `compositor/src/state.rs` — one `self.emit(Event::SessionLockChanged(locked))` line in `session_lock_changed`.
- `session/Cargo.toml` — new workspace member.
- `session/src/lib.rs`, `main.rs`, `logind.rs`, `idle.rs`, `policy.rs`, `service.rs`, `wm_client.rs` — the daemon.
- `session/systemd/icedtea-session.service`.
- `session/tests/` — policy table tests; fake-logind integration tests.

---

### Task 1: `config::Power` schema + defaults

**Files:**
- Modify: `config/src/lib.rs` (`Config` struct + `Power`), `config/src/defaults.rs`
- Test: `config` crate tests

**Interfaces:**
- Produces: `config::Power { locker_command: Option<String>, lock_idle_timeout_ms: Option<u64>, lock_before_sleep: bool }`; `Config.power: Power`.

- [ ] **Step 1: Failing test** — `Config::default().power == Power { locker_command: None, lock_idle_timeout_ms: None, lock_before_sleep: true }`; serde round-trip of a `Power` with all fields set; a legacy JSON `Config` without `power` still decodes.
- [ ] **Step 2: Run** `cargo test -p icedtea-config` — FAIL (no field).
- [ ] **Step 3: Implement** `Power` beside `Behavior` (same derive list), `#[serde(default)]` on the `Config.power` field, defaults in `defaults.rs`.
- [ ] **Step 4: Run** — PASS.
- [ ] **Step 5: Commit** `feat(config): Power section for session policy`

### Task 2: `org.icedtea.Compositor` — `IsLocked()` + `SessionLockChanged`

**Files:**
- Modify: `contract/src/event.rs` (variant), `contract/src/lib.rs` (if `Event` needs a helper), `compositor/src/dbus.rs` (interface method + signal name + emitter arm), `compositor/src/state.rs` (emit one line)
- Test: `compositor/tests/client_protocol.rs` (extend the existing M4.4 lock round-trip)

**Interfaces:**
- Consumes: `DbCommand::SessionLocked { reply: Sender<bool> }` (already exists) and `State.session_lock_changed(locked)` (already exists).
- Produces: `CompositorInterface::is_locked() -> bool` (wire member `IsLocked`); `contract::Event::SessionLockChanged(bool)`; wire signal `SessionLockChanged`.

- [ ] **Step 1: Failing test** — extend the lock round-trip: call `IsLocked()` (false before, true after take-lock, false after unlock) and assert a `SessionLockChanged` signal with matching values.
- [ ] **Step 2: Run** — FAIL.
- [ ] **Step 3: Implement** the `Event` variant, `event_signal_name` arm, emitter arm (`(seq, locked)` shape, `WorkspaceSet`-style two/three-field precedent), the interface method delegating to the existing `SessionLocked` round-trip, and the `self.emit(...)` line in `session_lock_changed`.
- [ ] **Step 4: Run** — PASS.
- [ ] **Step 5: Commit** `feat(compositor): production IsLocked + SessionLockChanged`

### Task 3: `session` crate skeleton + logind discovery + pure policy

**Files:**
- Create: `session/Cargo.toml`, `session/src/lib.rs`, `session/src/logind.rs`, `session/src/policy.rs`, `session/src/main.rs`
- Modify: root `Cargo.toml` (workspace members)
- Test: `session/src/policy.rs` unit tests, `session/src/logind.rs` unit tests

**Interfaces:**
- Produces:
  - `session::policy::{LockReason, Decision}` and `pub fn decide(reason: LockReason, locked: bool, locker_configured: bool, lock_before_sleep: bool) -> Decision` (`Decision::SpawnLocker` | `Decision::NoOp(&'static str)`), pure, exhaustively unit-tested.
  - `session::logind::Logind` trait: `session_path() -> zbus::zvariant::OwnedObjectPath`, `inhibit(what: &str, why: &str) -> io::Result<OwnedFd>`, `lock()`, `unlock()`, `suspend()`, `hibernate()`, `power_off()`, `reboot()`. `ZbusLogind` impl over `zbus::blocking::Connection::system()` + proxies; `RecordingLogind` test double.
  - `session::logind::resolve_session(conn, xdg_session_id: Option<&str>, pid: u32) -> Result<OwnedObjectPath>` (`$XDG_SESSION_ID` → `GetSession`, else `GetSessionByPID`).
- Consumes: `config::Power`.

- [ ] **Step 1: Failing tests** — policy table: `decide(LockReason::Idle, false, true, _) == SpawnLocker`; `decide(_, false, false, _) == NoOp("no locker configured")`; `decide(LockReason::PrepareForSleep, true, true, true) == NoOp("already locked")`; `decide(PrepareForSleep, false, true, false) == NoOp("lock-before-sleep disabled")`; and the idle-vs-sleep interleave pair both `SpawnLocker`-then-`NoOp`. `resolve_session` picks `GetSession` when the id is present and falls back to `GetSessionByPID` when `None` (against `RecordingLogind`).
- [ ] **Step 2: Run** `cargo test -p icedtea-session` — FAIL (crate absent).
- [ ] **Step 3: Implement** the crate, the trait + discovery, and the pure policy.
- [ ] **Step 4: Run** — PASS.
- [ ] **Step 5: Commit** `feat(session): crate skeleton, logind discovery, pure lock policy`

### Task 4: `org.icedtea.Session` service + CLI + wm client

**Files:**
- Create: `session/src/service.rs`, `session/src/wm_client.rs`
- Modify: `session/src/main.rs` (CLI arg dispatch + service boot)
- Test: `session/src/service.rs` unit tests; `session/tests/cli.rs`

**Interfaces:**
- Consumes: `Logind` trait; `contract::{COMPOSITOR_BUS_NAME, COMPOSITOR_PATH}`.
- Produces:
  - `SessionInterface` on `org.icedtea.Session` at `/org/icedtea/Session`: `lock()`, `suspend()`, `hibernate()`, `power_off()`, `reboot()`, `log_out()`, `is_locked() -> bool`.
  - `session::wm_client::WmClient` trait: `is_locked() -> bool`, `quit() -> bool`; `ZbusWmClient` over `zbus::blocking::Connection::session()`; `RecordingWm` double.
  - CLI: `icedtea-session lock|suspend|hibernate|poweroff|reboot|logout` calls the matching `org.icedtea.Session` method over the session bus and exits.

- [ ] **Step 1: Failing tests** — `SessionInterface::log_out()` forwards exactly once to `RecordingWm::quit`; `suspend()`/`power_off()`/`reboot()` call the `Logind` double once each with `interactive=false`; `is_locked()` mirrors `WmClient::is_locked()`; CLI `poweroff` with no daemon on the bus exits non-zero and does not panic.
- [ ] **Step 2: Run** — FAIL.
- [ ] **Step 3: Implement** the service (mirror `clipboard/src/service.rs::spawn`), the wm client, and `main.rs`'s arg dispatch.
- [ ] **Step 4: Run** — PASS.
- [ ] **Step 5: Commit** `feat(session): org.icedtea.Session service + CLI + compositor client`

### Task 5: Sleep inhibitor, bounded lock-before-sleep, power-key block, locker funnel

**Files:**
- Modify: `session/src/main.rs` (boot the inhibitor + signal loop), `session/src/logind.rs` (PrepareForSleep/Lock/Unlock subscription), `session/src/policy.rs` (extend if needed)
- Test: `session/tests/lock_flow.rs`

**Interfaces:**
- Produces: `session::logind::SleepInhibitor` (Drop guard that closes the fd; `#[must_use]`); `PrepareForSleep(bool)` handling — on `true`, decide via `policy::decide`, spawn the locker via `sh -c <locker_command>` (same invocation as the compositor's spawn arm), poll `WmClient::is_locked()` every 100ms up to 3s, then release; on `false` re-acquire. `Manager.Inhibit("handle-power-key:handle-suspend-key:handle-hibernate-key", "icedtea-session", …, "block")` held for daemon lifetime. `Lock`/`Unlock` signal funnel: spawn/wait locker, kill on external unlock, `Session.Unlock()` when the locker exits.

- [ ] **Step 1: Failing tests** (against `RecordingLogind` + `RecordingWm` + a stub locker script that touches a marker file): `PrepareForSleep(true)` with a configured locker spawns exactly one locker and releases the inhibitor *after* the marker appears; a stub locker that never marks locked still releases within the budget (no hang); `locker_command: None` never spawns and never delays; the `Lock`/`Unlock` funnel spawns once and does not double-spawn when idle and sleep race.
- [ ] **Step 2: Run** — FAIL.
- [ ] **Step 3: Implement** the inhibitor guard, the bounded flow, the block inhibitor, and the funnel.
- [ ] **Step 4: Run** — PASS.
- [ ] **Step 5: Commit** `feat(session): sleep inhibitor, bounded lock-before-sleep, power-key block`

### Task 6: Idle→lock via `ext_idle_notifier_v1`

**Files:**
- Create: `session/src/idle.rs`
- Modify: `session/src/main.rs` (spawn the idle thread)
- Test: `session/tests/idle_lock.rs` (harness-driven)

**Interfaces:**
- Consumes: `config::Power::lock_idle_timeout_ms`, `Logind`/`WmClient` seams, the harness's `IdleNotifyClient`/`SessionLockClient` patterns.
- Produces: an `ext_idle_notifier_v1` client (mirror `clipboard/src/manager.rs`'s wayland-client setup) requesting one timer at `lock_idle_timeout_ms`; `idled` → `policy::decide(Idle, …)` → `Logind::lock()`; `resumed` → no action.

- [ ] **Step 1: Failing test** — with a short configured timeout and a stub locker, no injected activity makes the mock's `Lock()` fire exactly once and the marker file appears; with `lock_idle_timeout_ms: None` nothing fires.
- [ ] **Step 2: Run** — FAIL.
- [ ] **Step 3: Implement** `idle.rs` and wire it in `main.rs`.
- [ ] **Step 4: Run** — PASS.
- [ ] **Step 5: Commit** `feat(session): idle-to-lock via ext-idle-notify`

### Task 7: systemd unit, default power-key bindings, docs, full gates

**Files:**
- Create: `session/systemd/icedtea-session.service`
- Modify: `config/src/defaults.rs` (power-key bindings), `docs/superpowers/specs/2026-08-20-icedtea-de-roadmap.md` (audit row)
- Test: full workspace

**Interfaces:**
- Produces: `icedtea-session.service` (mirror `clipboard/systemd/icedtea-clipboard.service`); default bindings `XF86PowerOff`/`XF86Sleep`/`XF86Hibernate` → `spawn:icedtea-session lock` (and suspend/poweroff per the spec's Milestone 3 note).

- [ ] **Step 1: Failing test** — `default_config()` contains the three power-key bindings with the expected commands.
- [ ] **Step 2: Run** — FAIL.
- [ ] **Step 3: Implement** bindings + unit + roadmap row.
- [ ] **Step 4: Run** full gates — `cargo test --workspace -- --test-threads=1`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --all --check`.
- [ ] **Step 5: Commit** `feat(session): systemd unit, default power-key bindings, docs`

## Self-Review

- Spec §Decisions 1–8 → Tasks 3 (crate/discovery), 4 (service/CLI/Logout), 5 (inhibitors/funnel/bounded), 6 (idle), 1 (config), 2 (compositor surface), 7 (unit/bindings). Spec §Testing items 1–5 → Tasks 5, 6, 4/2; success criteria 1–7 each map to a task test.
- No placeholders: every step names files, exact type/method names, and expected output.
- Type consistency: `Power{locker_command,lock_idle_timeout_ms,lock_before_sleep}`, `Logind`/`WmClient` seams, `Event::SessionLockChanged(bool)`, `IsLocked` wire member, `org.icedtea.Session` methods threaded unchanged.
