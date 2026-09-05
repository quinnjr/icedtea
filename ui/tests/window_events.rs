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
use icedtea_harness::{Compositor, ScreencopyClient, VirtualKeyboardClient, VirtualPointerClient};
use support::{
    PROBE_BG, PROBE_ENTRY_BG, matches, pixel_at, probe_report, probe_theme, spawn_window_probe,
    wait_for_probe_window, wait_for_report_line,
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

    let window = wait_for_probe_window(&compositor, Duration::from_secs(10));
    let window = &window;
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
fn a_committed_frame_asks_for_a_callback_and_the_callback_is_dated() {
    // `commit_buffer_to` requests `wl_surface.frame` on every commit, and
    // `Window::pump` stamps the resulting `InputEvent::Frame` with a reading
    // of the window's own clock. Without the request the event never fires at
    // all; without the stamp it arrives permanently dated `Duration::ZERO`.
    let compositor = Compositor::spawn();
    let theme = probe_theme();
    let report = tempfile::NamedTempFile::new().expect("report file");
    let _probe = spawn_window_probe(
        &compositor.socket_path().to_string_lossy(),
        "entry",
        theme.path(),
        report.path(),
    );
    let line = wait_for_report_line(report.path(), "frame ", Duration::from_secs(10))
        .expect("no frame callback ever reached the client");
    let micros: u128 = line
        .split_whitespace()
        .nth(1)
        .and_then(|f| f.parse().ok())
        .expect("the frame line carries a timestamp");
    assert!(
        micros > 0,
        "the frame was handed up still dated Duration::ZERO: {line}"
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
    let id = wait_for_probe_window(&compositor, Duration::from_secs(10)).id;

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
    let window = wait_for_probe_window(&compositor, Duration::from_secs(10));
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
    let window = wait_for_probe_window(&compositor, Duration::from_secs(10));
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
    let window = wait_for_probe_window(&compositor, Duration::from_secs(10));
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

/// Fix for the Task 17 review finding: `Window::pump` must arm the repeat
/// timer from the last `Key` event in a batch (`keymap.arm_repeat`), or a
/// held key never repeats no matter how long it stays down. `key_press`
/// above always presses-and-releases in one shot, so it can never surface
/// this; only a real hold (`key_down` without a matching `key_up` until the
/// end) does.
#[test]
fn a_held_key_repeats_through_pump_and_types_more_than_one_glyph() {
    let compositor = Compositor::spawn();
    let theme = probe_theme();
    let report = tempfile::NamedTempFile::new().expect("report file");
    let socket = compositor.socket_path().to_string_lossy().to_string();
    let _probe = spawn_window_probe(&socket, "entry", theme.path(), report.path());
    wait_for_report_line(report.path(), "configure ", Duration::from_secs(10))
        .expect("a first configure");
    let window = wait_for_probe_window(&compositor, Duration::from_secs(10));

    // Keyboard capability must exist on the seat before Focus can deliver
    // `keyboard-enter` (see reconciliation 1 above) -- spawn first.
    let mut keyboard = VirtualKeyboardClient::spawn(&socket);
    compositor.send(DbCommand::Focus(window.id));
    wait_for_report_line(report.path(), "keyboard-enter", Duration::from_secs(10))
        .expect("the probe never got keyboard focus");

    // The compositor's virtual keyboard gets `wl_keyboard.repeat_info` of
    // rate 25/delay 600ms (wlr's default, set on every new keyboard device).
    // Hold `H` well past the initial delay plus a few repeat intervals, so
    // the probe's `Window::pump` has every chance to arm and then re-fire
    // the repeat timer.
    //
    // The deadline is the shared `REACT_TIMEOUT` rather than a snug multiple
    // of 600ms + 1/25s: the loop breaks the moment a second glyph shows up,
    // so a generous ceiling costs a passing run nothing and only stops a
    // loaded machine — where the delay, the pumps and the report write can
    // all slip — from failing a working repeat.
    keyboard.key_down(KEY_H);
    let deadline = std::time::Instant::now() + support::REACT_TIMEOUT;
    let mut last = String::new();
    while std::time::Instant::now() < deadline {
        keyboard.pump();
        std::thread::sleep(Duration::from_millis(50));
        last = probe_report(report.path())
            .into_iter()
            .rev()
            .find_map(|l| l.strip_prefix("typed ").map(str::to_owned))
            .unwrap_or_default();
        if last.len() > 1 {
            break;
        }
    }
    keyboard.key_up(KEY_H);
    keyboard.pump();

    assert!(
        last.len() > 1,
        "a held key must repeat more than one glyph through Window::pump, got {last:?}: {:?}",
        probe_report(report.path())
    );
    assert!(
        last.chars().all(|c| c == 'h'),
        "every repeated glyph should be `h`: {last:?}"
    );
}

const BTN_LEFT: u32 = 0x110;

#[test]
fn a_popup_opened_from_a_menubutton_takes_the_grab_and_is_dismissed_outside_it() {
    let compositor = Compositor::spawn();
    let theme = probe_theme();
    let report = tempfile::NamedTempFile::new().expect("report file");
    let socket = compositor.socket_path().to_string_lossy().to_string();
    // The virtual pointer must exist *before* the probe connects: a headless
    // seat has no `wl_seat.pointer` capability until an input device shows up,
    // and the probe binds `wl_pointer` only from the `wl_seat.capabilities`
    // event it gets during `Window::open`'s own opening roundtrips. Spawning
    // the pointer first (as every compositor-side popup test already does,
    // e.g. `compositor/tests/popups.rs`'s own tests) means that capability is
    // already advertised by the probe's first roundtrip, so its `wl_pointer`
    // exists before any click can race it. Spawning it after, as this test
    // originally did, loses that race deterministically: the click's button
    // reaches the compositor and is delivered against the seat's focused
    // client before the probe has dispatched the *second*, late
    // `wl_seat.capabilities` the new device triggers and called
    // `get_pointer` in response, so the client's `pointers` list is still
    // empty when `wlr_seat_pointer_notify_button` looks it up and the button
    // is silently dropped -- confirmed with a throwaway
    // `ICEDTEA_DEBUG_POINTER_BUTTON` probe in the `wlr` crate's
    // `on_pointer_button` (reverted, not part of this change) that printed
    // `pointers_in_client=0` for exactly this ordering.
    let (output_w, output_h) = compositor.output_size();
    let mut pointer = VirtualPointerClient::spawn(&socket);
    let _probe = spawn_window_probe(&socket, "menu", theme.path(), report.path());
    wait_for_report_line(report.path(), "configure ", Duration::from_secs(10))
        .expect("a first configure");
    let window = wait_for_probe_window(&compositor, Duration::from_secs(10));
    compositor.send(DbCommand::Focus(window.id));

    // Click on the menubutton: the probe opens its popup with the serial that
    // press mints, which is what makes the grab legal.
    //
    // Reconciled from the plan's literal `width - 50, TITLE_BAR_HEIGHT + 17`:
    // that assumed the menubutton sits flush against the box's trailing
    // edge, but the retained tree's default `box` lays out two
    // natural-width children (`entry`, `menubutton`) as a centered group
    // rather than stretching the entry to fill the row, so the menubutton
    // lands well short of the frame's right edge. Measured directly from the
    // probe's own `LayoutTree::allocation` for this theme and node tree: a
    // content-relative border box of `(370, 169, 100, 34)`; click its
    // center.
    let click = (
        f64::from(window.geometry.x + 420),
        f64::from(window.geometry.y + TITLE_BAR_HEIGHT + 186),
    );
    pointer.motion_absolute(click.0, click.1, output_w as u32, output_h as u32);
    pointer.frame();
    pointer.button(BTN_LEFT, true);
    pointer.frame();
    pointer.button(BTN_LEFT, false);
    pointer.frame();
    pointer.pump();

    let opened = wait_for_report_line(report.path(), "popup-", Duration::from_secs(10))
        .expect("the probe never reported a popup");
    assert_eq!(
        opened,
        "popup-open",
        "opening the popup failed: {:?}",
        probe_report(report.path())
    );

    // A click well outside the popup dismisses it: the compositor owns that
    // rule for a grabbing popup, and the client learns about it through
    // xdg_popup.popup_done.
    pointer.motion_absolute(
        4.0,
        f64::from(output_h - 4),
        output_w as u32,
        output_h as u32,
    );
    pointer.frame();
    pointer.button(BTN_LEFT, true);
    pointer.frame();
    pointer.button(BTN_LEFT, false);
    pointer.frame();
    pointer.pump();
    assert!(
        wait_for_report_line(report.path(), "popup-done", Duration::from_secs(10)).is_some(),
        "the grab did not dismiss the popup: {:?}",
        probe_report(report.path())
    );
}

#[test]
fn probe_points_locate_a_live_windows_widgets() {
    // Every M5 gate addresses widgets by id rather than by hard-coded
    // coordinates; on a live window `App::probe` (offscreen-only) cannot
    // answer, so `Window::probe_points` must.
    // mutation: return `Vec::new()` from `probe_points_of`; no `probe ` line
    // is ever written and this fails.
    let compositor = Compositor::spawn();
    let theme = probe_theme();
    let report = tempfile::NamedTempFile::new().expect("report file");
    let _probe = support::spawn_window_probe_with(
        &compositor.socket_path().to_string_lossy(),
        "entry",
        theme.path(),
        report.path(),
        &["--emit-probe"],
    );
    assert!(
        wait_for_report_line(report.path(), "probe ", Duration::from_secs(20)).is_some(),
        "the probe emitted no probe points: {:?}",
        probe_report(report.path())
    );
    let points: Vec<support::ProbePoint> = probe_report(report.path())
        .into_iter()
        .filter_map(|line| line.strip_prefix("probe ").map(str::to_owned))
        // `parse_probe_line` wants `<widget> <label> <x> <y>`; a window's
        // lines have no widget column, so the label stands in for both.
        .filter_map(|rest| support::parse_probe_line(&format!("window {rest}")))
        .collect();
    assert!(
        points.iter().any(|p| p.label == "entry"),
        "the entry is not among the probe points: {points:?}"
    );
    assert!(
        points.iter().any(|p| p.label == "menubutton"),
        "the menubutton is not among the probe points: {points:?}"
    );
    for point in &points {
        assert!(
            point.x >= 0 && point.y >= 0,
            "a probe point must be inside the surface: {point:?}"
        );
    }
}

#[test]
fn allocation_by_id_matches_the_probe_point_centre() {
    // mutation: return the border box un-floored (`as i32` on the raw centre)
    // in `probe_points_of`; a half-pixel centre rounds the other way and the
    // equality below fails.
    let compositor = Compositor::spawn();
    let theme = probe_theme();
    let report = tempfile::NamedTempFile::new().expect("report file");
    let _probe = support::spawn_window_probe_with(
        &compositor.socket_path().to_string_lossy(),
        "entry",
        theme.path(),
        report.path(),
        &["--emit-probe"],
    );
    assert!(
        wait_for_report_line(report.path(), "alloc entry ", Duration::from_secs(20)).is_some(),
        "no allocation line for the entry: {:?}",
        probe_report(report.path())
    );
    let lines = probe_report(report.path());
    let alloc = lines
        .iter()
        .find_map(|l| l.strip_prefix("alloc "))
        .and_then(support::parse_allocation_line)
        .expect("an allocation line parses");
    // `probe_points_of` emits the window's own root point first (labelled
    // "root", per its shared derivation with `gallery::probe_points_of`),
    // then every descendant in tree order -- not "entry" first. Find the
    // entry's own probe point by label rather than assuming it leads.
    let point = lines
        .iter()
        .filter_map(|l| l.strip_prefix("probe "))
        .map(|rest| format!("window {rest}"))
        .filter_map(|line| support::parse_probe_line(&line))
        .find(|p| p.label == "entry")
        .expect("an entry probe line parses");
    assert_eq!(alloc.widget, "entry", "the first alloc line is the entry's");
    assert_eq!(
        (point.x, point.y),
        (
            (alloc.x + alloc.width / 2.0).floor() as i32,
            (alloc.y + alloc.height / 2.0).floor() as i32
        ),
        "the probe point is the floored centre of the allocation"
    );
}
