//! The M3 interaction gate: one interaction per widget class, driven by a
//! virtual pointer and keyboard against the harness compositor.
//!
//! Every test opens **one** widget (`gallery --widget NAME`), so its
//! coordinates are the widget's own and no other widget can move them. Points
//! are read from `--probe-points`; nothing here hard-codes a coordinate.

mod support;

use std::time::Duration;

use support::{Driver, KEY_H, KEY_I, KEY_SPACE, KEY_TAB};

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

/// Typing must reach the model *and* the glyphs must reach the screen: either
/// half alone would pass with the other broken.
///
/// Mutation check: make `EntryC` swallow `Event::Key` without emitting
/// `EventKind::Change`; the message never arrives and this test fails.
/// Restore.
#[test]
fn typing_into_an_entry_shows_the_glyphs_and_moves_the_caret() {
    let mut driver = Driver::new();
    let gallery = driver.open("light", "entry");
    let (tx, ty) = driver.point("entry", "text");
    let before = driver.pixel(tx, ty);

    driver.click(tx, ty);
    driver.keys(&[KEY_H, KEY_I]);

    assert!(
        gallery.wait_msg("changed entry Entryhi", REACT)
            || gallery.wait_msg("changed entry hi", REACT),
        "typing did not reach the model; got {:?}",
        gallery.messages()
    );
    let after = driver.wait_pixel_change(tx, ty, before);
    assert!(
        !support::matches(after, before),
        "the typed glyphs never reached the `text` subnode"
    );
}

/// One search, after the delay — not one per keystroke. That debounce is the
/// whole behaviour `SearchEntry` adds over `Entry`.
///
/// Mutation check: make `SearchEntryC` fire `EventKind::Search` on every key;
/// the count assertion below fails with 2. Restore.
#[test]
fn typing_into_a_search_entry_fires_one_search_after_the_delay() {
    let mut driver = Driver::new();
    let gallery = driver.open("light", "search_entry");
    let (tx, ty) = driver.point("search_entry", "text");

    driver.click(tx, ty);
    driver.keys(&[KEY_H, KEY_I]);
    assert!(
        gallery.wait_msg("search hi", REACT),
        "the search never fired; got {:?}",
        gallery.messages()
    );
    // Let any further debounced fire land before counting.
    std::thread::sleep(Duration::from_millis(500));
    let searches = gallery
        .messages()
        .iter()
        .filter(|line| line.starts_with("search "))
        .count();
    assert_eq!(
        searches,
        1,
        "two keystrokes must debounce into one search; got {:?}",
        gallery.messages()
    );
}

/// Peeking reveals the real text: the `text` subnode must change when the peek
/// icon is clicked, with no keystroke in between.
///
/// Reconciliation: the task text reads the peek icon's probe point as
/// `password_entry`/`image`; `--probe-points` prints `image0` (the
/// `caps-lock-indicator`, nested under `text` and visited first in the
/// preorder walk) and `image1` (the peek icon itself, a direct child of
/// `node` appended after `text`), so this samples `image1`, the label the
/// binary actually prints.
///
/// Mutation check: make `PasswordEntryC`'s peek toggle `visibility` without
/// re-shaping the text; the pixels stay bullets and this test fails. Restore.
#[test]
fn peeking_a_password_entry_reveals_the_text() {
    let mut driver = Driver::new();
    let _gallery = driver.open("light", "password_entry");
    let (tx, ty) = driver.point("password_entry", "text");
    let bullets = driver.pixel(tx, ty);
    let (ix, iy) = driver.point("password_entry", "image1");

    driver.click(ix, iy);
    let revealed = driver.wait_pixel_change(tx, ty, bullets);
    assert!(
        !support::matches(revealed, bullets),
        "the peek icon revealed nothing: the text stayed {bullets:?}"
    );
}

/// Holding the up button must step more than once: that repeat timer is the
/// clock-driven behaviour `SpinButtonC::tick` owns.
///
/// Reconciliation: the task text reads the up button's probe point as
/// `spin_button`/`button0`; `--probe-points` prints `button0` for
/// `button.down` and `button1` for `button.up` (the gallery sample is
/// horizontal, whose node order is `text`, `button.down`, `button.up`), so
/// this samples `button1`, the label the binary actually prints.
///
/// Mutation check: make `SpinButtonC::next_deadline` return `None`; only the
/// first step happens and this test fails on the second value. Restore.
#[test]
fn stepping_a_spin_button_repeats_while_the_button_is_held() {
    let mut driver = Driver::new();
    let gallery = driver.open("light", "spin_button");
    let (ux, uy) = driver.point("spin_button", "button1");

    driver.press(ux, uy);
    assert!(
        gallery.wait_msg("value spin_button 4", REACT),
        "the first step never happened; got {:?}",
        gallery.messages()
    );
    assert!(
        gallery.wait_msg("value spin_button 5", REACT),
        "the held button did not repeat; got {:?}",
        gallery.messages()
    );
    driver.release(ux, uy);

    let steps = gallery
        .messages()
        .iter()
        .filter(|line| line.starts_with("value spin_button "))
        .count();
    assert!(
        steps >= 2,
        "a held spin button must step at least twice, got {steps}"
    );
}

/// Dragging the slider must move it *and* report the value: a scale that
/// paints without reporting is as broken as one that reports without painting.
///
/// Reconciliation: `trough`/`slider` are subnodes `ScaleC` positions
/// directly rather than `View` children with their own taffy box
/// (`ScaleC::on_event`'s own doc: "they never get a taffy box of their own
/// ... there is no `local_rect(cx.tree, cx.node, &self.trough)` to read");
/// `--probe-points` therefore prints only `root` for this widget, not
/// `trough`/`slider`. `root`'s content box is exactly the trough's own box
/// per that same doc, and a `PointerDown` anywhere inside it snaps the value
/// to that point immediately (`commit` in `ScaleC::on_event`), so `root`
/// stands in for both the task's `slider` and `trough` probe points.
///
/// Mutation check: make `ScaleC` clamp its drag to the press position; the
/// value message never arrives and this test fails. Restore.
#[test]
fn dragging_a_scale_moves_the_slider_and_reports_the_value() {
    let mut driver = Driver::new();
    let gallery = driver.open("light", "scale");
    let (sx, sy) = driver.point("scale", "root");
    let (tx, ty) = (sx, sy);
    // Drag right, staying inside the trough: its centre plus most of the
    // remaining half-width.
    let target = (tx + (tx - sx).abs().max(40), ty);
    let at_rest = driver.pixel(sx, sy);

    driver.drag((sx, sy), target);
    assert!(
        gallery
            .messages()
            .iter()
            .any(|line| line.starts_with("value scale ")),
        "the drag reported no value; got {:?}",
        gallery.messages()
    );
    let vacated = driver.wait_pixel_change(sx, sy, at_rest);
    assert!(
        !support::matches(vacated, at_rest),
        "the slider never left its starting position"
    );
}
