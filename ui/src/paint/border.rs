//! Per-side borders.
//!
//! Each side is a *filled* path -- the border ring intersected with that
//! side's mitre wedge -- never a stroke. Stroking cannot express four
//! different widths, four colours and four corner radii meeting on the
//! diagonals, which is exactly what GTK draws.

use skia_rs_safe::canvas::{Canvas, ClipOp};
use skia_rs_safe::core::IRect;
use skia_rs_safe::path::{DashEffect, PathEffectRef};

use crate::css::value::{
    BorderImageSlice, BorderImageWidthSide, Image, Keyword, NumberOrPercent, RepeatStyle, Rgba,
};
use crate::layout::{Allocation, Rect};
use crate::paint::geometry::{Side, inner_radii, rounded_ring_path, side_wedge_path};
use crate::paint::{PaintCx, fill_paint, rounded_rect_path};

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

/// Paint a nine-patch `border-image` over the border box.
///
/// Returns `false` when the source cannot be resolved to pixels, so the
/// caller falls back to [`paint_borders`]. Only `url()` sources have pixels
/// in M2; gradients and `-gtk-*` images as border-image sources are M4.
pub fn paint_border_image(
    canvas: &mut Canvas<'_>,
    alloc: &Allocation,
    source: &Image,
    slice: &BorderImageSlice,
    widths: &[BorderImageWidthSide; 4],
    repeat: RepeatStyle,
    cx: &mut PaintCx<'_>,
) -> bool {
    let Image::Url(url) = source else {
        return false;
    };
    let Some((iw, ih)) = cx
        .images
        .get(url)
        .map(|img| (img.width() as f32, img.height() as f32))
    else {
        return false;
    };
    if iw <= 0.0 || ih <= 0.0 {
        return false;
    }

    // Slice offsets in source pixels, TRBL.
    let slice_px = |value: NumberOrPercent, basis: f32| -> f32 {
        match value {
            NumberOrPercent::Number(n) if n.is_finite() => n.clamp(0.0, basis),
            NumberOrPercent::Percent(p) if p.is_finite() => (p * basis).clamp(0.0, basis),
            _ => 0.0,
        }
    };
    let st = slice_px(slice.sides[0], ih);
    let sr = slice_px(slice.sides[1], iw);
    let sb = slice_px(slice.sides[2], ih);
    let sl = slice_px(slice.sides[3], iw);

    // Destination widths, TRBL: `auto` uses the slice, a number scales the
    // used border width, a length resolves directly.
    let ctx = cx.base_length_ctx();
    let dest = |side: &BorderImageWidthSide, used: f32, slice: f32| -> f32 {
        match side {
            BorderImageWidthSide::Auto => slice,
            BorderImageWidthSide::Number(n) if n.is_finite() => n * used,
            BorderImageWidthSide::Length(len) => len.resolve(&ctx).unwrap_or(used),
            BorderImageWidthSide::Number(_) => used,
        }
    };
    let dt = dest(&widths[0], alloc.border[0], st);
    let dr = dest(&widths[1], alloc.border[1], sr);
    let db = dest(&widths[2], alloc.border[2], sb);
    let dl = dest(&widths[3], alloc.border[3], sl);

    let outer = alloc.border_box;
    if outer.is_empty() {
        return false;
    }

    // Nine slots: (src IRect, dst Rect, tiles?).
    let src = |x: f32, y: f32, w: f32, h: f32| {
        IRect::new(x as i32, y as i32, (x + w) as i32, (y + h) as i32)
    };
    let mid_w = (iw - sl - sr).max(0.0);
    let mid_h = (ih - st - sb).max(0.0);
    let dmid_w = (outer.width - dl - dr).max(0.0);
    let dmid_h = (outer.height - dt - db).max(0.0);

    let mut slots: Vec<(IRect, Rect, bool, bool)> = vec![
        (
            src(0.0, 0.0, sl, st),
            Rect::new(outer.x, outer.y, dl, dt),
            false,
            false,
        ),
        (
            src(sl, 0.0, mid_w, st),
            Rect::new(outer.x + dl, outer.y, dmid_w, dt),
            true,
            false,
        ),
        (
            src(iw - sr, 0.0, sr, st),
            Rect::new(outer.right() - dr, outer.y, dr, dt),
            false,
            false,
        ),
        (
            src(0.0, st, sl, mid_h),
            Rect::new(outer.x, outer.y + dt, dl, dmid_h),
            false,
            true,
        ),
        (
            src(iw - sr, st, sr, mid_h),
            Rect::new(outer.right() - dr, outer.y + dt, dr, dmid_h),
            false,
            true,
        ),
        (
            src(0.0, ih - sb, sl, sb),
            Rect::new(outer.x, outer.bottom() - db, dl, db),
            false,
            false,
        ),
        (
            src(sl, ih - sb, mid_w, sb),
            Rect::new(outer.x + dl, outer.bottom() - db, dmid_w, db),
            true,
            false,
        ),
        (
            src(iw - sr, ih - sb, sr, sb),
            Rect::new(outer.right() - dr, outer.bottom() - db, dr, db),
            false,
            false,
        ),
    ];
    if slice.fill {
        slots.push((
            src(sl, st, mid_w, mid_h),
            Rect::new(outer.x + dl, outer.y + dt, dmid_w, dmid_h),
            true,
            true,
        ));
    }

    let tiles_x = matches!(repeat.x, Keyword::Repeat | Keyword::Round | Keyword::Space);
    let tiles_y = matches!(repeat.y, Keyword::Repeat | Keyword::Round | Keyword::Space);
    let Some(image) = cx.images.get(url) else {
        return false;
    };
    let mut painted = false;
    for (src_rect, dst_rect, edge_x, edge_y) in slots {
        if dst_rect.is_empty() || src_rect.width() <= 0 || src_rect.height() <= 0 {
            continue;
        }
        if (edge_x && tiles_x) || (edge_y && tiles_y) {
            // Tile the slot with source-sized copies rather than stretching.
            let step_x = if edge_x && tiles_x {
                src_rect.width() as f32
            } else {
                dst_rect.width
            };
            let step_y = if edge_y && tiles_y {
                src_rect.height() as f32
            } else {
                dst_rect.height
            };
            let mut y = dst_rect.y;
            while y < dst_rect.bottom() && step_y > 0.0 {
                let mut x = dst_rect.x;
                while x < dst_rect.right() && step_x > 0.0 {
                    let piece = Rect::new(
                        x,
                        y,
                        step_x.min(dst_rect.right() - x),
                        step_y.min(dst_rect.bottom() - y),
                    );
                    canvas.draw_image_rect(image, Some(&src_rect), &piece.to_skia(), None);
                    x += step_x;
                }
                y += step_y;
            }
        } else {
            canvas.draw_image_rect(image, Some(&src_rect), &dst_rect.to_skia(), None);
        }
        painted = true;
    }
    painted
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

    use super::paint_border_image;
    use crate::css::computed::ResolveEnv;
    use crate::css::value::{
        BorderImageSlice, BorderImageWidthSide, Image, NumberOrPercent, RepeatStyle,
    };
    use crate::paint::{ImageCache, PaintCx};
    use crate::text::FontDatabase;
    use std::collections::HashMap;

    #[test]
    fn an_unresolvable_border_image_reports_false_so_the_caller_falls_back() {
        // Contract §8: `false` means "I painted nothing; use paint_borders".
        // Mutation check: returning `true` unconditionally silently loses
        // every ordinary border on a theme with a bad border-image URL.
        let env = ResolveEnv::default();
        let colors = HashMap::new();
        let mut fonts = FontDatabase::probe_only();
        let mut images = ImageCache::new();
        let mut cx = PaintCx {
            env: &env,
            colors: &colors,
            fonts: &mut fonts,
            images: &mut images,
            text: None,
        };
        let mut surface = Surface::new_raster_n32_premul(40, 20).expect("raster surface");
        let mut canvas = surface.canvas();
        let painted = paint_border_image(
            &mut canvas,
            &alloc(40.0, 20.0, [4.0; 4]),
            &Image::Url("/nonexistent/icedtea-border.png".into()),
            &BorderImageSlice {
                sides: [NumberOrPercent::Number(4.0); 4],
                fill: false,
            },
            &[
                BorderImageWidthSide::Number(1.0),
                BorderImageWidthSide::Number(1.0),
                BorderImageWidthSide::Number(1.0),
                BorderImageWidthSide::Number(1.0),
            ],
            RepeatStyle {
                x: Keyword::Repeat,
                y: Keyword::Repeat,
            },
            &mut cx,
        );
        assert!(!painted);
    }

    #[test]
    fn image_none_reports_false_without_touching_the_canvas() {
        let env = ResolveEnv::default();
        let colors = HashMap::new();
        let mut fonts = FontDatabase::probe_only();
        let mut images = ImageCache::new();
        let mut cx = PaintCx {
            env: &env,
            colors: &colors,
            fonts: &mut fonts,
            images: &mut images,
            text: None,
        };
        let mut surface = Surface::new_raster_n32_premul(40, 20).expect("raster surface");
        surface.canvas().clear(Color::TRANSPARENT);
        {
            let mut canvas = surface.canvas();
            assert!(!paint_border_image(
                &mut canvas,
                &alloc(40.0, 20.0, [4.0; 4]),
                &Image::None,
                &BorderImageSlice {
                    sides: [NumberOrPercent::Number(0.0); 4],
                    fill: false,
                },
                &[
                    BorderImageWidthSide::Auto,
                    BorderImageWidthSide::Auto,
                    BorderImageWidthSide::Auto,
                    BorderImageWidthSide::Auto,
                ],
                RepeatStyle {
                    x: Keyword::Stretch,
                    y: Keyword::Stretch,
                },
                &mut cx,
            ));
        }
        assert_eq!(pixel(&surface, 20, 1).alpha(), 0);
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
