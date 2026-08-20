# icedtea-wm — A3: logind session/seat integration design

Date: 2026-08-20

## Goal

Give icedtea a real system-session layer: seat/session registration with
`systemd-logind` (`org.freedesktop.login1`), suspend/hibernate/shutdown
orchestration, lock-before-sleep, an idle→lock policy that rides the
compositor's existing `ext-idle-notify-v1`, and power-key/lid handling —
without which `loginctl suspend`, laptop lid-close, and the power key either
do nothing DE-aware or (worse) suspend the machine with the screen unlocked.
This is Track A item **A3** in
`docs/superpowers/specs/2026-08-20-icedtea-de-roadmap.md`. It produces a new
standalone daemon crate, a small compositor-side D-Bus addition, and a new
`config::Power` section — no `wlr`-crate or protocol changes are needed.

## Grounding: what already exists

- **`ext-session-lock-v1` is fully implemented and security-hardened** in the
  `wlr` crate and consumed by the compositor
  (`docs/superpowers/specs/2026-08-18-wlr-port-m4.4-session-lock-idle-design.md`,
  merged). `wlr::Runtime::is_session_locked()` exists; `state.rs`'s
  `SeatHandler::session_lock_changed(&mut self, locked: bool)`
  (`compositor/src/state.rs:4707`) already tracks the flag centrally. The
  compositor **never locks itself** — locking is always driven by an external
  client that binds `ext_session_lock_manager_v1`, takes the lock, paints a
  surface per output, and is trusted to have already authenticated the user
  before calling `unlock()`. Track B's **B2 lock-screen UI** is that client's
  eventual PAM-backed implementation; today nothing binds the protocol at all
  (`docs/superpowers/specs/2026-08-20-icedtea-de-roadmap.md`'s audit: "no
  password prompt").
- **`ext-idle-notify-v1` is implemented**: `wlr::Runtime::create_idle_notifier`
  is created at boot (`compositor/src/lib.rs:115`) and wired centrally from
  the crate's input path (M4.4 §"Idle-notify"). Any Wayland client can bind
  `ext_idle_notifier_v1` and request `ext_idle_notification_v1` timers —
  exactly the mechanism `swayidle` uses on sway, and what this design uses too.
- **`org.icedtea.WM`** (`compositor/src/dbus.rs`) is a session-bus service
  with a `DbCommand` enum forwarded from a `WmInterface` `#[interface]` impl,
  and a `SessionLocked { reply: Sender<bool> }` command already exists but is
  explicitly **test-only** — `compositor/src/dbus.rs:111`'s doc: "Not
  reachable from `WmInterface` — only the test harness sends this." This
  design promotes that read path to a real, production D-Bus surface (below).
- **`org.icedtea.Clipboard`** (`clipboard/` crate) is the precedent for "a
  standalone daemon crate, its own systemd user unit, its own D-Bus bus name,
  a Wayland client for one protocol, talks to `org.icedtea.WM` as a peer over
  D-Bus": `clipboard/src/service.rs` registers `org.icedtea.Clipboard` on the
  **session** bus via `zbus::blocking::Connection`; `clipboard/systemd/
  icedtea-clipboard.service` is `Type=simple`, `PartOf=graphical-session.target`,
  `WantedBy=graphical-session.target`. `shell/src/wm_client.rs` is the
  precedent for a **client** of `org.icedtea.WM`: an async worker thread
  (`zbus::block_on`, no tokio in the workspace) subscribes to signals via
  `zbus::MessageStream::for_match_rule`, plus a separate
  `zbus::blocking::Connection` for outgoing commands.
- **Keybindings already support arbitrary exec.** `config::defaults::default_config`
  populates a `HashMap<String, KeyCombo>` keyed by action strings like
  `"spawn:terminal"`; `compositor/src/state.rs:3081`'s `"spawn"` arm does
  `std::process::Command::new("sh").arg("-c").arg(&cmd).spawn()`. **No
  compositor code change is needed to bind a power key to a shell command** —
  a default binding of `"spawn:loginctl suspend"` on `XF86PowerOff` already
  works today, mechanically. What's missing is *policy*: a bare
  `loginctl suspend` bypasses lock-before-sleep entirely, which is the actual
  gap A3 closes.
- **No switch-device (lid) support in the `wlr` crate today.** `SW_LID` is a
  libinput *switch* device event (`WLR_INPUT_DEVICE_SWITCH`), not a keyboard
  keysym; `grep`ing `compositor/src/*.rs` and `config/src/*.rs` for
  `switch`/`lid` finds nothing wlroots-related. This design does **not** add
  switch-device support (see Out of scope) — it relies on `systemd-logind`'s
  own built-in lid handling instead (see Decision 3).
- **Everything D-Bus in this repo today is on the session bus.** `logind` is
  the first **system**-bus client icedtea ships; `zbus::Connection::system()`
  / `zbus::blocking::Connection::system()` is the only new API surface, not a
  new dependency (zbus is already a workspace dependency, `zbus.workspace =
  true` in `compositor`, `clipboard`, `shell`, `settings`).

## Decisions of record

1. **A new standalone crate, `session` (binary `icedtea-session`), not code
   inside the compositor.** Reasons, all grounded in existing precedent and
   the M4.4 ruling:
   - M4.4 already drew this exact line for locking: "the compositor only
     reacts to a `session_lock_changed(locked)` callback (suspend its own
     focus/layout logic)" — the compositor is deliberately hands-off about
     lock *policy*; it enforces the lock *mechanism*. Suspend/idle/lid policy
     is more of the same kind of policy, not mechanism, and belongs beside it,
     not inside `state.rs`.
   - **Trust boundary**: this is the first icedtea component that talks to
     the **system** bus (`org.freedesktop.login1` is system-bus-only) and
     that manages a `systemd-logind` inhibitor file descriptor whose lifetime
     bugs (a leaked fd silently blocks every future suspend on the machine)
     are exactly the kind of narrow, auditable surface that should not share
     a process/crash domain with the compositor's rendering loop.
   - **Precedent fit**: it is shaped exactly like `clipboard/` — a Wayland
     protocol client (here: `ext_idle_notifier_v1`) plus a D-Bus service,
     running as its own `systemd --user` unit alongside
     `icedtea-clipboard.service` and `icedtea-shell.service`.
   - A crash in `icedtea-session` (e.g. a logind D-Bus hiccup) must not take
     the compositor down; `Restart=always` on its own unit recovers it
     independently, same as the clipboard daemon today.
2. **Locking is delegated to a configured external locker; this daemon never
   implements a password prompt.** `config::Power::locker_command: Option<String>`
   (a shell command, spawned the same way `state.rs`'s `"spawn"` arm does)
   names the program to run to actually secure the screen — it is expected to
   bind `ext_session_lock_manager_v1` itself (any real locker: `swaylock`,
   `gtklock`, and eventually Track B's `icedtea-lockscreen` from B2). Building
   a built-in blank/no-auth locker into this daemon was considered and
   rejected: session-lock's `unlock()` is entirely up to the locker client —
   a locker with no authentication is either a no-op security theater (if
   anyone can dismiss it) or a way to brick the session (if nothing can). The
   roadmap itself scopes the password prompt to B2. Until `locker_command` is
   set, idle→lock and lock-before-sleep both **no-op with a loud `tracing::warn!`**
   instead of locking a screen nothing can unlock — matching how `swayidle`
   ships (its `lock` action is opt-in, pointed at a real locker by config).
3. **Lid-switch and power-key suspend/poweroff stay owned by `systemd-logind`'s
   own default handling; icedtea does not reimplement SW_LID.** `logind.conf`'s
   defaults (`HandleLidSwitch=suspend`, `HandlePowerKey=poweroff`, etc.) already
   act on lid/power-key events at the systemd level *before* any desktop
   session sees them, using logind's own evdev listener — independent of
   whatever wlroots/libinput also does with the same device fd. Icedtea does
   **not** need `wlr` switch-device support for lid-close-triggers-suspend to
   work: it already does, out of the box, via logind, with zero code here.
   What icedtea's own session daemon does need to own is the ONE hook that
   fires no matter what triggered the suspend (lid, power key, `loginctl
   suspend`, an idle timeout elsewhere): the `PrepareForSleep` signal (see
   Architecture § lock-before-sleep). For the **power key specifically**,
   icedtea overrides logind's default `poweroff` action (Decision 4) so a
   DE-aware action runs instead of an instant, unconfirmed shutdown.
4. **Power-key/suspend-key override via logind's `handle-*` block
   inhibitors, dispatched to an ordinary compositor keybinding.** At startup,
   `icedtea-session` takes `Manager.Inhibit("handle-power-key:handle-suspend-key:
   handle-hibernate-key", "icedtea-session", "desktop session handles these
   keys", "block")`. This is the same technique GNOME/KDE session daemons use:
   it tells logind not to act on those keys itself; the compositor still
   receives `KEY_POWER`/`KEY_SLEEP` as ordinary input events through libinput
   (logind doesn't grab the device exclusively — it's a passive listener too),
   which xkb resolves to `XF86PowerOff`/`XF86Sleep` keysyms, matched by the
   **existing** keybinding system with **zero compositor code changes**. Only
   `config::defaults::default_config` gains new default bindings (Milestone 3).
   `handle-lid-switch` is **not** in this inhibitor set — Decision 3 leaves lid
   handling to logind's own default.
5. **Lock-before-sleep via a delay-mode sleep inhibitor + `PrepareForSleep`,
   released as soon as the lock is confirmed (bounded).** `icedtea-session`
   holds `Manager.Inhibit("sleep", "icedtea-session", "lock screen before
   suspend", "delay")` continuously (one fd at a time; `systemd-logind`
   enforces `InhibitDelayMaxSec`, default 5s, as a hard ceiling on how long
   any delay inhibitor can hold up sleep — this design does not assume a
   larger value is configured). On `PrepareForSleep(true)`: if
   `config::Power::lock_before_sleep` is true and a `locker_command` is
   configured and the session is not already locked (queried via the new
   `IsLocked()` — Decision 6), spawn the locker, then poll `IsLocked()` at a
   short interval (matching the M4.4 harness's own polling style for
   protocol round-trips) until it reports `true` or a bounded deadline
   (comfortably under 5s, e.g. 3s) elapses; either way, close the inhibitor
   fd to let sleep proceed — **never block indefinitely**: a broken locker
   must not hang the whole machine's suspend. On `PrepareForSleep(false)`
   (resume): re-acquire a fresh delay inhibitor for the next sleep cycle
   (delay inhibitors are one-shot, consumed by closing the fd).
6. **`org.icedtea.WM` gains a real, non-test `IsLocked() -> bool` method and
   a `SessionLockChanged(seq: u64, locked: bool)` signal**, promoting the
   existing test-only `DbCommand::SessionLocked` plumbing
   (`compositor/src/dbus.rs:108-111`, `state.rs:2914`) to production API. This
   is additive to `WmInterface` (`compositor/src/dbus.rs:155`'s
   `#[interface(name = "org.icedtea.WM")]` block gains one more method) and to
   `contract::Event` (`contract/src/event.rs` gains one more variant,
   `SessionLockChanged(bool)`, fired from `state.rs`'s existing
   `session_lock_changed` handler at `state.rs:4707` via `self.emit(...)`,
   the same pattern every other event already uses). This is the one piece of
   this design that touches the compositor crate; everything else is new
   crate + new config fields. It is a minimal, backward-compatible addition
   (new method + new signal variant, no existing method/signal changes) in
   the same spirit as M4.4's own "additive-only" discipline for the `wlr`
   crate.
7. **`icedtea-session` idle→lock is its own `ext_idle_notifier_v1` client,
   independent of any other idle consumer.** It binds the global directly
   (any number of clients can each request their own `ext_idle_notification_v1`
   timer against the same notifier — this is not exclusive), requests one
   timer at `config::Power::lock_idle_timeout_ms` (default: unset/disabled;
   see Decision 2 for why locking defaults off without a configured locker).
   On `idled`: if not already locked and a `locker_command` is configured,
   spawn it (same path as lock-before-sleep). On `resumed`: no action —
   activity does not auto-unlock; only the locker process exiting (or an
   external `Session.Unlock()`) clears the locked state (Architecture below).
8. **`org.icedtea.Session` is the new D-Bus surface Track B's power UI plugs
   into.** A second interface, `org.icedtea.Session` at `/org/icedtea/Session`
   on the **session** bus (co-located with `org.icedtea.WM`/`org.icedtea.
   Clipboard` conventions), with methods `Lock()`, `Suspend()`, `Hibernate()`,
   `PowerOff()`, `Reboot()`, `LogOut()`, and `IsLocked() -> bool` (mirrors
   `org.icedtea.WM`'s, kept for symmetry/convenience — either works since both
   read the same underlying state). `LogOut()` forwards to `org.icedtea.WM`'s
   existing `Quit()` (this daemon becomes a `WmProxy`-style client of
   `org.icedtea.WM`, `shell/src/wm_client.rs`'s pattern). This is exactly the
   backend the roadmap's **B1** app-launcher "power/session control row
   (lock / log out / suspend / restart / shut down)" and a future power-menu
   quick-setting are specced to need — out of scope here (no UI), but this is
   the named plug-in point.

## Architecture

### Crate layout (`session/`, new workspace member)

```
session/
  Cargo.toml            # icedtea-session; deps: zbus, wayland-client,
                         # wayland-protocols-wlr (ext-idle-notify is a wlr-*
                         # staging protocol, same crate clipboard already
                         # depends on for wlr-data-control), icedtea-contract,
                         # icedtea-config, crossbeam-channel, tracing
  src/
    lib.rs               # wiring / re-exports for tests
    main.rs               # boot: open config, connect system+session buses,
                            # spawn idle client thread, spawn D-Bus service,
                            # spawn sleep-signal thread, block on shutdown
    logind.rs             # Manager/Session proxies (zbus::proxy), session
                            # discovery, Inhibit() wrappers, PrepareForSleep
                            # subscription
    idle.rs                # ext_idle_notifier_v1 Wayland client (mirrors
                            # clipboard/src/manager.rs's wayland-client setup)
    policy.rs              # pure decision logic: given (locked, has_locker,
                            # lock_before_sleep, lock_idle_timeout_ms) ->
                            # what to do; unit-tested without any D-Bus/Wayland
    service.rs             # org.icedtea.Session zbus service (mirrors
                            # clipboard/src/service.rs)
    wm_client.rs            # thin client of org.icedtea.WM: IsLocked() calls,
                            # SessionLockChanged subscription, Quit() forward
                            # for LogOut() (mirrors shell/src/wm_client.rs)
  systemd/
    icedtea-session.service
  tests/
    policy.rs               # pure policy-table tests (no D-Bus/Wayland needed)
```

### logind session/seat discovery

`org.freedesktop.login1.Manager` is a well-known system-bus singleton
(`org.freedesktop.login1`, `/org/freedesktop/login1`). At startup,
`logind.rs` resolves *this* login session two ways, tried in order:

1. `$XDG_SESSION_ID` (set by `pam_systemd` for every logind-managed login,
   which is how the compositor itself is normally launched — a `systemd
   --user` unit inherits it): `Manager.GetSession(session_id) -> ObjectPath`.
2. Fallback: `Manager.GetSessionByPID(getpid())` — covers a manually-started
   session where the env var didn't propagate (e.g. a debug run from a
   terminal that itself isn't in a logind session leader chain the daemon can
   see, or an unusual launch path); `getpid()` is `icedtea-session`'s own
   pid, and `GetSessionByPID` walks up the cgroup/session membership,
   matching how `loginctl` itself resolves "the current session" with no
   argument.

Both are wrapped in a single `Manager` `#[zbus::proxy]` trait (blocking, per
this workspace's no-tokio convention — `shell/src/wm_client.rs` already shows
both an async worker thread via `zbus::block_on` for a signal stream and a
plain `zbus::blocking::Connection` for calls; `icedtea-session` uses the same
split: an async task for `PrepareForSleep`/idle-notify events, blocking calls
for everything request/response). Failure to resolve a session is fatal at
startup (mirrors `clipboard/src/service.rs`'s "`Err` ... the binary treats
that as fatal" for its own bus-name acquisition) — a daemon that can't find
its own session has nothing useful to do.

The resolved `Session` object path backs a second proxy,
`org.freedesktop.login1.Session`, used for: `Lock()`/`Unlock()` methods (see
below), and its `Lock`/`Unlock` **signals** (fired by logind whenever
*anything* — `icedtea-session` itself, or an external `loginctl
lock-session`/`unlock-session` — calls those methods on this session object).
`icedtea-session` subscribes to both signals; this is the **single funnel**
all locking goes through, whether triggered by icedtea's own idle/sleep
policy or by an external `loginctl lock-session`:

- **`Lock` signal received** → if not already locked (`IsLocked()` against
  `org.icedtea.WM`) and a `locker_command` is configured, spawn it
  (`std::process::Command::new("sh").arg("-c").arg(cmd).spawn()`, same
  invocation shape as `state.rs`'s `"spawn"` keybinding arm, deliberately —
  one exec convention across the codebase) and start a background thread that
  `.wait()`s on the child.
- **Child process exits** → treat as "the locker's job is done" (it is
  trusted to have only exited after successfully authenticating an unlock,
  exactly as `swaylock`/`gtklock` behave — they exit on successful unlock,
  not before) → call `Session.Unlock()` so logind's own bookkeeping
  (`LockedHint` property, consumed by any future screensaver-aware app) stays
  correct.
- **`Unlock` signal received** (e.g. an admin/PAM agent calls
  `loginctl unlock-session` directly) → if the locker child is still running,
  kill it (`Command`'s `Child::kill()`) so the display isn't left in a locked
  visual state with a stale process; the actual Wayland-level unlock already
  happened the moment that locker calls `ext_session_lock_v1.unlock()` and
  exits — this branch only handles the unusual case of an *external* unlock
  request while a locker is still up.

`icedtea-session`'s own idle-timeout and pre-sleep policy (below) never spawn
a locker directly — they call `Session.Lock()`, which round-trips through
logind and comes back as the `Lock` signal on the same funnel above. One
control path, one place that decides "do we actually have a working locker to
run" (Decision 2's no-op-without-config rule lives here, once).

### Sleep inhibitor + lock-before-sleep

On startup and after every resume, `logind.rs` calls:

```
Manager.Inhibit("sleep", "icedtea-session", "lock screen before suspend", "delay")
  -> fd
```

held as a `zbus::zvariant::OwnedFd` field. `Manager` also emits
`PrepareForSleep(bool)` (a signal, not scoped to a session — every logind
client receives it) subscribed on the async task alongside `Session`'s
`Lock`/`Unlock`. On `PrepareForSleep(true)`:

1. If `config::Power::lock_before_sleep` and a `locker_command` is
   configured and the session isn't already locked: call `Session.Lock()`
   (funnels into the same path as idle-lock, above).
2. Poll `IsLocked()` every ~100ms (matching the M4.4 spec's own guidance on
   polling protocol state in tests: "generous margins ... inject activity
   rather than depend on wall-clock quiescence" — here there's no activity to
   inject, so a short poll loop is the direct equivalent) up to a 3s budget
   (comfortably inside logind's 5s default `InhibitDelayMaxSec`, leaving
   headroom for the D-Bus round-trip itself).
3. Either way — locked confirmed, budget exceeded, or locking was
   configured-off — **close the inhibitor fd** (`drop`), releasing sleep. A
   configured-off or timed-out lock logs `tracing::warn!` (visibly, so a user
   who expected the screen to lock on lid-close notices it didn't) but never
   blocks suspend past the budget.

On `PrepareForSleep(false)` (resume): re-acquire a fresh `Inhibit("sleep",
..., "delay")` fd for the next cycle — `Inhibit` fds are one-shot; the
previous one was already consumed by step 3 above.

### Power-key / suspend-key handling

At startup, `logind.rs` additionally calls:

```
Manager.Inhibit("handle-power-key:handle-suspend-key:handle-hibernate-key",
                 "icedtea-session", "desktop session handles these keys", "block")
```

held for the daemon's whole lifetime (released, and logind's defaults resume,
if `icedtea-session` exits — matching "no session daemon means no DE-aware
power handling" being the safe failure mode, same reasoning as Decision 2).
This is the *only* code for power-key handling — no keybinding dispatch, no
keysym matching, lives in this crate at all. The actual reaction to
`XF86PowerOff`/`XF86Sleep`/`XF86Hibernate` keysyms is an ordinary compositor
keybinding (Decision 4), added as new `config::defaults::default_config`
entries (Milestone 3):

```
insert(&mut keybindings, "spawn:icedtea-session lock", &[], "XF86_PowerOff");
```

(exact default command TBD-by-config, not TBD-in-code: `icedtea-session`
grows a tiny CLI subcommand mode — `icedtea-session lock`/`suspend`/
`hibernate`/`poweroff` — that just calls the corresponding `org.icedtea.
Session` D-Bus method and exits, so the default binding has something concrete
to `spawn:` without requiring `gdbus`/`busctl` on the user's `$PATH`; this is
a few lines in `main.rs`'s argument handling, mirroring how `loginctl` itself
is a thin CLI over the same bus).

Lid-switch is **not** bound to anything here (Decision 3) — logind's own
default `HandleLidSwitch=suspend` runs, which triggers `PrepareForSleep` like
any other suspend trigger, so lock-before-sleep applies uniformly without any
lid-specific code.

### `org.icedtea.Session` D-Bus service (session bus)

```rust
#[interface(name = "org.icedtea.Session")]
impl SessionInterface {
    fn lock(&self) { /* Session.Lock() on the login1 proxy */ }
    fn suspend(&self) { /* Manager.Suspend(false) */ }
    fn hibernate(&self) { /* Manager.Hibernate(false) */ }
    fn power_off(&self) { /* Manager.PowerOff(false) */ }
    fn reboot(&self) { /* Manager.Reboot(false) */ }
    fn log_out(&self) { /* forwards to org.icedtea.WM's Quit() via wm_client.rs */ }
    fn is_locked(&self) -> bool { /* same value org.icedtea.WM's IsLocked() reports */ }
}
```

(`Manager.Suspend`/`Hibernate`/`PowerOff`/`Reboot`'s boolean argument is
logind's own "interactive" flag — whether polkit may prompt interactively for
authorization; `false` here since this call already comes from an
authenticated desktop session's own daemon, matching how `loginctl` itself
calls these non-interactively from an already-privileged session context.)
Registered exactly like `clipboard/src/service.rs::spawn` — a
`zbus::blocking::Connection::session()`, `object_server().at(...)`,
`request_name(...)` — same pattern, same crate, same bus.

### `org.icedtea.WM` additions (compositor crate)

- `contract::Event` gains `SessionLockChanged(bool)`; `dbus.rs::event_signal_name`
  gains the `"SessionLockChanged"` arm; the emitter thread's `match` in
  `spawn_service` gains an arm emitting `(seq, locked)` — same shape as every
  other 2-field signal (`WorkspaceSet`'s `(seq, id, active)` is the closest
  precedent).
- `state.rs`'s existing `session_lock_changed(&mut self, locked: bool)`
  (`state.rs:4707`) gains one line, `self.emit(Event::SessionLockChanged(locked));`,
  alongside its existing focus-restoration logic.
- `WmInterface` (`dbus.rs:155`) gains `fn is_locked(&self) -> bool`, calling
  the existing `DbCommand::SessionLocked { reply }` round-trip
  (`dbus.rs:108-111`) — the doc comment there ("Not reachable from
  `WmInterface`") is updated to reflect the promotion; the `DbCommand` variant
  itself is unchanged (it already has the right shape), only its reachability
  changes.

This is the smallest possible compositor change that gives `icedtea-session`
production, non-test-gated read access to lock state and a change
notification, reusing 100% of M4.4's already-reviewed and already-hardened
plumbing rather than inventing a parallel path.

### `config::Power` (config crate)

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Power {
    /// Shell command spawned to actually secure the screen (must itself bind
    /// ext_session_lock_manager_v1 and authenticate before unlocking).
    /// `None` (the default) disables idle-lock and lock-before-sleep —
    /// locking with no configured, real locker is refused (Decision 2).
    pub locker_command: Option<String>,
    /// Lock after this many milliseconds of seat idle. `None` disables
    /// idle-lock even with a locker configured (opt-in, matches swayidle).
    pub lock_idle_timeout_ms: Option<u64>,
    /// Attempt to lock before suspend/hibernate (bounded; never blocks
    /// sleep past logind's own InhibitDelayMaxSec). Default true, but a
    /// no-op without a configured locker_command (Decision 2).
    pub lock_before_sleep: bool,
}
```

added to `Config` alongside `keybindings`/`appearance`/`behavior`, following
the exact pattern `Behavior` already uses; `default_config()` gets
`power: Power { locker_command: None, lock_idle_timeout_ms: None,
lock_before_sleep: true }` — everything locking-related opt-in except the
"attempt it, but only if a locker is actually configured" flag, which is
harmless with the default `locker_command: None`.

### systemd unit

```ini
[Unit]
Description=icedtea session/seat manager (logind integration)
Documentation=https://github.com/quinnjr/icedtea-wm
PartOf=graphical-session.target
After=graphical-session.target

[Service]
Type=simple
ExecStart=%h/.cargo/bin/icedtea-session
Restart=always
RestartSec=1

[Install]
WantedBy=graphical-session.target
```

Same shape as `clipboard/systemd/icedtea-clipboard.service` and
`shell/systemd/icedtea-shell.service`; no `After=` on either of those since
`icedtea-session` doesn't depend on them (only on the compositor's
`org.icedtea.WM` being reachable, which — like the shell's dependency on the
compositor — is handled by retry/backoff on connect failure rather than a
hard systemd ordering, since the compositor's D-Bus service starting is not
itself gated by a systemd unit boundary in this stack).

## Decomposition into milestones

1. **Session discovery + `org.icedtea.Session` skeleton.** `logind.rs`'s
   `Manager`/`Session` proxies, `$XDG_SESSION_ID`/`GetSessionByPID`
   resolution, `Lock`/`Unlock`/`Suspend`/`Hibernate`/`PowerOff`/`Reboot`
   methods wired straight through to logind with no policy yet; `LogOut()`
   forwarding via a `wm_client.rs` `Quit()` call. `service.rs` registers
   `org.icedtea.Session`. The `icedtea-session lock`/`suspend`/... CLI mode.
2. **`org.icedtea.WM` promotion.** The compositor-side `IsLocked()` +
   `SessionLockChanged` addition (Decision 6) — smallest, most isolated
   change, lands first so every later milestone can build against real
   (non-test-only) lock-state observation.
3. **Sleep inhibitor + lock-before-sleep + power-key/suspend-key
   inhibitors.** `Manager.Inhibit` for both `"sleep"` (delay) and
   `"handle-power-key:handle-suspend-key:handle-hibernate-key"` (block);
   `PrepareForSleep` subscription and the bounded lock-then-release flow;
   the `Lock`/`Unlock` signal funnel and locker spawn/wait/kill logic in
   `policy.rs` (kept pure/unit-testable, D-Bus/process effects pushed to
   thin wrapper functions per the crate-layout note above).
4. **Idle→lock.** `idle.rs`'s `ext_idle_notifier_v1` Wayland client (mirrors
   `clipboard/src/manager.rs`'s existing wayland-client setup for a different
   protocol); one `ext_idle_notification_v1` timer at `lock_idle_timeout_ms`;
   `idled` → `Session.Lock()` through the same funnel as milestone 3.
5. **`config::Power` + default keybindings + systemd unit + docs.** The
   config schema addition, `default_config()` additions (power-key bindings;
   `lock_before_sleep: true`/rest-disabled defaults), the systemd unit file,
   and updating `docs/superpowers/specs/2026-08-20-icedtea-de-roadmap.md`'s
   audit table row for "Session & system plumbing" once merged (out of this
   spec's own scope to edit now, per the run's rules, but noted as the
   natural follow-up).

## Testing

- **`policy.rs` is pure and exhaustively unit-tested without any D-Bus or
  Wayland**: given `(locked: bool, locker_configured: bool,
  lock_before_sleep: bool, idle_fired: bool)` inputs, assert the resulting
  decision (`Lock` / `NoOp` with a specific warn reason). This is the M4.4
  precedent applied here: `dbus.rs::event_signal_name` and
  `input.rs::match_action` (`compositor/src/input.rs:444`) are both already
  pure functions unit-tested exactly this way in this codebase.
- **`icedtea-harness` integration**: the harness already provides a headless
  compositor with `ext_idle_notifier_v1` and `ext_session_lock_manager_v1`
  advertised (M4.4's harness clients) and virtual-pointer/virtual-keyboard
  activity injectors (M4.2/M4.3). Extend it with a fake `logind` — not real
  `systemd-logind` (unavailable/unsafe in CI and in the sandboxed vagrant dev
  VM referenced by this repo's `.vagrant/` — suspending the CI box is not an
  option): a minimal in-process D-Bus mock of `org.freedesktop.login1` that
  answers `GetSession`/`Inhibit`/`Lock`/`Unlock`/`PrepareForSleep` on the
  **session** bus for tests only (swapped in via a connection the daemon
  accepts as a constructor argument in test builds — the same "abstracted so
  a test can inject a recording mock" shape `shell/src/wm_client.rs:79`'s
  `WmCommands` trait already uses for the taskbar).
  1. **Idle→lock round-trip**: fake logind + real headless
     `ext_idle_notifier_v1`; inject no activity past a short configured
     timeout; assert the mock's `Lock()` was called and (with a stub
     `locker_command` that just touches a marker file) the marker appears.
  2. **Lock-before-sleep round-trip**: fake `PrepareForSleep(true)`; assert
     the stub locker was spawned and the inhibitor fd was released only after
     the marker file's mtime, not before (ordering, not just occurrence).
  3. **Bounded release**: a stub locker that never marks itself "locked"
     (never calls the mocked `IsLocked`-backing state to true); assert the
     inhibitor fd is still released within the test's budget window (no
     hang) — the "never block indefinitely" invariant from Decision 5,
     tested the same deliberate way M4.4's own criterion 3 tested "locker
     death keeps the session locked": force the bad case and assert the safe
     outcome.
  4. **`org.icedtea.WM` promotion**: extend `compositor/tests/client_protocol.rs`
     with a `IsLocked()` call and a `SessionLockChanged` signal assertion
     around the existing M4.4 lock round-trip test (criterion 2's harness
     flow already exercises take-lock/unlock; this only adds the two new
     observation points to that same flow).
  5. **No-locker-configured is a safe no-op**: idle timeout fires and
     `PrepareForSleep(true)` both, with `locker_command: None`; assert
     neither ever calls `Lock()`/spawns anything, and sleep is never delayed
     past a trivial bound.
- **Gate**: `cargo test --workspace` + `cargo clippy --all-targets -- -D
  warnings`, this repo's standing bar for every crate (matches the gate every
  cited prior spec in this directory uses).

## Success criteria

1. `icedtea-session` resolves its own logind session at startup (both the
   `$XDG_SESSION_ID` and `GetSessionByPID` paths covered by a test double).
2. `PrepareForSleep(true)` with a configured locker results in a lock attempt
   before the sleep inhibitor is released, and the release is bounded even if
   the locker never confirms — automated (Testing items 2–3).
3. An idle timeout with a configured locker results in a lock attempt via the
   same funnel — automated (Testing item 1).
4. With no `locker_command` configured, neither idle nor pre-sleep ever
   attempts to lock and neither ever delays sleep beyond a trivial bound —
   automated (Testing item 5), the safety property from Decision 2.
5. `org.icedtea.WM` exposes a production `IsLocked()` method and
   `SessionLockChanged` signal, exercised by the existing M4.4 lock-round-trip
   test plus new assertions — automated (Testing item 4).
6. `org.icedtea.Session`'s `Lock`/`Suspend`/`Hibernate`/`PowerOff`/`Reboot`/
   `LogOut` methods are reachable and `LogOut()` observably reaches
   `org.icedtea.WM`'s existing `Quit()` path.
7. Power-key/suspend-key/hibernate-key presses reach a compositor keybinding
   (not logind's own default action) once `icedtea-session` is running,
   verified by asserting the corresponding `Manager.Inhibit("handle-...",
   ..., "block")` call was made against the fake logind (the actual keysym
   delivery is already covered by the existing keybinding-dispatch tests in
   `compositor/src/input.rs` — this only needs to prove the inhibitor is
   taken, which is what makes those keysyms reach the compositor instead of
   being consumed by logind first).

## Out of scope (explicit)

- **The lock-screen UI itself** (password prompt, PAM). That's Track B's
  **B2**, already named in the roadmap as depending on the pure-Rust UI
  toolkit (Track C M3). This design only defines the `locker_command`
  extension point B2 will eventually fill by default.
- **Power settings UI** (a Settings page for `lock_idle_timeout_ms`,
  `locker_command`, lid/power-key behavior). Track B's **B7** (Settings
  breadth); this design only defines the `config::Power` schema B7's UI would
  read/write, following exactly how the existing Settings app's Displays page
  already reads/writes `config::DisplayConfig`
  (`docs/superpowers/specs/2026-08-19-icedtea-displays-and-review-fixes-design.md`).
- **Battery/UPower integration** (battery percentage, low-battery warnings,
  power profiles). Named separately in the roadmap's cross-cutting backends
  list ("Power/battery: UPower + logind") — UPower is a different D-Bus
  service (`org.freedesktop.UPower`) with its own device/percentage model;
  out of scope here, which is scoped strictly to `org.freedesktop.login1`.
- **Clamshell/lid policy refinement** (e.g. "don't suspend on lid-close if an
  external monitor is connected"). Requires `SW_LID` switch-device visibility
  in the `wlr` crate, which does not exist today (Grounding, above) — a
  future `wlr`-crate addition, not this daemon. Decision 3's reliance on
  logind's own default lid handling is the interim behavior (suspend
  unconditionally on lid-close, same as most single-purpose logind clients
  ship by default).
- **Multi-seat / multi-session** beyond the single logind session icedtea's
  own compositor instance runs as. Matches every prior wlr-port spec's
  explicit single-seat scoping (e.g. M4.4's "Multi-seat lock semantics beyond
  the single headless seat" out-of-scope line).
- **A real `systemd-logind` in CI/dev-VM tests.** Testing uses a fake logind
  D-Bus mock (Testing, above) rather than exercising real suspend/hibernate,
  which is unsafe/unavailable in CI and in the project's own `.vagrant`-based
  dev environment.
- **Reboot/poweroff/hibernate authorization prompts (polkit UI).** The
  `interactive=false` argument to `Manager.Suspend`/etc. means icedtea relies
  on the desktop session already being authorized by logind's default polkit
  rules for its own active session (the standard "the user at the seat's own
  active session may suspend/etc. without a password" policy shipped by most
  distros); a polkit *agent* (prompting for privilege escalation beyond that)
  is the roadmap's separate **A8** item, not this one.

## Risks

- **Inhibitor fd leaks are a whole-machine footgun.** A bug that fails to
  close the `"sleep"` delay inhibitor fd (or fails to re-acquire it after
  resume) silently prevents the machine from ever suspending again — worse
  than "icedtea's feature doesn't work," it breaks a system-wide primitive
  other software may depend on. Mitigation: the bounded-release logic
  (Decision 5) always closes the fd on every code path (a `defer`-style
  `Drop` guard around the fd handle, not an early-return that could skip the
  close), and Testing item 3 specifically forces the "locker never confirms"
  case to prove the bound holds.
- **The `Lock`/`Unlock` signal funnel could double-fire or race** if both an
  idle timeout and a `PrepareForSleep` land close together (falling asleep
  right as the idle timer also expires). Mitigation: the funnel's "if not
  already locked" check (via `IsLocked()`) before spawning a locker makes a
  second concurrent trigger a no-op rather than a second locker process;
  `policy.rs`'s pure decision function is exactly the place to unit-test this
  interleaving without needing real timing.
- **A misbehaving `locker_command` that never exits** (crashes into a
  zombie, or genuinely hangs) leaves `icedtea-session` waiting on `Child::wait()`
  forever in its background thread, and — separately — leaves the visible
  screen locked with no automatic recovery. This is an accepted, *correct*
  failure mode for the locked-state half (a hung locker should not
  auto-unlock the screen — that would defeat the point) but the `.wait()`
  thread itself must not block anything else (the sleep-inhibitor path
  already doesn't depend on it: Decision 5's release is on a lock-*confirmed*
  bound via `IsLocked()`, not on the child's exit).
- **Power-key inhibitor scope is coarse** (`handle-power-key:handle-suspend-key:
  handle-hibernate-key` as one Inhibit call, matching the well-known
  GNOME/KDE pattern) — if a distro's logind.conf maps hibernate-key to
  something unusual, icedtea now intercepts it whether or not it has a
  keybinding configured for it, and an unbound key silently does nothing
  (better than logind's own surprising default, but worth calling out).
  Mitigation: default keybindings cover all three (Milestone 3); a user who
  removes a binding without disabling the corresponding inhibitor gets a
  silent no-op key rather than a crash — acceptable, matches "safe failure"
  posture elsewhere in this design.
- **`GetSessionByPID` fallback correctness** depends on `icedtea-session`
  being launched inside the same login session's cgroup as the compositor
  (true for the intended `systemd --user` unit launch path; not guaranteed
  for an ad hoc manual run from an unrelated shell). Mitigation: this is a
  fallback, not the primary path; the primary `$XDG_SESSION_ID` path is
  reliable for the shipped systemd-unit launch, and a resolution failure is
  a loud startup error (Architecture, above), not a silent wrong-session bug.
