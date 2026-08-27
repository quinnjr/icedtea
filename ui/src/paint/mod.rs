//! The CSS painter: one module per property family, `paint_node` for the
//! order they run in.

pub mod background;
pub mod blur;
pub mod border;
pub mod effects;
pub mod geometry;
pub mod outline;
pub mod shadow;
pub mod text;

use std::collections::HashMap;

use skia_rs_safe::canvas::Canvas;
use skia_rs_safe::codec::Image as DecodedImage;
use skia_rs_safe::paint::{Paint, Style};

use crate::anim::Overrides;
use crate::css::computed::{ComputedStyle, ResolveEnv};
use crate::css::node::Node;
use crate::css::registry::Prop;
use crate::css::value::{
    BorderImageSlice, BorderImageWidthSide, ColorCtx, ColorTable, Image, Keyword, LengthCtx,
    RepeatStyle, Rgba, Value,
};
use crate::layout::Allocation;
use crate::text::{FontDatabase, ShapedText};

pub use crate::css::computed::BackgroundLayer;
pub use background::paint_backgrounds;
pub use border::{is_visible_border_style, paint_border_image, paint_borders};
pub use effects::{begin_effects, end_effects};
pub use geometry::{
    Side, clamp_radii, inner_radii, rounded_rect_path, rounded_ring_path, side_wedge_path,
};
pub use outline::paint_outline;
pub use shadow::paint_box_shadows;
pub use text::paint_text;

/// Everything a paint pass needs beyond the node's own style and geometry.
pub struct PaintCx<'a> {
    /// DPI and root font size, for length resolution at paint time.
    pub env: &'a ResolveEnv,
    /// The sheet's `@define-color` table, still unresolved.
    pub colors: &'a ColorTable,
    /// Font matching, loading and shaping.
    pub fonts: &'a mut FontDatabase,
    /// Decoded `url()` images.
    pub images: &'a mut ImageCache,
    /// This node's own label, already shaped.
    pub text: Option<&'a ShapedText>,
}

impl PaintCx<'_> {
    /// A colour-resolution context whose `currentColor` is `current`.
    #[must_use]
    pub fn color_ctx(&self, current: Rgba) -> ColorCtx<'_> {
        ColorCtx {
            table: self.colors,
            current,
            depth: 0,
        }
    }

    /// A length context for paint-time resolution: `em` is unavailable here
    /// (the computed style already resolved it), so only absolute units,
    /// the DPI and an explicit percentage basis matter.
    #[must_use]
    pub fn base_length_ctx(&self) -> LengthCtx {
        LengthCtx {
            font_size_px: self.env.root_font_size,
            root_font_size_px: self.env.root_font_size,
            ex_ratio: 0.5,
            dpi: self.env.dpi,
            percent_basis: None,
        }
    }
}

/// Decoded `url()` images, keyed by URL.
///
/// A URL that fails to decode is *recorded unresolved*: it is remembered as
/// a miss so the file is not re-read every frame, logged once, and paints
/// nothing. That is not an error -- the computed value keeps the URL.
pub struct ImageCache {
    entries: HashMap<String, Option<DecodedImage>>,
}

impl Default for ImageCache {
    fn default() -> Self {
        Self::new()
    }
}

impl ImageCache {
    /// An empty cache.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// The decoded image for `url`, decoding it on first use.
    pub fn get(&mut self, url: &str) -> Option<&DecodedImage> {
        if !self.entries.contains_key(url) {
            let decoded = std::fs::read(url)
                .ok()
                .and_then(|bytes| skia_rs_safe::codec::decode_image(&bytes).ok());
            if decoded.is_none() {
                tracing::debug!(
                    url,
                    "background image could not be decoded; recorded unresolved"
                );
            }
            self.entries.insert(url.to_owned(), decoded);
        }
        self.entries.get(url).and_then(Option::as_ref)
    }
}

/// An anti-aliased fill paint in `color`.
#[must_use]
pub fn fill_paint(color: Rgba) -> Paint {
    let mut paint = Paint::new();
    paint.set_color32(color.to_color32());
    paint.set_style(Style::Fill);
    paint.set_anti_alias(true);
    paint
}

/// The corner radii of the box `k` names, shrunk from the border box.
#[must_use]
pub fn radii_for_box(radii: &[[f32; 2]; 4], alloc: &Allocation, k: Keyword) -> [[f32; 2]; 4] {
    match k {
        Keyword::PaddingBox => inner_radii(radii, alloc.border),
        Keyword::ContentBox => {
            let sides = [
                alloc.border[0] + alloc.padding[0],
                alloc.border[1] + alloc.padding[1],
                alloc.border[2] + alloc.padding[2],
                alloc.border[3] + alloc.padding[3],
            ];
            inner_radii(radii, sides)
        }
        _ => *radii,
    }
}

/// Paint one node, in CSS paint order.
///
/// outset `box-shadow` -> background layers -> `border-image` (else per-side
/// borders) -> inset `box-shadow` -> `outline` -> text. Children are painted
/// by the caller, which owns the tree walk and each child's allocation.
///
/// `overrides` layers Part 5's animation output over `style`.
pub fn paint_node(
    canvas: &mut Canvas<'_>,
    _node: &Node,
    style: &ComputedStyle,
    alloc: &Allocation,
    overrides: Option<&Overrides>,
    cx: &mut PaintCx<'_>,
) {
    let owned;
    let style: &ComputedStyle = match overrides {
        Some(o) if !o.is_empty() => {
            owned = style.with_overrides(o).into_owned();
            &owned
        }
        _ => style,
    };

    let save = effects::begin_effects(canvas, style, alloc, &cx.base_length_ctx());
    let len_ctx = cx.base_length_ctx();
    let radii = style.border_radii(alloc.border_box.width, alloc.border_box.height);
    let current = style.color();
    let shadows = style.box_shadows();

    shadow::paint_box_shadows(canvas, &shadows, false, alloc, &radii, current, &len_ctx);

    let layers = style.background_layers();
    let background_color: Rgba = style.get(Prop::BackgroundColor);
    paint_backgrounds(canvas, background_color, &layers, alloc, &radii, cx);

    let source: Image = style.get(Prop::BorderImageSource);
    let slice: BorderImageSlice = match style.raw(Prop::BorderImageSlice) {
        Value::Slice(slice) => slice.clone(),
        _ => BorderImageSlice {
            sides: [crate::css::value::NumberOrPercent::Number(100.0); 4],
            fill: false,
        },
    };
    let widths: [BorderImageWidthSide; 4] = match style.raw(Prop::BorderImageWidth) {
        Value::BorderImageWidths(sides) => sides.clone(),
        _ => std::array::from_fn(|_| BorderImageWidthSide::Number(1.0)),
    };
    // `RepeatStyle` has no `FromValue` impl (contract §5's `impl FromValue`
    // list does not register one), so it is read straight off the raw
    // `Value::Repeat`, falling back to the registry's own initial value
    // (`stretch stretch`, `registry.rs:883`) for anything else.
    let repeat: RepeatStyle = match style.raw(Prop::BorderImageRepeat) {
        Value::Repeat(repeat) => *repeat,
        _ => RepeatStyle {
            x: Keyword::Stretch,
            y: Keyword::Stretch,
        },
    };
    let drew_border_image =
        border::paint_border_image(canvas, alloc, &source, &slice, &widths, repeat, cx);
    if !drew_border_image {
        border::paint_borders(
            canvas,
            alloc,
            style.border_widths(),
            style.border_colors(),
            style.border_styles(),
            &radii,
        );
    }

    shadow::paint_box_shadows(canvas, &shadows, true, alloc, &radii, current, &len_ctx);

    let outline_width: f32 = style.get(Prop::OutlineWidth);
    let outline_offset: f32 = style.get(Prop::OutlineOffset);
    let outline_color: Rgba = style.get(Prop::OutlineColor);
    let outline_style: Keyword = style.get(Prop::OutlineStyle);
    outline::paint_outline(
        canvas,
        alloc,
        outline_width,
        outline_offset,
        outline_color,
        outline_style,
        &radii,
    );

    if let Some(shaped) = cx.text {
        text::paint_text(
            canvas,
            shaped,
            (alloc.content_box.x, alloc.content_box.y),
            style,
            &len_ctx,
        );
    }

    effects::end_effects(canvas, save);
}

#[cfg(test)]
mod tests {
    use super::{ImageCache, PaintCx, paint_node};
    use crate::css::cascade::CompiledSheet;
    use crate::css::computed::{ComputedStyle, ResolveEnv};
    use crate::css::node::Node;
    use crate::css::select::MatchCx;
    use crate::layout::{Allocation, Rect};
    use crate::text::FontDatabase;
    use skia_rs_safe::canvas::Surface;
    use skia_rs_safe::core::Color;

    fn painted(css: &str) -> Surface {
        let sheet = CompiledSheet::compile(css);
        let node = Node::new("button");
        let env = ResolveEnv::default();
        let mut cx = MatchCx::new();
        let style = ComputedStyle::resolve_chain(&sheet, &node, &env, &mut cx);
        let border_box = Rect::new(10.0, 10.0, 40.0, 20.0);
        let alloc = Allocation {
            border_box,
            content_box: border_box.inset([4.0; 4]),
            border: [4.0; 4],
            padding: [0.0; 4],
        };
        let mut fonts = FontDatabase::probe_only();
        let mut images = ImageCache::new();
        let mut paint_cx = PaintCx {
            env: &env,
            colors: &sheet.colors,
            fonts: &mut fonts,
            images: &mut images,
            text: None,
        };
        let mut surface = Surface::new_raster_n32_premul(80, 60).expect("raster surface");
        surface.canvas().clear(Color::TRANSPARENT);
        {
            let mut canvas = surface.canvas();
            paint_node(&mut canvas, &node, &style, &alloc, None, &mut paint_cx);
        }
        surface
    }

    fn pixel(surface: &Surface, x: i32, y: i32) -> Color {
        surface.pixel_buffer().get_pixel(x, y).expect("pixel")
    }

    #[test]
    fn the_border_paints_over_the_background_not_under_it() {
        // CSS paint order: backgrounds first, then borders. Mutation check:
        // swapping the two makes the border pixel read the background.
        let surface = painted(
            "button { background-color: #112233; border: 4px solid #ff0000; \
             border-radius: 0 }",
        );
        assert_eq!(pixel(&surface, 30, 11), Color(0xFFFF_0000), "border on top");
        assert_eq!(
            pixel(&surface, 30, 20),
            Color(0xFF11_2233),
            "interior below"
        );
    }

    #[test]
    fn an_outset_box_shadow_paints_behind_the_background() {
        // Mutation check: painting shadows after the background hides the
        // shadow entirely wherever the background is opaque.
        let surface = painted(
            "button { background-color: #ffffff; border: 0 solid transparent; \
             border-radius: 0; box-shadow: 6px 0 #000000 }",
        );
        assert_eq!(
            pixel(&surface, 54, 20),
            Color(0xFF00_0000),
            "shadow to the right"
        );
        assert_eq!(
            pixel(&surface, 30, 20),
            Color(0xFFFF_FFFF),
            "not under the box"
        );
    }

    #[test]
    fn the_outline_paints_outside_the_border_after_it() {
        // Mutation check: painting the outline before the border lets the
        // border overdraw a negative-offset ring.
        let surface = painted(
            "button { background-color: #112233; border: 0 solid transparent; \
             border-radius: 0; outline: 2px solid #00ff00; outline-offset: 2px }",
        );
        assert_eq!(pixel(&surface, 30, 7), Color(0xFF00_FF00));
        assert_eq!(pixel(&surface, 30, 9).alpha(), 0, "the 2px gap is clear");
    }

    #[test]
    fn a_border_image_replaces_the_per_side_borders_when_it_paints() {
        // Contract §8: border-image wins; a source with no pixels falls back.
        // Mutation check: painting both draws the plain border over the
        // nine-patch, so an undecodable url() would look identical either way.
        let surface = painted(
            "button { border: 4px solid #ff0000; border-radius: 0; \
             border-image-source: url(\"/nonexistent/icedtea-border.png\") }",
        );
        assert_eq!(
            pixel(&surface, 30, 11),
            Color(0xFFFF_0000),
            "an unresolvable border-image falls back to the plain border"
        );
    }

    #[test]
    fn paint_node_never_panics_on_a_degenerate_allocation() {
        let sheet = CompiledSheet::compile("button { border: 1px solid #000 }");
        let node = Node::new("button");
        let env = ResolveEnv::default();
        let mut cx = MatchCx::new();
        let style = ComputedStyle::resolve_chain(&sheet, &node, &env, &mut cx);
        let mut fonts = FontDatabase::probe_only();
        let mut images = ImageCache::new();
        let mut paint_cx = PaintCx {
            env: &env,
            colors: &sheet.colors,
            fonts: &mut fonts,
            images: &mut images,
            text: None,
        };
        let mut surface = Surface::new_raster_n32_premul(8, 8).expect("raster surface");
        let mut canvas = surface.canvas();
        for &v in &[f32::NAN, f32::INFINITY, -100.0, 0.0] {
            let rect = Rect::new(v, v, v, v);
            let alloc = Allocation {
                border_box: rect,
                content_box: rect,
                border: [v; 4],
                padding: [v; 4],
            };
            paint_node(&mut canvas, &node, &style, &alloc, None, &mut paint_cx);
        }
    }
}
