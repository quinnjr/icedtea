# M2 Part 0 — Shared Interface Contract

**Date:** 2026-08-26
**Spec:** `docs/superpowers/specs/2026-08-26-pure-rust-gtk-m2-css-engine-design.md`
**Parent spec:** `docs/superpowers/specs/2026-08-20-pure-rust-gtk-ui-design.md`
**Branch:** `rebuild/pure-rust-gtk-m2` · **Crate:** `ui/` (`icedtea-ui`)
**Research notes:** `.superpowers/m2-plan-notes/{current-crate,gtk-css-semantics,cssparser-selectors,skia-rs,taffy-fontconfig}.md`

This is the frozen interface six independently-authored part-plans code against.
Every signature below is normative: a part may add private items freely, but may
not change a signature here without a contract amendment. One-line semantics
follow each item. Nothing outside `css::registry` names a property string.

**Crate deps to add (P1 owns the Cargo.toml edit, all parts depend on it):**
`fontconfig = "0.11"`, `bitflags = "2"`, and `skia-rs-safe` features
`["std", "text", "codec", "codec-png", "svg"]` (M1 has only `std`,`text`;
`codec`/`svg` are required by `Image::Url` decoding — research/skia-rs.md §9).

---

## 0. Module map (owner in brackets)

```
ui/src/css/
  registry.rs   [P1]  Prop, PropertyDef, PROPERTIES, lookup
  tokens.rs     [P1]  M1's value.rs token helpers, moved verbatim
  value/        [P1]  mod.rs length.rs calc.rs color.rs image.rs shadow.rs border.rs
                      outline.rs font.rs text.rs transform.rs filter.rs timing.rs
                      keyframes.rs keyword.rs interpolate.rs
  parse.rs      [P1]  + @keyframes, @media, MediaEnv, MediaQuery
  node.rs       [P2]  Node tree (replaces CssNode)
  select.rs     [P2]  complete Element impl, rule buckets, bloom
  cascade.rs    [P3]  registry-driven cascade, CascadedValues keyed by Prop
  computed.rs   [P3]  ComputedStyle registry table, inheritance, unit resolution
ui/src/anim/    [P5]  mod.rs clock.rs transition.rs keyframes.rs
ui/src/layout.rs[P4]  LayoutTree over Nodes
ui/src/paint/   [P4]  mod.rs background.rs border.rs shadow.rs outline.rs text.rs effects.rs
ui/src/text.rs  [P6]  FontDatabase (fontconfig), FontFace, ShapedText, caches
ui/src/widget/button.rs [P3] Button as Node behaviour (M1 gate)
```

`ui/src/css/colors.rs` is **deleted**; its grammar moves into `value/color.rs`.
`ui/src/css/shorthand.rs` is **deleted**; expansion moves into registry `ExpandFn`s.
`ui/src/css/value.rs` is **renamed** to `css/tokens.rs` (bodies unchanged).

---

## 1. Registry — `css/registry.rs` [P1]

### 1.1 `Prop`

Discriminants are **longhands first** (indices `0..N_LONGHANDS`), then
shorthands (`N_LONGHANDS..N_PROPS`), each group in the order of the GTK 4.22
reference table (research/gtk-css-semantics.md §1). `prop as usize` is both the
`PROPERTIES` index and, for longhands, the `ComputedStyle` slot.

```rust
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum Prop {
    // ---- longhands 0..94 ----
    // colours & effects
    Color = 0, Opacity, Filter,
    // fonts
    FontFamily, FontSize, FontStyle, FontVariant, FontWeight, FontWidth,
    FontStretch, FontKerning, FontVariantLigatures, FontVariantPosition,
    FontVariantCaps, FontVariantNumeric, FontVariantAlternates,
    FontVariantEastAsian, FontFeatureSettings, FontVariationSettings, GtkDpi,
    // text
    CaretColor, GtkSecondaryCaretColor, LetterSpacing, TextTransform, LineHeight,
    TextDecorationLine, TextDecorationColor, TextDecorationStyle, TextShadow,
    // icons (parsed + stored; drawn in M4)
    GtkIconSource, GtkIconSize, GtkIconStyle, GtkIconTransform, GtkIconPalette,
    GtkIconShadow, GtkIconFilter, GtkIconWeight,
    // transform
    Transform, TransformOrigin,
    // box model
    MinWidth, MinHeight,
    MarginTop, MarginRight, MarginBottom, MarginLeft,
    PaddingTop, PaddingRight, PaddingBottom, PaddingLeft,
    // borders
    BorderTopWidth, BorderRightWidth, BorderBottomWidth, BorderLeftWidth,
    BorderTopStyle, BorderRightStyle, BorderBottomStyle, BorderLeftStyle,
    BorderTopLeftRadius, BorderTopRightRadius,
    BorderBottomRightRadius, BorderBottomLeftRadius,
    BorderTopColor, BorderRightColor, BorderBottomColor, BorderLeftColor,
    BorderImageSource, BorderImageRepeat, BorderImageSlice, BorderImageWidth,
    // outline
    OutlineStyle, OutlineWidth, OutlineColor, OutlineOffset,
    // backgrounds
    BackgroundColor, BackgroundClip, BackgroundOrigin, BackgroundSize,
    BackgroundPosition, BackgroundRepeat, BackgroundImage, BoxShadow,
    BackgroundBlendMode,
    // transitions
    TransitionProperty, TransitionDuration, TransitionTimingFunction,
    TransitionDelay,
    // animations
    AnimationName, AnimationDuration, AnimationTimingFunction,
    AnimationIterationCount, AnimationDirection, AnimationPlayState,
    AnimationDelay, AnimationFillMode,
    // misc
    BorderSpacing,
    // ---- shorthands 95..112 ----
    Font, TextDecoration, Margin, Padding,
    BorderWidth, BorderStyle, BorderColor,
    BorderTop, BorderRight, BorderBottom, BorderLeft,
    Border, BorderRadius, BorderImage,
    Outline, Background, Transition, Animation,
}

pub const N_LONGHANDS: usize = 95;
pub const N_PROPS: usize = 113;
```

`FontWidth` and `FontStretch` are two distinct rows with identical grammar and
initial value (GTK 4.22 registers both); they are **not** aliased — both are
cascaded, and `computed` reads `FontWidth` first, falling back to `FontStretch`.

### 1.2 `PropertyDef`

```rust
/// Whole-value parser. Consumes the entire input; a trailing token is an error.
/// Never panics on any token stream. Returns `Err(())` for a declaration that
/// is invalid at parse time (the declaration is then dropped, CSS-style).
pub type ParseFn = fn(&mut cssparser::Parser<'_, '_>) -> Result<Value, ()>;

/// Expands a shorthand, emitting one `(longhand, Value)` per longhand it sets.
/// MUST emit every longhand in `PropertyKind::Shorthand::longhands` (omitted
/// components are emitted at their initial value — CSS shorthand reset rule).
pub type ExpandFn =
    fn(&mut cssparser::Parser<'_, '_>, &mut dyn FnMut(Prop, Value)) -> Result<(), ()>;

/// Interpolate two *computed* values at progress `t` (usually 0..=1, may
/// overshoot for cubic-bezier). Both inputs are values of the same `Prop`.
pub type Interpolate = fn(&Value, &Value, f32) -> Value;

pub enum PropertyKind {
    Longhand {
        parse: ParseFn,
        initial: fn() -> Value,
        inherited: bool,
        animatable: Option<Interpolate>,
    },
    Shorthand { expand: ExpandFn, longhands: &'static [Prop] },
}

pub struct PropertyDef { pub name: &'static str, pub kind: PropertyKind }

/// Indexed by `prop as usize`. Row order == `Prop` discriminant order.
pub static PROPERTIES: [PropertyDef; N_PROPS];
```

### 1.3 Lookup

```rust
/// ASCII-case-insensitive; `None` for an unknown property name.
pub fn lookup(name: &str) -> Option<Prop>;
/// All longhands, in registry order — the `ComputedStyle` slot order.
pub fn longhands() -> impl Iterator<Item = Prop>;
/// Every animatable longhand — the expansion of `transition-property: all`.
pub fn animatable_longhands() -> impl Iterator<Item = Prop>;

impl Prop {
    pub fn def(self) -> &'static PropertyDef;
    pub fn name(self) -> &'static str;
    pub fn is_longhand(self) -> bool;                 // (self as usize) < N_LONGHANDS
    pub fn is_inherited(self) -> bool;                // false for shorthands
    pub fn initial(self) -> Value;                    // panics for shorthands
    pub fn interpolator(self) -> Option<Interpolate>;
    pub fn slot(self) -> usize;                       // == self as usize; longhands only
    pub fn expand_into(self, input: &mut cssparser::Parser<'_, '_>,
                       sink: &mut dyn FnMut(Prop, Value)) -> Result<(), ()>;
    /// Longhands this shorthand sets, in reset order; empty for a longhand.
    pub fn longhands(self) -> &'static [Prop];
}
```

**Initial values** are the research table's column, with these settled rulings:
`min-width`/`min-height` → `Length(0px)` (not `auto`; M1 content-box rule);
`border-*-width` `medium` → `Length(3px)`; `outline-color` → `CurrentColor`
(GTK, not `invert`); `outline-style` → `none` (no `auto`); `color` →
`Rgba(0,0,0,1)`; `font-size` → `Length(14px)` (`DEFAULT_FONT_SIZE`, unchanged
from M1); `font-family` → `[Generic(SansSerif)]`; `-gtk-dpi` → `Number(96.0)`;
`-gtk-icon-size`/`-gtk-icon-style`/`-gtk-icon-weight`/
`-gtk-secondary-caret-color` (NOT AVAILABLE upstream) → `Length(16px)`,
`Keyword(Requested)`, `Number(400.0)`, `Color(CurrentColor)` — each with a
`// NOT AVAILABLE from GTK docs; chosen fallback` comment at the row.

---

## 2. Values — `css/value/` [P1]

### 2.1 `Value`

```rust
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Wide(Wide),                                   // inherit | initial | unset
    Keyword(Keyword),                             // every discrete keyword value
    Number(f32),
    Percentage(f32),                              // 0..1 fraction, not 0..100
    Length(Length),
    Angle(f32),                                   // always degrees
    Time(Time),
    Color(ColorValue),
    Image(Image),
    Shadow(Shadow),
    Transform(Rc<[TransformFn]>),                 // `none` == empty slice
    Filter(Rc<[FilterFn]>),
    Position(Position),                           // background-position, transform-origin
    BgSize(BgSize),
    Repeat(RepeatStyle),
    FontFamilies(Rc<[FontFamily]>),
    FontWeight(FontWeight),
    FontStyle(FontStyle),
    FontVariant(FontVariantFlags),
    FontFeatures(Rc<[FeatureSetting]>),
    FontVariations(Rc<[VariationSetting]>),
    LineHeight(LineHeight),
    TextDecorationLines(TextDecorationLines),
    Timing(TimingFunction),
    AnimationName(AnimationName),
    IterationCount(IterationCount),
    Slice(BorderImageSlice),
    BorderImageWidths([BorderImageWidthSide; 4]),
    IconPalette(Rc<[(Rc<str>, ColorValue)]>),
    Pair(Rc<(Value, Value)>),                     // radii (h,v), border-spacing
    List(Rc<[Value]>),                            // every comma-multiplied (`#`) property
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Wide { Inherit, Initial, Unset }

impl Value {
    /// Parses `inherit|initial|unset` as a whole value. Every `ParseFn` calls
    /// this first via `parse_wide_or(input, f)`.
    pub fn parse_wide(input: &mut cssparser::Parser<'_, '_>) -> Result<Value, ()>;
    pub fn parse_wide_or(input: &mut cssparser::Parser<'_, '_>, f: ParseFn)
        -> Result<Value, ()>;
    pub fn is_wide(&self) -> bool;
    /// Comma-list helper: `parse_list(input, f)` -> `Value::List`.
    pub fn parse_list(input: &mut cssparser::Parser<'_, '_>, f: ParseFn)
        -> Result<Value, ()>;
}
```

**Parser conventions (binding on every `parse` in `value/`):**
1. Signature is exactly `fn(&mut cssparser::Parser<'_,'_>) -> Result<Value, ()>`.
2. Whole-value: caller wraps in `input.parse_entirely(..)`; trailing tokens fail.
3. ASCII-case-insensitive idents/functions; whitespace-insensitive.
4. Never panics on any input (fuzz-ish test battery per family is mandatory).
5. No colour-table, node, or font access at parse time — `@name`, `currentColor`,
   `em`/`rem`/`%` all survive into the parsed `Value` unresolved.
6. `Err(())` means *invalid at parse time* → declaration dropped and logged at
   `tracing::debug!` with property + serialized value.

### 2.2 Keywords

```rust
/// One flat enum for every discrete keyword value in the registry, so that
/// discrete properties share one `Value` variant and one interpolator.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[repr(u16)]
pub enum Keyword {
    // universal
    None, Auto, Normal,
    // border/outline style
    Hidden, Dotted, Dashed, Solid, Double, Groove, Ridge, Inset, Outset,
    // background boxes / repeat / blend
    BorderBox, PaddingBox, ContentBox, TextBox,
    Repeat, RepeatX, RepeatY, Space, Round, NoRepeat,
    Multiply, Screen, Overlay, Darken, Lighten, ColorDodge, ColorBurn,
    HardLight, SoftLight, Difference, Exclusion, Hue, Saturation,
    ColorBlend, Luminosity,
    // fonts
    Italic, Oblique, SmallCaps, Bold, Bolder, Lighter,
    UltraCondensed, ExtraCondensed, Condensed, SemiCondensed,
    SemiExpanded, Expanded, ExtraExpanded, UltraExpanded,
    Sub, Super, AllSmallCaps, PetiteCaps, AllPetiteCaps, Unicase, TitlingCaps,
    HistoricalForms, Ruby, Ordinal, SlashedZero,
    // text
    Capitalize, Uppercase, Lowercase, FullWidth, FullSizeKana,
    Underline, Overline, LineThrough, Blink, Wavy,
    // icons
    Requested, Regular, Symbolic, Builtin,
    // animation
    Forwards, Backwards, Both, Running, Paused,
    Reverse, Alternate, AlternateReverse, Infinite, All,
    // misc
    Cover, Contain, Left, Right, Top, Bottom, Center, Closest, Farthest,
    ClosestSide, ClosestCorner, FarthestSide, FarthestCorner, Circle, Ellipse,
    Thin, Medium, Thick, Transparent, CurrentColor, Invert, Ltr, Rtl,
}
impl Keyword {
    pub fn as_str(self) -> &'static str;
    pub fn from_str_ascii_ci(s: &str) -> Option<Self>;
}
```

`ColorBlend` is CSS `color` as a blend-mode keyword (name collides with `Color`).

### 2.3 Length & calc — `value/length.rs`, `value/calc.rs`

```rust
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LengthUnit { Px, Pt, Pc, In, Cm, Mm, Em, Ex, Rem }

#[derive(Clone, Debug, PartialEq)]
pub enum Length {
    Abs { value: f32, unit: LengthUnit },
    Percent(f32),                 // fraction, 1.0 == 100%
    Calc(Rc<CalcNode>),
    Auto,                         // margin-* and border-image-width only
}

#[derive(Clone, Debug, PartialEq)]
pub enum CalcNode {
    Number(f32), Length(Length), Percent(f32), Angle(f32), Time(Time),
    Sum(Rc<CalcNode>, Rc<CalcNode>),
    Difference(Rc<CalcNode>, Rc<CalcNode>),
    Product(Rc<CalcNode>, f32),
    Quotient(Rc<CalcNode>, f32),
    Min(Rc<[CalcNode]>), Max(Rc<[CalcNode]>),
    Clamp(Rc<CalcNode>, Rc<CalcNode>, Rc<CalcNode>),
}

/// Everything needed to turn a `Length` into device pixels.
#[derive(Copy, Clone, Debug)]
pub struct LengthCtx {
    pub font_size_px: f32,        // this element's computed font-size
    pub root_font_size_px: f32,   // GTK's *initial* font-size (rem; NOT the root node's)
    pub ex_ratio: f32,            // x-height / em, from the resolved face (0.5 fallback)
    pub dpi: f32,                 // `-gtk-dpi`; pt/pc/in/cm/mm convert through this, not 96
    pub percent_basis: Option<f32>, // None => percentages are invalid here
}

impl Length {
    pub fn parse(input: &mut cssparser::Parser<'_, '_>) -> Result<Length, ()>;
    /// `None` == invalid at computed-value time (e.g. `%` with no basis,
    /// non-finite result, unit-incompatible calc).
    pub fn resolve(&self, ctx: &LengthCtx) -> Option<f32>;
    pub fn zero() -> Length;
    pub fn px(v: f32) -> Length;
}

impl CalcNode {
    /// Parses the *contents* of a `calc()`/`min()`/`max()`/`clamp()` function.
    pub fn parse(input: &mut cssparser::Parser<'_, '_>) -> Result<CalcNode, ()>;
    pub fn resolve_length(&self, ctx: &LengthCtx) -> Option<f32>;
    pub fn resolve_number(&self) -> Option<f32>;
    pub fn resolve_angle(&self) -> Option<f32>;
    pub fn resolve_time(&self) -> Option<Time>;
}
```

cssparser 0.37 has **no** calc AST — `calc()` arrives as `Token::Function`;
`CalcNode::parse` is 100% project code (research/cssparser-selectors.md §1).

### 2.4 Colour — `value/color.rs`

```rust
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Rgba { pub r: f32, pub g: f32, pub b: f32, pub a: f32 }  // unpremultiplied sRGB
impl Rgba {
    pub fn to_color32(self) -> skia_rs_safe::core::Color;   // round-to-nearest byte
    pub fn from_color32(c: skia_rs_safe::core::Color) -> Self;
    pub const TRANSPARENT: Rgba;
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ColorSpace { Srgb, SrgbLinear, Hsl, Hwb, Lab, Lch, Oklab, Oklch }

#[derive(Clone, Debug, PartialEq)]
pub enum ColorValue {
    Absolute(Rgba),
    CurrentColor,
    Named(Rc<str>),                       // `@name` — resolved lazily against ColorTable
    Mix { space: ColorSpace, a: Rc<ColorValue>, wa: Option<f32>,
                             b: Rc<ColorValue>, wb: Option<f32> },
    Relative { space: ColorSpace, origin: Rc<ColorValue>,
               channels: [ChannelExpr; 3], alpha: Option<ChannelExpr> },
    Legacy(Rc<LegacyColorFn>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum ChannelExpr { Keep(u8), Number(f32), Percent(f32), Calc(Rc<CalcNode>) }

#[derive(Clone, Debug, PartialEq)]
pub enum LegacyColorFn {                  // GTK's deprecated colour expressions
    Alpha(ColorValue, f32),               // multiplies alpha
    Shade(ColorValue, f32),               // 0 = black .. 2 = white
    Mix(ColorValue, ColorValue, f32),
    Lighter(ColorValue),                  // == shade(c, 1.3)
    Darker(ColorValue),                   // == shade(c, 0.7)
}

pub type ColorTable = HashMap<String, ColorValue>;   // UNRESOLVED — was `Color` in M1

pub struct ColorCtx<'a> {
    pub table: &'a ColorTable,
    pub current: Rgba,                    // the element's computed `color`
    pub depth: u8,                        // cycle guard; hard cap 32
}

impl ColorValue {
    pub fn parse(input: &mut cssparser::Parser<'_, '_>) -> Result<ColorValue, ()>;
    /// `None` == invalid at computed-value time (unknown `@name`, cycle,
    /// non-finite channel).
    pub fn resolve(&self, ctx: &ColorCtx<'_>) -> Option<Rgba>;
}

/// Order-preserving, lazy: a definition may reference a name defined *later*
/// (M1 required earlier-only). Cycles are broken by `ColorCtx::depth`.
pub fn build_color_table(defs: &[(String, String)]) -> ColorTable;
```

Gate consequence: all **37/37** `@define-color`s now resolve (M1 pinned 29).

### 2.5 Images — `value/image.rs`

```rust
#[derive(Clone, Debug, PartialEq)]
pub enum Image {
    None,
    Url(Rc<str>),                          // PNG always; SVG via skia-rs-svg
    Solid(ColorValue),                     // image(<color>)
    Gradient(Rc<Gradient>),
    CrossFade(Rc<[(Option<f32>, Image)]>), // percentages, `None` == auto-share
    Icon(Rc<IconRef>),                     // -gtk-* image fns; stored, drawn in M4
}

#[derive(Clone, Debug, PartialEq)]
pub struct Gradient {
    pub kind: GradientKind,
    pub repeating: bool,
    pub stops: Rc<[ColorStop]>,
    pub interpolation: Option<ColorSpace>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum GradientKind {
    Linear { direction: LinearDirection },
    Radial { shape: RadialShape, extent: RadialExtent, position: Position },
    Conic { from_angle: f32, position: Position },
}
#[derive(Clone, Debug, PartialEq)]
pub enum LinearDirection { Angle(f32), Side(SideOrCorner) }
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SideOrCorner { Top, Right, Bottom, Left, TopLeft, TopRight, BottomLeft, BottomRight }
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RadialShape { Circle, Ellipse }
#[derive(Clone, Debug, PartialEq)]
pub enum RadialExtent {
    ClosestSide, ClosestCorner, FarthestSide, FarthestCorner,
    Explicit(Length, Length),
}
#[derive(Clone, Debug, PartialEq)]
pub struct ColorStop {
    pub color: ColorValue,
    pub position: Option<Length>,          // px/%/calc
    pub hint: Option<Length>,              // interpolation hint *before* this stop
}
#[derive(Clone, Debug, PartialEq)]
pub struct Position { pub x: Length, pub y: Length, pub z: Option<Length> }
impl Position { pub fn center() -> Self; }

#[derive(Clone, Debug, PartialEq)]
pub enum IconRef {
    Theme { name: Rc<str> },                       // -gtk-icontheme()
    Recolor { url: Rc<str>, palette: Option<Rc<[(Rc<str>, ColorValue)]>> },
    Scaled { lo: Rc<Image>, hi: Rc<Image> },
}

impl Image { pub fn parse(input: &mut cssparser::Parser<'_, '_>) -> Result<Image, ()>; }
impl Gradient {
    /// The exact sampler the pixel tests derive expected colours from.
    /// `t` is the gradient-line parameter already normalised for the box.
    pub fn color_at(&self, t: f32, ctx: &ColorCtx<'_>, len_ctx: &LengthCtx) -> Rgba;
    /// Gradient line endpoints for a box, per CSS Images L3 §linear-gradient.
    pub fn line_for_box(&self, w: f32, h: f32, ctx: &LengthCtx) -> (Point, Point);
}
```

`Image::Url` that fails to decode is **recorded-unresolved**: it stays in the
computed value and paints nothing (logged once per URL), it is not an error.

### 2.6 Shadows, border-image — `value/shadow.rs`, `value/border.rs`

```rust
#[derive(Clone, Debug, PartialEq)]
pub struct Shadow {
    pub color: Option<ColorValue>,   // None == currentColor at used time
    pub offset_x: Length, pub offset_y: Length,
    pub blur: Length,                // 0 for text-shadow when omitted
    pub spread: Length,              // always `0` for text-shadow / -gtk-icon-shadow
    pub inset: bool,                 // always false for text-shadow
}
impl Shadow {
    pub fn parse(input: &mut cssparser::Parser<'_, '_>) -> Result<Shadow, ()>;      // box-shadow
    pub fn parse_text(input: &mut cssparser::Parser<'_, '_>) -> Result<Shadow, ()>; // no spread/inset
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum NumberOrPercent { Number(f32), Percent(f32) }
#[derive(Clone, Debug, PartialEq)]
pub struct BorderImageSlice { pub sides: [NumberOrPercent; 4], pub fill: bool } // TRBL
#[derive(Clone, Debug, PartialEq)]
pub enum BorderImageWidthSide { Length(Length), Number(f32), Auto }
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct RepeatStyle { pub x: Keyword, pub y: Keyword }
#[derive(Clone, Debug, PartialEq)]
pub enum BgSize { Auto, Cover, Contain, Explicit(Length, Length) }
```

### 2.7 Fonts & text — `value/font.rs`, `value/text.rs`

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FontFamily { Named(Rc<str>), Generic(GenericFamily) }
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum GenericFamily { Serif, SansSerif, Monospace, Cursive, Fantasy, SystemUi }

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum FontWeight { Absolute(f32), Bolder, Lighter }   // computes to Absolute(1..1000)
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum FontStyle { Normal, Italic, Oblique(f32) }      // degrees; bare `oblique` == 14.0
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum LineHeight { Normal, Number(f32), Length(Length) }
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct FeatureSetting { pub tag: [u8; 4], pub value: i32 }
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct VariationSetting { pub tag: [u8; 4], pub value: f32 }

bitflags::bitflags! {
    /// One flag set covering font-variant + every font-variant-* longhand.
    #[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
    pub struct FontVariantFlags: u32 {
        const SMALL_CAPS = 1 << 0;  const ALL_SMALL_CAPS = 1 << 1;
        const PETITE_CAPS = 1 << 2; const ALL_PETITE_CAPS = 1 << 3;
        const UNICASE = 1 << 4;     const TITLING_CAPS = 1 << 5;
        const NO_COMMON_LIGATURES = 1 << 6; const DISCRETIONARY_LIGATURES = 1 << 7;
        const HISTORICAL_LIGATURES = 1 << 8; const NO_CONTEXTUAL = 1 << 9;
        const SUB = 1 << 10;        const SUPER = 1 << 11;
        const LINING_NUMS = 1 << 12; const OLDSTYLE_NUMS = 1 << 13;
        const PROPORTIONAL_NUMS = 1 << 14; const TABULAR_NUMS = 1 << 15;
        const DIAGONAL_FRACTIONS = 1 << 16; const STACKED_FRACTIONS = 1 << 17;
        const ORDINAL = 1 << 18;    const SLASHED_ZERO = 1 << 19;
        const HISTORICAL_FORMS = 1 << 20; const RUBY = 1 << 21;
        const JIS78 = 1 << 22; const JIS83 = 1 << 23; const JIS90 = 1 << 24;
        const JIS04 = 1 << 25; const SIMPLIFIED = 1 << 26; const TRADITIONAL = 1 << 27;
        const FULL_WIDTH_EA = 1 << 28; const PROPORTIONAL_WIDTH_EA = 1 << 29;
    }
    #[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
    pub struct TextDecorationLines: u8 {
        const UNDERLINE = 1; const OVERLINE = 2; const LINE_THROUGH = 4; const BLINK = 8;
    }
}
```

### 2.8 Transform & filter — `value/transform.rs`, `value/filter.rs`

```rust
#[derive(Clone, Debug, PartialEq)]
pub enum TransformFn {
    Matrix([f32; 6]), Matrix3d([f32; 16]),
    Translate(Length, Length), TranslateX(Length), TranslateY(Length),
    TranslateZ(Length), Translate3d(Length, Length, Length),
    Scale(f32, f32), ScaleX(f32), ScaleY(f32), ScaleZ(f32), Scale3d(f32, f32, f32),
    Rotate(f32), RotateX(f32), RotateY(f32), RotateZ(f32),
    Rotate3d(f32, f32, f32, f32),
    Skew(f32, f32), SkewX(f32), SkewY(f32),
    Perspective(Length),
}
impl TransformFn { pub fn to_matrix(&self, ctx: &LengthCtx, basis: (f32, f32)) -> Matrix; }

/// Flattens a list to a 3x3 (M2 paints 2-D only; 3-D collapses via Matrix44).
pub fn transform_list_matrix(list: &[TransformFn], ctx: &LengthCtx,
                             basis: (f32, f32), origin: (f32, f32)) -> Matrix;

/// 2-D matrix decomposition (translate/rotate/scale/skew). skia-rs has NONE —
/// this is project code (research/skia-rs.md §5). Used only when interpolating
/// two structurally-mismatched transform lists.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Decomposed2d { pub tx: f32, pub ty: f32, pub rotate: f32,
                          pub scale_x: f32, pub scale_y: f32, pub skew_xy: f32 }
pub fn decompose_2d(m: &Matrix) -> Option<Decomposed2d>;
pub fn recompose_2d(d: &Decomposed2d) -> Matrix;

#[derive(Clone, Debug, PartialEq)]
pub enum FilterFn {
    Blur(Length), Brightness(f32), Contrast(f32), DropShadow(Shadow),
    Grayscale(f32), HueRotate(f32), Invert(f32), Opacity(f32),
    Saturate(f32), Sepia(f32),
}
```

### 2.9 Timing, time, animation names, keyframes — `value/timing.rs`, `value/keyframes.rs`

```rust
#[derive(Copy, Clone, Debug, PartialEq, PartialOrd)]
pub struct Time(pub f32);        // seconds; `Time::from_ms`, `Time::as_secs_f32`

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum StepPosition { JumpStart, JumpEnd, JumpNone, JumpBoth }

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum TimingFunction {
    Linear,
    CubicBezier(f32, f32, f32, f32),
    Steps(u32, StepPosition),
}
impl TimingFunction {
    pub const EASE: TimingFunction;       // cubic-bezier(0.25, 0.1, 0.25, 1.0)
    pub const EASE_IN: TimingFunction;    pub const EASE_OUT: TimingFunction;
    pub const EASE_IN_OUT: TimingFunction;
    pub fn parse(input: &mut cssparser::Parser<'_, '_>) -> Result<Self, ()>;
    /// Newton-Raphson on x (8 iters) then bisection fallback; exact at 0 and 1.
    pub fn eval(self, t: f32) -> f32;
}

#[derive(Clone, Debug, PartialEq)]
pub enum AnimationName { None, Named(Rc<str>) }
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum IterationCount { Infinite, Count(f32) }

#[derive(Clone, Debug)]
pub struct Keyframe {
    pub offsets: Rc<[f32]>,                    // 0..=1; `from`==0.0, `to`==1.0
    pub declarations: Rc<[(Prop, Value)]>,     // longhands only, shorthands pre-expanded
    pub timing: Option<TimingFunction>,        // per-keyframe `animation-timing-function`
}
#[derive(Clone, Debug)]
pub struct Keyframes { pub name: Rc<str>, pub frames: Rc<[Keyframe]> }
impl Keyframes {
    /// Every longhand mentioned by any frame, deduped, in registry order.
    pub fn properties(&self) -> Vec<Prop>;
    /// The two frames bracketing `t` for `prop`, plus the local 0..1 progress
    /// and the segment's timing function. `None` if `prop` appears in <2 frames
    /// and no synthetic 0%/100% is available.
    pub fn segment(&self, prop: Prop, t: f32)
        -> Option<(&Value, &Value, f32, TimingFunction)>;
}
```

### 2.10 Interpolation — `value/interpolate.rs`

```rust
pub fn discrete(a: &Value, b: &Value, t: f32) -> Value;   // b when t >= 0.5, else a
pub fn number(a: &Value, b: &Value, t: f32) -> Value;
pub fn length(a: &Value, b: &Value, t: f32) -> Value;     // same unit, else calc-sum
pub fn percentage(a: &Value, b: &Value, t: f32) -> Value;
pub fn color(a: &Value, b: &Value, t: f32) -> Value;      // premultiplied sRGB (GTK)
pub fn pair(a: &Value, b: &Value, t: f32) -> Value;
pub fn shadow_list(a: &Value, b: &Value, t: f32) -> Value; // transparent-padded
pub fn image(a: &Value, b: &Value, t: f32) -> Value;       // stop-wise if same structure
pub fn transform(a: &Value, b: &Value, t: f32) -> Value;   // componentwise, else decompose
pub fn filter_list(a: &Value, b: &Value, t: f32) -> Value;
pub fn list(a: &Value, b: &Value, t: f32) -> Value;        // componentwise, repeat-to-longest
pub fn font_weight(a: &Value, b: &Value, t: f32) -> Value;
pub fn line_height(a: &Value, b: &Value, t: f32) -> Value;
```

A `Value` pair the interpolator cannot handle falls back to `discrete` rather
than panicking. Unresolved `@name`/`currentColor` colours are resolved by
`computed` *before* they reach an interpolator — interpolators see resolved
`ColorValue::Absolute` only.

---

## 3. Node tree — `css/node.rs` [P2]

```rust
pub struct Node(Rc<NodeInner>);          // Clone == another handle to the same node

impl Node {
    // --- construction ---
    pub fn new(name: &str) -> Node;                            // detached root
    pub fn with_classes(name: &str, classes: &[&str]) -> Node;
    // --- tree mutation (each bumps the tree generation + touched nodes) ---
    pub fn append_child(&self, child: &Node);                  // reparents if attached
    pub fn insert_child(&self, index: usize, child: &Node);
    pub fn remove_child(&self, child: &Node) -> bool;
    pub fn detach(&self);
    // --- identity mutation ---
    pub fn set_id(&self, id: Option<&str>);
    pub fn add_class(&self, class: &str) -> bool;              // false if already present
    pub fn remove_class(&self, class: &str) -> bool;
    pub fn set_classes(&self, classes: &[&str]);
    // --- state ---
    pub fn states(&self) -> PseudoStates;                      // incl. derived flags
    pub fn set_states(&self, states: PseudoStates);            // own flags only
    pub fn set_state(&self, flag: PseudoStates, on: bool);
    // --- direction ---
    pub fn set_direction(&self, direction: Option<Direction>); // None == inherit
    pub fn direction(&self) -> Direction;                      // resolved up the chain
    // --- reads ---
    pub fn name(&self) -> Rc<str>;
    pub fn id(&self) -> Option<CssString>;
    pub fn classes(&self) -> Vec<CssString>;
    pub fn parent(&self) -> Option<Node>;
    pub fn children(&self) -> Vec<Node>;
    pub fn child(&self, index: usize) -> Option<Node>;
    pub fn child_count(&self) -> usize;
    pub fn index_in_parent(&self) -> Option<usize>;
    pub fn root(&self) -> Node;
    pub fn ancestors(&self) -> impl Iterator<Item = Node>;     // parent-first, excludes self
    pub fn descendants(&self) -> impl Iterator<Item = Node>;   // pre-order, excludes self
    // --- change tracking ---
    pub fn generation(&self) -> u64;        // tree-wide; any mutation anywhere bumps it
    pub fn self_generation(&self) -> u64;   // bumped only by mutations affecting this node
    pub fn ptr_eq(&self, other: &Node) -> bool;
}

bitflags::bitflags! {
    #[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
    pub struct PseudoStates: u16 {
        const HOVER = 1 << 0;  const ACTIVE = 1 << 1;   const FOCUS = 1 << 2;
        const FOCUS_VISIBLE = 1 << 3; const FOCUS_WITHIN = 1 << 4;
        const CHECKED = 1 << 5; const INDETERMINATE = 1 << 6;
        const DISABLED = 1 << 7; const BACKDROP = 1 << 8; const SELECTED = 1 << 9;
        const DROP_ACTIVE = 1 << 10; const LINK = 1 << 11; const VISITED = 1 << 12;
    }
}
```

`FOCUS_WITHIN` and `FOCUS_VISIBLE` are **derived**: setting/clearing `FOCUS` on
a node propagates both up every ancestor (GTK's documented divergence from CSS
— research/gtk-css-semantics.md §12) and bumps each ancestor's
`self_generation`. Callers never set them directly; `set_states` masks them out.

`Direction` and `CssString` keep their M1 definitions (`css/select.rs`), moved
to `node.rs` and re-exported from `select.rs`.

### 3.1 `Element for Node` — complete obligations

Real implementations (must walk the tree):

| method | semantics |
|---|---|
| `opaque()` | `OpaqueElement::new(Rc::as_ptr(&self.0))` |
| `parent_element()` | `self.parent()` |
| `prev_sibling_element()` / `next_sibling_element()` | index-based sibling lookup — drives `+`, `~`, and all `:nth-*` |
| `first_element_child()` | `self.child(0)` |
| `is_empty()` | `self.child_count() == 0` |
| `is_root()` | `self.parent().is_none()` |
| `has_local_name` / `has_namespace` / `is_same_type` | name compare; namespace always the empty one |
| `has_id(id, case)` | real; GTK ids exist via `Node::set_id` |
| `has_class(name, case)` | real |
| `match_non_ts_pseudo_class(pc, ctx)` | `GtkPseudoClass` → `PseudoStates` bit; `:dir()` → `Node::direction()`; `:drop(active)` → `DROP_ACTIVE`; `:link`/`:visited` → their bits (never set by M2, so they never match) |
| `match_pseudo_element` | always `false` (no pseudo-element boxes) |
| `apply_selector_flags` | no-op (M2 restyles whole dirty subtrees) |
| `add_element_unique_hashes(filter)` | inserts `fnv1a(name)` + each class hash + id hash; returns `true` — **must not stay `false`**, or the bloom filter is a permanent no-op |

Constant `false` (documented on the impl, with the reason):
`attr_matches`, `has_custom_state`, `imported_part`, `is_part`,
`is_html_slot_element`, `is_html_element_in_html_document`,
`is_link`, `assigned_slot`, `containing_shadow_host`, `pseudo_element_originating_element`.
`:nth-child(… of S)` is **not** enabled: `GtkSelectorParser::parse_nth_child_of`
keeps the `false` default (GTK CSS has no `of S`).

`GtkPseudoClass` gains `FocusWithin`, `Indeterminate`, `Link`, `Visited`
variants; `Other`/`OtherFunctional` keep M1's parse-but-never-match behaviour.

### 3.2 Matching context & buckets — `css/select.rs` [P2]

```rust
/// Caller-owned, reused across a whole restyle pass (keeps the nth-index cache warm).
pub struct MatchCx { /* SelectorCaches + BloomFilter + ancestor depth */ }
impl MatchCx {
    pub fn new() -> Self;
    pub fn push_ancestor(&mut self, node: &Node);   // inserts the node's unique hashes
    pub fn pop_ancestor(&mut self, node: &Node);
    pub fn reset(&mut self);
    /// Positions the bloom filter for `node` by walking its ancestors.
    pub fn seed_for(&mut self, node: &Node);
}

/// Rules bucketed by the rightmost compound's id / class / local name,
/// built from `Selector::iter()` (selectors 0.40 has no bucketing of its own).
pub struct RuleBuckets { /* by_id, by_class, by_name, universal */ }
impl RuleBuckets {
    pub fn build(rules: &[CompiledRule]) -> Self;
    /// Candidate rule indices for `node`, in source order, deduped.
    pub fn candidates(&self, node: &Node) -> Vec<usize>;
}

pub fn parse_selector_list(text: &str) -> Option<SelectorList<GtkSelectorImpl>>;
pub fn matches(list: &SelectorList<GtkSelectorImpl>, node: &Node, cx: &mut MatchCx) -> bool;
pub fn matches_with_specificity(list: &SelectorList<GtkSelectorImpl>, node: &Node,
                                cx: &mut MatchCx) -> Option<u32>;
```

---

## 4. Stylesheets — `css/parse.rs` [P1] + `css/cascade.rs` [P3]

```rust
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ColorScheme { Light, Dark }
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Contrast { NoPreference, More, Less }

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct MediaEnv { pub color_scheme: ColorScheme, pub contrast: Contrast }
impl Default for MediaEnv { /* Light, NoPreference */ }

/// `prefers-reduced-motion` parses and always evaluates FALSE in M2 — GTK 4.20+
/// supports it; M2's env does not model it. (research/gtk-css-semantics.md §13)
#[derive(Clone, Debug, PartialEq)]
pub enum MediaQuery {
    ColorScheme(ColorScheme), Contrast(Contrast), ReducedMotion,
    Not(Rc<MediaQuery>), And(Rc<[MediaQuery]>), Or(Rc<[MediaQuery]>),
    AlwaysFalse,                        // unknown feature: parses, never matches
}
impl MediaQuery { pub fn evaluate(&self, env: &MediaEnv) -> bool; }

#[derive(Clone, Debug)]
pub struct MediaBlock {
    pub query: MediaQuery,
    pub rules: Vec<StyleRule>,          // source_order preserved from the outer sheet
    pub keyframes: Vec<KeyframesRule>,
    pub color_definitions: Vec<(String, String)>,
}

#[derive(Clone, Debug)]
pub struct KeyframesRule {
    pub name: String,
    pub frames: Vec<(Vec<f32>, Vec<Declaration>)>,   // raw; compiled in cascade.rs
    pub source_order: usize,
}

#[derive(Debug, Clone, Default)]
pub struct Stylesheet {
    pub rules: Vec<StyleRule>,                       // unchanged (M1 shape)
    pub color_definitions: Vec<(String, String)>,    // unchanged
    pub keyframes: Vec<KeyframesRule>,               // NEW
    pub media_blocks: Vec<MediaBlock>,               // NEW — kept UNEVALUATED
}

pub fn parse_stylesheet(css: &str) -> Stylesheet;                                  // unchanged
pub fn parse_stylesheet_with_base(css: &str, base: Option<&Path>) -> Stylesheet;   // unchanged
```

`@media` is retained unevaluated so one parse can be compiled under several
envs (the coverage gate compiles light/dark/hc from the same sheets).

```rust
pub struct CompiledSheet {
    pub rules: Vec<CompiledRule>,                       // unchanged shape
    pub colors: ColorTable,                             // now UNRESOLVED ColorValues
    pub keyframes: HashMap<Rc<str>, Rc<Keyframes>>,     // NEW; last definition wins
    pub buckets: RuleBuckets,                           // NEW
    pub env: MediaEnv,                                  // NEW; which env produced this
}
impl CompiledSheet {
    pub fn compile(css: &str) -> Self;                                   // MediaEnv::default()
    pub fn from_stylesheet(sheet: Stylesheet) -> Self;                   // MediaEnv::default()
    pub fn compile_with_env(sheet: &Stylesheet, env: &MediaEnv) -> Self; // NEW
    pub fn keyframes(&self, name: &str) -> Option<&Rc<Keyframes>>;
}
```

Matching `@media` blocks splice their rules/keyframes/colours in at the block's
source position; non-matching blocks are fully parsed and then dropped.

---

## 5. Cascade & computed style — `css/cascade.rs`, `css/computed.rs` [P3]

```rust
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct CascadeKey { pub important: bool, pub specificity: u32,
                        pub source_order: usize, pub declaration_order: usize }

#[derive(Clone, Debug)]
pub struct CascadedDecl { pub value: Value, pub key: CascadeKey, pub from_shorthand: Option<Prop> }

/// One slot per longhand; each `Vec` is sorted best-first.
pub struct CascadedValues { by_prop: Box<[Vec<CascadedDecl>; N_LONGHANDS]> }
impl CascadedValues {
    pub fn candidates(&self, prop: Prop) -> &[CascadedDecl];
    pub fn winner(&self, prop: Prop) -> Option<&Value>;
    pub fn is_empty(&self) -> bool;
    pub fn len(&self) -> usize;                       // declared longhands
}

/// Matches `sheet` against `node`, expands shorthands through their registry
/// `ExpandFn` (carrying the shorthand's own key onto each longhand), parses
/// each declaration into a `Value`, and keeps every runner-up for diagnostics.
pub fn cascade(sheet: &CompiledSheet, node: &Node, cx: &mut MatchCx) -> CascadedValues;
```

Parse-time-invalid declarations are dropped here (logged), never stored.

```rust
#[derive(Copy, Clone, Debug)]
pub struct ResolveEnv { pub dpi: f32, pub root_font_size: f32 }
impl Default for ResolveEnv { /* dpi 96.0, root_font_size 14.0 */ }

#[derive(Clone, Debug, PartialEq)]
pub struct ComputedStyle { values: Box<[Value; N_LONGHANDS]> }

pub trait FromValue: Sized { fn from_value(v: &Value) -> Self; }
// impls: f32, Rgba, Length-as-px via ctx-free `f32`, Keyword, bool,
// Rc<[FontFamily]>, Rc<[Shadow]>, Image, TimingFunction, Time, ...
// A type mismatch returns that type's initial-equivalent and logs at debug.

impl ComputedStyle {
    pub fn initial(env: &ResolveEnv) -> Rc<ComputedStyle>;
    pub fn raw(&self, prop: Prop) -> &Value;
    pub fn get<T: FromValue>(&self, prop: Prop) -> T;

    /// The one true entry point. `parent` supplies inherited values and the
    /// `em`/`currentColor` bases; `None` == this node is the style root.
    pub fn resolve(sheet: &CompiledSheet, node: &Node,
                   parent: Option<&ComputedStyle>, env: &ResolveEnv,
                   cx: &mut MatchCx) -> ComputedStyle;
    /// Walks the whole ancestor chain from the root down (M1 `resolve`'s shape).
    pub fn resolve_chain(sheet: &CompiledSheet, node: &Node, env: &ResolveEnv,
                         cx: &mut MatchCx) -> ComputedStyle;
    /// Applies animation output. Borrowed unchanged when `overrides` is empty.
    pub fn with_overrides<'a>(&'a self, overrides: &Overrides)
        -> std::borrow::Cow<'a, ComputedStyle>;

    // Typed accessors consumed by layout/paint/text (all sides TRBL):
    pub fn font_size_px(&self) -> f32;
    pub fn length_ctx(&self, env: &ResolveEnv, percent_basis: Option<f32>) -> LengthCtx;
    pub fn color(&self) -> Rgba;
    pub fn opacity(&self) -> f32;
    pub fn padding(&self, basis: f32) -> [f32; 4];
    pub fn margin(&self, basis: f32) -> [Option<f32>; 4];      // None == `auto`
    pub fn border_widths(&self) -> [f32; 4];                    // style none/hidden => 0.0
    pub fn border_colors(&self) -> [Rgba; 4];
    pub fn border_styles(&self) -> [Keyword; 4];
    pub fn border_radii(&self, w: f32, h: f32) -> [[f32; 2]; 4]; // TL,TR,BR,BL x (rx,ry), CSS-scaled
    pub fn min_size(&self, basis: (f32, f32)) -> (f32, f32);     // content box
    pub fn background_layers(&self) -> Vec<BackgroundLayer>;     // top-first, see §8
    pub fn box_shadows(&self) -> Rc<[Shadow]>;
    pub fn transition_specs(&self) -> Vec<TransitionSpec>;       // see §6
    pub fn animation_specs(&self) -> Vec<AnimationSpec>;         // see §6
}
```

**Resolution order inside `resolve` (fixed):** `-gtk-dpi` → `font-size` (its
`em`/`%` resolve against the *parent's* font-size) → `color` → every remaining
longhand in registry order.

**Inheritance rule.** For each longhand, in order:
1. `winner` is `Wide::Inherit` → parent's computed value (or initial at the root).
2. `winner` is `Wide::Initial` → `prop.initial()`.
3. `winner` is `Wide::Unset` → rule 1 if `prop.is_inherited()`, else rule 2.
4. No winner → parent's computed value if `prop.is_inherited()`, else `prop.initial()`.
5. Otherwise the winner's `Value`, with `@name`, `currentColor`, `em`/`rem`/`%`
   and `calc()` resolved against `parent` / `env` / this node's own `color` and
   `font-size`.

**Invalid at computed value time (Decision 6, replaces M1's runner-up rule).**
If step 5 fails to resolve (unknown `@name`, colour cycle, `%` with no basis,
non-finite calc), the property takes the **inherited** value if
`prop.is_inherited()`, else its **initial** value — it does *not* fall back to
the runner-up. Runner-ups stay in `CascadedValues` for diagnostics and are
logged at `tracing::debug!` with property, value, and rank.

---

## 6. Animation — `ui/src/anim/` [P5]

```rust
pub trait Clock { fn now(&self) -> Duration; }

pub struct MonotonicClock(Instant);                 // production; frame-callback driven
impl Clock for MonotonicClock { .. }
impl MonotonicClock { pub fn new() -> Self; }

pub struct ManualClock(Cell<Duration>);             // tests
impl Clock for ManualClock { .. }
impl ManualClock {
    pub fn new() -> Self;                           // t = 0
    pub fn advance_ms(&self, ms: u64);
    pub fn set_ms(&self, ms: u64);
}

/// Per-node animation output, sorted by `Prop`, applied over the computed style.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Overrides { entries: Vec<(Prop, Value)> }
impl Overrides {
    pub fn is_empty(&self) -> bool;
    pub fn get(&self, prop: Prop) -> Option<&Value>;
    pub fn iter(&self) -> impl Iterator<Item = (Prop, &Value)>;
    pub fn set(&mut self, prop: Prop, value: Value);   // animation wins over transition
}

/// Flattened from the computed `transition-*` / `animation-*` longhands.
#[derive(Clone, Debug, PartialEq)]
pub struct TransitionSpec { pub prop: Option<Prop>, pub all: bool, pub duration: Time,
                            pub delay: Time, pub timing: TimingFunction }
#[derive(Clone, Debug, PartialEq)]
pub struct AnimationSpec { pub name: AnimationName, pub duration: Time, pub delay: Time,
                           pub timing: TimingFunction, pub iterations: IterationCount,
                           pub direction: Keyword, pub fill: Keyword, pub play_state: Keyword }

pub struct AnimationState { /* Vec<Transition>, Vec<ActiveAnimation> */ }
impl AnimationState {
    pub fn new() -> Self;
    /// Call on every restyle. Diffs `old` vs `new` over each spec's properties,
    /// starting/retargeting/cancelling transitions with CSS Transitions §3.1
    /// reversal continuity, and (re)binding `animation-name` to the sheet's
    /// `@keyframes`. `old: None` == first style: no transitions start.
    pub fn restyle(&mut self, old: Option<&ComputedStyle>, new: &ComputedStyle,
                   now: Duration, sheet: &CompiledSheet);
    /// Current animated values. Precedence: animation > transition > base cascade.
    pub fn sample(&mut self, now: Duration) -> Overrides;
    pub fn is_active(&self, now: Duration) -> bool;     // true => request another frame
    /// Earliest instant `sample` would change; `None` when idle.
    pub fn next_deadline(&self, now: Duration) -> Option<Duration>;
}
```

Non-animatable property changes (`animatable: None`) switch discretely at
t = 0.5 of the transition. Zero-duration transitions are constructed and
complete instantly rather than being special-cased away.

---

## 7. Layout — `ui/src/layout.rs` [P4]

```rust
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Rect { pub x: f32, pub y: f32, pub width: f32, pub height: f32 }

/// Absolute, tree-origin coordinates. Replaces M1's `{width,height,label_x,label_y}`.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Allocation {
    pub border_box: Rect,
    pub content_box: Rect,
    pub border: [f32; 4],     // TRBL, used widths
    pub padding: [f32; 4],    // TRBL
}
impl Allocation {
    pub fn padding_box(&self) -> Rect;
    pub fn box_for(&self, k: Keyword) -> Rect;   // BorderBox|PaddingBox|ContentBox
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BoxDirection { Row, Column }
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Container { Box { direction: BoxDirection }, Leaf }   // default: Box{Column}

/// Intrinsic size for a leaf. `known`/`available` come straight from taffy.
pub trait Measure {
    fn measure(&mut self, node: &Node, style: &ComputedStyle,
               known: taffy::Size<Option<f32>>,
               available: taffy::Size<taffy::AvailableSpace>) -> taffy::Size<f32>;
}

pub struct LayoutTree { /* TaffyTree<NodeKey> + Node-ptr -> taffy::NodeId map */ }
impl LayoutTree {
    pub fn new() -> Self;
    /// Creates/reparents/removes taffy nodes to mirror `root`'s subtree. Idempotent;
    /// a no-op when the tree generation has not changed.
    pub fn sync(&mut self, root: &Node) -> Result<(), LayoutError>;
    /// Writes one node's CSS box into taffy. `BoxSizing::ContentBox` +
    /// min_size from `ComputedStyle::min_size` (GTK's content-box rule),
    /// margin/padding/border per side, `border-spacing` -> `Style::gap`.
    /// taffy's `set_style` self-marks dirty; do not call `mark_dirty` after it.
    pub fn set_style(&mut self, node: &Node, style: &ComputedStyle,
                     container: Container, env: &ResolveEnv);
    pub fn compute(&mut self, root: &Node,
                   available: taffy::Size<taffy::AvailableSpace>,
                   measure: &mut dyn Measure) -> Result<(), LayoutError>;
    pub fn allocation(&self, node: &Node) -> Option<Allocation>;
}

#[derive(Debug)]
pub enum LayoutError { Taffy(taffy::TaffyError), Unsynced }
```

`transform`, `opacity` and `filter` never reach taffy.

---

## 8. Paint — `ui/src/paint/` [P4]

```rust
/// Whole-node paint, in order: outset box-shadow -> background layers ->
/// border-image (else per-side borders) -> inset box-shadow -> outline ->
/// text -> children. `overrides` layers animation output over `style`.
pub fn paint_node(canvas: &mut Canvas<'_>, node: &Node, style: &ComputedStyle,
                  alloc: &Allocation, overrides: Option<&Overrides>, cx: &PaintCx);

pub struct PaintCx<'a> {
    pub env: &'a ResolveEnv,
    pub colors: &'a ColorTable,
    pub fonts: &'a mut FontDatabase,
    pub images: &'a mut ImageCache,
    pub text: Option<&'a ShapedText>,     // the node's own label, pre-shaped
}

/// One resolved `background-*` layer, top-first (CSS: first listed is topmost).
#[derive(Clone, Debug)]
pub struct BackgroundLayer {
    pub image: Image, pub position: Position, pub size: BgSize,
    pub repeat: RepeatStyle, pub origin: Keyword, pub clip: Keyword,
    pub blend: Keyword,
}

// --- per-family helpers (each takes the already-resolved geometry) ---
pub fn paint_backgrounds(canvas: &mut Canvas<'_>, color: Rgba, layers: &[BackgroundLayer],
                         alloc: &Allocation, radii: &[[f32; 2]; 4], cx: &mut PaintCx<'_>);
pub fn paint_borders(canvas: &mut Canvas<'_>, alloc: &Allocation, widths: [f32; 4],
                     colors: [Rgba; 4], styles: [Keyword; 4], radii: &[[f32; 2]; 4]);
pub fn paint_border_image(canvas: &mut Canvas<'_>, alloc: &Allocation, source: &Image,
                          slice: &BorderImageSlice, widths: &[BorderImageWidthSide; 4],
                          repeat: RepeatStyle, cx: &mut PaintCx<'_>) -> bool; // false => fall back to paint_borders
pub fn paint_box_shadows(canvas: &mut Canvas<'_>, shadows: &[Shadow], inset: bool,
                         alloc: &Allocation, radii: &[[f32; 2]; 4], current: Rgba,
                         ctx: &LengthCtx);
pub fn paint_outline(canvas: &mut Canvas<'_>, alloc: &Allocation, width: f32, offset: f32,
                     color: Rgba, style: Keyword, radii: &[[f32; 2]; 4]);
pub fn paint_text(canvas: &mut Canvas<'_>, text: &ShapedText, origin: (f32, f32),
                  style: &ComputedStyle, ctx: &LengthCtx);
/// Returns a save-count to restore; sets up opacity/transform/filter layers.
pub fn begin_effects(canvas: &mut Canvas<'_>, style: &ComputedStyle, alloc: &Allocation,
                     ctx: &LengthCtx) -> usize;
pub fn end_effects(canvas: &mut Canvas<'_>, save_count: usize);

/// skia-rs has no RRect->Path and no draw_rrect/clip_rrect (research/skia-rs.md §1-2).
pub fn rounded_rect_path(rect: Rect, radii: &[[f32; 2]; 4]) -> Path;
/// The ring between two rounded rects, as one even-odd path.
pub fn rounded_ring_path(outer: Rect, outer_radii: &[[f32; 2]; 4],
                         inner: Rect, inner_radii: &[[f32; 2]; 4]) -> Path;

pub struct ImageCache { /* url -> decoded Image / recorded-unresolved */ }
impl ImageCache {
    pub fn new() -> Self;
    pub fn get(&mut self, url: &str) -> Option<&skia_rs_codec::image::Image>;
}
```

Each side of a mixed border is a filled path between the outer and inner rounded
rects (mitred at the corners), never a stroke — so mixed widths/colours join
like GTK. `-gtk-icon-*` draws nothing in M2.

**Exactness rule (carried from M1):** asserted pixels must be derivable from the
CSS — `Gradient::color_at` is the sampler model, shadows are asserted only at
their flat cores, anti-aliased edges are never asserted exactly.

---

## 9. Fonts & text — `ui/src/text.rs` [P6]

```rust
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FontFace { pub path: PathBuf, pub index: i32, pub family: String }

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct FontQuery<'a> {
    pub families: &'a [FontFamily],
    pub weight: f32,          // CSS 1..1000
    pub style: FontStyle,
    pub stretch: f32,         // CSS percentage, 100.0 == normal
    pub size_px: f32,
}

pub struct FontDatabase { /* Option<Fontconfig> + caches; single-threaded, !Send */ }
impl FontDatabase {
    /// `FcInit` once per process; falls back to `probe_only()` when it fails.
    pub fn new() -> FontDatabase;
    /// M1's fixed `FONT_CANDIDATES` path probe — the CI / no-fontconfig fallback.
    pub fn probe_only() -> FontDatabase;
    pub fn has_fontconfig(&self) -> bool;
    /// fc-match parity: one FcPattern with every family in priority order plus
    /// FC_WEIGHT/FC_SLANT/FC_WIDTH/FC_PIXEL_SIZE, then FcFontMatch.
    pub fn match_face(&mut self, query: &FontQuery<'_>) -> Option<FontFace>;
    pub fn typeface(&mut self, face: &FontFace) -> Option<Arc<Typeface>>;
    pub fn font(&mut self, face: &FontFace, size_px: f32) -> Option<Font>;
    pub fn shape(&mut self, key: &ShapeKey<'_>) -> Rc<ShapedText>;
    pub fn clear_caches(&mut self);
}

/// CSS <-> fontconfig scale conversion. The two scales are NOT proportional
/// (research/taffy-fontconfig.md §2.4) — these are table lookups with
/// piecewise-linear interpolation, not casts.
pub fn css_weight_to_fc(w: f32) -> i32;
pub fn css_stretch_to_fc(pct: f32) -> i32;
pub fn css_style_to_fc_slant(s: FontStyle) -> i32;

#[derive(Clone, Debug, PartialEq)]
pub struct ShapeKey<'a> {
    pub text: &'a str, pub face: &'a FontFace, pub size_px: f32,
    pub letter_spacing_px: f32, pub features: &'a [FeatureSetting],
    pub variations: &'a [VariationSetting], pub transform: Keyword, // text-transform
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct TextMetrics { pub width: f32, pub ascent: f32, pub descent: f32,
                         pub line_height: f32 }   // unchanged from M1

pub struct ShapedText {
    pub blob: Option<TextBlob>,     // None for "" or a run with no glyphs
    pub metrics: TextMetrics,
    pub face: FontFace,
    pub size_px: f32,
}
```

`font-feature-settings` and `font-variation-settings` **cannot reach rustybuzz**
through `skia-rs-text` 0.4.0 (dead `Features` API, no variation axes —
research/skia-rs.md §8): they parse, compute, enter `ShapeKey`, and are dropped
at the shaper with a one-time `tracing::warn!`. Documented, not silently ignored.

`text-transform` (incl. `full-width`) is applied to the string *before* shaping,
inside `FontDatabase::shape`. `letter-spacing` is applied via
`TextBlobBuilder::add_positioned_run`.

---

## 10. Migration from M1

### 10.1 Replaced types

| M1 | M2 | note |
|---|---|---|
| `css::select::CssNode` | `css::node::Node` | mutable, real children/siblings; `Rc` handle |
| `css::select::PseudoStates` (7 bools) | `node::PseudoStates` bitflags (13) | adds focus-visible/-within, indeterminate, drop-active, link/visited |
| `css::computed::Background` / `GradientStop` / `BackgroundClip` | `Value::Image` + `BackgroundLayer` + `Keyword::{BorderBox,..}` | N layers, all gradient kinds |
| `ComputedStyle { background, color, border_width, .. }` (10 scalar fields) | `ComputedStyle { values: Box<[Value; 95]> }` + typed accessors | per-side borders, per-corner radii are first-class |
| `css::colors::{ColorRef, parse_color_ref, parse_color_value}` | `value::color::{ColorValue, ColorValue::parse, ColorValue::resolve}` | colors.rs deleted |
| `ColorTable = HashMap<String, Color>` | `ColorTable = HashMap<String, ColorValue>` | unresolved; lazy + cycle-guarded |
| `css::shorthand::expand(&str,&str) -> Vec<(String,String)>` | registry `ExpandFn` -> `(Prop, Value)` | shorthand.rs deleted |
| `CascadedValues { map: HashMap<String, ..> }`, `winner(&str)` | keyed by `Prop`, `winner(Prop) -> Option<&Value>` | |
| `cascade(&CompiledSheet, &CssNode)` | `cascade(&CompiledSheet, &Node, &mut MatchCx)` | |
| `ComputedStyle::resolve(sheet, node)` / `resolve_with_parent` | `resolve_chain(..)` / `resolve(.., parent, env, cx)` | |
| `layout::{ButtonLayout, layout_button}`, `Allocation{width,height,label_x,label_y}` | `LayoutTree`, `Allocation{border_box, content_box, border, padding}` | |
| `paint::paint_button(surface, origin, style, alloc, label)` | `paint::paint_node(canvas, node, style, alloc, overrides, cx)` | |
| `text::FontStack` (8 fixed paths) | `text::FontDatabase` + `FontFace` | `FONT_CANDIDATES` survives inside `probe_only()` |
| `css::value` (token helpers) | `css::tokens` (same bodies) | `css::value` is now the typed-value module |
| M1 runner-up fallback in `computed::pick` | invalid-at-computed-value-time (§5) | runner-ups kept for diagnostics only |

Unchanged and depended on by other crates/tests: `shm::*`, `wayland::*`,
`app::{ThemeSource, ThemeEnv, load_layered_stylesheet, compile_theme}`,
`parse::{Declaration, StyleRule, parse_stylesheet, parse_stylesheet_with_base,
MAX_IMPORT_DEPTH}`, `lib::BUNDLED_ADWAITA_LIGHT`, `css::select::{CssString,
Direction, GtkSelectorImpl, GtkPseudoClass, parse_selector_list}`.

### 10.2 THE GATE — M1 tests that must stay byte-identical (46)

No edit of any kind, including imports, is permitted in these:

- `src/css/parse.rs` — all **16** (`parses_selector_text_and_declarations` …
  `a_missing_import_target_does_not_abort_the_sheet`). New `@media`/`@keyframes`
  tests are appended, never interleaved.
- `src/css/tokens.rs` — all **4** (the M1 `value.rs` bodies, file relocated).
- `src/shm.rs` — all **11**.
- `src/wayland.rs` — all **11**.
- `src/lib.rs` — **1** (`bundled_adwaita_is_the_extracted_gtk4_theme`).
- `tests/layer_shell_screencopy.rs` — all **3**, including the `close()` ±12
  tolerance derivation and the implicit-grab gesture test.

Pinned constants that must not move: **900** compiled Adwaita rules; 1941 lines
/ 37 `@define-color`s; `POOL_INITIAL_BUFFERS=2`, `POOL_MAX_BUFFERS=3`,
`CONFIGURE_TIMEOUT=5s`, `MARGIN=0`, `BTN_LEFT=0x110`; every declared Adwaita
hex in the screencopy tolerance derivation (`#3584e4`, `#1c6fd4`, `#1961b9`).

### 10.3 M1 tests rewritten, with reason

| file | n | reason | invariant |
|---|---|---|---|
| `src/css/colors.rs` → `value/color.rs` | 10 | module folded into the value family | 9 keep their assertions verbatim; **`relative_color_syntax_is_recorded_as_unresolved_not_fabricated` inverts**: the table is now **37**, not 29, and is renamed `relative_color_syntax_resolves`. This 29→37 move is an M2 deliverable, not a regression. |
| `src/css/shorthand.rs` → `registry.rs` | 9 | string expansion → `ExpandFn`/`(Prop, Value)` | every case re-expressed against `Prop`; the four-sides rule, border reset, background colour/image split and line-width-keyword cases keep their inputs and expected side values |
| `src/css/cascade.rs` | 10 | `CssNode`→`Node`, `winner(&str)`→`winner(Prop)` | `every_adwaita_rule_compiles` keeps `== 900`; `adwaita_button_cascade_resolves_the_expected_declarations` keeps every declared value |
| `src/css/computed.rs` | 22 | field access → `get::<T>(Prop)`; `Background`→layers | every pinned Adwaita byte stays (normal/hover/active/suggested gradients, `#2e3436`, `#cdc7c2`, radius 5, padding `[4,9,4,9]`, min 16×24, font-size 14). **`an_uninterpretable_winner_falls_back_to_the_runner_up` inverts** (Decision 6): `.sidebar-button { border-radius: 100% }` is now *valid* (percentage radii), so the expectation becomes a percentage-resolved radius, not `5.0`; renamed `an_invalid_winner_yields_the_initial_or_inherited_value` with a fresh genuinely-invalid case. |
| `src/css/select.rs` | 9 | `CssNode`→`Node` | all 9 keep their assertions; **new**: the 33 Adwaita `:first-child`/`:last-child`/`:only-child`/`:not(:first-child)` rules become a regression test proving they now match correctly |
| `src/layout.rs` | 6 | tests build `ComputedStyle` field literals, which no longer exist; `Allocation` reshaped | fixtures built from CSS instead; **every number is preserved**: 80×34, label at (10, 8), 36×34 empty, 220×50 intrinsic, borderless == label, reused-tree equality |
| `src/paint.rs` | 4 | `paint_button`→`paint_node`, new `Allocation`/`PaintCx` | same CSS fixture, same pixels (`0xFF112233` at (0,0) and (20,10); padding-box and content-box clips; gradient-clip agreement) |
| `src/text.rs` | 5 | `FontStack`→`FontDatabase`/`FontFace` | same properties asserted; the "resolves to *something*" rule replaces any fixed family |
| `src/widget/button.rs` | 4 | `Button` becomes behaviour over a `Node` | the three cache-invalidation regressions (shaped label, parent style, per-sheet identity) are preserved as behaviours |
| `src/app.rs` | 4 of 11 | the 4 going through `button_style`/`headerbar_min_height` touch `ComputedStyle` fields | same values (`#cdc7c2`, padding `[4,9,4,9]`, layering/tie-break outcomes). The other **7** (theme-dir discovery, `$GTK_THEME` variants, XDG fallbacks, `the_bundled_source_is_exactly_the_vendored_sheet`) are byte-identical and join §10.2's gate. |
| `tests/themed_button_offscreen.rs` | 4 | **the M1 pixel gate** — reads `style.background`, `Background::LinearGradientToTop`, `allocation.width/label_x`, `CssNode::new` | rewritten *mechanically only*. Every numeric constant and every pixel assertion is byte-identical: height exactly `34.0`; `y = height-2` band `0xFFF6F5F4`; gutter column `bx = 4`; midpoint between the two stops; corner (0,0) transparent; border pixel `(cx,0)` `0xFFCDC7C2`; hover gutter `0xFFE8E6E3`; active `0xFFDAD6D2`; suggested-action `#2c7fe3`→`#3584e4`, white text, border `0xFF15539E`, gutter within ±10/channel of `#3584e4`; empty button 36×34; >20 dark label pixels. A diff that changes a *number* here fails review. |

New M2 gate tests (P6): `tests/adwaita_coverage.rs` (light + dark + hc: 0
unparseable declarations, 0 unknown properties, 37/37 `@define-color`s) and
`tests/gtk4_property_reference.rs` (`every_gtk4_property_is_registered`: the
vendored GTK 4.22 name list matches `PROPERTIES` on name, kind and inherited flag).

---

## 11. Part boundaries

Dependency order: **P1 → P2 → P3 → {P4, P5, P6}**. P4/P5/P6 are mutually
independent once P3 lands.

### P1 — registry, values, parse (@keyframes/@media)
**Owns:** `ui/Cargo.toml`, `css/registry.rs`, `css/tokens.rs`, `css/value/**`,
`css/parse.rs`. Deletes `css/colors.rs`, `css/shorthand.rs`, `css/value.rs`.
**Implements:** §1 (all), §2 (all), §4 `MediaEnv`/`MediaQuery`/`MediaBlock`/
`KeyframesRule`/`Stylesheet`, `ColorTable`, `build_color_table`.
**Consumes:** nothing. **Must not touch:** `cascade.rs`, `computed.rs`,
`select.rs`, `layout.rs`, `paint*`, `text.rs`, `anim/`.
**Gate:** every `ParseFn` has a never-panic battery; `parse.rs`'s 16 tests stay
byte-identical; `value/color.rs` reproduces colors.rs's 9 surviving tests.

### P2 — node tree & selectors
**Owns:** `css/node.rs`, `css/select.rs`.
**Implements:** §3 (all), §4's `RuleBuckets`, `MatchCx`.
**Consumes:** §1 (`Prop` for bucketing only). **Must not touch:** value parsing,
cascade, computed.
**Gate:** the 33-rule sibling regression; `add_element_unique_hashes` returns
`true`; select.rs's 9 M1 tests preserved.

### P3 — cascade, computed, inheritance, M1 migration
**Owns:** `css/cascade.rs`, `css/computed.rs`, `widget/button.rs`, `app.rs`
(fixture migration only).
**Implements:** §5 (all), §10.1 and §10.3's rewrites for colors/shorthand/
cascade/computed/select/button/app.
**Consumes:** §1, §2, §3, §4.
**Gate:** 900 rules; every Adwaita byte in computed.rs's 22 tests; Decision 6
behaviour proven both ways (inherited property → inherited, non-inherited →
initial).

### P4 — layout & paint
**Owns:** `layout.rs`, `paint/**` (replacing `paint.rs`).
**Implements:** §7 (all), §8 (all), `BackgroundLayer` production side.
**Consumes:** §1, §2, §3, §5, §9 (`ShapedText`, `FontDatabase` — may stub
against `probe_only()` until P6 lands).
**Gate:** paint.rs's 4 rewritten tests; per-family pixel tests under the
exactness rule; `tests/themed_button_offscreen.rs` byte-identical numbers.

### P5 — animation
**Owns:** `anim/**`; adds the frame-callback pump in `wayland.rs` and the
`Overrides` hand-off in `widget/button.rs`/`app.rs`.
**Implements:** §6 (all), `Value::interpolate` wiring through
`Prop::interpolator`.
**Consumes:** §1, §2 (`interpolate`, `TimingFunction`, `Keyframes`), §5.
**Must not touch:** paint internals (it produces `Overrides`, nothing more).
**Gate:** manual-clock exactness (linear midpoint at 100 ms of 200 ms, `ease`
at t=0.5, reversal continuity, `alternate`, `fill-mode: forwards`, Adwaita's
button `transition` at 100 ms); one Wayland start/end screencopy capture.

### P6 — fonts/text, coverage gate, docs
**Owns:** `text.rs`, `tests/adwaita_coverage.rs`,
`tests/gtk4_property_reference.rs`, the vendored `Default-dark.css` /
`Default-hc.css` / GTK-4.22 property-name fixture, `ui/README.md`.
**Implements:** §9 (all), the §7 gate instruments.
**Consumes:** §1 (`PROPERTIES` for the reference test), §2, §5, §8.
**Gate:** 0 unparseable / 0 unknown / 37 colours on all three sheets;
`every_gtk4_property_is_registered`; `cargo test -p icedtea-ui`, workspace
tests, `clippy -D warnings`, `cargo fmt --all --check`.

---

## 12. Execution notes (consistency pass, 2026-08-26)

Added by the cross-part consistency check after all six part plans were written.
Each note records a discrepancy the plans could not resolve among themselves and
the **ruling** an executor follows. A ruling here outranks the part plan it
contradicts; where a part plan's own "Contract deviations" section is correct, the
ruling simply ratifies it.

### E1 — Nobody deletes `css/colors.rs` and `css/shorthand.rs`. **P3 does.**

§11 tells P1 to delete both. P1 deviation 1 correctly refuses (M1's
`cascade.rs`/`computed.rs`, which P1 may not touch, still `use` them) and defers
the deletion to P3. But P3 deviation 11 lists both as *already deleted* at P3's
entry, and P3's File Structure has no deletion row — so as written, the two files
survive M2 with a second, stale copy of the colour grammar and the string
shorthand expander.

**Ruling.** P3 deletes `ui/src/css/colors.rs` and `ui/src/css/shorthand.rs`, and
removes their `pub mod` lines from `ui/src/css/mod.rs`, in the same task that
rewrites `cascade.rs`/`computed.rs` (Task 2 or Task 3, whichever first drops the
last `use`). P3 deviation 11's entry condition "`css::colors` and `css::shorthand`
deleted" is **wrong**; read it as "deleted by P3, before P3's first commit lands".
P1 deviation 1 stands otherwise: P1 only repoints `css/shorthand.rs:14`'s `use`
from `super::value` to `super::tokens` (its own file list has been corrected).
The §10.3 rewrites of those two files' 10 + 9 tests remain **P1's**, into
`value/color.rs` and `value/shorthand.rs`; P3 deletes the originals, it does not
re-port them.

### E2 — Nobody deletes `CssNode`, `NodeData` and `select::PseudoStates`. **P3 does.**

P2 deviation 2 keeps all three alive and says "Part 3 deletes `CssNode`, the M1
struct and the alias in one move". P3's Global Constraints then forbid P3 from
touching `css/select.rs` ("(P2)"), and P3's File Structure omits it.

**Ruling.** P3 **may and must** edit `ui/src/css/select.rs` for exactly one
purpose: deleting M1's `CssNode`, `NodeData`, `select::PseudoStates`, their
`impl Element for CssNode`, their tests, and the
`use crate::css::node::PseudoStates as NodeStates;` alias (the flags are then
imported under their own name). P3 changes **no** matching, bucketing,
`GtkSelectorImpl`, `GtkPseudoClass` or `MatchCx` logic, and no assertion in any of
P2's 29 select tests. Read P3's "must not touch `css/select.rs`" as "must not
change P2's matching layer".

### E3 — `ui/src/wayland.rs` migration is unowned. **P3 does the types, P4 the call sites.**

`wayland.rs` uses M1's `select::PseudoStates` in production (line 24 import, the
`update_states` literal at ~183, `states().hover`), constructs a `CssNode` in its
test `state()` helper (~669), and calls `button.restyle(&sheet, &fonts)` (~297,
~671) and `button.render(&mut self.skia, (0.0, 0.0))` (~422). P3 deviation 11
asserts P2 already moved all of this; P2 deviation 2 says it did not. P3's and
P4's File Structures both omit `wayland.rs`. P5 is the only part that lists it,
and only for the frame pump.

**Ruling.**
- **P3** moves `wayland.rs` onto `css::node::{Node, PseudoStates}`: the import, the
  `PseudoStates { hover, active, .. }` literal (becomes
  `PseudoStates::HOVER | PseudoStates::ACTIVE`, set conditionally), the two
  `states().hover` / `.active` reads (become `.contains(PseudoStates::HOVER)`), and
  the test module's `state()` helper (`CssNode::new(..)` →
  `Node::with_classes("window", &["background"])`).
- **P4** updates `wayland.rs`'s two `restyle` call sites and the one `render` call
  site to P4's new signatures (`fonts: &mut FontDatabase`; `render(&mut self,
  surface, origin, sheet, fonts, overrides)`), because P4 is the part that changes
  them. P5 then edits the same `repaint` body for the frame pump.
- §10.2's "`src/wayland.rs` — all **11**" pins the **eleven `#[test]` bodies**. The
  module's imports, its `state()` helper and its production code are explicitly
  **not** covered; no assertion, constant or tolerance in the eleven tests may
  change. Same reading for `src/shm.rs` and `src/lib.rs` (neither needs an edit).

### E4 — §10.2's `tests/layer_shell_screencopy.rs` pin is amended; P6's Task 12 diff check must be relaxed.

P4 deviation 2 is **ratified**: the file's only permitted edit is the single
`use icedtea_ui::layout::Allocation;` line, retargeted at
`support::PrintedAllocation`. Every constant, every assertion and the ±12
tolerance derivation stay byte-identical.

P6's Task 12 Step 1 as written demands **no diff at all** in
`ui/tests/layer_shell_screencopy.rs` and `ui/src/wayland.rs`. Both will legitimately
differ (E3, and P5 Task 13's pump plus appended tests).

**Ruling.** P6 Task 12's expectation becomes:
- `ui/src/lib.rs`, `ui/src/shm.rs` — **no diff at all**.
- `ui/tests/layer_shell_screencopy.rs` — a diff of exactly one `use` line.
- `ui/src/wayland.rs` — a diff confined to imports, `update_states`, `repaint`,
  the frame-pump additions, the `state()` helper and appended tests; the eleven M1
  `#[test]` bodies unchanged.
- `ui/tests/themed_button_offscreen.rs` — mechanical only, and the "every literal
  that leaves on a `-` line comes back on a `+` line" number check is unchanged and
  still binding.

### E5 — `Button::render`'s `overrides` argument: P5 owns it, P4 does not parameterise it.

P4 gives `Button::render` a caller-supplied `overrides: Option<&Overrides>`
parameter that every P4 call site passes `None` for; P5 instead has `Button` own
its `AnimationState` and expects `render` to read `self.overrides`. Keeping both
yields a shadowed, always-`None` parameter — and `button.render(.., Some(button.overrides()))`
cannot be written at all (`&mut self` plus `&self`).

**Ruling.** P4 keeps the parameter (it needs *some* overrides argument to reach
`paint_node`, and P5 has not landed). **P5 removes it** and passes
`Some(&self.overrides)` from the field, so `render` ends M2 as
`render(&mut self, surface: &mut Surface, origin: (f32, f32), sheet: &CompiledSheet,
fonts: &mut FontDatabase)`. P5 updates the two call sites (`wayland.rs::repaint`
and its own tests). P5's Task 12 has been corrected in place to say so.

### E6 — P5's consumed `Button` signatures are M1's/P3's, not P4's.

P5 Task 12 lists `restyle(&mut self, sheet, fonts: &FontStack)` and
`render(&self, surface, origin)`, and its test bodies call
`button.render(&mut start, (0.0, 0.0))` and `button.restyle(&sheet, &fonts)`.
Execution order is P3 → P4 → P5, so by the time P5 runs those are
`restyle(&mut self, sheet, fonts: &mut FontDatabase)` and P4's six-argument
`render`. **Ruling:** P5 adapts every call site mechanically (`&mut fonts`; the
extra `&sheet, &mut fonts` arguments) and changes nothing in the `anim` API. P5's
Consumes block has been corrected in place; its inline test snippets have not, and
are to be read through that correction.

### E7 — §10.3's `app.rs` row is **8 of 11 rewritten / 3 byte-identical**.

P3 deviation 3 is **ratified**: eight `app.rs` tests read a `ComputedStyle` field
through `button_style`/`headerbar_min_height`; only
`an_empty_xdg_config_home_falls_back_to_home_dot_config`,
`no_home_and_no_xdg_means_no_override_path` and
`theme_dirs_are_searched_in_gtk_order` are byte-identical.
`the_bundled_source_is_exactly_the_vendored_sheet` reads `border_color` and
therefore does **not** join §10.2's gate; its `900` constant is preserved instead.
P6's Global Constraints said "app.rs's 7 byte-identical ones" and has been
corrected in place to 3.

### E8 — P3 re-implements P1's parse/keyframe helpers privately; `Stylesheet::append_layer` has no caller.

P1 deviation 7 adds three public helpers "later parts consume":
`registry::parse_declaration_value(prop, text)`, `Keyframes::compile(&KeyframesRule)`
and `Stylesheet::append_layer(&mut self, layer: Stylesheet)`. P3 references none of
them: it writes a private `parse_declaration(prop, value, sink)` and a private
`compile_keyframes(rule)` in `cascade.rs`, and leaves `app.rs`'s layering untouched.

**Ruling.**
- P3's private `compile_keyframes` is **replaced by** `Keyframes::compile`; the
  compiled-keyframe grammar lives in one place. P3's private `parse_declaration`
  may stay (it also drives the shorthand `expand_into` fan-out that
  `parse_declaration_value` does not), but it **calls**
  `registry::parse_declaration_value` for the longhand case rather than
  re-deriving it.
- `Stylesheet::append_layer` **must** be used: P3's `app.rs` task is extended to
  route `load_layered_stylesheet`'s merge through it, so a user theme's
  `@keyframes` and `@media` blocks are no longer silently dropped. This is a
  production-code change in `app.rs` — P3 deviation-free, and the only one; the
  three byte-identical `app.rs` tests are unaffected.
- If, after this, any of the three helpers still has no caller, it is deleted
  rather than shipped as unused public surface.

### E9 — `Keyframes::segment` (§2.9) has no consumer, by design.

P5 Task 5 explains why: `segment` cannot synthesise the missing `0%`/`100%`
endpoints CSS Animations L1 requires, because it has no access to the underlying
computed style, so P5 uses
`anim::keyframes::resolve_segment(frames, prop, q, base)` instead. **Ruling:**
`Keyframes::segment` is dropped from the contract unless P1's own tests are its
only caller — P1 either keeps it *and* tests it as a standalone bracketing helper,
or removes it. `Keyframes::properties` stays; P5 consumes it.

### E10 — Ratified additive amendments to §1 / §2 that later parts already rely on.

These come from P1's deviations 3-6 and are **binding on every part**, because
P4 and P6 already consume them:

- `Keyword` gains `Stretch`, `Fill`, `XxSmall`, `XSmall`, `Small`, `Large`,
  `XLarge`, `XxLarge`, `XxxLarge`, `Larger`, `Smaller` (11 variants). **P4 uses
  `Keyword::Stretch`** for `border-image-repeat`.
- `Length::parse` rejects `auto`; `Length::parse_allowing_auto` accepts it
  (`margin-*`, `border-image-width`).
- `CalcNode` gains `Var(u8)` and `CalcNode::resolve_number_with(&self, vars: &[f32; 4])`,
  without which 8 of Adwaita's 37 `@define-color`s (relative colour syntax with
  `calc()` over channel variables) cannot resolve and the 37/37 gate is unreachable.
- `transition-property` items parse to `Keyword(All)` / `Keyword(None)` /
  `AnimationName(Named(..))` — P1 deviation 6 and P3 deviation 7 agree; this is the
  frozen shape.
- `ui/src/css/value/shorthand.rs` (P1, the 18 `ExpandFn`s) is a legitimate addition
  to §0's `value/` file list.

### E11 — `paint/mod.rs` re-exports `BackgroundLayer`.

P3 deviation 6 (ratified: `BackgroundLayer` is defined in `css/computed.rs`, next
to `background_layers()`, its only producer) says P4's `paint/mod.rs` re-exports it
so §8's signatures still read `BackgroundLayer`. P4 imports it but does not
re-export. **Ruling:** P4 adds `pub use crate::css::computed::BackgroundLayer;` to
`ui/src/paint/mod.rs`.

### E12 — Everything else checked and clean.

- Every §1-§9 contract item has exactly **one** implementing part; no item is
  implemented twice. `BackgroundLayer` (P3 produces, P4 consumes),
  `Overrides`/`TransitionSpec`/`AnimationSpec` (P3 produces the three structs, P5
  the machinery) and `text.rs` (P4 shim, P6 real) are the three deliberate
  producer/consumer splits, and all three are acknowledged on both sides.
- No two parts **create** the same file. `ui/src/anim/mod.rs` is created by P3 and
  re-entered idempotently by P5 (P3 deviation 5 / P5 deviation 1 agree);
  `ui/src/text.rs`, `ui/src/widget/button.rs`, `ui/src/layout.rs`,
  `ui/src/paint.rs`, `ui/src/app.rs`, `ui/src/lib.rs`, `ui/src/css/mod.rs` and
  `ui/tests/themed_button_offscreen.rs` are modified by more than one part, always
  in dependency order and always acknowledged.
- Execution order 1 → 6 never requires a later part's symbol earlier, once E1-E3
  are applied: P2's `RuleBuckets::build` reads M1's `CompiledRule` (same shape as
  P3's), P3 creates the `anim` types it returns, P4 creates the `text.rs` surface
  it consumes, and P5/P6 consume only what P1-P4 produced.
- Signature agreement was verified name-by-name for the whole §8 paint surface,
  the §5 `ComputedStyle` accessors as P4 calls them, `ShapeKey`/`ShapedText`/
  `FontQuery`/`FontFace`/`FontDatabase` between P4's shim and P6's rewrite,
  `Stylesheet`/`KeyframesRule`/`MediaBlock` between P1 and P3, and the whole §6
  `anim` surface between P3 and P5. All match.
- The §10.3 rewrite rows map to exactly one owner each: colors 10 + shorthand 9 →
  P1; select 9 → P2; cascade 10 + computed 22 + button 4 + app 8 → P3;
  layout 6 + paint 4 → P4; text 5 → P6. `tests/themed_button_offscreen.rs`'s 4 are
  touched twice by design — P3 keeps them compiling against the new accessors
  (P3 deviation 1), P4 finishes the rewrite onto `paint_node`/`Allocation` and owns
  the gate — with every number byte-identical through both.
- `113` / `95` longhands / `18` shorthands, `900` compiled Adwaita rules and
  `37/37` `@define-color`s are used consistently in P1, P3 and P6.
