//! `-gtk-icon-source: builtin` shapes, as skia paths.
//!
//! GTK draws `checkbutton > check`, `radiobutton > radio`, the spin
//! button's steppers and the expander's arrow from code rather than from an
//! icon file, via `gtkcssimagebuiltin`. These are those shapes.
//!
//! **The geometry is a reconstruction, not a port.** GTK4's C source is not
//! available in this tree, so the proportions below come from the documented
//! behaviour and from what the shapes have to look like; the tests assert
//! *bands* (a pixel the tick must cross, a coverage fraction) rather than
//! constants. Tightening them means rendering the same widgets in a real
//! GTK4 client under the harness compositor and comparing — which is P8's
//! gallery gate's job, not this module's.
//!
//! Every shape is expressed in a unit box and scaled, so a 16 px check and a
//! 64 px one are the same drawing.

use skia_rs_safe::canvas::Canvas;
use skia_rs_safe::paint::Style;
use skia_rs_safe::path::{Path, PathBuilder};

use crate::css::value::Rgba;
use crate::layout::Rect;
use crate::paint::fill_paint;

/// A shape `-gtk-icon-source: builtin` can draw.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Builtin {
    /// A checkmark, stroked.
    Check,
    /// A checkbox's mixed state: a horizontal bar.
    CheckIndeterminate,
    /// A radio's dot: a small filled circle.
    Radio,
    /// A radio's mixed state: the same bar as [`Builtin::CheckIndeterminate`].
    RadioIndeterminate,
    /// A triangle pointing up.
    ArrowUp,
    /// A triangle pointing down.
    ArrowDown,
    /// A triangle pointing left.
    ArrowLeft,
    /// A triangle pointing right.
    ArrowRight,
    /// The expander's disclosure arrow: [`Builtin::ArrowRight`]'s shape,
    /// rotated by the caller's `-gtk-icon-transform` when expanded, exactly
    /// as GTK4 does it.
    Expander,
    /// A spin button's `+`.
    SpinPlus,
    /// A spin button's `−`.
    SpinMinus,
}

/// A drawable size: non-finite and non-positive collapse to zero, so every
/// coordinate derived from it stays finite.
fn sane(size: f32) -> f32 {
    if size.is_finite() && size > 0.0 {
        size
    } else {
        0.0
    }
}

/// A triangle inscribed in the unit box, pointing in one of four directions,
/// spanning `SPAN` of the box.
fn arrow(size: f32, dx: f32, dy: f32) -> Path {
    /// How much of the box the triangle spans.
    const SPAN: f32 = 0.65;
    let half = SPAN / 2.0;
    let mut b = PathBuilder::new();
    // The tip, then the two shoulders, each expressed as an offset from the
    // centre along (dx, dy) and its perpendicular.
    let (px, py) = (-dy, dx);
    let tip = (0.5 + dx * half, 0.5 + dy * half);
    let a = (0.5 - dx * half + px * half, 0.5 - dy * half + py * half);
    let c = (0.5 - dx * half - px * half, 0.5 - dy * half - py * half);
    b.move_to(tip.0 * size, tip.1 * size);
    b.line_to(a.0 * size, a.1 * size);
    b.line_to(c.0 * size, c.1 * size);
    b.close();
    b.build()
}

/// A horizontal bar across the middle third of the unit box.
fn bar(size: f32) -> Path {
    let mut b = PathBuilder::new();
    b.add_rect(&skia_rs_safe::core::Rect::from_xywh(
        0.2 * size,
        0.44 * size,
        0.6 * size,
        0.12 * size,
    ));
    b.build()
}

impl Builtin {
    /// The shape, in a `size × size` box at the origin.
    ///
    /// A non-finite or non-positive `size` yields an empty path rather than
    /// one with NaN bounds — a NaN clip poisons everything drawn after it.
    #[must_use]
    pub fn path(self, size: f32) -> Path {
        let size = sane(size);
        match self {
            Builtin::Check => {
                let mut b = PathBuilder::new();
                b.move_to(0.20 * size, 0.52 * size);
                b.line_to(0.43 * size, 0.74 * size);
                b.line_to(0.80 * size, 0.28 * size);
                b.build()
            }
            Builtin::CheckIndeterminate | Builtin::RadioIndeterminate => bar(size),
            Builtin::Radio => {
                let mut b = PathBuilder::new();
                b.add_circle(0.5 * size, 0.5 * size, 0.20 * size);
                b.build()
            }
            Builtin::ArrowUp => arrow(size, 0.0, -1.0),
            Builtin::ArrowDown => arrow(size, 0.0, 1.0),
            Builtin::ArrowLeft => arrow(size, -1.0, 0.0),
            Builtin::ArrowRight | Builtin::Expander => arrow(size, 1.0, 0.0),
            Builtin::SpinPlus => {
                let mut b = PathBuilder::new();
                b.add_rect(&skia_rs_safe::core::Rect::from_xywh(
                    0.15 * size,
                    0.44 * size,
                    0.70 * size,
                    0.12 * size,
                ));
                b.add_rect(&skia_rs_safe::core::Rect::from_xywh(
                    0.44 * size,
                    0.15 * size,
                    0.12 * size,
                    0.70 * size,
                ));
                b.build()
            }
            Builtin::SpinMinus => {
                let mut b = PathBuilder::new();
                b.add_rect(&skia_rs_safe::core::Rect::from_xywh(
                    0.15 * size,
                    0.44 * size,
                    0.70 * size,
                    0.12 * size,
                ));
                b.build()
            }
        }
    }

    /// Stroke width for the outline shapes, ~size/8.
    ///
    /// Zero for the filled shapes, which is how [`draw`](Self::draw) knows
    /// which paint style to use.
    #[must_use]
    pub fn stroke_width(self, size: f32) -> f32 {
        match self {
            Builtin::Check => sane(size) / 8.0,
            _ => 0.0,
        }
    }

    /// Draw the shape into `rect`, in `color`.
    ///
    /// The shape is square: it is inscribed in the largest square that fits
    /// `rect` and centred in it, as GTK's own icon rendering does.
    pub fn draw(self, canvas: &mut Canvas<'_>, rect: Rect, color: Rgba) {
        let side = sane(rect.width.min(rect.height));
        if side <= 0.0 || color.a <= 0.0 {
            return;
        }
        let x = if rect.x.is_finite() { rect.x } else { 0.0 };
        let y = if rect.y.is_finite() { rect.y } else { 0.0 };
        let ox = x + (sane(rect.width) - side) / 2.0;
        let oy = y + (sane(rect.height) - side) / 2.0;

        let path = self.path(side);
        let stroke = self.stroke_width(side);
        let mut paint = fill_paint(color);
        if stroke > 0.0 {
            paint.set_style(Style::Stroke);
            paint.set_stroke_width(stroke);
        }
        let save = canvas.save();
        canvas.translate(ox, oy);
        canvas.draw_path(&path, &paint);
        canvas.restore_to_count(save);
    }

    /// Parsed from a `builtin(<name>)` CSS value.
    ///
    /// The registry parses `-gtk-icon-source: builtin` as a bare keyword
    /// (`css/registry.rs:367-378`); *which* shape is the CSS node's business,
    /// so this exists for widgets that name one, and for the gallery.
    #[must_use]
    pub fn from_css_name(name: &str) -> Option<Builtin> {
        let all = [
            Builtin::Check,
            Builtin::CheckIndeterminate,
            Builtin::Radio,
            Builtin::RadioIndeterminate,
            Builtin::ArrowUp,
            Builtin::ArrowDown,
            Builtin::ArrowLeft,
            Builtin::ArrowRight,
            Builtin::Expander,
            Builtin::SpinPlus,
            Builtin::SpinMinus,
        ];
        all.into_iter()
            .find(|builtin| name.eq_ignore_ascii_case(builtin.css_name()))
    }

    /// The name [`from_css_name`](Self::from_css_name) accepts.
    #[must_use]
    pub const fn css_name(self) -> &'static str {
        match self {
            Builtin::Check => "check",
            Builtin::CheckIndeterminate => "check-inconsistent",
            Builtin::Radio => "option",
            Builtin::RadioIndeterminate => "option-inconsistent",
            Builtin::ArrowUp => "arrow-up",
            Builtin::ArrowDown => "arrow-down",
            Builtin::ArrowLeft => "arrow-left",
            Builtin::ArrowRight => "arrow-right",
            Builtin::Expander => "expander",
            Builtin::SpinPlus => "spin-plus",
            Builtin::SpinMinus => "spin-minus",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Builtin;
    use crate::css::value::Rgba;
    use crate::layout::Rect;
    use skia_rs_safe::canvas::Surface;
    use skia_rs_safe::core::Color;

    const BLACK: Rgba = Rgba {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    };

    /// Draw one builtin into a 16x16 transparent surface.
    fn drawn(builtin: Builtin) -> Surface {
        let mut surface = Surface::new_raster_n32_premul(16, 16).expect("surface");
        {
            let mut canvas = surface.canvas();
            canvas.clear(Color::TRANSPARENT);
            builtin.draw(&mut canvas, Rect::new(0.0, 0.0, 16.0, 16.0), BLACK);
        }
        surface
    }

    fn painted(surface: &Surface, x: i32, y: i32) -> bool {
        surface
            .pixel_buffer()
            .get_pixel(x, y)
            .is_some_and(|color| (color.0 >> 24) & 0xFF > 40)
    }

    /// The fraction of the 16x16 box with any coverage at all.
    fn coverage(surface: &Surface) -> f32 {
        let mut hits = 0;
        for y in 0..16 {
            for x in 0..16 {
                if painted(surface, x, y) {
                    hits += 1;
                }
            }
        }
        hits as f32 / 256.0
    }

    // Tolerance, not a constant: the geometry is a reconstruction of
    // gtkcssimagebuiltin's, and the numbers below are bands wide enough to
    // survive a later tightening against a real GTK render.
    // Mutation check: return an empty path from `Builtin::Check` and both
    // assertions fail.
    #[test]
    fn the_check_is_a_tick_across_the_middle_of_the_box() {
        let surface = drawn(Builtin::Check);
        assert!(painted(&surface, 7, 10), "the tick's elbow is unpainted");
        assert!(!painted(&surface, 0, 0), "the corner should be empty");
        let covered = coverage(&surface);
        assert!(
            (0.05..0.35).contains(&covered),
            "check coverage {covered} is outside the 5%-35% band"
        );
    }

    // Mutation check: draw the indeterminate bar vertically and the
    // `painted(3, 8)` assertion fails.
    #[test]
    fn the_indeterminate_shapes_are_a_horizontal_bar() {
        for builtin in [Builtin::CheckIndeterminate, Builtin::RadioIndeterminate] {
            let surface = drawn(builtin);
            assert!(painted(&surface, 8, 8), "{builtin:?} centre unpainted");
            assert!(
                painted(&surface, 5, 8),
                "{builtin:?} left of centre unpainted"
            );
            assert!(
                !painted(&surface, 8, 2),
                "{builtin:?} painted above the bar"
            );
            let covered = coverage(&surface);
            assert!(
                (0.02..0.25).contains(&covered),
                "{builtin:?} coverage {covered} is outside the 2%-25% band"
            );
        }
    }

    // The radio dot is a filled circle about 40% of the box across.
    // Mutation check: draw it as a full-box circle and the coverage band
    // fails at ~78%.
    #[test]
    fn the_radio_is_a_small_centred_dot() {
        let surface = drawn(Builtin::Radio);
        assert!(painted(&surface, 8, 8));
        assert!(!painted(&surface, 1, 1));
        assert!(!painted(&surface, 15, 8));
        let covered = coverage(&surface);
        assert!(
            (0.06..0.25).contains(&covered),
            "radio coverage {covered} is outside the 6%-25% band"
        );
    }

    // Each arrow points where its name says.
    // Mutation check: swap ArrowUp's and ArrowDown's vertices and the first
    // two assertions of each direction swap too.
    #[test]
    fn each_arrow_points_where_its_name_says() {
        let down = drawn(Builtin::ArrowDown);
        assert!(
            painted(&down, 8, 10),
            "ArrowDown has no point at the bottom"
        );
        assert!(!painted(&down, 8, 14), "ArrowDown reaches past its box");
        assert!(painted(&down, 4, 5), "ArrowDown has no left shoulder");

        let up = drawn(Builtin::ArrowUp);
        assert!(painted(&up, 8, 6));
        assert!(painted(&up, 4, 11));

        let right = drawn(Builtin::ArrowRight);
        assert!(painted(&right, 10, 8));
        assert!(painted(&right, 5, 4));

        let left = drawn(Builtin::ArrowLeft);
        assert!(painted(&left, 6, 8));
        assert!(painted(&left, 11, 4));

        // The expander is the right-pointing arrow; rotation is the
        // caller's `-gtk-icon-transform`, not a second shape.
        assert_eq!(
            Builtin::Expander.path(16.0).bounds(),
            Builtin::ArrowRight.path(16.0).bounds()
        );
    }

    // Mutation check: draw SpinPlus as one bar and the vertical-arm
    // assertion fails.
    #[test]
    fn the_spin_glyphs_are_a_plus_and_a_minus() {
        let plus = drawn(Builtin::SpinPlus);
        assert!(painted(&plus, 8, 8));
        assert!(painted(&plus, 3, 8), "plus has no horizontal arm");
        assert!(painted(&plus, 8, 3), "plus has no vertical arm");

        let minus = drawn(Builtin::SpinMinus);
        assert!(painted(&minus, 3, 8));
        assert!(!painted(&minus, 8, 3), "minus has a vertical arm");
    }

    // The CSS name round-trips for all eleven shapes, and nothing else
    // parses.
    // Mutation check: drop one arm of the match and its round trip fails.
    #[test]
    fn every_builtin_round_trips_through_its_css_name() {
        for builtin in [
            Builtin::Check,
            Builtin::CheckIndeterminate,
            Builtin::Radio,
            Builtin::RadioIndeterminate,
            Builtin::ArrowUp,
            Builtin::ArrowDown,
            Builtin::ArrowLeft,
            Builtin::ArrowRight,
            Builtin::Expander,
            Builtin::SpinPlus,
            Builtin::SpinMinus,
        ] {
            assert_eq!(Builtin::from_css_name(builtin.css_name()), Some(builtin));
        }
        assert_eq!(Builtin::from_css_name("CHECK"), Some(Builtin::Check));
        assert_eq!(Builtin::from_css_name("nonsense"), None);
        assert_eq!(Builtin::from_css_name(""), None);
    }

    // Sizes come from CSS and CSS can say anything.
    // Mutation check: remove the non-finite guard and `path(f32::NAN)`
    // produces a path whose bounds are NaN, which then poisons every clip
    // the caller sets from it.
    #[test]
    fn a_hostile_size_never_panics_and_never_produces_a_nan_path() {
        for size in [0.0, -16.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1e30] {
            for builtin in [Builtin::Check, Builtin::Radio, Builtin::SpinPlus] {
                let path = builtin.path(size);
                let bounds = path.bounds();
                assert!(
                    bounds.left.is_finite() && bounds.top.is_finite(),
                    "{builtin:?} at {size} produced non-finite bounds"
                );
                assert!(builtin.stroke_width(size).is_finite());
            }
        }
        let mut surface = Surface::new_raster_n32_premul(4, 4).expect("surface");
        let mut canvas = surface.canvas();
        Builtin::Check.draw(&mut canvas, Rect::new(0.0, 0.0, f32::NAN, -3.0), BLACK);
    }
}
