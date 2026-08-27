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

use std::borrow::Cow;
use std::collections::HashMap;
use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::Once;

use skia_rs_safe::core::Point;
use skia_rs_safe::text::{Font, Shaper, TextBlob, TextBlobBuilder, Typeface};

use crate::css::computed::ComputedStyle;
use crate::css::registry::Prop;
use crate::css::value::{
    FeatureSetting, FontFamily, FontStyle, FontWeight, GenericFamily, Keyword, Length, LengthUnit,
    LineHeight, Value, VariationSetting,
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
///
/// `features`/`variations` are stored (as `(tag, bits)` pairs, so the
/// `f32` variation value stays `Hash`/`Eq`) even though this shaper stack
/// cannot honour them: when it can, the cache must not serve a run shaped
/// without them.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct OwnedShapeKey {
    text: String,
    face: FontFace,
    size_bits: u32,
    spacing_bits: u32,
    transform: Keyword,
    features: Vec<([u8; 4], i32)>,
    variations: Vec<([u8; 4], u32)>,
}

impl OwnedShapeKey {
    fn from_key(key: &ShapeKey<'_>) -> Self {
        Self {
            text: key.text.to_owned(),
            face: key.face.clone(),
            size_bits: key.size_px.to_bits(),
            spacing_bits: key.letter_spacing_px.to_bits(),
            transform: key.transform,
            features: key.features.iter().map(|f| (f.tag, f.value)).collect(),
            variations: key
                .variations
                .iter()
                .map(|v| (v.tag, v.value.to_bits()))
                .collect(),
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
    fontconfig: Option<fontconfig::Fontconfig>,
    typefaces: HashMap<(PathBuf, i32), Option<Arc<Typeface>>>,
    faces: HashMap<String, Option<FontFace>>,
    shapes: HashMap<OwnedShapeKey, Rc<ShapedText>>,
    shaper: Shaper,
    warned_collection_index: bool,
}

impl Default for FontDatabase {
    fn default() -> Self {
        Self::new()
    }
}

impl FontDatabase {
    /// The production database: `FcInit` once for this process, falling back
    /// to [`probe_only`](Self::probe_only) when it fails.
    ///
    /// The handle is never dropped-and-recreated: the `fontconfig` crate
    /// deliberately never calls `FcFini`, so one database should live for the
    /// lifetime of the UI thread.
    #[must_use]
    pub fn new() -> Self {
        match fontconfig::Fontconfig::new() {
            Some(fc) => {
                tracing::debug!("fontconfig initialised");
                Self {
                    fontconfig: Some(fc),
                    typefaces: HashMap::new(),
                    faces: HashMap::new(),
                    shapes: HashMap::new(),
                    shaper: Shaper::new(),
                    warned_collection_index: false,
                }
            }
            None => {
                tracing::warn!("FcInit failed; falling back to the FONT_CANDIDATES probe list");
                Self::probe_only()
            }
        }
    }

    /// The fontconfig-free database: M1's fixed [`FONT_CANDIDATES`] probe.
    ///
    /// This is the CI / stripped-container fallback, and what
    /// [`FontDatabase::new`] degrades to when `FcInit` fails.
    #[must_use]
    pub fn probe_only() -> Self {
        Self {
            fontconfig: None,
            typefaces: HashMap::new(),
            faces: HashMap::new(),
            shapes: HashMap::new(),
            shaper: Shaper::new(),
            warned_collection_index: false,
        }
    }

    /// Whether real fontconfig matching is in play.
    #[must_use]
    pub fn has_fontconfig(&self) -> bool {
        self.fontconfig.is_some()
    }

    /// Resolve `query` to a face, with `fc-match` parity when fontconfig is
    /// available (a later task wires that in) and the [`FONT_CANDIDATES`]
    /// probe otherwise.
    ///
    /// Cached per query; a query that resolves to nothing is cached as
    /// nothing, so a missing font costs one lookup, not one per restyle.
    pub fn match_face(&mut self, query: &FontQuery<'_>) -> Option<FontFace> {
        let key = match_cache_key(query);
        if let Some(cached) = self.faces.get(&key) {
            return cached.clone();
        }
        let face = match self.fontconfig.as_ref() {
            // A fontconfig miss still falls through to the probe list: a
            // configured-but-empty fontconfig must not leave the UI textless.
            Some(fc) => fc_match(fc, query).or_else(|| probe_face(query)),
            None => probe_face(query),
        };
        self.faces.insert(key, face.clone());
        face
    }

    /// The loaded typeface for `face`, cached by `(path, index)`.
    ///
    /// `index` is kept even though `skia-rs-text` 0.4.0's `Typeface::from_data`
    /// cannot select a face inside a collection: it is part of the cache
    /// identity and of `fc-match` parity, so this warns once when it is
    /// non-zero rather than pretending the right face was loaded.
    pub fn typeface(&mut self, face: &FontFace) -> Option<Arc<Typeface>> {
        let key = (face.path.clone(), face.index);
        if let Some(cached) = self.typefaces.get(&key) {
            return cached.clone();
        }
        if face.index != 0 && !self.warned_collection_index {
            self.warned_collection_index = true;
            tracing::warn!(
                path = %face.path.display(),
                index = face.index,
                "skia-rs-text 0.4.0 cannot select a face index inside a font collection; \
                 loading face 0 instead"
            );
        }
        let loaded = load_typeface(&face.path);
        self.typefaces.insert(key, loaded.clone());
        loaded
    }

    /// A `Font` for `face` at `size_px`.
    pub fn font(&mut self, face: &FontFace, size_px: f32) -> Option<Font> {
        let typeface = self.typeface(face)?;
        Some(Font::new(typeface, size_px))
    }

    /// x-height ÷ em for `face`, the `ex` unit's basis.
    ///
    /// Falls back to the CSS-recommended `0.5` when the face has no usable
    /// `x_height` (`LengthCtx::ex_ratio`'s documented default), and when the
    /// face cannot be loaded at all.
    pub fn ex_ratio(&mut self, face: &FontFace) -> f32 {
        const PROBE_SIZE: f32 = 100.0;
        let Some(font) = self.font(face, PROBE_SIZE) else {
            return 0.5;
        };
        let x_height = font.metrics().x_height;
        if x_height.is_finite() && x_height > 0.0 {
            x_height / PROBE_SIZE
        } else {
            0.5
        }
    }

    /// Shape and measure a run, cached.
    ///
    /// `text-transform` is applied first (it changes which characters
    /// exist), then the shaper runs, then `letter-spacing` is added after
    /// *every* glyph, including the last -- CSS 2.1's rule, so a trailing
    /// step is part of the measured width.
    ///
    /// A face that will not load yields an empty run rather than `None`: a
    /// missing font must not take the widget tree down with it.
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

        let text = apply_text_transform(key.text, key.transform);
        let shaped = Rc::new(self.shape_uncached(&text, key));
        self.shapes.insert(owned, Rc::clone(&shaped));
        shaped
    }

    fn shape_uncached(&mut self, text: &str, key: &ShapeKey<'_>) -> ShapedText {
        // Neither `Font::new` nor the shaper is documented against
        // non-finite or non-positive sizes; hostile CSS (`font-size:
        // calc(NaN)`, a negative computed length) must not reach either.
        let size_px = if key.size_px.is_finite() && key.size_px > 0.0 {
            key.size_px
        } else {
            0.0
        };
        let Some(font) = self.font(key.face, size_px) else {
            return ShapedText {
                blob: None,
                metrics: TextMetrics {
                    width: 0.0,
                    ascent: 0.0,
                    descent: 0.0,
                    line_height: 0.0,
                },
                face: key.face.clone(),
                size_px,
            };
        };
        let font_metrics = font.metrics();
        let spacing = if key.letter_spacing_px.is_finite() {
            key.letter_spacing_px
        } else {
            0.0
        };

        let runs = if text.is_empty() {
            None
        } else {
            self.shaper.shape_auto(text, &font)
        };

        let mut builder = TextBlobBuilder::new();
        let mut pen_x = 0.0f32;
        let mut any = false;
        if let Some(runs) = runs.as_ref() {
            for run in runs {
                let mut glyphs = Vec::with_capacity(run.glyphs.len());
                let mut positions = Vec::with_capacity(run.glyphs.len());
                for glyph in &run.glyphs {
                    glyphs.push(glyph.glyph_id.0);
                    positions.push(Point::new(pen_x + glyph.x_offset, glyph.y_offset));
                    pen_x += glyph.x_advance + spacing;
                }
                if !glyphs.is_empty() {
                    builder.add_positioned_run(&run.font, &glyphs, &positions);
                    any = true;
                }
            }
        }

        // Width comes from the shaper's advances when shaping succeeded (so
        // kerning and ligatures count), and from `Font::measure_text`'s
        // per-glyph `hmtx` sum otherwise -- M1's rule, unchanged.
        let width = if any {
            pen_x
        } else if text.is_empty() {
            0.0
        } else {
            font.measure_text(text) + spacing * text.chars().count() as f32
        };

        ShapedText {
            blob: if any { builder.build() } else { None },
            metrics: TextMetrics {
                width,
                // `FontMetrics::ascent` is negative (above the baseline).
                ascent: -font_metrics.ascent,
                descent: font_metrics.descent,
                line_height: font_metrics.line_height(),
            },
            face: key.face.clone(),
            size_px,
        }
    }

    #[cfg(test)]
    fn match_cache_len(&self) -> usize {
        self.faces.len()
    }

    #[cfg(test)]
    fn shape_cache_len(&self) -> usize {
        self.shapes.len()
    }

    /// Drop every cached face, typeface and shaped run.
    pub fn clear_caches(&mut self) {
        self.typefaces.clear();
        self.faces.clear();
        self.shapes.clear();
    }
}

/// A stable string identity for a query — `FontFamily` is not `Hash` and the
/// numeric fields are floats, so [`FontDatabase::match_face`]'s cache is keyed
/// by this rather than by the query itself.
fn match_cache_key(query: &FontQuery<'_>) -> String {
    use std::fmt::Write as _;

    let mut key = String::new();
    for family in query.families {
        for name in family_names(family) {
            key.push_str(name);
            key.push('\u{1}');
        }
    }
    let _ = write!(
        key,
        "\u{2}{}\u{2}{}\u{2}{}\u{2}{}",
        query.weight.to_bits(),
        css_style_to_fc_slant(query.style),
        query.stretch.to_bits(),
        query.size_px.to_bits()
    );
    key
}

/// The fontconfig family strings for one CSS family, in the order they should
/// be offered. `system-ui` is offered with `sans-serif` behind it because not
/// every fontconfig configuration aliases it.
fn family_names(family: &FontFamily) -> Vec<&str> {
    match family {
        FontFamily::Named(name) => vec![name.as_ref()],
        FontFamily::Generic(GenericFamily::Serif) => vec!["serif"],
        FontFamily::Generic(GenericFamily::SansSerif) => vec!["sans-serif"],
        FontFamily::Generic(GenericFamily::Monospace) => vec!["monospace"],
        FontFamily::Generic(GenericFamily::Cursive) => vec!["cursive"],
        FontFamily::Generic(GenericFamily::Fantasy) => vec!["fantasy"],
        FontFamily::Generic(GenericFamily::SystemUi) => vec!["system-ui", "sans-serif"],
    }
}

/// One `FcPattern` carrying the whole family list in priority order plus
/// `FC_WEIGHT`/`FC_SLANT`/`FC_WIDTH`/`FC_PIXEL_SIZE`, then `FcFontMatch` — the
/// same path `fc-match` takes, so aliases, generic families and user rules in
/// `~/.config/fontconfig` are honoured without any special-casing here.
fn fc_match(fc: &fontconfig::Fontconfig, query: &FontQuery<'_>) -> Option<FontFace> {
    let mut pattern = fontconfig::Pattern::new(fc).ok()?;
    for family in query.families {
        for name in family_names(family) {
            // A family with an embedded NUL cannot reach fontconfig; skip that
            // one name rather than abandoning the whole query.
            match CString::new(name) {
                Ok(value) => {
                    if let Err(error) = pattern.add_string(fontconfig::FC_FAMILY, &value) {
                        tracing::debug!(family = name, ?error, "FcPatternAddString failed");
                    }
                }
                Err(_) => tracing::debug!(family = name, "font family contains a NUL byte"),
            }
        }
    }
    let _ = pattern.add_integer(fontconfig::FC_WEIGHT, css_weight_to_fc(query.weight));
    let _ = pattern.add_integer(fontconfig::FC_SLANT, css_style_to_fc_slant(query.style));
    let _ = pattern.add_integer(fontconfig::FC_WIDTH, css_stretch_to_fc(query.stretch));
    if query.size_px.is_finite() && query.size_px > 0.0 {
        let _ = pattern.add_integer(
            fontconfig::FC_PIXEL_SIZE,
            query.size_px.round().clamp(1.0, 4096.0) as i32,
        );
    }

    // `font_match` runs FcConfigSubstitute + FcDefaultSubstitute itself, and
    // the pattern it returns borrows this one -- copy everything out here.
    let matched = match pattern.font_match() {
        Ok(matched) => matched,
        Err(error) => {
            tracing::debug!(?error, "FcFontMatch found nothing");
            return None;
        }
    };
    let path = PathBuf::from(matched.filename().ok()?);
    let index = matched.face_index().unwrap_or(0);
    let family = matched
        .get_string(fontconfig::FC_FAMILY)
        .or_else(|_| matched.name())
        .map(str::to_owned)
        .unwrap_or_default();
    Some(FontFace {
        path,
        index,
        family,
    })
}

/// The no-fontconfig path: a named family that is itself a readable font file
/// wins (so tests can name a file directly), then M1's fixed
/// [`FONT_CANDIDATES`] list.
fn probe_face(query: &FontQuery<'_>) -> Option<FontFace> {
    for family in query.families {
        if let FontFamily::Named(name) = family {
            let path = Path::new(name.as_ref());
            if path.is_file()
                && let Some(face) = face_from_file(path)
            {
                return Some(face);
            }
        }
    }
    for candidate in FONT_CANDIDATES {
        if let Some(face) = face_from_file(Path::new(candidate)) {
            tracing::debug!(font = candidate, "probed UI typeface");
            return Some(face);
        }
    }
    tracing::warn!(
        candidates = FONT_CANDIDATES.len(),
        "no UI typeface found in the probe list"
    );
    None
}

/// Read and parse `path` just far enough to report its family name; the
/// [`Typeface`] itself is not kept here -- [`FontDatabase::typeface`] owns
/// that cache.
fn face_from_file(path: &Path) -> Option<FontFace> {
    let typeface = load_typeface(path)?;
    Some(FontFace {
        path: path.to_path_buf(),
        index: 0,
        family: typeface.family_name().to_string(),
    })
}

/// Read and parse one font file into a [`Typeface`], with no caching.
fn load_typeface(path: &Path) -> Option<Arc<Typeface>> {
    let data = std::fs::read(path).ok()?;
    Typeface::from_data(data).map(Arc::new)
}

/// Small kana → their full-size form, for `text-transform: full-size-kana`.
///
/// The hiragana and katakana small letters GTK's own `full-size-kana` covers;
/// the half-width katakana block is left alone (it is a *width* transform, not
/// a size one, and `full-width` handles it).
const FULL_SIZE_KANA: &[(char, char)] = &[
    ('ぁ', 'あ'),
    ('ぃ', 'い'),
    ('ぅ', 'う'),
    ('ぇ', 'え'),
    ('ぉ', 'お'),
    ('っ', 'つ'),
    ('ゃ', 'や'),
    ('ゅ', 'ゆ'),
    ('ょ', 'よ'),
    ('ゎ', 'わ'),
    ('ゕ', 'か'),
    ('ゖ', 'け'),
    ('ァ', 'ア'),
    ('ィ', 'イ'),
    ('ゥ', 'ウ'),
    ('ェ', 'エ'),
    ('ォ', 'オ'),
    ('ッ', 'ツ'),
    ('ャ', 'ヤ'),
    ('ュ', 'ユ'),
    ('ョ', 'ヨ'),
    ('ヮ', 'ワ'),
    ('ヵ', 'カ'),
    ('ヶ', 'ケ'),
];

/// Apply CSS `text-transform` to `text`.
///
/// This runs *before* shaping (as it does in GTK): the transform changes which
/// characters exist, so the shaper must see the transformed string, and the
/// result is what [`FontDatabase::shape`] caches.
///
/// A keyword that is not a `text-transform` value — including
/// [`Keyword::None`] — borrows the input unchanged.
#[must_use]
pub fn apply_text_transform(text: &str, transform: Keyword) -> Cow<'_, str> {
    match transform {
        Keyword::Uppercase => Cow::Owned(text.to_uppercase()),
        Keyword::Lowercase => Cow::Owned(text.to_lowercase()),
        Keyword::Capitalize => Cow::Owned(capitalize(text)),
        Keyword::FullWidth => Cow::Owned(text.chars().map(to_full_width).collect()),
        Keyword::FullSizeKana => Cow::Owned(text.chars().map(to_full_size_kana).collect()),
        _ => Cow::Borrowed(text),
    }
}

/// Upper-case the first letter of every whitespace-delimited word.
fn capitalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at_word_start = true;
    for ch in text.chars() {
        if ch.is_whitespace() {
            at_word_start = true;
            out.push(ch);
        } else if at_word_start {
            at_word_start = false;
            out.extend(ch.to_uppercase());
        } else {
            out.push(ch);
        }
    }
    out
}

/// ASCII → the Halfwidth and Fullwidth Forms block; space → ideographic space.
fn to_full_width(ch: char) -> char {
    match ch {
        ' ' => '\u{3000}',
        '!'..='~' => char::from_u32(ch as u32 + 0xFEE0).unwrap_or(ch),
        _ => ch,
    }
}

fn to_full_size_kana(ch: char) -> char {
    FULL_SIZE_KANA
        .iter()
        .find_map(|&(small, full)| (small == ch).then_some(full))
        .unwrap_or(ch)
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

/// The computed text properties of one element, in the shape the font stack
/// consumes them.
///
/// Read once per restyle: [`TextStyle::query`] resolves the face and
/// [`TextStyle::shape_key`] shapes a label with it.
#[derive(Clone, Debug, PartialEq)]
pub struct TextStyle {
    /// `font-family`, in priority order.
    pub families: Rc<[FontFamily]>,
    /// `font-weight`, computed to an absolute 1..=1000.
    pub weight: f32,
    /// `font-style`.
    pub style: FontStyle,
    /// `font-width`, falling back to `font-stretch`, as a percentage.
    pub stretch: f32,
    /// `font-size`, in device pixels.
    pub size_px: f32,
    /// `letter-spacing`, in device pixels (`normal` is 0).
    pub letter_spacing_px: f32,
    /// `font-feature-settings`.
    pub features: Rc<[FeatureSetting]>,
    /// `font-variation-settings`.
    pub variations: Rc<[VariationSetting]>,
    /// `text-transform`.
    pub transform: Keyword,
    /// `line-height`, unresolved (it needs the face's metrics — see
    /// [`TextStyle::line_height_px`]).
    pub line_height: LineHeight,
}

impl TextStyle {
    /// Read the text properties out of a computed style.
    ///
    /// Values are read raw and matched on their variant rather than coerced,
    /// so a property that somehow holds the wrong shape falls back to its
    /// initial value instead of to a coerced nonsense number.
    #[must_use]
    pub fn from_computed(style: &ComputedStyle) -> TextStyle {
        let families = match style.raw(Prop::FontFamily) {
            Value::FontFamilies(list) => Rc::clone(list),
            _ => Rc::from(vec![FontFamily::Generic(GenericFamily::SansSerif)]),
        };
        let weight = match style.raw(Prop::FontWeight) {
            Value::FontWeight(FontWeight::Absolute(w)) if w.is_finite() => w.clamp(1.0, 1000.0),
            _ => 400.0,
        };
        let font_style = match style.raw(Prop::FontStyle) {
            Value::FontStyle(s) => *s,
            _ => FontStyle::Normal,
        };
        // GTK 4.22 registers `font-width` and `font-stretch` as two rows with
        // one grammar; the L4 name wins where it is set.
        let width = stretch_percent(style.raw(Prop::FontWidth));
        let stretch = if (width - 100.0).abs() < f32::EPSILON {
            stretch_percent(style.raw(Prop::FontStretch))
        } else {
            width
        };
        let features = match style.raw(Prop::FontFeatureSettings) {
            Value::FontFeatures(list) => Rc::clone(list),
            _ => Rc::from(Vec::<FeatureSetting>::new()),
        };
        let variations = match style.raw(Prop::FontVariationSettings) {
            Value::FontVariations(list) => Rc::clone(list),
            _ => Rc::from(Vec::<VariationSetting>::new()),
        };
        TextStyle {
            families,
            weight,
            style: font_style,
            stretch,
            size_px: style.font_size_px(),
            letter_spacing_px: computed_px(style.raw(Prop::LetterSpacing)),
            features,
            variations,
            transform: match style.raw(Prop::TextTransform) {
                Value::Keyword(keyword) => *keyword,
                _ => Keyword::None,
            },
            line_height: match style.raw(Prop::LineHeight) {
                Value::LineHeight(line_height) => line_height.clone(),
                _ => LineHeight::Normal,
            },
        }
    }

    /// The font query for these properties.
    #[must_use]
    pub fn query(&self) -> FontQuery<'_> {
        FontQuery {
            families: &self.families,
            weight: self.weight,
            style: self.style,
            stretch: self.stretch,
            size_px: self.size_px,
        }
    }

    /// The shaping key for `text` on `face`.
    #[must_use]
    pub fn shape_key<'a>(&'a self, text: &'a str, face: &'a FontFace) -> ShapeKey<'a> {
        ShapeKey {
            text,
            face,
            size_px: self.size_px,
            letter_spacing_px: self.letter_spacing_px,
            features: &self.features,
            variations: &self.variations,
            transform: self.transform,
        }
    }

    /// `line-height` in device pixels. `normal` is the face's own line height;
    /// a number multiplies the computed `font-size`.
    #[must_use]
    pub fn line_height_px(&self, metrics: &TextMetrics) -> f32 {
        match self.line_height {
            LineHeight::Normal => metrics.line_height,
            LineHeight::Number(factor) if factor.is_finite() && factor >= 0.0 => {
                factor * self.size_px
            }
            LineHeight::Number(_) => metrics.line_height,
            LineHeight::Length(ref length) => absolute_px(length).unwrap_or(metrics.line_height),
        }
    }
}

/// A computed length as device pixels. Computed values are already resolved to
/// `px` (the contract's resolution order), so anything else is a bug upstream
/// and reads as 0 rather than as a guess.
fn computed_px(value: &Value) -> f32 {
    match value {
        Value::Length(length) => absolute_px(length).unwrap_or(0.0),
        Value::Number(number) if number.is_finite() => *number,
        _ => 0.0,
    }
}

fn absolute_px(length: &Length) -> Option<f32> {
    match length {
        Length::Abs {
            value,
            unit: LengthUnit::Px,
        } if value.is_finite() => Some(*value),
        _ => None,
    }
}

/// A computed `font-width`/`font-stretch` as a percentage. `Value::Percentage`
/// is a fraction (1.0 == 100%), per the value contract.
fn stretch_percent(value: &Value) -> f32 {
    match value {
        Value::Percentage(fraction) if fraction.is_finite() => fraction * 100.0,
        Value::Number(number) if number.is_finite() => *number,
        Value::Keyword(keyword) => match keyword {
            Keyword::UltraCondensed => 50.0,
            Keyword::ExtraCondensed => 62.5,
            Keyword::Condensed => 75.0,
            Keyword::SemiCondensed => 87.5,
            Keyword::SemiExpanded => 112.5,
            Keyword::Expanded => 125.0,
            Keyword::ExtraExpanded => 150.0,
            Keyword::UltraExpanded => 200.0,
            _ => 100.0,
        },
        _ => 100.0,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        FontDatabase, FontFace, FontQuery, ShapeKey, ShapedText, TextMetrics, TextStyle,
        css_stretch_to_fc, css_style_to_fc_slant, css_weight_to_fc,
    };
    use crate::css::cascade::CompiledSheet;
    use crate::css::computed::{ComputedStyle, ResolveEnv};
    use crate::css::node::Node;
    use crate::css::select::MatchCx;
    use crate::css::value::{FontFamily, FontStyle, GenericFamily, Keyword, LineHeight};
    use std::rc::Rc;

    fn style_for(css: &str) -> ComputedStyle {
        let sheet = CompiledSheet::compile(css);
        let node = Node::new("label");
        let mut cx = MatchCx::new();
        ComputedStyle::resolve_chain(&sheet, &node, &ResolveEnv::default(), &mut cx)
    }

    #[test]
    fn the_text_style_reads_every_font_property_off_the_computed_style() {
        let style = style_for(
            "label { font-family: \"Adwaita Sans\", sans-serif; font-size: 20px; \
             font-weight: bold; font-style: italic; font-width: 75%; \
             letter-spacing: 2px; text-transform: uppercase; line-height: 1.5; }",
        );
        let text = TextStyle::from_computed(&style);
        assert_eq!(
            text.families.as_ref(),
            &[
                FontFamily::Named("Adwaita Sans".into()),
                FontFamily::Generic(GenericFamily::SansSerif),
            ]
        );
        assert_eq!(text.size_px, 20.0);
        assert_eq!(text.weight, 700.0);
        assert_eq!(text.style, FontStyle::Italic);
        assert_eq!(text.stretch, 75.0);
        assert_eq!(text.letter_spacing_px, 2.0);
        assert_eq!(text.transform, Keyword::Uppercase);
        assert_eq!(text.line_height, LineHeight::Number(1.5));
    }

    #[test]
    fn an_unstyled_node_gets_the_registrys_initial_font() {
        let text = TextStyle::from_computed(&style_for("other { color: red; }"));
        assert_eq!(
            text.families.as_ref(),
            &[FontFamily::Generic(GenericFamily::SansSerif)]
        );
        assert_eq!(text.weight, 400.0);
        assert_eq!(text.style, FontStyle::Normal);
        assert_eq!(text.stretch, 100.0);
        assert_eq!(text.letter_spacing_px, 0.0);
        assert_eq!(text.transform, Keyword::None);
        assert_eq!(text.line_height, LineHeight::Normal);
        assert!(text.features.is_empty() && text.variations.is_empty());
    }

    #[test]
    fn font_stretch_is_read_only_when_font_width_is_normal() {
        // GTK 4.22 registers both rows; font-width wins where it is set.
        let both = TextStyle::from_computed(&style_for(
            "label { font-width: 125%; font-stretch: condensed; }",
        ));
        assert_eq!(both.stretch, 125.0, "font-width must win over font-stretch");

        let stretch_only =
            TextStyle::from_computed(&style_for("label { font-stretch: condensed; }"));
        assert_eq!(stretch_only.stretch, 75.0, "the condensed keyword is 75%");
    }

    #[test]
    fn the_text_style_hands_the_database_a_query_and_a_key() {
        let style = style_for("label { font-size: 18px; text-transform: lowercase; }");
        let text = TextStyle::from_computed(&style);
        let mut db = FontDatabase::probe_only();
        let face = db.match_face(&text.query()).expect("system font");
        let run = db.shape(&text.shape_key("ABC", &face));
        assert_eq!(run.size_px, 18.0);
        assert!(run.blob.is_some());

        let untransformed = db.shape(&text.shape_key("abc", &face));
        assert!(
            (run.metrics.width - untransformed.metrics.width).abs() < 0.01,
            "text-transform: lowercase did not reach the shaper through shape_key"
        );
    }

    #[test]
    fn line_height_resolves_against_the_faces_metrics() {
        let metrics = TextMetrics {
            width: 0.0,
            ascent: 12.0,
            descent: 4.0,
            line_height: 18.0,
        };
        let normal = TextStyle::from_computed(&style_for("label { font-size: 20px; }"));
        assert_eq!(
            normal.line_height_px(&metrics),
            18.0,
            "`normal` is the face's own"
        );

        let numeric =
            TextStyle::from_computed(&style_for("label { font-size: 20px; line-height: 1.5; }"));
        assert_eq!(numeric.line_height_px(&metrics), 30.0);

        let absolute =
            TextStyle::from_computed(&style_for("label { font-size: 20px; line-height: 26px; }"));
        assert_eq!(absolute.line_height_px(&metrics), 26.0);
    }

    fn db() -> FontDatabase {
        FontDatabase::probe_only()
    }

    fn sans() -> Vec<FontFamily> {
        vec![FontFamily::Generic(GenericFamily::SansSerif)]
    }

    fn query(families: &[FontFamily]) -> FontQuery<'_> {
        FontQuery {
            families,
            weight: 400.0,
            style: FontStyle::Normal,
            stretch: 100.0,
            size_px: 14.0,
        }
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

    fn key<'a>(text: &'a str, face: &'a FontFace, size_px: f32) -> ShapeKey<'a> {
        ShapeKey {
            text,
            face,
            size_px,
            letter_spacing_px: 0.0,
            features: &[],
            variations: &[],
            transform: Keyword::None,
        }
    }

    fn shaped(db: &mut FontDatabase, text: &str, size_px: f32) -> Rc<ShapedText> {
        let families = sans();
        let face = db.match_face(&query(&families)).expect("system font");
        db.shape(&key(text, &face, size_px))
    }

    #[test]
    fn measurement_scales_with_size_and_length() {
        let mut db = db();
        let small = shaped(&mut db, "Click me", 14.0);
        let large = shaped(&mut db, "Click me", 28.0);
        assert!(
            small.metrics.width > 0.0,
            "zero-width measurement means no hmtx data reached us"
        );
        assert!(
            large.metrics.width > small.metrics.width * 1.8,
            "28px measured {} vs 14px {}: advances are not scaling with size",
            large.metrics.width,
            small.metrics.width
        );
        let longer = shaped(&mut db, "Click me twice", 14.0);
        assert!(longer.metrics.width > small.metrics.width);

        assert!(small.metrics.ascent > 0.0 && small.metrics.descent > 0.0);
        assert!(small.metrics.line_height >= small.metrics.ascent + small.metrics.descent);
    }

    #[test]
    fn shaping_produces_one_positioned_glyph_per_character() {
        let mut db = db();
        let run = shaped(&mut db, "Click me", 14.0);
        let blob = run.blob.as_ref().expect("shaping produced no runs");
        let glyphs: usize = blob.runs().iter().map(|r| r.glyphs.len()).sum();
        assert_eq!(
            glyphs,
            "Click me".chars().count(),
            "Latin text with no ligatures should shape 1:1"
        );
        for run in blob.runs() {
            assert_eq!(run.glyphs.len(), run.positions.len());
        }
        let first = &blob.runs()[0];
        for pair in first.positions.windows(2) {
            assert!(pair[1].x > pair[0].x, "glyph positions did not advance");
        }
    }

    #[test]
    fn shaping_returns_the_blob_the_metrics_and_the_face_together() {
        let mut db = db();
        let families = sans();
        let face = db.match_face(&query(&families)).expect("system font");
        let run = db.shape(&key("Click me", &face, 14.0));
        assert!(run.blob.is_some());
        assert_eq!(run.face, face);
        assert_eq!(run.size_px, 14.0);

        let empty = db.shape(&key("", &face, 14.0));
        assert!(empty.blob.is_none(), "empty text must not build a blob");
        assert_eq!(empty.metrics.width, 0.0);
        assert!(
            empty.metrics.line_height > 0.0,
            "an empty label still occupies a line"
        );
    }

    #[test]
    fn the_last_glyph_origin_sits_inside_the_measured_width() {
        let mut db = db();
        let run = shaped(&mut db, "Click me", 14.0);
        let blob = run.blob.as_ref().expect("blob");
        let last = blob.runs()[0].positions.last().copied().expect("positions");
        assert!(
            last.x < run.metrics.width && last.x > run.metrics.width * 0.5,
            "last glyph origin {} is not inside the measured width {}",
            last.x,
            run.metrics.width
        );
    }

    #[test]
    fn letter_spacing_widens_the_run_by_one_step_per_glyph() {
        let mut db = db();
        let families = sans();
        let face = db.match_face(&query(&families)).expect("system font");
        let tight = db.shape(&key("Click me", &face, 14.0));
        let mut spaced_key = key("Click me", &face, 14.0);
        spaced_key.letter_spacing_px = 3.0;
        let spaced = db.shape(&spaced_key);

        let glyphs = "Click me".chars().count() as f32;
        assert!(
            (spaced.metrics.width - (tight.metrics.width + 3.0 * glyphs)).abs() < 0.01,
            "letter-spacing 3px over {glyphs} glyphs widened {} to {}",
            tight.metrics.width,
            spaced.metrics.width
        );

        let tight_positions = &tight.blob.as_ref().expect("blob").runs()[0].positions;
        let spaced_positions = &spaced.blob.as_ref().expect("blob").runs()[0].positions;
        // The nth glyph has n spacing steps in front of it; the first has none.
        assert!((spaced_positions[0].x - tight_positions[0].x).abs() < 0.01);
        assert!((spaced_positions[3].x - (tight_positions[3].x + 9.0)).abs() < 0.01);
    }

    #[test]
    fn a_transform_changes_the_glyphs_and_the_cache_key() {
        let mut db = db();
        let families = sans();
        let face = db.match_face(&query(&families)).expect("system font");
        let plain = db.shape(&key("click", &face, 14.0));
        let mut upper_key = key("click", &face, 14.0);
        upper_key.transform = Keyword::Uppercase;
        let upper = db.shape(&upper_key);
        assert!(
            upper.metrics.width > plain.metrics.width,
            "CLICK ({}) is not wider than click ({}) — was the transform applied?",
            upper.metrics.width,
            plain.metrics.width
        );
        assert_eq!(
            db.shape_cache_len(),
            2,
            "the transform is not part of the cache key"
        );
    }

    #[test]
    fn identical_keys_are_served_from_the_shaping_cache() {
        let mut db = db();
        let families = sans();
        let face = db.match_face(&query(&families)).expect("system font");
        let first = db.shape(&key("Click me", &face, 14.0));
        let second = db.shape(&key("Click me", &face, 14.0));
        assert!(Rc::ptr_eq(&first, &second), "the same key reshaped");
        assert_eq!(db.shape_cache_len(), 1);

        db.clear_caches();
        let third = db.shape(&key("Click me", &face, 14.0));
        assert!(
            !Rc::ptr_eq(&first, &third),
            "clear_caches did not drop the shaping cache"
        );
    }

    #[test]
    fn shaping_never_panics_on_hostile_text() {
        let mut db = db();
        let families = sans();
        let face = db.match_face(&query(&families)).expect("system font");
        for text in [
            "",
            " ",
            "\u{0}",
            "\u{200b}\u{feff}",
            "مرحبا بالعالم",
            "🇯🇵👩‍👩‍👧‍👦",
            "e\u{301}\u{301}",
            "\u{10FFFF}",
        ] {
            for size in [0.0_f32, -12.0, 1.0, 4096.0, f32::NAN] {
                let mut hostile = key(text, &face, size);
                hostile.letter_spacing_px = if size.is_nan() { f32::INFINITY } else { -100.0 };
                let _ = db.shape(&hostile);
            }
        }
        let long = "Click me ".repeat(2_000);
        let _ = db.shape(&key(&long, &face, 14.0));
    }

    #[test]
    fn a_probe_only_database_reports_no_fontconfig() {
        assert!(!FontDatabase::probe_only().has_fontconfig());
    }

    #[test]
    fn the_probe_only_database_resolves_a_face_without_fontconfig() {
        // Mutation check: making `has_fontconfig` return `true` unconditionally
        // passes the first assertion regardless of `probe_only`'s body.
        let mut db = db();
        assert!(
            !db.has_fontconfig(),
            "probe_only must not initialise fontconfig"
        );
        let families = sans();
        let face = db
            .match_face(&FontQuery {
                families: &families,
                weight: 400.0,
                style: FontStyle::Normal,
                stretch: 100.0,
                size_px: 14.0,
            })
            .expect(
                "no font found among FONT_CANDIDATES; install adwaita-fonts/dejavu/liberation/noto",
            );
        assert!(
            face.path.is_file(),
            "{} is not a readable file",
            face.path.display()
        );
        assert!(
            !face.family.is_empty(),
            "the probed face reported no family name"
        );
        assert_eq!(face.index, 0, "the probe list only ever loads face 0");
    }

    #[test]
    fn a_typeface_is_loaded_once_per_face_and_cached() {
        // Mutation check: skipping the cache insert in `typeface` fails the
        // `Arc::ptr_eq` assertion because a fresh allocation is loaded each call.
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
            .expect("system font");
        let first = db.typeface(&face).expect("typeface");
        let second = db.typeface(&face).expect("typeface");
        assert!(
            std::sync::Arc::ptr_eq(&first, &second),
            "the typeface cache handed out two different allocations for one face"
        );
        assert_eq!(first.family_name(), face.family);

        db.clear_caches();
        let third = db.typeface(&face).expect("typeface");
        assert!(
            !std::sync::Arc::ptr_eq(&first, &third),
            "clear_caches did not drop the typeface cache"
        );
    }

    #[test]
    fn a_font_carries_the_requested_size_and_an_x_height_ratio() {
        // Mutation check: returning `x_height / PROBE_SIZE` unconditionally in
        // `ex_ratio` (dropping the finite/positive guard) still passes this
        // test on a real face, but fails
        // `an_unreadable_face_resolves_to_nothing_rather_than_panicking` below.
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
            .expect("system font");
        let font = db.font(&face, 28.0).expect("font");
        let metrics = font.metrics();
        assert!(
            metrics.line_height() > 14.0,
            "28px face reported a {}px line height",
            metrics.line_height()
        );
        let ratio = db.ex_ratio(&face);
        assert!(
            (0.2..=0.9).contains(&ratio),
            "x-height/em ratio {ratio} is outside every real UI face's range"
        );
    }

    #[test]
    fn an_unreadable_face_resolves_to_nothing_rather_than_panicking() {
        // Mutation check: making `ex_ratio` unconditionally compute
        // `x_height / PROBE_SIZE` panics here on the `None` font instead of
        // returning the documented 0.5 fallback.
        let mut db = db();
        let missing = FontFace {
            path: std::path::PathBuf::from("/nonexistent/font/does-not-exist.ttf"),
            index: 0,
            family: String::from("Nothing"),
        };
        assert!(db.typeface(&missing).is_none());
        assert!(db.font(&missing, 14.0).is_none());
        assert_eq!(
            db.ex_ratio(&missing),
            0.5,
            "the documented ex-ratio fallback"
        );
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

    use std::process::Command;

    /// `fc-match`'s answer for a pattern, or `None` when fc-match is absent.
    fn fc_match_file(pattern: &str) -> Option<String> {
        let output = Command::new("fc-match")
            .arg("--format=%{file}")
            .arg(pattern)
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let file = String::from_utf8(output.stdout).ok()?;
        (!file.trim().is_empty()).then(|| file.trim().to_string())
    }

    #[test]
    fn a_generic_family_resolves_to_something() {
        let mut db = FontDatabase::new();
        let families = sans();
        let face = db
            .match_face(&query(&families))
            .expect("sans-serif resolved to no face at all");
        // Deliberately NOT a fixed family: which face `sans-serif` means is the
        // machine's business (spec section 6).
        assert!(
            face.path.is_file(),
            "{} is not readable",
            face.path.display()
        );
        assert!(!face.family.is_empty());
    }

    #[test]
    fn the_query_matches_what_fc_match_would_pick() {
        let mut db = FontDatabase::new();
        if !db.has_fontconfig() {
            eprintln!("fontconfig unavailable; parity check skipped");
            return;
        }
        let Some(expected) = fc_match_file("sans-serif:weight=200:slant=0:width=100:pixelsize=14")
        else {
            eprintln!("fc-match unavailable; parity check skipped");
            return;
        };
        let families = [FontFamily::Generic(GenericFamily::SansSerif)];
        let bold = FontQuery {
            families: &families,
            weight: 700.0,
            style: FontStyle::Normal,
            stretch: 100.0,
            size_px: 14.0,
        };
        let face = db.match_face(&bold).expect("bold sans-serif");
        assert_eq!(
            face.path.to_string_lossy(),
            expected,
            "our FcPattern disagrees with fc-match for bold sans-serif"
        );
    }

    #[test]
    fn a_named_family_is_offered_before_the_generic_fallback() {
        let mut db = FontDatabase::new();
        if !db.has_fontconfig() {
            eprintln!("fontconfig unavailable; family-order check skipped");
            return;
        }
        let Some(expected) = fc_match_file("monospace") else {
            eprintln!("fc-match unavailable; family-order check skipped");
            return;
        };
        let families = [
            FontFamily::Named("Definitely Not An Installed Family".into()),
            FontFamily::Generic(GenericFamily::Monospace),
        ];
        let face = db.match_face(&query(&families)).expect("monospace");
        assert_eq!(
            face.path.to_string_lossy(),
            expected,
            "an unknown first family must fall through to the next one, not abort the match"
        );
    }

    #[test]
    fn matches_are_cached_per_query() {
        let mut db = FontDatabase::new();
        let families = sans();
        let first = db.match_face(&query(&families));
        let second = db.match_face(&query(&families));
        assert_eq!(first, second, "two identical queries disagreed");

        let mut heavier = query(&families);
        heavier.weight = 900.0;
        // A different query must not be served from the first one's slot; it
        // may legitimately resolve to the same file on a one-weight system, so
        // assert on the cache size, not on the face.
        let _ = db.match_face(&heavier);
        assert_eq!(
            db.match_cache_len(),
            2,
            "the weight is not part of the cache key"
        );
    }

    use super::apply_text_transform;

    #[test]
    fn text_transform_none_borrows_the_input_unchanged() {
        let out = apply_text_transform("Click me", Keyword::None);
        assert_eq!(out, "Click me");
        assert!(
            matches!(out, std::borrow::Cow::Borrowed(_)),
            "`none` must not allocate"
        );
        // Any keyword that is not a text-transform value is also a no-op.
        assert_eq!(apply_text_transform("Click me", Keyword::Solid), "Click me");
    }

    #[test]
    fn text_transform_cases_follow_unicode_not_ascii() {
        assert_eq!(
            apply_text_transform("straße", Keyword::Uppercase),
            "STRASSE"
        );
        assert_eq!(
            apply_text_transform("ÅNGSTRÖM", Keyword::Lowercase),
            "ångström"
        );
        assert_eq!(
            apply_text_transform("ábc déf", Keyword::Capitalize),
            "Ábc Déf"
        );
    }

    #[test]
    fn capitalize_starts_a_word_after_any_whitespace_or_punctuation_run() {
        assert_eq!(
            apply_text_transform("click me now", Keyword::Capitalize),
            "Click Me Now"
        );
        assert_eq!(
            apply_text_transform("  leading", Keyword::Capitalize),
            "  Leading"
        );
        assert_eq!(
            apply_text_transform("multi\tword\nlines", Keyword::Capitalize),
            "Multi\tWord\nLines"
        );
        assert_eq!(
            apply_text_transform("ALREADY UP", Keyword::Capitalize),
            "ALREADY UP",
            "capitalize only touches the first letter of each word"
        );
    }

    #[test]
    fn full_width_maps_ascii_into_the_fullwidth_block() {
        assert_eq!(apply_text_transform("AB1!", Keyword::FullWidth), "ＡＢ１！");
        assert_eq!(apply_text_transform(" ", Keyword::FullWidth), "\u{3000}");
        assert_eq!(
            apply_text_transform("あ", Keyword::FullWidth),
            "あ",
            "a character that is already full width is left alone"
        );
    }

    #[test]
    fn full_size_kana_promotes_the_small_kana() {
        assert_eq!(
            apply_text_transform("ぁぃっゃ", Keyword::FullSizeKana),
            "あいつや"
        );
        assert_eq!(
            apply_text_transform("ァィッャ", Keyword::FullSizeKana),
            "アイツヤ"
        );
        assert_eq!(
            apply_text_transform("あア", Keyword::FullSizeKana),
            "あア",
            "full-size kana are already full size"
        );
    }

    #[test]
    fn text_transform_never_panics_on_hostile_text() {
        let hostile = [
            "",
            "\u{0}",
            "\u{7}\u{1b}\u{7f}",
            "\u{200b}\u{200e}\u{feff}",
            "e\u{301}\u{301}\u{301}",
            "🇯🇵👩‍👩‍👧‍👦",
            "\u{10FFFF}",
            "ﬁﬂﬀ",
        ];
        for transform in [
            Keyword::None,
            Keyword::Capitalize,
            Keyword::Uppercase,
            Keyword::Lowercase,
            Keyword::FullWidth,
            Keyword::FullSizeKana,
        ] {
            for text in hostile {
                let _ = apply_text_transform(text, transform);
            }
            let long = "aあ ".repeat(5_000);
            let _ = apply_text_transform(&long, transform);
        }
    }
}
