//! Pure lock policy: no D-Bus, no Wayland, no clocks. Given whether the
//! session is already locked, whether a locker is configured, and the
//! lock-before-sleep preference, say whether to spawn the locker.
//!
//! This is the single place the "refuse to lock without a configured locker"
//! rule (Decision 2) lives.
//!
//! **Caller obligation:** [`decide`] is pure and carries no state, so it
//! cannot itself prevent two near-simultaneous triggers (an idle timeout and a
//! `PrepareForSleep` landing together) from both spawning a locker. The wiring
//! must re-query `IsLocked()` (or otherwise pass the current `locked`) *immediately
//! before each* `decide` call; then the second trigger observes `locked == true`
//! and no-ops. The no-double-spawn property is enforced by that caller-side
//! re-query *and* by the flow's atomic spawn commit (the loser of the race
//! stands down), not by `decide` remembering anything. The request half of the
//! funnel deliberately passes `locked: false`: it only gates on configuration,
//! and the already-locked check belongs to the funnel's own `decide` call.

/// Why a lock is being considered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockReason {
    /// The seat went idle past the configured timeout.
    Idle,
    /// `logind` announced an imminent suspend/hibernate (`PrepareForSleep`).
    PrepareForSleep,
}

/// What the caller should do about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Spawn the configured locker.
    SpawnLocker,
    /// Do nothing; the string names why, for logging.
    NoOp(&'static str),
}

/// Decide whether to spawn the locker.
///
/// `locked` is the session's current lock state (`IsLocked()`); a session that
/// is already locked is never locked again. `locker_configured` is whether
/// `config::Power::locker_command` is set; locking is refused without one so a
/// screen nothing can unlock is never produced.
///
/// `lock_before_sleep` mirrors `config::Power::lock_before_sleep` and gates
/// lock-before-sleep ([`LockReason::PrepareForSleep`]) **only**; idle-lock is
/// gated by `lock_idle_timeout_ms` alone, checked at the call site, never by
/// this flag.
pub fn decide(
    reason: LockReason,
    locked: bool,
    locker_configured: bool,
    lock_before_sleep: bool,
) -> Decision {
    if !locker_configured {
        return Decision::NoOp("no locker configured");
    }
    if locked {
        return Decision::NoOp("already locked");
    }
    match reason {
        LockReason::Idle => Decision::SpawnLocker,
        LockReason::PrepareForSleep if lock_before_sleep => Decision::SpawnLocker,
        LockReason::PrepareForSleep => Decision::NoOp("lock-before-sleep disabled"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_with_a_locker_and_unlocked_spawns() {
        assert_eq!(
            decide(LockReason::Idle, false, true, false),
            Decision::SpawnLocker
        );
        assert_eq!(
            decide(LockReason::Idle, false, true, true),
            Decision::SpawnLocker
        );
    }

    #[test]
    fn no_locker_configured_is_always_a_noop() {
        for reason in [LockReason::Idle, LockReason::PrepareForSleep] {
            for locked in [false, true] {
                for before in [false, true] {
                    assert_eq!(
                        decide(reason, locked, false, before),
                        Decision::NoOp("no locker configured"),
                        "{reason:?} locked={locked} before={before}"
                    );
                }
            }
        }
    }

    #[test]
    fn already_locked_never_spawns_a_second_locker() {
        for reason in [LockReason::Idle, LockReason::PrepareForSleep] {
            for before in [false, true] {
                assert_eq!(
                    decide(reason, true, true, before),
                    Decision::NoOp("already locked")
                );
            }
        }
    }

    #[test]
    fn prepare_for_sleep_honors_lock_before_sleep() {
        assert_eq!(
            decide(LockReason::PrepareForSleep, false, true, true),
            Decision::SpawnLocker
        );
        assert_eq!(
            decide(LockReason::PrepareForSleep, false, true, false),
            Decision::NoOp("lock-before-sleep disabled")
        );
    }

    #[test]
    fn idle_ignores_the_lock_before_sleep_flag() {
        assert_eq!(
            decide(LockReason::Idle, false, true, false),
            Decision::SpawnLocker
        );
        assert_eq!(
            decide(LockReason::Idle, false, true, true),
            Decision::SpawnLocker
        );
    }

    /// An idle timeout and a `PrepareForSleep` can land close together (falling
    /// asleep right as the idle timer expires). The first spawns; the second
    /// sees `locked` and no-ops, so no second locker process is started.
    #[test]
    fn idle_then_sleep_interleave_spawns_once() {
        assert_eq!(
            decide(LockReason::Idle, false, true, true),
            Decision::SpawnLocker
        );
        assert_eq!(
            decide(LockReason::PrepareForSleep, true, true, true),
            Decision::NoOp("already locked")
        );
    }

    #[test]
    fn sleep_then_idle_interleave_spawns_once() {
        assert_eq!(
            decide(LockReason::PrepareForSleep, false, true, true),
            Decision::SpawnLocker
        );
        assert_eq!(
            decide(LockReason::Idle, true, true, true),
            Decision::NoOp("already locked")
        );
    }

    /// Pin every input combination in one place: a policy edit that changes
    /// any cell of the table fails here by name.
    #[test]
    fn decision_table_is_exhaustive() {
        for reason in [LockReason::Idle, LockReason::PrepareForSleep] {
            for locked in [false, true] {
                for configured in [false, true] {
                    for before in [false, true] {
                        let expected = if !configured {
                            Decision::NoOp("no locker configured")
                        } else if locked {
                            Decision::NoOp("already locked")
                        } else if reason == LockReason::PrepareForSleep && !before {
                            Decision::NoOp("lock-before-sleep disabled")
                        } else {
                            Decision::SpawnLocker
                        };
                        assert_eq!(
                            decide(reason, locked, configured, before),
                            expected,
                            "{reason:?} locked={locked} configured={configured} before={before}"
                        );
                    }
                }
            }
        }
    }
}
