//! Per-side borders.
//!
//! Each side is a *filled* path -- the border ring intersected with that
//! side's mitre wedge -- never a stroke. Stroking cannot express four
//! different widths, four colours and four corner radii meeting on the
//! diagonals, which is exactly what GTK draws.

use skia_rs_safe::canvas::{Canvas, ClipOp};
use skia_rs_safe::path::{DashEffect, PathEffectRef};

use crate::css::value::{Keyword, Rgba};
use crate::layout::Allocation;
use crate::paint::geometry::{Side, inner_radii, rounded_ring_path, side_wedge_path};
use crate::paint::{fill_paint, rounded_rect_path};

/// Whether a `border-style` keyword paints anything at all.
#[must_use]
pub fn is_visible_border_style(style: Keyword) -> bool {
    !matches!(style, Keyword::None | Keyword::Hidden)
}

/// Paint all four borders.
pub fn paint_borders(
    canvas: &mut Canvas<'_>,
    alloc: &Allocation,
    widths: [f32; 4],
    colors: [Rgba; 4],
    styles: [Keyword; 4],
    radii: &[[f32; 2]; 4],
) {
    let used: [f32; 4] = std::array::from_fn(|i| {
        if widths[i].is_finite() && widths[i] > 0.0 && is_visible_border_style(styles[i]) {
            widths[i]
        } else {
            0.0
        }
    });
    if used.iter().all(|w| *w <= 0.0) {
        return;
    }

    let outer = alloc.border_box;
    let inner = outer.inset(used);
    let inner_r = inner_radii(radii, used);
    let ring = rounded_ring_path(outer, radii, inner, &inner_r);

    let uniform = used[1..].iter().all(|w| (w - used[0]).abs() < f32::EPSILON)
        && colors[1..].iter().all(|c| *c == colors[0])
        && styles[1..].iter().all(|s| *s == styles[0])
        && styles[0] == Keyword::Solid;
    if uniform {
        // One fill, so the four sides never seam against each other along
        // the mitres -- this is the path Adwaita's 1px button border takes.
        canvas.draw_path(&ring, &fill_paint(colors[0]));
        return;
    }

    for (index, side) in [Side::Top, Side::Right, Side::Bottom, Side::Left]
        .into_iter()
        .enumerate()
    {
        if used[index] <= 0.0 {
            continue;
        }
        let save = canvas.save();
        canvas.clip_path(
            &side_wedge_path(outer, inner, side),
            ClipOp::Intersect,
            true,
        );
        paint_one_side(
            canvas,
            &ring,
            outer,
            used,
            index,
            colors[index],
            styles[index],
            radii,
        );
        canvas.restore_to_count(save);
    }
}

/// Fill (or dash, or double) one side's share of the ring.
#[allow(clippy::too_many_arguments, reason = "one side's full CSS description")]
fn paint_one_side(
    canvas: &mut Canvas<'_>,
    ring: &skia_rs_safe::path::Path,
    outer: crate::layout::Rect,
    used: [f32; 4],
    index: usize,
    color: Rgba,
    style: Keyword,
    radii: &[[f32; 2]; 4],
) {
    match style {
        Keyword::Double => {
            // Three equal bands; the middle one is left empty.
            let third: [f32; 4] = std::array::from_fn(|i| used[i] / 3.0);
            let two_thirds: [f32; 4] = std::array::from_fn(|i| used[i] * 2.0 / 3.0);
            let outer_band =
                rounded_ring_path(outer, radii, outer.inset(third), &inner_radii(radii, third));
            let inner_start = outer.inset(two_thirds);
            let inner_band = rounded_ring_path(
                inner_start,
                &inner_radii(radii, two_thirds),
                outer.inset(used),
                &inner_radii(radii, used),
            );
            canvas.draw_path(&outer_band, &fill_paint(color));
            canvas.draw_path(&inner_band, &fill_paint(color));
        }
        Keyword::Dotted | Keyword::Dashed => {
            // Stroke the centre line of this side with a dash effect, then
            // let the wedge clip keep it inside the side. `DashEffect` is
            // the crate's only dash primitive and it applies to strokes.
            let w = used[index];
            let (on, off) = if matches!(style, Keyword::Dotted) {
                (w, w)
            } else {
                (w * 3.0, w * 2.0)
            };
            let centre = outer.inset(std::array::from_fn(|i| used[i] / 2.0));
            let path = rounded_rect_path(
                centre,
                &inner_radii(radii, std::array::from_fn(|i| used[i] / 2.0)),
            );
            let mut paint = fill_paint(color);
            paint.set_style(skia_rs_safe::paint::Style::Stroke);
            paint.set_stroke_width(w);
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
        // `groove`, `ridge`, `inset` and `outset` are drawn as solid in M2;
        // GTK itself renders them flat in Adwaita, and the spec's paint list
        // asks only that they render, not that they emboss.
        _ => {
            canvas.draw_path(ring, &fill_paint(color));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{is_visible_border_style, paint_borders};
    use crate::css::value::{Keyword, Rgba};
    use crate::layout::{Allocation, Rect};
    use skia_rs_safe::canvas::Surface;
    use skia_rs_safe::core::Color;

    fn rgba(r: u8, g: u8, b: u8) -> Rgba {
        Rgba {
            r: f32::from(r) / 255.0,
            g: f32::from(g) / 255.0,
            b: f32::from(b) / 255.0,
            a: 1.0,
        }
    }

    fn alloc(w: f32, h: f32, border: [f32; 4]) -> Allocation {
        let border_box = Rect::new(0.0, 0.0, w, h);
        Allocation {
            border_box,
            content_box: border_box.inset(border),
            border,
            padding: [0.0; 4],
        }
    }

    fn painted(
        widths: [f32; 4],
        colors: [Rgba; 4],
        styles: [Keyword; 4],
        radii: [[f32; 2]; 4],
    ) -> Surface {
        let mut surface = Surface::new_raster_n32_premul(40, 20).expect("raster surface");
        surface.canvas().clear(Color::TRANSPARENT);
        {
            let mut canvas = surface.canvas();
            paint_borders(
                &mut canvas,
                &alloc(40.0, 20.0, widths),
                widths,
                colors,
                styles,
                &radii,
            );
        }
        surface
    }

    fn pixel(surface: &Surface, x: i32, y: i32) -> Color {
        surface.pixel_buffer().get_pixel(x, y).expect("pixel")
    }

    #[test]
    fn a_uniform_border_paints_adwaitas_exact_edge_colour() {
        // The M1 gate's `pixel(cx, 0) == 0xFFCDC7C2`, as a unit test. The
        // straight run of the top edge is fully covered, so it is exact.
        // Mutation check: stroking on the centre line (M1's model) halves
        // the coverage of a 1px border and this stops being exact.
        let surface = painted(
            [1.0; 4],
            [rgba(0xCD, 0xC7, 0xC2); 4],
            [Keyword::Solid; 4],
            [[5.0, 5.0]; 4],
        );
        assert_eq!(pixel(&surface, 20, 0), Color(0xFFCD_C7C2));
        assert_eq!(
            pixel(&surface, 0, 0).alpha(),
            0,
            "outside the 5px corner arc"
        );
        assert_eq!(
            pixel(&surface, 20, 10).alpha(),
            0,
            "the interior is untouched"
        );
    }

    #[test]
    fn mixed_per_side_colours_meet_on_the_mitre_and_do_not_overdraw() {
        // Mutation check: filling the whole ring once per side paints the
        // last side's colour everywhere, so the left edge reads red.
        let red = rgba(0xFF, 0x00, 0x00);
        let blue = rgba(0x00, 0x00, 0xFF);
        let surface = painted(
            [6.0, 6.0, 6.0, 6.0],
            [red, blue, blue, red],
            [Keyword::Solid; 4],
            [[0.0, 0.0]; 4],
        );
        assert_eq!(pixel(&surface, 20, 1), Color(0xFFFF_0000), "top is red");
        assert_eq!(pixel(&surface, 38, 10), Color(0xFF00_00FF), "right is blue");
        assert_eq!(pixel(&surface, 1, 10), Color(0xFFFF_0000), "left is red");
    }

    #[test]
    fn none_and_hidden_sides_paint_nothing_even_with_a_width() {
        // CSS: `border-style: none` forces the used width to zero. P3
        // already zeroes the width, but a caller passing both must still be
        // safe. Mutation check: dropping the style check paints a black bar.
        let surface = painted(
            [4.0; 4],
            [rgba(0, 0, 0); 4],
            [
                Keyword::None,
                Keyword::Hidden,
                Keyword::None,
                Keyword::Hidden,
            ],
            [[0.0, 0.0]; 4],
        );
        assert_eq!(pixel(&surface, 20, 1).alpha(), 0);
        assert_eq!(pixel(&surface, 1, 10).alpha(), 0);
        assert!(!is_visible_border_style(Keyword::None));
        assert!(!is_visible_border_style(Keyword::Hidden));
        assert!(is_visible_border_style(Keyword::Double));
    }

    #[test]
    fn a_dashed_border_leaves_gaps_along_its_edge() {
        // Mutation check: falling back to a solid fill makes every sampled
        // pixel opaque and `gaps` stays zero.
        let surface = painted(
            [4.0; 4],
            [rgba(0, 0, 0); 4],
            [Keyword::Dashed; 4],
            [[0.0, 0.0]; 4],
        );
        let gaps = (6..34)
            .filter(|&x| pixel(&surface, x, 1).alpha() == 0)
            .count();
        assert!(gaps > 0, "a dashed top edge must have gaps");
        let inked = (6..34)
            .filter(|&x| pixel(&surface, x, 1).alpha() > 0)
            .count();
        assert!(inked > 0, "a dashed top edge must also have ink");
    }

    #[test]
    fn border_painting_never_panics_on_hostile_geometry() {
        for &v in &[f32::NAN, f32::INFINITY, -8.0, 0.0, 1.0e30] {
            let _ = painted([v; 4], [rgba(1, 2, 3); 4], [Keyword::Solid; 4], [[v, v]; 4]);
            let _ = painted(
                [v; 4],
                [rgba(1, 2, 3); 4],
                [Keyword::Double; 4],
                [[v, v]; 4],
            );
            let _ = painted(
                [v; 4],
                [rgba(1, 2, 3); 4],
                [Keyword::Dotted; 4],
                [[v, v]; 4],
            );
        }
    }
}
