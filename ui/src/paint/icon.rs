//! The single entry point every icon-shaped paint goes through.
//!
//! CSS `-gtk-icon-source`, `background-image: -gtk-icontheme(…)`, and the
//! `Image` and `Picture` widgets all arrive here, so the `-gtk-icon-*`
//! properties are read in exactly one place and a widget that draws an icon
//! cannot accidentally read a different set of them.
//!
//! The icon is drawn into the largest square that fits the allocation,
//! centred, at `-gtk-icon-size` when the style declares one — GTK's own
//! rule, and the reason a 16 px icon in a 24 px button does not stretch.

use std::sync::Arc;

use skia_rs_safe::canvas::{Canvas, SaveLayerFlags, SaveLayerRec};
use skia_rs_safe::core::{Matrix, Rect as SkRect};
use skia_rs_safe::paint::{ColorFilterRef, ColorMatrixFilter, Paint};

use crate::css::computed::ComputedStyle;
use crate::css::registry::Prop;
use crate::css::value::transform::transform_list_matrix;
use crate::css::value::{
    ColorCtx, ColorValue, FilterFn, IconRef, Image as CssImage, Keyword, Rgba, Shadow, TransformFn,
    Value,
};
use crate::icons::{Builtin, Handle, Palette};
use crate::layout::Rect;

use super::PaintCx;
use super::effects::color_matrix_for;
use super::shadow::blit_blurred;

/// How deep `-gtk-scaled()`/`-gtk-recolor()` nesting may go.
///
/// A stylesheet is untrusted, and `Image` nests: `-gtk-scaled()` takes two
/// `<image>`s, either of which may be another `-gtk-scaled()`.
pub(crate) const MAX_ICON_NESTING: u8 = 8;

/// The icon's box size in the allocation's units.
///
/// `-gtk-icon-size` when the style declares one, else the largest square
/// that fits the allocation. Always at least 1.
#[must_use]
pub fn icon_size_px(style: &ComputedStyle, rect: Rect, cx: &PaintCx<'_>) -> u32 {
    let fallback = rect.width.min(rect.height);
    let declared = if style.is_specified(Prop::GtkIconSize) {
        match style.raw(Prop::GtkIconSize) {
            Value::Length(length) => length
                .resolve(&cx.base_length_ctx())
                .filter(|px| px.is_finite() && *px > 0.0),
            _ => None,
        }
    } else {
        None
    };
    let px = declared.unwrap_or(fallback);
    if px.is_finite() && px >= 1.0 {
        px.min(crate::icons::MAX_ICON_PX as f32) as u32
    } else {
        1
    }
}

/// Whether this paint wants the symbolic variant.
///
/// `-gtk-icon-style: symbolic|regular` forces the answer; `requested` (the
/// initial) leaves it to the name, which is how a theme asking for
/// `foo-symbolic` gets a symbolic icon without every caller opting in.
fn wants_symbolic(style: &ComputedStyle, name: &str) -> bool {
    match style.get::<Keyword>(Prop::GtkIconStyle) {
        Keyword::Symbolic => true,
        Keyword::Regular => false,
        _ => name.ends_with("-symbolic"),
    }
}

/// The palette a `-gtk-recolor()` declaration's own slot list produces,
/// layered over the node's.
fn palette_with_overrides(
    base: Palette,
    overrides: Option<&crate::css::value::image::IconPalette>,
    cx: &PaintCx<'_>,
) -> Palette {
    let mut palette = base;
    let Some(entries) = overrides else {
        return palette;
    };
    let ctx = ColorCtx {
        table: cx.colors,
        current: base.foreground,
        depth: 0,
    };
    for (name, value) in entries.iter() {
        let Some(resolved) = ColorValue::resolve(value, &ctx) else {
            continue;
        };
        if name.eq_ignore_ascii_case("success") {
            palette.success = resolved;
        } else if name.eq_ignore_ascii_case("warning") {
            palette.warning = resolved;
        } else if name.eq_ignore_ascii_case("error") {
            palette.error = resolved;
        }
    }
    palette
}

/// Resolve an `IconRef` to a rasterised handle at `size`.
///
/// This is the half a widget controller calls to keep a handle between
/// frames (`ImageC { resolved: Option<icons::Handle>, .. }`); `paint_icon`
/// is resolve-and-draw.
#[must_use]
pub fn resolve_icon(
    icon: &IconRef,
    size: u32,
    style: &ComputedStyle,
    cx: &mut PaintCx<'_>,
) -> Option<Handle> {
    let palette = Palette::from_style(style, cx.colors);
    resolve_inner(icon, size, style, &palette, cx, 0)
}

/// `resolve_icon`, carrying the nesting depth and an explicit palette so
/// `-gtk-recolor()`'s own slots can override the node's.
fn resolve_inner(
    icon: &IconRef,
    size: u32,
    style: &ComputedStyle,
    palette: &Palette,
    cx: &mut PaintCx<'_>,
    depth: u8,
) -> Option<Handle> {
    if depth >= MAX_ICON_NESTING {
        tracing::debug!("icon reference nests deeper than the limit; nothing drawn");
        return None;
    }
    let scale = cx.icons.scale();
    match icon {
        IconRef::Theme { name } => {
            let symbolic = wants_symbolic(style, name);
            cx.icons.render(name, size, scale, symbolic, palette)
        }
        IconRef::Recolor {
            url,
            palette: overrides,
        } => {
            let path = cx.images.resolve_path(url);
            let palette = palette_with_overrides(*palette, overrides.as_ref(), cx);
            cx.icons.render_path(path, size, scale, true, &palette)
        }
        IconRef::Scaled { lo, hi } => {
            let chosen = if scale >= 2 { hi } else { lo };
            resolve_css_image(chosen, size, style, palette, cx, depth + 1)
        }
    }
}

/// The `-gtk-scaled()` arms are whole `<image>`s, not `IconRef`s.
///
/// Only the icon-shaped ones rasterise here; a `url()` arm goes through the
/// same decode cache `background-image` uses, and a gradient arm is not an
/// icon and draws nothing.
fn resolve_css_image(
    image: &CssImage,
    size: u32,
    style: &ComputedStyle,
    palette: &Palette,
    cx: &mut PaintCx<'_>,
    depth: u8,
) -> Option<Handle> {
    match image {
        CssImage::Icon(icon) => resolve_inner(icon, size, style, palette, cx, depth),
        CssImage::Url(url) => {
            let path = cx.images.resolve_path(url);
            cx.icons
                .render_path(path, size, cx.icons.scale(), false, palette)
        }
        _ => None,
    }
}

/// The largest square that fits `rect`, centred in it.
///
/// `None` for a degenerate or non-finite allocation: nothing is drawn rather
/// than a NaN rectangle handed to the canvas.
fn icon_box(rect: Rect, size: u32) -> Option<SkRect> {
    if !(rect.x.is_finite()
        && rect.y.is_finite()
        && rect.width.is_finite()
        && rect.height.is_finite())
    {
        return None;
    }
    if rect.width <= 0.0 || rect.height <= 0.0 {
        return None;
    }
    let side = (size as f32).min(rect.width).min(rect.height);
    if side <= 0.0 {
        return None;
    }
    Some(SkRect::from_xywh(
        rect.x + (rect.width - side) / 2.0,
        rect.y + (rect.height - side) / 2.0,
        side,
        side,
    ))
}

/// The single entry point every icon-shaped paint goes through: CSS
/// `-gtk-icon-source`, `background-image: -gtk-icontheme(…)`, and the `Image`
/// and `Picture` widgets. Reads `-gtk-icon-size`, `-gtk-icon-transform`,
/// `-gtk-icon-shadow`, `-gtk-icon-filter`, `-gtk-icon-style` and
/// `-gtk-icon-palette` off `style`; HiDPI comes from `-gtk-scaled()` and the
/// output scale on `cx.icons`.
pub fn paint_icon(
    canvas: &mut Canvas<'_>,
    icon: &IconRef,
    rect: Rect,
    style: &ComputedStyle,
    cx: &mut PaintCx<'_>,
) {
    let size = icon_size_px(style, rect, cx);
    let Some(dst) = icon_box(rect, size) else {
        return;
    };
    let Some(image) = resolve_icon(icon, size, style, cx) else {
        return;
    };
    with_icon_effects(canvas, style, dst, cx, |canvas| {
        let paint = Paint::new();
        canvas.draw_image_rect(&image, None, &dst, Some(&paint));
    });
}

/// `-gtk-icon-transform`, flattened about the icon box's own centre.
///
/// About the icon's centre, not the allocation's: a `rotate()` on a
/// `-gtk-icon-transform` is how GTK4 turns the expander's arrow, and turning
/// it about a widget-sized box would swing it out of view.
#[must_use]
pub(crate) fn icon_transform(
    style: &ComputedStyle,
    dst: SkRect,
    cx: &PaintCx<'_>,
) -> Option<Matrix> {
    let Value::Transform(list) = style.raw(Prop::GtkIconTransform) else {
        return None;
    };
    if list.is_empty() {
        return None;
    }
    let functions: Vec<TransformFn> = list.to_vec();
    let basis = (dst.width(), dst.height());
    let origin = (dst.left + dst.width() / 2.0, dst.top + dst.height() / 2.0);
    Some(transform_list_matrix(
        &functions,
        &cx.base_length_ctx(),
        basis,
        origin,
    ))
}

/// The colour matrix `-gtk-icon-filter` composes to.
fn icon_color_matrix(style: &ComputedStyle) -> Option<[f32; 20]> {
    let Value::Filter(list) = style.raw(Prop::GtkIconFilter) else {
        return None;
    };
    let filters: Vec<FilterFn> = list.to_vec();
    color_matrix_for(&filters)
}

/// A colour matrix that replaces every pixel's colour with `color`, keeping
/// the source's alpha (scaled by the colour's own).
fn tint_matrix(color: Rgba) -> [f32; 20] {
    [
        0.0,
        0.0,
        0.0,
        0.0,
        color.r, //
        0.0,
        0.0,
        0.0,
        0.0,
        color.g, //
        0.0,
        0.0,
        0.0,
        0.0,
        color.b, //
        0.0,
        0.0,
        0.0,
        color.a.clamp(0.0, 1.0),
        0.0,
    ]
}

/// Run `draw` inside a save-layer carrying `matrix` as its colour filter.
fn with_color_matrix(canvas: &mut Canvas<'_>, matrix: [f32; 20], draw: &dyn Fn(&mut Canvas<'_>)) {
    let mut paint = Paint::new();
    let filter: ColorFilterRef = Arc::new(ColorMatrixFilter::new(matrix));
    paint.set_color_filter(Some(filter));
    let save = canvas.save_layer(&SaveLayerRec {
        bounds: None,
        paint: Some(&paint),
        flags: SaveLayerFlags::NONE,
    });
    draw(canvas);
    canvas.restore_to_count(save);
}

/// Every `-gtk-icon-shadow`, behind the icon, back to front.
fn paint_icon_shadows(
    canvas: &mut Canvas<'_>,
    style: &ComputedStyle,
    dst: SkRect,
    cx: &PaintCx<'_>,
    draw: &dyn Fn(&mut Canvas<'_>),
) {
    let shadows: std::rc::Rc<[Shadow]> = style.get(Prop::GtkIconShadow);
    if shadows.is_empty() {
        return;
    }
    let ctx = cx.base_length_ctx();
    let current = style.color();
    for shadow in shadows.iter().rev() {
        // `computed` has already resolved `@name` and `currentColor` for a
        // shadow colour that survived; `None` means currentColor.
        let color = match shadow.color.as_ref() {
            Some(ColorValue::Absolute(rgba)) => *rgba,
            Some(_) | None => current,
        };
        if color.a <= 0.0 {
            continue;
        }
        let px = |length: &crate::css::value::Length| {
            length
                .resolve(&ctx)
                .filter(|value| value.is_finite())
                .unwrap_or(0.0)
        };
        let (dx, dy) = (px(&shadow.offset_x), px(&shadow.offset_y));
        let blur = px(&shadow.blur).max(0.0);
        let matrix = tint_matrix(color);
        if blur <= 0.0 {
            let save = canvas.save();
            canvas.concat(&Matrix::translate(dx, dy));
            with_color_matrix(canvas, matrix, draw);
            canvas.restore_to_count(save);
            continue;
        }
        let shape = Rect::new(dst.left + dx, dst.top + dy, dst.width(), dst.height());
        // `blit_blurred` owns the offscreen budget -- the padding, the sigma
        // cap and the dimension ceiling that is the only defence against
        // `-gtk-icon-shadow: 0 0 99999px`.
        blit_blurred(canvas, shape, blur, None, |offscreen, offset, _| {
            let save = offscreen.save();
            offscreen.concat(&Matrix::translate(dx - offset.0, dy - offset.1));
            with_color_matrix(offscreen, matrix, draw);
            offscreen.restore_to_count(save);
        });
    }
}

/// Draw `draw` under the icon effect stack: shadows behind, then the
/// transform and the colour filter around the icon itself.
fn with_icon_effects(
    canvas: &mut Canvas<'_>,
    style: &ComputedStyle,
    dst: SkRect,
    cx: &PaintCx<'_>,
    draw: impl Fn(&mut Canvas<'_>),
) {
    let draw: &dyn Fn(&mut Canvas<'_>) = &draw;
    let save = canvas.save();
    if let Some(matrix) = icon_transform(style, dst, cx) {
        canvas.concat(&matrix);
    }
    paint_icon_shadows(canvas, style, dst, cx, draw);
    match icon_color_matrix(style) {
        Some(matrix) => with_color_matrix(canvas, matrix, draw),
        None => draw(canvas),
    }
    canvas.restore_to_count(save);
}

/// `-gtk-icon-source: builtin`'s draw.
///
/// The registry parses the property as a bare keyword, so *which* shape is
/// the CSS node's business: `checkbutton > check` passes `Builtin::Check`,
/// `spinbutton button.up` passes `Builtin::SpinPlus`, and so on. The shape
/// is drawn in the node's own `color`, through the same
/// transform/filter/shadow stack an icon file goes through — which is what
/// makes an expander's `-gtk-icon-transform: rotate(90deg)` work.
pub fn paint_builtin(
    canvas: &mut Canvas<'_>,
    builtin: Builtin,
    rect: Rect,
    style: &ComputedStyle,
    cx: &mut PaintCx<'_>,
) {
    let size = icon_size_px(style, rect, cx);
    let Some(dst) = icon_box(rect, size) else {
        return;
    };
    let color = style.color();
    let box_rect = Rect::new(dst.left, dst.top, dst.width(), dst.height());
    with_icon_effects(canvas, style, dst, &*cx, move |canvas| {
        builtin.draw(canvas, box_rect, color);
    });
}

#[cfg(test)]
mod tests {
    use super::{icon_size_px, paint_icon, resolve_icon};
    use crate::css::cascade::CompiledSheet;
    use crate::css::computed::{ComputedStyle, ResolveEnv};
    use crate::css::node::Node;
    use crate::css::select::MatchCx;
    use crate::css::value::{IconRef, Image as CssImage};
    use crate::icons::IconTheme;
    use crate::icons::test_support::roots;
    use crate::layout::Rect;
    use crate::paint::{ImageCache, PaintCx};
    use crate::text::FontDatabase;
    use skia_rs_safe::canvas::Surface;
    use skia_rs_safe::core::Color;
    use std::rc::Rc;

    /// Paint one `IconRef` under `css` into a `side × side` surface, with the
    /// icon box filling it.
    fn painted(css: &str, icon: &IconRef, side: i32, scale: u32) -> Surface {
        painted_in(
            css,
            icon,
            side,
            scale,
            Rect::new(0.0, 0.0, side as f32, side as f32),
        )
    }

    fn painted_in(css: &str, icon: &IconRef, side: i32, scale: u32, rect: Rect) -> Surface {
        let sheet = CompiledSheet::compile(css);
        let node = Node::new("image");
        let env = ResolveEnv::default();
        let mut cx = MatchCx::new();
        let style = ComputedStyle::resolve_chain(&sheet, &node, &env, &mut cx);

        let mut fonts = FontDatabase::probe_only();
        let mut images = ImageCache::new();
        let mut icons = IconTheme::with_name_and_roots("MiniTheme", roots());
        icons.set_scale(scale);
        let mut paint_cx = PaintCx {
            env: &env,
            colors: &sheet.colors,
            fonts: &mut fonts,
            images: &mut images,
            icons: &mut icons,
            text: None,
        };
        let mut surface = Surface::new_raster_n32_premul(side, side).expect("surface");
        {
            let mut canvas = surface.canvas();
            canvas.clear(Color::TRANSPARENT);
            paint_icon(&mut canvas, icon, rect, &style, &mut paint_cx);
        }
        surface
    }

    fn pixel(surface: &Surface, x: i32, y: i32) -> Color {
        surface
            .pixel_buffer()
            .get_pixel(x, y)
            .expect("pixel is inside the surface")
    }

    fn theme_icon(name: &str) -> IconRef {
        IconRef::Theme {
            name: Rc::from(name),
        }
    }

    // `-gtk-icontheme(document-open)` at 16px is the fixture's red PNG.
    // Mutation check: pass the node's name instead of the IconRef's to
    // `IconTheme::render` and this paints image-missing's magenta.
    #[test]
    fn a_theme_icon_paints_its_file() {
        let surface = painted(
            "image { color: #000000 }",
            &theme_icon("document-open"),
            16,
            1,
        );
        assert_eq!(pixel(&surface, 8, 8), Color(0xFFFF_0000));
    }

    // `-gtk-icon-style: symbolic` forces the symbolic variant, which is then
    // recoloured to the node's `color`.
    // Mutation check: ignore `-gtk-icon-style` and this paints the red PNG.
    #[test]
    fn gtk_icon_style_symbolic_forces_the_symbolic_variant() {
        let surface = painted(
            "image { color: #3584e4; -gtk-icon-style: symbolic }",
            &theme_icon("document-open"),
            16,
            1,
        );
        assert_eq!(pixel(&surface, 4, 8), Color(0xFF35_84E4));
    }

    // ... and `regular` forces the full-colour one even for a name that ends
    // in `-symbolic`.
    // Mutation check: drop the `Regular` arm and this paints the recoloured
    // symbolic SVG.
    #[test]
    fn gtk_icon_style_regular_refuses_the_symbolic_variant() {
        let surface = painted(
            "image { color: #3584e4; -gtk-icon-style: regular }",
            &theme_icon("document-open"),
            16,
            1,
        );
        assert_eq!(pixel(&surface, 8, 8), Color(0xFFFF_0000));
    }

    // `-gtk-icon-size` sets the icon's box; the icon is centred in the
    // allocation rather than stretched to it, as GTK does.
    //
    // At 8px the fixture's `16x16` `Fixed` directory no longer matches (the
    // spec's `DirectoryMatchesSize` requires an exact hit), so the theme
    // falls back to its `scalable` directory (`MinSize=8`) at distance 0 --
    // the fixture's blue `document-open.svg`, not the red PNG. The pixel
    // asserted on is therefore the scalable variant's colour; what this test
    // is actually pinning down is the centred box, not which file wins.
    // Mutation check: fill `rect` instead of the centred box and the corner
    // assertion fails.
    #[test]
    fn gtk_icon_size_sets_a_centred_box_inside_the_allocation() {
        let surface = painted(
            "image { color: #000000; -gtk-icon-size: 8px }",
            &theme_icon("document-open"),
            16,
            1,
        );
        assert_eq!(pixel(&surface, 8, 8), Color(0xFF1C_71D8));
        assert_eq!(pixel(&surface, 1, 1), Color(0x0000_0000));
        assert_eq!(icon_size_px_for("image { -gtk-icon-size: 8px }", 16.0), 8);
        assert_eq!(icon_size_px_for("image { color: #000 }", 24.0), 24);
    }

    fn icon_size_px_for(css: &str, side: f32) -> u32 {
        let sheet = CompiledSheet::compile(css);
        let node = Node::new("image");
        let env = ResolveEnv::default();
        let mut cx = MatchCx::new();
        let style = ComputedStyle::resolve_chain(&sheet, &node, &env, &mut cx);
        let mut fonts = FontDatabase::probe_only();
        let mut images = ImageCache::new();
        let mut icons = IconTheme::with_name_and_roots("MiniTheme", roots());
        let paint_cx = PaintCx {
            env: &env,
            colors: &sheet.colors,
            fonts: &mut fonts,
            images: &mut images,
            icons: &mut icons,
            text: None,
        };
        icon_size_px(&style, Rect::new(0.0, 0.0, side, side), &paint_cx)
    }

    // `-gtk-scaled(lo, hi)` picks by the output scale.
    //
    // At scale 2 no fixture directory has `Scale=2` (the default is 1), so
    // `document-open`'s exact-scale match fails for every directory and the
    // spec's distance fallback runs: the `scalable` directory (`MinSize=8`)
    // is in-range at distance 0 and beats the size-16 `Fixed` directory, so
    // the hi arm resolves to the blue `document-open.svg`, not the red PNG.
    // What matters for this test is that it is neither transparent nor the
    // lo arm's green `dual` -- i.e. that the *hi* arm was chosen at all.
    // Mutation check: always take `lo` and the scale-2 assertion paints the
    // green `dual` icon.
    #[test]
    fn gtk_scaled_picks_the_hi_dpi_source_at_scale_two() {
        let scaled = IconRef::Scaled {
            lo: Rc::new(CssImage::Icon(Rc::new(theme_icon("dual")))),
            hi: Rc::new(CssImage::Icon(Rc::new(theme_icon("document-open")))),
        };
        let at_one = painted("image { color: #000000 }", &scaled, 16, 1);
        assert_eq!(pixel(&at_one, 8, 8), Color(0xFF00_FF00));
        let at_two = painted("image { color: #000000 }", &scaled, 16, 2);
        assert_eq!(pixel(&at_two, 8, 8), Color(0xFF1C_71D8));
    }

    // `-gtk-recolor(url(...))` recolours an SVG named by URL, with the
    // declaration's own palette overriding the node's.
    // Mutation check: ignore the declaration's palette and the right half
    // comes back as the theme default success colour, not #cd9309.
    #[test]
    fn gtk_recolor_recolours_a_url_with_its_own_palette() {
        let url = roots()[0]
            .join("MiniTheme/symbolic/actions/document-open-symbolic.svg")
            .display()
            .to_string();
        let css = format!(
            "image {{ color: #3584e4; \
             -gtk-icon-source: -gtk-recolor(url(\"{url}\"), success #cd9309) }}"
        );
        let sheet = CompiledSheet::compile(&css);
        let node = Node::new("image");
        let env = ResolveEnv::default();
        let mut match_cx = MatchCx::new();
        let style = ComputedStyle::resolve_chain(&sheet, &node, &env, &mut match_cx);
        let crate::css::value::Value::Image(CssImage::Icon(icon)) =
            style.raw(crate::css::registry::Prop::GtkIconSource)
        else {
            panic!("-gtk-icon-source did not parse as an icon image");
        };
        let icon = Rc::clone(icon);

        let mut fonts = FontDatabase::probe_only();
        let mut images = ImageCache::new();
        let mut icons = IconTheme::with_name_and_roots("MiniTheme", roots());
        let mut paint_cx = PaintCx {
            env: &env,
            colors: &sheet.colors,
            fonts: &mut fonts,
            images: &mut images,
            icons: &mut icons,
            text: None,
        };
        let mut surface = Surface::new_raster_n32_premul(16, 16).expect("surface");
        {
            let mut canvas = surface.canvas();
            canvas.clear(Color::TRANSPARENT);
            paint_icon(
                &mut canvas,
                &icon,
                Rect::new(0.0, 0.0, 16.0, 16.0),
                &style,
                &mut paint_cx,
            );
        }
        assert_eq!(pixel(&surface, 4, 8), Color(0xFF35_84E4));
        assert_eq!(pixel(&surface, 12, 8), Color(0xFFCD_9309));
    }

    // A name nothing has paints image-missing, not nothing.
    // Mutation check: return early on a lookup miss and this paints
    // transparent.
    #[test]
    fn an_unknown_icon_name_paints_image_missing() {
        let surface = painted(
            "image { color: #000000 }",
            &theme_icon("no-such-icon"),
            48,
            1,
        );
        assert_eq!(pixel(&surface, 24, 24), Color(0xFFFF_00FF));
    }

    // `-gtk-scaled()` nests, and a stylesheet is untrusted.
    // Mutation check: remove the nesting guard and this overflows the stack.
    #[test]
    fn deeply_nested_icon_references_are_refused_rather_than_recursed() {
        let mut icon = theme_icon("document-open");
        for _ in 0..512 {
            icon = IconRef::Scaled {
                lo: Rc::new(CssImage::Icon(Rc::new(icon.clone()))),
                hi: Rc::new(CssImage::Icon(Rc::new(icon))),
            };
        }
        let surface = painted("image { color: #000000 }", &icon, 16, 1);
        // Nothing is asserted about the pixels: the assertion is that this
        // returns at all.
        let _ = pixel(&surface, 8, 8);
    }

    // Degenerate allocations paint nothing rather than allocating a
    // zero-pixel or gigapixel surface.
    // Mutation check: drop the guard and the NaN case panics inside skia's
    // surface allocation.
    #[test]
    fn a_degenerate_allocation_paints_nothing() {
        for rect in [
            Rect::new(0.0, 0.0, 0.0, 0.0),
            Rect::new(0.0, 0.0, -8.0, 16.0),
            Rect::new(f32::NAN, 0.0, 16.0, 16.0),
            Rect::new(0.0, 0.0, f32::INFINITY, 16.0),
        ] {
            let surface = painted_in(
                "image { color: #000000 }",
                &theme_icon("document-open"),
                16,
                1,
                rect,
            );
            let _ = pixel(&surface, 0, 0);
        }
    }

    // `resolve_icon` is the half P5's `ImageC` calls to cache a `Handle`
    // between frames.
    // Mutation check: return `None` for a `Theme` ref and the assertion
    // fails, taking the Image widget's measure() with it.
    #[test]
    fn resolve_icon_hands_back_a_shareable_handle() {
        let sheet = CompiledSheet::compile("image { color: #000000 }");
        let node = Node::new("image");
        let env = ResolveEnv::default();
        let mut match_cx = MatchCx::new();
        let style = ComputedStyle::resolve_chain(&sheet, &node, &env, &mut match_cx);
        let mut fonts = FontDatabase::probe_only();
        let mut images = ImageCache::new();
        let mut icons = IconTheme::with_name_and_roots("MiniTheme", roots());
        let mut cx = PaintCx {
            env: &env,
            colors: &sheet.colors,
            fonts: &mut fonts,
            images: &mut images,
            icons: &mut icons,
            text: None,
        };
        let first =
            resolve_icon(&theme_icon("document-open"), 16, &style, &mut cx).expect("handle");
        let second =
            resolve_icon(&theme_icon("document-open"), 16, &style, &mut cx).expect("handle");
        assert!(Rc::ptr_eq(&first, &second));
        assert_eq!(first.width(), 16);
    }

    use super::paint_builtin;
    use crate::icons::Builtin;

    // `-gtk-icon-transform` moves the icon inside its allocation, about the
    // icon box's own centre.
    // Mutation check: apply the transform about the canvas origin and the
    // `(1, 8)` assertion still holds but `(14, 8)` moves -- so assert both.
    #[test]
    fn gtk_icon_transform_moves_the_icon() {
        let plain = painted_in(
            "image { color: #000000; -gtk-icon-size: 8px }",
            &theme_icon("document-open"),
            16,
            1,
            Rect::new(0.0, 0.0, 16.0, 16.0),
        );
        assert_eq!(pixel(&plain, 8, 8), Color(0xFF1C_71D8));
        assert_eq!(pixel(&plain, 14, 8), Color(0x0000_0000));

        let moved = painted_in(
            "image { color: #000000; -gtk-icon-size: 8px; \
             -gtk-icon-transform: translate(4px, 0) }",
            &theme_icon("document-open"),
            16,
            1,
            Rect::new(0.0, 0.0, 16.0, 16.0),
        );
        assert_eq!(pixel(&moved, 14, 8), Color(0xFF1C_71D8));
        assert_eq!(pixel(&moved, 2, 8), Color(0x0000_0000));
    }

    // `-gtk-icon-filter` is the same colour-matrix stack `filter` uses.
    // Mutation check: skip the save-layer and the red channel stays 0xFF.
    #[test]
    fn gtk_icon_filter_applies_a_colour_matrix() {
        let surface = painted(
            "image { color: #000000; -gtk-icon-filter: grayscale(1) }",
            &theme_icon("document-open"),
            16,
            1,
        );
        let color = pixel(&surface, 8, 8).0;
        let (r, g, b) = ((color >> 16) & 0xFF, (color >> 8) & 0xFF, color & 0xFF);
        assert_eq!(r, g, "grayscale left r != g");
        assert_eq!(g, b, "grayscale left g != b");
        assert!(r > 0 && r < 0xFF, "grayscale produced {r:#x}");
    }

    // `-gtk-icon-shadow` draws the icon's own silhouette, offset and tinted.
    // Mutation check: draw the shadow *after* the icon and the `(8, 8)`
    // assertion sees black instead of red.
    #[test]
    fn gtk_icon_shadow_draws_a_tinted_offset_silhouette() {
        let surface = painted_in(
            "image { color: #000000; -gtk-icon-size: 8px; \
             -gtk-icon-shadow: 6px 0 #000000 }",
            &theme_icon("document-open"),
            24,
            1,
            Rect::new(0.0, 0.0, 24.0, 24.0),
        );
        assert_eq!(pixel(&surface, 12, 12), Color(0xFF1C_71D8));
        assert_eq!(pixel(&surface, 18, 12), Color(0xFF00_0000));
    }

    // An omitted shadow colour is `currentColor`, as everywhere else in the
    // engine.
    // Mutation check: default it to black and this fails.
    #[test]
    fn an_icon_shadow_without_a_colour_is_current_color() {
        let surface = painted_in(
            "image { color: #3584e4; -gtk-icon-size: 8px; -gtk-icon-shadow: 6px 0 }",
            &theme_icon("document-open"),
            24,
            1,
            Rect::new(0.0, 0.0, 24.0, 24.0),
        );
        assert_eq!(pixel(&surface, 18, 12), Color(0xFF35_84E4));
    }

    // A shadow's blur radius comes from CSS and CSS can ask for a gigapixel
    // buffer; `blit_blurred` owns that budget and refuses.
    // Mutation check: allocate the offscreen here instead of through
    // blit_blurred and this hangs or aborts.
    #[test]
    fn an_absurd_icon_shadow_blur_is_refused_not_allocated() {
        let surface = painted(
            "image { color: #000000; -gtk-icon-shadow: 0 0 99999px #000000 }",
            &theme_icon("document-open"),
            16,
            1,
        );
        assert_eq!(pixel(&surface, 8, 8), Color(0xFFFF_0000));
    }

    // `paint_builtin` is `-gtk-icon-source: builtin`'s draw, and honours the
    // same effect stack.
    // Mutation check: paint the builtin in black instead of the node's
    // colour and the first assertion fails.
    #[test]
    fn paint_builtin_draws_in_the_nodes_colour_through_the_same_stack() {
        let sheet = CompiledSheet::compile(
            "image { color: #3584e4; -gtk-icon-transform: translate(0, 0) }",
        );
        let node = Node::new("image");
        let env = ResolveEnv::default();
        let mut match_cx = MatchCx::new();
        let style = ComputedStyle::resolve_chain(&sheet, &node, &env, &mut match_cx);
        let mut fonts = FontDatabase::probe_only();
        let mut images = ImageCache::new();
        let mut icons = IconTheme::with_name_and_roots("MiniTheme", roots());
        let mut cx = PaintCx {
            env: &env,
            colors: &sheet.colors,
            fonts: &mut fonts,
            images: &mut images,
            icons: &mut icons,
            text: None,
        };
        let mut surface = Surface::new_raster_n32_premul(16, 16).expect("surface");
        {
            let mut canvas = surface.canvas();
            canvas.clear(Color::TRANSPARENT);
            paint_builtin(
                &mut canvas,
                Builtin::Radio,
                Rect::new(0.0, 0.0, 16.0, 16.0),
                &style,
                &mut cx,
            );
        }
        assert_eq!(pixel(&surface, 8, 8), Color(0xFF35_84E4));
        assert_eq!(pixel(&surface, 0, 0), Color(0x0000_0000));
    }
}
