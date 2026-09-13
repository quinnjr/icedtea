//! Lock-before-sleep and the `Lock`/`Unlock` locker funnel, driven against
//! the `RecordingLogind` / `RecordingWm` seams and a stub locker script that
//! signals itself by appending to a marker file. No real D-Bus, no real
//! suspend: every effect the flow has goes through a trait double, so the
//! ordering (confirmation before release) and the bound (never held past the
//! budget) are observed directly.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use icedtea_session::flow::LockFlow;
use icedtea_session::logind::{InhibitMode, LogindCall, RecordingLogind};
use icedtea_session::wm_client::{RecordingWm, WmClient};

/// A [`WmClient`] whose lock state is "the marker file exists". The stub
/// locker only marks itself locked by creating that file, so this double
/// reports locked exactly when a real locker would have taken the lock, and
/// records whether the flow ever observed that confirmation.
struct MarkerWm {
    marker: PathBuf,
    calls: AtomicUsize,
    observed_locked: AtomicBool,
}

impl MarkerWm {
    fn new(marker: &Path) -> Self {
        Self {
            marker: marker.to_path_buf(),
            calls: AtomicUsize::new(0),
            observed_locked: AtomicBool::new(false),
        }
    }
}

impl WmClient for MarkerWm {
    fn is_locked(&self) -> bool {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let locked = self.marker.exists();
        if locked {
            self.observed_locked.store(true, Ordering::SeqCst);
        }
        locked
    }

    fn quit(&self) -> bool {
        false
    }
}

/// A `sh -c` locker command that (optionally) waits `delay` seconds, marks
/// itself locked by writing a line to `marker`, then (optionally) lingers for
/// `linger` seconds so it is still running when a later trigger arrives.
///
/// The marker is written to a temp file and renamed into place, so it never
/// exists empty: the tests treat "the marker exists" as "the lock is confirmed",
/// and a bare `printf > marker` redirect would create the file a moment before
/// writing, letting that check observe a zero-line file.
fn locker_command(marker: &Path, delay: f64, linger: f64) -> String {
    let mut command = String::new();
    if delay > 0.0 {
        command.push_str(&format!("sleep {delay}; "));
    }
    let tmp = format!("{}.tmp", marker.display());
    command.push_str(&format!(
        "printf 'locked\\n' > '{tmp}' && mv '{tmp}' '{}'",
        marker.display()
    ));
    if linger > 0.0 {
        command.push_str(&format!("; sleep {linger}"));
    }
    command
}

/// Number of times the stub locker wrote to the marker (one line per spawn).
fn marker_lines(path: &Path) -> usize {
    std::fs::read_to_string(path)
        .map(|text| text.lines().filter(|line| !line.is_empty()).count())
        .unwrap_or(0)
}

fn wait_for_marker(path: &Path, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if path.exists() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    path.exists()
}

fn session_path() -> &'static str {
    "/org/freedesktop/login1/session/c1"
}

fn acquired_inhibitor(logind: &RecordingLogind, mode: InhibitMode) -> bool {
    inhibit_count(logind, mode) > 0
}

fn inhibit_count(logind: &RecordingLogind, mode: InhibitMode) -> usize {
    logind
        .calls()
        .iter()
        .filter(|call| matches!(call, LogindCall::Inhibit { mode: m, .. } if *m == mode))
        .count()
}

fn wait_for_call(logind: &RecordingLogind, call: &LogindCall) -> bool {
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if logind.calls().contains(call) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

/// Build a flow and wire `RecordingLogind`'s `Lock` echo to its funnel.
/// `on_idle`/`on_prepare_for_sleep` only *request* a lock from logind; in
/// production logind answers with the `Lock` signal, and the signal loop calls
/// `on_lock`. This models that, so the tests exercise the single funnel rather
/// than a direct spawn.
fn wired_flow(
    logind: &Arc<RecordingLogind>,
    wm: Arc<dyn WmClient + Send + Sync>,
    command: Option<String>,
    lock_before_sleep: bool,
    poll: Duration,
    budget: Duration,
) -> Arc<LockFlow> {
    let flow = Arc::new(LockFlow::with_timing(
        logind.clone(),
        wm,
        command,
        lock_before_sleep,
        poll,
        budget,
    ));
    let weak = Arc::downgrade(&flow);
    logind.set_lock_echo(move || {
        if let Some(flow) = weak.upgrade() {
            flow.on_lock();
        }
    });
    flow
}

/// `PrepareForSleep(true)` with a configured locker: exactly one locker is
/// spawned, and the flow only returns — releasing the sleep inhibitor, via the
/// `Drop` guard — after the compositor has observed the lock (the marker). The
/// `observed_locked` flag is set inside a poll that only runs before the
/// release, so it proves the waiter was still held when the marker appeared.
#[test]
fn prepare_for_sleep_spawns_one_locker_and_releases_after_the_marker() {
    let dir = tempfile::tempdir().expect("tempdir");
    let marker = dir.path().join("locked");
    let logind = Arc::new(RecordingLogind::new(session_path()));
    let wm = Arc::new(MarkerWm::new(&marker));
    let flow = wired_flow(
        &logind,
        wm.clone(),
        Some(locker_command(&marker, 0.3, 0.0)),
        true,
        Duration::from_millis(10),
        Duration::from_secs(2),
    );

    let start = Instant::now();
    flow.on_prepare_for_sleep(true);
    let elapsed = start.elapsed();

    assert!(marker.exists(), "the locker ran and marked itself locked");
    assert_eq!(marker_lines(&marker), 1, "exactly one locker spawned");
    assert!(
        wm.observed_locked.load(Ordering::SeqCst),
        "the flow waited until the lock was confirmed (marker) before proceeding"
    );
    assert!(
        elapsed >= Duration::from_millis(250),
        "did not proceed before the marker: {elapsed:?}"
    );
    assert!(
        acquired_inhibitor(&logind, InhibitMode::Delay),
        "the sleep delay inhibitor was taken"
    );
    assert!(
        acquired_inhibitor(&logind, InhibitMode::Block),
        "the power-key block inhibitor was taken"
    );
}

/// A locker already running when sleep is announced might not have taken the
/// lock yet (an idle trigger racing the suspend). `PrepareForSleep(true)` must
/// still wait for the compositor to confirm — a running process is not proof
/// the screen is secured.
#[test]
fn prepare_for_sleep_waits_for_an_unconfirmed_running_locker() {
    let dir = tempfile::tempdir().expect("tempdir");
    let marker = dir.path().join("unconfirmed");
    let logind = Arc::new(RecordingLogind::new(session_path()));
    let wm = Arc::new(MarkerWm::new(&marker));
    let flow = wired_flow(
        &logind,
        wm.clone(),
        Some(locker_command(&marker, 0.3, 2.0)),
        true,
        Duration::from_millis(10),
        Duration::from_secs(2),
    );

    flow.on_idle();
    assert!(flow.locker_running(), "idle spawned a locker");
    assert!(
        !wm.observed_locked.load(Ordering::SeqCst),
        "the running locker has not taken the lock yet"
    );

    let start = Instant::now();
    flow.on_prepare_for_sleep(true);
    let elapsed = start.elapsed();

    assert!(marker.exists());
    assert!(
        wm.observed_locked.load(Ordering::SeqCst),
        "prepare-for-sleep waited for the running locker to confirm the lock"
    );
    assert!(
        elapsed >= Duration::from_millis(250),
        "did not skip the wait on an unconfirmed locker: {elapsed:?}"
    );
    assert_eq!(marker_lines(&marker), 1, "no second locker spawned");

    flow.on_unlock();
}

/// `lock_before_sleep=false` disables pre-sleep locking outright: a running
/// but unconfirmed locker (from an idle trigger) must not delay sleep. The
/// trigger warns and releases immediately, and does not spawn a second locker.
#[test]
fn lock_before_sleep_disabled_does_not_wait_on_a_running_locker() {
    let dir = tempfile::tempdir().expect("tempdir");
    let marker = dir.path().join("disabled");
    let logind = Arc::new(RecordingLogind::new(session_path()));
    let wm = Arc::new(MarkerWm::new(&marker));
    let flow = wired_flow(
        &logind,
        wm.clone(),
        Some(locker_command(&marker, 0.3, 2.0)),
        false,
        Duration::from_millis(10),
        Duration::from_secs(2),
    );

    flow.on_idle();
    assert!(flow.locker_running(), "idle spawned a locker");

    let start = Instant::now();
    flow.on_prepare_for_sleep(true);
    let elapsed = start.elapsed();

    assert!(
        elapsed < Duration::from_millis(250),
        "lock_before_sleep=false must not delay sleep: {elapsed:?}"
    );
    assert!(
        !wm.observed_locked.load(Ordering::SeqCst),
        "did not wait for an unconfirmed lock"
    );

    // The idle locker is the only spawn; sleep must not have added another.
    assert!(wait_for_marker(&marker, Duration::from_secs(2)));
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(marker_lines(&marker), 1, "no second locker spawned");

    flow.on_unlock();
}

/// A locker that never confirms the lock still must not hold sleep past the
/// budget: the flow returns (releasing the inhibitor) anyway.
#[test]
fn a_locker_that_never_confirms_is_released_within_the_budget() {
    let dir = tempfile::tempdir().expect("tempdir");
    let marker = dir.path().join("never-confirms");
    let logind = Arc::new(RecordingLogind::new(session_path()));
    let wm = Arc::new(RecordingWm::new(false));
    let flow = wired_flow(
        &logind,
        wm,
        Some("sleep 30".to_string()),
        true,
        Duration::from_millis(10),
        Duration::from_millis(150),
    );

    let start = Instant::now();
    flow.on_prepare_for_sleep(true);
    let elapsed = start.elapsed();

    assert!(!marker.exists(), "the never-confirming stub wrote nothing");
    assert!(
        elapsed >= Duration::from_millis(120),
        "waited the budget: {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_secs(2),
        "did not hang on the un-confirmed lock: {elapsed:?}"
    );

    flow.on_unlock();
    assert!(!flow.locker_running(), "cleanup killed the linger stub");
}

/// `PrepareForSleep(false)` (resume) re-arms a fresh one-shot delay inhibitor
/// for the next sleep cycle (spec Decision 5).
#[test]
fn resume_re_arms_a_fresh_delay_inhibitor() {
    let dir = tempfile::tempdir().expect("tempdir");
    let marker = dir.path().join("resume");
    let logind = Arc::new(RecordingLogind::new(session_path()));
    let wm = Arc::new(RecordingWm::new(false));
    let flow = wired_flow(
        &logind,
        wm,
        Some(locker_command(&marker, 0.0, 0.0)),
        true,
        Duration::from_millis(10),
        Duration::from_millis(100),
    );
    assert_eq!(
        inhibit_count(&logind, InhibitMode::Delay),
        1,
        "armed at boot"
    );

    flow.on_prepare_for_sleep(true);
    flow.on_prepare_for_sleep(false);

    assert_eq!(
        inhibit_count(&logind, InhibitMode::Delay),
        2,
        "resume re-armed the consumed delay inhibitor"
    );
}

/// With no `locker_command`, `PrepareForSleep(true)` spawns nothing and
/// releases immediately — no screen nothing could unlock, no delay.
#[test]
fn no_locker_configured_never_spawns_and_never_delays() {
    let dir = tempfile::tempdir().expect("tempdir");
    let marker = dir.path().join("never-spawned");
    let logind = Arc::new(RecordingLogind::new(session_path()));
    let wm = Arc::new(MarkerWm::new(&marker));
    let flow = wired_flow(
        &logind,
        wm,
        None,
        true,
        Duration::from_millis(10),
        Duration::from_secs(2),
    );

    let start = Instant::now();
    flow.on_prepare_for_sleep(true);
    let elapsed = start.elapsed();

    assert!(!marker.exists(), "nothing was spawned");
    assert!(
        elapsed < Duration::from_millis(250),
        "no delay without a locker: {elapsed:?}"
    );
}

/// An idle timeout and a `PrepareForSleep` landing together must spawn exactly
/// one locker: the second trigger sees a locker already running and no-ops.
#[test]
fn idle_and_sleep_race_spawns_only_one_locker() {
    let dir = tempfile::tempdir().expect("tempdir");
    let marker = dir.path().join("race");
    let logind = Arc::new(RecordingLogind::new(session_path()));
    let wm = Arc::new(RecordingWm::new(false));
    let flow = wired_flow(
        &logind,
        wm,
        Some(locker_command(&marker, 0.0, 2.0)),
        true,
        Duration::from_millis(10),
        Duration::from_millis(200),
    );

    flow.on_idle();
    assert!(flow.locker_running(), "the first trigger spawned a locker");
    flow.on_prepare_for_sleep(true);

    assert!(wait_for_marker(&marker, Duration::from_secs(2)));
    // Let any erroneous second spawn append its line before counting.
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(marker_lines(&marker), 1, "no second locker spawned");

    flow.on_unlock();
    assert!(!flow.locker_running());
}

/// The `Lock` funnel spawns a locker; when that locker exits (a successful
/// unlock, by the locker contract) the flow mirrors it into `Session.Unlock`.
#[test]
fn lock_signal_funnel_spawns_and_unlocks_when_the_locker_exits() {
    let dir = tempfile::tempdir().expect("tempdir");
    let marker = dir.path().join("funnel");
    let logind = Arc::new(RecordingLogind::new(session_path()));
    let wm = Arc::new(RecordingWm::new(false));
    let flow = wired_flow(
        &logind,
        wm,
        Some(locker_command(&marker, 0.0, 0.0)),
        true,
        Duration::from_millis(10),
        Duration::from_millis(200),
    );

    flow.on_lock();

    assert!(
        wait_for_marker(&marker, Duration::from_secs(2)),
        "the locker spawned and ran"
    );
    assert!(
        wait_for_call(&logind, &LogindCall::Unlock),
        "the locker's exit unlocked the session: {:?}",
        logind.calls()
    );
}

/// An external `Unlock` while the locker is still up kills it, and does not
/// round-trip another `Session.Unlock` (the unlock already happened).
#[test]
fn external_unlock_kills_a_running_locker() {
    let dir = tempfile::tempdir().expect("tempdir");
    let marker = dir.path().join("external");
    let logind = Arc::new(RecordingLogind::new(session_path()));
    let wm = Arc::new(RecordingWm::new(false));
    let flow = wired_flow(
        &logind,
        wm,
        Some(locker_command(&marker, 0.0, 30.0)),
        true,
        Duration::from_millis(10),
        Duration::from_millis(200),
    );

    flow.on_lock();
    assert!(flow.locker_running());
    flow.on_unlock();
    assert!(
        !flow.locker_running(),
        "the external unlock killed the locker"
    );
    assert!(
        !logind.calls().contains(&LogindCall::Unlock),
        "an external unlock does not issue another Unlock"
    );
}
