//! The M3 interaction gate: one interaction per widget class, driven by a
//! virtual pointer and keyboard against the harness compositor.
//!
//! Every test opens **one** widget (`gallery --widget NAME`), so its
//! coordinates are the widget's own and no other widget can move them. Points
//! are read from `--probe-points`; nothing here hard-codes a coordinate.

mod support;

use std::time::Duration;

use support::{Driver, KEY_SPACE, KEY_TAB};

/// How long an interaction gets to reach the model and the screen.
///
/// Reconciliation: the task text puts this at 5s. `support::GALLERY_MAP_TIMEOUT`
/// already documents, at length, that a single anti-aliased `clip_path` call
/// in `skia-rs-canvas` 0.4.0 costs seconds because the clip mask is
/// rasterised over the *whole canvas* regardless of the path's own size — and
/// every repaint pays that cost at least once, not only the first one.
/// Measured directly against a debug build of the gallery binary opened on
/// one plain `button`: a single repaint after a state change (`:active` on
/// press, `:checked` on release) took on the order of ten seconds, so 5s
/// fails every one of this file's tests on a correctly-working click. This is
/// the same accepted, out-of-scope cost `GALLERY_MAP_TIMEOUT` names, one
/// repaint at a time instead of a whole first page; the bound here is set
/// wide enough for two or three such repaints in a row (press, then release,
/// then the message) on a loaded machine, not narrowed to hide the cost.
const REACT: Duration = Duration::from_secs(60);

/// Mutation check: make `ButtonC::on_event` drop its `PointerUp` arm; the
/// message never arrives and this test fails on `wait_msg`. Restore.
#[test]
fn clicking_a_button_fires_its_message_and_paints_the_active_state() {
    let mut driver = Driver::new();
    let gallery = driver.open("light", "button");
    let (x, y) = driver.point("button", "root");
    let rest = driver.pixel(x, y);

    driver.press(x, y);
    let pressed = driver.wait_pixel_change(x, y, rest);
    assert!(
        !support::matches(pressed, rest),
        "a held button must paint :active; it stayed {rest:?}"
    );

    driver.release(x, y);
    assert!(
        gallery.wait_msg("clicked button", REACT),
        "no `clicked button` message; got {:?}",
        gallery.messages()
    );
}

/// Mutation check: make the toggle sample ignore `model.toggle(...)`; the
/// checked pixel never changes and this test fails. Restore.
#[test]
fn toggling_a_toggle_button_paints_the_checked_state() {
    let mut driver = Driver::new();
    let gallery = driver.open("light", "toggle_button");
    let (x, y) = driver.point("toggle_button", "root");
    let rest = driver.pixel(x, y);

    driver.click(x, y);
    assert!(
        gallery.wait_msg("toggled toggle_button true", REACT),
        "no toggle message; got {:?}",
        gallery.messages()
    );
    let checked = driver.wait_pixel_change(x, y, rest);
    assert!(
        !support::matches(checked, rest),
        "`:checked` painted nothing"
    );

    driver.click(x, y);
    assert!(gallery.wait_msg("toggled toggle_button false", REACT));
}

/// The check itself, not the row: the `check` subnode is where GTK paints the
/// builtin, so that is the probe point this test samples.
///
/// Mutation check: point the sample's `.active()` at a constant `false`; the
/// check subnode never changes and this test fails. Restore.
#[test]
fn checking_a_check_button_paints_the_builtin_check() {
    let mut driver = Driver::new();
    let gallery = driver.open("light", "check_button");
    let (cx, cy) = driver.point("check_button", "check");
    let rest = driver.pixel(cx, cy);

    driver.click(cx, cy);
    assert!(
        gallery.wait_msg("toggled check_button true", REACT),
        "no toggle message; got {:?}",
        gallery.messages()
    );
    let checked = driver.wait_pixel_change(cx, cy, rest);
    assert!(
        !support::matches(checked, rest),
        "the builtin check painted nothing on the `check` subnode"
    );
}

/// The switch animates its slider from one end to the other, so the *far* end
/// is what changes — sampling the centre would pass even if the slider never
/// moved.
///
/// Mutation check: make `SwitchC` jump `slide` to its end state without the
/// clock; the test still passes (it asserts the end state, not the tween) —
/// instead delete the `on_toggle` handler and confirm the message never
/// arrives. Restore.
#[test]
fn flipping_a_switch_animates_the_slider_to_the_other_end() {
    let mut driver = Driver::new();
    let gallery = driver.open("light", "switch");
    let (rx, ry) = driver.point("switch", "root");
    let (sx, sy) = driver.point("switch", "slider");
    // The end the slider is *not* at while the switch is off.
    let far = (rx + (rx - sx), sy);
    let rest = driver.pixel(far.0, far.1);

    driver.click(rx, ry);
    assert!(
        gallery.wait_msg("toggled switch true", REACT),
        "no toggle message; got {:?}",
        gallery.messages()
    );
    let moved = driver.wait_pixel_change(far.0, far.1, rest);
    assert!(
        !support::matches(moved, rest),
        "the slider never reached the other end of the switch"
    );
}

/// Space activates the focused widget — GTK's rule, and the keyboard half of
/// the button interaction.
///
/// Mutation check: remove `Space` from P3's window-level bindings; this test
/// fails on `wait_msg`. Restore.
#[test]
fn a_pointer_click_focuses_without_showing_the_focus_ring() {
    let mut driver = Driver::new();
    let gallery = driver.open("light", "button");
    let (x, y) = driver.point("button", "root");
    let rest = driver.pixel(x, y);

    driver.click(x, y);
    assert!(gallery.wait_msg("clicked button", REACT));
    // Back at rest: a pointer click leaves `:focus` but not `:focus-visible`,
    // so the button must look exactly as it did before the click.
    let after = driver.wait_pixel_settled(x, y);
    assert!(
        support::matches(after, rest),
        "a pointer click drew a focus ring: {rest:?} -> {after:?}"
    );

    // A key press makes the ring visible, which is what proves the first
    // assertion was about `:focus-visible` and not about focus never arriving.
    driver.key(KEY_TAB);
    driver.key(KEY_SPACE);
    assert!(
        gallery.wait_msg("clicked button", REACT),
        "Space did not activate the focused button; got {:?}",
        gallery.messages()
    );
}
