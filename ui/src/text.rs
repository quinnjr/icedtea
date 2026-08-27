//! The text stack: `skia-rs-text` for shaping and rasterisation, with font
//! *discovery* behind [`FontDatabase`].
//!
//! M2 Part 4 lands the database's API shape with M1's fixed probe list
//! behind it; Part 6 replaces `match_face`'s body with a real
//! `FcPattern`/`FcFontMatch` call. Every signature here is final either way.
//!
//! `font-feature-settings` and `font-variation-settings` reach [`ShapeKey`]
//! and stop there: `skia-rs-text` 0.4.0's `Shaper::shape` calls
//! `rustybuzz::shape(&face, &[], buffer)` with a hardcoded empty feature
//! slice and exposes no variation axes, so they are dropped at the shaper
//! with a one-time warning rather than silently ignored.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::Once;

use skia_rs_safe::core::Point;
use skia_rs_safe::text::{Font, Shaper, TextBlob, TextBlobBuilder, Typeface};

use crate::css::value::{
    FeatureSetting, FontFamily, FontStyle, GenericFamily, Keyword, VariationSetting,
};

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

/// One concrete font file, as fontconfig would name it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FontFace {
    /// Absolute path to the font file.
    pub path: PathBuf,
    /// Face index within a collection; 0 for a single-face file.
    pub index: i32,
    /// The family name the face reports.
    pub family: String,
}

/// What the cascade asks the database for.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct FontQuery<'a> {
    /// `font-family`, in priority order.
    pub families: &'a [FontFamily],
    /// CSS `font-weight`, 1..1000.
    pub weight: f32,
    /// CSS `font-style`.
    pub style: FontStyle,
    /// CSS `font-stretch` as a percentage; 100.0 is normal.
    pub stretch: f32,
    /// Used `font-size` in px.
    pub size_px: f32,
}

/// Everything that changes a shaped run.
#[derive(Clone, Debug, PartialEq)]
pub struct ShapeKey<'a> {
    /// The string, before `text-transform`.
    pub text: &'a str,
    /// The resolved face.
    pub face: &'a FontFace,
    /// Used `font-size` in px.
    pub size_px: f32,
    /// Used `letter-spacing` in px.
    pub letter_spacing_px: f32,
    /// `font-feature-settings` (dropped at the shaper -- see the module docs).
    pub features: &'a [FeatureSetting],
    /// `font-variation-settings` (dropped at the shaper).
    pub variations: &'a [VariationSetting],
    /// `text-transform`.
    pub transform: Keyword,
}

/// The owned form of a [`ShapeKey`], for the cache map.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct OwnedShapeKey {
    text: String,
    face: FontFace,
    size_bits: u32,
    spacing_bits: u32,
    transform: Keyword,
}

impl OwnedShapeKey {
    fn from_key(key: &ShapeKey<'_>) -> Self {
        Self {
            text: key.text.to_owned(),
            face: key.face.clone(),
            size_bits: key.size_px.to_bits(),
            spacing_bits: key.letter_spacing_px.to_bits(),
            transform: key.transform,
        }
    }
}

/// A shaped run: its blob (empty text shapes to `None`) and its extents.
pub struct ShapedText {
    /// The positioned glyph run, relative to the baseline origin `(0, 0)`.
    pub blob: Option<TextBlob>,
    /// The measured extents that layout sizes the run against.
    pub metrics: TextMetrics,
    /// The face the run was shaped with.
    pub face: FontFace,
    /// The size the run was shaped at.
    pub size_px: f32,
}

static FEATURES_WARNED: Once = Once::new();

/// Font discovery, loading and shaping, with caches.
///
/// Single-threaded by construction: fontconfig's own objects are `!Send`
/// and `!Sync`, and Part 6 will own one `Fontconfig` for the process.
pub struct FontDatabase {
    fontconfig: bool,
    typefaces: HashMap<(PathBuf, i32), Arc<Typeface>>,
    faces: HashMap<String, Option<FontFace>>,
    shapes: HashMap<OwnedShapeKey, Rc<ShapedText>>,
    shaper: Shaper,
}

impl Default for FontDatabase {
    fn default() -> Self {
        Self::new()
    }
}

impl FontDatabase {
    /// The production database. Part 6 initialises fontconfig here and falls
    /// back to [`probe_only`](Self::probe_only) when `FcInit` fails.
    #[must_use]
    pub fn new() -> Self {
        Self::probe_only()
    }

    /// M1's fixed `FONT_CANDIDATES` path probe -- the no-fontconfig fallback.
    #[must_use]
    pub fn probe_only() -> Self {
        Self {
            fontconfig: false,
            typefaces: HashMap::new(),
            faces: HashMap::new(),
            shapes: HashMap::new(),
            shaper: Shaper::new(),
        }
    }

    /// Whether real fontconfig matching is in play.
    #[must_use]
    pub fn has_fontconfig(&self) -> bool {
        self.fontconfig
    }

    /// Resolve a CSS font query to one face.
    ///
    /// The probe-only body ignores weight, style and stretch: it returns the
    /// first readable, parseable entry of [`FONT_CANDIDATES`]. Part 6
    /// replaces this body with `FcPattern` + `FcFontMatch`.
    pub fn match_face(&mut self, query: &FontQuery<'_>) -> Option<FontFace> {
        let cache_key = Self::query_cache_key(query);
        if let Some(cached) = self.faces.get(&cache_key) {
            return cached.clone();
        }
        let mut found = None;
        for candidate in FONT_CANDIDATES {
            let path = PathBuf::from(candidate);
            if let Some(typeface) = Self::load(&path) {
                found = Some(FontFace {
                    family: typeface.family_name().to_owned(),
                    path: path.clone(),
                    index: 0,
                });
                self.typefaces.insert((path, 0), typeface);
                break;
            }
        }
        if found.is_none() {
            tracing::warn!(candidates = FONT_CANDIDATES.len(), "no UI typeface found");
        }
        self.faces.insert(cache_key, found.clone());
        found
    }

    /// A stable cache key for a query.
    fn query_cache_key(query: &FontQuery<'_>) -> String {
        use std::fmt::Write as _;

        let mut key = String::new();
        for family in query.families {
            match family {
                FontFamily::Named(name) => key.push_str(name),
                FontFamily::Generic(generic) => key.push_str(match generic {
                    GenericFamily::Serif => "serif",
                    GenericFamily::SansSerif => "sans-serif",
                    GenericFamily::Monospace => "monospace",
                    GenericFamily::Cursive => "cursive",
                    GenericFamily::Fantasy => "fantasy",
                    GenericFamily::SystemUi => "system-ui",
                }),
            }
            key.push(',');
        }
        let _ = write!(
            key,
            "|{}|{}|{}",
            css_weight_to_fc(query.weight),
            css_style_to_fc_slant(query.style),
            css_stretch_to_fc(query.stretch)
        );
        key
    }

    fn load(path: &Path) -> Option<Arc<Typeface>> {
        let data = std::fs::read(path).ok()?;
        Typeface::from_data(data).map(Arc::new)
    }

    /// The loaded typeface for `face`, cached by `(path, index)`.
    pub fn typeface(&mut self, face: &FontFace) -> Option<Arc<Typeface>> {
        let key = (face.path.clone(), face.index);
        if let Some(typeface) = self.typefaces.get(&key) {
            return Some(Arc::clone(typeface));
        }
        let typeface = Self::load(&face.path)?;
        self.typefaces.insert(key, Arc::clone(&typeface));
        Some(typeface)
    }

    /// A `Font` for `face` at `size_px`.
    pub fn font(&mut self, face: &FontFace, size_px: f32) -> Option<Font> {
        let typeface = self.typeface(face)?;
        Some(Font::new(typeface, size_px))
    }

    /// Shape and measure a run, cached.
    ///
    /// `text-transform` is applied to the string *before* shaping;
    /// `letter-spacing` is added between glyphs (never after the last one),
    /// through `TextBlobBuilder::add_positioned_run`.
    pub fn shape(&mut self, key: &ShapeKey<'_>) -> Rc<ShapedText> {
        if !key.features.is_empty() || !key.variations.is_empty() {
            FEATURES_WARNED.call_once(|| {
                tracing::warn!(
                    "font-feature-settings and font-variation-settings are computed but cannot \
                     reach rustybuzz through skia-rs-text 0.4.0; they are dropped at the shaper"
                );
            });
        }

        let owned = OwnedShapeKey::from_key(key);
        if let Some(cached) = self.shapes.get(&owned) {
            return Rc::clone(cached);
        }

        let text = transform_text(key.text, key.transform);
        let shaped = Rc::new(self.shape_uncached(&text, key));
        self.shapes.insert(owned, Rc::clone(&shaped));
        shaped
    }

    fn shape_uncached(&mut self, text: &str, key: &ShapeKey<'_>) -> ShapedText {
        let Some(font) = self.font(key.face, key.size_px) else {
            return ShapedText {
                blob: None,
                metrics: TextMetrics {
                    width: 0.0,
                    ascent: 0.0,
                    descent: 0.0,
                    line_height: 0.0,
                },
                face: key.face.clone(),
                size_px: key.size_px,
            };
        };
        let font_metrics = font.metrics();
        let spacing = if key.letter_spacing_px.is_finite() {
            key.letter_spacing_px
        } else {
            0.0
        };

        let mut builder = TextBlobBuilder::new();
        let mut pen_x = 0.0f32;
        let mut glyph_count = 0usize;
        let mut any = false;
        if let Some(runs) = self.shaper.shape_auto(text, &font) {
            for run in &runs {
                let mut glyphs = Vec::with_capacity(run.glyphs.len());
                let mut positions = Vec::with_capacity(run.glyphs.len());
                for glyph in &run.glyphs {
                    if glyph_count > 0 {
                        pen_x += spacing;
                    }
                    glyphs.push(glyph.glyph_id.0);
                    positions.push(Point::new(pen_x + glyph.x_offset, glyph.y_offset));
                    pen_x += glyph.x_advance;
                    glyph_count += 1;
                }
                if !glyphs.is_empty() {
                    builder.add_positioned_run(&run.font, &glyphs, &positions);
                    any = true;
                }
            }
        } else {
            pen_x = font.measure_text(text);
        }

        ShapedText {
            blob: if any { builder.build() } else { None },
            metrics: TextMetrics {
                width: pen_x,
                // `FontMetrics::ascent` is negative (above the baseline).
                ascent: -font_metrics.ascent,
                descent: font_metrics.descent,
                line_height: font_metrics.line_height(),
            },
            face: key.face.clone(),
            size_px: key.size_px,
        }
    }

    /// Drop every cached face, typeface and shaped run.
    pub fn clear_caches(&mut self) {
        self.typefaces.clear();
        self.faces.clear();
        self.shapes.clear();
    }
}

/// Apply CSS `text-transform` to a string.
///
/// `full-width` maps the ASCII range onto its fullwidth forms (U+FF01..FF5E,
/// and U+3000 for the space), which is what GTK does.
fn transform_text(text: &str, transform: Keyword) -> String {
    match transform {
        Keyword::Uppercase => text.to_uppercase(),
        Keyword::Lowercase => text.to_lowercase(),
        Keyword::Capitalize => {
            let mut out = String::with_capacity(text.len());
            let mut at_word_start = true;
            for ch in text.chars() {
                if at_word_start {
                    out.extend(ch.to_uppercase());
                } else {
                    out.push(ch);
                }
                at_word_start = ch.is_whitespace();
            }
            out
        }
        Keyword::FullWidth => text
            .chars()
            .map(|ch| match ch {
                ' ' => '\u{3000}',
                '!'..='~' => char::from_u32(ch as u32 - 0x21 + 0xFF01).unwrap_or(ch),
                other => other,
            })
            .collect(),
        _ => text.to_owned(),
    }
}

/// CSS `font-weight` (1..1000) → fontconfig `FC_WEIGHT`.
///
/// This is a table lookup with piecewise-linear interpolation, **not** a cast:
/// the two scales are not proportional (fontconfig's own
/// `FcWeightFromOpenType` table). `fc-match "DejaVu Sans:weight=200"` picks the
/// Bold face, which is CSS 700 — passing 700 straight through would ask for a
/// weight past extra-black.
const CSS_TO_FC_WEIGHT: &[(f32, f32)] = &[
    (100.0, 0.0),
    (200.0, 40.0),
    (300.0, 50.0),
    (350.0, 55.0),
    (380.0, 75.0),
    (400.0, 80.0),
    (500.0, 100.0),
    (600.0, 180.0),
    (700.0, 200.0),
    (800.0, 205.0),
    (900.0, 210.0),
    (1000.0, 215.0),
];

/// CSS `font-width`/`font-stretch` percentage → fontconfig `FC_WIDTH`.
const CSS_TO_FC_WIDTH: &[(f32, f32)] = &[
    (50.0, 50.0),
    (62.5, 63.0),
    (75.0, 75.0),
    (87.5, 87.0),
    (100.0, 100.0),
    (112.5, 113.0),
    (125.0, 125.0),
    (150.0, 150.0),
    (200.0, 200.0),
];

/// Piecewise-linear lookup over an ascending `(input, output)` table. Values
/// outside the table clamp to its ends rather than extrapolating.
fn interpolate_table(table: &[(f32, f32)], x: f32) -> f32 {
    let first = table[0];
    let last = table[table.len() - 1];
    if x <= first.0 {
        return first.1;
    }
    if x >= last.0 {
        return last.1;
    }
    for pair in table.windows(2) {
        let (x0, y0) = pair[0];
        let (x1, y1) = pair[1];
        if x <= x1 {
            let span = x1 - x0;
            if span <= 0.0 {
                return y1;
            }
            return y0 + (y1 - y0) * ((x - x0) / span);
        }
    }
    last.1
}

/// CSS `font-weight` → `FC_WEIGHT`. Non-finite input resolves to regular (80).
#[must_use]
pub fn css_weight_to_fc(w: f32) -> i32 {
    if !w.is_finite() {
        return 80;
    }
    interpolate_table(CSS_TO_FC_WEIGHT, w)
        .round()
        .clamp(0.0, 215.0) as i32
}

/// CSS `font-width`/`font-stretch` percentage → `FC_WIDTH`. Non-finite input
/// resolves to normal (100).
#[must_use]
pub fn css_stretch_to_fc(pct: f32) -> i32 {
    if !pct.is_finite() {
        return 100;
    }
    interpolate_table(CSS_TO_FC_WIDTH, pct)
        .round()
        .clamp(1.0, 400.0) as i32
}

/// CSS `font-style` → `FC_SLANT` (`ROMAN` 0, `ITALIC` 100, `OBLIQUE` 110).
///
/// `oblique 0deg` is upright text, so it maps to `ROMAN`; every other angle,
/// including a non-finite one, is oblique.
#[must_use]
pub fn css_style_to_fc_slant(s: FontStyle) -> i32 {
    match s {
        FontStyle::Normal => 0,
        FontStyle::Italic => 100,
        FontStyle::Oblique(degrees) if degrees.is_finite() && degrees.abs() < 0.5 => 0,
        FontStyle::Oblique(_) => 110,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        FontDatabase, FontFace, FontQuery, ShapeKey, css_stretch_to_fc, css_style_to_fc_slant,
        css_weight_to_fc,
    };
    use crate::css::value::{FontFamily, FontStyle, GenericFamily, Keyword};

    fn db() -> FontDatabase {
        FontDatabase::probe_only()
    }

    fn sans() -> Vec<FontFamily> {
        vec![FontFamily::Generic(GenericFamily::SansSerif)]
    }

    #[test]
    fn a_sans_serif_query_resolves_to_something() {
        // Deliberately NOT a fixed family: CI may have any UI face installed
        // (spec §6). Mutation check: returning None unconditionally fails.
        let mut db = db();
        let families = sans();
        let face = db
            .match_face(&FontQuery {
                families: &families,
                weight: 400.0,
                style: FontStyle::Normal,
                stretch: 100.0,
                size_px: 14.0,
            })
            .expect("no system font found; install dejavu/liberation/noto sans");
        assert!(face.path.exists(), "the matched face must be a real file");
        assert!(!face.family.is_empty());
    }

    #[test]
    fn measurement_scales_with_size_and_length() {
        // M1's assertion, preserved through the new API.
        let mut db = db();
        let families = sans();
        let query = FontQuery {
            families: &families,
            weight: 400.0,
            style: FontStyle::Normal,
            stretch: 100.0,
            size_px: 14.0,
        };
        let face = db.match_face(&query).expect("face");
        let small = db.shape(&ShapeKey {
            text: "Click me",
            face: &face,
            size_px: 14.0,
            letter_spacing_px: 0.0,
            features: &[],
            variations: &[],
            transform: Keyword::None,
        });
        let big = db.shape(&ShapeKey {
            text: "Click me",
            face: &face,
            size_px: 28.0,
            letter_spacing_px: 0.0,
            features: &[],
            variations: &[],
            transform: Keyword::None,
        });
        assert!(big.metrics.width > small.metrics.width * 1.8);
        assert!(small.blob.is_some());
    }

    #[test]
    fn letter_spacing_widens_the_run_by_one_gap_per_glyph() {
        // Mutation check: applying spacing after the last glyph too (or not
        // at all) changes this width by exactly one or eight gaps.
        let mut db = db();
        let families = sans();
        let face = db
            .match_face(&FontQuery {
                families: &families,
                weight: 400.0,
                style: FontStyle::Normal,
                stretch: 100.0,
                size_px: 14.0,
            })
            .expect("face");
        let base = ShapeKey {
            text: "Click me",
            face: &face,
            size_px: 14.0,
            letter_spacing_px: 0.0,
            features: &[],
            variations: &[],
            transform: Keyword::None,
        };
        let plain = db.shape(&base);
        let spaced = db.shape(&ShapeKey {
            letter_spacing_px: 2.0,
            ..base
        });
        let glyphs = "Click me".chars().count() as f32;
        let delta = spaced.metrics.width - plain.metrics.width;
        assert!(
            (delta - 2.0 * (glyphs - 1.0)).abs() < 0.01,
            "letter-spacing added {delta}px across {glyphs} glyphs"
        );
    }

    #[test]
    fn text_transform_is_applied_before_shaping() {
        // Mutation check: leaving the string untouched makes the two widths
        // equal for a lowercase-only label.
        let mut db = db();
        let families = sans();
        let face = db
            .match_face(&FontQuery {
                families: &families,
                weight: 400.0,
                style: FontStyle::Normal,
                stretch: 100.0,
                size_px: 14.0,
            })
            .expect("face");
        let key = ShapeKey {
            text: "iii",
            face: &face,
            size_px: 20.0,
            letter_spacing_px: 0.0,
            features: &[],
            variations: &[],
            transform: Keyword::None,
        };
        let lower = db.shape(&key);
        let upper = db.shape(&ShapeKey {
            transform: Keyword::Uppercase,
            ..key
        });
        assert!(
            upper.metrics.width > lower.metrics.width,
            "III is wider than iii"
        );
    }

    #[test]
    fn an_empty_string_has_no_blob_but_a_positive_line_height() {
        // M1's assertion, preserved.
        let mut db = db();
        let families = sans();
        let face = db
            .match_face(&FontQuery {
                families: &families,
                weight: 400.0,
                style: FontStyle::Normal,
                stretch: 100.0,
                size_px: 14.0,
            })
            .expect("face");
        let shaped = db.shape(&ShapeKey {
            text: "",
            face: &face,
            size_px: 14.0,
            letter_spacing_px: 0.0,
            features: &[],
            variations: &[],
            transform: Keyword::None,
        });
        assert!(shaped.blob.is_none());
        assert!(shaped.metrics.line_height > 0.0);
        assert_eq!(shaped.metrics.width, 0.0);
    }

    #[test]
    fn shaping_the_same_key_twice_returns_the_cached_handle() {
        // Mutation check: dropping the cache makes the two Rc's differ, and
        // the widget would reshape on every paint (M1's perf regression).
        let mut db = db();
        let families = sans();
        let face = db
            .match_face(&FontQuery {
                families: &families,
                weight: 400.0,
                style: FontStyle::Normal,
                stretch: 100.0,
                size_px: 14.0,
            })
            .expect("face");
        let key = ShapeKey {
            text: "Click me",
            face: &face,
            size_px: 14.0,
            letter_spacing_px: 0.0,
            features: &[],
            variations: &[],
            transform: Keyword::None,
        };
        let a = db.shape(&key);
        let b = db.shape(&key);
        assert!(std::rc::Rc::ptr_eq(&a, &b));
        db.clear_caches();
        let c = db.shape(&key);
        assert!(!std::rc::Rc::ptr_eq(&a, &c));
    }

    #[test]
    fn the_css_and_fontconfig_scales_are_a_table_lookup_not_a_cast() {
        // research/taffy-fontconfig.md §2.4: the two scales are NOT
        // proportional. Mutation check: `w as i32 / 4` passes none of these.
        assert_eq!(css_weight_to_fc(100.0), 0);
        assert_eq!(css_weight_to_fc(400.0), 80);
        assert_eq!(css_weight_to_fc(600.0), 180);
        assert_eq!(css_weight_to_fc(700.0), 200);
        assert_eq!(css_weight_to_fc(900.0), 210);
        assert_eq!(
            css_weight_to_fc(450.0),
            90,
            "interpolated between 400 and 500"
        );

        assert_eq!(css_stretch_to_fc(50.0), 50);
        assert_eq!(css_stretch_to_fc(100.0), 100);
        assert_eq!(css_stretch_to_fc(200.0), 200);
        assert_eq!(css_stretch_to_fc(75.0), 75);

        assert_eq!(css_style_to_fc_slant(FontStyle::Normal), 0);
        assert_eq!(css_style_to_fc_slant(FontStyle::Italic), 100);
        assert_eq!(css_style_to_fc_slant(FontStyle::Oblique(14.0)), 110);
    }

    /// The `ShapeKey` M1's three restored shaping tests all shape.
    fn click_me(face: &FontFace) -> ShapeKey<'_> {
        ShapeKey {
            text: "Click me",
            face,
            size_px: 14.0,
            letter_spacing_px: 0.0,
            features: &[],
            variations: &[],
            transform: Keyword::None,
        }
    }

    #[test]
    fn shaping_produces_one_positioned_glyph_per_character() {
        // M1's assertion, restored onto `FontDatabase`/`ShapedText`
        // (contract §10.3 pins src/text.rs at "same properties asserted").
        // Mutation check: dropping the `pen_x += glyph.x_advance` in
        // `shape_uncached` stops the positions advancing; pushing a glyph id
        // without its position breaks the two-vector length equality.
        let mut db = db();
        let families = sans();
        let face = db
            .match_face(&FontQuery {
                families: &families,
                weight: 400.0,
                style: FontStyle::Normal,
                stretch: 100.0,
                size_px: 14.0,
            })
            .expect("face");
        let shaped = db.shape(&click_me(&face));
        let blob = shaped.blob.as_ref().expect("shaping produced no runs");
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
    fn shaping_returns_the_blob_and_the_metrics_together() {
        // M1's assertion, restored. M1 compared `shape()`'s metrics against a
        // separate `measure()` call; `FontDatabase` has one entry point, so
        // the same invariant is that one `ShapedText` carries a blob *and*
        // usable metrics, and that the cached second call carries both too.
        // Mutation check: returning `blob: None` on the cache hit, or
        // building the metrics from a separate unshaped measure, fails here.
        let mut db = db();
        let families = sans();
        let face = db
            .match_face(&FontQuery {
                families: &families,
                weight: 400.0,
                style: FontStyle::Normal,
                stretch: 100.0,
                size_px: 14.0,
            })
            .expect("face");
        let shaped = db.shape(&click_me(&face));
        assert!(shaped.blob.is_some());
        assert!(shaped.metrics.width > 0.0);
        assert!(shaped.metrics.ascent > 0.0 && shaped.metrics.descent > 0.0);

        let again = db.shape(&click_me(&face));
        assert!(again.blob.is_some());
        assert_eq!(again.metrics, shaped.metrics);
    }

    #[test]
    fn blob_width_agrees_with_measure() {
        // M1's assertion, restored: the last glyph's origin sits inside the
        // measured width, and past its half -- the blob and the metrics come
        // from the same pen walk. Mutation check: measuring with
        // `font.measure_text` while positioning from the shaper (M1's bug)
        // drifts these apart on any face whose advances differ.
        let mut db = db();
        let families = sans();
        let face = db
            .match_face(&FontQuery {
                families: &families,
                weight: 400.0,
                style: FontStyle::Normal,
                stretch: 100.0,
                size_px: 14.0,
            })
            .expect("face");
        let shaped = db.shape(&click_me(&face));
        let blob = shaped.blob.as_ref().expect("blob");
        let last = blob.runs()[0].positions.last().copied().expect("positions");
        assert!(
            last.x < shaped.metrics.width && last.x > shaped.metrics.width * 0.5,
            "last glyph origin {} is not inside the measured width {}",
            last.x,
            shaped.metrics.width
        );
    }

    #[test]
    fn a_probe_only_database_reports_no_fontconfig() {
        assert!(!FontDatabase::probe_only().has_fontconfig());
    }

    #[test]
    fn css_weights_map_onto_fontconfigs_own_scale() {
        // fontconfig's FcWeightFromOpenType table, the mapping `fc-match` itself
        // uses. Verified on this machine: `fc-match "DejaVu Sans:weight=200"`
        // resolves to DejaVu Sans Bold, `weight=80` to Book.
        assert_eq!(css_weight_to_fc(100.0), 0, "thin");
        assert_eq!(css_weight_to_fc(200.0), 40, "extra-light");
        assert_eq!(css_weight_to_fc(300.0), 50, "light");
        assert_eq!(css_weight_to_fc(350.0), 55, "semi-light");
        assert_eq!(css_weight_to_fc(380.0), 75, "book");
        assert_eq!(css_weight_to_fc(400.0), 80, "regular");
        assert_eq!(css_weight_to_fc(500.0), 100, "medium");
        assert_eq!(css_weight_to_fc(600.0), 180, "semi-bold");
        assert_eq!(css_weight_to_fc(700.0), 200, "bold");
        assert_eq!(css_weight_to_fc(800.0), 205, "extra-bold");
        assert_eq!(css_weight_to_fc(900.0), 210, "black");
        assert_eq!(css_weight_to_fc(1000.0), 215, "extra-black");
    }

    #[test]
    fn css_weights_between_table_rows_interpolate_and_clamp() {
        // 450 is halfway between 400 (fc 80) and 500 (fc 100).
        assert_eq!(css_weight_to_fc(450.0), 90);
        // 650 is halfway between 600 (fc 180) and 700 (fc 200).
        assert_eq!(css_weight_to_fc(650.0), 190);
        // Outside the table the ends hold, they do not extrapolate.
        assert_eq!(css_weight_to_fc(0.0), 0);
        assert_eq!(css_weight_to_fc(-500.0), 0);
        assert_eq!(css_weight_to_fc(5000.0), 215);
    }

    #[test]
    fn css_stretch_percentages_map_onto_fc_width() {
        assert_eq!(css_stretch_to_fc(50.0), 50, "ultra-condensed");
        assert_eq!(css_stretch_to_fc(62.5), 63, "extra-condensed");
        assert_eq!(css_stretch_to_fc(75.0), 75, "condensed");
        assert_eq!(css_stretch_to_fc(87.5), 87, "semi-condensed");
        assert_eq!(css_stretch_to_fc(100.0), 100, "normal");
        assert_eq!(css_stretch_to_fc(112.5), 113, "semi-expanded");
        assert_eq!(css_stretch_to_fc(125.0), 125, "expanded");
        assert_eq!(css_stretch_to_fc(150.0), 150, "extra-expanded");
        assert_eq!(css_stretch_to_fc(200.0), 200, "ultra-expanded");
        // The two scales are not proportional: 68.75% sits between
        // extra-condensed (63) and condensed (75), i.e. 69, not 68.
        assert_eq!(css_stretch_to_fc(68.75), 69);
        assert_eq!(
            css_stretch_to_fc(10.0),
            50,
            "clamped at the ultra-condensed end"
        );
        assert_eq!(
            css_stretch_to_fc(900.0),
            200,
            "clamped at the ultra-expanded end"
        );
    }

    #[test]
    fn css_styles_map_onto_fc_slant() {
        assert_eq!(css_style_to_fc_slant(FontStyle::Normal), 0);
        assert_eq!(css_style_to_fc_slant(FontStyle::Italic), 100);
        assert_eq!(css_style_to_fc_slant(FontStyle::Oblique(14.0)), 110);
        assert_eq!(css_style_to_fc_slant(FontStyle::Oblique(-14.0)), 110);
        // `oblique 0deg` is upright, per CSS Fonts 4.
        assert_eq!(css_style_to_fc_slant(FontStyle::Oblique(0.0)), 0);
    }

    #[test]
    fn the_scale_conversions_never_panic_on_hostile_numbers() {
        for value in [
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::MAX,
            f32::MIN,
            f32::MIN_POSITIVE,
            -0.0,
            0.0,
            1e30,
            -1e30,
        ] {
            let _ = css_weight_to_fc(value);
            let _ = css_stretch_to_fc(value);
            let _ = css_style_to_fc_slant(FontStyle::Oblique(value));
        }
        // A non-finite input must land on the neutral row, never on 0 or a
        // saturated cast.
        assert_eq!(css_weight_to_fc(f32::NAN), 80);
        assert_eq!(css_stretch_to_fc(f32::NAN), 100);
        assert_eq!(css_style_to_fc_slant(FontStyle::Oblique(f32::NAN)), 110);
    }
}
