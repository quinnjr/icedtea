//! Proves the Wayland seam: the themed button really reaches a real
//! compositor's screen as the color the theme declares.
//!
//! Modelled on `compositor/tests/compat_protocols.rs`'s viewporter +
//! screencopy test: boot the headless harness compositor, run a real
//! client against it, capture the output, and read pixels back.

mod support;

use std::time::{Duration, Instant};

use icedtea_harness::{CapturedFrame, Compositor, ScreencopyClient, VirtualPointerClient};
use icedtea_ui::layout::Allocation;
use icedtea_ui::wayland::BTN_LEFT;

use support::{allocation_of, spawn_themed_button};

/// The class list the pixel assertions below are pinned to.
const CLASSES: &str = "suggested-action";
/// The label the pixel assertions below are pinned to.
const LABEL: &str = "Click me";

/// Where on the output the button's background can be sampled.
///
/// The button is anchored top-left with margin 0, so its border box starts
/// at the output origin. `x` is the padding gutter: inside the 1 px border,
/// left of the label's content box, and clear of the 5 px corner arcs. `y`
/// is the button's centre row, which is derived from the allocation rather
/// than hardcoded -- A2's content-box minimum moved it.
struct Sample {
    allocation: Allocation,
    x: u32,
    y: u32,
}

impl Sample {
    fn derive() -> Self {
        let allocation = allocation_of(LABEL, CLASSES);
        let x = 4;
        let y = (allocation.height / 2.0) as u32;
        assert!(
            allocation.label_x > x as f32,
            "sampling column {x} is not clear of the label, which starts at {}",
            allocation.label_x
        );
        assert!(
            y >= 6 && (allocation.height - y as f32) >= 6.0,
            "sampling row {y} is not clear of the 5px corner arcs of a \
             {}-high button",
            allocation.height
        );
        Self { allocation, x, y }
    }

    /// The centre of the button, in output coordinates: where to put the
    /// pointer so it is unambiguously inside the border box.
    fn centre(&self) -> (f64, f64) {
        (
            f64::from(self.allocation.width) / 2.0,
            f64::from(self.allocation.height) / 2.0,
        )
    }

    /// Somewhere on the output that is definitely *outside* the button.
    fn outside(&self) -> (f64, f64) {
        (
            f64::from(self.allocation.width) + 40.0,
            f64::from(self.allocation.height) + 40.0,
        )
    }

    fn pixel(&self, frame: &CapturedFrame) -> (u8, u8, u8) {
        pixel_at(frame, self.x, self.y).expect("sample pixel inside the frame")
    }
}

fn pixel_at(frame: &CapturedFrame, x: u32, y: u32) -> Option<(u8, u8, u8)> {
    icedtea_ui::shm::pixel_rgb(frame.format, &frame.bytes, frame.stride, x, y)
}

// 12 is a ceiling, not a comfortable margin: at this tolerance the accent
// (0x35, 0x84, 0xE4) and hover (0x1C, 0x6F, 0xD4) bands already overlap on
// green (diff 21) and blue (diff 16) -- only red (diff 25) still separates
// them, and only by exactly one unit (25 vs. the 24 the two ±12 windows can
// span before touching). Raising this constant makes `matches_accent` and
// `matches_hover` mutually satisfiable, so no pixel could tell the two
// states apart.
fn close(a: u8, b: u8) -> bool {
    i32::from(a).abs_diff(i32::from(b)) <= 12
}

/// Adwaita's `@accent_color`: `button.suggested-action`'s background gradient
/// runs #2c7fe3 -> #3584e4, both within 12 per channel of #3584e4, and it is
/// far from the compositor's wallpaper (#1e1e2e) and palette foreground
/// (#cdd6f4) on every channel.
fn matches_accent(px: (u8, u8, u8)) -> bool {
    close(px.0, 0x35) && close(px.1, 0x84) && close(px.2, 0xE4)
}

/// `.suggested-action:hover`'s background is
/// `linear-gradient(to top, #185cb0, #1c6fd4 1px)`, i.e. flat #1c6fd4
/// everywhere above the bottom pixel row -- distinct from the unhovered
/// gradient (#2c7fe3 -> #3584e4) on every channel by more than the
/// tolerance above.
fn matches_hover(px: (u8, u8, u8)) -> bool {
    close(px.0, 0x1C) && close(px.1, 0x6F) && close(px.2, 0xD4)
}

/// `.suggested-action:active`'s background is `image(#1961b9)`, flat. Its
/// blue channel is 27 from `:hover`'s #1c6fd4, so the two ±12 windows do not
/// touch and a pixel can tell "pressed" from "hovered".
fn matches_active(px: (u8, u8, u8)) -> bool {
    close(px.0, 0x19) && close(px.1, 0x61) && close(px.2, 0xB9)
}

fn count_accent_pixels(frame: &CapturedFrame) -> u32 {
    let mut count = 0;
    for y in 0..frame.height {
        for x in 0..frame.width {
            if pixel_at(frame, x, y).is_some_and(matches_accent) {
                count += 1;
            }
        }
    }
    count
}

/// Capture until `ready` accepts the sample pixel, or the deadline passes.
fn capture_until(
    sc: &mut ScreencopyClient,
    pointer: Option<&mut VirtualPointerClient>,
    sample: &Sample,
    ready: impl Fn((u8, u8, u8)) -> bool,
) -> (u8, u8, u8) {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut pointer = pointer;
    let mut px = sample.pixel(&sc.capture());
    while !ready(px) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
        if let Some(pointer) = pointer.as_deref_mut() {
            pointer.pump();
        }
        px = sample.pixel(&sc.capture());
    }
    px
}

#[test]
fn themed_button_paints_accent_blue_on_a_layer_surface() {
    let sample = Sample::derive();
    let comp = Compositor::spawn();
    let _child = spawn_themed_button(&comp.socket, LABEL, CLASSES);

    let mut sc = ScreencopyClient::spawn(&comp.socket);
    let mut frame = sc.capture();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut accent = count_accent_pixels(&frame);
    while accent <= 100 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
        frame = sc.capture();
        accent = count_accent_pixels(&frame);
    }

    assert!(
        accent > 100,
        "only {accent} accent-blue pixels on screen: the themed button never reached the \
         compositor's output"
    );

    // And it is where the layer surface anchors it: top-left, margin 0.
    let px = sample.pixel(&frame);
    assert!(
        matches_accent(px),
        "pixel ({}, {}) is {px:?}, not the accent blue the top-left-anchored \
         {}x{} button should be painting there",
        sample.x,
        sample.y,
        sample.allocation.width,
        sample.allocation.height
    );
}

/// The other half of the seam: a real pointer entering the layer surface
/// drives `:hover` through the cascade and back out to the compositor's
/// screen as the hover colour the theme declares.
#[test]
fn hovering_the_button_repaints_it_in_the_themes_hover_color() {
    let sample = Sample::derive();
    let comp = Compositor::spawn();
    // The headless harness output, which the pointer's absolute motion is
    // expressed against.
    let (w, h) = comp.output_size();
    let (output_w, output_h) = (w as u32, h as u32);
    let _child = spawn_themed_button(&comp.socket, LABEL, CLASSES);

    // Wait for the unhovered button to be on screen before injecting, so a
    // hover match cannot be confused with "the surface never mapped".
    let mut sc = ScreencopyClient::spawn(&comp.socket);
    let before = capture_until(&mut sc, None, &sample, matches_accent);
    assert!(
        matches_accent(before),
        "the button was not showing its unhovered accent blue at \
         ({}, {}) before the pointer moved: {before:?}",
        sample.x,
        sample.y
    );

    let mut pointer = VirtualPointerClient::spawn(&comp.socket);
    let (cx, cy) = sample.centre();
    pointer.motion_absolute(cx, cy, output_w, output_h);
    pointer.frame();
    pointer.pump();

    let px = capture_until(&mut sc, Some(&mut pointer), &sample, matches_hover);
    assert!(
        matches_hover(px),
        "pixel ({}, {}) is {px:?}, not the #1c6fd4 that \
         `.suggested-action:hover` declares: the pointer never drove :hover through \
         the cascade and back onto the screen",
        sample.x,
        sample.y
    );
}

/// A6 end to end: pressing BTN_LEFT arms `:active`, dragging *off* the
/// button while still held drops it, dragging back on **re-arms** it, and a
/// release *off* the button ends the press for good. The pre-fix client
/// never re-armed, because leaving cleared the only state it tracked and
/// nothing remembered the mouse button was still down.
///
/// The release half used to be left to the unit tests, because the
/// compositor did not honour the Wayland implicit pointer grab: it
/// re-focused on every motion, so a release delivered after the cursor had
/// been dragged off the surface went to whatever was under the cursor and
/// this client never saw it. `wlr` 0.20.27 fixes that in the crate, which
/// makes the whole gesture assertable end to end -- and makes this the
/// test that would catch the regression, since a client left believing the
/// button is still down re-arms `:active` on a *hover* that should only
/// ever paint `:hover`.
#[test]
fn dragging_off_and_back_while_held_re_arms_active() {
    let sample = Sample::derive();
    let comp = Compositor::spawn();
    let (w, h) = comp.output_size();
    let (output_w, output_h) = (w as u32, h as u32);
    let _child = spawn_themed_button(&comp.socket, LABEL, CLASSES);

    let mut sc = ScreencopyClient::spawn(&comp.socket);
    let before = capture_until(&mut sc, None, &sample, matches_accent);
    assert!(
        matches_accent(before),
        "the button never mapped: {before:?}"
    );

    let mut pointer = VirtualPointerClient::spawn(&comp.socket);
    let (cx, cy) = sample.centre();
    pointer.motion_absolute(cx, cy, output_w, output_h);
    pointer.frame();
    pointer.pump();
    // Wait for `:hover` on screen before pressing: that is the proof the
    // compositor has moved pointer focus onto the layer surface, so the
    // button event that follows cannot race the enter.
    let hovered = capture_until(&mut sc, Some(&mut pointer), &sample, matches_hover);
    assert!(
        matches_hover(hovered),
        "the pointer never entered: {hovered:?}"
    );

    pointer.button(BTN_LEFT, true);
    pointer.frame();
    pointer.pump();

    let pressed = capture_until(&mut sc, Some(&mut pointer), &sample, matches_active);
    assert!(
        matches_active(pressed),
        "pressing BTN_LEFT did not paint `:active`'s #1961b9: {pressed:?}"
    );

    // Drag off the button, still held: `:active` must drop.
    let (ox, oy) = sample.outside();
    pointer.motion_absolute(ox, oy, output_w, output_h);
    pointer.frame();
    pointer.pump();
    let away = capture_until(&mut sc, Some(&mut pointer), &sample, |px| {
        !matches_active(px)
    });
    assert!(
        !matches_active(away),
        "dragging off the button left it painted `:active`: {away:?}"
    );

    // Drag back on, still held: `:active` must come back.
    pointer.motion_absolute(cx, cy, output_w, output_h);
    pointer.frame();
    pointer.pump();
    let again = capture_until(&mut sc, Some(&mut pointer), &sample, matches_active);
    assert!(
        matches_active(again),
        "re-entering the button while BTN_LEFT was still held did not re-arm \
         `:active`: {again:?}"
    );

    // Drag off once more and release *there*. The implicit grab means the
    // release still reaches this surface even though the cursor has left
    // it -- which is the only way the client can learn the press ended.
    pointer.motion_absolute(ox, oy, output_w, output_h);
    pointer.frame();
    pointer.pump();
    let off = capture_until(&mut sc, Some(&mut pointer), &sample, |px| {
        !matches_active(px)
    });
    assert!(
        !matches_active(off),
        "the second drag off left `:active`: {off:?}"
    );

    pointer.button(BTN_LEFT, false);
    pointer.frame();
    pointer.pump();

    // Move back onto the button with nothing held. It must paint `:hover`,
    // not `:active`: a client that never saw the release would still think
    // the button was down and re-arm.
    pointer.motion_absolute(cx, cy, output_w, output_h);
    pointer.frame();
    pointer.pump();
    let after_release = capture_until(&mut sc, Some(&mut pointer), &sample, matches_hover);
    assert!(
        matches_hover(after_release),
        "after a release off the button, hovering it paints {after_release:?} rather \
         than `:hover`'s #1c6fd4: the release never reached the client, so it \
         still believes BTN_LEFT is held"
    );
    assert!(
        !matches_active(after_release),
        "the button re-armed `:active` on a plain hover: the release was lost"
    );
}
