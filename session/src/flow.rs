//! Session lifecycle flow: the bounded lock-before-sleep path and the
//! `Lock`/`Unlock` locker funnel.
//!
//! Every effect goes through the [`Logind`] and [`WmClient`] seams, so the
//! whole flow is driven from tests without a bus. The two hard invariants live
//! here: the sleep delay inhibitor is released on *every* path (spec Decision
//! 5, via the [`SleepInhibitor`] guard) and the pre-sleep wait never runs past
//! [`SLEEP_LOCK_BUDGET`].

use std::process::{Child, Command};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use icedtea_config::Power;

use crate::logind::{LogindEvents, PowerKeyInhibitor, SleepInhibitor};
use crate::policy::{self, Decision, LockReason};
use crate::service::{SharedLogind, SharedWm};

/// How often the bounded pre-sleep wait re-checks `IsLocked()`.
pub const SLEEP_POLL_INTERVAL: Duration = Duration::from_millis(100);
/// Hard ceiling on the pre-sleep wait (spec Decision 5) — comfortably inside
/// logind's 5s default `InhibitDelayMaxSec`.
pub const SLEEP_LOCK_BUDGET: Duration = Duration::from_secs(3);
/// How often the locker-waiter thread checks whether the locker exited.
const LOCKER_WAIT_POLL: Duration = Duration::from_millis(20);

/// The mutable state shared with the locker-waiter thread.
struct FlowState {
    /// The one live sleep delay inhibitor, if any.
    sleep: Option<SleepInhibitor>,
    /// The daemon-lifetime power-key block inhibitor.
    power_key: Option<PowerKeyInhibitor>,
    /// The running locker child, if one is up.
    locker: Option<Child>,
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
    pub fn on_prepare_for_sleep(&self, going_to_sleep: bool) {
        if !going_to_sleep {
            self.acquire_sleep();
            return;
        }
        if self.request_lock(LockReason::PrepareForSleep) {
            self.wait_until_locked();
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
        let child = self.state().locker.take();
        if let Some(mut child) = child {
            tracing::info!("external unlock; terminating the running locker");
            if let Err(err) = child.kill() {
                tracing::warn!(%err, "could not kill the locker on external unlock");
            }
            let _ = child.wait();
        }
    }

    /// Whether a locker child is currently running.
    pub fn locker_running(&self) -> bool {
        self.state().locker.is_some()
    }

    /// Re-read the lock state *immediately* before deciding, per the caller
    /// obligation in [`crate::policy`]. A locker already running counts as
    /// locked even if the compositor has not published the lock yet, so a
    /// second near-simultaneous trigger (idle and `PrepareForSleep` landing
    /// together) cannot double-spawn.
    fn request_lock(&self, reason: LockReason) -> bool {
        let locked = self.wm.is_locked() || self.locker_running();
        match policy::decide(
            reason,
            locked,
            self.locker_command.is_some(),
            self.lock_before_sleep,
        ) {
            Decision::SpawnLocker => self.spawn_locker(),
            Decision::NoOp(why) => {
                tracing::warn!(reason = ?reason, why, "not locking");
                false
            }
        }
    }

    /// Spawn the configured locker and start the waiter that reaps it and
    /// mirrors its exit into `Session.Unlock`.
    fn spawn_locker(&self) -> bool {
        let Some(command) = self.locker_command.as_deref() else {
            tracing::warn!("SpawnLocker with no locker_command; refusing to lock");
            return false;
        };
        // Same invocation shape as the compositor's `"spawn"` arm: a shell so
        // the command string can carry its own arguments.
        match Command::new("sh").arg("-c").arg(command).spawn() {
            Ok(child) => {
                self.state().locker = Some(child);
                self.spawn_waiter();
                tracing::info!(command, "spawned the locker");
                true
            }
            Err(err) => {
                tracing::warn!(%err, command, "could not spawn the locker");
                false
            }
        }
    }

    fn spawn_waiter(&self) {
        let state = Arc::clone(&self.state);
        let logind = Arc::clone(&self.logind);
        let waiter = std::thread::Builder::new()
            .name("icedtea-session-locker".to_string())
            .spawn(move || locker_waiter(state, logind));
        if let Err(err) = waiter {
            tracing::warn!(%err, "could not spawn the locker waiter thread");
        }
    }

    /// Wait for the lock to be confirmed by the compositor, at most
    /// [`Self::budget`]. Returns whether it was confirmed; the caller releases
    /// the inhibitor either way.
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

/// Poll `try_wait` until the locker exits (or an external unlock took it),
/// then mirror a natural exit into `Session.Unlock`. A locker is trusted to
/// exit only after a successful unlock, exactly as `swaylock`/`gtklock` do.
fn locker_waiter(state: Arc<Mutex<FlowState>>, logind: SharedLogind) {
    loop {
        let exited = {
            let mut guard = state.lock().unwrap_or_else(|e| e.into_inner());
            match guard.locker.as_mut() {
                Some(child) => match child.try_wait() {
                    Ok(Some(_status)) => {
                        guard.locker = None;
                        true
                    }
                    Ok(None) => false,
                    Err(err) => {
                        tracing::warn!(%err, "could not wait on the locker");
                        guard.locker = None;
                        true
                    }
                },
                // Taken by an external unlock; that path owns the cleanup.
                None => return,
            }
        };
        if exited {
            if let Err(err) = logind.unlock() {
                tracing::warn!(%err, "Session.Unlock after locker exit failed");
            }
            return;
        }
        std::thread::sleep(LOCKER_WAIT_POLL);
    }
}
