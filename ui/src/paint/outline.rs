//! `outline-*`.
//!
//! An outline is an ordinary ring, but it lives outside the border box by
//! `outline-offset` (which may be negative, pulling it inside), it never
//! takes part in layout, and it carries its own radius derived from the
//! element's `border-radius` plus the offset.

use skia_rs_safe::canvas::Canvas;
use skia_rs_safe::path::{DashEffect, PathEffectRef};

use crate::css::value::{Keyword, Rgba};
use crate::layout::{Allocation, Rect};
use crate::paint::border::is_visible_border_style;
use crate::paint::fill_paint;
use crate::paint::geometry::{inner_radii, rounded_rect_path, rounded_ring_path};

/// Paint one outline around `alloc`.
pub fn paint_outline(
    canvas: &mut Canvas<'_>,
    alloc: &Allocation,
    width: f32,
    offset: f32,
    color: Rgba,
    style: Keyword,
    radii: &[[f32; 2]; 4],
) {
    if !width.is_finite() || width <= 0.0 || !is_visible_border_style(style) || color.a <= 0.0 {
        return;
    }
    let offset = if offset.is_finite() { offset } else { 0.0 };
    let base = alloc.border_box;
    let outer = Rect::new(
        base.x - offset - width,
        base.y - offset - width,
        base.width + (offset + width) * 2.0,
        base.height + (offset + width) * 2.0,
    );
    if outer.is_empty() {
        return;
    }
    let grow = offset + width;
    let outer_radii: [[f32; 2]; 4] =
        std::array::from_fn(|i| [(radii[i][0] + grow).max(0.0), (radii[i][1] + grow).max(0.0)]);
    let inner = outer.inset([width; 4]);
    let inner_r = inner_radii(&outer_radii, [width; 4]);

    match style {
        Keyword::Double => {
            let third = width / 3.0;
            canvas.draw_path(
                &rounded_ring_path(
                    outer,
                    &outer_radii,
                    outer.inset([third; 4]),
                    &inner_radii(&outer_radii, [third; 4]),
                ),
                &fill_paint(color),
            );
            let start = outer.inset([third * 2.0; 4]);
            canvas.draw_path(
                &rounded_ring_path(
                    start,
                    &inner_radii(&outer_radii, [third * 2.0; 4]),
                    inner,
                    &inner_r,
                ),
                &fill_paint(color),
            );
        }
        Keyword::Dotted | Keyword::Dashed => {
            let (on, off) = if matches!(style, Keyword::Dotted) {
                (width, width)
            } else {
                (width * 3.0, width * 2.0)
            };
            let centre = outer.inset([width / 2.0; 4]);
            let path = rounded_rect_path(centre, &inner_radii(&outer_radii, [width / 2.0; 4]));
            let mut paint = fill_paint(color);
            paint.set_style(skia_rs_safe::paint::Style::Stroke);
            paint.set_stroke_width(width);
            paint.set_stroke_cap(if matches!(style, Keyword::Dotted) {
                skia_rs_safe::paint::StrokeCap::Round
            } else {
                skia_rs_safe::paint::StrokeCap::Butt
            });
            if let Some(dash) = DashEffect::new(vec![on, off], 0.0) {
                let effect: PathEffectRef = std::sync::Arc::new(dash);
                paint.set_path_effect(Some(effect));
            }
            canvas.draw_path(&path, &paint);
        }
        // `wavy`, `groove`, `ridge`, `inset`, `outset` and `solid` are all
        // drawn solid in M2 -- the spec's ruling for `wavy`, and what GTK
        // itself renders for the emboss styles.
        _ => {
            canvas.draw_path(
                &rounded_ring_path(outer, &outer_radii, inner, &inner_r),
                &fill_paint(color),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::paint_outline;
    use crate::css::value::{Keyword, Rgba};
    use crate::layout::{Allocation, Rect};
    use skia_rs_safe::canvas::Surface;
    use skia_rs_safe::core::Color;

    fn blue() -> Rgba {
        Rgba {
            r: 0.0,
            g: 0.0,
            b: 1.0,
            a: 1.0,
        }
    }

    fn painted(width: f32, offset: f32, style: Keyword, radii: [[f32; 2]; 4]) -> Surface {
        let border_box = Rect::new(20.0, 15.0, 20.0, 10.0);
        let alloc = Allocation {
            border_box,
            content_box: border_box,
            border: [0.0; 4],
            padding: [0.0; 4],
        };
        let mut surface = Surface::new_raster_n32_premul(60, 40).expect("raster surface");
        surface.canvas().clear(Color::TRANSPARENT);
        {
            let mut canvas = surface.canvas();
            paint_outline(&mut canvas, &alloc, width, offset, blue(), style, &radii);
        }
        surface
    }

    fn pixel(surface: &Surface, x: i32, y: i32) -> Color {
        surface.pixel_buffer().get_pixel(x, y).expect("pixel")
    }

    #[test]
    fn an_outline_sits_outside_the_border_box_by_its_offset() {
        // Adwaita draws focus rings as `outline: 2px solid; outline-offset:
        // -3px` and positive offsets elsewhere. Mutation check: ignoring the
        // offset paints at y == 14 instead of y == 11.
        let surface = painted(2.0, 3.0, Keyword::Solid, [[0.0, 0.0]; 4]);
        assert_eq!(pixel(&surface, 30, 11), Color(0xFF00_00FF));
        assert_eq!(
            pixel(&surface, 30, 14).alpha(),
            0,
            "the offset gap is clear"
        );
        assert_eq!(
            pixel(&surface, 30, 20).alpha(),
            0,
            "the box itself is untouched"
        );
    }

    #[test]
    fn a_negative_offset_pulls_the_outline_inside_the_box() {
        // Mutation check: clamping the offset at zero puts nothing at y == 16
        // (a clamped ring sits at y in [13, 15) instead, per the un-clamped
        // band computed below).
        let surface = painted(2.0, -3.0, Keyword::Solid, [[0.0, 0.0]; 4]);
        assert_eq!(pixel(&surface, 30, 16), Color(0xFF00_00FF));
    }

    #[test]
    fn a_zero_width_or_invisible_style_paints_nothing() {
        for (w, style) in [
            (0.0, Keyword::Solid),
            (3.0, Keyword::None),
            (3.0, Keyword::Hidden),
        ] {
            let surface = painted(w, 0.0, style, [[0.0, 0.0]; 4]);
            assert_eq!(pixel(&surface, 30, 14).alpha(), 0);
        }
    }

    #[test]
    fn a_wavy_outline_renders_as_solid_rather_than_disappearing() {
        // The spec's ruling: `wavy` is drawn solid in M2. Mutation check:
        // falling through to "unknown -> nothing" loses the focus ring.
        let surface = painted(2.0, 0.0, Keyword::Wavy, [[0.0, 0.0]; 4]);
        assert_eq!(pixel(&surface, 30, 14), Color(0xFF00_00FF));
    }

    #[test]
    fn outline_painting_never_panics_on_hostile_numbers() {
        for &v in &[f32::NAN, f32::INFINITY, -1.0e9, 1.0e9] {
            let _ = painted(v, v, Keyword::Solid, [[v, v]; 4]);
            let _ = painted(v, v, Keyword::Dashed, [[v, v]; 4]);
        }
    }
}
