//! `box-shadow`, outset and inset.
//!
//! `BlurMaskFilter` never reaches a pixel through `skia-rs-safe` 0.4.0's
//! rasterizer, so a blurred shadow is drawn into an offscreen premultiplied
//! surface, blurred by `paint::blur`, and blitted back with `draw_image`,
//! which does honour the current path clip.

use skia_rs_safe::canvas::{Canvas, ClipOp};

use crate::css::value::{ColorValue, LengthCtx, Rgba, Shadow};
use crate::layout::{Allocation, Rect};
use crate::paint::blur::{blur_reach, blurred_image, sigma_for_blur_radius, sigma_within_pad};
use crate::paint::fill_paint;
use crate::paint::geometry::{inner_radii, rounded_rect_path, rounded_ring_path};

/// A fill paint for a shape that is about to be box-blurred, with
/// anti-aliasing off.
///
/// The box blur's own reach (`paint::blur::box_radii_for_gauss`'s three
/// passes) already produces the soft edge a blurred shadow wants;
/// anti-aliasing the pre-blur shape as well stacks a second soft edge on
/// top of the first, pushing the total nonzero-alpha reach out past what
/// the blur radius alone predicts. Drawing the shape hard-edged keeps the
/// blur's own math the only source of softness, so a sample far enough
/// outside `radius + blur reach` is exactly transparent.
fn hard_fill_paint(color: Rgba) -> skia_rs_safe::paint::Paint {
    let mut paint = fill_paint(color);
    paint.set_anti_alias(false);
    paint
}

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

/// The smallest rectangle containing `border_box` and every *outset*
/// shadow in `shadows`.
///
/// A surface sized to the border box alone clips its own drop shadows away:
/// the shadow is painted, but into pixels the buffer does not have. Callers
/// that own a buffer (the layer window) size and damage against this.
#[must_use]
pub fn outset_shadow_ink_rect(border_box: Rect, shadows: &[Shadow], ctx: &LengthCtx) -> Rect {
    let mut ink = border_box;
    for shadow in shadows {
        if shadow.inset {
            continue;
        }
        let dx = px(&shadow.offset_x, ctx, 0.0);
        let dy = px(&shadow.offset_y, ctx, 0.0);
        let blur = px(&shadow.blur, ctx, 0.0).max(0.0);
        let spread = px(&shadow.spread, ctx, 0.0);
        // The blur's reach, capped exactly as `blit_blurred` caps it, so the
        // ink rect matches what actually gets painted.
        let reach = blur_reach(sigma_within_pad(sigma_for_blur_radius(blur), MAX_BLUR_PAD));
        let grow = spread + reach;
        let shape = Rect::new(
            border_box.x + dx - grow,
            border_box.y + dy - grow,
            border_box.width + grow * 2.0,
            border_box.height + grow * 2.0,
        );
        if shape.x.is_finite() && shape.y.is_finite() && !shape.is_empty() {
            ink = bounding(ink, shape);
        }
    }
    ink
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
    let shape_radii = spread_radii(radii, spread);

    // CSS Backgrounds L3 §7.1.1: an outer shadow is cast "as if the
    // border-box were opaque", and is *not painted inside the border box*.
    // Filling the whole shape instead put an opaque core under the element,
    // which shows through any translucent background.
    let save = canvas.save();
    canvas.clip_path(&rounded_rect_path(base, radii), ClipOp::Difference, true);
    if blur <= 0.0 || blur_reach(sigma_for_blur_radius(blur)) <= 0.0 {
        // A blur too small for the box approximation to resolve (under
        // ~1.2px) used to take the offscreen path, where the pre-blur shape
        // is drawn with anti-aliasing *off* and the blur then no-ops: a
        // jagged hard edge, strictly worse than no blur at all.
        canvas.draw_path(&rounded_rect_path(shape, &shape_radii), &fill_paint(color));
        canvas.restore_to_count(save);
        return;
    }
    let key = blur_key(
        0,
        &[
            rect_parts(shape).as_slice(),
            radii_parts(&shape_radii).as_slice(),
            color_parts(color).as_slice(),
            &[blur],
        ]
        .concat(),
    );
    blit_blurred(
        canvas,
        shape,
        blur,
        Some(key),
        |offscreen_canvas, offset, _surface| {
            let local = Rect::new(
                shape.x - offset.0,
                shape.y - offset.1,
                shape.width,
                shape.height,
            );
            offscreen_canvas.draw_path(
                &rounded_rect_path(local, &shape_radii),
                &hard_fill_paint(color),
            );
        },
    );
    canvas.restore_to_count(save);
}

/// The smallest rectangle containing both `a` and `b`.
///
/// The "everything outside the hole" shape is one even-odd path, and the
/// even-odd rule only punches a hole where the outer contour actually
/// *contains* the inner one; a hole that has been offset or spread clear of
/// the padding box would otherwise come back filled.
fn bounding(a: Rect, b: Rect) -> Rect {
    if b.is_empty() || !(b.x.is_finite() && b.y.is_finite()) {
        return a;
    }
    let x = a.x.min(b.x);
    let y = a.y.min(b.y);
    Rect::new(
        x,
        y,
        a.right().max(b.right()) - x,
        a.bottom().max(b.bottom()) - y,
    )
}

/// Grow every *non-zero* corner radius by `spread`.
///
/// CSS Backgrounds L3 §7.1.1: the spread shape's radii are the element's
/// own, adjusted by the spread -- but "if the border radius is 0, the
/// corner is not rounded", so a square-cornered box casts a square-cornered
/// shadow however large the spread is.
fn spread_radii(radii: &[[f32; 2]; 4], spread: f32) -> [[f32; 2]; 4] {
    let grow = |r: f32| if r > 0.0 { (r + spread).max(0.0) } else { 0.0 };
    std::array::from_fn(|i| [grow(radii[i][0]), grow(radii[i][1])])
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
    let hole_radii = spread_radii(&outer_radii, -spread);

    let save = canvas.save();
    canvas.clip_path(
        &rounded_rect_path(outer, &outer_radii),
        ClipOp::Intersect,
        true,
    );
    // CSS Backgrounds L3 §7.1.1: an inner shadow fills *everything outside*
    // the offset, spread inner rectangle, clipped to the padding box -- it
    // is not a ring bounded at the padding box. Bounding it there made
    // `inset 0 0 4px` (hole == padding box) and Adwaita's
    // `inset 0 2px 2px -2px` (hole strictly *contains* the padding box)
    // paint nothing at all, when both are visible edge glows.
    if blur <= 0.0 || blur_reach(sigma_for_blur_radius(blur)) <= 0.0 {
        let field = bounding(outer.outset([1.0; 4]), hole.outset([1.0; 4]));
        let outside = rounded_ring_path(field, &[[0.0, 0.0]; 4], hole, &hole_radii);
        canvas.draw_path(&outside, &fill_paint(color));
    } else {
        let key = blur_key(
            1,
            &[
                rect_parts(outer).as_slice(),
                radii_parts(&outer_radii).as_slice(),
                rect_parts(hole).as_slice(),
                radii_parts(&hole_radii).as_slice(),
                color_parts(color).as_slice(),
                &[blur],
            ]
            .concat(),
        );
        blit_blurred(
            canvas,
            outer,
            blur,
            Some(key),
            |offscreen_canvas, offset, surface| {
                let local_hole = Rect::new(
                    hole.x - offset.0,
                    hole.y - offset.1,
                    hole.width,
                    hole.height,
                );
                // The whole offscreen surface, minus the hole: the padded
                // margin has to be *inked* so the blur reaching in from it is
                // at full alpha at the padding box's own edge.
                let field = bounding(surface, local_hole.outset([1.0; 4]));
                let outside = rounded_ring_path(field, &[[0.0, 0.0]; 4], local_hole, &hole_radii);
                offscreen_canvas.draw_path(&outside, &hard_fill_paint(color));
            },
        );
    }
    canvas.restore_to_count(save);
}

/// The most padding an offscreen blur surface is given per side.
///
/// Bounds the buffer a single `box-shadow`/`text-shadow` can ask for. The
/// *sigma*, not the padding, is what gets clamped to fit
/// (see [`sigma_within_pad`]): padding a surface by less than the blur's
/// own reach cuts the blur off at a hard rectangular edge, which is exactly
/// what a large blur must not do.
pub(crate) const MAX_BLUR_PAD: f32 = 512.0;

/// The largest offscreen blur surface, per axis.
pub(crate) const MAX_BLUR_SURFACE: f32 = 8192.0;

/// Render `draw` into an offscreen surface padded for `blur`, blur it and
/// blit it at the right place.
///
/// `draw` receives the offscreen's origin in the caller's coordinate space
/// and the offscreen's own rectangle in *offscreen-local* coordinates, so a
/// shape that has to reach the surface edge (an inset shadow's field) can
/// be built without re-deriving the padding.
pub(crate) fn blit_blurred(
    canvas: &mut Canvas<'_>,
    shape: Rect,
    blur: f32,
    key: Option<u64>,
    draw: impl FnOnce(&mut Canvas<'_>, (f32, f32), Rect),
) {
    let sigma = sigma_within_pad(sigma_for_blur_radius(blur), MAX_BLUR_PAD);
    // The blur's *actual* reach, so a sample outside the padding is exactly
    // transparent rather than a truncated tail.
    let pad = blur_reach(sigma).ceil().clamp(0.0, MAX_BLUR_PAD);
    let ox = (shape.x - pad).floor();
    let oy = (shape.y - pad).floor();
    let w = (shape.width + pad * 2.0).ceil();
    let h = (shape.height + pad * 2.0).ceil();
    if !(w.is_finite() && h.is_finite())
        || w <= 0.0
        || h <= 0.0
        || w > MAX_BLUR_SURFACE
        || h > MAX_BLUR_SURFACE
    {
        return;
    }
    if let Some(key) = key
        && let Some(image) = cached_blur(key)
    {
        #[cfg(test)]
        record_blur(BlurEvent::Hit);
        canvas.draw_image(&image, ox, oy, None);
        return;
    }
    let local = Rect::new(0.0, 0.0, w, h);
    #[cfg(test)]
    record_blur(BlurEvent::Rasterised);
    let Some(image) = blurred_image(w as i32, h as i32, sigma, |offscreen| {
        draw(offscreen, (ox, oy), local);
    }) else {
        return;
    };
    if let Some(key) = key {
        remember_blur(key, &image);
    }
    canvas.draw_image(&image, ox, oy, None);
}

/// What the blur cache did for one shadow, counted so a test can observe the
/// cache directly instead of inferring a hit from how long a repaint took.
///
/// The timing version of that assertion flaked on an idle machine (a cold
/// 16.2ms against a warm 8.3ms is a ratio of 1.96, and it wanted 2), because
/// a single wall-clock sample of a few-millisecond paint is mostly noise.
#[cfg(test)]
#[derive(Copy, Clone)]
enum BlurEvent {
    /// The bitmap came back from the cache; nothing was rasterised.
    Hit,
    /// The offscreen was allocated, filled and blurred.
    Rasterised,
}

#[cfg(test)]
thread_local! {
    /// `(hits, rasterisations)` since the last [`reset_blur_counts`].
    ///
    /// Thread-local like `BLUR_CACHE` itself, and libtest gives each test its
    /// own thread, so two tests cannot see each other's counts.
    static BLUR_COUNTS: std::cell::Cell<(u32, u32)> = const { std::cell::Cell::new((0, 0)) };
}

#[cfg(test)]
fn record_blur(event: BlurEvent) {
    BLUR_COUNTS.with(|counts| {
        let (hits, rasterised) = counts.get();
        counts.set(match event {
            BlurEvent::Hit => (hits + 1, rasterised),
            BlurEvent::Rasterised => (hits, rasterised + 1),
        });
    });
}

/// `(hits, rasterisations)` since the last [`reset_blur_counts`].
#[cfg(test)]
fn blur_counts() -> (u32, u32) {
    BLUR_COUNTS.with(std::cell::Cell::get)
}

/// Forget both the counts and the cached bitmaps, so a test starts cold.
#[cfg(test)]
fn reset_blur_counts() {
    BLUR_COUNTS.with(|counts| counts.set((0, 0)));
    BLUR_CACHE.with_borrow_mut(Vec::clear);
}

/// How many blurred shadow bitmaps are kept.
///
/// A shadow is re-rasterised from scratch on every frame it is painted --
/// an offscreen allocation, a fill and six box passes -- even when nothing
/// about it changed, which is every frame of an animation that is
/// transitioning some *other* property. Sixteen is far more than the handful
/// of distinct shadows one widget tree paints, and each entry is one
/// already-allocated `Arc`'d bitmap.
const BLUR_CACHE_CAPACITY: usize = 16;

thread_local! {
    /// The blurred bitmaps painted recently, most-recent last.
    ///
    /// Content-addressed: the key is a hash of *everything* the offscreen
    /// content depends on -- the shape, its radii, the hole, the colour and
    /// the sigma -- so an entry can never be stale and needs no generation
    /// counter to invalidate it. Thread-local because this crate's paint
    /// path is single-threaded by construction (see `text::FontDatabase`).
    static BLUR_CACHE: std::cell::RefCell<Vec<(u64, skia_rs_safe::codec::Image)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// The cached bitmap for `key`, if it is still held.
fn cached_blur(key: u64) -> Option<skia_rs_safe::codec::Image> {
    BLUR_CACHE.with_borrow(|cache| {
        cache
            .iter()
            .find(|(cached, _)| *cached == key)
            .map(|(_, image)| image.clone())
    })
}

/// Remember `image` under `key`, evicting the oldest entry past
/// [`BLUR_CACHE_CAPACITY`].
fn remember_blur(key: u64, image: &skia_rs_safe::codec::Image) {
    BLUR_CACHE.with_borrow_mut(|cache| {
        cache.retain(|(cached, _)| *cached != key);
        if cache.len() >= BLUR_CACHE_CAPACITY {
            cache.remove(0);
        }
        cache.push((key, image.clone()));
    });
}

/// A content hash of everything one blurred shadow's offscreen depends on.
///
/// `tag` separates the outset and inset shapes, which take different
/// geometry; `parts` is that geometry, hashed by bit pattern so a NaN is
/// stable rather than never-equal.
fn blur_key(tag: u8, parts: &[f32]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    tag.hash(&mut hasher);
    for part in parts {
        part.to_bits().hash(&mut hasher);
    }
    hasher.finish()
}

/// The four corner radii, flattened for [`blur_key`].
fn radii_parts(radii: &[[f32; 2]; 4]) -> [f32; 8] {
    [
        radii[0][0],
        radii[0][1],
        radii[1][0],
        radii[1][1],
        radii[2][0],
        radii[2][1],
        radii[3][0],
        radii[3][1],
    ]
}

/// A rect and a colour, flattened for [`blur_key`].
fn rect_parts(rect: Rect) -> [f32; 4] {
    [rect.x, rect.y, rect.width, rect.height]
}

/// `color`'s four channels, flattened for [`blur_key`].
fn color_parts(color: Rgba) -> [f32; 4] {
    [color.r, color.g, color.b, color.a]
}

#[cfg(test)]
mod tests {
    use super::{blur_counts, paint_box_shadows, reset_blur_counts};
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
        // The shape spans x 25..45; the border box (x 20..40) is knocked out
        // of it (F70), so the flat core is sampled in 40..45 rather than at
        // the old x == 35, which is inside the border box.
        assert_eq!(pixel(&surface, 42, 20), Color(0xFF00_0000));
        assert_eq!(
            pixel(&surface, 22, 20).alpha(),
            0,
            "left of the offset shadow"
        );
    }

    #[test]
    fn an_outset_shadow_is_not_painted_inside_the_border_box() {
        // F70. CSS Backgrounds L3 §7.1.1: an outer shadow is cast as if the
        // border box were opaque and is not painted inside it. Painting the
        // full shape put an opaque core under the element, which shows
        // through a translucent background.
        // Mutation check: dropping the Difference clip inks (30, 20).
        let surface = painted(&[shadow(0.0, 0.0, 0.0, 6.0, false)], false);
        assert_eq!(
            pixel(&surface, 30, 20).alpha(),
            0,
            "nothing under the border box"
        );
        assert_eq!(
            pixel(&surface, 16, 20),
            Color(0xFF00_0000),
            "the spread ring outside it still paints"
        );
    }

    #[test]
    fn spread_leaves_a_square_corner_square() {
        // F69. `box-shadow: 0 0 0 8px` on border-radius 0 must stay a sharp
        // ring; adding the spread to a zero radius rounded it.
        // Mutation check: growing zero radii too leaves (13, 8) clear.
        let surface = painted(&[shadow(0.0, 0.0, 0.0, 8.0, false)], false);
        assert_eq!(
            pixel(&surface, 13, 8),
            Color(0xFF00_0000),
            "the spread shape's own top-left corner is square"
        );
    }

    #[test]
    fn an_inset_shadow_with_no_offset_or_spread_still_paints_its_blur() {
        // F71. The hole equals the padding box here, so the old even-odd
        // ring was two identical contours and filled nothing at all.
        // Mutation check: rebuilding the ring bounded at `outer` clears
        // (21, 20).
        // blur 4 -> sigma 2 -> box radii [1, 1, 2], reach 4px inward.
        let surface = painted(&[shadow(0.0, 0.0, 4.0, 0.0, true)], true);
        assert!(
            pixel(&surface, 22, 20).alpha() > 0,
            "the blur reaches 2px in from the padding box's left edge"
        );
        assert_eq!(
            pixel(&surface, 30, 20).alpha(),
            0,
            "the centre is 5px from every edge, past a 4px reach"
        );
        assert_eq!(pixel(&surface, 5, 5).alpha(), 0, "nothing outside the box");
    }

    #[test]
    fn an_inset_shadow_whose_hole_swallows_the_box_still_glows_at_the_edge() {
        // F71, Adwaita's `inset 0 2px 2px -2px`: the negative spread makes
        // the hole strictly *contain* the padding box, so the even-odd ring
        // was empty. Filling outside the hole leaves the blur bleeding in
        // from the hole's top edge -- the 2px top highlight Adwaita wants.
        // Adwaita's shape with the blur widened to 4px so the 4px reach is
        // legible on this 20x10 box: hole == (18, 15, 24, 14) strictly
        // contains the padding box (20, 15, 20, 10), and its top edge sits
        // exactly on the box's, so the ink outside it blurs downward in.
        let surface = painted(&[shadow(0.0, 2.0, 4.0, -2.0, true)], true);
        assert!(pixel(&surface, 30, 16).alpha() > 0, "the top edge is lit");
        assert_eq!(
            pixel(&surface, 30, 23).alpha(),
            0,
            "the bottom of the box is not"
        );
    }

    #[test]
    fn a_blur_too_small_for_the_box_passes_is_still_anti_aliased() {
        // F63. Under ~1.2px of blur `box_radii_for_gauss` returns [0, 0, 0]
        // and the blur no-ops -- but the pre-blur shape is drawn with
        // anti-aliasing *off*, so a 1px blur used to render a jagged edge,
        // strictly worse than blur 0. Mutation check: routing this back
        // through `blit_blurred` makes the rounded corner hard-stepped.
        let mut surface = Surface::new_raster_n32_premul(60, 40).expect("raster surface");
        surface.canvas().clear(Color::TRANSPARENT);
        {
            let mut canvas = surface.canvas();
            paint_box_shadows(
                &mut canvas,
                &[shadow(0.0, 0.0, 1.0, 6.0, false)],
                false,
                &alloc(),
                &[[6.0, 6.0]; 4],
                black(),
                &ctx(),
            );
        }
        // The spread shape's top-left corner arc: an anti-aliased fill
        // leaves a partially-covered pixel on the diagonal, a hard fill
        // never does.
        let soft = (10..20)
            .flat_map(|x| (5..15).map(move |y| (x, y)))
            .any(|(x, y)| {
                let a = pixel(&surface, x, y).alpha();
                a > 0 && a < 0xFF
            });
        assert!(soft, "a sub-pixel blur radius still anti-aliases the shape");
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

    /// A large-radius `box-shadow` must not take a user-visible amount of
    /// time to paint. The box passes used to re-sum the whole `2r + 1`
    /// window per pixel -- `O(w * h * r)` -- so `0 8px 200px` on a 600x400
    /// box (a 1200x1000 offscreen, radius ~100) took minutes in a debug
    /// build. The sliding-window passes are `O(w * h)`, radius-independent.
    ///
    /// The bound is deliberately generous: this is a "did the complexity
    /// regress" guard, not a latency budget. A debug build's floor for the
    /// two offscreens this paints (1200x1000 and 1624x1424, three box passes
    /// per axis each) is a few seconds of pure per-pixel work no correct
    /// `O(w * h)` implementation can go under; the radius-independence of
    /// the passes themselves is pinned tightly in
    /// `blur::tests::a_large_sigma_blur_costs_no_more_than_a_small_one`.
    #[test]
    fn a_large_radius_shadow_paints_in_bounded_time() {
        let big = Rect::new(0.0, 0.0, 600.0, 400.0);
        let alloc = Allocation {
            border_box: big,
            content_box: big,
            border: [0.0; 4],
            padding: [0.0; 4],
        };
        let mut surface = Surface::new_raster_n32_premul(600, 400).expect("raster surface");
        surface.canvas().clear(Color::TRANSPARENT);
        let started = std::time::Instant::now();
        {
            let mut canvas = surface.canvas();
            paint_box_shadows(
                &mut canvas,
                &[shadow(0.0, 8.0, 200.0, 0.0, false)],
                false,
                &alloc,
                &[[0.0, 0.0]; 4],
                black(),
                &ctx(),
            );
            // The pathological inset case from the review: a blur and spread
            // far larger than the box, which must be bounded by the same
            // offscreen surface cap the outset path uses.
            paint_box_shadows(
                &mut canvas,
                &[shadow(0.0, 0.0, 999_999.0, 999_999.0, true)],
                true,
                &alloc,
                &[[0.0, 0.0]; 4],
                black(),
                &ctx(),
            );
        }
        let elapsed = started.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(60),
            "a large-radius box-shadow took {elapsed:?}; the box blur is not O(w * h)"
        );
    }

    #[test]
    fn shadow_painting_never_panics_on_hostile_lengths() {
        for &v in &[f32::NAN, f32::INFINITY, -1.0e9, 1.0e9] {
            let _ = painted(&[shadow(v, v, v, v, false)], false);
            let _ = painted(&[shadow(v, v, v, v, true)], true);
        }
    }
    #[test]
    fn an_identical_blurred_shadow_is_rasterised_once_and_reused() {
        // A blurred shadow was re-rasterised from scratch every frame it was
        // painted -- an offscreen allocation, a fill and six box passes --
        // even when nothing about it had changed, which is every frame of an
        // animation transitioning some *other* property.
        //
        // Observed at the cache rather than on the clock: this used to assert
        // that the second paint took less than half as long as the first,
        // which flaked on an idle machine (16.2ms cold against 8.3ms warm is
        // a ratio of 1.96) because one wall-clock sample of a few-millisecond
        // paint is mostly noise. What the fix actually claims is that the
        // second paint rasterises *nothing*, and that is now what is asserted.
        //
        // Mutation check: pass `None` for the key at both call sites in
        // `draw_blurred` and the second paint rasterises again, so the counts
        // below read `(0, 2)`.
        let shadows = [shadow(0.0, 0.0, 40.0, 0.0, false)];
        reset_blur_counts();

        let cold = painted(&shadows, false);
        assert_eq!(
            blur_counts(),
            (0, 1),
            "the first paint of a shadow must rasterise it, and exactly once"
        );

        let warm = painted(&shadows, false);
        assert_eq!(
            blur_counts(),
            (1, 1),
            "the second paint of an identical shadow rasterised it again"
        );

        // Same pixels, either way -- the cache is content-addressed.
        for (x, y) in [(20, 12), (30, 20), (5, 5), (55, 35)] {
            assert_eq!(
                pixel(&cold, x, y),
                pixel(&warm, x, y),
                "the cached blit differs at ({x}, {y})"
            );
        }

        // A *different* shadow is a different key, so it misses and is
        // rasterised: the cache is not simply answering everything.
        let _ = painted(&[shadow(0.0, 0.0, 24.0, 0.0, false)], false);
        assert_eq!(
            blur_counts(),
            (1, 2),
            "a shadow with a different blur radius must not hit the cache"
        );
    }
}
