//! Input-method candidate popup placement.
//!
//! When an IME (`zwp_input_method_v2`) opens a candidate list, the crate hands
//! the compositor a [`wlr::InputPopupSurfaceId`] via the seat handler's
//! `new_popup_surface` callback (see the impl on `State` in `state.rs`). The
//! compositor owns where that popup sits: it creates the scene node with
//! [`wlr::Runtime::add_input_popup_in_band`], positions it under the focused
//! text input's cursor rectangle, and tells the popup which rectangle it was
//! anchored against so the IME can lay out its candidates.
//!
//! The geometry — "below the cursor, but kept on screen" — is a [pure
//! function](place_below_clamped) with its own unit tests, following the same
//! `render.rs` discipline the rest of the compositor's geometry uses: the
//! handler in `state.rs` does only the FFI plumbing and delegates every
//! coordinate decision here.

use icedtea_contract::Rectangle;

/// Where an input-method candidate popup of size `popup` should be placed,
/// given the text cursor `anchor` and the output geometry to stay within.
///
/// `anchor` MUST already be in output-local coordinates: the cursor rectangle
/// a client commits is in its surface's local space (text-input-v3), so the
/// caller translates it through the focused surface's content origin first
/// (see `State::new_popup_surface`). Only `anchor.x/y/height` are read (the
/// seat below the cursor's bottom edge); only `popup.width/height` are read
/// (`popup.x/y` are ignored); `anchor.width` is ignored.
///
/// The popup sits directly below the cursor rectangle — its top-left at the
/// anchor's left edge, one pixel below the anchor's bottom — which is where a
/// candidate list belongs relative to the character being composed. It is then
/// clamped so it stays wholly on `output`:
///
/// * If it would spill off the right edge, it slides left until its right edge
///   meets the output's right edge.
/// * If it would spill off the bottom (the common case near the bottom of the
///   screen), it flips to sit *above* the cursor rectangle instead, so the
///   candidate list never covers the text being composed and never falls off
///   screen.
/// * Left/top edges are clamped to the output origin last, so a popup wider or
///   taller than the output pins to the top-left rather than going negative.
///
/// `output` is `None` when the compositor has no output yet (nothing has been
/// created, or every output is disabled); with nowhere to clamp against, the
/// unclamped below-the-cursor position is returned as-is. `popup` with a
/// zero-or-negative size (the IME has not committed a buffer yet) is treated as
/// empty, so only the anchor position matters and clamping is a no-op.
///
/// Returned coordinates are output-local `(x, y)` suitable for
/// [`wlr::Runtime::set_node_position`].
pub fn place_below_clamped(
    anchor: Rectangle,
    popup: Rectangle,
    output: Option<Rectangle>,
) -> (i32, i32) {
    // Preferred spot: left-aligned with the cursor, just under it.
    // Saturating: the anchor is client-committed and unbounded, so extreme
    // values must clamp, never panic (debug) or wrap (release).
    let mut x = anchor.x;
    let mut y = anchor.y.saturating_add(anchor.height);

    let Some(out) = output else {
        // No output to clamp against — hand back the naive placement.
        return (x, y);
    };

    let pw = popup.width.max(0);
    let ph = popup.height.max(0);

    let out_right = out.x.saturating_add(out.width);
    let out_bottom = out.y.saturating_add(out.height);

    // Right spill: slide left so the popup's right edge meets the output's.
    if x.saturating_add(pw) > out_right {
        x = out_right.saturating_sub(pw);
    }
    // Bottom spill: flip above the cursor rather than let the list cover the
    // caret or run off the bottom edge.
    if y.saturating_add(ph) > out_bottom {
        y = anchor.y.saturating_sub(ph);
    }

    // Final origin clamp: never place off the top-left, even when the popup is
    // larger than the output (pin to the origin instead of going negative).
    x = x.max(out.x);
    y = y.max(out.y);

    (x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle {
        Rectangle {
            x,
            y,
            width: w,
            height: h,
        }
    }

    #[test]
    fn places_directly_below_the_cursor_when_it_fits() {
        // Cursor at (100,200) 2x16 on a roomy 1920x1080 output; a 120x80 popup.
        let (x, y) = place_below_clamped(
            rect(100, 200, 2, 16),
            rect(0, 0, 120, 80),
            Some(rect(0, 0, 1920, 1080)),
        );
        assert_eq!(
            (x, y),
            (100, 216),
            "left-aligned with the cursor, just below it"
        );
    }

    #[test]
    fn slides_left_off_the_right_edge() {
        // Cursor near the right edge; the 120-wide popup would spill past 1920.
        let (x, y) = place_below_clamped(
            rect(1900, 200, 2, 16),
            rect(0, 0, 120, 80),
            Some(rect(0, 0, 1920, 1080)),
        );
        assert_eq!(x, 1920 - 120, "right edge meets the output's right edge");
        assert_eq!(y, 216, "vertical placement unchanged");
    }

    #[test]
    fn flips_above_the_cursor_off_the_bottom_edge() {
        // Cursor near the bottom; below-the-cursor would run off 1080.
        let (x, y) = place_below_clamped(
            rect(100, 1040, 2, 16),
            rect(0, 0, 120, 80),
            Some(rect(0, 0, 1920, 1080)),
        );
        assert_eq!(x, 100, "horizontal placement unchanged");
        assert_eq!(y, 1040 - 80, "flipped to sit above the cursor rectangle");
    }

    #[test]
    fn pins_to_the_origin_when_larger_than_the_output() {
        // A popup taller/wider than a tiny output pins to (0,0) rather than
        // going negative in either axis.
        let (x, y) = place_below_clamped(
            rect(10, 10, 2, 16),
            rect(0, 0, 400, 400),
            Some(rect(0, 0, 200, 200)),
        );
        assert_eq!(
            (x, y),
            (0, 0),
            "clamped to the output origin, never negative"
        );
    }

    #[test]
    fn respects_a_non_zero_output_origin() {
        // Output that does not start at (0,0): clamps use its own edges.
        let (x, y) = place_below_clamped(
            rect(1500, 300, 2, 16),
            rect(0, 0, 120, 80),
            Some(rect(1000, 0, 800, 600)),
        );
        // 1500 + 120 = 1620 > 1800 (=1000+800)? No — fits. Stays at 1500.
        assert_eq!(x, 1500);
        assert_eq!(y, 316);
    }

    #[test]
    fn spills_against_a_non_zero_output_origin() {
        // Right spill is measured against the output's own right edge, not
        // the origin: 1750 + 120 = 1870 > 1800 (= 1000 + 800) → slide to 1680.
        // A (0,0)-relative clamp would park this at 1500 instead.
        let (x, y) = place_below_clamped(
            rect(1750, 300, 2, 16),
            rect(0, 0, 120, 80),
            Some(rect(1000, 0, 800, 600)),
        );
        assert_eq!(x, 1680);
        assert_eq!(y, 316);
    }

    #[test]
    fn oversized_popup_pins_to_a_non_zero_output_origin() {
        // A popup larger than an offset output pins to the output's origin,
        // not (0, 0).
        let (x, y) = place_below_clamped(
            rect(1010, 10, 2, 16),
            rect(0, 0, 400, 400),
            Some(rect(1000, 0, 200, 200)),
        );
        assert_eq!((x, y), (1000, 0));
    }

    #[test]
    fn naive_placement_when_no_output() {
        // No output to clamp against: just below the cursor, unclamped.
        let (x, y) = place_below_clamped(rect(100, 200, 2, 16), rect(0, 0, 120, 80), None);
        assert_eq!((x, y), (100, 216));
    }

    #[test]
    fn zero_size_popup_only_uses_the_anchor() {
        // IME has not committed a buffer yet: size <= 0 means clamping is inert
        // and only the below-the-cursor anchor position matters.
        let (x, y) = place_below_clamped(
            rect(100, 200, 2, 16),
            rect(0, 0, 0, 0),
            Some(rect(0, 0, 1920, 1080)),
        );
        assert_eq!((x, y), (100, 216));
    }

    #[test]
    fn negative_size_popup_only_uses_the_anchor() {
        // Uninitialized geometry reported as negative clamps exactly like
        // zero: without the `.max(0)` calls the spill comparisons would
        // invert and misplace the popup.
        let (x, y) = place_below_clamped(
            rect(100, 200, 2, 16),
            rect(0, 0, -50, -30),
            Some(rect(0, 0, 1920, 1080)),
        );
        assert_eq!((x, y), (100, 216));
    }
}
