//! The CSS painter: one module per property family, `paint_node` for the
//! order they run in.

pub mod background;
pub mod blur;
pub mod geometry;

use std::collections::HashMap;

use skia_rs_safe::codec::Image as DecodedImage;
use skia_rs_safe::paint::{Paint, Style};

use crate::css::computed::ResolveEnv;
use crate::css::value::{ColorCtx, ColorTable, Keyword, LengthCtx, Rgba};
use crate::layout::Allocation;
use crate::text::{FontDatabase, ShapedText};

pub use crate::css::computed::BackgroundLayer;
pub use background::paint_backgrounds;
pub use geometry::{
    Side, clamp_radii, inner_radii, rounded_rect_path, rounded_ring_path, side_wedge_path,
};

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
