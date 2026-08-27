//! The animation seam, end to end: a `transition: background-color` really
//! animates on a real compositor's output.
//!
//! Start and end colours are asserted; intermediate frames are not (the M2
//! spec's rule — on-screen timing is not reproducible enough to pin a colour
//! to a millisecond). What *is* asserted is that reaching the end took
//! roughly the declared duration, which is the only thing that distinguishes
//! an animation from a snap when both begin red and end blue.
//!
//! Self-contained on purpose: it does not share `tests/support/mod.rs`,
//! whose `allocation_of` helper belongs to the offscreen pixel gate.

use std::io::Write;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use icedtea_harness::{CapturedFrame, Compositor, ScreencopyClient, VirtualPointerClient};

/// A whole theme (never layered under Adwaita) declaring one long, linear,
/// unmistakable colour transition on hover.
///
/// Geometry is pinned so the sample point can be a constant: no border, no
/// radius, no padding, an 80x60 content-box minimum, so the border box starts
/// at the output origin and (10, 10) is inside it and clear of the centred
/// label.
const THEME: &str = "\
window { background-color: rgba(0, 0, 0, 0); }
button {
  min-width: 80px;
  min-height: 60px;
  padding: 0;
  border: 0 solid transparent;
  border-radius: 0;
  background-color: rgb(255 0 0);
  background-image: none;
  color: rgb(0 0 0);
  transition: background-color 2000ms linear;
}
button:hover { background-color: rgb(0 0 255); }
";

/// Where the background is sampled: inside the button, outside the label.
const SAMPLE: (u32, u32) = (10, 10);
/// Where the pointer is put to arm `:hover`: the button's centre.
const CENTRE: (f64, f64) = (40.0, 30.0);
/// The declared transition duration.
const DURATION: Duration = Duration::from_millis(2000);

struct Reaper(Child);

impl Drop for Reaper {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn spawn_button(socket: &str, theme_path: &std::path::Path) -> Reaper {
    Reaper(
        Command::new(env!("CARGO_BIN_EXE_themed-button"))
            .env("WAYLAND_DISPLAY", socket)
            .env("ICEDTEA_UI_THEME", theme_path)
            .env("ICEDTEA_UI_LABEL", "Click me")
            .env("ICEDTEA_UI_CLASSES", "")
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to spawn themed-button"),
    )
}

fn pixel(frame: &CapturedFrame) -> (u8, u8, u8) {
    icedtea_ui::shm::pixel_rgb(frame.format, &frame.bytes, frame.stride, SAMPLE.0, SAMPLE.1)
        .expect("sample pixel inside the captured frame")
}

fn close(a: u8, b: u8) -> bool {
    i32::from(a).abs_diff(i32::from(b)) <= 12
}

/// `rgb(255 0 0)` — the button's resting background.
fn is_start(px: (u8, u8, u8)) -> bool {
    close(px.0, 0xFF) && close(px.1, 0x00) && close(px.2, 0x00)
}

/// `rgb(0 0 255)` — `button:hover`'s background.
fn is_end(px: (u8, u8, u8)) -> bool {
    close(px.0, 0x00) && close(px.1, 0x00) && close(px.2, 0xFF)
}

/// Capture until `ready` accepts the sample pixel, pumping `pointer` so the
/// compositor keeps delivering, or until `timeout` passes.
fn capture_until(
    screencopy: &mut ScreencopyClient,
    mut pointer: Option<&mut VirtualPointerClient>,
    timeout: Duration,
    ready: impl Fn((u8, u8, u8)) -> bool,
) -> ((u8, u8, u8), Duration) {
    let started = Instant::now();
    let mut px = pixel(&screencopy.capture());
    while !ready(px) && started.elapsed() < timeout {
        std::thread::sleep(Duration::from_millis(25));
        if let Some(pointer) = pointer.as_deref_mut() {
            pointer.pump();
        }
        px = pixel(&screencopy.capture());
    }
    (px, started.elapsed())
}

/// The one on-screen animation proof: hovering starts a 2 s transition that
/// really takes about 2 s to arrive.
///
/// Mutation check: delete the `self.surface.frame(&self.qh, ())` request in
/// `LayerWindow::repaint` and the button stays red forever -- the "reached the
/// hover colour" assertion times out. Delete the `self.anim.restyle(..)` call
/// in `Button::restyle` and it snaps to blue in well under a second, failing
/// the elapsed-time assertion instead.
#[test]
fn a_background_color_transition_animates_on_a_real_compositor() {
    let mut theme = tempfile::NamedTempFile::new().expect("temp theme file");
    theme
        .write_all(THEME.as_bytes())
        .expect("writing the theme");
    theme.flush().expect("flushing the theme");

    let comp = Compositor::spawn();
    let (output_w, output_h) = comp.output_size();
    let (output_w, output_h) = (output_w as u32, output_h as u32);
    let _child = spawn_button(&comp.socket, theme.path());

    let mut screencopy = ScreencopyClient::spawn(&comp.socket);
    let (before, _) = capture_until(&mut screencopy, None, Duration::from_secs(10), is_start);
    assert!(
        is_start(before),
        "the button never reached the screen in its resting rgb(255 0 0): \
         sampled {before:?} at {SAMPLE:?}"
    );

    let mut pointer = VirtualPointerClient::spawn(&comp.socket);
    pointer.motion_absolute(CENTRE.0, CENTRE.1, output_w, output_h);
    pointer.frame();
    pointer.pump();

    let (after, elapsed) = capture_until(
        &mut screencopy,
        Some(&mut pointer),
        Duration::from_secs(10),
        is_end,
    );
    assert!(
        is_end(after),
        "the button never reached `button:hover`'s rgb(0 0 255): sampled \
         {after:?} after {elapsed:?}. Without a wl_surface.frame pump the \
         first repaint paints t = 0 and nothing ever advances the clock."
    );
    assert!(
        elapsed >= DURATION / 2,
        "the hover colour arrived after only {elapsed:?}: a {DURATION:?} \
         linear transition cannot finish that fast, so the change snapped \
         instead of animating"
    );
    assert!(
        elapsed <= Duration::from_secs(9),
        "the hover colour took {elapsed:?}, far longer than the declared \
         {DURATION:?}: frames are being dropped or the clock is not advancing"
    );
}
