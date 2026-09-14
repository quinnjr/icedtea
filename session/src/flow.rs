//! Session lifecycle flow: the bounded lock-before-sleep path and the
//! `Lock`/`Unlock` locker funnel.
//!
//! Every effect goes through the [`Logind`] and [`WmClient`] seams, so the
//! whole flow is driven from tests without a bus. The hard invariants live
//! here: the sleep delay inhibitor is released on *every* path (spec Decision
//! 5, via the [`SleepInhibitor`] guard) and the pre-sleep wait never runs
//! meaningfully past [`SLEEP_LOCK_BUDGET`]. Every D-Bus call on that path is
//! bounded — `IsLocked` by
//! [`crate::wm_client::COMPOSITOR_CALL_TIMEOUT`], `Session.Lock`/`Unlock` by
//! [`crate::logind::LOGIND_CALL_TIMEOUT`] — and no call is made while the
//! shared [`FlowState`] mutex is held, so a stalled system bus can neither
//! overshoot the budget nor block the sleep-inhibitor release.
//!
//! Idle and pre-sleep *policy* do not spawn a locker themselves: they ask
//! logind to lock ([`LockFlow::request_logind_lock`]) and the spawn happens in
//! the `Lock` signal handler ([`LockFlow::on_lock`]) — the spec's single
//! funnel. The spawn commit is atomic against concurrent triggers, and a
//! `Session.Unlock` this daemon issued is recognized by its generation marker
//! so it never kills a locker that spawned after it. Because that `Lock`
//! signal is delivered on the logind signal-loop thread, the pre-sleep wait
//! runs on a dedicated worker — blocking the loop would starve the funnel.

use std::collections::VecDeque;
use std::os::unix::process::CommandExt as _;
use std::process::{Child, Command, ExitStatus};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use icedtea_config::Power;

use crate::logind::{Logind, LogindEvents, PowerKeyInhibitor, SleepInhibitor};
use crate::policy::{self, Decision, LockReason};
use crate::service::{SharedLogind, SharedWm};
use crate::wm_client::LockState;

/// How often the bounded pre-sleep wait re-checks `IsLocked()`.
pub const SLEEP_POLL_INTERVAL: Duration = Duration::from_millis(100);
/// Hard ceiling on the pre-sleep wait (spec Decision 5) — comfortably inside
/// logind's 5s default `InhibitDelayMaxSec`. A single in-flight `IsLocked` call
/// may overshoot this by up to [`crate::wm_client::COMPOSITOR_CALL_TIMEOUT`]; the two together
/// stay under 5s (pinned by a unit test).
pub const SLEEP_LOCK_BUDGET: Duration = Duration::from_secs(3);
/// How often the locker-waiter thread checks whether the locker exited.
const LOCKER_WAIT_POLL: Duration = Duration::from_millis(20);

/// A spawned locker plus the generation it was created with. The generation
/// lets a waiter for an older locker recognize that a newer one has since
/// been spawned and stand down.
struct LockerSlot {
    generation: u64,
    child: Child,
}

/// The mutable state shared with the locker-waiter thread.
struct FlowState {
    /// The one live sleep delay inhibitor, if any.
    sleep: Option<SleepInhibitor>,
    /// The daemon-lifetime power-key block inhibitor.
    power_key: Option<PowerKeyInhibitor>,
    /// The running locker child, if one is up.
    locker: Option<LockerSlot>,
    /// Monotonic id of the most recent locker spawn. A waiter acts only while
    /// its id still equals this one, so a locker that exited before a newer
    /// one was spawned cannot issue a stale `Session.Unlock` (which compositor
    /// bookkeeping would turn into a kill of the newer locker).
    active_generation: u64,
    /// The generations whose exits prompted `Session.Unlock()` calls this
    /// daemon issued whose logind `Unlock` echoes are still in flight, oldest
    /// first. [`LockFlow::on_unlock`] consumes one per echo, so a daemon-issued
    /// unlock is never mistaken for a genuine external unlock. A FIFO (rather
    /// than a single slot) is needed because two lockers can exit back-to-back
    /// and queue two echoes before either is delivered: the first echo must not
    /// consume the second's marker, or the second would be misread as external
    /// and kill an unrelated running locker.
    self_unlock_gens: VecDeque<SelfUnlockMarker>,
}

/// One in-flight self-unlock echo marker: the locker generation whose exit
/// prompted the `Session.Unlock()` this daemon issued, and a lifetime bound
/// (see [`SELF_UNLOCK_MARKER_TTL`]).
///
/// The generation correlates the marker to the locker it belongs to. The
/// lifetime bound keeps a *lost* echo (the `Unlock` signal never delivered,
/// e.g. logind restarted) from swallowing the next genuine external unlock:
/// once the marker expires it no longer absorbs a signal, so an external
/// unlock still kills a live locker.
#[derive(Debug, Clone, Copy)]
struct SelfUnlockMarker {
    generation: u64,
    expires_at: Instant,
}

/// Defensive ceiling on [`FlowState::self_unlock_gens`]. Only a handful of
/// self-unlock echoes can be in flight at once (one per locker that exited
/// without a newer spawn); this bound keeps a wedged or lost echo from growing
/// the queue without limit. The oldest marker is dropped past the cap.
const MAX_SELF_UNLOCK_MARKERS: usize = 64;

/// How long a self-unlock marker may absorb an `Unlock` signal. A local-bus
/// echo lands in well under a millisecond and a stalled `Session.Unlock()`
/// call is itself bounded by [`crate::logind::LOGIND_CALL_TIMEOUT`], so 2s is
/// ample for every legitimate echo; past it the marker is treated as lost and
/// stops masking external unlocks.
const SELF_UNLOCK_MARKER_TTL: Duration = Duration::from_secs(2);

/// Record that the daemon issued a `Session.Unlock()` for `generation`; its
/// echo is now in flight. Appends to the FIFO, pruning the oldest marker at the
/// defensive cap, and stamps the lifetime bound above.
fn push_self_unlock(state: &mut FlowState, generation: u64) {
    if state.self_unlock_gens.len() >= MAX_SELF_UNLOCK_MARKERS {
        state.self_unlock_gens.pop_front();
    }
    state.self_unlock_gens.push_back(SelfUnlockMarker {
        generation,
        expires_at: Instant::now() + SELF_UNLOCK_MARKER_TTL,
    });
}

/// Drop the newest recorded marker for `generation` after a failed
/// `Session.Unlock()` call. A failed call produces no echo, so the marker must
/// not linger and swallow a later external unlock.
fn remove_self_unlock(state: &mut FlowState, generation: u64) {
    if let Some(idx) = state
        .self_unlock_gens
        .iter()
        .rposition(|marker| marker.generation == generation)
    {
        state.self_unlock_gens.remove(idx);
    }
}

/// What [`LockFlow::request_lock`] did, consumed by [`LockFlow::on_lock`] to
/// log the outcome of the funnel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LockAttempt {
    /// A new locker was spawned; the lock is not confirmed yet.
    Spawned,
    /// An earlier locker is still running, but the compositor has not
    /// confirmed the lock yet.
    AlreadyRunning,
    /// The compositor already reports locked.
    Locked,
    /// Policy refused, or the spawn failed (a no-op): nothing will lock this
    /// cycle.
    Refused,
}

/// Drives lock decisions and owns the inhibitor guards and the locker child.
pub struct LockFlow {
    logind: SharedLogind,
    wm: SharedWm,
    locker_command: Option<String>,
    lock_before_sleep: bool,
    poll_interval: Duration,
    budget: Duration,
    state: Arc<Mutex<FlowState>>,
}

impl LockFlow {
    /// Build the flow from the daemon's config and arm both inhibitors.
    pub fn new(logind: SharedLogind, wm: SharedWm, power: &Power) -> Self {
        Self::with_timing(
            logind,
            wm,
            power.locker_command.clone(),
            power.lock_before_sleep,
            SLEEP_POLL_INTERVAL,
            SLEEP_LOCK_BUDGET,
        )
    }

    /// Build the flow with explicit timing, so tests can exercise the bound
    /// without waiting out the production 3s.
    pub fn with_timing(
        logind: SharedLogind,
        wm: SharedWm,
        locker_command: Option<String>,
        lock_before_sleep: bool,
        poll_interval: Duration,
        budget: Duration,
    ) -> Self {
        let flow = Self {
            logind,
            wm,
            locker_command,
            lock_before_sleep,
            poll_interval,
            budget,
            state: Arc::new(Mutex::new(FlowState {
                sleep: None,
                power_key: None,
                locker: None,
                active_generation: 0,
                self_unlock_gens: VecDeque::new(),
            })),
        };
        flow.arm();
        flow
    }

    /// Take the daemon-lifetime power-key block inhibitor and the initial
    /// sleep delay inhibitor. A failure is logged, never fatal: without the
    /// block inhibitor logind keeps the keys (safe), and without the delay
    /// inhibitor there is simply nothing to release.
    fn arm(&self) {
        let power_key = PowerKeyInhibitor::acquire(&*self.logind);
        let sleep = SleepInhibitor::acquire(&*self.logind);
        let mut state = self.state();
        match power_key {
            Ok(guard) => state.power_key = Some(guard),
            Err(err) => tracing::warn!(
                %err,
                "could not take the power-key block inhibitor; logind keeps handling the keys"
            ),
        }
        match sleep {
            Ok(guard) => state.sleep = Some(guard),
            Err(err) => tracing::warn!(
                %err,
                "could not take the sleep delay inhibitor; lock-before-sleep is off this cycle"
            ),
        }
    }

    /// `PrepareForSleep(bool)`: `true` requests a lock through the logind
    /// funnel and waits (bounded) for confirmation before releasing sleep,
    /// `false` (resume) re-arms a fresh one-shot delay inhibitor.
    ///
    /// This is dispatched on the logind signal-loop thread, and the `Lock`
    /// signal that actually spawns the locker can only be delivered by that
    /// same thread. So the lock request and the bounded wait are handed to a
    /// dedicated worker here and this returns at once — blocking would
    /// deadlock the funnel: the `Lock` the request triggers could never be
    /// processed, the budget would burn, and the delay inhibitor would be
    /// released with the screen still unlocked. The worker owns the delay
    /// guard, so it is released on every path (confirmed, budget exceeded, or
    /// a failed request); with `lock_before_sleep` disabled there is no worker
    /// and no wait.
    ///
    /// A locker process already running is *not* proof the screen is secured:
    /// an idle-triggered locker may still be starting up. So, when
    /// `lock_before_sleep` is enabled, the wait runs whenever logind accepts
    /// the lock request, whether the trigger just spawned the locker or found
    /// one already running. The already-locked case falls out of the wait's
    /// first poll (immediate confirmation), so no extra `IsLocked` round-trip
    /// precedes it and the bounded total stays under logind's ceiling.
    pub fn on_prepare_for_sleep(&self, going_to_sleep: bool) {
        if !going_to_sleep {
            self.acquire_sleep();
            return;
        }
        // Policy is evaluated here (it is pure and cheap), but the lock
        // request and the bounded wait run on the worker. A policy no-op —
        // no locker configured, or `lock_before_sleep` disabled — must not
        // delay sleep, so it releases immediately and spawns no worker.
        if !self.should_request_lock(LockReason::PrepareForSleep) {
            self.release_sleep();
            return;
        }
        // A delay inhibitor is one-shot and only re-armed on resume. If the
        // daemon never got one (the boot acquisition failed, or a resume's
        // re-arm failed), there is nothing to hold sleep open, so try a fresh
        // `Inhibit` right before the wait. If it still cannot be taken, the
        // lock-before-sleep hold cannot be honored: log that at error level
        // so the lost invariant is visible.
        self.ensure_sleep_guard(LockReason::PrepareForSleep);
        // Move the guard off this thread and into the worker, which owns and
        // releases it. If the worker cannot start, dropping the closure drops
        // the guard, so the inhibitor is still released.
        let sleep = {
            let mut state = self.state();
            state.sleep.take()
        };
        if sleep.is_none() {
            tracing::error!(
                "suspend announced with no sleep delay inhibitor; the lock-before-sleep hold cannot be honored"
            );
        }
        let logind = Arc::clone(&self.logind);
        let wm = Arc::clone(&self.wm);
        let poll_interval = self.poll_interval;
        let budget = self.budget;
        let worker = std::thread::Builder::new()
            .name("icedtea-session-presleep".to_string())
            .spawn(move || run_pre_sleep_lock(logind, wm, poll_interval, budget, sleep));
        if let Err(err) = worker {
            tracing::warn!(
                %err,
                "could not spawn the pre-sleep lock worker; releasing sleep"
            );
        }
    }

    /// Whether policy says to request a lock for `reason`, logging the refusal
    /// reason at warn level. The single pre-decide gate shared by the idle
    /// request path ([`Self::request_logind_lock`]) and the pre-sleep path
    /// ([`Self::on_prepare_for_sleep`]); the funnel's own `decide` call in
    /// [`Self::request_lock`] additionally checks the live lock state.
    fn should_request_lock(&self, reason: LockReason) -> bool {
        match policy::decide(
            reason,
            false,
            self.locker_command.is_some(),
            self.lock_before_sleep,
        ) {
            Decision::SpawnLocker => true,
            Decision::NoOp(why) => {
                tracing::warn!(reason = ?reason, why, "not locking");
                false
            }
        }
    }

    /// Ensure a sleep delay inhibitor is held, acquiring a fresh one if the
    /// last was consumed (or never taken) and logging at error level when it
    /// cannot be honored. The acquire call is made outside the state lock, so
    /// it never blocks another flow operation on the mutex.
    fn ensure_sleep_guard(&self, reason: LockReason) {
        if self.state().sleep.is_some() {
            return;
        }
        match SleepInhibitor::acquire(&*self.logind) {
            Ok(guard) => {
                let mut state = self.state();
                if state.sleep.is_none() {
                    state.sleep = Some(guard);
                    tracing::debug!(reason = ?reason, "sleep delay inhibitor re-armed");
                } else {
                    // Another path armed one between our check and here; keep
                    // the existing guard and drop ours.
                    drop(guard);
                }
            }
            Err(err) => tracing::error!(
                %err,
                reason = ?reason,
                "could not acquire a sleep delay inhibitor; the lock-before-sleep hold cannot be honored"
            ),
        }
    }

    /// An idle timeout: ask logind to lock this session, which emits the `Lock`
    /// signal the spawn funnel handles (spec Decision 7). No sleep inhibitor is
    /// involved, so there is nothing to release.
    pub fn on_idle(&self) {
        let _ = self.request_logind_lock(LockReason::Idle);
    }

    /// An explicit `Session.Lock` (an external lock, or logind's echo of our
    /// own request): the funnel that actually spawns the locker. The outcome
    /// is logged; a refusal means nothing locked this cycle, which the idle and
    /// pre-sleep paths rely on (and which [`crate::service`] surfaces to the
    /// `lock` CLI by confirming against the compositor).
    pub fn on_lock(&self) {
        match self.request_lock(LockReason::Idle) {
            LockAttempt::Spawned => tracing::debug!("lock funnel spawned a locker"),
            LockAttempt::AlreadyRunning => {
                tracing::debug!("lock funnel found a locker already running")
            }
            LockAttempt::Locked => tracing::debug!("lock funnel found the session already locked"),
            LockAttempt::Refused => tracing::error!(
                "lock request refused: no locker was spawned, the session is not locked"
            ),
        }
    }

    /// An `Unlock` signal from logind: either the echo of our own
    /// `Session.Unlock()` (ignored) or a genuine external unlock (e.g.
    /// `loginctl unlock-session`), in which case a still-running locker is
    /// killed so the screen is not left locked with a stale process. The
    /// Wayland unlock already happened, so this does not round-trip another
    /// `Session.Unlock`.
    ///
    /// The echo is correlated by generation: `self_unlock_gens` is a FIFO of
    /// the locker exits whose `Session.Unlock()` requests we issued. Each echo
    /// consumes the oldest marker, so two back-to-back exits cannot have the
    /// first echo eat the second's marker. A marker means the unlock is ours;
    /// a locker that spawned *after* it (a greater generation) therefore
    /// survives, and no locker is killed. With no marker the signal is
    /// external, so the current locker is killed.
    ///
    /// Markers are lifetime-bounded ([`SELF_UNLOCK_MARKER_TTL`]): a marker for
    /// an echo that was never delivered is pruned, so the next genuine external
    /// unlock is not swallowed and still kills the running locker.
    pub fn on_unlock(&self) {
        let now = Instant::now();
        let mut guard = self.state();
        guard
            .self_unlock_gens
            .retain(|marker| marker.expires_at > now);
        let Some(marker) = guard.self_unlock_gens.pop_front() else {
            let slot = guard.locker.take();
            drop(guard);
            if let Some(mut slot) = slot {
                tracing::info!("external unlock; terminating the running locker");
                terminate_locker(&mut slot.child);
            }
            return;
        };
        match guard.locker.as_ref().map(|slot| slot.generation) {
            Some(current) if current > marker.generation => tracing::debug!(
                marker = marker.generation,
                current,
                "our earlier Session.Unlock echo; a newer locker survives"
            ),
            _ => tracing::debug!(
                marker = marker.generation,
                "ignoring the echo of our own Session.Unlock"
            ),
        }
    }

    /// The compositor's authoritative `SessionLockChanged(bool)` signal, fed
    /// from the daemon's subscription as an additional cross-check on
    /// logind's `Lock`/`Unlock` funnel.
    ///
    /// A transition to *locked* needs no action: logind's `Lock` is the
    /// primary driver and has already asked the funnel to spawn a locker. A
    /// transition to *unlocked* is the Wayland side of an unlock that already
    /// happened, so it is routed through the same external-unlock handling as
    /// logind's `Unlock` ([`Self::on_unlock`]): a still-running locker is
    /// terminated, while a self-unlock marker absorbs an unlock that is one of
    /// our own echoes so a newer locker survives. This matters because logind
    /// will not always deliver an `Unlock` for an external unlock, and the
    /// compositor's own state is the authority on whether the screen is
    /// actually unlocked.
    pub fn on_session_lock_changed(&self, locked: bool) {
        if locked {
            return;
        }
        self.on_unlock();
    }

    /// Whether a locker child is currently running.
    pub fn locker_running(&self) -> bool {
        self.state().locker.is_some()
    }

    /// Ask logind to lock this session — the request half of the spec's single
    /// funnel. The actual spawn happens in [`Self::on_lock`] when logind echoes
    /// the `Lock` signal. Returns whether the request was issued.
    ///
    /// Idle uses this directly (on its own thread). The pre-sleep path does
    /// *not*: its request must run off the signal loop, so it shares
    /// [`Self::should_request_lock`] and issues the call on its worker.
    fn request_logind_lock(&self, reason: LockReason) -> bool {
        if !self.should_request_lock(reason) {
            return false;
        }
        match self.logind.lock() {
            Ok(()) => true,
            Err(err) => {
                tracing::warn!(%err, reason = ?reason, "Session.Lock call failed");
                false
            }
        }
    }

    /// Re-read the lock state *immediately* before deciding, per the caller
    /// obligation in [`crate::policy`]. For *policy*, a locker already running
    /// counts as locked even if the compositor has not published the lock yet,
    /// so a second near-simultaneous trigger cannot double-spawn; the outcome
    /// still distinguishes "confirmed locked" from "just running" so the sleep
    /// path can await confirmation.
    fn request_lock(&self, reason: LockReason) -> LockAttempt {
        let confirmed = self.wm.is_locked();
        let running = self.locker_running();
        match policy::decide(
            reason,
            confirmed || running,
            self.locker_command.is_some(),
            self.lock_before_sleep,
        ) {
            Decision::SpawnLocker => self.spawn_locker(),
            Decision::NoOp(why) => {
                tracing::warn!(reason = ?reason, why, "not locking");
                if confirmed {
                    LockAttempt::Locked
                } else if running {
                    LockAttempt::AlreadyRunning
                } else {
                    LockAttempt::Refused
                }
            }
        }
    }

    /// Spawn the configured locker, install it as the active generation, and
    /// start the waiter that reaps it and mirrors its exit into
    /// `Session.Unlock`.
    ///
    /// The install is atomic against concurrent triggers: the process is
    /// spawned outside the state lock, then committed under it only if no other
    /// locker is installed. The loser in a race drops its own child (rather
    /// than orphaning it) and reports [`LockAttempt::AlreadyRunning`].
    fn spawn_locker(&self) -> LockAttempt {
        let Some(command) = self.locker_command.as_deref() else {
            tracing::warn!("SpawnLocker with no locker_command; refusing to lock");
            return LockAttempt::Refused;
        };
        // The configured string is shell-interpreted so it may carry its own
        // arguments (and, in tests, compound steps). This is deliberately NOT
        // the compositor's hardened no-shell `"spawn"` path, which parses an
        // argv and never invokes a shell; the two are not equivalent. To keep
        // an external unlock effective, the shell is started in its own
        // process group and the whole group is killed by
        // [`terminate_locker`], not just the `sh` wrapper.
        let mut child = match Command::new("sh")
            .arg("-c")
            .arg(command)
            .process_group(0)
            .spawn()
        {
            Ok(child) => child,
            Err(err) => {
                tracing::warn!(%err, command, "could not spawn the locker");
                return LockAttempt::Refused;
            }
        };
        let generation = {
            let mut state = self.state();
            if state.locker.is_some() {
                // Lost the race: another trigger committed first. Drop our
                // just-spawned child instead of overwriting the winner.
                drop(state);
                terminate_locker(&mut child);
                tracing::info!("a locker was already running; dropped the duplicate spawn");
                return LockAttempt::AlreadyRunning;
            }
            let generation = state.active_generation.wrapping_add(1);
            state.active_generation = generation;
            state.locker = Some(LockerSlot { generation, child });
            generation
        };
        self.spawn_waiter(generation);
        tracing::info!(command, "spawned the locker");
        LockAttempt::Spawned
    }

    fn spawn_waiter(&self, generation: u64) {
        let state = Arc::clone(&self.state);
        let logind = Arc::clone(&self.logind);
        let waiter = std::thread::Builder::new()
            .name("icedtea-session-locker".to_string())
            .spawn(move || locker_waiter(state, logind, generation));
        if let Err(err) = waiter {
            tracing::warn!(
                %err,
                "could not spawn the locker waiter thread; reaping the locker"
            );
            self.abandon_locker(generation);
        }
    }

    /// A waiter could not be started: kill and reap the locker it would have
    /// watched, clear its slot, and mirror the abort into `Session.Unlock` so
    /// the session is not left with an unmanaged locked state. No-op if a
    /// newer locker has already replaced this one.
    ///
    /// The locker is taken out under the state lock (with the generation check)
    /// so a new spawn cannot install itself in the gap; the kill/reap and the
    /// blocking `Session.Unlock` then happen outside the lock, so a stalled
    /// system bus cannot block every other flow operation on the mutex.
    fn abandon_locker(&self, generation: u64) {
        let slot = {
            let mut guard = self.state();
            let current = guard
                .locker
                .as_ref()
                .is_some_and(|slot| slot.generation == generation);
            if !current || guard.active_generation != generation {
                return;
            }
            guard.locker.take()
        };
        let Some(mut slot) = slot else {
            return;
        };
        terminate_locker(&mut slot.child);
        commit_unlock(&self.state, &*self.logind, generation);
    }

    /// Take the sleep inhibitor (if armed) and drop it, closing the fd.
    fn release_sleep(&self) {
        let inhibitor = self.state().sleep.take();
        drop(inhibitor);
    }

    /// Acquire a fresh delay inhibitor for the next sleep cycle; `delay` fds
    /// are one-shot and the previous one was consumed by [`Self::release_sleep`].
    fn acquire_sleep(&self) {
        match SleepInhibitor::acquire(&*self.logind) {
            Ok(guard) => {
                self.state().sleep = Some(guard);
                tracing::debug!("sleep delay inhibitor re-armed");
            }
            Err(err) => tracing::warn!(
                %err,
                "could not re-arm the sleep delay inhibitor; lock-before-sleep is off this cycle"
            ),
        }
    }

    fn state(&self) -> MutexGuard<'_, FlowState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl LogindEvents for LockFlow {
    fn prepare_for_sleep(&self, going_to_sleep: bool) {
        self.on_prepare_for_sleep(going_to_sleep);
    }

    fn lock(&self) {
        self.on_lock();
    }

    fn unlock(&self) {
        self.on_unlock();
    }
}

/// The pre-sleep lock attempt, moved off the logind signal-loop thread: ask
/// logind to lock, wait up to `budget` for the compositor to confirm, then
/// release the sleep delay inhibitor by dropping `_sleep` (on **every** path —
/// confirmed, budget exceeded, or a failed request). It runs on its own
/// thread because the `Lock` signal the request triggers is delivered to the
/// signal loop, which must stay free to process it.
///
/// The wait distinguishes a slow locker from an unreachable compositor using
/// the tri-state [`LockState`] read: `Unreachable` is definitive, so the wait
/// stops at once and proceeds unlocked rather than burning the budget and
/// then misreporting it as "locker did not confirm". When sleep is released
/// without confirmation, it is logged at error level: the suspend is
/// proceeding with the session UNLOCKED.
fn run_pre_sleep_lock(
    logind: SharedLogind,
    wm: SharedWm,
    poll_interval: Duration,
    budget: Duration,
    _sleep: Option<SleepInhibitor>,
) {
    if let Err(err) = logind.lock() {
        tracing::error!(%err, "Session.Lock call failed; suspend proceeding with session UNLOCKED");
        return;
    }
    let deadline = Instant::now() + budget;
    let mut polls: u64 = 0;
    loop {
        polls += 1;
        match wm.lock_state() {
            LockState::Locked => return,
            // No amount of further polling can confirm a lock the compositor
            // cannot even be asked about, so stop early and release.
            LockState::Unreachable => {
                tracing::error!(
                    polls,
                    budget_ms = budget.as_millis() as u64,
                    "suspend proceeding with session UNLOCKED: compositor unreachable"
                );
                return;
            }
            LockState::Unlocked => {}
        }
        let now = Instant::now();
        if now >= deadline {
            tracing::error!(
                polls,
                budget_ms = budget.as_millis() as u64,
                "suspend proceeding with session UNLOCKED: locker did not confirm within the budget"
            );
            return;
        }
        std::thread::sleep(poll_interval.min(deadline - now));
    }
}

/// Commit a `Session.Unlock()` for `generation`, but only while it is still the
/// most recent spawn. The generation check and the `self_unlock_gens` marker
/// happen under the state lock so a new spawn cannot install itself between the
/// check and the commit; the blocking logind call itself is made outside the
/// lock, so a stalled system bus cannot block the sleep-inhibitor release or
/// any other flow path. The marker records *which* generation's exit prompted
/// the unlock, so [`LockFlow::on_unlock`] can tell the echo apart from an
/// external unlock and lets a newer locker survive; the FIFO keeps two
/// back-to-back exits' echoes distinct. On a failed call (no echo will come)
/// the marker is removed again. Shared by the waiter's natural-exit path
/// ([`finish_locker`]) and the waiter-spawn-failure teardown
/// ([`LockFlow::abandon_locker`]).
fn commit_unlock(
    state: &Mutex<FlowState>,
    logind: &(dyn Logind + Send + Sync),
    generation: u64,
) -> bool {
    {
        let mut guard = state.lock().unwrap_or_else(|e| e.into_inner());
        if guard.active_generation != generation {
            return false;
        }
        push_self_unlock(&mut guard, generation);
    }
    if let Err(err) = logind.unlock() {
        tracing::warn!(%err, "Session.Unlock after locker exit failed");
        // A failed call produces no echo; do not let an unconsumed marker
        // swallow a later external unlock.
        let mut guard = state.lock().unwrap_or_else(|e| e.into_inner());
        remove_self_unlock(&mut guard, generation);
    }
    true
}

/// [`commit_unlock`] for a locker that exited naturally; see its doc.
fn finish_locker(
    state: &Mutex<FlowState>,
    logind: &(dyn Logind + Send + Sync),
    generation: u64,
) -> bool {
    commit_unlock(state, logind, generation)
}

/// SIGKILL the locker's whole process group, then reap it. The locker is
/// spawned as its own process-group leader (`process_group(0)` in
/// [`LockFlow::spawn_locker`]), so this reaches the configured command's own
/// descendant processes rather than only the `sh` wrapper. `sh` is already
/// required to spawn the locker, so its builtin `kill` is used to signal the
/// negative pgid; `child.kill()` is kept as a fallback if that fails.
fn terminate_locker(child: &mut Child) {
    let pid = child.id();
    if let Err(err) = Command::new("sh")
        .arg("-c")
        .arg(format!("kill -KILL -- -{pid} 2>/dev/null"))
        .status()
    {
        tracing::warn!(%err, pid, "could not signal the locker process group");
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Poll `try_wait` until the locker exits (or an external unlock took it),
/// then mirror a natural exit into `Session.Unlock`. A locker is trusted to
/// exit only after a successful unlock, exactly as `swaylock`/`gtklock` do; a
/// locker that exited **unsuccessfully** (e.g. an instantly-dying `exit 127`)
/// did not confirm the unlock, so its exit is logged at error level and no
/// `Session.Unlock` is issued — a failed spawn must never look like a clean
/// lock cycle.
fn locker_waiter(state: Arc<Mutex<FlowState>>, logind: SharedLogind, generation: u64) {
    /// What one polling iteration observed under the state lock.
    enum Wait {
        /// Still running.
        Running,
        /// Exited with a status this waiter owns.
        Exited(ExitStatus),
        /// The wait itself failed; the child (taken for reaping) is carried.
        Unknown(Child),
        /// A newer locker replaced this one, or an external unlock took it.
        Replaced,
    }
    loop {
        let outcome = {
            let mut guard = state.lock().unwrap_or_else(|e| e.into_inner());
            match guard.locker.as_mut() {
                Some(slot) if slot.generation == generation => match slot.child.try_wait() {
                    Ok(Some(status)) => {
                        guard.locker = None;
                        Wait::Exited(status)
                    }
                    Ok(None) => Wait::Running,
                    Err(err) => {
                        tracing::warn!(%err, "could not wait on the locker; reaping it");
                        // Take the child so it can be reaped (no zombie), but
                        // its exit status is unknown: not a confirmed cycle.
                        match guard.locker.take().map(|slot| slot.child) {
                            Some(child) => Wait::Unknown(child),
                            None => Wait::Replaced,
                        }
                    }
                },
                _ => Wait::Replaced,
            }
        };
        match outcome {
            Wait::Running => std::thread::sleep(LOCKER_WAIT_POLL),
            Wait::Replaced => return,
            Wait::Unknown(mut child) => {
                let _ = child.wait();
                tracing::error!(
                    generation,
                    "locker exit status unknown; not treating this as a clean unlock"
                );
                return;
            }
            Wait::Exited(status) => {
                if status.success() {
                    finish_locker(&state, &*logind, generation);
                } else {
                    tracing::error!(
                        generation,
                        code = status.code(),
                        "locker exited without confirming the unlock; leaving the session locked"
                    );
                }
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::os::fd::OwnedFd;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use zbus::zvariant::OwnedObjectPath;

    use super::*;
    use crate::logind::{InhibitMode, LOGIND_CALL_TIMEOUT, LogindCall, RecordingLogind};
    use crate::wm_client::{COMPOSITOR_CALL_TIMEOUT, RecordingWm, WmCall, WmClient};

    fn test_flow(logind: Arc<RecordingLogind>) -> LockFlow {
        LockFlow::with_timing(
            logind,
            Arc::new(RecordingWm::new(false)),
            Some("sleep 30".to_string()),
            true,
            Duration::from_millis(10),
            Duration::from_millis(50),
        )
    }

    /// A [`Logind`] double wrapping a [`RecordingLogind`] that can be told to
    /// fail selected calls, so the flow's failure paths are exercisable.
    struct FaultyLogind {
        inner: RecordingLogind,
        delay_inhibit_failures: AtomicUsize,
        unlock_fails: AtomicBool,
    }

    impl FaultyLogind {
        fn new(inner: RecordingLogind) -> Self {
            Self {
                inner,
                delay_inhibit_failures: AtomicUsize::new(0),
                unlock_fails: AtomicBool::new(false),
            }
        }

        /// Refuse the next `n` `delay`-mode inhibits, then succeed.
        fn fail_delay_inhibits(&self, n: usize) {
            self.delay_inhibit_failures.store(n, Ordering::SeqCst);
        }

        fn fail_unlock(&self) {
            self.unlock_fails.store(true, Ordering::SeqCst);
        }

        fn calls(&self) -> Vec<LogindCall> {
            self.inner.calls()
        }
    }

    impl Logind for FaultyLogind {
        fn session_path(&self) -> OwnedObjectPath {
            self.inner.session_path()
        }

        fn inhibit(&self, what: &str, why: &str, mode: InhibitMode) -> io::Result<OwnedFd> {
            if mode == InhibitMode::Delay && self.delay_inhibit_failures.load(Ordering::SeqCst) > 0
            {
                self.delay_inhibit_failures.fetch_sub(1, Ordering::SeqCst);
                // Record the attempt (and drop the fd) so the call is visible.
                let _ = self.inner.inhibit(what, why, mode)?;
                return Err(io::Error::other("inhibit refused"));
            }
            self.inner.inhibit(what, why, mode)
        }

        fn lock(&self) -> io::Result<()> {
            self.inner.lock()
        }

        fn unlock(&self) -> io::Result<()> {
            self.inner.unlock()?;
            if self.unlock_fails.load(Ordering::SeqCst) {
                return Err(io::Error::other("unlock refused"));
            }
            Ok(())
        }

        fn suspend(&self) -> io::Result<()> {
            self.inner.suspend()
        }

        fn hibernate(&self) -> io::Result<()> {
            self.inner.hibernate()
        }

        fn power_off(&self) -> io::Result<()> {
            self.inner.power_off()
        }

        fn reboot(&self) -> io::Result<()> {
            self.inner.reboot()
        }
    }

    /// A [`WmClient`] whose `is_locked` blocks both concurrent callers on a
    /// barrier, so a double-spawn race deterministically passes the "is a
    /// locker running?" read in both threads before either commits.
    struct BarrierWm {
        barrier: Arc<std::sync::Barrier>,
    }

    impl WmClient for BarrierWm {
        fn is_locked(&self) -> bool {
            self.barrier.wait();
            false
        }

        fn quit(&self) -> bool {
            false
        }
    }

    /// A [`WmClient`] whose tri-state read is permanently `Unreachable`, so the
    /// pre-sleep wait's distinction between "cannot ask" and "answered
    /// unlocked" is exercisable.
    struct UnreachableWm;

    impl WmClient for UnreachableWm {
        fn is_locked(&self) -> bool {
            false
        }

        fn lock_state(&self) -> LockState {
            LockState::Unreachable
        }

        fn quit(&self) -> bool {
            false
        }
    }

    fn inhibit_count(calls: &[LogindCall], mode: InhibitMode) -> usize {
        calls
            .iter()
            .filter(|call| matches!(call, LogindCall::Inhibit { mode: m, .. } if *m == mode))
            .count()
    }

    fn wait_until(mut done: impl FnMut() -> bool, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if done() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        done()
    }

    fn pid_alive(pid: u32) -> bool {
        std::path::Path::new(&format!("/proc/{pid}")).exists()
    }

    fn sleeping_child() -> Child {
        Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn a stub locker")
    }

    /// The A-exit → B-spawn → A-unlock race: A's waiter observes A's exit and
    /// clears its slot, a new trigger spawns B, and only then does A's waiter
    /// reach its post-exit step. The stale generation must not issue an
    /// `Unlock`, so B survives.
    #[test]
    fn a_stale_waiter_does_not_unlock_over_a_newer_locker() {
        let logind = Arc::new(RecordingLogind::new("/org/freedesktop/login1/session/c1"));
        let flow = test_flow(logind.clone());

        // A is installed as generation 1.
        {
            let mut state = flow.state();
            state.active_generation = 1;
            state.locker = Some(LockerSlot {
                generation: 1,
                child: sleeping_child(),
            });
        }

        // A's waiter observed A's exit and cleared the slot; a new trigger
        // then spawned B as generation 2.
        let mut finished_a = {
            let mut state = flow.state();
            let finished = state.locker.take();
            state.active_generation = 2;
            state.locker = Some(LockerSlot {
                generation: 2,
                child: sleeping_child(),
            });
            finished
        };

        // A's waiter now reaps A and runs its post-exit step for generation 1.
        if let Some(slot) = finished_a.as_mut() {
            let _ = slot.child.kill();
            let _ = slot.child.wait();
        }
        let unlocked = finish_locker(&flow.state, &*logind, 1);

        assert!(!unlocked, "the stale generation must not unlock");
        assert!(
            !logind.calls().contains(&LogindCall::Unlock),
            "no stale Unlock reached logind: {:?}",
            logind.calls()
        );
        assert!(flow.locker_running(), "B survived A's stale unlock");

        flow.on_unlock(); // cleanup: kill and reap B
    }

    /// The current generation does unlock (the normal locker-exit path).
    #[test]
    fn the_active_waiter_unlocks() {
        let logind = Arc::new(RecordingLogind::new("/org/freedesktop/login1/session/c1"));
        let flow = test_flow(logind.clone());
        {
            let mut state = flow.state();
            state.active_generation = 7;
        }

        assert!(finish_locker(&flow.state, &*logind, 7));
        assert!(logind.calls().contains(&LogindCall::Unlock));
    }

    /// The waiter-spawn-failure teardown for the *current* generation kills,
    /// reaps, and unlocks the locker.
    #[test]
    fn abandon_locker_tears_down_and_unlocks_the_current_locker() {
        let logind = Arc::new(RecordingLogind::new("/org/freedesktop/login1/session/c1"));
        let flow = test_flow(logind.clone());
        {
            let mut state = flow.state();
            state.active_generation = 3;
            state.locker = Some(LockerSlot {
                generation: 3,
                child: sleeping_child(),
            });
        }

        flow.abandon_locker(3);

        assert!(!flow.locker_running());
        assert!(logind.calls().contains(&LogindCall::Unlock));
    }

    /// A stale teardown (its generation was superseded) must not touch the
    /// newer locker, and must not issue an `Unlock` that could kill it.
    #[test]
    fn abandon_locker_does_not_tear_down_a_newer_locker() {
        let logind = Arc::new(RecordingLogind::new("/org/freedesktop/login1/session/c1"));
        let flow = test_flow(logind.clone());
        {
            let mut state = flow.state();
            state.active_generation = 2;
            state.locker = Some(LockerSlot {
                generation: 2,
                child: sleeping_child(),
            });
        }

        flow.abandon_locker(1);

        assert!(flow.locker_running(), "the newer locker survived");
        assert!(!logind.calls().contains(&LogindCall::Unlock));

        flow.on_unlock();
    }

    /// The pre-sleep wait is `SLEEP_LOCK_BUDGET` plus at most one bounded
    /// `IsLocked` call, and is preceded by at most one bounded `Session.Lock`
    /// call; keep the sum under logind's 5s `InhibitDelayMaxSec`.
    #[test]
    fn the_bounded_calls_keep_the_sleep_wait_below_loginds_ceiling() {
        assert!(
            LOGIND_CALL_TIMEOUT + SLEEP_LOCK_BUDGET + COMPOSITOR_CALL_TIMEOUT
                < Duration::from_secs(5),
            "Session.Lock + the pre-sleep wait + one in-flight IsLocked must stay under 5s"
        );
    }

    /// A locker already installed must make [`LockFlow::spawn_locker`] stand
    /// down without bumping the generation or touching the winner. This is the
    /// deterministic half of the double-spawn fix; the concurrent half is
    /// `concurrent_triggers_commit_only_one_locker` below.
    #[test]
    fn spawn_locker_refuses_when_one_is_already_installed() {
        let logind = Arc::new(RecordingLogind::new("/org/freedesktop/login1/session/c1"));
        let flow = test_flow(logind);
        let original = {
            let mut state = flow.state();
            state.active_generation = 1;
            state.locker = Some(LockerSlot {
                generation: 1,
                child: sleeping_child(),
            });
            state.locker.as_ref().expect("installed").child.id()
        };

        assert_eq!(
            flow.spawn_locker(),
            LockAttempt::AlreadyRunning,
            "a second spawn must lose the race"
        );
        let state = flow.state();
        assert_eq!(
            state.active_generation, 1,
            "the loser must not bump the generation"
        );
        assert_eq!(
            state
                .locker
                .as_ref()
                .expect("winner still installed")
                .child
                .id(),
            original,
            "the winner is untouched"
        );
        drop(state);

        flow.on_unlock();
        assert!(!flow.locker_running());
    }

    /// The daemon's own `Session.Unlock` echo, with no newer locker, must not
    /// kill the locker: the generation marker absorbs exactly the echo.
    #[test]
    fn our_own_unlock_echo_with_no_newer_locker_does_not_kill() {
        let logind = Arc::new(RecordingLogind::new("/org/freedesktop/login1/session/c1"));
        let flow = test_flow(logind);
        {
            let mut state = flow.state();
            state.active_generation = 5;
            state.locker = Some(LockerSlot {
                generation: 5,
                child: sleeping_child(),
            });
            // `finish_locker` already issued an Unlock whose echo is in flight.
            push_self_unlock(&mut state, 5);
        }

        flow.on_unlock();
        assert!(
            flow.locker_running(),
            "the echo of our own unlock must not kill the locker"
        );
        assert!(
            flow.state().self_unlock_gens.is_empty(),
            "the echo consumed the marker"
        );

        // With no marker, an Unlock is genuine and kills the locker.
        flow.on_unlock();
        assert!(!flow.locker_running());
    }

    /// A locker that spawned *after* our `Session.Unlock` was issued (its
    /// generation is greater than the marker's) must survive the echo; the
    /// marker is cleared so it cannot absorb a later external unlock.
    #[test]
    fn a_newer_locker_survives_our_own_unlock_echo() {
        let logind = Arc::new(RecordingLogind::new("/org/freedesktop/login1/session/c1"));
        let flow = test_flow(logind);
        {
            let mut state = flow.state();
            state.active_generation = 3;
            state.locker = Some(LockerSlot {
                generation: 3,
                child: sleeping_child(),
            });
            // The unlock was issued for generation 2; locker 3 is newer.
            push_self_unlock(&mut state, 2);
        }

        flow.on_unlock();
        assert!(
            flow.locker_running(),
            "a newer locker must survive our earlier unlock's echo"
        );
        assert!(
            flow.state().self_unlock_gens.is_empty(),
            "the echo cleared the marker"
        );

        // Cleanup: now an external unlock kills the surviving locker.
        flow.on_unlock();
        assert!(!flow.locker_running());
    }

    /// Two lockers exiting back-to-back queue two self-unlock echoes. The first
    /// echo must consume only the first marker: the old single-slot code let it
    /// eat the second's marker, after which the second echo looked external and
    /// killed the unrelated newer locker.
    #[test]
    fn two_in_flight_self_unlocks_are_consumed_in_order_without_killing_the_newer_locker() {
        let logind = Arc::new(RecordingLogind::new("/org/freedesktop/login1/session/c1"));
        let flow = test_flow(logind);
        {
            let mut state = flow.state();
            state.active_generation = 3;
            state.locker = Some(LockerSlot {
                generation: 3,
                child: sleeping_child(),
            });
            // Unlocks for generations 1 and 2 were issued; both echoes are in
            // flight while newer locker 3 is running.
            push_self_unlock(&mut state, 1);
            push_self_unlock(&mut state, 2);
        }

        // First echo: consumes marker 1, leaves marker 2, kills nothing.
        flow.on_unlock();
        assert!(
            flow.locker_running(),
            "the first echo must not kill locker 3"
        );
        assert_eq!(
            flow.state()
                .self_unlock_gens
                .iter()
                .map(|marker| marker.generation)
                .collect::<Vec<_>>(),
            vec![2],
            "the first echo consumed only the first marker"
        );

        // Second echo: consumes marker 2, kills nothing.
        flow.on_unlock();
        assert!(
            flow.locker_running(),
            "the second echo must not kill locker 3"
        );
        assert!(
            flow.state().self_unlock_gens.is_empty(),
            "both markers were consumed"
        );

        // No markers remain: an unlock is external and kills locker 3.
        flow.on_unlock();
        assert!(!flow.locker_running());
    }

    /// An `Unlock` with no marker is external and kills the current locker.
    #[test]
    fn an_external_unlock_with_no_marker_kills_the_locker() {
        let logind = Arc::new(RecordingLogind::new("/org/freedesktop/login1/session/c1"));
        let flow = test_flow(logind);
        {
            let mut state = flow.state();
            state.active_generation = 9;
            state.locker = Some(LockerSlot {
                generation: 9,
                child: sleeping_child(),
            });
            assert!(state.self_unlock_gens.is_empty());
        }

        flow.on_unlock();
        assert!(
            !flow.locker_running(),
            "an external unlock kills the running locker"
        );
    }

    /// A successful post-exit unlock records the exiting generation, so its
    /// echo can be told apart from an external unlock.
    #[test]
    fn finishing_a_locker_records_the_exiting_generation() {
        let logind = Arc::new(RecordingLogind::new("/org/freedesktop/login1/session/c1"));
        let flow = test_flow(logind.clone());
        {
            let mut state = flow.state();
            state.active_generation = 7;
        }

        assert!(finish_locker(&flow.state, &*logind, 7));
        assert!(logind.calls().contains(&LogindCall::Unlock));
        assert_eq!(
            flow.state()
                .self_unlock_gens
                .iter()
                .map(|marker| marker.generation)
                .collect::<Vec<_>>(),
            vec![7]
        );
    }

    /// Two triggers hammering the funnel at once must commit exactly one
    /// locker: the read-decide-commit is atomic, so the loser drops its
    /// duplicate child instead of orphaning it and overwriting the winner.
    /// `active_generation` is bumped only on a commit, so it is the direct
    /// count of committed lockers. The interleaving is made deterministic by a
    /// [`WmClient`] double that barriers both threads past the `is_locked` read
    /// before either reaches the spawn commit; the loser's child is observed
    /// reaped (no surviving duplicate process).
    #[test]
    fn concurrent_triggers_commit_only_one_locker() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pids_file = dir.path().join("pids");
        let logind = Arc::new(RecordingLogind::new("/org/freedesktop/login1/session/c1"));
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let flow = Arc::new(LockFlow::with_timing(
            logind.clone(),
            Arc::new(BarrierWm {
                barrier: Arc::clone(&barrier),
            }),
            Some(format!(
                "echo $$ >> '{}'; exec sleep 30",
                pids_file.display()
            )),
            true,
            Duration::from_millis(10),
            Duration::from_millis(50),
        ));
        let weak = Arc::downgrade(&flow);
        logind.set_lock_echo(move || {
            if let Some(flow) = weak.upgrade() {
                flow.on_lock();
            }
        });

        let mut handles = Vec::new();
        for _ in 0..2 {
            let flow = Arc::clone(&flow);
            handles.push(std::thread::spawn(move || flow.on_idle()));
        }
        for handle in handles {
            handle.join().expect("trigger thread");
        }

        {
            let state = flow.state();
            assert_eq!(
                state.active_generation, 1,
                "exactly one trigger committed a locker"
            );
            assert!(state.locker.is_some(), "the committed locker is installed");
        }

        // Exactly one spawned locker may still be alive: the loser must have
        // been killed and reaped, not left as an orphaned duplicate.
        let alive = std::fs::read_to_string(&pids_file)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| line.trim().parse::<u32>().ok())
            .filter(|pid| pid_alive(*pid))
            .count();
        assert_eq!(alive, 1, "exactly one locker survived the race");

        flow.on_unlock();
        assert!(!flow.locker_running());
    }

    /// A locker that exits unsuccessfully (here `exit 127`) did not confirm an
    /// unlock: its exit must not be mirrored into a clean `Session.Unlock`.
    #[test]
    fn an_unsuccessful_locker_exit_does_not_issue_a_clean_unlock() {
        let logind = Arc::new(RecordingLogind::new("/org/freedesktop/login1/session/c1"));
        let flow = LockFlow::with_timing(
            logind.clone(),
            Arc::new(RecordingWm::new(false)),
            Some("exit 127".to_string()),
            true,
            Duration::from_millis(10),
            Duration::from_millis(50),
        );

        flow.on_lock();
        assert!(
            wait_until(|| !flow.locker_running(), Duration::from_secs(2)),
            "the failing locker exited"
        );
        assert!(
            !logind.calls().contains(&LogindCall::Unlock),
            "a failed locker exit must not look like a clean unlock: {:?}",
            logind.calls()
        );
    }

    /// A self-unlock marker whose echo was never delivered must not swallow a
    /// later genuine external unlock: once its lifetime bound passes, the
    /// external unlock still kills the running locker.
    #[test]
    fn an_expired_self_unlock_marker_does_not_mask_an_external_unlock() {
        let logind = Arc::new(RecordingLogind::new("/org/freedesktop/login1/session/c1"));
        let flow = test_flow(logind);
        {
            let mut state = flow.state();
            state.active_generation = 4;
            state.locker = Some(LockerSlot {
                generation: 4,
                child: sleeping_child(),
            });
            // The unlock for generation 4 was issued long ago and its echo was
            // lost; the marker is now past its lifetime bound.
            state.self_unlock_gens.push_back(SelfUnlockMarker {
                generation: 4,
                expires_at: Instant::now() - Duration::from_secs(1),
            });
        }

        flow.on_unlock();
        assert!(
            !flow.locker_running(),
            "an external unlock must not be masked by an expired marker"
        );
    }

    /// A failed `Session.Unlock()` (which produces no echo) must remove its
    /// marker, so it cannot absorb a later external unlock.
    #[test]
    fn a_failed_unlock_removes_its_marker() {
        let logind = Arc::new(FaultyLogind::new(RecordingLogind::new(
            "/org/freedesktop/login1/session/c1",
        )));
        logind.fail_unlock();
        let flow = LockFlow::with_timing(
            logind.clone(),
            Arc::new(RecordingWm::new(false)),
            Some("sleep 30".to_string()),
            true,
            Duration::from_millis(10),
            Duration::from_millis(50),
        );
        flow.state().active_generation = 3;

        assert!(finish_locker(&flow.state, &*logind, 3));
        assert!(logind.calls().contains(&LogindCall::Unlock));
        assert!(
            flow.state().self_unlock_gens.is_empty(),
            "a failed unlock must leave no marker behind"
        );
    }

    /// A missing sleep delay inhibitor at suspend is re-attempted right before
    /// the pre-sleep wait rather than silently leaving lock-before-sleep off.
    #[test]
    fn prepare_for_sleep_reacquires_a_missing_delay_inhibitor() {
        let logind = Arc::new(FaultyLogind::new(RecordingLogind::new(
            "/org/freedesktop/login1/session/c1",
        )));
        // The boot acquisition of the delay inhibitor fails; the power-key
        // block acquire still succeeds.
        logind.fail_delay_inhibits(1);
        let flow = LockFlow::with_timing(
            logind.clone(),
            Arc::new(RecordingWm::new(false)),
            Some("sleep 30".to_string()),
            true,
            Duration::from_millis(10),
            Duration::from_millis(100),
        );
        assert_eq!(
            inhibit_count(&logind.calls(), InhibitMode::Delay),
            1,
            "the boot delay inhibit was attempted and refused"
        );

        flow.on_prepare_for_sleep(true);

        assert_eq!(
            inhibit_count(&logind.calls(), InhibitMode::Delay),
            2,
            "a fresh delay inhibitor was attempted at suspend"
        );

        flow.on_unlock();
    }

    /// An idle timeout with no configured locker is a policy no-op: it must not
    /// even ask logind to lock (spec Decision 2).
    #[test]
    fn idle_with_no_locker_never_requests_a_lock() {
        let logind = Arc::new(RecordingLogind::new("/org/freedesktop/login1/session/c1"));
        let flow = LockFlow::with_timing(
            logind.clone(),
            Arc::new(RecordingWm::new(false)),
            None,
            true,
            Duration::from_millis(10),
            Duration::from_millis(50),
        );

        flow.on_idle();
        assert!(!flow.locker_running(), "nothing was spawned");
        assert!(
            !logind.calls().contains(&LogindCall::Lock),
            "a no-op idle must not call Session.Lock: {:?}",
            logind.calls()
        );
    }

    /// An unreachable compositor is not a slow locker: the tri-state read is
    /// definitive, so the wait stops well inside the budget and releases the
    /// delay inhibitor rather than burning the whole budget and then
    /// misreporting it as "locker did not confirm".
    #[test]
    fn an_unreachable_compositor_releases_without_burning_the_budget() {
        let logind = Arc::new(RecordingLogind::new("/org/freedesktop/login1/session/c1"));
        let flow = LockFlow::with_timing(
            logind.clone(),
            Arc::new(UnreachableWm),
            Some("sleep 30".to_string()),
            true,
            Duration::from_millis(10),
            Duration::from_secs(2),
        );

        let start = Instant::now();
        flow.on_prepare_for_sleep(true);

        assert!(
            wait_until(|| logind.live_inhibitors() == 1, Duration::from_millis(500)),
            "the delay inhibitor was released"
        );
        assert!(
            start.elapsed() < Duration::from_secs(1),
            "an unreachable compositor must not burn the budget: {:?}",
            start.elapsed()
        );

        flow.on_unlock();
    }

    /// The `SessionLockChanged` subscription is the additional authoritative
    /// cross-check: a fired transition to unlocked kills a still-running
    /// locker through the same external-unlock handling logind's `Unlock` gets,
    /// while a transition to locked is a no-op.
    #[test]
    fn a_subscribed_external_unlock_kills_the_running_locker() {
        let logind = Arc::new(RecordingLogind::new("/org/freedesktop/login1/session/c1"));
        let wm = Arc::new(RecordingWm::new(false));
        let flow = Arc::new(LockFlow::with_timing(
            logind,
            wm.clone(),
            Some("sleep 30".to_string()),
            true,
            Duration::from_millis(10),
            Duration::from_millis(50),
        ));
        {
            let mut state = flow.state();
            state.active_generation = 1;
            state.locker = Some(LockerSlot {
                generation: 1,
                child: sleeping_child(),
            });
        }

        let weak = Arc::downgrade(&flow);
        let handle = wm
            .subscribe_lock_changed(Box::new(move |locked| {
                if let Some(flow) = weak.upgrade() {
                    flow.on_session_lock_changed(locked);
                }
            }))
            .expect("the recording double supports subscriptions");
        assert_eq!(wm.calls(), vec![WmCall::SubscribeLockChanged]);

        // A transition to locked changes nothing on this side.
        wm.fire_lock_changed(true);
        assert!(flow.locker_running(), "a lock transition is a no-op");

        // A transition to unlocked is the external-unlock path: the locker dies.
        wm.fire_lock_changed(false);
        assert!(
            !flow.locker_running(),
            "the subscribed external unlock killed the running locker"
        );

        drop(handle);
    }
}
