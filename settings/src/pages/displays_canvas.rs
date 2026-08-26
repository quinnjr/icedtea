//! GTK-free geometry for the Displays drag canvas.
//!
//! The canvas draws each enabled head as a rectangle in *layout* coordinates
//! (the compositor's global space, in logical pixels) scaled down to fit the
//! drawing area. None of this touches GTK — it is pure arithmetic on plain
//! [`Rect`]s so the fit/hit-test/snap logic is unit-testable without a display.
//! [`super::displays`] wraps `cairo`/`GtkGestureDrag` around it.

/// An axis-aligned rectangle in an unspecified 2-D space (layout pixels for the
/// model side, canvas pixels once mapped through a [`View`]). `f64` throughout
/// so scaling never rounds mid-computation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Rect { x, y, w, h }
    }

    /// Whether `(px, py)` falls inside (edges inclusive on the near side).
    pub fn contains(&self, px: f64, py: f64) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
}

/// The logical on-desktop size a head occupies, given its mode pixels, scale,
/// and `wl_output.transform` value. A 90°/270° transform (raw 1, 3, 5, 7) swaps
/// width and height; the fractional scale divides both. Returned as a [`Rect`]
/// positioned at `(x, y)` in layout space.
pub fn head_rect(width: i32, height: i32, scale: f64, transform: i32, x: i32, y: i32) -> Rect {
    let s = if scale > 0.0 { scale } else { 1.0 };
    let mut w = width as f64 / s;
    let mut h = height as f64 / s;
    // 90/270 and their flipped variants rotate the output a quarter turn.
    if matches!(transform, 1 | 3 | 5 | 7) {
        std::mem::swap(&mut w, &mut h);
    }
    Rect::new(x as f64, y as f64, w, h)
}

/// The bounding box of a set of rectangles as `(min_x, min_y, max_x, max_y)`,
/// or `None` when the slice is empty.
pub fn content_bounds(rects: &[Rect]) -> Option<(f64, f64, f64, f64)> {
    let first = rects.first()?;
    let mut min_x = first.x;
    let mut min_y = first.y;
    let mut max_x = first.x + first.w;
    let mut max_y = first.y + first.h;
    for r in &rects[1..] {
        min_x = min_x.min(r.x);
        min_y = min_y.min(r.y);
        max_x = max_x.max(r.x + r.w);
        max_y = max_y.max(r.y + r.h);
    }
    Some((min_x, min_y, max_x, max_y))
}

/// A layout→canvas mapping: a single shared `scale` (canvas px per layout px)
/// plus a translation that centres the content in the viewport.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub scale: f64,
    pub off_x: f64,
    pub off_y: f64,
}

impl View {
    /// Map a layout-space rect into canvas space.
    pub fn to_canvas(&self, r: &Rect) -> Rect {
        Rect::new(
            r.x * self.scale + self.off_x,
            r.y * self.scale + self.off_y,
            r.w * self.scale,
            r.h * self.scale,
        )
    }

    /// Map a canvas-space delta back into layout space.
    pub fn canvas_delta_to_layout(&self, dx: f64, dy: f64) -> (f64, f64) {
        if self.scale > 0.0 {
            (dx / self.scale, dy / self.scale)
        } else {
            (dx, dy)
        }
    }
}

/// Compute the shared scale factor and centring translation that fits every
/// rect's bounding box into a `view_w × view_h` viewport, leaving `margin`
/// canvas pixels of padding on each side. Empty input yields an identity-ish
/// view so callers never divide by zero.
pub fn compute_view(rects: &[Rect], view_w: f64, view_h: f64, margin: f64) -> View {
    let Some((min_x, min_y, max_x, max_y)) = content_bounds(rects) else {
        return View {
            scale: 1.0,
            off_x: margin,
            off_y: margin,
        };
    };
    let avail_w = (view_w - 2.0 * margin).max(1.0);
    let avail_h = (view_h - 2.0 * margin).max(1.0);
    let content_w = (max_x - min_x).max(1.0);
    let content_h = (max_y - min_y).max(1.0);
    let mut scale = (avail_w / content_w).min(avail_h / content_h);
    if !scale.is_finite() || scale <= 0.0 {
        scale = 1.0;
    }
    let used_w = content_w * scale;
    let used_h = content_h * scale;
    // Centre the scaled content in the available area, then shift so the
    // content's own min corner lands at the padded origin.
    let off_x = margin + (avail_w - used_w) / 2.0 - min_x * scale;
    let off_y = margin + (avail_h - used_h) / 2.0 - min_y * scale;
    View {
        scale,
        off_x,
        off_y,
    }
}

/// Hit-test a canvas-space point against layout-space `rects` mapped through
/// `view`. Returns the index of the topmost (last, i.e. drawn-on-top) rect that
/// contains the point, or `None`.
pub fn hit_test(rects: &[Rect], view: &View, px: f64, py: f64) -> Option<usize> {
    for (i, r) in rects.iter().enumerate().rev() {
        if view.to_canvas(r).contains(px, py) {
            return Some(i);
        }
    }
    None
}

/// Edge-snap a dragged rect (in layout space) against `others` and the `0,0`
/// origin, returning the resulting layout `(x, y)` as integers.
///
/// For each axis the candidate positions are: the origin `0`, edge-alignment
/// with every neighbour (near-edge-to-near-edge and far-edge-to-far-edge), and
/// adjacency (the dragged rect's near edge butted against a neighbour's far
/// edge and vice-versa). The nearest candidate within `threshold` layout pixels
/// wins per axis; axes snap independently. When nothing is close enough the
/// dragged position is passed through (rounded).
pub fn snap(dragged: Rect, others: &[Rect], threshold: f64) -> (i32, i32) {
    let mut x_candidates = vec![0.0_f64];
    let mut y_candidates = vec![0.0_f64];
    for o in others {
        // Vertical edges → x snapping.
        x_candidates.push(o.x); // left edges aligned
        x_candidates.push(o.x + o.w - dragged.w); // right edges aligned
        x_candidates.push(o.x + o.w); // dragged sits to the right of o
        x_candidates.push(o.x - dragged.w); // dragged sits to the left of o
        // Horizontal edges → y snapping.
        y_candidates.push(o.y);
        y_candidates.push(o.y + o.h - dragged.h);
        y_candidates.push(o.y + o.h);
        y_candidates.push(o.y - dragged.h);
    }
    let snapped_x = nearest_within(dragged.x, &x_candidates, threshold).unwrap_or(dragged.x);
    let snapped_y = nearest_within(dragged.y, &y_candidates, threshold).unwrap_or(dragged.y);
    (snapped_x.round() as i32, snapped_y.round() as i32)
}

/// The candidate nearest `value` whose distance is `<= threshold`, or `None`.
fn nearest_within(value: f64, candidates: &[f64], threshold: f64) -> Option<f64> {
    candidates
        .iter()
        .copied()
        .map(|c| (c, (c - value).abs()))
        .filter(|(_, d)| *d <= threshold)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(c, _)| c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn head_rect_applies_scale_and_rotation() {
        // 3840x2160 at scale 2 → 1920x1080 logical.
        let r = head_rect(3840, 2160, 2.0, 0, 100, 50);
        assert_eq!(r, Rect::new(100.0, 50.0, 1920.0, 1080.0));
        // 90° transform swaps width/height.
        let rot = head_rect(1920, 1080, 1.0, 1, 0, 0);
        assert_eq!(rot, Rect::new(0.0, 0.0, 1080.0, 1920.0));
    }

    #[test]
    fn compute_view_fits_content_into_viewport() {
        // Two 1920-wide heads side by side → 3840 layout px total.
        let rects = [
            Rect::new(0.0, 0.0, 1920.0, 1080.0),
            Rect::new(1920.0, 0.0, 1920.0, 1080.0),
        ];
        let view = compute_view(&rects, 400.0, 300.0, 10.0);
        // Width is the binding dimension: (400-20)/3840.
        let expected = 380.0 / 3840.0;
        assert!(
            (view.scale - expected).abs() < 1e-9,
            "scale {} != {}",
            view.scale,
            expected
        );
        // The whole content must land inside the viewport.
        for r in &rects {
            let c = view.to_canvas(r);
            assert!(
                c.x >= 10.0 - 1e-6 && c.x + c.w <= 400.0 - 10.0 + 1e-6,
                "rect {c:?} escaped viewport"
            );
        }
        // Non-vacuous: a big desktop is scaled *down*, not left at 1.0.
        assert!(view.scale < 0.5);
    }

    #[test]
    fn compute_view_handles_empty() {
        let view = compute_view(&[], 400.0, 300.0, 10.0);
        assert_eq!(view.scale, 1.0);
    }

    #[test]
    fn hit_test_picks_the_right_head_and_topmost_on_overlap() {
        let rects = [
            Rect::new(0.0, 0.0, 1920.0, 1080.0),
            Rect::new(1920.0, 0.0, 1920.0, 1080.0),
        ];
        let view = compute_view(&rects, 400.0, 300.0, 10.0);
        // A point inside the second head's canvas rect resolves to index 1.
        let c1 = view.to_canvas(&rects[1]);
        let hit = hit_test(&rects, &view, c1.x + c1.w / 2.0, c1.y + c1.h / 2.0);
        assert_eq!(hit, Some(1));
        // A point inside the first head resolves to index 0.
        let c0 = view.to_canvas(&rects[0]);
        assert_eq!(hit_test(&rects, &view, c0.x + 1.0, c0.y + 1.0), Some(0));
        // Well outside everything → None.
        assert_eq!(hit_test(&rects, &view, 5.0, 5.0), None);
    }

    #[test]
    fn hit_test_topmost_wins_when_rects_overlap() {
        // Two rects sharing space; the later one is "on top".
        let rects = [
            Rect::new(0.0, 0.0, 100.0, 100.0),
            Rect::new(50.0, 50.0, 100.0, 100.0),
        ];
        let view = View {
            scale: 1.0,
            off_x: 0.0,
            off_y: 0.0,
        };
        // (60,60) is inside both; index 1 (drawn last) must win.
        assert_eq!(hit_test(&rects, &view, 60.0, 60.0), Some(1));
        // (10,10) is only inside index 0.
        assert_eq!(hit_test(&rects, &view, 10.0, 10.0), Some(0));
    }

    #[test]
    fn snap_butts_a_head_against_its_neighbour() {
        let neighbour = Rect::new(0.0, 0.0, 1920.0, 1080.0);
        // Dragged near the neighbour's right edge (1920) but 12px short.
        let dragged = Rect::new(1908.0, 3.0, 1920.0, 1080.0);
        let (x, y) = snap(dragged, &[neighbour], 30.0);
        assert_eq!(
            x, 1920,
            "should snap to adjacency at the neighbour's right edge"
        );
        assert_eq!(y, 0, "should snap to top-edge alignment");
    }

    #[test]
    fn snap_leaves_a_far_head_alone() {
        let neighbour = Rect::new(0.0, 0.0, 1920.0, 1080.0);
        // 500px away on both axes — well past the 30px threshold.
        let dragged = Rect::new(2420.0, 600.0, 1920.0, 1080.0);
        let (x, y) = snap(dragged, &[neighbour], 30.0);
        assert_eq!(
            (x, y),
            (2420, 600),
            "nothing within threshold: pass-through"
        );
    }

    #[test]
    fn snap_pulls_a_near_origin_head_to_zero() {
        let dragged = Rect::new(6.0, -4.0, 1920.0, 1080.0);
        let (x, y) = snap(dragged, &[], 30.0);
        assert_eq!((x, y), (0, 0), "near-origin head snaps to 0,0");
    }
}
