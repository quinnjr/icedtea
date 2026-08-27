//! `background-color` plus the `background-*` layer stack.
//!
//! Layers arrive top-first (CSS: the first listed image is topmost), so they
//! are painted in reverse; the colour goes underneath them all, clipped by
//! the *last* layer's `background-clip` -- CSS Backgrounds L3 §3.11.

use skia_rs_safe::canvas::{Canvas, ClipOp};

use crate::css::computed::BackgroundLayer;
use crate::css::value::{Image, Keyword, Rgba};
use crate::layout::Allocation;
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

    // Gradients, `url()`, cross-fades and icon references arrive in Tasks 9
    // and 10; `-gtk-icon-*` never paints in M2.
    if let Image::Solid(color) = &layer.image
        && let Some(rgba) = color.resolve(&cx.color_ctx(current))
    {
        let origin = alloc.box_for(layer.origin);
        let mut paint = fill_paint(rgba);
        paint.set_anti_alias(false);
        canvas.draw_rect(&origin.to_skia(), &paint);
    }

    canvas.restore_to_count(save);
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
}
