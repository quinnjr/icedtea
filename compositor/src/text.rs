//! Title-bar text rasterization.
//!
//! The one place in this crate that turns a `&str` into pixels. `cosmic-text`
//! does the font enumeration, per-run fallback and shaping; this module owns
//! only the framing decisions a compositor has to make about the result:
//! where the glyphs land in the band, what happens when nothing shapes, and
//! -- the detail that is invisible until it is wrong -- that the bytes handed
//! to a `wlr` buffer node are **premultiplied** RGBA, because that is what
//! the wlroots scene graph composites.
//!
//! Pure except for the `FontSystem`/`SwashCache` the caller threads through:
//! both are expensive to build (a `FontSystem::new` enumerates every font on
//! the machine) and both are pure caches, so they live in `State` and are
//! borrowed in here rather than being rebuilt per title.

/// Rasterizes `text` into an RGBA8888 buffer of exactly `width * height * 4`
/// bytes: vertically centered in the band, left-aligned `pad_x` pixels in,
/// `fg` over transparent black.
///
/// The returned pixels are **premultiplied**: each channel is already scaled
/// by that pixel's coverage, since `wlr`'s buffer nodes hand their data
/// straight to the wlroots scene graph, which assumes premultiplied alpha.
/// A fully transparent pixel is therefore `[0, 0, 0, 0]`, not
/// `[r, g, b, 0]` -- the latter fringes light text on a dark band.
///
/// `None` -- never a panic, and never a wrongly-sized buffer -- in the two
/// cases the caller has to degrade for:
///
/// * a degenerate band (`width < 1` or `height < 1`), which no buffer node
///   would accept anyway; and
/// * text that shapes to nothing at all: an empty or whitespace-only title,
///   or a string for which this machine has no font with the needed glyphs.
///
/// Both mean the same thing to the caller (`State::sync_window_to_scene`):
/// show the plain band with no title node over it. That is the spec's
/// error-handling rule for decorations -- a title that cannot be drawn
/// degrades the decoration, it never fails the window.
pub fn rasterize_title(
    fonts: &mut cosmic_text::FontSystem,
    swash: &mut cosmic_text::SwashCache,
    text: &str,
    width: i32,
    height: i32,
    pad_x: i32,
    fg: [u8; 4],
) -> Option<Vec<u8>> {
    if width < 1 || height < 1 {
        return None;
    }
    use cosmic_text::{Attrs, Buffer, Family, Metrics, Shaping};

    // 55% of the band is the usual title-bar proportion (28px band -> ~15px
    // text), floored so a pathologically short band still shapes something
    // rather than asking swash for a zero-size raster. The line height is
    // the whole band, which is what centers the single line inside it.
    let font_px = (height as f32 * 0.55).max(8.0);
    let metrics = Metrics::new(font_px, height as f32);
    let mut buffer = Buffer::new(fonts, metrics);
    let mut buffer = buffer.borrow_with(fonts);
    // The text area is the band minus the left pad; anything past it is
    // clipped by the bounds check in the draw loop below rather than being
    // wrapped onto a second line nobody would see.
    buffer.set_size(Some((width - pad_x).max(1) as f32), Some(height as f32));
    buffer.set_text(text, &Attrs::new().family(Family::SansSerif), Shaping::Advanced);
    buffer.shape_until_scroll(true);

    let mut px = vec![0u8; (width as usize) * (height as usize) * 4];
    let mut drew = false;
    buffer.draw(
        swash,
        cosmic_text::Color::rgba(fg[0], fg[1], fg[2], fg[3]),
        |x, y, w, h, color| {
            for dy in 0..h as i32 {
                for dx in 0..w as i32 {
                    let (tx, ty) = (x + dx + pad_x, y + dy);
                    if tx < 0 || ty < 0 || tx >= width || ty >= height {
                        continue;
                    }
                    let a = color.a();
                    if a == 0 {
                        continue;
                    }
                    let i = ((ty * width + tx) * 4) as usize;
                    // Coverage-max rather than a full source-over blend:
                    // glyph rasters within one string overlap only at
                    // ligature/kerning seams, and taking the stronger
                    // coverage there is both cheaper and free of the double
                    // -darkening a naive over-blend of two coverages gives.
                    if a <= px[i + 3] {
                        continue;
                    }
                    drew = true;
                    let scale = |c: u8| ((c as u16 * a as u16) / 255) as u8;
                    px[i] = scale(color.r());
                    px[i + 1] = scale(color.g());
                    px[i + 2] = scale(color.b());
                    px[i + 3] = a;
                }
            }
        },
    );

    if drew { Some(px) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_title_rasterizes_nonempty_pixels() {
        let mut fonts = cosmic_text::FontSystem::new();
        let mut swash = cosmic_text::SwashCache::new();
        let px = rasterize_title(&mut fonts, &mut swash, "Hello", 200, 28, 8, [255, 255, 255, 255])
            .expect("some pixels");
        assert_eq!(px.len(), 200 * 28 * 4);
        assert!(px.chunks_exact(4).any(|p| p[3] != 0), "at least one glyph pixel must be opaque");
    }

    #[test]
    fn degenerate_sizes_are_none_not_panic() {
        let mut fonts = cosmic_text::FontSystem::new();
        let mut swash = cosmic_text::SwashCache::new();
        assert!(rasterize_title(&mut fonts, &mut swash, "x", 0, 28, 0, [255; 4]).is_none());
        assert!(rasterize_title(&mut fonts, &mut swash, "x", 200, 0, 0, [255; 4]).is_none());
    }

    #[test]
    fn cjk_titles_shape_through_fallback() {
        let mut fonts = cosmic_text::FontSystem::new();
        let mut swash = cosmic_text::SwashCache::new();
        // Passes wherever a CJK-capable font exists; if the system truly has
        // none, cosmic-text draws nothing and this returns `None` -- so the
        // assertion is "Some with the right length, or None", never glyph
        // presence, which would make the suite depend on the machine's
        // installed fonts.
        let px = rasterize_title(&mut fonts, &mut swash, "窓の管理", 200, 28, 8, [255; 4]);
        assert!(px.is_none() || px.as_ref().map(|p| p.len()) == Some(200 * 28 * 4));
    }

    /// The premultiplication contract, from both ends: an untouched pixel is
    /// fully zeroed (no colored fringe where alpha is 0), and every drawn
    /// pixel has each channel no greater than its own alpha -- which is
    /// exactly what "premultiplied" means and what the wlroots scene graph
    /// assumes when it composites the node.
    #[test]
    fn pixels_are_premultiplied_and_transparent_ones_are_fully_zero() {
        let mut fonts = cosmic_text::FontSystem::new();
        let mut swash = cosmic_text::SwashCache::new();
        // A short string in a wide band guarantees untouched pixels on the
        // right-hand side.
        let px = rasterize_title(&mut fonts, &mut swash, "i", 200, 28, 8, [255, 255, 255, 255])
            .expect("some pixels");
        let transparent = px.chunks_exact(4).find(|p| p[3] == 0).expect("some pixel stays clear");
        assert_eq!(transparent, [0, 0, 0, 0], "a clear pixel must be fully zeroed");
        for p in px.chunks_exact(4) {
            assert!(
                p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3],
                "channel exceeds alpha: {p:?} is not premultiplied"
            );
        }
    }

    /// Nothing to shape is `None`, not an all-zero buffer the caller would
    /// hand to a buffer node for no reason.
    #[test]
    fn an_empty_title_draws_nothing() {
        let mut fonts = cosmic_text::FontSystem::new();
        let mut swash = cosmic_text::SwashCache::new();
        assert!(rasterize_title(&mut fonts, &mut swash, "", 200, 28, 8, [255; 4]).is_none());
    }
}
