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
use icedtea_harness::{Compositor, ScreencopyClient, VirtualKeyboardClient};
use support::{
    PROBE_BG, PROBE_ENTRY_BG, matches, pixel_at, probe_report, probe_theme, spawn_window_probe,
    wait_for_report_line,
};

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

/// Linux evdev codes.
const KEY_H: u32 = 35;
const KEY_I: u32 = 23;
const KEY_TAB: u32 = 15;

#[test]
fn a_virtual_keyboard_types_into_the_entry_and_the_glyphs_appear() {
    let compositor = Compositor::spawn();
    let theme = probe_theme();
    let report = tempfile::NamedTempFile::new().expect("report file");
    let socket = compositor.socket_path().to_string_lossy().to_string();
    let _probe = spawn_window_probe(&socket, "entry", theme.path(), report.path());
    wait_for_report_line(report.path(), "configure ", Duration::from_secs(10))
        .expect("a first configure");
    let window = compositor
        .snapshot()
        .windows
        .into_iter()
        .find(|w| w.app_id == "org.icedtea.WindowProbe")
        .expect("the probe's window");
    // The headless backend advertises `wl_seat`'s Keyboard capability only
    // once a device exists on the seat -- a virtual keyboard, here -- so the
    // probe cannot bind `wl_keyboard` (and therefore cannot receive `enter`)
    // until this is spawned. Ordering reconciled from the plan's literal
    // "focus, then wait for keyboard-enter, then spawn the keyboard" text,
    // which assumed a keyboard was already on the seat.
    let mut keyboard = VirtualKeyboardClient::spawn(&socket);
    compositor.send(DbCommand::Focus(window.id));
    wait_for_report_line(report.path(), "keyboard-enter", Duration::from_secs(10))
        .expect("the probe never got keyboard focus");

    keyboard.key_press(KEY_H);
    keyboard.key_press(KEY_I);
    keyboard.pump();
    let typed = wait_for_report_line(report.path(), "typed hi", Duration::from_secs(10));
    assert!(
        typed.is_some(),
        "the probe did not receive `hi`: {:?}",
        probe_report(report.path())
    );

    // And it is on screen: the entry's flat background, with dark glyph
    // pixels somewhere along the text baseline. The box (theme: `min-width:
    // 380px`) centres its two children's combined width, so the entry does
    // not sit at a fixed offset from the window's corner the way the SSD
    // strip does -- this locates it by its own unmistakable colour instead
    // of assuming a coordinate. The probe's render loop also repaints
    // asynchronously after the key events it just reported (`render` runs at
    // the top of its next loop iteration, past the `pump` that delivered the
    // keys), so this polls a screencopy rather than trusting a single
    // capture right after the report line to have already landed -- the
    // same reasoning `a_configure_resize_relayouts_and_repaints` uses.
    let mut screencopy = ScreencopyClient::spawn(&socket);
    let content_top = window.geometry.y + TITLE_BAR_HEIGHT;
    let locate_entry = |frame: &icedtea_harness::CapturedFrame| -> Option<(u32, u32, u32, u32)> {
        let mut min_x = u32::MAX;
        let mut min_y = u32::MAX;
        let mut max_x = 0u32;
        let mut max_y = 0u32;
        let mut found = false;
        for x in 0..window.geometry.width as u32 {
            for y in 0..(window.geometry.height - TITLE_BAR_HEIGHT) as u32 {
                if let Some(px) =
                    pixel_at(frame, window.geometry.x as u32 + x, content_top as u32 + y)
                    && matches(px, PROBE_ENTRY_BG)
                {
                    found = true;
                    min_x = min_x.min(x);
                    min_y = min_y.min(y);
                    max_x = max_x.max(x);
                    max_y = max_y.max(y);
                }
            }
        }
        found.then_some((min_x, min_y, max_x, max_y))
    };
    let count_dark = |frame: &icedtea_harness::CapturedFrame, bounds: (u32, u32, u32, u32)| {
        let (min_x, min_y, max_x, max_y) = bounds;
        let mut dark = 0;
        for x in min_x..=max_x {
            for y in min_y..=max_y {
                if let Some(px) =
                    pixel_at(frame, window.geometry.x as u32 + x, content_top as u32 + y)
                    && px.0 < 0x60
                    && px.1 < 0x60
                    && px.2 < 0x60
                {
                    dark += 1;
                }
            }
        }
        dark
    };
    let started = std::time::Instant::now();
    let mut frame = screencopy.capture();
    let mut bounds = locate_entry(&frame);
    let mut dark = bounds.map_or(0, |b| count_dark(&frame, b));
    while dark <= 8 && started.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(support::CAPTURE_POLL);
        frame = screencopy.capture();
        bounds = locate_entry(&frame);
        dark = bounds.map_or(0, |b| count_dark(&frame, b));
    }
    assert!(
        bounds.is_some(),
        "the entry's own background never appeared on screen: {:?}",
        probe_report(report.path())
    );
    assert!(dark > 8, "no glyph pixels in the entry: {dark} dark pixels");
}

#[test]
fn tab_moves_the_focus_ring_and_it_is_only_visible_after_a_key() {
    let compositor = Compositor::spawn();
    let theme = probe_theme();
    let report = tempfile::NamedTempFile::new().expect("report file");
    let socket = compositor.socket_path().to_string_lossy().to_string();
    let _probe = spawn_window_probe(&socket, "entry", theme.path(), report.path());
    wait_for_report_line(report.path(), "configure ", Duration::from_secs(10))
        .expect("a first configure");
    let window = compositor
        .snapshot()
        .windows
        .into_iter()
        .find(|w| w.app_id == "org.icedtea.WindowProbe")
        .expect("the probe's window");
    // See the ordering note in the sibling test: the seat only advertises
    // Keyboard once a device exists on it.
    let mut keyboard = VirtualKeyboardClient::spawn(&socket);
    compositor.send(DbCommand::Focus(window.id));
    wait_for_report_line(report.path(), "keyboard-enter", Duration::from_secs(10))
        .expect("keyboard focus");

    keyboard.key_press(KEY_TAB);
    keyboard.pump();
    let moved = wait_for_report_line(report.path(), "focus ", Duration::from_secs(10))
        .expect("Tab moved nothing");
    assert!(
        moved.contains("visible=true"),
        "a Tab that moved the focus must show the ring (ruling R3): {moved}"
    );
    assert!(
        moved.contains("entry") || moved.contains("menubutton"),
        "Tab landed on nothing focusable: {moved}"
    );

    // The probe's ring holds exactly two stops (`entry`, `menubutton`), so a
    // second Tab wraps it back onto `entry` -- the one node the probe routes
    // typed text to. Reconciled from the plan's single `key_press(KEY_TAB)`:
    // with only two focusable nodes, one Tab leaves focus on `menubutton`,
    // which the probe's hand-built tree never routes text into, so the
    // literal single press cannot make the next assertion's `typed h`
    // observable. The wrap is real `navigate` behaviour (`window-probe.rs`'s
    // `.or_else(|| navigate(..., None, dir))` fallback), not a test fudge.
    keyboard.key_press(KEY_TAB);
    keyboard.pump();
    // `wait_for_report_line` matches on prefix from the start of the
    // (append-only) report, so a bare `"focus "` prefix would just re-find
    // the first Tab's "focus menubutton" line again -- match the specific
    // line this second Tab must produce instead.
    let wrapped = wait_for_report_line(report.path(), "focus entry", Duration::from_secs(10));
    assert!(
        wrapped.is_some(),
        "a second Tab should wrap the two-stop ring back onto entry: {:?}",
        probe_report(report.path())
    );

    // Typing a letter that moves nothing hides the ring again.
    keyboard.key_press(KEY_H);
    keyboard.pump();
    let after_typing = wait_for_report_line(report.path(), "typed h", Duration::from_secs(10));
    assert!(after_typing.is_some(), "the letter never arrived");
    let lines = probe_report(report.path());
    let last_focus = lines.iter().rev().find(|l| l.starts_with("focus "));
    assert!(
        last_focus.is_some_and(|l| l.contains("visible=true")),
        "the ring's last reported state should still be the Tab's: {lines:?}"
    );
}
