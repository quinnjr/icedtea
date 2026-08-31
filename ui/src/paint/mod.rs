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
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

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

/// How long a `url()` that failed to resolve stays a recorded miss.
///
/// A miss must not be permanent: a relative `url()` is resolved against the
/// stylesheet's own directory, and a theme that is being edited (or one
/// whose files arrive after the first frame) would otherwise never paint its
/// images for the lifetime of the process. Long enough that a missing file
/// is not re-`stat`ed every frame, short enough that a fixed one shows up.
const IMAGE_MISS_COOLDOWN: Duration = Duration::from_secs(5);

/// One cache slot: the decoded image, or the moment the decode last failed.
enum CacheEntry {
    /// Decoded and ready.
    Decoded(DecodedImage),
    /// The last attempt failed, at this instant. Retried after
    /// [`IMAGE_MISS_COOLDOWN`].
    Missed(Instant),
}

/// Decoded `url()` images, keyed by `(base directory, url)`.
///
/// The base directory is part of the key because a relative `url()` means
/// different files under different stylesheets, and one `ImageCache` outlives
/// a restyle: keying on the URL alone let a theme swap serve the previous
/// theme's bitmap.
///
/// A URL that fails to decode is *recorded unresolved*: it is remembered as
/// a miss so the file is not re-read every frame, logged once, and paints
/// nothing. That is not an error -- the computed value keeps the URL. The
/// miss expires after [`IMAGE_MISS_COOLDOWN`] rather than lasting forever.
pub struct ImageCache {
    /// What a relative `url()` resolves against: the stylesheet's own
    /// directory. Empty means "the process's working directory", which is
    /// the only thing a sheet with no path of its own can mean.
    base_dir: PathBuf,
    entries: HashMap<(PathBuf, String), CacheEntry>,
}

impl Default for ImageCache {
    fn default() -> Self {
        Self::new()
    }
}

impl ImageCache {
    /// An empty cache resolving relative URLs against the working directory.
    #[must_use]
    pub fn new() -> Self {
        Self {
            base_dir: PathBuf::new(),
            entries: HashMap::new(),
        }
    }

    /// Resolve relative `url()`s against `dir` from now on.
    ///
    /// Already-cached entries are keyed by the base they were read under, so
    /// this never serves the previous base's pixels.
    pub fn set_base_dir(&mut self, dir: impl Into<PathBuf>) {
        self.base_dir = dir.into();
    }

    /// The path `url` names, resolved against the base directory.
    ///
    /// An absolute path is used as-is; CSS `url()`s in a GTK theme are
    /// otherwise relative to the sheet that declared them.
    #[must_use]
    fn resolve(&self, url: &str) -> PathBuf {
        let path = Path::new(url);
        if path.is_absolute() || self.base_dir.as_os_str().is_empty() {
            path.to_path_buf()
        } else {
            self.base_dir.join(path)
        }
    }

    /// Forget every recorded miss, as [`IMAGE_MISS_COOLDOWN`] passing does.
    ///
    /// Test-only: the cooldown is measured against a real `Instant`, and a
    /// test that slept through five seconds to prove a miss expires would be
    /// five seconds of nothing.
    #[cfg(test)]
    fn expire_misses_for_test(&mut self) {
        self.entries
            .retain(|_, entry| matches!(entry, CacheEntry::Decoded(_)));
    }

    /// The decoded image for `url`, decoding it on first use.
    pub fn get(&mut self, url: &str) -> Option<&DecodedImage> {
        let key = (self.base_dir.clone(), url.to_owned());
        let now = Instant::now();
        let retry = match self.entries.get(&key) {
            Some(CacheEntry::Decoded(_)) => false,
            Some(CacheEntry::Missed(at)) => now.duration_since(*at) >= IMAGE_MISS_COOLDOWN,
            None => true,
        };
        if retry {
            let path = self.resolve(url);
            let decoded = std::fs::read(&path)
                .ok()
                .and_then(|bytes| skia_rs_safe::codec::decode_image(&bytes).ok());
            let entry = match decoded {
                Some(image) => CacheEntry::Decoded(image),
                None => {
                    tracing::debug!(
                        url,
                        path = %path.display(),
                        "background image could not be decoded; recorded unresolved"
                    );
                    CacheEntry::Missed(now)
                }
            };
            self.entries.insert(key.clone(), entry);
        }
        match self.entries.get(&key) {
            Some(CacheEntry::Decoded(image)) => Some(image),
            _ => None,
        }
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
/// `overrides` layers the animation state's sampled output over `style`.
///
/// `node` is unread here: painting one node needs only its computed style
/// and its allocation. It stays in the signature because contract §8 names
/// it, and because M3's tree walk -- which descends from a node to its
/// children -- is the caller this function is shaped for.
#[allow(
    unused_variables,
    reason = "`node` is contract §8's signature; the tree walk that passes it \
              is `view::render::paint_tree` (M3 P4)"
)]
pub fn paint_node(
    canvas: &mut Canvas<'_>,
    node: &Node,
    style: &ComputedStyle,
    alloc: &Allocation,
    overrides: Option<&Overrides>,
    cx: &mut PaintCx<'_>,
) {
    paint_node_with_children(canvas, node, style, alloc, overrides, cx, |_, _| {});
}

/// Paint one node, then its children, inside the node's own effect layer.
///
/// `opacity`, `transform` and `filter` are one save-layer, and a save-layer
/// only affects what is drawn while it is open. [`paint_node`] opens and
/// closes it around *this node's* boxes, so a caller that paints the
/// children afterwards paints them onto a fresh canvas: `button { opacity:
/// .5 }` left the label fully opaque, a hover `translateY` moved the
/// background out from under a stationary label, and a `grayscale()` filter
/// missed the label entirely.
///
/// `children` runs with the layer still open, immediately after this node's
/// own paint and before the layer is popped, which is the tree order CSS
/// wants. It receives the canvas and the paint context so a child can paint
/// its own text and images.
#[allow(
    unused_variables,
    reason = "`node` is contract §8's signature; the tree walk that passes it \
              is `view::render::paint_tree` (M3 P4)"
)]
pub fn paint_node_with_children<'cx>(
    canvas: &mut Canvas<'_>,
    node: &Node,
    style: &ComputedStyle,
    alloc: &Allocation,
    overrides: Option<&Overrides>,
    cx: &mut PaintCx<'cx>,
    children: impl FnOnce(&mut Canvas<'_>, &mut PaintCx<'cx>),
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
        text::paint_text(canvas, shaped, alloc.content_box, style, &len_ctx);
    }

    children(canvas, cx);

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
    #[test]
    fn the_image_cache_resolves_relative_urls_against_its_base_directory() {
        // A relative `url()` in a GTK theme means "relative to this sheet",
        // not "relative to whatever directory the process happens to be in".
        // Mutation check: drop `resolve` and read `url` directly and the
        // first lookup misses.
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("bg.png"), RED_3X3_PNG).expect("write");

        let mut cache = ImageCache::new();
        assert!(
            cache.get("bg.png").is_none(),
            "with no base directory the relative url is not found"
        );
        cache.set_base_dir(dir.path());
        assert!(
            cache.get("bg.png").is_some(),
            "under the sheet's own directory it is"
        );
    }

    #[test]
    fn a_cache_miss_is_not_permanent() {
        // A miss used to be remembered forever, so a theme whose image
        // arrives after the first frame never painted it for the lifetime of
        // the process. Mutation check: store `None` forever and the second
        // lookup still misses.
        let dir = tempfile::tempdir().expect("tempdir");
        let mut cache = ImageCache::new();
        cache.set_base_dir(dir.path());
        assert!(cache.get("late.png").is_none(), "not there yet");

        std::fs::write(dir.path().join("late.png"), RED_3X3_PNG).expect("write");
        // The miss is still inside its cooldown, so it is *not* re-read --
        // that is the point of recording it. Expiring the entry is what a
        // later frame does.
        assert!(cache.get("late.png").is_none(), "still inside the cooldown");
        cache.expire_misses_for_test();
        assert!(
            cache.get("late.png").is_some(),
            "once the cooldown passes the file is read again"
        );
    }

    #[test]
    fn two_base_directories_do_not_share_a_relative_urls_pixels() {
        // The key is `(base directory, url)`: one `ImageCache` outlives a
        // restyle, and keying on the URL alone let a theme swap serve the
        // previous theme's bitmap. Mutation check: key on `url` alone and
        // the second lookup returns the first directory's image.
        let first = tempfile::tempdir().expect("tempdir");
        let second = tempfile::tempdir().expect("tempdir");
        std::fs::write(first.path().join("bg.png"), RED_3X3_PNG).expect("write");

        let mut cache = ImageCache::new();
        cache.set_base_dir(first.path());
        assert!(cache.get("bg.png").is_some());
        cache.set_base_dir(second.path());
        assert!(
            cache.get("bg.png").is_none(),
            "the second theme has no bg.png of its own"
        );
    }

    /// A 3x3 opaque-red PNG, as in `paint::border`'s tests.
    const RED_3X3_PNG: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00, 0x03, 0x08, 0x06, 0x00, 0x00, 0x00, 0x56,
        0x28, 0xb5, 0xbf, 0x00, 0x00, 0x00, 0x11, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8,
        0xcf, 0xc0, 0xf0, 0x1f, 0x86, 0x19, 0x70, 0x72, 0x00, 0x5d, 0xd7, 0x11, 0xef, 0xdc, 0x4f,
        0x31, 0x10, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];
}
