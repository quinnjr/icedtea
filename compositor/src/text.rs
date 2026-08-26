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
/// The most bytes of `text` this function will ever hand to cosmic-text for
/// shaping (finding 3, security): a client's `xdg_toplevel.set_title` is
/// otherwise unbounded, and the title band clips the result visually
/// regardless of how long the string is, so nothing past this point could
/// ever be seen -- only shaped, at whatever cost an adversarial or buggy
/// client's string imposes. Comfortably longer than any title a real band
/// could show even at the smallest sane font size, short enough that
/// shaping it is not itself a way to burn CPU on every `set_title`.
pub(crate) const MAX_TITLE_BYTES: usize = 512;

/// `text` truncated to at most [`MAX_TITLE_BYTES`] bytes, on a `char`
/// boundary -- never splitting a multi-byte UTF-8 sequence, which would
/// otherwise hand cosmic-text (or a `str` slice) invalid input for a title
/// ending mid-codepoint.
pub(crate) fn cap_title(text: &str) -> &str {
    if text.len() <= MAX_TITLE_BYTES {
        return text;
    }
    let mut end = MAX_TITLE_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

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
    let text = cap_title(text);
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
    buffer.set_text(
        text,
        &Attrs::new().family(Family::SansSerif),
        Shaping::Advanced,
    );
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
                    // `color.a()` here is glyph *coverage*, not `fg`'s alpha:
                    // cosmic-text's `SwashCache::with_pixels` builds each
                    // mask-glyph pixel as `coverage << 24 | base.0 &
                    // 0xFF_FF_FF`, discarding the base color's alpha byte
                    // outright. Folding `fg[3]` back in here -- coverage and
                    // requested alpha multiplied, not either alone -- is
                    // what makes a dimmed `fg` actually dim the glyph instead
                    // of doing nothing.
                    let a = ((color.a() as u16 * fg[3] as u16) / 255) as u8;
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

/// Rasterizes a single short `glyph` (a button icon -- an en-dash, a
/// square, an "x") centered in a `width * height` cell: the button rect
/// itself, unlike [`rasterize_title`]'s left-aligned band. Same contract
/// otherwise -- premultiplied RGBA8888, exactly `width * height * 4` bytes,
/// `None` on a degenerate cell or a glyph that shapes to nothing (no font on
/// this machine has it) -- because this feeds the same buffer-node seam
/// (`Wayland::sync_ssd`) and that seam already knows how to degrade a `None`
/// into "no node," the same way it degrades a title that didn't shape.
///
/// Centering, both axes: `cosmic-text`'s own line-level `Align::Center`
/// handles the horizontal axis (set on the buffer's one line before
/// shaping, same as any other paragraph alignment), and the vertical axis
/// falls out of the same trick `rasterize_title` uses -- a `Metrics` line
/// height equal to the *whole* cell centers a single line inside it with no
/// extra bookkeeping.
pub fn rasterize_glyph(
    fonts: &mut cosmic_text::FontSystem,
    swash: &mut cosmic_text::SwashCache,
    glyph: &str,
    width: i32,
    height: i32,
    fg: [u8; 4],
) -> Option<Vec<u8>> {
    if width < 1 || height < 1 {
        return None;
    }
    use cosmic_text::{Align, Attrs, Buffer, Family, Metrics, Shaping};

    // A button glyph reads best a bit larger, proportionally, than title
    // text does -- there's no surrounding word to give it scale -- so this
    // uses a higher fraction of the cell than `rasterize_title`'s 0.55.
    let font_px = (height as f32 * 0.6).max(8.0);
    let metrics = Metrics::new(font_px, height as f32);
    let mut buffer = Buffer::new(fonts, metrics);
    let mut buffer = buffer.borrow_with(fonts);
    buffer.set_size(Some(width as f32), Some(height as f32));
    buffer.set_text(
        glyph,
        &Attrs::new().family(Family::SansSerif),
        Shaping::Advanced,
    );
    for line in buffer.lines.iter_mut() {
        line.set_align(Some(Align::Center));
    }
    buffer.shape_until_scroll(true);

    let mut px = vec![0u8; (width as usize) * (height as usize) * 4];
    let mut drew = false;
    buffer.draw(
        swash,
        cosmic_text::Color::rgba(fg[0], fg[1], fg[2], fg[3]),
        |x, y, w, h, color| {
            for dy in 0..h as i32 {
                for dx in 0..w as i32 {
                    let (tx, ty) = (x + dx, y + dy);
                    if tx < 0 || ty < 0 || tx >= width || ty >= height {
                        continue;
                    }
                    let a = color.a();
                    if a == 0 {
                        continue;
                    }
                    let i = ((ty * width + tx) * 4) as usize;
                    // Same coverage-max rationale as `rasterize_title`.
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
        let px = rasterize_title(
            &mut fonts,
            &mut swash,
            "Hello",
            200,
            28,
            8,
            [255, 255, 255, 255],
        )
        .expect("some pixels");
        assert_eq!(px.len(), 200 * 28 * 4);
        assert!(
            px.chunks_exact(4).any(|p| p[3] != 0),
            "at least one glyph pixel must be opaque"
        );
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
        let px = rasterize_title(
            &mut fonts,
            &mut swash,
            "i",
            200,
            28,
            8,
            [255, 255, 255, 255],
        )
        .expect("some pixels");
        let transparent = px
            .chunks_exact(4)
            .find(|p| p[3] == 0)
            .expect("some pixel stays clear");
        assert_eq!(
            transparent,
            [0, 0, 0, 0],
            "a clear pixel must be fully zeroed"
        );
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

    #[test]
    fn a_button_glyph_rasterizes_centered_nonempty() {
        let mut fonts = cosmic_text::FontSystem::new();
        let mut swash = cosmic_text::SwashCache::new();
        let px = rasterize_glyph(
            &mut fonts,
            &mut swash,
            "\u{2715}",
            40,
            28,
            [255, 255, 255, 255],
        )
        .expect("glyph");
        assert_eq!(px.len(), 40 * 28 * 4);
        assert!(
            px.chunks_exact(4).any(|p| p[3] != 0),
            "glyph has opaque pixels"
        );
    }

    #[test]
    fn glyph_degenerate_sizes_are_none_not_panic() {
        let mut fonts = cosmic_text::FontSystem::new();
        let mut swash = cosmic_text::SwashCache::new();
        assert!(rasterize_glyph(&mut fonts, &mut swash, "\u{2715}", 0, 28, [255; 4]).is_none());
        assert!(rasterize_glyph(&mut fonts, &mut swash, "\u{2715}", 40, 0, [255; 4]).is_none());
    }

    #[test]
    fn an_empty_glyph_draws_nothing() {
        let mut fonts = cosmic_text::FontSystem::new();
        let mut swash = cosmic_text::SwashCache::new();
        assert!(rasterize_glyph(&mut fonts, &mut swash, "", 40, 28, [255; 4]).is_none());
    }

    /// The glyph is centered, not left-aligned like a title: a narrow glyph
    /// in a wide cell must leave both margins clear, not just the right one.
    #[test]
    fn glyph_pixels_are_centered_not_left_aligned() {
        let mut fonts = cosmic_text::FontSystem::new();
        let mut swash = cosmic_text::SwashCache::new();
        let width = 80;
        let px = rasterize_glyph(
            &mut fonts,
            &mut swash,
            "\u{2715}",
            width,
            28,
            [255, 255, 255, 255],
        )
        .expect("glyph");
        let row_stride = width as usize * 4;
        let mut left_opaque = false;
        let mut right_opaque = false;
        for row in px.chunks_exact(row_stride) {
            for (x, p) in row.chunks_exact(4).enumerate() {
                if p[3] != 0 {
                    if x < width as usize / 4 {
                        left_opaque = true;
                    }
                    if x >= width as usize * 3 / 4 {
                        right_opaque = true;
                    }
                }
            }
        }
        assert!(
            !left_opaque,
            "centered glyph must leave the left quarter clear"
        );
        assert!(
            !right_opaque,
            "centered glyph must leave the right quarter clear"
        );
    }

    /// M3: a title rasterized with a lower-alpha `fg` must produce
    /// lower-alpha glyph pixels than the same title at full alpha -- the
    /// alpha-dimming contract `ensure_title_raster` relies on for unfocused
    /// windows instead of an RGB-blend approximation.
    #[test]
    fn a_dimmed_title_has_lower_alpha_than_a_full_title() {
        let mut fonts = cosmic_text::FontSystem::new();
        let mut swash = cosmic_text::SwashCache::new();
        let full = rasterize_title(
            &mut fonts,
            &mut swash,
            "Hi",
            200,
            28,
            8,
            [255, 255, 255, 255],
        )
        .expect("full");
        let dim = rasterize_title(
            &mut fonts,
            &mut swash,
            "Hi",
            200,
            28,
            8,
            [255, 255, 255, 153],
        )
        .expect("dim");
        let max_a = |px: &[u8]| px.chunks_exact(4).map(|p| p[3]).max().unwrap_or(0);
        assert!(
            max_a(&dim) < max_a(&full),
            "a lower-alpha fg yields lower-alpha glyph pixels"
        );
    }

    /// Finding 3: an unbounded client title must not reach cosmic-text at
    /// full length -- a string under the cap passes through untouched, one
    /// over it is truncated to exactly `MAX_TITLE_BYTES`, and the cut never
    /// lands mid-codepoint.
    #[test]
    fn cap_title_truncates_on_a_char_boundary() {
        let short = "a normal title";
        assert_eq!(
            cap_title(short),
            short,
            "under the cap must pass through unchanged"
        );

        let long = "x".repeat(MAX_TITLE_BYTES + 100);
        let capped = cap_title(&long);
        assert_eq!(capped.len(), MAX_TITLE_BYTES);

        // A multi-byte character straddling the cap must not be split --
        // the cut backs off to the nearest earlier char boundary instead of
        // slicing through the codepoint.
        let mut straddling = "y".repeat(MAX_TITLE_BYTES - 1);
        straddling.push('窓'); // 3-byte character landing right at the cap
        straddling.push_str(&"z".repeat(50));
        let capped = cap_title(&straddling);
        assert!(capped.len() <= MAX_TITLE_BYTES);
        assert!(straddling.is_char_boundary(capped.len()));
        assert!(
            capped.chars().all(|c| c != '\u{FFFD}'),
            "no replacement character from a mid-codepoint cut"
        );
    }
}
