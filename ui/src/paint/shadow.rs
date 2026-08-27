//! `box-shadow`, outset and inset.
//!
//! `BlurMaskFilter` never reaches a pixel through `skia-rs-safe` 0.4.0's
//! rasterizer, so a blurred shadow is drawn into an offscreen premultiplied
//! surface, blurred by `paint::blur`, and blitted back with `draw_image`,
//! which does honour the current path clip.

use skia_rs_safe::canvas::{Canvas, ClipOp};

use crate::css::value::{ColorValue, LengthCtx, Rgba, Shadow};
use crate::layout::{Allocation, Rect};
use crate::paint::blur::{blurred_image, sigma_for_blur_radius};
use crate::paint::fill_paint;
use crate::paint::geometry::{inner_radii, rounded_rect_path, rounded_ring_path};

/// A finite length in px, or `default`.
fn px(length: &crate::css::value::Length, ctx: &LengthCtx, default: f32) -> f32 {
    length
        .resolve(ctx)
        .filter(|v| v.is_finite())
        .unwrap_or(default)
}

/// Paint every shadow in `shadows` whose `inset` flag equals `inset`.
///
/// Outset shadows paint behind the box (the caller draws them first), inset
/// shadows on top of the background inside the padding box. CSS paints a
/// shadow list back-to-front, so the list is iterated in reverse.
pub fn paint_box_shadows(
    canvas: &mut Canvas<'_>,
    shadows: &[Shadow],
    inset: bool,
    alloc: &Allocation,
    radii: &[[f32; 2]; 4],
    current: Rgba,
    ctx: &LengthCtx,
) {
    for shadow in shadows.iter().rev() {
        if shadow.inset != inset {
            continue;
        }
        // `computed` (contract §5, inheritance rule 5) has already resolved
        // `@name` and `currentColor` for this slot, so a shadow colour that
        // survived is `ColorValue::Absolute`; `None` means currentColor.
        let color = match shadow.color.as_ref() {
            Some(ColorValue::Absolute(rgba)) => *rgba,
            Some(_) | None => current,
        };
        if color.a <= 0.0 {
            continue;
        }
        let dx = px(&shadow.offset_x, ctx, 0.0);
        let dy = px(&shadow.offset_y, ctx, 0.0);
        let blur = px(&shadow.blur, ctx, 0.0).max(0.0);
        let spread = px(&shadow.spread, ctx, 0.0);
        if inset {
            paint_inset(canvas, alloc, radii, color, dx, dy, blur, spread);
        } else {
            paint_outset(canvas, alloc, radii, color, dx, dy, blur, spread);
        }
    }
}

/// One outset shadow: the border box, offset, spread and blurred.
#[allow(
    clippy::too_many_arguments,
    reason = "one shadow's full CSS description"
)]
fn paint_outset(
    canvas: &mut Canvas<'_>,
    alloc: &Allocation,
    radii: &[[f32; 2]; 4],
    color: Rgba,
    dx: f32,
    dy: f32,
    blur: f32,
    spread: f32,
) {
    let base = alloc.border_box;
    let shape = Rect::new(
        base.x + dx - spread,
        base.y + dy - spread,
        base.width + spread * 2.0,
        base.height + spread * 2.0,
    );
    if shape.is_empty() {
        return;
    }
    let shape_radii: [[f32; 2]; 4] = std::array::from_fn(|i| {
        [
            (radii[i][0] + spread).max(0.0),
            (radii[i][1] + spread).max(0.0),
        ]
    });

    if blur <= 0.0 {
        canvas.draw_path(&rounded_rect_path(shape, &shape_radii), &fill_paint(color));
        return;
    }
    blit_blurred(canvas, shape, blur, |offscreen_canvas, offset| {
        let local = Rect::new(
            shape.x - offset.0,
            shape.y - offset.1,
            shape.width,
            shape.height,
        );
        offscreen_canvas.draw_path(&rounded_rect_path(local, &shape_radii), &fill_paint(color));
    });
}

/// One inset shadow: the ring between the padding box and the shrunken,
/// offset copy of it, clipped to the padding box.
#[allow(
    clippy::too_many_arguments,
    reason = "one shadow's full CSS description"
)]
fn paint_inset(
    canvas: &mut Canvas<'_>,
    alloc: &Allocation,
    radii: &[[f32; 2]; 4],
    color: Rgba,
    dx: f32,
    dy: f32,
    blur: f32,
    spread: f32,
) {
    let outer = alloc.padding_box();
    if outer.is_empty() {
        return;
    }
    let outer_radii = inner_radii(radii, alloc.border);
    let hole = Rect::new(
        outer.x + dx + spread,
        outer.y + dy + spread,
        (outer.width - spread * 2.0).max(0.0),
        (outer.height - spread * 2.0).max(0.0),
    );
    let hole_radii: [[f32; 2]; 4] = std::array::from_fn(|i| {
        [
            (outer_radii[i][0] - spread).max(0.0),
            (outer_radii[i][1] - spread).max(0.0),
        ]
    });

    let save = canvas.save();
    canvas.clip_path(
        &rounded_rect_path(outer, &outer_radii),
        ClipOp::Intersect,
        true,
    );
    if blur <= 0.0 {
        let ring = rounded_ring_path(outer, &outer_radii, hole, &hole_radii);
        canvas.draw_path(&ring, &fill_paint(color));
    } else {
        blit_blurred(canvas, outer, blur, |offscreen_canvas, offset| {
            let local_outer = Rect::new(
                outer.x - offset.0,
                outer.y - offset.1,
                outer.width,
                outer.height,
            );
            let local_hole = Rect::new(
                hole.x - offset.0,
                hole.y - offset.1,
                hole.width,
                hole.height,
            );
            let ring = rounded_ring_path(local_outer, &outer_radii, local_hole, &hole_radii);
            offscreen_canvas.draw_path(&ring, &fill_paint(color));
        });
    }
    canvas.restore_to_count(save);
}

/// Render `draw` into an offscreen surface padded for `blur`, blur it and
/// blit it at the right place.
fn blit_blurred(
    canvas: &mut Canvas<'_>,
    shape: Rect,
    blur: f32,
    draw: impl FnOnce(&mut Canvas<'_>, (f32, f32)),
) {
    let sigma = sigma_for_blur_radius(blur);
    let pad = (sigma * 3.0).ceil().clamp(0.0, 512.0);
    let ox = (shape.x - pad).floor();
    let oy = (shape.y - pad).floor();
    let w = (shape.width + pad * 2.0).ceil();
    let h = (shape.height + pad * 2.0).ceil();
    if !(w.is_finite() && h.is_finite()) || w <= 0.0 || h <= 0.0 || w > 8192.0 || h > 8192.0 {
        return;
    }
    let Some(image) = blurred_image(w as i32, h as i32, sigma, |offscreen| {
        draw(offscreen, (ox, oy))
    }) else {
        return;
    };
    canvas.draw_image(&image, ox, oy, None);
}

#[cfg(test)]
mod tests {
    use super::paint_box_shadows;
    use crate::css::value::{ColorValue, Length, LengthCtx, Rgba, Shadow};
    use crate::layout::{Allocation, Rect};
    use skia_rs_safe::canvas::Surface;
    use skia_rs_safe::core::Color;

    fn ctx() -> LengthCtx {
        LengthCtx {
            font_size_px: 14.0,
            root_font_size_px: 14.0,
            ex_ratio: 0.5,
            dpi: 96.0,
            percent_basis: None,
        }
    }

    fn black() -> Rgba {
        Rgba {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        }
    }

    fn shadow(dx: f32, dy: f32, blur: f32, spread: f32, inset: bool) -> Shadow {
        Shadow {
            color: Some(ColorValue::Absolute(black())),
            offset_x: Length::px(dx),
            offset_y: Length::px(dy),
            blur: Length::px(blur),
            spread: Length::px(spread),
            inset,
        }
    }

    /// A 20x10 box centred in a 60x40 surface.
    fn alloc() -> Allocation {
        let border_box = Rect::new(20.0, 15.0, 20.0, 10.0);
        Allocation {
            border_box,
            content_box: border_box,
            border: [0.0; 4],
            padding: [0.0; 4],
        }
    }

    fn painted(shadows: &[Shadow], inset: bool) -> Surface {
        let mut surface = Surface::new_raster_n32_premul(60, 40).expect("raster surface");
        surface.canvas().clear(Color::TRANSPARENT);
        {
            let mut canvas = surface.canvas();
            paint_box_shadows(
                &mut canvas,
                shadows,
                inset,
                &alloc(),
                &[[0.0, 0.0]; 4],
                black(),
                &ctx(),
            );
        }
        surface
    }

    fn pixel(surface: &Surface, x: i32, y: i32) -> Color {
        surface.pixel_buffer().get_pixel(x, y).expect("pixel")
    }

    #[test]
    fn an_unblurred_outset_shadow_is_exact_at_its_flat_core() {
        // The exactness rule: with blur 0 the shadow is a hard-edged
        // rounded rect, so its interior is the declared colour exactly.
        // Mutation check: applying the offset to the wrong axis moves the
        // core off (35, 20) and the assertion fails.
        let surface = painted(&[shadow(5.0, 0.0, 0.0, 0.0, false)], false);
        assert_eq!(pixel(&surface, 35, 20), Color(0xFF00_0000));
        assert_eq!(
            pixel(&surface, 22, 20).alpha(),
            0,
            "left of the offset shadow"
        );
    }

    #[test]
    fn spread_grows_the_shadow_shape_in_every_direction() {
        // Mutation check: adding spread to only one axis leaves (20, 12)
        // clear, since the box's top is at y == 15.
        let surface = painted(&[shadow(0.0, 0.0, 0.0, 4.0, false)], false);
        assert_eq!(pixel(&surface, 20, 12), Color(0xFF00_0000));
        assert_eq!(pixel(&surface, 18, 20), Color(0xFF00_0000));
    }

    #[test]
    fn a_blurred_shadow_spreads_outside_its_shape_and_dilutes_its_edge() {
        // Mutation check: a blur that no-ops (the BlurMaskFilter trap) leaves
        // (20, 8) fully clear and (20, 14) fully opaque.
        let surface = painted(&[shadow(0.0, 0.0, 8.0, 0.0, false)], false);
        let outside = pixel(&surface, 20, 8);
        assert!(outside.alpha() > 0 && outside.alpha() < 0xFF, "soft edge");
    }

    #[test]
    fn an_inset_shadow_paints_inside_the_padding_box_only() {
        // Mutation check: forgetting the clip paints the inset shadow's ring
        // across the whole surface and (5, 5) stops being clear.
        let surface = painted(&[shadow(0.0, 6.0, 0.0, 0.0, true)], true);
        assert_eq!(
            pixel(&surface, 30, 17),
            Color(0xFF00_0000),
            "top strip is shadowed"
        );
        assert_eq!(pixel(&surface, 30, 24).alpha(), 0, "the bottom is not");
        assert_eq!(pixel(&surface, 5, 5).alpha(), 0, "nothing outside the box");
    }

    #[test]
    fn only_shadows_matching_the_requested_inset_flag_are_painted() {
        // Mutation check: ignoring the flag paints outset shadows during the
        // inset pass, which lands them on top of the background.
        let surface = painted(&[shadow(5.0, 0.0, 0.0, 0.0, false)], true);
        assert_eq!(pixel(&surface, 35, 20).alpha(), 0);
    }

    #[test]
    fn shadow_painting_never_panics_on_hostile_lengths() {
        for &v in &[f32::NAN, f32::INFINITY, -1.0e9, 1.0e9] {
            let _ = painted(&[shadow(v, v, v, v, false)], false);
            let _ = painted(&[shadow(v, v, v, v, true)], true);
        }
    }
}
