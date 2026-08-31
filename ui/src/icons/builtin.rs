//! `-gtk-icon-source: builtin` shapes.
//!
//! P5 needs these names to build `CheckButton`'s `check` node and
//! `SpinButton`'s steppers. **The geometry here is a placeholder** — P7 fills
//! [`Builtin::path`] and [`Builtin::stroke_width`] in against
//! `gtkcssimagebuiltin` and un-ignores the two P5 pixel tests named in the
//! part-5 plan's D9.

use skia_rs_safe::canvas::Canvas;
use skia_rs_safe::core::Matrix;
use skia_rs_safe::path::Path;

use crate::css::value::Rgba;
use crate::layout::Rect;
use crate::paint::fill_paint;
use crate::paint::geometry::rounded_rect_path;

/// The builtin shapes GTK's CSS engine can name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Builtin {
    /// A check mark.
    Check,
    /// A check button's indeterminate dash.
    CheckIndeterminate,
    /// A radio dot.
    Radio,
    /// A radio button's indeterminate dash.
    RadioIndeterminate,
    /// An upward arrow.
    ArrowUp,
    /// A downward arrow.
    ArrowDown,
    /// A leftward arrow.
    ArrowLeft,
    /// A rightward arrow.
    ArrowRight,
    /// An expander triangle.
    Expander,
    /// A spin button's `+`.
    SpinPlus,
    /// A spin button's `−`.
    SpinMinus,
}

impl Builtin {
    /// The shape, in a `size × size` box at the origin.
    ///
    /// P7 replaces this body; today every shape is the inscribed square, which
    /// is enough for layout and for "did anything ink here" assertions.
    #[must_use]
    pub fn path(self, size: f32) -> Path {
        let size = if size.is_finite() && size > 0.0 {
            size
        } else {
            0.0
        };
        let inset = size * 0.25;
        rounded_rect_path(
            Rect::new(inset, inset, size - 2.0 * inset, size - 2.0 * inset),
            &[[0.0, 0.0]; 4],
        )
    }

    /// Stroke width for the outline shapes.
    #[must_use]
    pub fn stroke_width(self, size: f32) -> f32 {
        if size.is_finite() && size > 0.0 {
            size / 8.0
        } else {
            0.0
        }
    }

    /// Draw the shape filled with `color`, scaled into `rect`.
    pub fn draw(self, canvas: &mut Canvas<'_>, rect: Rect, color: Rgba) {
        if rect.is_empty() || color.a <= 0.0 {
            return;
        }
        let size = rect.width.min(rect.height);
        let path = self.path(size);
        let save = canvas.save();
        canvas.concat(&Matrix::translate(
            rect.x + (rect.width - size) / 2.0,
            rect.y + (rect.height - size) / 2.0,
        ));
        canvas.draw_path(&path, &fill_paint(color));
        canvas.restore_to_count(save);
    }

    /// Parse a `builtin(<name>)` CSS value's argument.
    #[must_use]
    pub fn from_css_name(name: &str) -> Option<Self> {
        Some(match name.trim().to_ascii_lowercase().as_str() {
            "check" => Builtin::Check,
            "check-indeterminate" => Builtin::CheckIndeterminate,
            "radio" => Builtin::Radio,
            "radio-indeterminate" => Builtin::RadioIndeterminate,
            "arrow-up" => Builtin::ArrowUp,
            "arrow-down" => Builtin::ArrowDown,
            "arrow-left" => Builtin::ArrowLeft,
            "arrow-right" => Builtin::ArrowRight,
            "expander" => Builtin::Expander,
            "spin-plus" => Builtin::SpinPlus,
            "spin-minus" => Builtin::SpinMinus,
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::Builtin;

    #[test]
    fn a_builtin_name_round_trips_and_a_hostile_size_never_panics() {
        // mutation: drop the `is_finite` guard in `path` and the NaN case
        // reaches skia with a NaN rect.
        assert_eq!(Builtin::from_css_name(" Check "), Some(Builtin::Check));
        assert_eq!(Builtin::from_css_name("nonesuch"), None);
        for shape in [Builtin::Check, Builtin::ArrowUp, Builtin::SpinMinus] {
            for size in [f32::NAN, f32::INFINITY, -8.0, 0.0, 16.0] {
                let _ = shape.path(size);
                assert!(shape.stroke_width(size).is_finite());
            }
        }
    }
}
