//! Idle→lock, driven end to end against the harness's real headless
//! compositor (which advertises `ext_idle_notifier_v1` and runs the real
//! wlroots idle timer) and the production [`icedtea_session::idle`] client.
//!
//! No activity is injected, so the seat goes idle and the compositor sends
//! `ext_idle_notification_v1.idled` after the short timeout; the client routes
//! that into the same [`LockFlow`] funnel lock-before-sleep uses, which spawns
//! the configured stub locker. The stub appends to a marker file and then
//! lingers, so "the lock fired exactly once" is observable as exactly one
//! marker line and exactly one `IsLocked` re-query (the funnel's guard is
//! entered once per lock attempt — a second `idled` would add both).
//!
//! With `lock_idle_timeout_ms: None` the client is never started at all, so no
//! connection is opened and nothing fires.

use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use icedtea_harness::Compositor;
use icedtea_session::flow::LockFlow;
use icedtea_session::logind::RecordingLogind;
use icedtea_session::wm_client::{RecordingWm, WmCall};
use wayland_client::Connection;

/// A `sh -c` stub locker that marks itself locked by writing one line to
/// `marker`, then lingers `linger` seconds so it is still running while the
/// test counts the spawns. Written-then-renamed so the marker never exists
/// empty (the tests read "marker exists" as "the lock is confirmed").
fn locker_command(marker: &Path, linger: f64) -> String {
    let tmp = format!("{}.tmp", marker.display());
    format!(
        "printf 'locked\\n' > '{tmp}' && mv '{tmp}' '{}'; sleep {linger}",
        marker.display()
    )
}

/// Number of lines the stub locker appended (one per spawn).
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

/// How many times the funnel queried the compositor's lock state. One query
/// per lock attempt, so this is "how many times the idle path drove the
/// funnel".
fn lock_attempts(wm: &RecordingWm) -> usize {
    wm.calls()
        .iter()
        .filter(|call| matches!(call, WmCall::IsLocked))
        .count()
}

fn session_path() -> &'static str {
    "/org/freedesktop/login1/session/c1"
}

/// One idle timeout with a configured locker spawns exactly one locker (the
/// marker gets exactly one line) and drives the funnel exactly once. The stub
/// lingers, so a second `idled` (or a re-entrant request) would leave a second
/// marker line and a second `IsLocked` query rather than being masked by the
/// first locker exiting.
#[test]
fn one_idle_timeout_spawns_exactly_one_locker() {
    let comp = Compositor::spawn();
    let dir = tempfile::tempdir().expect("tempdir");
    let marker = dir.path().join("locked");

    let logind = Arc::new(RecordingLogind::new(session_path()));
    let wm = Arc::new(RecordingWm::new(false));
    let flow = Arc::new(LockFlow::with_timing(
        logind.clone(),
        wm.clone(),
        Some(locker_command(&marker, 30.0)),
        true,
        Duration::from_millis(10),
        Duration::from_millis(200),
    ));
    // The idle path only asks logind to lock; logind's `Lock` signal echo is
    // what drives the funnel's spawn. Wire it so this exercises the real
    // single-funnel path, not a direct spawn.
    let weak = Arc::downgrade(&flow);
    logind.set_lock_echo(move || {
        if let Some(flow) = weak.upgrade() {
            flow.on_lock();
        }
    });

    let stream = UnixStream::connect(comp.socket_path()).expect("connect to harness compositor");
    let conn = Connection::from_socket(stream).expect("wayland connection");
    let idle_flow = Arc::clone(&flow);
    let _idle = std::thread::spawn(move || icedtea_session::idle::run(conn, 50, idle_flow));

    assert!(
        wait_for_marker(&marker, Duration::from_secs(5)),
        "the idle timeout never spawned the locker"
    );
    // Give a buggy re-request time to fire a second time before counting.
    std::thread::sleep(Duration::from_millis(200));

    assert_eq!(
        marker_lines(&marker),
        1,
        "exactly one locker spawned for one idle timeout"
    );
    assert_eq!(
        lock_attempts(&wm),
        1,
        "the idle path drove the lock funnel exactly once"
    );

    // Kill and reap the lingering stub so the test leaves nothing behind.
    flow.on_unlock();
}

/// `lock_idle_timeout_ms: None` disables idle-lock: no client is started, no
/// Wayland connection is opened, and nothing ever locks.
#[test]
fn unset_idle_timeout_starts_no_client_and_never_locks() {
    let dir = tempfile::tempdir().expect("tempdir");
    let marker: PathBuf = dir.path().join("locked");
    let logind = Arc::new(RecordingLogind::new(session_path()));
    let wm = Arc::new(RecordingWm::new(false));
    let flow = Arc::new(LockFlow::with_timing(
        logind,
        wm.clone(),
        Some(locker_command(&marker, 0.0)),
        true,
        Duration::from_millis(10),
        Duration::from_millis(200),
    ));

    let started = icedtea_session::idle::spawn(Arc::clone(&flow), None)
        .expect("None must not need a Wayland connection");
    assert!(
        started.is_none(),
        "lock_idle_timeout_ms=None must not start an idle client"
    );

    std::thread::sleep(Duration::from_millis(150));
    assert!(
        !marker.exists(),
        "no locker may be spawned with idle-lock off"
    );
    assert_eq!(
        lock_attempts(&wm),
        0,
        "the funnel must not be entered at all"
    );
    assert!(!flow.locker_running());
}
