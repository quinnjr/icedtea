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
//! `Session.Unlock` this daemon issued is recognized by its echo so it never
//! kills a locker that spawned after it.

use std::process::{Child, Command};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use icedtea_config::Power;

use crate::logind::{Logind, LogindEvents, PowerKeyInhibitor, SleepInhibitor};
use crate::policy::{self, Decision, LockReason};
use crate::service::{SharedLogind, SharedWm};

/// How often the bounded pre-sleep wait re-checks `IsLocked()`.
pub const SLEEP_POLL_INTERVAL: Duration = Duration::from_millis(100);
/// Hard ceiling on the pre-sleep wait (spec Decision 5) — comfortably inside
/// logind's 5s default `InhibitDelayMaxSec`. A single in-flight `IsLocked` call
/// may overshoot this by up to [`COMPOSITOR_CALL_TIMEOUT`]; the two together
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
    /// How many `Session.Unlock()` calls this daemon has issued whose logind
    /// `Unlock` echo is still in flight. [`LockFlow::on_unlock`] consumes one
    /// per echo, so the daemon's own unlock is never mistaken for a genuine
    /// external unlock and does not kill a locker that spawned in between.
    pending_self_unlocks: u64,
}

/// What [`LockFlow::request_lock`] did, so the sleep path knows whether a
/// confirmation still has to be awaited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LockAttempt {
    /// A new locker was spawned; the lock is not confirmed yet.
    Spawned,
    /// An earlier locker is still running, but the compositor has not
    /// confirmed the lock yet.
    AlreadyRunning,
    /// The compositor already reports locked.
    Locked,
    /// Policy refused (a no-op); nothing will lock this cycle.
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
                pending_self_unlocks: 0,
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
    /// funnel (bounded wait for confirmation) then releases sleep, `false`
    /// (resume) re-arms a fresh one-shot delay inhibitor.
    ///
    /// A locker process already running is *not* proof the screen is secured:
    /// an idle-triggered locker may still be starting up. So, when
    /// `lock_before_sleep` is enabled, the wait runs whenever logind accepts
    /// the lock request, whether the trigger just spawned the locker or found
    /// one already running. The request itself is gated (configuration,
    /// `lock_before_sleep`) by [`Self::request_logind_lock`]; the already-locked
    /// case falls out of the wait's first poll (immediate confirmation), so no
    /// extra `IsLocked` round-trip precedes it and the bounded total stays
    /// under logind's ceiling.
    pub fn on_prepare_for_sleep(&self, going_to_sleep: bool) {
        if !going_to_sleep {
            self.acquire_sleep();
            return;
        }
        // A confirmed lock needs no wait. With `lock_before_sleep` disabled the
        // user setting wins outright: even a running-but-unconfirmed locker
        // must not delay sleep (the request is refused below, so no wait).
        if self.request_logind_lock(LockReason::PrepareForSleep) {
            self.wait_until_locked();
        }
        // Release on every path: confirmed, budget exceeded, or a policy no-op.
        self.release_sleep();
    }

    /// An idle timeout: ask logind to lock this session, which emits the `Lock`
    /// signal the spawn funnel handles (spec Decision 7). No sleep inhibitor is
    /// involved, so there is nothing to release.
    pub fn on_idle(&self) {
        let _ = self.request_logind_lock(LockReason::Idle);
    }

    /// An explicit `Session.Lock` (an external lock, or logind's echo of our
    /// own request): the funnel that actually spawns the locker.
    pub fn on_lock(&self) {
        let _ = self.request_lock(LockReason::Idle);
    }

    /// An `Unlock` signal from logind: either the echo of our own
    /// `Session.Unlock()` (ignored — see `pending_self_unlocks`) or a genuine
    /// external unlock (e.g. `loginctl unlock-session`), in which case a
    /// still-running locker is killed so the screen is not left locked with a
    /// stale process. The Wayland unlock already happened, so this does not
    /// round-trip another `Session.Unlock`.
    pub fn on_unlock(&self) {
        let mut guard = self.state();
        if guard.pending_self_unlocks > 0 {
            guard.pending_self_unlocks -= 1;
            tracing::debug!("ignoring the echo of our own Session.Unlock");
            return;
        }
        let slot = guard.locker.take();
        drop(guard);
        if let Some(mut slot) = slot {
            tracing::info!("external unlock; terminating the running locker");
            if let Err(err) = slot.child.kill() {
                tracing::warn!(%err, "could not kill the locker on external unlock");
            }
            let _ = slot.child.wait();
        }
    }

    /// Whether a locker child is currently running.
    pub fn locker_running(&self) -> bool {
        self.state().locker.is_some()
    }

    /// Ask logind to lock this session — the request half of the spec's single
    /// funnel. The actual spawn happens in [`Self::on_lock`] when logind echoes
    /// the `Lock` signal. Returns whether the request was issued, so the
    /// pre-sleep path knows to await confirmation.
    ///
    /// The refuse-without-a-locker rule (Decision 2) and the
    /// `lock_before_sleep` gate live here: calling `Session.Lock()` with no
    /// locker to run would leave logind's bookkeeping claiming a locked session
    /// that nothing painted. `policy::decide` is called with `locked = false`
    /// on purpose — the current lock state is the funnel's to re-query, so an
    /// idle timeout does not spend a redundant `IsLocked` round-trip.
    fn request_logind_lock(&self, reason: LockReason) -> bool {
        match policy::decide(
            reason,
            false,
            self.locker_command.is_some(),
            self.lock_before_sleep,
        ) {
            Decision::SpawnLocker => match self.logind.lock() {
                Ok(()) => true,
                Err(err) => {
                    tracing::warn!(%err, reason = ?reason, "Session.Lock call failed");
                    false
                }
            },
            Decision::NoOp(why) => {
                tracing::warn!(reason = ?reason, why, "not locking");
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
    /// than orphaning it) and reports [`LockAttempt::AlreadyRunning`] so the
    /// pre-sleep path still awaits the winner's confirmation.
    fn spawn_locker(&self) -> LockAttempt {
        let Some(command) = self.locker_command.as_deref() else {
            tracing::warn!("SpawnLocker with no locker_command; refusing to lock");
            return LockAttempt::Refused;
        };
        // Same invocation shape as the compositor's `"spawn"` arm: a shell so
        // the command string can carry its own arguments.
        let mut child = match Command::new("sh").arg("-c").arg(command).spawn() {
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
                let _ = child.kill();
                let _ = child.wait();
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
            let slot = guard.locker.take();
            if slot.is_some() {
                guard.pending_self_unlocks = guard.pending_self_unlocks.saturating_add(1);
            }
            slot
        };
        let Some(mut slot) = slot else {
            return;
        };
        let _ = slot.child.kill();
        let _ = slot.child.wait();
        if let Err(err) = self.logind.unlock() {
            tracing::warn!(%err, "Session.Unlock after locker-waiter spawn failure failed");
            // A failed call produces no echo; do not swallow a later external
            // unlock with an unconsumed token.
            let mut state = self.state();
            state.pending_self_unlocks = state.pending_self_unlocks.saturating_sub(1);
        }
    }

    /// Wait for the lock to be confirmed by the compositor, at most
    /// [`Self::budget`] (plus at most one in-flight bounded `IsLocked` call).
    /// Returns whether it was confirmed; the caller releases the inhibitor
    /// either way.
    fn wait_until_locked(&self) -> bool {
        let deadline = Instant::now() + self.budget;
        loop {
            if self.wm.is_locked() {
                return true;
            }
            let now = Instant::now();
            if now >= deadline {
                tracing::warn!(
                    budget_ms = self.budget.as_millis() as u64,
                    "locker did not confirm the lock within the budget; releasing sleep"
                );
                return false;
            }
            std::thread::sleep(self.poll_interval.min(deadline - now));
        }
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

/// Issue the post-exit `Session.Unlock` for a waiter, but only while its
/// `generation` is still the most recent spawn. The generation check and the
/// `pending_self_unlocks` increment happen under the state lock so a new spawn
/// cannot install itself between the check and the commit; the blocking logind
/// call itself is made outside the lock, so a stalled system bus cannot block
/// the sleep-inhibitor release or any other flow path. The incremented token
/// marks the resulting `Unlock` echo as ours, so [`LockFlow::on_unlock`] does
/// not mistake it for an external unlock and kills an innocent newer locker.
fn finish_locker(
    state: &Mutex<FlowState>,
    logind: &(dyn Logind + Send + Sync),
    generation: u64,
) -> bool {
    {
        let mut guard = state.lock().unwrap_or_else(|e| e.into_inner());
        if guard.active_generation != generation {
            return false;
        }
        guard.pending_self_unlocks = guard.pending_self_unlocks.saturating_add(1);
    }
    if let Err(err) = logind.unlock() {
        tracing::warn!(%err, "Session.Unlock after locker exit failed");
        // A failed call produces no echo; do not swallow a later external
        // unlock with an unconsumed token.
        let mut guard = state.lock().unwrap_or_else(|e| e.into_inner());
        guard.pending_self_unlocks = guard.pending_self_unlocks.saturating_sub(1);
    }
    true
}

/// Poll `try_wait` until the locker exits (or an external unlock took it),
/// then mirror a natural exit into `Session.Unlock`. A locker is trusted to
/// exit only after a successful unlock, exactly as `swaylock`/`gtklock` do.
fn locker_waiter(state: Arc<Mutex<FlowState>>, logind: SharedLogind, generation: u64) {
    loop {
        let exited = {
            let mut guard = state.lock().unwrap_or_else(|e| e.into_inner());
            match guard.locker.as_mut() {
                Some(slot) if slot.generation == generation => match slot.child.try_wait() {
                    Ok(Some(_status)) => {
                        guard.locker = None;
                        true
                    }
                    Ok(None) => false,
                    Err(err) => {
                        tracing::warn!(%err, "could not wait on the locker; reaping it");
                        // Reap before dropping the handle so the child cannot
                        // become a zombie.
                        if let Some(mut slot) = guard.locker.take() {
                            let _ = slot.child.wait();
                        }
                        true
                    }
                },
                // A newer locker replaced this one, or an external unlock took
                // it: that path owns the cleanup.
                _ => return,
            }
        };
        if exited {
            finish_locker(&state, &*logind, generation);
            return;
        }
        std::thread::sleep(LOCKER_WAIT_POLL);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::logind::{LOGIND_CALL_TIMEOUT, LogindCall, RecordingLogind};
    use crate::wm_client::{COMPOSITOR_CALL_TIMEOUT, RecordingWm};

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

    /// The daemon's own `Session.Unlock` echo must not kill a locker that
    /// spawned after the unlock was issued: the pending token absorbs exactly
    /// one echo, and a following genuine external unlock still kills it.
    #[test]
    fn our_own_unlock_echo_does_not_kill_a_newer_locker() {
        let logind = Arc::new(RecordingLogind::new("/org/freedesktop/login1/session/c1"));
        let flow = test_flow(logind);
        {
            let mut state = flow.state();
            state.active_generation = 2;
            state.locker = Some(LockerSlot {
                generation: 2,
                child: sleeping_child(),
            });
            // `finish_locker` already issued an Unlock whose echo is in flight.
            state.pending_self_unlocks = 1;
        }

        flow.on_unlock();
        assert!(
            flow.locker_running(),
            "the echo of our own unlock must not kill the newer locker"
        );
        assert_eq!(
            flow.state().pending_self_unlocks,
            0,
            "the echo consumed the pending token"
        );

        // With no token pending, an Unlock is genuine and kills the locker.
        flow.on_unlock();
        assert!(!flow.locker_running());
    }

    /// A successful post-exit unlock records exactly one expected self-unlock,
    /// so its echo can be told apart from an external unlock.
    #[test]
    fn finishing_a_locker_records_one_expected_self_unlock() {
        let logind = Arc::new(RecordingLogind::new("/org/freedesktop/login1/session/c1"));
        let flow = test_flow(logind.clone());
        {
            let mut state = flow.state();
            state.active_generation = 7;
        }

        assert!(finish_locker(&flow.state, &*logind, 7));
        assert!(logind.calls().contains(&LogindCall::Unlock));
        assert_eq!(flow.state().pending_self_unlocks, 1);
    }

    /// Two triggers hammering the funnel at once must commit exactly one
    /// locker: the read-decide-commit is atomic, so the loser drops its
    /// duplicate child instead of orphaning it and overwriting the winner.
    /// `active_generation` is bumped only on a commit, so it is the direct
    /// count of committed lockers — the pre-fix code would reach 2 here.
    #[test]
    fn concurrent_triggers_commit_only_one_locker() {
        let logind = Arc::new(RecordingLogind::new("/org/freedesktop/login1/session/c1"));
        let flow = Arc::new(test_flow(logind.clone()));
        let weak = Arc::downgrade(&flow);
        logind.set_lock_echo(move || {
            if let Some(flow) = weak.upgrade() {
                flow.on_lock();
            }
        });

        let barrier = Arc::new(std::sync::Barrier::new(2));
        let mut handles = Vec::new();
        for _ in 0..2 {
            let flow = Arc::clone(&flow);
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                flow.on_idle();
            }));
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

        flow.on_unlock();
        assert!(!flow.locker_running());
    }
}
