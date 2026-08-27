//! `background-color` plus the `background-*` layer stack.
//!
//! Layers arrive top-first (CSS: the first listed image is topmost), so they
//! are painted in reverse; the colour goes underneath them all, clipped by
//! the *last* layer's `background-clip` -- CSS Backgrounds L3 §3.11.

use skia_rs_safe::canvas::{Canvas, ClipOp};
use skia_rs_safe::paint::{BlendMode, Paint};

use std::f32::consts::SQRT_2;

use crate::css::computed::BackgroundLayer;
use crate::css::value::image::{GradientKind, RadialExtent, RadialShape};
use crate::css::value::{BgSize, Gradient, Image, Keyword, LengthCtx, Position, RepeatStyle, Rgba};
use crate::layout::{Allocation, Rect};
use crate::paint::{PaintCx, fill_paint, radii_for_box, rounded_rect_path};

/// Paint the background colour and every layer of `layers`.
pub fn paint_backgrounds(
    canvas: &mut Canvas<'_>,
    color: Rgba,
    layers: &[BackgroundLayer],
    alloc: &Allocation,
    radii: &[[f32; 2]; 4],
    cx: &mut PaintCx<'_>,
) {
    let colour_clip = layers.last().map_or(Keyword::BorderBox, |l| l.clip);
    if color.a > 0.0 {
        let rect = alloc.box_for(colour_clip);
        if !rect.is_empty() {
            let path = rounded_rect_path(rect, &radii_for_box(radii, alloc, colour_clip));
            canvas.draw_path(&path, &fill_paint(color));
        }
    }

    for layer in layers.iter().rev() {
        paint_layer(canvas, layer, alloc, radii, cx, color);
    }
}

/// Paint one `background-image` layer under its own clip.
fn paint_layer(
    canvas: &mut Canvas<'_>,
    layer: &BackgroundLayer,
    alloc: &Allocation,
    radii: &[[f32; 2]; 4],
    cx: &mut PaintCx<'_>,
    current: Rgba,
) {
    if matches!(layer.image, Image::None) {
        return;
    }
    let clip_rect = alloc.box_for(layer.clip);
    if clip_rect.is_empty() {
        return;
    }

    let save = canvas.save();
    let clip_path = rounded_rect_path(clip_rect, &radii_for_box(radii, alloc, layer.clip));
    canvas.clip_path(&clip_path, ClipOp::Intersect, true);

    // `background-size`, `-position` and `-repeat` apply to *every* layer
    // image, not just `url()`: a gradient or an `image(<color>)` is sized and
    // tiled exactly like a bitmap (CSS Backgrounds L3 §3.5-§3.9). Only
    // `url()` has an intrinsic size for `auto` to fall back to; a gradient's
    // `auto` is the whole origin box, which is what `layer_tile` does with
    // `None`.
    let origin = alloc.box_for(layer.origin);
    if origin.is_empty() {
        canvas.restore_to_count(save);
        return;
    }
    let len_ctx = cx.base_length_ctx();
    let intrinsic = match &layer.image {
        Image::Url(url) => {
            let Some(size) = cx
                .images
                .get(url)
                .map(|img| (img.width() as f32, img.height() as f32))
            else {
                // Recorded unresolved: paints nothing, already logged once.
                canvas.restore_to_count(save);
                return;
            };
            Some(size)
        }
        _ => None,
    };
    let tile = layer_tile(layer, origin, intrinsic, &len_ctx);
    if tile.rect.is_empty() {
        canvas.restore_to_count(save);
        return;
    }
    let rects = tile_positions(&tile, clip_rect, layer.repeat);
    let blend = blend_mode_for(layer.blend);

    match &layer.image {
        Image::Solid(color) => {
            if let Some(rgba) = color.resolve(&cx.color_ctx(current)) {
                let mut paint = fill_paint(rgba);
                paint.set_anti_alias(false);
                paint.set_blend_mode(blend);
                for rect in rects {
                    canvas.draw_rect(&rect.to_skia(), &paint);
                }
            }
        }
        Image::Gradient(gradient) => {
            for rect in rects {
                // Each copy is its own gradient box: the gradient line is
                // sized against the tile, and the copy is clipped to itself
                // so `no-repeat` cannot flood the clip box.
                let Some(piece) = intersect(rect, clip_rect) else {
                    continue;
                };
                paint_gradient(canvas, gradient, rect, piece, cx, current, blend);
            }
        }
        Image::Url(url) => {
            let mut paint = Paint::new();
            paint.set_blend_mode(blend);
            if let Some(image) = cx.images.get(url) {
                for rect in rects {
                    canvas.draw_image_rect(image, None, &rect.to_skia(), Some(&paint));
                }
            }
        }
        // `cross-fade()` and `-gtk-*` icon images are stored by the registry
        // and drawn in M4; they paint nothing here (spec, Out of scope).
        Image::None | Image::CrossFade(_) | Image::Icon(_) => {}
    }

    canvas.restore_to_count(save);
}

/// Resolve a `Position` against a box of `w` x `h`, in box-local px.
fn resolve_position(position: &Position, w: f32, h: f32, ctx: &LengthCtx) -> (f32, f32) {
    let mut x_ctx = *ctx;
    x_ctx.percent_basis = Some(w);
    let mut y_ctx = *ctx;
    y_ctx.percent_basis = Some(h);
    (
        position.x.resolve(&x_ctx).unwrap_or(w / 2.0),
        position.y.resolve(&y_ctx).unwrap_or(h / 2.0),
    )
}

/// The gradient-line parameter for the box-local point `(px, py)`.
///
/// `t` is what `Gradient::color_at` consumes: 0 at the first stop position
/// and 1 at the last, before repeating and stop placement are applied.
#[must_use]
pub fn gradient_t(
    gradient: &Gradient,
    box_w: f32,
    box_h: f32,
    px: f32,
    py: f32,
    ctx: &LengthCtx,
) -> f32 {
    match &gradient.kind {
        GradientKind::Linear { .. } => {
            let (p0, p1) = gradient.line_for_box(box_w, box_h, ctx);
            let (dx, dy) = (p1.x - p0.x, p1.y - p0.y);
            let len2 = dx.mul_add(dx, dy * dy);
            if !len2.is_finite() || len2 <= f32::EPSILON {
                return 1.0;
            }
            ((px - p0.x) * dx + (py - p0.y) * dy) / len2
        }
        GradientKind::Radial {
            shape,
            extent,
            position,
        } => {
            let (cx, cy) = resolve_position(position, box_w, box_h, ctx);
            let (rx, ry) = radial_radii(*shape, extent, cx, cy, box_w, box_h, ctx);
            if !(rx.is_finite() && ry.is_finite()) || rx <= 0.0 || ry <= 0.0 {
                return 1.0;
            }
            let nx = (px - cx) / rx;
            let ny = (py - cy) / ry;
            nx.hypot(ny)
        }
        GradientKind::Conic {
            from_angle,
            position,
        } => {
            let (cx, cy) = resolve_position(position, box_w, box_h, ctx);
            // CSS conic-gradient: 0deg points straight up, angles increase
            // clockwise.
            let angle = (px - cx).atan2(cy - py).to_degrees() - from_angle;
            let wrapped = angle.rem_euclid(360.0);
            if wrapped.is_finite() {
                wrapped / 360.0
            } else {
                0.0
            }
        }
    }
}

/// The x and y radii of a radial gradient's ending shape.
fn radial_radii(
    shape: RadialShape,
    extent: &RadialExtent,
    cx: f32,
    cy: f32,
    w: f32,
    h: f32,
    ctx: &LengthCtx,
) -> (f32, f32) {
    let (left, right, top, bottom) = (cx, w - cx, cy, h - cy);
    let (sx, sy) = (left.abs().min(right.abs()), top.abs().min(bottom.abs()));
    let (fx, fy) = (left.abs().max(right.abs()), top.abs().max(bottom.abs()));
    let (rx, ry) = match extent {
        RadialExtent::ClosestSide => (sx, sy),
        RadialExtent::FarthestSide => (fx, fy),
        // CSS Images L3 §3.2: an *ellipse* sized to a corner has the same
        // aspect ratio as the matching `-side` ellipse and passes through
        // that corner, which works out to `side * sqrt(2)` per axis --
        // not one circular `hypot(sx, sy)` on both axes. The circle arm
        // below still uses the true corner distance.
        RadialExtent::ClosestCorner => (sx * SQRT_2, sy * SQRT_2),
        RadialExtent::FarthestCorner => (fx * SQRT_2, fy * SQRT_2),
        RadialExtent::Explicit(x, y) => {
            let mut x_ctx = *ctx;
            x_ctx.percent_basis = Some(w);
            let mut y_ctx = *ctx;
            y_ctx.percent_basis = Some(h);
            (
                x.resolve(&x_ctx).unwrap_or(0.0),
                y.resolve(&y_ctx).unwrap_or(0.0),
            )
        }
    };
    match shape {
        RadialShape::Circle => {
            let r = match extent {
                RadialExtent::ClosestSide => sx.min(sy),
                RadialExtent::FarthestSide => fx.max(fy),
                RadialExtent::ClosestCorner => sx.hypot(sy),
                RadialExtent::FarthestCorner => fx.hypot(fy),
                RadialExtent::Explicit(..) => rx,
            };
            (r, r)
        }
        RadialShape::Ellipse => (rx, ry),
    }
}

/// The current clip's bounds, mapped back into the coordinate space the
/// caller draws in.
///
/// `Canvas::clip_bounds` is in device pixels and is always intersected with
/// the surface rectangle, so this is a *finite* superset of every pixel a
/// draw call can touch. `None` means the CTM is singular, so nothing this
/// function's caller draws could land anywhere.
fn drawable_bounds(canvas: &Canvas<'_>) -> Option<Rect> {
    let device = canvas.clip_bounds();
    let inverse = canvas.total_matrix().invert()?;
    // Under a rotation the inverse-mapped rect is the quad's bounding box,
    // which is still a superset -- exactly what a loop bound needs.
    let local = inverse.map_rect(&device);
    Some(Rect::new(
        local.left,
        local.top,
        local.right - local.left,
        local.bottom - local.top,
    ))
}

/// The overlap of `a` and `b`, or `None` when they do not overlap.
///
/// A non-finite edge propagates into a non-positive extent, which
/// `Rect::is_empty` rejects, so a NaN or infinite input yields `None`.
fn intersect(a: Rect, b: Rect) -> Option<Rect> {
    let x = a.x.max(b.x);
    let y = a.y.max(b.y);
    let right = a.right().min(b.right());
    let bottom = a.bottom().min(b.bottom());
    let out = Rect::new(x, y, right - x, bottom - y);
    (!out.is_empty() && x.is_finite() && y.is_finite()).then_some(out)
}

/// Fill `clip` with `gradient`, sized against `origin`.
///
/// Axis-aligned linear gradients are filled band-by-band -- one `draw_rect`
/// per constant row or column -- which is exactly M1's model and keeps the
/// gate's band assertions exact. Everything else samples per pixel, in runs
/// of equal colour.
///
/// # Bounded work
///
/// Every loop below is driven by the *painted* rectangle, which is `clip`
/// intersected with [`drawable_bounds`] -- never by `clip` alone.
/// `Rect::is_empty` accepts `+inf` and huge finite extents, so a CSS-reachable
/// clip of `1e9` px (or an infinite one) would otherwise spin up to
/// `i32::MAX` iterations, each issuing a `draw_rect`: a hang, not a panic.
/// Clamping to the surface makes the cost proportional to visible pixels.
pub fn paint_gradient(
    canvas: &mut Canvas<'_>,
    gradient: &Gradient,
    origin: Rect,
    clip: Rect,
    cx: &mut PaintCx<'_>,
    current: Rgba,
    blend: BlendMode,
) {
    if origin.is_empty() || clip.is_empty() {
        return;
    }
    let Some(bounds) = drawable_bounds(canvas) else {
        return;
    };
    let Some(clip) = intersect(clip, bounds) else {
        return;
    };
    let len_ctx = &LengthCtx {
        percent_basis: Some(origin.width),
        ..cx.base_length_ctx()
    };
    let color_ctx = cx.color_ctx(current);
    let (x0, x1) = (clip.x.floor() as i32, clip.right().ceil() as i32);
    let (y0, y1) = (clip.y.floor() as i32, clip.bottom().ceil() as i32);

    let vertical = match &gradient.kind {
        GradientKind::Linear { .. } => {
            let (p0, p1) = gradient.line_for_box(origin.width, origin.height, len_ctx);
            Some((p1.x - p0.x).abs() < 1.0e-4)
        }
        _ => None,
    };
    let horizontal = match &gradient.kind {
        GradientKind::Linear { .. } => {
            let (p0, p1) = gradient.line_for_box(origin.width, origin.height, len_ctx);
            Some((p1.y - p0.y).abs() < 1.0e-4)
        }
        _ => None,
    };

    let band = |canvas: &mut Canvas<'_>, rect: Rect, color: Rgba| {
        let mut paint = fill_paint(color);
        paint.set_anti_alias(false);
        paint.set_blend_mode(blend);
        canvas.draw_rect(&rect.to_skia(), &paint);
    };

    if vertical == Some(true) {
        for row in y0..y1 {
            let py = row as f32 + 0.5 - origin.y;
            let t = gradient_t(gradient, origin.width, origin.height, 0.0, py, len_ctx);
            let color = gradient.color_at(t, &color_ctx, len_ctx);
            band(
                canvas,
                Rect::new(clip.x, row as f32, clip.width, 1.0),
                color,
            );
        }
        return;
    }
    if horizontal == Some(true) {
        for col in x0..x1 {
            let px = col as f32 + 0.5 - origin.x;
            let t = gradient_t(gradient, origin.width, origin.height, px, 0.0, len_ctx);
            let color = gradient.color_at(t, &color_ctx, len_ctx);
            band(
                canvas,
                Rect::new(col as f32, clip.y, 1.0, clip.height),
                color,
            );
        }
        return;
    }

    // Per-pixel sampling, emitted as horizontal runs of the identical colour.
    // The sampled colour of every pixel is unchanged -- only the number of
    // `draw_rect` calls drops, from one per pixel to one per colour change.
    for row in y0..y1 {
        let py = row as f32 + 0.5 - origin.y;
        let mut run_start = x0;
        let mut run_color: Option<Rgba> = None;
        for col in x0..x1 {
            let px = col as f32 + 0.5 - origin.x;
            let t = gradient_t(gradient, origin.width, origin.height, px, py, len_ctx);
            let color = gradient.color_at(t, &color_ctx, len_ctx);
            match run_color {
                Some(previous) if previous == color => {}
                Some(previous) => {
                    let width = (col - run_start) as f32;
                    band(
                        canvas,
                        Rect::new(run_start as f32, row as f32, width, 1.0),
                        previous,
                    );
                    run_start = col;
                    run_color = Some(color);
                }
                None => {
                    run_start = col;
                    run_color = Some(color);
                }
            }
        }
        if let Some(previous) = run_color {
            let width = (x1 - run_start) as f32;
            band(
                canvas,
                Rect::new(run_start as f32, row as f32, width, 1.0),
                previous,
            );
        }
    }
}

/// One placed copy of a background image, plus its repeat pitch.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Tile {
    /// Where the first copy goes.
    pub rect: Rect,
    /// Horizontal distance between copies.
    pub step_x: f32,
    /// Vertical distance between copies.
    pub step_y: f32,
}

/// Size and place one layer's image inside `origin`.
///
/// `intrinsic` is the image's own size when it has one (a decoded `url()`);
/// gradients have none, so `auto` resolves to the whole origin box.
#[must_use]
pub fn layer_tile(
    layer: &BackgroundLayer,
    origin: Rect,
    intrinsic: Option<(f32, f32)>,
    ctx: &LengthCtx,
) -> Tile {
    let (iw, ih) = intrinsic.unwrap_or((origin.width, origin.height));
    let mut w_ctx = *ctx;
    w_ctx.percent_basis = Some(origin.width);
    let mut h_ctx = *ctx;
    h_ctx.percent_basis = Some(origin.height);

    let (w, h) = match &layer.size {
        BgSize::Auto => (iw, ih),
        BgSize::Cover | BgSize::Contain => {
            if iw <= 0.0 || ih <= 0.0 {
                (origin.width, origin.height)
            } else {
                let sx = origin.width / iw;
                let sy = origin.height / ih;
                let s = if matches!(layer.size, BgSize::Cover) {
                    sx.max(sy)
                } else {
                    sx.min(sy)
                };
                (iw * s, ih * s)
            }
        }
        BgSize::Explicit(x, y) => {
            // CSS Backgrounds L3 §3.9: with one axis given and the other
            // `auto`, the `auto` axis is scaled to keep the image's own
            // aspect ratio -- it does *not* fall back to the raw intrinsic
            // size. `background-size: 100% auto` on a 200px box with a
            // 10x40 image is (200, 800), not (200, 40).
            let given_w = x.resolve(&w_ctx);
            let given_h = y.resolve(&h_ctx);
            let ratio = (iw > 0.0 && ih > 0.0).then(|| ih / iw);
            match (given_w, given_h, ratio) {
                (Some(w), None, Some(r)) => (w, w * r),
                (None, Some(h), Some(r)) => (h / r, h),
                (w, h, _) => (w.unwrap_or(iw), h.unwrap_or(ih)),
            }
        }
    };
    let (w, h) = (
        if w.is_finite() && w > 0.0 { w } else { 0.0 },
        if h.is_finite() && h > 0.0 { h } else { 0.0 },
    );

    // CSS resolves a background-position percentage against
    // `container - image`, not against the container.
    let mut px_ctx = *ctx;
    px_ctx.percent_basis = Some(origin.width - w);
    let mut py_ctx = *ctx;
    py_ctx.percent_basis = Some(origin.height - h);
    let x = layer.position.x.resolve(&px_ctx).unwrap_or(0.0);
    let y = layer.position.y.resolve(&py_ctx).unwrap_or(0.0);

    Tile {
        rect: Rect::new(origin.x + x, origin.y + y, w, h),
        step_x: w,
        step_y: h,
    }
}

/// The most tile copies one axis may emit.
///
/// `background-size` is CSS-reachable at any positive magnitude, and
/// `layer_tile` only rejects a non-finite or non-positive step -- so
/// `background-size: 0.01px; background-repeat: repeat` over a 100px box
/// asks for 10 000 copies per axis (100 million rects, ~1.6 GB, and worse
/// as the size shrinks). Two copies inside the same device pixel cannot be
/// told apart on the device grid, so each axis emits at most one copy per
/// device pixel of clip, plus one at each end, and never more than
/// `MAX_TILES_PER_AXIS` however large the clip claims to be.
const MAX_TILES_PER_AXIS: i32 = 4096;

/// The narrowest repeat step, in px.
///
/// One device pixel: two copies starting inside the same pixel are
/// indistinguishable on the device grid, so nothing is lost by stepping at
/// least this far -- and it is what keeps a `background-size: 0.01px`
/// repeat proportional to the clip's own pixel count instead of to
/// `clip / size`.
const MIN_TILE_STEP: f32 = 1.0;

/// The most tile copies both axes together may emit.
///
/// The per-axis cap alone still allows `MAX_TILES_PER_AXIS` squared.
const MAX_TILES_TOTAL: usize = 1 << 16;

/// Every copy of `tile` that intersects `clip`, per `repeat`.
///
/// The returned vector is bounded by [`MAX_TILES_TOTAL`] for every input,
/// including a sub-device-pixel step or an infinite clip.
#[must_use]
pub fn tile_positions(tile: &Tile, clip: Rect, repeat: RepeatStyle) -> Vec<Rect> {
    let repeats = |axis: Keyword| matches!(axis, Keyword::Repeat | Keyword::Round | Keyword::Space);
    // Two copies whose starts fall inside the same device pixel cannot be
    // told apart on the device grid, so a sub-pixel step is *widened* to one
    // device pixel, with the copy's own extent scaled by the same factor so
    // the copies still meet. Capping the copy *count* instead -- what this
    // did -- left most of the box unpainted: `background-size: 2%` on a 40px
    // box stepped 0.8px, and 42 copies covered 33.6px of it.
    let widen = |step: f32, extent: f32| -> (f32, f32) {
        if !(step.is_finite() && step > 0.0) || step >= MIN_TILE_STEP {
            return (step, extent);
        }
        let factor = MIN_TILE_STEP / step;
        (MIN_TILE_STEP, extent * factor)
    };
    let (step_x, width) = widen(tile.step_x, tile.rect.width);
    let (step_y, height) = widen(tile.step_y, tile.rect.height);
    let count = |start: f32, step: f32, lo: f32, hi: f32| -> (i32, i32) {
        if !(step.is_finite() && step > 0.0) || !(start.is_finite() && lo.is_finite()) {
            return (0, 0);
        }
        let first = ((lo - start) / step).floor() as i32;
        // Not `.ceil()`: for a non-exact `(hi - start) / step` (e.g. a 40px
        // clip tiled from x=5 in 10px steps -> 3.5), rounding up lands a
        // whole extra step past `hi` with zero overlap. `.floor()` keeps the
        // last index whose start is at-or-before `hi` (still touching-
        // inclusive when the division is exact, e.g. the y axis below).
        // `as i32` saturates, so a non-finite `hi` lands on `i32::MAX`; the
        // cap below is what actually bounds the loop.
        let last = (((hi - start) / step).floor() as i32).max(first);
        let span = hi - lo;
        let cap = if span.is_finite() {
            // `as i64` saturates too, so a huge finite span is safe here.
            i32::try_from((span.ceil() as i64).clamp(0, i64::from(MAX_TILES_PER_AXIS)))
                .unwrap_or(MAX_TILES_PER_AXIS)
                .saturating_add(2)
                .min(MAX_TILES_PER_AXIS)
        } else {
            MAX_TILES_PER_AXIS
        };
        (first, last.min(first.saturating_add(cap - 1)))
    };

    let (ix0, ix1) = if repeats(repeat.x) {
        count(tile.rect.x, step_x, clip.x, clip.right())
    } else {
        (0, 0)
    };
    let (iy0, iy1) = if repeats(repeat.y) {
        count(tile.rect.y, step_y, clip.y, clip.bottom())
    } else {
        (0, 0)
    };

    let mut out = Vec::new();
    'rows: for iy in iy0..=iy1 {
        for ix in ix0..=ix1 {
            if out.len() >= MAX_TILES_TOTAL {
                break 'rows;
            }
            out.push(Rect::new(
                step_x.mul_add(ix as f32, tile.rect.x),
                step_y.mul_add(iy as f32, tile.rect.y),
                width,
                height,
            ));
        }
    }
    if out.is_empty() {
        out.push(tile.rect);
    }
    out
}

/// CSS `background-blend-mode` keyword -> `skia-rs` blend mode.
#[must_use]
pub fn blend_mode_for(keyword: Keyword) -> BlendMode {
    match keyword {
        Keyword::Multiply => BlendMode::Multiply,
        Keyword::Screen => BlendMode::Screen,
        Keyword::Overlay => BlendMode::Overlay,
        Keyword::Darken => BlendMode::Darken,
        Keyword::Lighten => BlendMode::Lighten,
        Keyword::ColorDodge => BlendMode::ColorDodge,
        Keyword::ColorBurn => BlendMode::ColorBurn,
        Keyword::HardLight => BlendMode::HardLight,
        Keyword::SoftLight => BlendMode::SoftLight,
        Keyword::Difference => BlendMode::Difference,
        Keyword::Exclusion => BlendMode::Exclusion,
        Keyword::Hue => BlendMode::Hue,
        Keyword::Saturation => BlendMode::Saturation,
        Keyword::ColorBlend => BlendMode::Color,
        Keyword::Luminosity => BlendMode::Luminosity,
        _ => BlendMode::SrcOver,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_TILES_TOTAL, Tile, blend_mode_for, gradient_t, layer_tile, paint_gradient,
        tile_positions,
    };
    use crate::css::cascade::CompiledSheet;
    use crate::css::computed::{ComputedStyle, ResolveEnv};
    use crate::css::node::Node;
    use crate::css::registry::Prop;
    use crate::css::select::MatchCx;
    use std::rc::Rc;

    use crate::css::value::RepeatStyle;
    use crate::css::value::{Gradient, Image, Keyword, Rgba};
    use crate::layout::{Allocation, Rect};
    use crate::paint::{ImageCache, PaintCx, paint_backgrounds, radii_for_box};
    use crate::text::FontDatabase;
    use skia_rs_safe::canvas::Surface;
    use skia_rs_safe::core::Color;
    use skia_rs_safe::paint::BlendMode;

    /// A 40x20 node with a 4px border and 4px padding, painted from `css`.
    fn painted(css: &str) -> Surface {
        painted_with(css, [4.0; 4], [4.0; 4])
    }

    /// A 40x20 node with no border and no padding, so the origin box, the
    /// clip box and the surface are all the same rectangle.
    ///
    /// The gradient tests need this: `painted` hard-codes a 4px border and
    /// 4px padding whatever the CSS says, so its origin box is (4, 4, 32, 12)
    /// while its clip box is the whole surface. Sampling the gradient's own
    /// shape outside its origin box only worked while a gradient layer
    /// ignored `background-repeat` and flooded the clip; with the default
    /// `repeat` honoured, the area outside the origin box is the *next copy*.
    fn painted_flat(css: &str) -> Surface {
        painted_with(css, [0.0; 4], [0.0; 4])
    }

    fn painted_with(css: &str, border: [f32; 4], padding: [f32; 4]) -> Surface {
        let sheet = CompiledSheet::compile(css);
        let window = Node::new("window");
        let button = Node::new("button");
        window.append_child(&button);
        let env = ResolveEnv::default();
        let mut cx = MatchCx::new();
        let style = ComputedStyle::resolve_chain(&sheet, &button, &env, &mut cx);

        let border_box = Rect::new(0.0, 0.0, 40.0, 20.0);
        let sides = std::array::from_fn(|i| border[i] + padding[i]);
        let alloc = Allocation {
            border_box,
            content_box: border_box.inset(sides),
            border,
            padding,
        };
        let radii = style.border_radii(40.0, 20.0);
        let layers = style.background_layers();
        let mut fonts = FontDatabase::probe_only();
        let mut images = ImageCache::new();
        let mut paint_cx = PaintCx {
            env: &env,
            colors: &sheet.colors,
            fonts: &mut fonts,
            images: &mut images,
            text: None,
        };
        let mut surface = Surface::new_raster_n32_premul(40, 20).expect("raster surface");
        surface.canvas().clear(Color::TRANSPARENT);
        {
            let mut canvas = surface.canvas();
            paint_backgrounds(
                &mut canvas,
                style.get::<Rgba>(Prop::BackgroundColor),
                &layers,
                &alloc,
                &radii,
                &mut paint_cx,
            );
        }
        surface
    }

    fn pixel(surface: &Surface, x: i32, y: i32) -> Color {
        surface
            .pixel_buffer()
            .get_pixel(x, y)
            .unwrap_or_else(|| panic!("pixel ({x}, {y}) is outside the surface"))
    }

    // Reconciliation (see task-8-report.md): `ComputedStyle::background_layers`
    // (P3, frozen) drops every entry whose `background-image` is `none` --
    // clip/origin included -- because it treats "layer" as "has an image".
    // A bare `background-clip` with no image therefore never reaches
    // `paint_backgrounds`'s colour-clip step through `layers`, which is the
    // only channel `paint_backgrounds` has into the cascade. `image(<color>)`
    // matching `background-color` exactly keeps every asserted pixel the
    // plan's colour would have produced while giving the clip a real layer
    // to travel on -- the same pattern Adwaita's own `:active` rule uses.
    const SQUARE: &str = "button { background-color: #112233; \
                          background-image: image(#112233); border-radius: 0; \
                          border: 4px solid transparent; padding: 4px }";

    #[test]
    fn the_background_fills_the_border_box_by_default() {
        // M1's E9 regression, preserved: the background was clipped to the
        // padding box unconditionally, so a translucent border showed the
        // surface through. CSS and GTK default to border-box.
        let surface = painted(SQUARE);
        assert_eq!(pixel(&surface, 0, 0), Color(0xFF11_2233));
        assert_eq!(pixel(&surface, 20, 10), Color(0xFF11_2233));
    }

    #[test]
    fn padding_box_clips_the_background_inside_the_border() {
        let surface = painted(&format!(
            "{SQUARE}\nbutton {{ background-clip: padding-box }}"
        ));
        assert_eq!(pixel(&surface, 0, 0).alpha(), 0);
        assert_eq!(pixel(&surface, 3, 3).alpha(), 0);
        assert_eq!(pixel(&surface, 5, 5), Color(0xFF11_2233));
    }

    #[test]
    fn content_box_clips_the_background_inside_the_padding_too() {
        // Adwaita:1600 uses `background-clip: content-box`; border 4 plus
        // padding 4 means the content box starts at x=8. Keyword matching is
        // ASCII case-insensitive.
        let surface = painted(&format!(
            "{SQUARE}\nbutton {{ background-clip: CONTENT-BOX }}"
        ));
        assert_eq!(pixel(&surface, 5, 5).alpha(), 0);
        assert_eq!(pixel(&surface, 9, 9), Color(0xFF11_2233));
    }

    #[test]
    fn an_image_color_layer_paints_over_the_background_colour() {
        // Adwaita's `:active` uses `image(<color>)`. Mutation check: painting
        // the layers before the colour hides the layer entirely.
        let surface = painted(
            "button { background-color: #112233; background-image: image(#dad6d2); \
             border-radius: 0; border: 0 solid transparent; padding: 0 }",
        );
        assert_eq!(pixel(&surface, 20, 10), Color(0xFFDA_D6D2));
    }

    #[test]
    fn radii_for_box_shrinks_by_border_then_padding() {
        // Mutation check: using the same radii for every box makes a rounded
        // background bleed past the inner clip's corners.
        let border_box = Rect::new(0.0, 0.0, 40.0, 20.0);
        let alloc = Allocation {
            border_box,
            content_box: border_box.inset([8.0, 8.0, 8.0, 8.0]),
            border: [4.0; 4],
            padding: [4.0; 4],
        };
        let radii = [[10.0, 10.0]; 4];
        assert_eq!(
            radii_for_box(&radii, &alloc, Keyword::BorderBox)[0],
            [10.0, 10.0]
        );
        assert_eq!(
            radii_for_box(&radii, &alloc, Keyword::PaddingBox)[0],
            [6.0, 6.0]
        );
        assert_eq!(
            radii_for_box(&radii, &alloc, Keyword::ContentBox)[0],
            [2.0, 2.0]
        );
    }

    #[test]
    fn a_vertical_gradient_reproduces_adwaitas_flat_pre_stop_band() {
        // The M1 gate's exact number: `linear-gradient(to top, #f6f5f4 2px,
        // #fbfafa)` over a 20px-tall origin box is flat #f6f5f4 below the
        // 2px first stop. Mutation check: sampling at `y` instead of
        // `y + 0.5`, or against the border box instead of the origin box,
        // moves this off the stop colour.
        let surface = painted_flat(
            "button { background-image: linear-gradient(to top, #f6f5f4 2px, #fbfafa); \
             border-radius: 0; border: 0 solid transparent; padding: 0 }",
        );
        assert_eq!(pixel(&surface, 20, 19), Color(0xFFF6_F5F4));
        assert_eq!(pixel(&surface, 20, 0), Color(0xFFFB_FAFA));
        let middle = pixel(&surface, 20, 10);
        assert!(middle != Color(0xFFF6_F5F4) && middle != Color(0xFFFB_FAFA));
        assert!((0xF6..=0xFB).contains(&middle.red()));
    }

    #[test]
    fn a_horizontal_gradient_varies_along_x_not_y() {
        // Mutation check: falling through to the vertical fast path paints
        // uniform columns and the first two assertions become equal.
        let surface = painted_flat(
            "button { background-image: linear-gradient(to right, #000000, #ffffff); \
             border-radius: 0; border: 0 solid transparent; padding: 0 }",
        );
        assert!(pixel(&surface, 2, 10).red() < pixel(&surface, 37, 10).red());
        assert_eq!(pixel(&surface, 20, 2), pixel(&surface, 20, 17));
    }

    #[test]
    fn a_radial_gradient_is_darkest_at_its_centre() {
        // Mutation check: normalising the radius against the box width only
        // makes the corner sample equal the centre on a 40x20 box.
        let surface = painted_flat(
            "button { background-image: radial-gradient(circle closest-side, #000000, #ffffff); \
             border-radius: 0; border: 0 solid transparent; padding: 0 }",
        );
        assert!(pixel(&surface, 20, 10).red() < 0x20);
        assert!(pixel(&surface, 20, 0).red() > 0xE0);
    }

    #[test]
    fn a_conic_gradient_varies_with_angle_around_the_centre() {
        // Mutation check: measuring the angle from the +x axis rather than
        // straight up (CSS's 0deg) rotates the whole wheel by 90 degrees and
        // swaps these two samples.
        let surface = painted_flat(
            "button { background-image: conic-gradient(#000000, #ffffff 50%, #000000); \
             border-radius: 0; border: 0 solid transparent; padding: 0 }",
        );
        assert!(pixel(&surface, 20, 2).red() < 0x40, "straight up is 0deg");
        assert!(
            pixel(&surface, 20, 17).red() > 0xC0,
            "straight down is 180deg"
        );
    }

    #[test]
    fn gradient_sampling_never_panics_on_a_degenerate_box() {
        // A zero-area origin box, a NaN position and a single-stop gradient
        // all reach `gradient_t`; none may panic or hang.
        for css in [
            "button { background-image: linear-gradient(#f00, #f00); \
             border: 0 solid transparent; padding: 0; min-width: 0; min-height: 0 }",
            "button { background-image: radial-gradient(closest-corner at 0 0, #f00, #00f); \
             border: 0 solid transparent; padding: 0 }",
            "button { background-image: repeating-linear-gradient(45deg, #f00 0, #00f 1px); \
             border: 0 solid transparent; padding: 0 }",
        ] {
            let _ = painted(css);
        }
    }

    /// The hostile floats every geometry-consuming function in this part is
    /// fuzzed over (Global Constraints, "Never-panic discipline").
    const HOSTILE: [f32; 10] = [
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
        -1.0e30,
        -1.0,
        -0.0,
        0.0,
        0.5,
        1.0,
        1.0e30,
    ];

    /// The `Gradient` a one-image `background-image` declaration computes to.
    fn gradient_of(image: &str) -> Rc<Gradient> {
        let sheet = CompiledSheet::compile(&format!("button {{ background-image: {image} }}"));
        let window = Node::new("window");
        let button = Node::new("button");
        window.append_child(&button);
        let env = ResolveEnv::default();
        let mut cx = MatchCx::new();
        let style = ComputedStyle::resolve_chain(&sheet, &button, &env, &mut cx);
        match style.background_layers().pop().expect("one layer").image {
            Image::Gradient(gradient) => gradient,
            other => panic!("{image} did not compute to a gradient: {other:?}"),
        }
    }

    #[test]
    fn gradient_t_and_color_at_survive_every_hostile_float() {
        // The battery the degenerate-box test's comment always claimed:
        // NaN, +/-inf, negatives, signed zero and 1e30 are fed *directly*
        // into `gradient_t` as the box size and the sample point, and the
        // `t` that comes back is fed straight into `color_at`. Mutation
        // check: dropping either `is_finite` guard in `gradient_t` (the
        // linear `len2` one or the radial `rx`/`ry` one) makes `color_at`
        // receive a NaN `t` and the stop search index out of bounds.
        let env = ResolveEnv::default();
        let sheet = CompiledSheet::compile("button { color: #000 }");
        let mut fonts = FontDatabase::probe_only();
        let mut images = ImageCache::new();
        let cx = PaintCx {
            env: &env,
            colors: &sheet.colors,
            fonts: &mut fonts,
            images: &mut images,
            text: None,
        };
        let len_ctx = cx.base_length_ctx();
        let color_ctx = cx.color_ctx(Rgba::TRANSPARENT);

        for image in [
            "linear-gradient(to top, #f6f5f4 2px, #fbfafa)",
            "linear-gradient(45deg, #f00, #00f)",
            "radial-gradient(circle closest-side at 0 0, #f00, #00f)",
            "radial-gradient(ellipse farthest-corner, #f00, #0f0 50%, #00f)",
            "conic-gradient(from 30deg, #f00, #fff 50%, #f00)",
            "repeating-linear-gradient(#f00 0, #00f 1px)",
        ] {
            let gradient = gradient_of(image);
            for w in HOSTILE {
                for h in HOSTILE {
                    for p in HOSTILE {
                        let t = gradient_t(&gradient, w, h, p, p, &len_ctx);
                        let _ = gradient.color_at(t, &color_ctx, &len_ctx);
                        let t = gradient_t(&gradient, w, h, p, 0.5, &len_ctx);
                        let _ = gradient.color_at(t, &color_ctx, &len_ctx);
                    }
                }
            }
            // `t` itself, unfiltered.
            for t in HOSTILE {
                let _ = gradient.color_at(t, &color_ctx, &len_ctx);
            }
        }
    }

    #[test]
    fn paint_gradient_clamps_a_hostile_clip_to_the_surface() {
        // Mutation check: removing the `drawable_bounds` intersection makes
        // the `1e9` and `INFINITY` clips below loop up to `i32::MAX` times,
        // one `draw_rect` each -- this test then never returns.
        let env = ResolveEnv::default();
        let sheet = CompiledSheet::compile("button { color: #000 }");
        let mut fonts = FontDatabase::probe_only();
        let mut images = ImageCache::new();
        let mut cx = PaintCx {
            env: &env,
            colors: &sheet.colors,
            fonts: &mut fonts,
            images: &mut images,
            text: None,
        };
        let mut surface = Surface::new_raster_n32_premul(8, 8).expect("raster surface");
        surface.canvas().clear(Color::TRANSPARENT);

        let hostile = [
            Rect::new(0.0, 0.0, 1.0e9, 1.0e9),
            Rect::new(0.0, 0.0, f32::INFINITY, f32::INFINITY),
            Rect::new(-1.0e9, -1.0e9, f32::INFINITY, f32::INFINITY),
            Rect::new(f32::NAN, 0.0, 8.0, 8.0),
            Rect::new(0.0, 0.0, f32::NAN, 8.0),
            Rect::new(0.0, 0.0, -8.0, -8.0),
        ];
        for image in [
            "linear-gradient(to top, #f00, #00f)",
            "linear-gradient(to right, #f00, #00f)",
            "radial-gradient(#f00, #00f)",
            "conic-gradient(#f00, #00f)",
        ] {
            let gradient = gradient_of(image);
            for clip in hostile {
                for origin in hostile {
                    let mut canvas = surface.canvas();
                    paint_gradient(
                        &mut canvas,
                        &gradient,
                        origin,
                        clip,
                        &mut cx,
                        Rgba::TRANSPARENT,
                        BlendMode::SrcOver,
                    );
                }
            }
        }
        // The surface is still a surface: nothing above wrote out of bounds.
        assert!(surface.pixel_buffer().get_pixel(7, 7).is_some());
    }

    #[test]
    fn a_sub_pixel_background_size_cannot_explode_the_tile_list() {
        // `background-size: 0.01px; background-repeat: repeat` over a 100x100
        // origin box asks for 10 000 copies per axis -- 100 million rects.
        // Mutation check: dropping the per-axis cap in `tile_positions`
        // brings this assertion (and ~1.6 GB of allocation) down with it.
        let sheet = CompiledSheet::compile(
            "button { background-image: image(#f00); background-size: 0.01px; \
             background-repeat: repeat }",
        );
        let window = Node::new("window");
        let button = Node::new("button");
        window.append_child(&button);
        let env = ResolveEnv::default();
        let mut cx = MatchCx::new();
        let style = ComputedStyle::resolve_chain(&sheet, &button, &env, &mut cx);
        let layer = style.background_layers().pop().expect("one layer");
        let ctx = style.length_ctx(&env, None);
        let origin = Rect::new(0.0, 0.0, 100.0, 100.0);
        let tile = layer_tile(&layer, origin, None, &ctx);
        assert!(
            (tile.step_x - 0.01).abs() < 1.0e-6,
            "the CSS step survives; only the copy count is capped"
        );

        let rects = tile_positions(&tile, origin, layer.repeat);
        assert!(
            rects.len() <= MAX_TILES_TOTAL,
            "{} tiles emitted for a 0.01px step",
            rects.len()
        );
        // One copy per device pixel of clip, plus an end at each side.
        assert!(rects.len() <= 102 * 102, "{} tiles emitted", rects.len());

        // An infinite clip is bounded too.
        let unbounded = tile_positions(
            &tile,
            Rect::new(0.0, 0.0, f32::INFINITY, f32::INFINITY),
            layer.repeat,
        );
        assert!(unbounded.len() <= MAX_TILES_TOTAL);
    }

    #[test]
    fn a_gradient_layer_honours_background_size_position_and_repeat() {
        // F60. `layer_tile`/`tile_positions` were wired only into
        // `Image::Url`, so a gradient flooded the whole clip box whatever
        // `background-size`/`-position`/`-repeat` said. Adwaita's
        // `stackswitcher > button.needs-attention` is a 6x6 no-repeat
        // radial gradient pinned to the right edge.
        // Mutation check: skipping the tiling paints the gradient across the
        // whole 40x20 box and (2, 10) stops being clear.
        let surface = painted_flat(
            "button { background-image: radial-gradient(circle closest-side, #ff0000, #ff0000);              background-size: 6px 6px; background-position: right 0;              background-repeat: no-repeat;              border-radius: 0; border: 0 solid transparent; padding: 0 }",
        );
        assert_eq!(
            pixel(&surface, 36, 2),
            Color(0xFFFF_0000),
            "the 6x6 tile sits at the right edge"
        );
        assert_eq!(
            pixel(&surface, 2, 10).alpha(),
            0,
            "no-repeat leaves the rest of the box clear"
        );
    }

    #[test]
    fn an_image_color_layer_honours_background_size_and_repeat() {
        // F59. `Image::Solid` filled the whole origin box with one
        // `draw_rect`, ignoring size/position/repeat -- which is how
        // Adwaita's `paned > separator.wide` (two 1px `image()` rules with
        // the pane colour between them) rendered as a solid slab.
        // Mutation check: restoring the single `draw_rect` inks (20, 10).
        let surface = painted_flat(
            "button { background-image: image(#00ff00); background-size: 2px 20px;              background-position: left 0; background-repeat: no-repeat;              border-radius: 0; border: 0 solid transparent; padding: 0 }",
        );
        assert_eq!(pixel(&surface, 1, 10), Color(0xFF00_FF00), "the 2px rule");
        assert_eq!(
            pixel(&surface, 20, 10).alpha(),
            0,
            "no-repeat does not flood the origin box"
        );
    }

    #[test]
    fn one_auto_axis_keeps_the_images_aspect_ratio() {
        // F61. `background-size: 100% auto` on a 200px-wide box with a
        // 10x40 image is (200, 800): the `auto` axis scales with the given
        // one. Falling back to the raw intrinsic size gave (200, 40).
        let sheet = CompiledSheet::compile(
            "button { background-image: image(#f00); background-size: 100% auto }",
        );
        let window = Node::new("window");
        let button = Node::new("button");
        window.append_child(&button);
        let env = ResolveEnv::default();
        let mut cx = MatchCx::new();
        let style = ComputedStyle::resolve_chain(&sheet, &button, &env, &mut cx);
        let layer = style.background_layers().pop().expect("one layer");
        let ctx = style.length_ctx(&env, None);
        let tile = layer_tile(
            &layer,
            Rect::new(0.0, 0.0, 200.0, 100.0),
            Some((10.0, 40.0)),
            &ctx,
        );
        assert_eq!(tile.rect.width, 200.0);
        assert_eq!(tile.rect.height, 800.0);
    }

    #[test]
    fn a_sub_pixel_repeat_step_still_covers_the_whole_box() {
        // F62. The per-axis cap truncated the copy *count*, so
        // `background-size: 2%; repeat` on a 40px box stepped 0.8px and its
        // 42 copies covered 33.6px, leaving the right 6.4px blank. The step
        // is widened to one device pixel instead.
        // Mutation check: dropping `widen` leaves (39, 10) clear.
        let surface = painted_flat(
            "button { background-image: image(#0000ff); background-size: 0.02px 0.02px;              background-repeat: repeat;              border-radius: 0; border: 0 solid transparent; padding: 0 }",
        );
        assert_eq!(
            pixel(&surface, 39, 19),
            Color(0xFF00_00FF),
            "the far corner is painted, not left blank"
        );
    }

    #[test]
    fn an_ellipse_sized_to_a_corner_is_not_a_circle() {
        // CSS Images L3 §3.2: a corner-sized *ellipse* keeps the matching
        // `-side` aspect ratio and passes through the corner, i.e.
        // `side * sqrt(2)` per axis. Using one `hypot(sx, sy)` on both axes
        // made every corner-sized ellipse a circle.
        // Mutation check: restoring the circular radii makes these equal.
        let gradient = gradient_of("radial-gradient(ellipse farthest-corner, #000, #fff)");
        let ctx = crate::css::value::LengthCtx {
            font_size_px: 14.0,
            root_font_size_px: 14.0,
            ex_ratio: 0.5,
            dpi: 96.0,
            percent_basis: None,
        };
        // 40x20 box, centred: fx = 20, fy = 10 -> rx = 28.28, ry = 14.14.
        let along_x = gradient_t(&gradient, 40.0, 20.0, 20.0 + 28.284_27, 10.0, &ctx);
        let along_y = gradient_t(&gradient, 40.0, 20.0, 20.0, 10.0 + 14.142_136, &ctx);
        assert!((along_x - 1.0).abs() < 1.0e-3, "{along_x}");
        assert!((along_y - 1.0).abs() < 1.0e-3, "{along_y}");
    }

    #[test]
    fn a_sized_and_positioned_tile_lands_where_the_css_says() {
        // `background-size: 10px 5px; background-position: right bottom` in a
        // 40x20 origin box. Mutation check: resolving `right` as 100% of the
        // box rather than `box - tile` puts the tile off the right edge.
        let sheet = CompiledSheet::compile(
            "button { background-image: image(#f00); background-size: 10px 5px; \
             background-position: right bottom }",
        );
        let window = Node::new("window");
        let button = Node::new("button");
        window.append_child(&button);
        let env = ResolveEnv::default();
        let mut cx = MatchCx::new();
        let style = ComputedStyle::resolve_chain(&sheet, &button, &env, &mut cx);
        let layer = style.background_layers().pop().expect("one layer");
        let ctx = style.length_ctx(&env, None);
        let tile = layer_tile(&layer, Rect::new(0.0, 0.0, 40.0, 20.0), None, &ctx);
        assert_eq!(tile.rect, Rect::new(30.0, 15.0, 10.0, 5.0));
    }

    #[test]
    fn repeat_tiles_across_the_clip_and_no_repeat_paints_once() {
        // Mutation check: stepping from the clip's origin instead of the
        // tile's makes the first repeated rect land at x == 0 instead of -10.
        let tile = Tile {
            rect: Rect::new(5.0, 5.0, 10.0, 5.0),
            step_x: 10.0,
            step_y: 5.0,
        };
        let clip = Rect::new(0.0, 0.0, 40.0, 20.0);
        let once = tile_positions(
            &tile,
            clip,
            RepeatStyle {
                x: Keyword::NoRepeat,
                y: Keyword::NoRepeat,
            },
        );
        assert_eq!(once, vec![tile.rect]);

        let both = tile_positions(
            &tile,
            clip,
            RepeatStyle {
                x: Keyword::Repeat,
                y: Keyword::Repeat,
            },
        );
        assert!(both.contains(&Rect::new(-5.0, 0.0, 10.0, 5.0)));
        assert!(both.contains(&Rect::new(35.0, 15.0, 10.0, 5.0)));
        assert_eq!(both.len(), 5 * 5);

        let x_only = tile_positions(
            &tile,
            clip,
            RepeatStyle {
                x: Keyword::Repeat,
                y: Keyword::NoRepeat,
            },
        );
        assert!(x_only.iter().all(|r| (r.y - 5.0).abs() < f32::EPSILON));
    }

    #[test]
    fn every_css_background_blend_mode_maps_to_a_skia_blend_mode() {
        // Mutation check: mapping Keyword::ColorBlend to BlendMode::Color is
        // the whole point of the ColorBlend rename; sending it to SrcOver
        // silently disables the mode.
        assert_eq!(blend_mode_for(Keyword::Normal), BlendMode::SrcOver);
        assert_eq!(blend_mode_for(Keyword::Multiply), BlendMode::Multiply);
        assert_eq!(blend_mode_for(Keyword::ColorDodge), BlendMode::ColorDodge);
        assert_eq!(blend_mode_for(Keyword::ColorBlend), BlendMode::Color);
        assert_eq!(blend_mode_for(Keyword::Luminosity), BlendMode::Luminosity);
        assert_eq!(
            blend_mode_for(Keyword::Auto),
            BlendMode::SrcOver,
            "unknown falls back"
        );
    }

    #[test]
    fn an_undecodable_url_paints_nothing_and_is_not_an_error() {
        // Contract §2.5: `url()` that fails to decode is recorded-unresolved.
        // Mutation check: treating it as an error would abort the whole
        // background stack and lose the colour underneath.
        let surface = painted(
            "button { background-color: #112233; \
             background-image: url(\"/nonexistent/icedtea-test.png\"); \
             border-radius: 0; border: 0 solid transparent; padding: 0 }",
        );
        assert_eq!(pixel(&surface, 20, 10), Color(0xFF11_2233));
    }
}
