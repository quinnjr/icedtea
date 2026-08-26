//! Proves the Wayland seam: the themed button really reaches a real
//! compositor's screen as the color the theme declares.
//!
//! Modelled on `compositor/tests/compat_protocols.rs`'s viewporter +
//! screencopy test: boot the headless harness compositor, run a real
//! client against it, capture the output, and read pixels back.

use std::process::{Child, Command};
use std::time::{Duration, Instant};

use icedtea_harness::{CapturedFrame, Compositor, ScreencopyClient, VirtualPointerClient};

/// The button's padding gutter, left of the label (`label_x` is 10) and clear
/// of the 4px corner arcs: the one column whose color is the background alone.
const GUTTER_X: u32 = 4;
/// The button's centre row; its border box is 78x27 at the output origin.
const CENTRE_Y: u32 = 13;

/// Kill the child on the way out however the test ends.
struct Reaper(Child);

impl Drop for Reaper {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn pixel_at(frame: &CapturedFrame, x: u32, y: u32) -> Option<(u8, u8, u8)> {
    use wayland_client::protocol::wl_shm::Format;
    // Byte order per the compositor suite's own screencopy tests: Xrgb/Argb
    // are B, G, R, X in memory; Bgr888 is R, G, B despite the name.
    let (bpp, order): (usize, [usize; 3]) = match frame.format {
        Format::Xrgb8888 | Format::Argb8888 => (4, [2, 1, 0]),
        Format::Bgr888 => (3, [0, 1, 2]),
        _ => return None,
    };
    let offset = y as usize * frame.stride as usize + x as usize * bpp;
    if offset + 2 >= frame.bytes.len() {
        return None;
    }
    Some((
        frame.bytes[offset + order[0]],
        frame.bytes[offset + order[1]],
        frame.bytes[offset + order[2]],
    ))
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

#[test]
fn themed_button_paints_accent_blue_on_a_layer_surface() {
    let comp = Compositor::spawn();

    let child = Command::new(env!("CARGO_BIN_EXE_themed-button"))
        .env("WAYLAND_DISPLAY", &comp.socket)
        // Bundled, not the developer's own gtk.css: the assertion below is
        // Adwaita's accent blue and the test must not depend on the host.
        .env("ICEDTEA_UI_THEME", "bundled")
        .env("ICEDTEA_UI_CLASSES", "suggested-action")
        .env("ICEDTEA_UI_LABEL", "Click me")
        .spawn()
        .expect("failed to spawn themed-button");
    let _reaper = Reaper(child);

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
    let sample = pixel_at(&frame, GUTTER_X, CENTRE_Y).expect("sample pixel inside the frame");
    assert!(
        matches_accent(sample),
        "pixel ({GUTTER_X}, {CENTRE_Y}) is {sample:?}, not the accent blue the \
         top-left-anchored button should be painting there"
    );
}

/// `.suggested-action:hover`'s background is
/// `linear-gradient(to top, #185cb0, #1c6fd4 1px)`, i.e. flat #1c6fd4
/// everywhere above the bottom pixel row -- distinct from the unhovered
/// gradient (#2c7fe3 -> #3584e4) on every channel by more than the
/// tolerance below.
fn matches_hover(px: (u8, u8, u8)) -> bool {
    close(px.0, 0x1C) && close(px.1, 0x6F) && close(px.2, 0xD4)
}

/// The other half of the seam: a real pointer entering the layer surface
/// drives `:hover` through the cascade and back out to the compositor's
/// screen as the hover colour the theme declares.
#[test]
fn hovering_the_button_repaints_it_in_the_themes_hover_color() {
    let comp = Compositor::spawn();

    let child = Command::new(env!("CARGO_BIN_EXE_themed-button"))
        .env("WAYLAND_DISPLAY", &comp.socket)
        .env("ICEDTEA_UI_THEME", "bundled")
        .env("ICEDTEA_UI_CLASSES", "suggested-action")
        .env("ICEDTEA_UI_LABEL", "Click me")
        .spawn()
        .expect("failed to spawn themed-button");
    let _reaper = Reaper(child);

    // Wait for the unhovered button to be on screen before injecting, so a
    // hover match cannot be confused with "the surface never mapped".
    let mut sc = ScreencopyClient::spawn(&comp.socket);
    let mut frame = sc.capture();
    let deadline = Instant::now() + Duration::from_secs(10);
    while count_accent_pixels(&frame) == 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
        frame = sc.capture();
    }
    let before = pixel_at(&frame, GUTTER_X, CENTRE_Y).expect("sample pixel inside the frame");
    assert!(
        matches_accent(before),
        "the button was not showing its unhovered accent blue at \
         ({GUTTER_X}, {CENTRE_Y}) before the pointer moved: {before:?}"
    );

    // Onto the button's centre: inside its 78x27 border box either way.
    let mut pointer = VirtualPointerClient::spawn(&comp.socket);
    pointer.motion_absolute(39.0, f64::from(CENTRE_Y), 1280, 720);
    pointer.frame();
    pointer.pump();

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut sample = before;
    while !matches_hover(sample) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
        pointer.pump();
        let frame = sc.capture();
        sample = pixel_at(&frame, GUTTER_X, CENTRE_Y).expect("sample pixel inside the frame");
    }
    assert!(
        matches_hover(sample),
        "pixel ({GUTTER_X}, {CENTRE_Y}) is {sample:?}, not the #1c6fd4 that \
         `.suggested-action:hover` declares: the pointer never drove :hover through \
         the cascade and back onto the screen"
    );
}
