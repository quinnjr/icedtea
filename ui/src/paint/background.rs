//! `background-color` plus the `background-*` layer stack.
//!
//! Layers arrive top-first (CSS: the first listed image is topmost), so they
//! are painted in reverse; the colour goes underneath them all, clipped by
//! the *last* layer's `background-clip` -- CSS Backgrounds L3 §3.11.

use skia_rs_safe::canvas::{Canvas, ClipOp};
use skia_rs_safe::paint::{BlendMode, Paint};

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

    match &layer.image {
        Image::Solid(color) => {
            if let Some(rgba) = color.resolve(&cx.color_ctx(current)) {
                let origin = alloc.box_for(layer.origin);
                let mut paint = fill_paint(rgba);
                paint.set_anti_alias(false);
                paint.set_blend_mode(blend_mode_for(layer.blend));
                canvas.draw_rect(&origin.to_skia(), &paint);
            }
        }
        Image::Gradient(gradient) => {
            let origin = alloc.box_for(layer.origin);
            paint_gradient(
                canvas,
                gradient,
                origin,
                clip_rect,
                cx,
                current,
                blend_mode_for(layer.blend),
            );
        }
        Image::Url(url) => {
            let origin = alloc.box_for(layer.origin);
            let len_ctx = cx.base_length_ctx();
            let Some((iw, ih)) = cx
                .images
                .get(url)
                .map(|img| (img.width() as f32, img.height() as f32))
            else {
                // Recorded unresolved: paints nothing, already logged once.
                canvas.restore_to_count(save);
                return;
            };
            let tile = layer_tile(layer, origin, Some((iw, ih)), &len_ctx);
            let rects = tile_positions(&tile, clip_rect, layer.repeat);
            let mut paint = Paint::new();
            paint.set_blend_mode(blend_mode_for(layer.blend));
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
        RadialExtent::ClosestCorner => {
            let d = sx.hypot(sy);
            (d, d)
        }
        RadialExtent::FarthestCorner => {
            let d = fx.hypot(fy);
            (d, d)
        }
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
                _ => rx,
            };
            (r, r)
        }
        RadialShape::Ellipse => (rx, ry),
    }
}

/// Fill `clip` with `gradient`, sized against `origin`.
///
/// Axis-aligned linear gradients are filled band-by-band -- one `draw_rect`
/// per constant row or column -- which is exactly M1's model and keeps the
/// gate's band assertions exact. Everything else samples per pixel.
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

    for row in y0..y1 {
        let py = row as f32 + 0.5 - origin.y;
        for col in x0..x1 {
            let px = col as f32 + 0.5 - origin.x;
            let t = gradient_t(gradient, origin.width, origin.height, px, py, len_ctx);
            let color = gradient.color_at(t, &color_ctx, len_ctx);
            band(canvas, Rect::new(col as f32, row as f32, 1.0, 1.0), color);
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
        BgSize::Explicit(x, y) => (
            x.resolve(&w_ctx).unwrap_or(iw),
            y.resolve(&h_ctx).unwrap_or(ih),
        ),
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

/// Every copy of `tile` that intersects `clip`, per `repeat`.
#[must_use]
pub fn tile_positions(tile: &Tile, clip: Rect, repeat: RepeatStyle) -> Vec<Rect> {
    let repeats = |axis: Keyword| matches!(axis, Keyword::Repeat | Keyword::Round | Keyword::Space);
    let count = |start: f32, step: f32, lo: f32, hi: f32| -> (i32, i32) {
        if !(step.is_finite() && step > 0.0) {
            return (0, 0);
        }
        let first = ((lo - start) / step).floor() as i32;
        // Not `.ceil()`: for a non-exact `(hi - start) / step` (e.g. a 40px
        // clip tiled from x=5 in 10px steps -> 3.5), rounding up lands a
        // whole extra step past `hi` with zero overlap. `.floor()` keeps the
        // last index whose start is at-or-before `hi` (still touching-
        // inclusive when the division is exact, e.g. the y axis below).
        let last = ((hi - start) / step).floor() as i32;
        (first, last.max(first))
    };

    let (ix0, ix1) = if repeats(repeat.x) {
        count(tile.rect.x, tile.step_x, clip.x, clip.right())
    } else {
        (0, 0)
    };
    let (iy0, iy1) = if repeats(repeat.y) {
        count(tile.rect.y, tile.step_y, clip.y, clip.bottom())
    } else {
        (0, 0)
    };

    let mut out = Vec::new();
    for iy in iy0..=iy1 {
        for ix in ix0..=ix1 {
            out.push(Rect::new(
                tile.step_x.mul_add(ix as f32, tile.rect.x),
                tile.step_y.mul_add(iy as f32, tile.rect.y),
                tile.rect.width,
                tile.rect.height,
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
    use super::{Tile, blend_mode_for, layer_tile, tile_positions};
    use crate::css::cascade::CompiledSheet;
    use crate::css::computed::{ComputedStyle, ResolveEnv};
    use crate::css::node::Node;
    use crate::css::registry::Prop;
    use crate::css::select::MatchCx;
    use crate::css::value::RepeatStyle;
    use crate::css::value::{Keyword, Rgba};
    use crate::layout::{Allocation, Rect};
    use crate::paint::{ImageCache, PaintCx, paint_backgrounds, radii_for_box};
    use crate::text::FontDatabase;
    use skia_rs_safe::canvas::Surface;
    use skia_rs_safe::core::Color;
    use skia_rs_safe::paint::BlendMode;

    /// A 40x20 node with a 4px border and 4px padding, painted from `css`.
    fn painted(css: &str) -> Surface {
        let sheet = CompiledSheet::compile(css);
        let window = Node::new("window");
        let button = Node::new("button");
        window.append_child(&button);
        let env = ResolveEnv::default();
        let mut cx = MatchCx::new();
        let style = ComputedStyle::resolve_chain(&sheet, &button, &env, &mut cx);

        let border_box = Rect::new(0.0, 0.0, 40.0, 20.0);
        let alloc = Allocation {
            border_box,
            content_box: border_box.inset([8.0, 8.0, 8.0, 8.0]),
            border: [4.0; 4],
            padding: [4.0; 4],
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
        let surface = painted(
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
        let surface = painted(
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
        let surface = painted(
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
        let surface = painted(
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
