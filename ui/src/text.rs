//! The text stack: `skia-rs-text`, no fontconfig, no `cosmic-text`.
//!
//! M1's resolution of the spec's "text stack decision" risk. `skia-rs-text`
//! already loads a TTF/OTF from raw bytes (`ttf_parser`), shapes with
//! `rustybuzz`, reports real `hmtx` advances, and rasterizes glyph outlines
//! through `Canvas::draw_text_blob` -- so the whole shape/measure/draw path
//! is one crate and `cosmic-text` buys nothing here.
//!
//! Font *discovery* is deliberately a fixed probe list rather than
//! fontconfig: M1 needs one predictable UI face, and linking fontconfig
//! would re-import exactly the platform dependency this rebuild is leaving.
//! Real font matching (family/weight/style from the theme's `font-family`)
//! is M2/M3 work.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use skia_rs_safe::core::Point;
use skia_rs_safe::text::{Font, Shaper, TextBlob, TextBlobBuilder, Typeface};

/// Well-known UI sans-serif faces, in preference order.
pub const FONT_CANDIDATES: &[&str] = &[
    "/usr/share/fonts/Adwaita/AdwaitaSans-Regular.ttf",
    "/usr/share/fonts/cantarell/Cantarell-Regular.otf",
    "/usr/share/fonts/noto/NotoSans-Regular.ttf",
    "/usr/share/fonts/TTF/DejaVuSans.ttf",
    "/usr/share/fonts/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/liberation/LiberationSans-Regular.ttf",
    "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
];

/// Measured extents of a laid-out string.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextMetrics {
    /// Total advance width in px.
    pub width: f32,
    /// Distance from the baseline to the top of the text, positive upward.
    pub ascent: f32,
    /// Distance from the baseline to the bottom of the text, positive downward.
    pub descent: f32,
    /// Recommended distance between successive baselines.
    pub line_height: f32,
}

/// A loaded typeface plus a shaper.
pub struct FontStack {
    typeface: Arc<Typeface>,
    shaper: Shaper,
}

impl FontStack {
    /// Load the first readable, parseable face from [`FONT_CANDIDATES`].
    #[must_use]
    pub fn system() -> Option<Self> {
        for candidate in FONT_CANDIDATES {
            let path = PathBuf::from(candidate);
            if let Some(stack) = Self::from_file(&path) {
                tracing::debug!(font = %path.display(), "loaded UI typeface");
                return Some(stack);
            }
        }
        tracing::warn!(candidates = FONT_CANDIDATES.len(), "no UI typeface found");
        None
    }

    /// Load a specific font file.
    #[must_use]
    pub fn from_file(path: &Path) -> Option<Self> {
        let data = std::fs::read(path).ok()?;
        let typeface = Typeface::from_data(data)?;
        Some(Self {
            typeface: Arc::new(typeface),
            shaper: Shaper::new(),
        })
    }

    /// The loaded face's family name.
    #[must_use]
    pub fn family_name(&self) -> &str {
        self.typeface.family_name()
    }

    /// A [`Font`] at `size_px`.
    #[must_use]
    pub fn font(&self, size_px: f32) -> Font {
        Font::new(Arc::clone(&self.typeface), size_px)
    }

    /// Measure `text` at `size_px`.
    ///
    /// Width comes from the shaper's advances when shaping succeeds (so
    /// kerning and ligatures count), and from `Font::measure_text`'s
    /// per-glyph `hmtx` sum otherwise.
    #[must_use]
    pub fn measure(&self, text: &str, size_px: f32) -> TextMetrics {
        let font = self.font(size_px);
        let metrics = font.metrics();
        let width = match self.shaper.shape_auto(text, &font) {
            Some(runs) if !runs.is_empty() => runs.iter().map(|run| run.width).sum(),
            _ => font.measure_text(text),
        };
        TextMetrics {
            width,
            // `FontMetrics::ascent` is negative (above the baseline), matching Skia.
            ascent: -metrics.ascent,
            descent: metrics.descent,
            line_height: metrics.line_height(),
        }
    }

    /// Shape `text` into a blob whose glyph positions are relative to the
    /// origin `(0, 0)` on the baseline.
    #[must_use]
    pub fn blob(&self, text: &str, size_px: f32) -> Option<TextBlob> {
        let font = self.font(size_px);
        let runs = self.shaper.shape_auto(text, &font)?;
        let mut builder = TextBlobBuilder::new();
        let mut pen_x = 0.0f32;
        let mut any = false;
        for run in &runs {
            let mut glyphs = Vec::with_capacity(run.glyphs.len());
            let mut positions = Vec::with_capacity(run.glyphs.len());
            for glyph in &run.glyphs {
                glyphs.push(glyph.glyph_id.0);
                positions.push(Point::new(pen_x + glyph.x_offset, glyph.y_offset));
                pen_x += glyph.x_advance;
            }
            if !glyphs.is_empty() {
                builder.add_positioned_run(&run.font, &glyphs, &positions);
                any = true;
            }
        }
        if !any {
            return None;
        }
        builder.build()
    }
}

#[cfg(test)]
mod tests {
    use super::FontStack;

    #[test]
    fn a_system_typeface_is_found_without_fontconfig() {
        let stack = FontStack::system()
            .expect("no font found among FONT_CANDIDATES; install dejavu/liberation/noto sans");
        assert!(!stack.family_name().is_empty());
    }

    #[test]
    fn measurement_scales_with_size_and_length() {
        let stack = FontStack::system().expect("system font");
        let small = stack.measure("Click me", 14.0);
        let large = stack.measure("Click me", 28.0);
        assert!(
            small.width > 0.0,
            "zero-width measurement means no hmtx data reached us"
        );
        assert!(
            large.width > small.width * 1.8,
            "28px measured {} vs 14px {}: advances are not scaling with size",
            large.width,
            small.width
        );
        let longer = stack.measure("Click me twice", 14.0);
        assert!(longer.width > small.width);

        assert!(small.ascent > 0.0 && small.descent > 0.0);
        assert!(small.line_height >= small.ascent + small.descent);
    }

    #[test]
    fn shaping_produces_one_positioned_glyph_per_character() {
        let stack = FontStack::system().expect("system font");
        let blob = stack
            .blob("Click me", 14.0)
            .expect("shaping produced no runs");
        let glyphs: usize = blob.runs().iter().map(|r| r.glyphs.len()).sum();
        assert_eq!(
            glyphs,
            "Click me".chars().count(),
            "Latin text with no ligatures should shape 1:1"
        );
        for run in blob.runs() {
            assert_eq!(run.glyphs.len(), run.positions.len());
        }
        // Positions must advance left to right.
        let run = &blob.runs()[0];
        for pair in run.positions.windows(2) {
            assert!(pair[1].x > pair[0].x, "glyph positions did not advance");
        }
    }

    #[test]
    fn blob_width_agrees_with_measure() {
        let stack = FontStack::system().expect("system font");
        let metrics = stack.measure("Click me", 14.0);
        let blob = stack.blob("Click me", 14.0).expect("blob");
        let last = blob.runs()[0].positions.last().copied().expect("positions");
        assert!(
            last.x < metrics.width && last.x > metrics.width * 0.5,
            "last glyph origin {} is not inside the measured width {}",
            last.x,
            metrics.width
        );
    }
}
