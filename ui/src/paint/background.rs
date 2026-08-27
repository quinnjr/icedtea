//! `background-color` plus the `background-*` layer stack.
//!
//! Layers arrive top-first (CSS: the first listed image is topmost), so they
//! are painted in reverse; the colour goes underneath them all, clipped by
//! the *last* layer's `background-clip` -- CSS Backgrounds L3 §3.11.

use skia_rs_safe::canvas::{Canvas, ClipOp};

use crate::css::computed::BackgroundLayer;
use crate::css::value::image::{GradientKind, RadialExtent, RadialShape};
use crate::css::value::{Gradient, Image, Keyword, LengthCtx, Position, Rgba};
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
                canvas.draw_rect(&origin.to_skia(), &paint);
            }
        }
        Image::Gradient(gradient) => {
            let origin = alloc.box_for(layer.origin);
            let len_ctx = LengthCtx {
                percent_basis: Some(origin.width),
                ..cx.base_length_ctx()
            };
            paint_gradient(canvas, gradient, origin, clip_rect, cx, current, &len_ctx);
        }
        // `url()`, `cross-fade()` and icon references land in Task 10;
        // `-gtk-icon-*` never paints in M2.
        Image::None | Image::Url(_) | Image::CrossFade(_) | Image::Icon(_) => {}
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
    len_ctx: &LengthCtx,
) {
    if origin.is_empty() || clip.is_empty() {
        return;
    }
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

#[cfg(test)]
mod tests {
    use crate::css::cascade::CompiledSheet;
    use crate::css::computed::{ComputedStyle, ResolveEnv};
    use crate::css::node::Node;
    use crate::css::registry::Prop;
    use crate::css::select::MatchCx;
    use crate::css::value::{Keyword, Rgba};
    use crate::layout::{Allocation, Rect};
    use crate::paint::{ImageCache, PaintCx, paint_backgrounds, radii_for_box};
    use crate::text::FontDatabase;
    use skia_rs_safe::canvas::Surface;
    use skia_rs_safe::core::Color;

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
}
