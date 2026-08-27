//! The animation seam, end to end: a `transition: background-color` really
//! animates on a real compositor's output.
//!
//! Start and end colours are asserted; intermediate frames are not (the M2
//! spec's rule — on-screen timing is not reproducible enough to pin a colour
//! to a millisecond). What *is* asserted is that reaching the end took
//! roughly the declared duration, which is the only thing that distinguishes
//! an animation from a snap when both begin red and end blue.

use std::io::Write;
use std::time::Duration;

use icedtea_harness::{Compositor, ScreencopyClient, VirtualPointerClient};

mod support;
use support::{capture_until, matches, spawn_themed_button_with_theme};

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

/// `rgb(255 0 0)` — the button's resting background.
fn is_start(px: (u8, u8, u8)) -> bool {
    matches(px, (0xFF, 0x00, 0x00))
}

/// `rgb(0 0 255)` — `button:hover`'s background.
fn is_end(px: (u8, u8, u8)) -> bool {
    matches(px, (0x00, 0x00, 0xFF))
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
    let _child = spawn_themed_button_with_theme(&comp.socket, theme.path(), "Click me", "");

    let mut screencopy = ScreencopyClient::spawn(&comp.socket);
    let (before, _) = capture_until(
        &mut screencopy,
        None,
        SAMPLE,
        Duration::from_secs(10),
        is_start,
    );
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
        SAMPLE,
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
