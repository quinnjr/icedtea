//! Session lifecycle flow: the bounded lock-before-sleep path and the
//! `Lock`/`Unlock` locker funnel.
//!
//! Every effect goes through the [`Logind`] and [`WmClient`] seams, so the
//! whole flow is driven from tests without a bus. The hard invariants live
//! here: the sleep delay inhibitor is released on *every* path (spec Decision
//! 5, via the [`SleepInhibitor`] guard) and the pre-sleep wait never runs
//! meaningfully past [`SLEEP_LOCK_BUDGET`] — the one unbounded step is a single
//! `IsLocked` D-Bus call, itself bounded by
//! [`crate::wm_client::COMPOSITOR_CALL_TIMEOUT`].

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

    /// `PrepareForSleep(bool)`: `true` locks (bounded) then releases sleep,
    /// `false` (resume) re-arms a fresh one-shot delay inhibitor.
    ///
    /// A locker process already running is *not* proof the screen is secured:
    /// an idle-triggered locker may still be starting up. So the wait is
    /// entered whenever the compositor has not *confirmed* the lock, whether
    /// the trigger just spawned the locker or found one already running. It is
    /// only skipped when the compositor already reports locked, or when policy
    /// refused to lock at all.
    pub fn on_prepare_for_sleep(&self, going_to_sleep: bool) {
        if !going_to_sleep {
            self.acquire_sleep();
            return;
        }
        match self.request_lock(LockReason::PrepareForSleep) {
            LockAttempt::Spawned | LockAttempt::AlreadyRunning => {
                self.wait_until_locked();
            }
            LockAttempt::Locked | LockAttempt::Refused => {}
        }
        // Release on every path: confirmed, budget exceeded, or a policy no-op.
        self.release_sleep();
    }

    /// An idle timeout: same lock path, no sleep inhibitor to release.
    pub fn on_idle(&self) {
        let _ = self.request_lock(LockReason::Idle);
    }

    /// An explicit `Session.Lock` (an external lock, or our own `Lock()`
    /// call): the generic "lock now" branch, never gated by
    /// `lock_before_sleep`.
    pub fn on_lock(&self) {
        let _ = self.request_lock(LockReason::Idle);
    }

    /// An external `Session.Unlock`: kill a still-running locker so the screen
    /// is not left locked with a stale process. The Wayland unlock already
    /// happened, so this does not round-trip another `Session.Unlock`.
    pub fn on_unlock(&self) {
        let slot = self.state().locker.take();
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
            Decision::SpawnLocker => {
                if self.spawn_locker() {
                    LockAttempt::Spawned
                } else {
                    LockAttempt::Refused
                }
            }
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
    fn spawn_locker(&self) -> bool {
        let Some(command) = self.locker_command.as_deref() else {
            tracing::warn!("SpawnLocker with no locker_command; refusing to lock");
            return false;
        };
        // Same invocation shape as the compositor's `"spawn"` arm: a shell so
        // the command string can carry its own arguments.
        match Command::new("sh").arg("-c").arg(command).spawn() {
            Ok(child) => {
                let generation = {
                    let mut state = self.state();
                    let generation = state.active_generation.wrapping_add(1);
                    state.active_generation = generation;
                    state.locker = Some(LockerSlot { generation, child });
                    generation
                };
                self.spawn_waiter(generation);
                tracing::info!(command, "spawned the locker");
                true
            }
            Err(err) => {
                tracing::warn!(%err, command, "could not spawn the locker");
                false
            }
        }
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
    fn abandon_locker(&self, generation: u64) {
        let slot = {
            let mut guard = self.state();
            match guard.locker.as_ref() {
                Some(slot) if slot.generation == generation => guard.locker.take(),
                _ => None,
            }
        };
        if let Some(mut slot) = slot {
            let _ = slot.child.kill();
            let _ = slot.child.wait();
            if let Err(err) = self.logind.unlock() {
                tracing::warn!(%err, "Session.Unlock after locker-waiter spawn failure failed");
            }
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
/// `generation` is still the most recent spawn. The state lock is held across
/// the call so a new spawn cannot install itself between the check and the
/// unlock: without that, a locker that exited just before a new one spawned
/// would unlock — and the compositor's `Unlock` signal would then kill the
/// innocent new locker.
fn finish_locker(
    state: &Mutex<FlowState>,
    logind: &(dyn Logind + Send + Sync),
    generation: u64,
) -> bool {
    let guard = state.lock().unwrap_or_else(|e| e.into_inner());
    if guard.active_generation != generation {
        return false;
    }
    if let Err(err) = logind.unlock() {
        tracing::warn!(%err, "Session.Unlock after locker exit failed");
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
    use crate::logind::{LogindCall, RecordingLogind};
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
        Command::new("sh")
            .arg("-c")
            .arg("sleep 30")
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

    /// The pre-sleep wait is `SLEEP_LOCK_BUDGET` plus at most one bounded
    /// `IsLocked` call; keep the sum under logind's 5s `InhibitDelayMaxSec`.
    #[test]
    fn the_is_locked_call_timeout_keeps_the_sleep_wait_below_loginds_ceiling() {
        assert!(
            SLEEP_LOCK_BUDGET + COMPOSITOR_CALL_TIMEOUT < Duration::from_secs(5),
            "the pre-sleep wait may overshoot by one in-flight call"
        );
    }
}
