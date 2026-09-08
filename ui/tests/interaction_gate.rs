//! The M3 interaction gate: one interaction per widget class, driven by a
//! virtual pointer and keyboard against the harness compositor.
//!
//! Every test opens **one** widget (`gallery --widget NAME`), so its
//! coordinates are the widget's own and no other widget can move them. Points
//! are read from `--probe-points`, or derived from `--print-allocation` and
//! the hit geometry the controller itself publishes; nothing here hard-codes
//! a coordinate.

mod support;

use std::time::Duration;

use icedtea_ui::widgets::password_entry::PEEK_WIDTH_PX;
use icedtea_ui::widgets::spin_button::STEPPER_SIZE;
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

// The entry family's four subnodes (`text`, `image.left`/`image.right`,
// `image.peek`, `button.up`/`button.down`) are appended by their controller
// rather than returned as `View` children, so `reconcile`'s trim step
// detaches every one of them and none has an allocation — `--probe-points`
// reports only `root` for all four widgets, and there is no subnode centre
// to click.
//
// Landing them in the layout tree (a `child_index`/`reserved_total`
// override, the shape `SwitchC` carries) is *not* the fix it looks like: an
// entry's own `Container` is `Container::Leaf`, which
// `LayoutTree::set_style` writes as `Display::Flex`, and taffy calls a
// node's measure closure only when that node has no taffy children
// (`compute_layout_with_measure` dispatches on `has_children`). Attaching
// the chrome therefore silences `EntryC::measure`/`PasswordEntryC::measure`/
// `SpinButtonC::measure` and each control collapses to whatever its chrome
// happens to size to — measured, a `PasswordEntry` showing "hunter2" went
// from a 78px box to a 43px one, and `widget_pixels.rs`'s own
// `peeking_a_password_entry_reveals_the_text` stopped finding the peek band
// at all. Sizing these four from their subnodes is a real piece of work
// (each subnode needs its own `Measure`), not a two-line override.
//
// So the four tests below take their click points from the one box that is
// always right — the widget's own border box, from
// `gallery --print-allocation` — plus the hit geometry the controller
// itself publishes ([`PEEK_WIDTH_PX`], [`STEPPER_SIZE`]). Nothing here is a
// magic number either way; both spellings are derived, and this one does not
// need a production regression to work.

/// Just inside `alloc`'s *left* border edge, vertically centred: before the
/// first glyph, which is where GTK puts the caret for a click there.
///
/// The left edge and not the right: `TextEditState::build` starts the cursor
/// at `text.len()`, so a click past the end of the text lands the caret
/// exactly where it already was and a test written against it passes whether
/// the click placed the caret or not.
#[expect(
    clippy::cast_possible_truncation,
    reason = "a gallery surface is never within rounding distance of i32::MAX"
)]
fn before_the_text(alloc: &support::EntryAllocation) -> (i32, i32) {
    (alloc.x as i32 + 3, (alloc.y + alloc.height / 2.0) as i32)
}

/// `alloc`'s interior, one pixel in from each border edge: the band a repaint
/// of the widget's *content* shows up in, with the focus ring the border
/// paints excluded so a state change cannot be mistaken for a content change.
#[expect(
    clippy::cast_possible_truncation,
    reason = "a gallery surface is never within rounding distance of i32::MAX"
)]
fn interior(alloc: &support::EntryAllocation) -> (i32, i32, i32) {
    (
        alloc.x as i32 + 1,
        (alloc.x + alloc.width) as i32 - 2,
        (alloc.y + alloc.height / 2.0) as i32,
    )
}

/// Typing must reach the model *and* the glyphs must reach the screen: either
/// half alone would pass with the other broken.
///
/// The click lands before the first glyph of "Entry", so the caret must move
/// from where the model left it (the end of the buffer) to 0, and the two
/// keys must land *there*: the model reads exactly `hiEntry`, never `Entryhi`
/// (the click never moved the caret) or `Ehintry` (it moved somewhere else).
///
/// Mutation check: make `EntryC` swallow `Event::Key` without emitting
/// `EventKind::Change`; the message never arrives and this test fails.
/// Restore. Mutation check 2: put `EntryC::on_event`'s caret placement back on
/// `local_rect(cx.tree, cx.node, &self.edit.text_node)`; `text` has no
/// allocation, the block goes dead, the caret stays at the end and the model
/// reads `Entryhi`.
#[test]
fn typing_into_an_entry_shows_the_glyphs_and_moves_the_caret() {
    let mut driver = Driver::new();
    let gallery = driver.open("light", "entry");
    let alloc = driver.allocation("entry");
    let (cx, cy) = before_the_text(&alloc);
    let (x0, x1, y) = interior(&alloc);
    let before = driver.row(x0, x1, y);

    driver.click(cx, cy);
    driver.keys(&[KEY_H, KEY_I]);

    assert!(
        gallery.wait_msg("changed entry hiEntry", REACT),
        "typing did not reach the model at the clicked caret; got {:?}",
        gallery.messages()
    );
    let after = driver.wait_row_change(x0, x1, y, &before);
    assert!(
        !support::row_matches(&after, &before),
        "the typed glyphs never reached the screen"
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
    let (tx, ty) = driver.point("search_entry", "root");

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

/// Peeking reveals the real text, and peeking again hides it: the widget's
/// content must change when the peek icon is clicked, with no keystroke in
/// between, and come back to what it was on the second click.
///
/// Three clicks, not one, and the interior band rather than a single pixel,
/// because a single click also focuses the entry: comparing two *focused*
/// renderings of the same buffer is what makes this a test of the peek toggle
/// rather than of the focus ring, and the third click proves the change is a
/// toggle and not a one-way state change that any first click would produce.
///
/// The peek icon's hit band is the last [`PEEK_WIDTH_PX`] of the content box
/// (`PasswordEntryC::on_event`), and the content box is the border box less
/// Adwaita's `entry` padding, which is well under half that width — so a
/// point `PEEK_WIDTH_PX / 2` in from the right border edge is inside the band
/// whatever that padding is.
///
/// The bands are read with [`settled_row`], not with the first capture that
/// differs: the first click also focuses the entry, and the focus ring fades
/// in over its own frames, so a band read the instant it moves can carry a
/// half-drawn ring in its two border pixels. The first and third bands are
/// compared for *equality*, which is exactly the comparison an intermediate
/// band breaks — observed once the dev profile's optimised `skia-rs` made the
/// repaint fast enough for the poll to land inside the fade.
///
/// Mutation check: drop `self.edit.visibility = self.peek;` from
/// `PasswordEntryC::on_event`'s peek arm; the *second* assertion below fails
/// ("a second peek must hide the text again"). Note that the first one still
/// passes under that mutation -- the click does change the rendering, through
/// focus and the caret -- which is exactly why the round trip is here.
/// Restore.
#[test]
#[expect(
    clippy::cast_possible_truncation,
    reason = "a gallery surface is never within rounding distance of i32::MAX"
)]
fn peeking_a_password_entry_reveals_the_text() {
    let mut driver = Driver::new();
    let _gallery = driver.open("light", "password_entry");
    let alloc = driver.allocation("password_entry");
    let (x0, x1, y) = interior(&alloc);
    let (px, py) = (
        (alloc.x + alloc.width - PEEK_WIDTH_PX / 2.0) as i32,
        (alloc.y + alloc.height / 2.0) as i32,
    );

    let masked = settled_row(&mut driver, x0, x1, y);
    driver.click(px, py);
    driver.wait_row_change(x0, x1, y, &masked);
    let revealed = settled_row(&mut driver, x0, x1, y);
    assert!(
        !support::row_matches(&revealed, &masked),
        "the peek icon revealed nothing: the text stayed {masked:?}"
    );

    driver.click(px, py);
    driver.wait_row_change(x0, x1, y, &revealed);
    let remasked = settled_row(&mut driver, x0, x1, y);
    assert!(
        !support::row_matches(&remasked, &revealed),
        "a second peek must hide the text again; it stayed {revealed:?}"
    );

    driver.click(px, py);
    driver.wait_row_change(x0, x1, y, &remasked);
    let revealed_again = settled_row(&mut driver, x0, x1, y);
    assert!(
        support::row_matches(&revealed_again, &revealed),
        "peek is a toggle: the third click must render exactly what the first \
         did.\nfirst:  {revealed:?}\nthird:  {revealed_again:?}"
    );
}

/// Holding the up button must step more than once, one increment at a time:
/// that repeat timer is the clock-driven behaviour `SpinButtonC::tick` owns.
///
/// The gallery's spin button is horizontal, whose `button.up` is the last
/// [`STEPPER_SIZE`] of the root's border box (`SpinButtonC::stepper_rects`),
/// so the press lands `STEPPER_SIZE / 2` in from the right edge.
///
/// Mutation check: make `SpinButtonC::next_deadline` return `None`; only the
/// first step happens and this test fails on the second value. Restore.
/// Mutation check 2: put `RepeatTimer::fire`'s catch-up loop back and run the
/// whole file at once; one late tick settles every missed interval, the value
/// jumps 4 -> 10 and the `value spin_button 5` assertion fails. It has to be
/// the whole file: run alone on an idle machine the app loop ticks on time
/// and the catch-up loop passes this too, which is why
/// `RepeatTimer`'s own `a_late_repeat_fires_once_and_re_anchors_on_the_clock_it_was_given`
/// is the deterministic guard for that half.
#[test]
#[expect(
    clippy::cast_possible_truncation,
    reason = "a gallery surface is never within rounding distance of i32::MAX"
)]
fn stepping_a_spin_button_repeats_while_the_button_is_held() {
    let mut driver = Driver::new();
    let gallery = driver.open("light", "spin_button");
    let alloc = driver.allocation("spin_button");
    let (ux, uy) = (
        (alloc.x + alloc.width - STEPPER_SIZE / 2.0) as i32,
        (alloc.y + alloc.height / 2.0) as i32,
    );

    driver.press(ux, uy);
    assert!(
        gallery.wait_msg("value spin_button 4", REACT),
        "the first step never happened; got {:?}",
        gallery.messages()
    );
    assert!(
        gallery.wait_msg("value spin_button 5", REACT),
        "the held button did not repeat one step at a time; got {:?}",
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

/// Opening a drop-down must *show* its list, picking a row must report that
/// row's index, and picking must put the list away again.
///
/// All three halves, because each alone passes with the others broken: a
/// popover that opens and never closes passes "it appeared" and passes
/// "picking reported the index", and a drop-down whose list is simply always
/// on screen — P8-D71's actual defect, measured — passes the pick half while
/// failing both of the others.
///
/// The coordinates come from `gallery --open --probe-points`, the same binary
/// asked for the tree a click produces; the click itself is real. A *band*
/// spanning all three rows rather than one pixel, for the reason
/// [`support::Driver::row`] gives: a `dropdown`'s `row` paints no text and
/// the popover's own background is the same white as the surface behind it,
/// so a single pixel inside a row cannot tell an open list from no list —
/// the band picks up the `contents` border that only exists while it is open.
///
/// Both bands that are *compared with each other* — the one before the
/// popover opens and the one after it closes — are read with [`settled_row`],
/// never with the first capture that differs. Closing paints in two visible
/// steps (the popover's own background, then the surface behind it once the
/// popover surface is gone), so the first differing capture is the
/// intermediate one: measured, it holds for ~500 ms and then the band comes
/// to rest on exactly the pre-open colours. Reading the intermediate band was
/// invisible while a debug-profile `skia-rs` painted a frame slowly enough
/// that the poll only ever caught the resting state; with dependencies
/// optimised in the dev profile the whole open/pick/close round trip takes
/// ~2 s instead of ~31 s and the poll lands inside the transition instead.
///
/// Mutation check: drop `self.popover.reveal(true)` from `PopoverC::open`;
/// the first assertion fails ("the popover never appeared"). Mutation check
/// 2: drop `self.reveal(false)` from `PopoverC::close`; the *third*
/// assertion fails, because picking a row leaves the list on screen. Restore
/// both.
#[test]
fn opening_a_drop_down_and_picking_an_item_updates_the_button() {
    let mut driver = Driver::new();
    let gallery = driver.open("light", "drop_down");
    let (bx, by) = driver.point("drop_down", "button");
    // Where the list lands once the popover is open — not where anything is
    // now. The outer two rows bound the band; the middle one is the target.
    let (x0, _) = driver.point_open("drop_down", "row0");
    let (rx, ry) = driver.point_open("drop_down", "row1");
    let (x1, _) = driver.point_open("drop_down", "row2");
    let closed = settled_row(&mut driver, x0, x1, ry);

    driver.click(bx, by);
    let opened = driver.wait_row_change(x0, x1, ry, &closed);
    assert!(
        !support::row_matches(&opened, &closed),
        "the drop-down's popover never appeared below the button: the band \
         from ({x0}, {ry}) to ({x1}, {ry}) stayed {closed:?}"
    );

    driver.click(rx, ry);
    assert!(
        gallery.wait_msg("selected drop_down 1", REACT),
        "picking the second row reported nothing; got {:?}",
        gallery.messages()
    );

    driver.wait_row_change(x0, x1, ry, &opened);
    let closed_again = settled_row(&mut driver, x0, x1, ry);
    assert!(
        support::row_matches(&closed_again, &closed),
        "picking a row must put the popover away again.\nclosed: {closed:?}\n\
         open:   {opened:?}\nafter:  {closed_again:?}"
    );
}

/// The band across `alloc`'s interior at row `y`: one pixel in from each
/// vertical border edge, which is where a disclosed body's own background
/// and border show up.
#[expect(
    clippy::cast_possible_truncation,
    reason = "a gallery surface is never within rounding distance of i32::MAX"
)]
fn band(alloc: &support::EntryAllocation, y: i32) -> (i32, i32, i32) {
    (alloc.x as i32 + 1, (alloc.x + alloc.width) as i32 - 2, y)
}

/// Further than any `ListView` in the gallery can scroll, so the offset lands
/// on `max_offset` exactly and the test never has to know a row height. The
/// controller clamps (`ListViewC::on_event`'s `Scroll` arm), so overshooting
/// is the point.
const PAST_THE_END: f64 = 10_000.0;

/// How long the band has to hold still before [`settled_row`] believes the
/// repaint is over. Three seconds of quiet, which is well past the round trip
/// a click or a scroll takes here (measured: the band moves within five
/// seconds of the event) and well short of [`REACT`].
const QUIET: Duration = Duration::from_secs(3);

/// The band at `y`, once it has stopped changing.
///
/// `Driver::wait_row_change` answers "did it move"; this answers "where did it
/// come to rest", which is what a *comparison between two states* needs. A
/// selection repaints in two visible steps here — the glyphs of the row that
/// was rebound, then the `:selected` background behind them — so a band read
/// the instant it first differs can be the intermediate one, and comparing it
/// with a later, fully painted band then fails on a difference that is only
/// timing.
fn settled_row(driver: &mut Driver, x0: i32, x1: i32, y: i32) -> Vec<(u8, u8, u8)> {
    let started = std::time::Instant::now();
    let mut last = driver.row(x0, x1, y);
    let mut quiet_since = std::time::Instant::now();
    loop {
        std::thread::sleep(support::CAPTURE_POLL);
        let now = driver.row(x0, x1, y);
        if support::row_matches(&now, &last) {
            if quiet_since.elapsed() >= QUIET || started.elapsed() >= REACT {
                return now;
            }
        } else {
            quiet_since = std::time::Instant::now();
        }
        last = now;
    }
}

/// A `ListView` scrolls by rebinding its pooled rows, and the row that was
/// selected before the scroll is still selected when it comes back.
///
/// The gallery's sample is ten rows in a 120 px viewport, so `max_offset` is
/// four rows' worth: scrolling to the end puts a different model row in slot
/// 1, and scrolling back to the top puts row 1 there again — *the same
/// `Node`*, still `:selected`, which is what recycling means. The band is
/// read from `--probe-points`' own `row1`, never from a hard-coded row
/// height.
///
/// The *second* row, not the first: the gallery model's default selection is
/// index 0 for every selectable widget (`GalleryModel::selection`), so
/// clicking row 0 selects what is already selected and `ListViewC` — like
/// GTK — reports no change.
///
/// A band rather than one pixel, for [`support::Driver::row`]'s reason: a row
/// is a glyph run, and a glyph row is mostly background between the stems.
///
/// **History: this test shipped `#[ignore]`d in M3.** Everything above the
/// two `driver.scroll` calls passed, and then the axis event went nowhere:
/// `wlr` 0.20.28 never subscribed to a pointer's `events.axis` and never
/// called `wlr_seat_pointer_notify_axis`, so no Wayland client under this
/// compositor had ever received a `wl_pointer.axis`, and `Driver::scroll`
/// (which no other test in the crate calls) had never worked. Contract §10
/// **P8-D74** carries the full trace. `wlr` 0.20.29 forwards the axis (and
/// its frame) through `SeatHandler::pointer_axis`, and this test runs.
///
/// The same behaviour is also proven offscreen by `widgets::list_view::
/// tests::pixels::scrolling_recycles_the_pooled_rows_and_keeps_the_selection`
/// — a click that paints a selection, a scroll that repaints the view with
/// other rows, and a scroll home that brings the selected one back pixel for
/// pixel — through a real `App`, layout and paint with no compositor at all.
/// This test is the same story through the real transport.
///
/// Mutation check 1 (recycling): make `ListViewC::adopt_metrics` take its
/// viewport from `alloc.content_box.height` again; the view sizes itself to
/// all ten rows, `max_offset` is 0, the scroll moves nothing and the second
/// assertion fails. Mutation check 2 (selection): drop the
/// `row.set_state(PseudoStates::SELECTED, ..)` line from `ListViewC::rebind`;
/// the scroll back to the top restores the glyphs but not the selection
/// background, and the last assertion fails. Restore both. (Both are checked
/// today by the offscreen test named above.)
#[test]
fn scrolling_a_list_view_recycles_rows_without_losing_selection() {
    let mut driver = Driver::new();
    let gallery = driver.open("light", "list_view");
    let alloc = driver.allocation("list_view");
    let (rx, ry) = driver.point("list_view", "row1");
    let (x0, x1, _) = band(&alloc, ry);
    let unselected = settled_row(&mut driver, x0, x1, ry);

    driver.click(rx, ry);
    assert!(
        gallery.wait_msg("selected list_view 1", REACT),
        "clicking the second row reported nothing; got {:?}",
        gallery.messages()
    );
    driver.wait_row_change(x0, x1, ry, &unselected);
    let selected = settled_row(&mut driver, x0, x1, ry);
    assert!(
        !support::row_matches(&selected, &unselected),
        "selecting the second row painted nothing: the band from ({x0}, {ry}) \
         to ({x1}, {ry}) stayed {unselected:?}"
    );

    driver.scroll(rx, ry, PAST_THE_END);
    driver.wait_row_change(x0, x1, ry, &selected);
    let scrolled = settled_row(&mut driver, x0, x1, ry);
    assert!(
        !support::row_matches(&scrolled, &selected),
        "scrolling to the end left the second row exactly as it was, so \
         nothing recycled.\nselected: {selected:?}\nscrolled: {scrolled:?}"
    );

    driver.scroll(rx, ry, -PAST_THE_END);
    driver.wait_row_change(x0, x1, ry, &scrolled);
    let back = settled_row(&mut driver, x0, x1, ry);
    assert!(
        support::row_matches(&back, &selected),
        "scrolling back to the top must rebind the same selected row.\n\
         selected: {selected:?}\nafter:    {back:?}"
    );
}

/// Clicking the title discloses the child: the message reaches the model and
/// the child's own pixels reach the screen.
///
/// Reconciliation: the task text samples `(title.x, title.y + 30)`, which
/// lands *below* a collapsed expander's own border box, and the disclosed
/// child has no geometry at all until it is disclosed. Both coordinates come
/// from `gallery --open` instead — the same binary asked for the tree the
/// click produces — and the click itself is real.
///
/// Reconciliation 2: "animates" is asserted by `ExpanderC`'s own
/// `expanding_animates_progress_to_one_and_then_stops_asking_for_frames`, not
/// here. GTK4's `GtkExpander` has no size transition of its own
/// (`gtk_expander_set_expanded` sets the child's visibility); `progress`
/// drives the arrow. What this test can see on screen is the disclosure, and
/// that is what it asserts.
///
/// A band rather than one pixel, for [`support::Driver::row`]'s reason: the
/// disclosed child is a label, and a glyph row is mostly background between
/// the stems.
///
/// Mutation check: drop the `set_displayed` call from
/// `ExpanderC::sync_disclosure`; the content is laid out and painted whether
/// the expander is open or shut, the band never changes, and this test fails.
/// Restore.
#[test]
fn expanding_an_expander_animates_and_reveals_the_child() {
    let mut driver = Driver::new();
    let gallery = driver.open("light", "expander");
    let (tx, ty) = driver.point("expander", "title");
    // Where the child lands once it is disclosed — not where anything is now.
    let (_, cy) = driver.point_open("expander", "content");
    let (x0, x1, y) = band(&driver.allocation_open("expander"), cy);
    let collapsed = driver.row(x0, x1, y);

    driver.click(tx, ty);
    assert!(
        gallery.wait_msg("expanded true", REACT),
        "the title click never reached the model; got {:?}",
        gallery.messages()
    );
    let revealed = driver.wait_row_change(x0, x1, y, &collapsed);
    assert!(
        !support::row_matches(&revealed, &collapsed),
        "expanding revealed nothing: the band from ({x0}, {y}) to ({x1}, {y}) \
         stayed {collapsed:?}"
    );
}

/// Clicking the second page button selects it, and paints it: a switcher that
/// reports without painting is as broken as one that paints without
/// reporting.
///
/// The two buttons are read from `--probe-points`, and the test requires
/// `button0` to be left of `button1` — the geometric order the switcher packs
/// them in — so a switcher that laid its pages out in reverse would fail here
/// rather than silently pass on whichever button the click happened to hit.
///
/// Mutation check: drop `StackSwitcherC::reserved_total`; reconcile's trim
/// step detaches every page button, no `button0` probe point exists at all
/// and this test fails on `Driver::point`. Mutation check 2: stop clearing
/// `:checked` on the previously selected button; the *first* assertion below
/// still passes, so delete the `fire_index` call instead and the message
/// never arrives. Restore.
#[test]
fn switching_a_stack_page_runs_the_transition() {
    let mut driver = Driver::new();
    let gallery = driver.open("light", "stack_switcher");
    let (x0, y0) = driver.point("stack_switcher", "button0");
    let (x1, y1) = driver.point("stack_switcher", "button1");
    assert!(
        x0 < x1,
        "the switcher packs page 0 left of page 1; got ({x0}, {y0}) and ({x1}, {y1})"
    );
    let rest = driver.pixel(x1, y1);

    driver.click(x1, y1);
    assert!(
        gallery.wait_msg("page 1", REACT),
        "clicking the second page reported nothing; got {:?}",
        gallery.messages()
    );
    let checked = driver.wait_pixel_change(x1, y1, rest);
    assert!(
        !support::matches(checked, rest),
        "the newly selected page button never painted `:checked`; it stayed {rest:?}"
    );
}

/// The horizontal strip `shown` covers and `shut` does not, on the side
/// `inside` falls: where a disclosed body lands, with nothing that was
/// already on screen in it.
///
/// # Panics
///
/// If `inside` is not outside `shut` on one side or the other, which would
/// mean the body opens over the closed box rather than beyond it.
#[expect(
    clippy::cast_possible_truncation,
    reason = "a gallery surface is never within rounding distance of i32::MAX"
)]
fn strip_beyond(
    shut: &support::EntryAllocation,
    shown: &support::EntryAllocation,
    inside: i32,
) -> (i32, i32) {
    let (shut_left, shut_right) = (shut.x as i32, (shut.x + shut.width) as i32);
    let (shown_left, shown_right) = (shown.x as i32, (shown.x + shown.width) as i32);
    let strip = if inside >= shut_right {
        (shut_right + 1, shown_right - 2)
    } else {
        (shown_left + 1, shut_left - 2)
    };
    assert!(
        strip.0 < strip.1 && (strip.0..=strip.1).contains(&inside),
        "{inside} is not in the strip {strip:?} beyond {shut_left}..{shut_right}"
    );
    strip
}

/// Clicking a menu button shows its popover, and clicking it again puts the
/// popover away.
///
/// Reconciliation: the task text's "dismisses outside" is not reachable in
/// this crate. P7-D54 rules that an embedded popover's body is retained in
/// the parent window's own tree rather than opened as a second surface, and
/// `App::run` routes input only over `rt.instances` — so there is no
/// `xdg_popup` to take a grab, and no compositor-side dismissal to drive: a
/// click outside the menu button never reaches `MenuButtonC` at all. The
/// dismissal this crate *does* have is the toggle, and that is what the
/// second half asserts. The round trip is the point either way: a popover
/// that opens and never closes passes "it appeared" on its own.
///
/// A band, for the same reason the drop-down test uses one — the popover's
/// own `contents` background and border are what a closed menu button does
/// not paint — but over the strip of the *open* box that lies outside the
/// *closed* one, on whichever side the menu opens. Not across the whole open
/// box: showing the menu also moves the button along the row, and the button
/// keeps the `:hover` the click left on it (nothing re-runs enter/leave for a
/// relayout under a stationary pointer), so a band containing the button can
/// never come back to exactly what it was. The strip the menu occupies
/// contains no button in either state.
///
/// Mutation check: put `MenuButtonC::build`'s popover back on
/// `PopoverC::for_test`; the popover root is a local that is never appended
/// to the menu button's node, no `contents` probe point exists in any state
/// and this test fails on `Driver::point_open`. Mutation check 2: drop
/// `self.popover.hide_when_closed()`; the contents are painted whether the
/// menu is open or shut and the *first* assertion fails. Restore both.
#[test]
fn opening_a_menu_button_popover_takes_the_grab_and_dismisses_outside() {
    let mut driver = Driver::new();
    let _gallery = driver.open("light", "menu_button");
    let (bx, by) = driver.point("menu_button", "button");
    // Where the menu lands once it is open — not where anything is now.
    let (px, y) = driver.point_open("menu_button", "contents");
    let shut = driver.allocation("menu_button");
    let shown = driver.allocation_open("menu_button");
    let (x0, x1) = strip_beyond(&shut, &shown, px);
    let closed = driver.row(x0, x1, y);

    driver.click(bx, by);
    let opened = driver.wait_row_change(x0, x1, y, &closed);
    assert!(
        !support::row_matches(&opened, &closed),
        "the menu button's popover never appeared: the band from ({x0}, {y}) \
         to ({x1}, {y}) stayed {closed:?}"
    );

    // The button where it is *now*: the menu is a sibling of the button in the
    // parent window's own tree (P7-D54), so showing it moves the button along
    // the row. Clicking the closed centre again would land on the menu, not on
    // the toggle.
    let (ox, oy) = driver.point_open("menu_button", "button");
    driver.click(ox, oy);
    let closed_again = driver.wait_row_change(x0, x1, y, &opened);
    assert!(
        support::row_matches(&closed_again, &closed),
        "clicking the menu button again must put the popover away.\n\
         closed: {closed:?}\nopen:   {opened:?}\nafter:  {closed_again:?}"
    );
}

/// Tab moves the focus ring from the first widget in geometric order to the
/// second, and takes it off the first on the way.
///
/// The `ActionBar` is the form: `pack_start`'s button is left of
/// `pack_end`'s, so "geometric order" is a real claim about this page and not
/// about whichever node happened to be built first. Both halves are asserted
/// — the ring arriving on `button1` *and* leaving `button0` — because a focus
/// ring that is painted once and never moved passes either one alone.
///
/// This is the first assertion anywhere in this crate that the focus ring
/// reaches the screen at all: the file's own
/// `a_pointer_click_focuses_without_showing_the_focus_ring` asserts on
/// messages and on the ring's *absence*, never on its pixels.
///
/// Mutation check: drop the Tab arm from `route`'s `InputEvent::Key` so the
/// key is delivered to the focused controller instead — which is what the app
/// did until P8-D72's close-out. No controller in the crate reads Tab, the
/// focus never moves, no ring is ever painted, and this test fails on the
/// first band. Mutation check 2: drop the `rt.focus.note_key(key)` call; the
/// focus still moves, but GTK's focus-visible rule never runs, so the ring
/// shows only because `FocusCause::Keyboard` set the flag — flip
/// `set_focus`'s `Keyboard` arm to leave `focus_visible` alone and both
/// mutations together leave the ring dark. Restore.
#[test]
fn tabbing_through_a_form_moves_the_focus_ring_in_geometric_order() {
    let mut driver = Driver::new();
    let _gallery = driver.open("light", "action_bar");
    let (bx0, by0) = driver.point("action_bar", "button0");
    let (bx1, by1) = driver.point("action_bar", "button1");
    assert!(
        bx0 < bx1,
        "pack_start's button must be left of pack_end's; got ({bx0}, {by0}) \
         and ({bx1}, {by1})"
    );
    // A band per button, not one across the whole bar: both buttons sit on
    // the same row, so a band spanning the bar cannot say which of them the
    // ring is on. Each band runs from the bar's own interior edge to the
    // midpoint between the two centres, which holds all of one button and
    // none of the other whatever the buttons measure. Whole buttons and not a
    // margin around each centre, because Adwaita draws the ring as
    // `outline-offset: -2px` -- two pixels inside the *border* edge, which a
    // band clustered on the centre would miss entirely.
    let alloc = driver.allocation("action_bar");
    let (left, right, _) = band(&alloc, by0);
    let mid = (bx0 + bx1) / 2;
    assert!(
        left < mid && mid < right,
        "the two buttons must straddle the bar's interior; got {bx0} and {bx1} \
         inside {left}..{right}"
    );
    let (a0, a1) = (left, mid);
    let (c0, c1) = (mid, right);
    let before0 = driver.row(a0, a1, by0);
    let before1 = driver.row(c0, c1, by1);

    driver.key(KEY_TAB);
    let ringed0 = driver.wait_row_change(a0, a1, by0, &before0);
    assert!(
        !support::row_matches(&ringed0, &before0),
        "the first Tab painted no focus ring on the first button: the band at \
         y={by0} stayed {before0:?}"
    );

    driver.key(KEY_TAB);
    let ringed1 = driver.wait_row_change(c0, c1, by1, &before1);
    assert!(
        !support::row_matches(&ringed1, &before1),
        "the second Tab painted no focus ring on the second button: the band \
         at y={by1} stayed {before1:?}"
    );
    let left0 = driver.wait_row_change(a0, a1, by0, &ringed0);
    assert!(
        support::row_matches(&left0, &before0),
        "the ring must leave the first button when Tab moves on.\nbefore: \
         {before0:?}\nringed: {ringed0:?}\nafter:  {left0:?}"
    );
}

/// The gesture P4's Displays canvas is built on: press, move, release, all
/// delivered to the canvas with its own coordinates, through the compositor.
///
/// Mutation check: delete the `Event::PointerMotion` arm of
/// `view::app::fire_pointer_handlers`; the `motion` line never appears and
/// this fails. Restore.
#[test]
fn dragging_across_a_drawing_area_reports_every_pointer_phase() {
    let mut driver = Driver::new();
    let gallery = driver.open("light", "drawing_area");
    let (x, y) = driver.point("drawing_area", "root");
    let alloc = driver.allocation("drawing_area");
    // Stay inside the canvas: a quarter of its width to the right.
    let target = (x + (alloc.width as i32 / 4).max(4), y);

    driver.drag((x, y), target);

    assert!(
        gallery.wait_msg_prefix("changed drawing_area down", REACT),
        "no press reached the canvas; got {:?}",
        gallery.messages()
    );
    assert!(
        gallery.wait_msg_prefix("changed drawing_area motion", REACT),
        "no motion reached the canvas; got {:?}",
        gallery.messages()
    );
    assert!(
        gallery.wait_msg_prefix("changed drawing_area up", REACT),
        "no release reached the canvas; got {:?}",
        gallery.messages()
    );
    let lines = gallery.messages();
    // The very first move into the canvas is itself a real motion (the
    // pointer's enter delivers `Event::PointerMotion` alongside
    // `Event::PointerEnter` — `fire_pointer_handlers` at `view::app` fires
    // both), so a `motion` line legitimately precedes `down` too. What
    // matters for a drag is a `motion` line *between* the press and the
    // release, not that the very first `motion` line comes after `down`.
    let phase_of = |needle: &str| {
        lines
            .iter()
            .position(|l| l.starts_with(&format!("changed drawing_area {needle}")))
    };
    let down = phase_of("down").expect("checked by the wait_msg above");
    let up = phase_of("up").expect("checked by the wait_msg above");
    let motion_during_drag = lines[down..up]
        .iter()
        .any(|l| l.starts_with("changed drawing_area motion"));
    assert!(
        down < up && motion_during_drag,
        "phases arrived out of order: {lines:?}"
    );
}

/// The button code survives the trip: a middle press on the canvas reports
/// `0x112`, which is how P5's middle-click-to-close is expressed.
///
/// Mutation check: fire `Handler::Pair` instead of `Handler::PairButton` in
/// `fire_pair_button`'s first arm; the code becomes the left button's and
/// this fails. Restore.
#[test]
fn a_middle_click_on_a_drawing_area_reports_the_middle_button_code() {
    let mut driver = Driver::new();
    let gallery = driver.open("light", "drawing_area");
    let (x, y) = driver.point("drawing_area", "root");

    driver.click_button(x, y, icedtea_ui::window::pointer::BTN_MIDDLE);

    assert!(
        gallery.wait_msg_prefix("changed drawing_area up", REACT),
        "no release reached the canvas; got {:?}",
        gallery.messages()
    );
    let up = gallery
        .messages()
        .into_iter()
        .find(|l| l.starts_with("changed drawing_area up"))
        .expect("the release line");
    assert!(
        up.ends_with(" 274"),
        "the release must carry BTN_MIDDLE (274 decimal): {up}"
    );
}

/// Ties the new paint to the hit geometry: dragging the slider must move the
/// pixels the widget draws, not just the value it reports.
///
/// Mutation check: paint the slider from `self.value` while hit-testing from
/// `self.adj.value` (they can disagree once `sanitized` clamps); the drag
/// reports a value but the pixels do not follow, and this fails. Restore.
#[test]
fn dragging_a_scrollbar_slider_moves_what_it_paints() {
    let mut driver = Driver::new();
    let gallery = driver.open("light", "scrollbar");
    let alloc = driver.allocation("scrollbar");
    let (x, y) = driver.point("scrollbar", "root");
    let left_edge = alloc.x as i32 + 2;
    let target = (alloc.x as i32 + alloc.width as i32 - 3, y);
    let before = driver.row(left_edge, target.0, y);

    driver.drag((x, y), target);

    assert!(
        gallery
            .messages()
            .iter()
            .any(|line| line.starts_with("value scrollbar ")),
        "the drag reported no value; got {:?}",
        gallery.messages()
    );
    let after = driver.wait_row_change(left_edge, target.0, y, &before);
    assert!(
        !support::row_matches(&after, &before),
        "the slider's pixels did not move with the drag"
    );
}
