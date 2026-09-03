//! M5 P0's ingress surface: `Window::watch_fd`, the inbox, `App::on_fd`,
//! `Cmd::Task`, the view-layer pointer events and `Keymap::base_keysym`.
//!
//! Live-`Window` tests drive `window-probe` under the harness compositor,
//! because `Window::open` reads `WAYLAND_DISPLAY` from the process
//! environment and a test process cannot set that per-test (edition 2024
//! makes `set_var` unsafe, and the tests in this binary run in parallel).
//! Everything that does not need a surface runs offscreen, in-process.

mod support;

use std::time::Duration;

use icedtea_harness::Compositor;
use support::{probe_report, probe_theme, spawn_window_probe_with, wait_for_report_line};

/// How long a probe gets to boot, map and report. The gallery gate's own
/// `GALLERY_MAP_TIMEOUT` documents why this is seconds rather than
/// milliseconds: a first anti-aliased paint is slow in a debug build.
const REPORT: Duration = Duration::from_secs(20);

#[test]
fn a_watched_pipe_wakes_pump_with_fd_ready() {
    // mutation: drop the `for (index, watch) in watches.entries()` loop from
    // `wait_bounded`'s `Ok(_)` arm; nothing ever pushes `FdReady` and this
    // test times out on `fd-ready`.
    let compositor = Compositor::spawn();
    let theme = probe_theme();
    let report = tempfile::NamedTempFile::new().expect("report file");
    let _probe = spawn_window_probe_with(
        &compositor.socket_path().to_string_lossy(),
        "ingress",
        theme.path(),
        report.path(),
        &[],
    );
    let registered = wait_for_report_line(report.path(), "watch-registered ", REPORT)
        .expect("the probe never registered its pipe");
    let id = registered
        .split_whitespace()
        .nth(1)
        .expect("the line carries the watch id")
        .to_string();
    let ready = wait_for_report_line(report.path(), "fd-ready ", REPORT)
        .expect("a byte on the watched pipe never woke pump");
    assert_eq!(
        ready.split_whitespace().nth(1),
        Some(id.as_str()),
        "the FdReady names a different watch: {ready}"
    );
}

#[test]
fn an_unwatched_fd_stops_waking_the_loop() {
    // mutation: make `Watches::remove` a no-op; the probe keeps reporting
    // `fd-ready` after the unwatch and never reports `fd-quiet`.
    let compositor = Compositor::spawn();
    let theme = probe_theme();
    let report = tempfile::NamedTempFile::new().expect("report file");
    let _probe = spawn_window_probe_with(
        &compositor.socket_path().to_string_lossy(),
        "ingress",
        theme.path(),
        report.path(),
        &[],
    );
    assert!(
        wait_for_report_line(report.path(), "fd-quiet", REPORT).is_some(),
        "the probe still saw readiness after unwatching: {:?}",
        probe_report(report.path())
    );
    let lines = probe_report(report.path());
    let unwatched = lines
        .iter()
        .position(|l| l.starts_with("unwatched "))
        .expect("the probe unwatched");
    let quiet = lines
        .iter()
        .position(|l| l == "fd-quiet")
        .expect("the probe reported quiet");
    assert!(
        quiet > unwatched,
        "the quiet window must follow the unwatch"
    );
    assert!(
        !lines[unwatched..]
            .iter()
            .any(|l| l.starts_with("fd-ready ")),
        "an FdReady arrived after the unwatch: {lines:?}"
    );
}

#[test]
fn two_watched_fds_report_in_registration_order() {
    // mutation: reverse `Watches::entries()`; the ids arrive swapped.
    let compositor = Compositor::spawn();
    let theme = probe_theme();
    let report = tempfile::NamedTempFile::new().expect("report file");
    let _probe = spawn_window_probe_with(
        &compositor.socket_path().to_string_lossy(),
        "ingress",
        theme.path(),
        report.path(),
        &["--two-watches"],
    );
    assert!(
        wait_for_report_line(report.path(), "fd-pair ", REPORT).is_some(),
        "the probe never saw both fds ready in one batch: {:?}",
        probe_report(report.path())
    );
    let line = probe_report(report.path())
        .into_iter()
        .find(|l| l.starts_with("fd-pair "))
        .expect("the pair line");
    let ids: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .filter_map(|f| f.parse().ok())
        .collect();
    assert_eq!(ids.len(), 2, "the pair line carries two ids: {line}");
    assert!(
        ids[0] < ids[1],
        "FdReady must arrive in registration order, got {ids:?}"
    );
}

#[test]
fn a_watch_does_not_turn_a_wayland_wakeup_into_a_timeout() {
    // A window with a registered but silent watch must still configure, paint
    // and report frames exactly as `window_events.rs` expects of one without.
    // mutation: return `SurfaceError::Timeout` whenever any watch is
    // registered; `frame ` never appears.
    let compositor = Compositor::spawn();
    let theme = probe_theme();
    let report = tempfile::NamedTempFile::new().expect("report file");
    let _probe = spawn_window_probe_with(
        &compositor.socket_path().to_string_lossy(),
        "ingress",
        theme.path(),
        report.path(),
        &["--silent-watch"],
    );
    assert!(
        wait_for_report_line(report.path(), "configure ", REPORT).is_some(),
        "a window with a silent watch never configured"
    );
    assert!(
        wait_for_report_line(report.path(), "frame ", REPORT).is_some(),
        "a window with a silent watch never painted a frame"
    );
}

#[test]
fn unwatching_an_unknown_id_is_a_no_op() {
    // mutation: `panic!` in `Watches::remove` for an id it does not hold; the
    // probe dies before reporting `unwatch-unknown-ok`.
    let compositor = Compositor::spawn();
    let theme = probe_theme();
    let report = tempfile::NamedTempFile::new().expect("report file");
    let _probe = spawn_window_probe_with(
        &compositor.socket_path().to_string_lossy(),
        "ingress",
        theme.path(),
        report.path(),
        &[],
    );
    assert!(
        wait_for_report_line(report.path(), "unwatch-unknown-ok", REPORT).is_some(),
        "unwatching an unknown id was not survivable: {:?}",
        probe_report(report.path())
    );
}

#[test]
fn an_idle_watch_costs_no_extra_wakeups() {
    // A complexity bound, not a wall-clock pin (contract §5 cross-cutting
    // rule): over a one-second idle window with a registered, silent watch,
    // `pump` must return far fewer times than a spin would produce. A spin
    // returns thousands; the frame clock alone returns tens.
    // mutation: add `Duration::ZERO` to `frame_deadline` whenever a watch is
    // registered; `wakes` climbs past the bound and this fails.
    let compositor = Compositor::spawn();
    let theme = probe_theme();
    let report = tempfile::NamedTempFile::new().expect("report file");
    let _probe = spawn_window_probe_with(
        &compositor.socket_path().to_string_lossy(),
        "ingress",
        theme.path(),
        report.path(),
        &["--silent-watch"],
    );
    let line = wait_for_report_line(report.path(), "wakes ", REPORT)
        .expect("the probe never closed its idle window");
    let wakes: u64 = line
        .split_whitespace()
        .nth(1)
        .and_then(|f| f.parse().ok())
        .expect("the wakes line carries a count");
    assert!(
        wakes < 500,
        "an idle window with one silent watch woke {wakes} times in a second; \
         that is a spin, not a poll"
    );
}
