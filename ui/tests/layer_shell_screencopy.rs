//! Proves the Wayland seam: the themed button really reaches a real
//! compositor's screen as the color the theme declares.
//!
//! Modelled on `compositor/tests/compat_protocols.rs`'s viewporter +
//! screencopy test: boot the headless harness compositor, run a real
//! client against it, capture the output, and read pixels back.

use std::process::{Child, Command};
use std::time::{Duration, Instant};

use icedtea_harness::{CapturedFrame, Compositor, ScreencopyClient};

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
    while accent == 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
        frame = sc.capture();
        accent = count_accent_pixels(&frame);
    }

    assert!(
        accent > 100,
        "only {accent} accent-blue pixels on screen: the themed button never reached the \
         compositor's output"
    );

    // And it is where the layer surface anchors it: top-left, margin 0. The
    // sample is the padding gutter left of the label (`label_x` is 10) on the
    // button's centre row, clear of the corner arcs and of the glyphs.
    let sample = pixel_at(&frame, 4, 13).expect("sample pixel inside the frame");
    assert!(
        matches_accent(sample),
        "pixel (4, 13) is {sample:?}, not the accent blue the top-left-anchored button \
         should be painting there"
    );
}
