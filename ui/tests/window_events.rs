//! End-to-end tests for `ui/src/window/**` against the harness compositor.
//!
//! The client is `window-probe`, a real `icedtea_ui::window::Window` holding a
//! hand-built GTK node tree (`window > box > entry > text`, plus a
//! `menubutton` in menu mode). The widgets that will produce those trees are
//! P5's and P6's; the node names are the ones they will use, so what these
//! tests measure does not change when they arrive.

mod support;

use std::time::Duration;

use icedtea_compositor::dbus::DbCommand;
use icedtea_harness::{Compositor, ScreencopyClient};
use support::{PROBE_BG, matches, pixel_at, probe_theme, spawn_window_probe, wait_for_report_line};

/// The compositor's server-side title bar, from `compositor/src/decoration.rs`.
const TITLE_BAR_HEIGHT: i32 = 28;

#[test]
fn a_toplevel_maps_under_server_side_decorations() {
    let compositor = Compositor::spawn();
    let theme = probe_theme();
    let report = tempfile::NamedTempFile::new().expect("report file");
    let _probe = spawn_window_probe(
        &compositor.socket_path().to_string_lossy(),
        "entry",
        theme.path(),
        report.path(),
    );
    let line = wait_for_report_line(report.path(), "configure ", Duration::from_secs(10))
        .expect("the probe never reported a configure");
    let fields: Vec<i32> = line
        .split_whitespace()
        .skip(1)
        .filter_map(|f| f.parse().ok())
        .collect();
    let (configured_w, configured_h) = (fields[0], fields[1]);

    let snapshot = compositor.snapshot();
    let window = snapshot
        .windows
        .iter()
        .find(|w| w.app_id == "org.icedtea.WindowProbe")
        .expect("the probe's window is in the model");
    assert_eq!(
        configured_w, window.geometry.width,
        "an SSD window is configured at the full frame width"
    );
    assert_eq!(
        configured_h,
        window.geometry.height - TITLE_BAR_HEIGHT,
        "and at the frame height minus the title bar the compositor draws"
    );

    let mut screencopy = ScreencopyClient::spawn(&compositor.socket_path().to_string_lossy());
    let frame = screencopy.capture();
    let inside = pixel_at(
        &frame,
        (window.geometry.x + 8) as u32,
        (window.geometry.y + TITLE_BAR_HEIGHT + 8) as u32,
    )
    .expect("a pixel inside the client area");
    assert!(
        matches(inside, PROBE_BG),
        "the client area does not show the probe's background: {inside:?}"
    );
    let bar = pixel_at(
        &frame,
        (window.geometry.x + 8) as u32,
        (window.geometry.y + 4) as u32,
    )
    .expect("a pixel in the title bar");
    assert!(
        !matches(bar, PROBE_BG),
        "the title-bar strip shows the client's own pixels: the client was mapped over it"
    );
}

#[test]
fn a_configure_resize_relayouts_and_repaints() {
    let compositor = Compositor::spawn();
    let theme = probe_theme();
    let report = tempfile::NamedTempFile::new().expect("report file");
    let _probe = spawn_window_probe(
        &compositor.socket_path().to_string_lossy(),
        "entry",
        theme.path(),
        report.path(),
    );
    let first = wait_for_report_line(report.path(), "configure ", Duration::from_secs(10))
        .expect("a first configure");
    let id = compositor
        .snapshot()
        .windows
        .iter()
        .find(|w| w.app_id == "org.icedtea.WindowProbe")
        .expect("the probe's window")
        .id;

    compositor.send(DbCommand::Maximize(id, true));
    // `wait_for_report_line` matches on prefix alone, and the report is
    // append-only with `first` already the earliest "configure " line in it,
    // so a single prefix search (or one un-timed re-check right after it)
    // always re-finds `first` before the compositor's asynchronous
    // `Maximize` has had time to produce a second one. Poll for a line that
    // actually differs from `first`, for up to the same budget.
    // A window activating on map produces its own "configure " line with no
    // size change (states only), interleaved with the asynchronous
    // `Maximize` command's real one -- so "any line that differs from
    // `first`" is not enough; wait specifically for the one that carries
    // `MAXIMIZED`, which is the actual event under test.
    let started = std::time::Instant::now();
    let maximized = loop {
        if let Some(line) = support::probe_report(report.path())
            .into_iter()
            .find(|line| line.starts_with("configure ") && line.contains("MAXIMIZED"))
        {
            break Some(line);
        }
        if started.elapsed() > Duration::from_secs(10) {
            break None;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    .expect("maximizing produced no second configure");
    assert_ne!(maximized, first, "the size did not change");
    assert!(
        maximized.contains("MAXIMIZED"),
        "the configure did not carry the maximized state: {maximized}"
    );
    assert!(
        maximized.contains("ACTIVATED"),
        "a focused window must not be in :backdrop: {maximized}"
    );

    let mut screencopy = ScreencopyClient::spawn(&compositor.socket_path().to_string_lossy());
    let window = compositor
        .snapshot()
        .windows
        .into_iter()
        .find(|w| w.app_id == "org.icedtea.WindowProbe")
        .expect("the probe's window");
    // The far corner of the *new* geometry is painted, which it would not be
    // if the buffer had stayed at its original size. The probe's own render
    // loop repaints asynchronously after the configure it just reported, so
    // this polls rather than trusting a single capture to land after that.
    let (corner, _) = support::capture_until(
        &mut screencopy,
        None,
        (
            (window.geometry.x + window.geometry.width - 8) as u32,
            (window.geometry.y + window.geometry.height - 8) as u32,
        ),
        Duration::from_secs(5),
        |px| matches(px, PROBE_BG),
    );
    assert!(
        matches(corner, PROBE_BG),
        "the resized window did not repaint its new area: {corner:?} geometry={:?}",
        window.geometry
    );
}
