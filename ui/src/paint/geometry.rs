//! Rounded-rectangle path construction.
//!
//! `skia-rs-safe` 0.4.0 has an `RRect` value type but nothing that draws or
//! clips one: there is no `draw_rrect`, no `draw_drrect` and no
//! `clip_rrect`, and `PathBuilder::add_round_rect` takes a single uniform
//! `(rx, ry)`. CSS needs four independent elliptical corners, so every
//! rounded shape in this crate is built here, out of straight segments and
//! SVG-form elliptical arcs.

use skia_rs_safe::path::{FillType, Path, PathBuilder};

use crate::layout::Rect;

/// One edge of a box, for per-side border painting.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Side {
    /// The top edge.
    Top,
    /// The right edge.
    Right,
    /// The bottom edge.
    Bottom,
    /// The left edge.
    Left,
}

/// Replace a non-finite or negative number with zero.
fn sane(v: f32) -> f32 {
    if v.is_finite() && v > 0.0 { v } else { 0.0 }
}

/// Scale `radii` down so that no pair of corner radii overflows its edge.
///
/// CSS Backgrounds and Borders L3 §5.5: compute `f` as the minimum over the
/// four edges of `edge_length / (radius_a + radius_b)`, and if `f < 1`
/// multiply *every* radius by `f`. Scaling each corner independently would
/// change the shape's proportions, which CSS explicitly forbids.
#[must_use]
pub fn clamp_radii(rect: Rect, radii: [[f32; 2]; 4]) -> [[f32; 2]; 4] {
    let w = sane(rect.width);
    let h = sane(rect.height);
    let r = [
        [sane(radii[0][0]), sane(radii[0][1])],
        [sane(radii[1][0]), sane(radii[1][1])],
        [sane(radii[2][0]), sane(radii[2][1])],
        [sane(radii[3][0]), sane(radii[3][1])],
    ];

    let ratio = |edge: f32, a: f32, b: f32| -> f32 {
        let sum = a + b;
        if sum > 0.0 { edge / sum } else { f32::INFINITY }
    };
    let f = ratio(w, r[0][0], r[1][0])
        .min(ratio(w, r[3][0], r[2][0]))
        .min(ratio(h, r[0][1], r[3][1]))
        .min(ratio(h, r[1][1], r[2][1]));

    if f >= 1.0 || !f.is_finite() {
        return r;
    }
    [
        [r[0][0] * f, r[0][1] * f],
        [r[1][0] * f, r[1][1] * f],
        [r[2][0] * f, r[2][1] * f],
        [r[3][0] * f, r[3][1] * f],
    ]
}

/// Shrink `radii` by the adjacent side widths (top, right, bottom, left).
///
/// Each corner loses the width of the side it touches on that axis: the
/// top-left x-radius loses the left width, its y-radius the top width.
#[must_use]
pub fn inner_radii(radii: &[[f32; 2]; 4], sides: [f32; 4]) -> [[f32; 2]; 4] {
    let [top, right, bottom, left] = [
        sane(sides[0]),
        sane(sides[1]),
        sane(sides[2]),
        sane(sides[3]),
    ];
    let shrink =
        |r: [f32; 2], dx: f32, dy: f32| [(sane(r[0]) - dx).max(0.0), (sane(r[1]) - dy).max(0.0)];
    [
        shrink(radii[0], left, top),
        shrink(radii[1], right, top),
        shrink(radii[2], right, bottom),
        shrink(radii[3], left, bottom),
    ]
}

/// A closed path around `rect` with per-corner elliptical radii.
///
/// `radii` is `[TopLeft, TopRight, BottomRight, BottomLeft]`, each
/// `[rx, ry]`; the radii are clamped to the rect before use, so a caller may
/// pass raw computed values.
#[must_use]
pub fn rounded_rect_path(rect: Rect, radii: &[[f32; 2]; 4]) -> Path {
    let r = clamp_radii(rect, *radii);
    let x = if rect.x.is_finite() { rect.x } else { 0.0 };
    let y = if rect.y.is_finite() { rect.y } else { 0.0 };
    let w = sane(rect.width);
    let h = sane(rect.height);
    let (right, bottom) = (x + w, y + h);

    let mut b = PathBuilder::new();
    b.move_to(x + r[0][0], y);
    b.line_to(right - r[1][0], y);
    if r[1][0] > 0.0 && r[1][1] > 0.0 {
        b.arc_to(r[1][0], r[1][1], 0.0, false, true, right, y + r[1][1]);
    } else {
        b.line_to(right, y);
    }
    b.line_to(right, bottom - r[2][1]);
    if r[2][0] > 0.0 && r[2][1] > 0.0 {
        b.arc_to(r[2][0], r[2][1], 0.0, false, true, right - r[2][0], bottom);
    } else {
        b.line_to(right, bottom);
    }
    b.line_to(x + r[3][0], bottom);
    if r[3][0] > 0.0 && r[3][1] > 0.0 {
        b.arc_to(r[3][0], r[3][1], 0.0, false, true, x, bottom - r[3][1]);
    } else {
        b.line_to(x, bottom);
    }
    b.line_to(x, y + r[0][1]);
    if r[0][0] > 0.0 && r[0][1] > 0.0 {
        b.arc_to(r[0][0], r[0][1], 0.0, false, true, x + r[0][0], y);
    } else {
        b.line_to(x, y);
    }
    b.close();
    b.build()
}

/// The ring between two rounded rects, as one even-odd path.
///
/// There is no `draw_drrect` in `skia-rs`, so the two contours share a path
/// and the even-odd rule punches the hole.
#[must_use]
pub fn rounded_ring_path(
    outer: Rect,
    outer_radii: &[[f32; 2]; 4],
    inner: Rect,
    inner_radii: &[[f32; 2]; 4],
) -> Path {
    let mut b = PathBuilder::with_fill_type(FillType::EvenOdd);
    b.add_path(&rounded_rect_path(outer, outer_radii));
    b.add_path(&rounded_rect_path(inner, inner_radii));
    let mut path = b.build();
    path.set_fill_type(FillType::EvenOdd);
    path
}

/// The mitred trapezoid covering one side's share of a border ring.
///
/// Intersecting the ring with this wedge is how each side gets its own
/// width and colour while still meeting its neighbours on the diagonal,
/// exactly as GTK draws a mixed border. Stroking cannot express that.
#[must_use]
pub fn side_wedge_path(outer: Rect, inner: Rect, side: Side) -> Path {
    let ox = if outer.x.is_finite() { outer.x } else { 0.0 };
    let oy = if outer.y.is_finite() { outer.y } else { 0.0 };
    let or = ox + sane(outer.width);
    let ob = oy + sane(outer.height);
    let ix = if inner.x.is_finite() { inner.x } else { ox };
    let iy = if inner.y.is_finite() { inner.y } else { oy };
    let ir = ix + sane(inner.width);
    let ib = iy + sane(inner.height);

    let quad = match side {
        Side::Top => [(ox, oy), (or, oy), (ir, iy), (ix, iy)],
        Side::Right => [(or, oy), (or, ob), (ir, ib), (ir, iy)],
        Side::Bottom => [(or, ob), (ox, ob), (ix, ib), (ir, ib)],
        Side::Left => [(ox, ob), (ox, oy), (ix, iy), (ix, ib)],
    };

    let mut b = PathBuilder::new();
    b.move_to(quad[0].0, quad[0].1);
    for point in &quad[1..] {
        b.line_to(point.0, point.1);
    }
    b.close();
    b.build()
}

#[cfg(test)]
mod tests {
    use super::{
        Side, clamp_radii, inner_radii, rounded_rect_path, rounded_ring_path, side_wedge_path,
    };
    use crate::layout::Rect;
    use skia_rs_safe::core::Point;
    use skia_rs_safe::path::FillType;

    const SQUARE: [[f32; 2]; 4] = [[0.0, 0.0]; 4];

    #[test]
    fn a_square_rect_is_four_lines_and_its_bounds_are_the_rect() {
        // Mutation check: emitting a corner arc for a zero radius adds
        // verbs and rounds the bounds in.
        let path = rounded_rect_path(Rect::new(10.0, 20.0, 40.0, 30.0), &SQUARE);
        let bounds = path.bounds();
        assert_eq!(bounds.left, 10.0);
        assert_eq!(bounds.top, 20.0);
        assert_eq!(bounds.right, 50.0);
        assert_eq!(bounds.bottom, 50.0);
        assert!(
            path.contains(Point::new(30.0, 35.0)),
            "the interior is filled"
        );
    }

    #[test]
    fn a_rounded_corner_excludes_the_point_outside_its_arc() {
        // The M1 gate's "corner (0,0) is transparent past border-radius: 5px",
        // as pure geometry. Mutation check: dropping the arcs makes (1,1)
        // inside the path and the gate's corner assertion fails.
        let path = rounded_rect_path(Rect::new(0.0, 0.0, 40.0, 20.0), &[[5.0, 5.0]; 4]);
        assert!(
            !path.contains(Point::new(0.5, 0.5)),
            "inside a 5px corner arc"
        );
        assert!(
            path.contains(Point::new(20.0, 10.0)),
            "the centre is inside"
        );
        assert!(
            path.contains(Point::new(20.0, 0.5)),
            "the straight top edge is inside"
        );
    }

    #[test]
    fn radii_are_scaled_down_when_a_pair_overflows_its_edge() {
        // CSS Backgrounds L3 §5.5: f = min over all edges of edge/(r1+r2),
        // applied to EVERY radius when f < 1 -- even the three corners
        // whose own edge does not overflow.
        // Mutation check: scaling only the overflowing (bottom) pair by its
        // own edge ratio, instead of every corner by the global minimum
        // over all four edges, leaves the top-left corner at 5.0 instead
        // of 2.5.
        let scaled = clamp_radii(
            Rect::new(0.0, 0.0, 20.0, 20.0),
            [[5.0, 5.0], [15.0, 15.0], [10.0, 10.0], [30.0, 30.0]],
        );
        assert_eq!(scaled[0], [2.5, 2.5]);
        assert_eq!(scaled[1], [7.5, 7.5]);
        assert_eq!(scaled[2], [5.0, 5.0]);
        assert_eq!(scaled[3], [15.0, 15.0]);
    }

    #[test]
    fn inner_radii_shrink_by_the_adjacent_side_widths_and_floor_at_zero() {
        // CSS: the inner curve's radius is the outer radius minus the
        // adjacent border width, never negative.
        // Mutation check: subtracting the same side from rx and ry makes the
        // second assertion 3.0 instead of 1.0.
        let inner = inner_radii(
            &[[5.0, 5.0], [5.0, 5.0], [1.0, 1.0], [0.0, 0.0]],
            [4.0, 2.0, 3.0, 1.0],
        );
        assert_eq!(
            inner[0],
            [4.0, 1.0],
            "TL loses left width in x, top width in y"
        );
        assert_eq!(
            inner[1],
            [3.0, 1.0],
            "TR loses right width in x, top width in y"
        );
        assert_eq!(inner[2], [0.0, 0.0], "BR floors at zero");
        assert_eq!(inner[3], [0.0, 0.0]);
    }

    #[test]
    fn the_ring_between_two_rects_is_even_odd_and_hollow() {
        // Mutation check: leaving the fill type at Winding fills the hole,
        // so a 1px border would paint over the whole background.
        let ring = rounded_ring_path(
            Rect::new(0.0, 0.0, 40.0, 20.0),
            &SQUARE,
            Rect::new(2.0, 2.0, 36.0, 16.0),
            &SQUARE,
        );
        assert_eq!(ring.fill_type(), FillType::EvenOdd);
        assert!(ring.contains(Point::new(1.0, 10.0)), "the ring itself");
        assert!(
            !ring.contains(Point::new(20.0, 10.0)),
            "the hole is not filled"
        );
    }

    #[test]
    fn a_side_wedge_is_the_mitred_trapezoid_between_the_two_rects() {
        // Mutation check: emitting an axis-aligned rectangle instead of the
        // mitred trapezoid puts (1, 1) inside BOTH the top and the left
        // wedge, so adjacent border colours would overdraw each other.
        let outer = Rect::new(0.0, 0.0, 40.0, 20.0);
        let inner = Rect::new(4.0, 4.0, 32.0, 12.0);
        let top = side_wedge_path(outer, inner, Side::Top);
        let left = side_wedge_path(outer, inner, Side::Left);
        assert!(top.contains(Point::new(20.0, 1.0)));
        assert!(!top.contains(Point::new(1.0, 10.0)));
        assert!(left.contains(Point::new(1.0, 10.0)));
        assert!(!left.contains(Point::new(20.0, 1.0)));
        // The mitre corner: (1, 1) belongs to exactly one of the two.
        assert_ne!(
            top.contains(Point::new(1.5, 1.0)),
            left.contains(Point::new(1.5, 1.0))
        );
    }

    #[test]
    fn geometry_never_panics_on_hostile_numbers() {
        let hostile = [
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            -50.0,
            0.0,
            1.0e30,
        ];
        for &v in &hostile {
            let rect = Rect::new(v, v, v, v);
            let radii = [[v, v]; 4];
            let _ = clamp_radii(rect, radii);
            let _ = inner_radii(&radii, [v; 4]);
            let _ = rounded_rect_path(rect, &radii);
            let _ = rounded_ring_path(rect, &radii, rect, &radii);
            for side in [Side::Top, Side::Right, Side::Bottom, Side::Left] {
                let _ = side_wedge_path(rect, rect, side);
            }
        }
    }
}
