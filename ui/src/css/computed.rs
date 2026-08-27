//! Computed values for the properties the M1 slice paints.
//!
//! Deliberately narrow: `background-color`, `background-image`, `color`,
//! `border`/`border-width`/`border-color`/`border-radius`, `padding`,
//! `min-width`, `min-height`, `font-size`. Everything else in a real theme
//! is parsed (Task 2) and cascaded (Task 5) but not yet interpreted.
//!
//! `background` is an enum rather than a `Color` because Adwaita's buttons
//! have no `background-color` at all -- every button background is a
//! `background-image`, either GTK's `image(<color>)` flat-fill extension or
//! a two-stop `linear-gradient(to top, ...)`.

use std::rc::Rc;

use skia_rs_safe::core::Color;

use super::cascade::{CascadedValues, CompiledSheet, cascade};
use super::node::Node;
use super::registry::{self, N_LONGHANDS, Prop};
use super::select::MatchCx;
use super::value::color::{ColorCtx, ColorTable, ColorValue, Rgba};
use super::value::image::ColorStop;
use super::value::{
    AnimationName, BgSize, CalcNode, FilterFn, FontFamily, FontStyle, FontWeight, Gradient, Image,
    IterationCount, Keyword, Length, LengthCtx, LengthUnit, LineHeight, Position, Shadow, Time,
    TimingFunction, TransformFn, Value, Wide,
};

/// One gradient stop: a color and an optional absolute position, measured in
/// pixels along the gradient line from its origin.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GradientStop {
    /// The stop's color.
    pub color: Color,
    /// Distance in px from the gradient line's origin, or `None` for the
    /// implicit position (0 for the first stop, the full length for the last).
    pub position_px: Option<f32>,
}

/// A resolved background.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Background {
    /// Nothing painted.
    Transparent,
    /// A flat fill -- `background-color`, or GTK's `image(<color>)`.
    Solid(Color),
    /// `linear-gradient(to top, <stop>, <stop>)`. The gradient line runs
    /// from the box's bottom edge (`from`) to its top edge (`to`).
    LinearGradientToTop {
        /// The stop at the gradient line's origin (the bottom edge).
        from: GradientStop,
        /// The stop at the gradient line's end (the top edge).
        to: GradientStop,
    },
}

/// Round-to-nearest linear interpolation between two bytes.
///
/// Deliberately *not* `Color4f::lerp`: that round-trips each channel
/// through `x / 255.0` and back, so it is not bit-identical to rounding the
/// byte-space interpolation once. The offscreen gate asserts exact pixel
/// equality against values derived straight from the theme's declarations,
/// which only this form guarantees.
fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    let a = f32::from(a);
    let b = f32::from(b);
    (a + (b - a) * t).round().clamp(0.0, 255.0) as u8
}

impl Background {
    /// The color this background paints at `y_from_top` in a box `height` tall.
    ///
    /// `y_from_top` is the painter's coordinate; `to top` gradients are
    /// sampled along a line whose origin is the *bottom* edge, so this
    /// converts. Positions before the first stop and after the last clamp to
    /// that stop's color, per CSS's gradient-stop rules.
    #[must_use]
    pub fn color_at(&self, height: f32, y_from_top: f32) -> Color {
        match self {
            Self::Transparent => Color::TRANSPARENT,
            Self::Solid(color) => *color,
            Self::LinearGradientToTop { from, to } => {
                let line = height.max(1.0);
                let p0 = from.position_px.unwrap_or(0.0);
                let p1 = to.position_px.unwrap_or(line);
                let y = (line - y_from_top).clamp(0.0, line);
                if y <= p0 || (p1 - p0).abs() < f32::EPSILON {
                    return from.color;
                }
                if y >= p1 {
                    return to.color;
                }
                let t = (y - p0) / (p1 - p0);
                Color::from_argb(
                    lerp_u8(from.color.alpha(), to.color.alpha(), t),
                    lerp_u8(from.color.red(), to.color.red(), t),
                    lerp_u8(from.color.green(), to.color.green(), t),
                    lerp_u8(from.color.blue(), to.color.blue(), t),
                )
            }
        }
    }
}

/// Which box a background is clipped to.
///
/// CSS's -- and GTK's -- default is `border-box`: the background fills the
/// whole element and the border is painted over it, which is what makes a
/// translucent border show the element's own background rather than what is
/// behind it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BackgroundClip {
    /// The whole element, border included. The default.
    #[default]
    BorderBox,
    /// Inside the border.
    PaddingBox,
    /// Inside the border and the padding.
    ContentBox,
}

impl BackgroundClip {
    /// Parse a `background-clip` keyword, ASCII-case-insensitively.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        if value.eq_ignore_ascii_case("border-box") {
            Some(Self::BorderBox)
        } else if value.eq_ignore_ascii_case("padding-box") {
            Some(Self::PaddingBox)
        } else if value.eq_ignore_ascii_case("content-box") {
            Some(Self::ContentBox)
        } else {
            None
        }
    }
}

/// The environment a computed style resolves against: the values GTK reads
/// from the desktop rather than from CSS.
#[derive(Copy, Clone, Debug)]
pub struct ResolveEnv {
    /// Screen resolution, seeding `-gtk-dpi`. Physical units (`pt pc in cm mm`)
    /// convert through this, **not** CSS's fixed 96.
    pub dpi: f32,
    /// The initial `font-size`, which is what `rem` resolves against in GTK --
    /// not the root node's computed size, as CSS would have it.
    pub root_font_size: f32,
}

impl Default for ResolveEnv {
    fn default() -> Self {
        Self {
            dpi: 96.0,
            root_font_size: ComputedStyle::DEFAULT_FONT_SIZE,
        }
    }
}

/// Read a computed `Value` as a concrete type.
///
/// A type mismatch is never a panic: it yields that type's initial-equivalent
/// and logs at debug. The table is indexed by the registry, so a mismatch means
/// a property's `ParseFn` and its consumer disagree -- a bug to find in the log,
/// not a reason to take the process down mid-paint.
pub trait FromValue: Sized {
    /// Read `value`, or this type's initial-equivalent if it is the wrong shape.
    fn from_value(value: &Value) -> Self;
}

/// The x-height/em ratio used before a face is resolved. The real ratio comes
/// from the matched face (P6); `ex` appears in no GTK theme this crate ships.
const EX_RATIO: f32 = 0.5;

/// Every longhand's computed value, indexed by `Prop::slot()`.
///
/// The ten M1 scalar fields are still here and still derived on every resolve;
/// Task 9 deletes them once `layout`, `paint` and the offscreen gate read the
/// typed accessors instead.
#[derive(Clone, Debug, PartialEq)]
pub struct ComputedStyle {
    values: Box<[Value; N_LONGHANDS]>,
    /// Resolved background (M1 view; deleted in Task 9).
    pub background: Background,
    /// Foreground (text) color (M1 view; deleted in Task 9).
    pub color: Color,
    /// Uniform border width in px (M1 view; deleted in Task 9).
    pub border_width: f32,
    /// Border color (M1 view; deleted in Task 9).
    pub border_color: Color,
    /// Uniform corner radius in px (M1 view; deleted in Task 9).
    pub border_radius: f32,
    /// `[top, right, bottom, left]` in px (M1 view; deleted in Task 9).
    pub padding: [f32; 4],
    /// `min-width` in px (M1 view; deleted in Task 9).
    pub min_width: f32,
    /// `min-height` in px (M1 view; deleted in Task 9).
    pub min_height: f32,
    /// Font size in px (M1 view; deleted in Task 9).
    pub font_size: f32,
    /// Which box the background is clipped to (M1 view; deleted in Task 9).
    pub background_clip: BackgroundClip,
}

impl ComputedStyle {
    /// The font size used when no rule sets one.
    ///
    /// No Adwaita `button` rule declares `font-size`; GTK inherits it from
    /// the desktop's font setting, which M1 does not read.
    pub const DEFAULT_FONT_SIZE: f32 = 14.0;

    /// Every longhand at its registry initial, with `-gtk-dpi` and `font-size`
    /// seeded from `env` so a chain's root inherits the environment.
    #[must_use]
    pub fn initial(env: &ResolveEnv) -> Rc<ComputedStyle> {
        let mut values: Box<[Value; N_LONGHANDS]> =
            Box::new(std::array::from_fn(|_| Value::Keyword(Keyword::None)));
        for prop in registry::longhands() {
            values[prop.slot()] = prop.initial();
        }
        values[Prop::GtkDpi.slot()] = Value::Number(env.dpi);
        values[Prop::FontSize.slot()] = Value::Length(Length::px(env.root_font_size));
        let mut style = Self {
            values,
            background: Background::Transparent,
            color: Color::BLACK,
            border_width: 0.0,
            border_color: Color::TRANSPARENT,
            border_radius: 0.0,
            padding: [0.0; 4],
            min_width: 0.0,
            min_height: 0.0,
            font_size: env.root_font_size,
            background_clip: BackgroundClip::default(),
        };
        style.derive_m1_view();
        Rc::new(style)
    }

    /// This longhand's computed value.
    ///
    /// # Panics
    ///
    /// If `prop` is a shorthand: shorthands have no computed slot.
    #[must_use]
    pub fn raw(&self, prop: Prop) -> &Value {
        assert!(prop.is_longhand(), "{} is a shorthand", prop.name());
        &self.values[prop.slot()]
    }

    /// This longhand's computed value, read as `T`.
    #[must_use]
    pub fn get<T: FromValue>(&self, prop: Prop) -> T {
        T::from_value(self.raw(prop))
    }
}

/// Used-value accessors: the computed table, read the way layout, paint and
/// text want it -- in device pixels, per side, with percentages resolved
/// against the basis the caller actually has.
impl ComputedStyle {
    /// The computed `font-size`, in px.
    #[must_use]
    pub fn font_size_px(&self) -> f32 {
        let size = self.get::<f32>(Prop::FontSize);
        if size.is_finite() && size >= 0.0 {
            size
        } else {
            Self::DEFAULT_FONT_SIZE
        }
    }

    /// The computed `-gtk-dpi`. Physical units convert through this.
    #[must_use]
    pub fn dpi(&self) -> f32 {
        let dpi = self.get::<f32>(Prop::GtkDpi);
        if dpi.is_finite() && dpi > 0.0 {
            dpi
        } else {
            ResolveEnv::default().dpi
        }
    }

    /// A length context for resolving whatever survived computed time.
    ///
    /// Only percentages do, so `root_font_size_px` is inert here; it is taken
    /// from `env` anyway so a caller with a non-default environment stays
    /// self-consistent.
    #[must_use]
    pub fn length_ctx(&self, env: &ResolveEnv, percent_basis: Option<f32>) -> LengthCtx {
        LengthCtx {
            font_size_px: self.font_size_px(),
            root_font_size_px: env.root_font_size,
            ex_ratio: EX_RATIO,
            dpi: self.dpi(),
            percent_basis,
        }
    }

    /// The default-environment context the accessors below use.
    fn used_ctx(&self, percent_basis: Option<f32>) -> LengthCtx {
        self.length_ctx(&ResolveEnv::default(), percent_basis)
    }

    /// One longhand as a used length in px, or `None` if it is not a length.
    fn length_px(&self, prop: Prop, basis: Option<f32>) -> Option<f32> {
        match self.raw(prop) {
            Value::Length(length) => length
                .resolve(&self.used_ctx(basis))
                .filter(|px| px.is_finite()),
            Value::Number(number) if *number == 0.0 => Some(0.0),
            _ => None,
        }
    }

    /// The computed `color`.
    #[must_use]
    pub fn color(&self) -> Rgba {
        self.get::<Rgba>(Prop::Color)
    }

    /// The computed `opacity`, clamped into `0..=1` as CSS requires.
    #[must_use]
    pub fn opacity(&self) -> f32 {
        let opacity = self.get::<f32>(Prop::Opacity);
        if opacity.is_finite() {
            opacity.clamp(0.0, 1.0)
        } else {
            1.0
        }
    }

    /// `padding-*` in px, `[top, right, bottom, left]`. `basis` is the
    /// containing block's width: CSS resolves every padding percentage,
    /// vertical ones included, against the width.
    #[must_use]
    pub fn padding(&self, basis: f32) -> [f32; 4] {
        const SIDES: [Prop; 4] = [
            Prop::PaddingTop,
            Prop::PaddingRight,
            Prop::PaddingBottom,
            Prop::PaddingLeft,
        ];
        let mut padding = [0.0; 4];
        for (slot, prop) in SIDES.into_iter().enumerate() {
            padding[slot] = self.length_px(prop, Some(basis)).unwrap_or(0.0).max(0.0);
        }
        padding
    }

    /// `margin-*` in px, `[top, right, bottom, left]`; `None` is `auto`.
    /// Negative margins are legal and are **not** clamped.
    #[must_use]
    pub fn margin(&self, basis: f32) -> [Option<f32>; 4] {
        const SIDES: [Prop; 4] = [
            Prop::MarginTop,
            Prop::MarginRight,
            Prop::MarginBottom,
            Prop::MarginLeft,
        ];
        let mut margin = [Some(0.0); 4];
        for (slot, prop) in SIDES.into_iter().enumerate() {
            margin[slot] = match self.raw(prop) {
                Value::Length(Length::Auto) => None,
                _ => Some(self.length_px(prop, Some(basis)).unwrap_or(0.0)),
            };
        }
        margin
    }

    /// `min-width`/`min-height` as a **content-box** minimum, in px.
    ///
    /// GTK's minimums floor the content box, not the border box -- the M1
    /// ruling that makes Adwaita's empty button 36x34 rather than 20x27.
    #[must_use]
    pub fn min_size(&self, basis: (f32, f32)) -> (f32, f32) {
        (
            self.length_px(Prop::MinWidth, Some(basis.0))
                .unwrap_or(0.0)
                .max(0.0),
            self.length_px(Prop::MinHeight, Some(basis.1))
                .unwrap_or(0.0)
                .max(0.0),
        )
    }
}

impl Default for ComputedStyle {
    fn default() -> Self {
        (*Self::initial(&ResolveEnv::default())).clone()
    }
}

/// `f32`: numbers, percentages as their 0..1 fraction, absolute lengths in px,
/// and times in seconds. Anything else is `0.0`.
impl FromValue for f32 {
    fn from_value(value: &Value) -> Self {
        match value {
            Value::Number(number) => *number,
            Value::Percentage(fraction) => *fraction,
            Value::Angle(degrees) => *degrees,
            Value::Time(time) => time.as_secs_f32(),
            Value::Length(Length::Abs {
                value,
                unit: LengthUnit::Px,
            }) => *value,
            Value::Length(Length::Percent(fraction)) => *fraction,
            other => {
                tracing::debug!(value = ?other, "value is not a number; reading 0.0");
                0.0
            }
        }
    }
}

impl FromValue for Rgba {
    fn from_value(value: &Value) -> Self {
        match value {
            Value::Color(ColorValue::Absolute(rgba)) => *rgba,
            other => {
                tracing::debug!(value = ?other, "value is not a resolved colour; reading transparent");
                Rgba::TRANSPARENT
            }
        }
    }
}

impl FromValue for Keyword {
    fn from_value(value: &Value) -> Self {
        match value {
            Value::Keyword(keyword) => *keyword,
            Value::List(items) => match items.first() {
                Some(Value::Keyword(keyword)) => *keyword,
                _ => Keyword::None,
            },
            _ => Keyword::None,
        }
    }
}

impl FromValue for bool {
    /// `none` is false; any other keyword, and any non-zero number, is true.
    fn from_value(value: &Value) -> Self {
        match value {
            Value::Keyword(Keyword::None) => false,
            Value::Keyword(_) => true,
            Value::Number(number) => *number != 0.0,
            _ => false,
        }
    }
}

impl FromValue for Length {
    fn from_value(value: &Value) -> Self {
        match value {
            Value::Length(length) => length.clone(),
            _ => Length::zero(),
        }
    }
}

impl FromValue for Time {
    fn from_value(value: &Value) -> Self {
        match value {
            Value::Time(time) => *time,
            _ => Time(0.0),
        }
    }
}

impl FromValue for Image {
    fn from_value(value: &Value) -> Self {
        match value {
            Value::Image(image) => image.clone(),
            Value::List(items) => match items.first() {
                Some(Value::Image(image)) => image.clone(),
                _ => Image::None,
            },
            _ => Image::None,
        }
    }
}

impl FromValue for TimingFunction {
    fn from_value(value: &Value) -> Self {
        match value {
            Value::Timing(timing) => *timing,
            _ => TimingFunction::EASE,
        }
    }
}

impl FromValue for LineHeight {
    fn from_value(value: &Value) -> Self {
        match value {
            Value::LineHeight(line_height) => line_height.clone(),
            _ => LineHeight::Normal,
        }
    }
}

impl FromValue for FontWeight {
    fn from_value(value: &Value) -> Self {
        match value {
            Value::FontWeight(weight) => *weight,
            Value::Number(number) => FontWeight::Absolute(*number),
            _ => FontWeight::Absolute(400.0),
        }
    }
}

impl FromValue for FontStyle {
    fn from_value(value: &Value) -> Self {
        match value {
            Value::FontStyle(style) => *style,
            _ => FontStyle::Normal,
        }
    }
}

impl FromValue for AnimationName {
    fn from_value(value: &Value) -> Self {
        match value {
            Value::AnimationName(name) => name.clone(),
            _ => AnimationName::None,
        }
    }
}

impl FromValue for IterationCount {
    fn from_value(value: &Value) -> Self {
        match value {
            Value::IterationCount(count) => *count,
            Value::Number(number) => IterationCount::Count(*number),
            _ => IterationCount::Count(1.0),
        }
    }
}

impl FromValue for Rc<[FontFamily]> {
    fn from_value(value: &Value) -> Self {
        match value {
            Value::FontFamilies(families) => Rc::clone(families),
            _ => Rc::from(Vec::new()),
        }
    }
}

impl FromValue for Rc<[Shadow]> {
    fn from_value(value: &Value) -> Self {
        match value {
            Value::Shadow(shadow) => Rc::from(vec![shadow.clone()]),
            Value::List(items) => Rc::from(
                items
                    .iter()
                    .filter_map(|item| match item {
                        Value::Shadow(shadow) => Some(shadow.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>(),
            ),
            _ => Rc::from(Vec::new()),
        }
    }
}

/// The M1 bridge: read a computed-shaped scalar out of a parsed `Value`.
///
/// Task 3 replaces every one of these with the registry-indexed table and
/// deletes this module. It exists so the cascade can move to `Prop` in one
/// commit while `ComputedStyle`'s M1 fields -- and all of its tests -- stay
/// exactly as they are, which is what makes them a regression bar rather than a
/// thing to be rewritten on trust.
mod m1_bridge {
    use super::{Background, BackgroundClip, GradientStop};
    use crate::css::value::color::{ColorCtx, ColorTable, ColorValue, Rgba};
    use crate::css::value::image::{
        ColorStop, Gradient, GradientKind, LinearDirection, SideOrCorner,
    };
    use crate::css::value::{Image, Keyword, Length, LengthCtx, Value};
    use skia_rs_safe::core::Color;

    /// A length context with M1's fixed assumptions: 14px `rem` base, 96 dpi,
    /// and no percentage basis unless the caller supplies one.
    pub fn ctx(font_size_px: f32, percent_basis: Option<f32>) -> LengthCtx {
        LengthCtx {
            font_size_px,
            root_font_size_px: super::ComputedStyle::DEFAULT_FONT_SIZE,
            ex_ratio: 0.5,
            dpi: 96.0,
            percent_basis,
        }
    }

    /// A `<length>` in px. A percentage with no basis is uninterpretable, which
    /// is what keeps `border-radius: 100%` falling through to its runner-up.
    pub fn px(value: &Value, ctx: &LengthCtx) -> Option<f32> {
        match value {
            Value::Length(length) => length.resolve(ctx).filter(|px| px.is_finite()),
            Value::Number(number) if *number == 0.0 => Some(0.0),
            _ => None,
        }
    }

    /// The horizontal half of a corner radius.
    pub fn radius(value: &Value, ctx: &LengthCtx) -> Option<f32> {
        match value {
            Value::Pair(pair) => px(&pair.0, ctx),
            other => px(other, ctx),
        }
    }

    /// A `<color>`, with `currentColor` mapped onto `current`.
    pub fn color(value: &Value, colors: &ColorTable, current: Color) -> Option<Color> {
        let Value::Color(color) = value else {
            return None;
        };
        resolve(color, colors, current)
    }

    /// `true` when this border style forces a used width of zero.
    pub fn border_style_is_none(value: &Value) -> Option<bool> {
        match value {
            Value::Keyword(Keyword::None | Keyword::Hidden) => Some(true),
            Value::Keyword(_) => Some(false),
            _ => None,
        }
    }

    /// A `background-clip` keyword. The property is comma-multiplied; M1 paints
    /// one layer, so the first entry wins.
    pub fn clip(value: &Value) -> Option<BackgroundClip> {
        match first(value) {
            Value::Keyword(Keyword::BorderBox) => Some(BackgroundClip::BorderBox),
            Value::Keyword(Keyword::PaddingBox) => Some(BackgroundClip::PaddingBox),
            Value::Keyword(Keyword::ContentBox) => Some(BackgroundClip::ContentBox),
            _ => None,
        }
    }

    /// The first entry of a comma-multiplied value, or the value itself.
    fn first(value: &Value) -> &Value {
        match value {
            Value::List(items) => items.first().unwrap_or(value),
            other => other,
        }
    }

    /// `background-image` as an M1 [`Background`].
    ///
    /// `Some(None)` means "interpretable, but paints nothing" (`none`), which
    /// must not fall through to a runner-up image.
    pub fn background(
        value: &Value,
        colors: &ColorTable,
        current: Color,
        ctx: &LengthCtx,
    ) -> Option<Option<Background>> {
        let Value::Image(image) = first(value) else {
            return None;
        };
        match image {
            Image::None => Some(None),
            Image::Solid(fill) => Some(Some(Background::Solid(resolve(fill, colors, current)?))),
            Image::Gradient(gradient) => Some(Some(to_top(gradient, colors, current, ctx)?)),
            _ => None,
        }
    }

    fn resolve(value: &ColorValue, colors: &ColorTable, current: Color) -> Option<Color> {
        let ctx = ColorCtx {
            table: colors,
            current: Rgba::from_color32(current),
            depth: 0,
        };
        value.resolve(&ctx).map(Rgba::to_color32)
    }

    /// A two-stop, non-repeating `linear-gradient(to top, ...)` -- the only
    /// gradient shape M1's `Background` can hold.
    fn to_top(
        gradient: &Gradient,
        colors: &ColorTable,
        current: Color,
        ctx: &LengthCtx,
    ) -> Option<Background> {
        if gradient.repeating {
            return None;
        }
        let GradientKind::Linear {
            direction: LinearDirection::Side(SideOrCorner::Top),
        } = gradient.kind
        else {
            return None;
        };
        let [from, to] = gradient.stops.as_ref() else {
            return None;
        };
        let stop = |stop: &ColorStop| -> Option<GradientStop> {
            Some(GradientStop {
                color: resolve(&stop.color, colors, current)?,
                position_px: match &stop.position {
                    Some(length @ (Length::Abs { .. } | Length::Calc(_))) => {
                        Some(length.resolve(ctx).filter(|px| px.is_finite())?)
                    }
                    Some(_) => return None,
                    None => None,
                },
            })
        };
        Some(Background::LinearGradientToTop {
            from: stop(from)?,
            to: stop(to)?,
        })
    }
}

impl ComputedStyle {
    /// Resolve `node`'s style. `parent` supplies inherited values and the
    /// `em`/`currentColor` bases; `None` means this node is the style root.
    ///
    /// The order is fixed: `-gtk-dpi` first (physical units convert through
    /// it), then `font-size` (whose own `em`/`%` resolve against the *parent's*
    /// size), then `color` (which `currentColor` everywhere else resolves to),
    /// then every remaining longhand in registry order.
    #[must_use]
    pub fn resolve(
        sheet: &CompiledSheet,
        node: &Node,
        parent: Option<&ComputedStyle>,
        env: &ResolveEnv,
        cx: &mut MatchCx,
    ) -> ComputedStyle {
        let root = ComputedStyle::initial(env);
        let parent = parent.unwrap_or(&root);
        let cascaded = cascade(sheet, node, cx);

        let mut values: Box<[Value; N_LONGHANDS]> =
            Box::new(std::array::from_fn(|_| Value::Keyword(Keyword::None)));

        // 1. -gtk-dpi.
        let seed_ctx = LengthCtx {
            font_size_px: parent.font_size,
            root_font_size_px: env.root_font_size,
            ex_ratio: EX_RATIO,
            dpi: env.dpi,
            percent_basis: None,
        };
        let seed_colors = ColorCtx {
            table: &sheet.colors,
            current: Rgba::from_color32(parent.color),
            depth: 0,
        };
        values[Prop::GtkDpi.slot()] =
            compute_one(Prop::GtkDpi, &cascaded, parent, &seed_ctx, &seed_colors);
        let dpi = match &values[Prop::GtkDpi.slot()] {
            Value::Number(number) if number.is_finite() && *number > 0.0 => *number,
            _ => env.dpi,
        };

        // 2. font-size: its own `em`/`%` are against the parent's size.
        let font_ctx = LengthCtx {
            font_size_px: parent.font_size,
            root_font_size_px: env.root_font_size,
            ex_ratio: EX_RATIO,
            dpi,
            percent_basis: Some(parent.font_size),
        };
        values[Prop::FontSize.slot()] =
            compute_one(Prop::FontSize, &cascaded, parent, &font_ctx, &seed_colors);
        let font_size = match &values[Prop::FontSize.slot()] {
            Value::Length(Length::Abs {
                value,
                unit: LengthUnit::Px,
            }) if value.is_finite() && *value >= 0.0 => *value,
            // `font-size: medium` is the initial size; every other keyword form
            // is out of the registry's grammar and is invalid at computed-value
            // time, which for an inherited property means the parent's size.
            Value::Keyword(Keyword::Medium) => env.root_font_size,
            _ => parent.font_size,
        };
        values[Prop::FontSize.slot()] = Value::Length(Length::px(font_size));

        // 3. color: `currentColor` on `color` itself is the inherited colour.
        let length_ctx = LengthCtx {
            font_size_px: font_size,
            root_font_size_px: env.root_font_size,
            ex_ratio: EX_RATIO,
            dpi,
            percent_basis: None,
        };
        values[Prop::Color.slot()] =
            compute_one(Prop::Color, &cascaded, parent, &length_ctx, &seed_colors);
        let own_color = Rgba::from_value(&values[Prop::Color.slot()]);
        let color_ctx = ColorCtx {
            table: &sheet.colors,
            current: own_color,
            depth: 0,
        };

        // 4. everything else, in registry order.
        for prop in registry::longhands() {
            if matches!(prop, Prop::GtkDpi | Prop::FontSize | Prop::Color) {
                continue;
            }
            values[prop.slot()] = compute_one(prop, &cascaded, parent, &length_ctx, &color_ctx);
        }

        let mut style = ComputedStyle {
            values,
            background: Background::Transparent,
            color: own_color.to_color32(),
            border_width: 0.0,
            border_color: Color::TRANSPARENT,
            border_radius: 0.0,
            padding: [0.0; 4],
            min_width: 0.0,
            min_height: 0.0,
            font_size,
            background_clip: BackgroundClip::default(),
        };
        style.derive_m1_view();
        style
    }

    /// Resolve `node` by walking its whole ancestor chain from the root down --
    /// the shape M1's `resolve` had, for callers that hold no parent style.
    #[must_use]
    pub fn resolve_chain(
        sheet: &CompiledSheet,
        node: &Node,
        env: &ResolveEnv,
        cx: &mut MatchCx,
    ) -> ComputedStyle {
        let mut chain = vec![node.clone()];
        while let Some(parent) = chain.last().and_then(Node::parent) {
            chain.push(parent);
        }
        let mut style: Option<ComputedStyle> = None;
        for ancestor in chain.iter().rev() {
            style = Some(Self::resolve(sheet, ancestor, style.as_ref(), env, cx));
        }
        style.expect("the chain always contains the node itself")
    }

    /// Fill the ten M1 scalar fields from the table.
    ///
    /// The M1 tests, `layout`, `paint` and the offscreen gate still read these;
    /// deriving them here means all 22 of `computed.rs`'s pinned Adwaita bytes
    /// are a regression bar on the new table rather than a thing to rewrite on
    /// trust. Task 9 deletes this together with the fields.
    fn derive_m1_view(&mut self) {
        let ctx = m1_bridge::ctx(self.font_size, None);
        let table = ColorTable::new();
        self.color = Rgba::from_value(self.raw(Prop::Color)).to_color32();
        // The table's `background-color` is always a resolved colour, so the
        // M1 "nothing declared" case is now "declared fully transparent" --
        // which paints exactly the same nothing.
        self.background =
            match m1_bridge::color(self.raw(Prop::BackgroundColor), &table, self.color) {
                Some(color) if color.alpha() > 0 => Background::Solid(color),
                _ => Background::Transparent,
            };
        if let Some(background) =
            m1_bridge::background(self.raw(Prop::BackgroundImage), &table, self.color, &ctx)
                .flatten()
        {
            self.background = background;
        }
        self.border_width = m1_bridge::px(self.raw(Prop::BorderTopWidth), &ctx)
            .unwrap_or(0.0)
            .max(0.0);
        if m1_bridge::border_style_is_none(self.raw(Prop::BorderTopStyle)) == Some(true) {
            self.border_width = 0.0;
        }
        self.border_color = m1_bridge::color(self.raw(Prop::BorderTopColor), &table, self.color)
            .unwrap_or(Color::TRANSPARENT);
        self.border_radius = m1_bridge::radius(self.raw(Prop::BorderTopLeftRadius), &ctx)
            .unwrap_or(0.0)
            .max(0.0);
        for (index, prop) in [
            Prop::PaddingTop,
            Prop::PaddingRight,
            Prop::PaddingBottom,
            Prop::PaddingLeft,
        ]
        .into_iter()
        .enumerate()
        {
            self.padding[index] = m1_bridge::px(self.raw(prop), &ctx).unwrap_or(0.0).max(0.0);
        }
        self.background_clip = m1_bridge::clip(self.raw(Prop::BackgroundClip)).unwrap_or_default();
        self.min_width = m1_bridge::px(self.raw(Prop::MinWidth), &ctx)
            .unwrap_or(0.0)
            .max(0.0);
        self.min_height = m1_bridge::px(self.raw(Prop::MinHeight), &ctx)
            .unwrap_or(0.0)
            .max(0.0);
    }
}

/// One longhand's computed value: the inheritance rule, the wide keywords and
/// invalid-at-computed-value-time (contract §5).
fn compute_one(
    prop: Prop,
    cascaded: &CascadedValues,
    parent: &ComputedStyle,
    length: &LengthCtx,
    color: &ColorCtx<'_>,
) -> Value {
    // The registry's initial values are unresolved too: `border-*-color`'s is
    // `currentColor`, which has to become an absolute colour before it lands
    // in the table.
    let initial = || {
        let declared = prop.initial();
        resolve_value(&declared, length, color).unwrap_or(declared)
    };
    let fallback = || {
        if prop.is_inherited() {
            parent.raw(prop).clone()
        } else {
            initial()
        }
    };
    let Some(declared) = cascaded.winner(prop) else {
        return fallback();
    };
    match declared {
        Value::Wide(Wide::Inherit) => return parent.raw(prop).clone(),
        Value::Wide(Wide::Initial) => return initial(),
        Value::Wide(Wide::Unset) => return fallback(),
        _ => {}
    }
    match resolve_value(declared, length, color) {
        Some(value) => value,
        None => {
            // Decision 6: CSS's "invalid at computed value time". The runner-ups
            // stay in `CascadedValues` for diagnostics; they are not consulted.
            tracing::debug!(
                property = prop.name(),
                value = ?declared,
                runner_ups = cascaded.candidates(prop).len().saturating_sub(1),
                inherited = prop.is_inherited(),
                "invalid at computed-value time; using the inherited or initial value"
            );
            fallback()
        }
    }
}

/// Turn a parsed value into a computed one: absolute colours, absolute lengths.
///
/// A percentage is *kept*: it is valid, its basis is simply not known until
/// layout. `None` means invalid at computed-value time -- an unknown `@name`, a
/// colour cycle, a unit-incompatible or non-finite `calc()`.
fn resolve_value(value: &Value, length: &LengthCtx, color: &ColorCtx<'_>) -> Option<Value> {
    Some(match value {
        Value::Wide(_) => return None,
        Value::Number(number) => {
            if !number.is_finite() {
                return None;
            }
            Value::Number(*number)
        }
        Value::Percentage(fraction) => {
            if !fraction.is_finite() {
                return None;
            }
            Value::Percentage(*fraction)
        }
        Value::Angle(degrees) => {
            if !degrees.is_finite() {
                return None;
            }
            Value::Angle(*degrees)
        }
        Value::Length(inner) => Value::Length(resolve_length(inner, length)?),
        Value::Color(inner) => Value::Color(ColorValue::Absolute(inner.resolve(color)?)),
        Value::Shadow(shadow) => Value::Shadow(resolve_shadow(shadow, length, color)?),
        Value::Image(image) => Value::Image(resolve_image(image, length, color)?),
        Value::Position(position) => Value::Position(resolve_position(position, length)?),
        Value::BgSize(BgSize::Explicit(x, y)) => Value::BgSize(BgSize::Explicit(
            resolve_length(x, length)?,
            resolve_length(y, length)?,
        )),
        Value::LineHeight(LineHeight::Length(inner)) => {
            Value::LineHeight(LineHeight::Length(resolve_length(inner, length)?))
        }
        Value::Transform(list) => {
            let mut out = Vec::with_capacity(list.len());
            for function in list.iter() {
                out.push(resolve_transform(function, length)?);
            }
            Value::Transform(Rc::from(out))
        }
        Value::Filter(list) => {
            let mut out = Vec::with_capacity(list.len());
            for function in list.iter() {
                out.push(resolve_filter(function, length, color)?);
            }
            Value::Filter(Rc::from(out))
        }
        Value::IconPalette(entries) => {
            let mut out = Vec::with_capacity(entries.len());
            for (name, slot) in entries.iter() {
                out.push((Rc::clone(name), ColorValue::Absolute(slot.resolve(color)?)));
            }
            Value::IconPalette(Rc::from(out))
        }
        Value::Pair(pair) => Value::Pair(Rc::new((
            resolve_value(&pair.0, length, color)?,
            resolve_value(&pair.1, length, color)?,
        ))),
        Value::List(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items.iter() {
                out.push(resolve_value(item, length, color)?);
            }
            Value::List(Rc::from(out))
        }
        other => other.clone(),
    })
}

/// Absolutise a length, keeping percentages whose basis is not yet known.
fn resolve_length(length: &Length, ctx: &LengthCtx) -> Option<Length> {
    if matches!(length, Length::Auto) {
        return Some(Length::Auto);
    }
    if length_has_percent(length) && ctx.percent_basis.is_none() {
        return Some(length.clone());
    }
    let px = length.resolve(ctx)?;
    px.is_finite().then(|| Length::px(px))
}

fn length_has_percent(length: &Length) -> bool {
    match length {
        Length::Percent(_) => true,
        Length::Calc(node) => calc_has_percent(node),
        Length::Abs { .. } | Length::Auto => false,
    }
}

fn calc_has_percent(node: &CalcNode) -> bool {
    match node {
        CalcNode::Percent(_) => true,
        CalcNode::Length(length) => length_has_percent(length),
        CalcNode::Sum(a, b) | CalcNode::Difference(a, b) => {
            calc_has_percent(a) || calc_has_percent(b)
        }
        CalcNode::Product(a, _) | CalcNode::Quotient(a, _) => calc_has_percent(a),
        CalcNode::Min(nodes) | CalcNode::Max(nodes) => nodes.iter().any(calc_has_percent),
        CalcNode::Clamp(a, b, c) => {
            calc_has_percent(a) || calc_has_percent(b) || calc_has_percent(c)
        }
        CalcNode::Number(_) | CalcNode::Angle(_) | CalcNode::Time(_) | CalcNode::Var(_) => false,
    }
}

/// A shadow's omitted colour *is* `currentColor`; it is resolved here so an
/// interpolator never has to know about a paint context (contract §2.10).
fn resolve_shadow(shadow: &Shadow, length: &LengthCtx, color: &ColorCtx<'_>) -> Option<Shadow> {
    let resolved = match &shadow.color {
        Some(declared) => declared.resolve(color)?,
        None => color.current,
    };
    Some(Shadow {
        color: Some(ColorValue::Absolute(resolved)),
        offset_x: resolve_length(&shadow.offset_x, length)?,
        offset_y: resolve_length(&shadow.offset_y, length)?,
        blur: resolve_length(&shadow.blur, length)?,
        spread: resolve_length(&shadow.spread, length)?,
        inset: shadow.inset,
    })
}

fn resolve_position(position: &Position, length: &LengthCtx) -> Option<Position> {
    Some(Position {
        x: resolve_length(&position.x, length)?,
        y: resolve_length(&position.y, length)?,
        z: match &position.z {
            Some(z) => Some(resolve_length(z, length)?),
            None => None,
        },
    })
}

fn resolve_image(image: &Image, length: &LengthCtx, color: &ColorCtx<'_>) -> Option<Image> {
    Some(match image {
        Image::Solid(fill) => Image::Solid(ColorValue::Absolute(fill.resolve(color)?)),
        Image::Gradient(gradient) => {
            let mut stops = Vec::with_capacity(gradient.stops.len());
            for stop in gradient.stops.iter() {
                stops.push(ColorStop {
                    color: ColorValue::Absolute(stop.color.resolve(color)?),
                    position: match &stop.position {
                        Some(position) => Some(resolve_length(position, length)?),
                        None => None,
                    },
                    hint: match &stop.hint {
                        Some(hint) => Some(resolve_length(hint, length)?),
                        None => None,
                    },
                });
            }
            Image::Gradient(Rc::new(Gradient {
                kind: gradient.kind.clone(),
                repeating: gradient.repeating,
                stops: Rc::from(stops),
                interpolation: gradient.interpolation,
            }))
        }
        Image::CrossFade(layers) => {
            let mut out = Vec::with_capacity(layers.len());
            for (share, image) in layers.iter() {
                out.push((*share, resolve_image(image, length, color)?));
            }
            Image::CrossFade(Rc::from(out))
        }
        // `url()` and the `-gtk-*` icon functions carry nothing to resolve; a
        // URL that fails to decode is recorded-unresolved at paint time, not
        // invalid here.
        other => other.clone(),
    })
}

fn resolve_transform(function: &TransformFn, ctx: &LengthCtx) -> Option<TransformFn> {
    Some(match function {
        TransformFn::Translate(x, y) => {
            TransformFn::Translate(resolve_length(x, ctx)?, resolve_length(y, ctx)?)
        }
        TransformFn::TranslateX(x) => TransformFn::TranslateX(resolve_length(x, ctx)?),
        TransformFn::TranslateY(y) => TransformFn::TranslateY(resolve_length(y, ctx)?),
        TransformFn::TranslateZ(z) => TransformFn::TranslateZ(resolve_length(z, ctx)?),
        TransformFn::Translate3d(x, y, z) => TransformFn::Translate3d(
            resolve_length(x, ctx)?,
            resolve_length(y, ctx)?,
            resolve_length(z, ctx)?,
        ),
        TransformFn::Perspective(d) => TransformFn::Perspective(resolve_length(d, ctx)?),
        other => other.clone(),
    })
}

fn resolve_filter(
    function: &FilterFn,
    length: &LengthCtx,
    color: &ColorCtx<'_>,
) -> Option<FilterFn> {
    Some(match function {
        FilterFn::Blur(radius) => FilterFn::Blur(resolve_length(radius, length)?),
        FilterFn::DropShadow(shadow) => {
            FilterFn::DropShadow(resolve_shadow(shadow, length, color)?)
        }
        other => other.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::{Background, ComputedStyle, GradientStop};
    use crate::css::cascade::CompiledSheet;
    use crate::css::computed::{FromValue, ResolveEnv};
    use crate::css::node::{Node, PseudoStates};
    use crate::css::registry::Prop;
    use crate::css::select::MatchCx;
    use crate::css::value::color::Rgba;
    use crate::css::value::{Length, Value};
    use skia_rs_safe::core::Color;

    fn adwaita() -> CompiledSheet {
        CompiledSheet::compile(crate::BUNDLED_ADWAITA_LIGHT)
    }

    /// A `window.background > button` tree.
    ///
    /// A [`Node`]'s parent link is a `Weak`, so the fixture keeps the window
    /// alive for as long as the button is used; it derefs to the button so
    /// every call site reads as if it were the bare node.
    struct ButtonTree {
        _window: Node,
        button: Node,
    }

    impl std::ops::Deref for ButtonTree {
        type Target = Node;

        fn deref(&self) -> &Node {
            &self.button
        }
    }

    fn button(classes: &[&str], states: PseudoStates) -> ButtonTree {
        let window = Node::with_classes("window", &["background"]);
        let button = Node::with_classes("button", classes);
        window.append_child(&button);
        button.set_states(states);
        ButtonTree {
            _window: window,
            button,
        }
    }

    /// Resolve `node`'s whole ancestor chain under the default environment.
    fn resolve(sheet: &CompiledSheet, node: &Node) -> ComputedStyle {
        let mut cx = MatchCx::new();
        ComputedStyle::resolve_chain(sheet, node, &ResolveEnv::default(), &mut cx)
    }

    fn stop(color: u32, position_px: Option<f32>) -> GradientStop {
        GradientStop {
            color: Color(color),
            position_px,
        }
    }

    #[test]
    fn non_finite_lengths_are_rejected() {
        // M1's `parse_px` is now the registry's `<length>` parser plus
        // `m1_bridge::px`; `NaNpx`/`infpx` are identifiers, not dimensions,
        // so they are invalid at parse time, and `1e40px` overflows f32.
        use crate::css::registry::parse_declaration_value;
        let ctx = super::m1_bridge::ctx(ComputedStyle::DEFAULT_FONT_SIZE, None);
        let px = |text: &str| {
            parse_declaration_value(Prop::PaddingTop, text)
                .ok()
                .and_then(|value| super::m1_bridge::px(&value, &ctx))
        };
        assert_eq!(px("NaNpx"), None);
        assert_eq!(px("infpx"), None);
        assert_eq!(px("1e40px"), None);
        assert_eq!(px("4px"), Some(4.0));
        assert_eq!(px("0"), Some(0.0));
    }

    #[test]
    fn adwaita_hex_bytes_round_trip_through_rgba() {
        // The M1 tests pin exact bytes; M2 resolves colours as f32 `Rgba` and
        // converts back. If that round trip were lossy, every pinned byte in
        // this file and in the offscreen gate would drift by one.
        for declared in [
            0xFF2E_3436u32,
            0xFFCD_C7C2,
            0xFFF6_F5F4,
            0xFFFB_FAFA,
            0xFFD6_D1CD,
            0xFFE8_E6E3,
            0xFFDA_D6D2,
            0xFF2C_7FE3,
            0xFF35_84E4,
            0xFF15_539E,
            0xFF19_61B9,
        ] {
            let color = Color(declared);
            assert_eq!(Rgba::from_color32(color).to_color32(), color);
        }
    }

    #[test]
    fn adwaita_normal_button() {
        let s = resolve(&adwaita(), &button(&[], PseudoStates::default()));
        assert_eq!(
            s.background,
            Background::LinearGradientToTop {
                from: stop(0xFFF6_F5F4, Some(2.0)),
                to: stop(0xFFFB_FAFA, None),
            }
        );
        assert_eq!(s.color, Color(0xFF2E_3436));
        assert_eq!(s.border_width, 1.0);
        assert_eq!(s.border_color, Color(0xFFCD_C7C2));
        assert_eq!(s.border_radius, 5.0);
        assert_eq!(s.padding, [4.0, 9.0, 4.0, 9.0]);
        assert_eq!(s.min_width, 16.0);
        assert_eq!(s.min_height, 24.0);
        assert_eq!(s.font_size, ComputedStyle::DEFAULT_FONT_SIZE);
    }

    #[test]
    fn adwaita_hover_and_active_button() {
        let sheet = adwaita();
        let hovered = resolve(&sheet, &button(&[], PseudoStates::HOVER));
        assert_eq!(
            hovered.background,
            Background::LinearGradientToTop {
                from: stop(0xFFD6_D1CD, None),
                to: stop(0xFFE8_E6E3, Some(1.0)),
            }
        );
        assert_eq!(hovered.border_color, Color(0xFFCD_C7C2));

        let active = resolve(&sheet, &button(&[], PseudoStates::ACTIVE));
        assert_eq!(active.background, Background::Solid(Color(0xFFDA_D6D2)));
        assert_eq!(
            active.border_radius, 5.0,
            "radius must survive the state change"
        );
    }

    #[test]
    fn adwaita_suggested_action_button() {
        let sheet = adwaita();
        let s = resolve(
            &sheet,
            &button(&["suggested-action"], PseudoStates::default()),
        );
        assert_eq!(
            s.background,
            Background::LinearGradientToTop {
                from: stop(0xFF2C_7FE3, Some(2.0)),
                to: stop(0xFF35_84E4, None),
            }
        );
        assert_eq!(s.color, Color(0xFFFF_FFFF));
        assert_eq!(s.border_color, Color(0xFF15_539E));

        let pressed = resolve(&sheet, &button(&["suggested-action"], PseudoStates::ACTIVE));
        assert_eq!(pressed.background, Background::Solid(Color(0xFF19_61B9)));
    }

    #[test]
    fn gradient_sampling_follows_to_top_semantics() {
        // to top => the gradient line starts at the bottom edge.
        let hover = Background::LinearGradientToTop {
            from: stop(0xFFD6_D1CD, None),
            to: stop(0xFFE8_E6E3, Some(1.0)),
        };
        let h = 30.0;
        // The bottom edge itself is the first stop.
        assert_eq!(hover.color_at(h, h), Color(0xFFD6_D1CD));
        // The bottom pixel row's centre sits halfway through the 1px stop
        // band, so it is the midpoint of the two stops, not either of them.
        assert_eq!(hover.color_at(h, h - 0.5), Color(0xFFDF_DCD8));
        // Anything above 1px from the bottom is past the second stop.
        assert_eq!(hover.color_at(h, h / 2.0), Color(0xFFE8_E6E3));
        assert_eq!(hover.color_at(h, 0.5), Color(0xFFE8_E6E3));

        let normal = Background::LinearGradientToTop {
            from: stop(0xFFF6_F5F4, Some(2.0)),
            to: stop(0xFFFB_FAFA, None),
        };
        // Below the 2px first stop the fill is flat.
        assert_eq!(normal.color_at(h, h - 0.5), Color(0xFFF6_F5F4));
        assert_eq!(normal.color_at(h, h - 1.5), Color(0xFFF6_F5F4));
        // The top edge is the second stop.
        assert_eq!(normal.color_at(h, 0.0), Color(0xFFFB_FAFA));
        // The middle is strictly between the two.
        let mid = normal.color_at(h, h / 2.0);
        assert!(mid != Color(0xFFF6_F5F4) && mid != Color(0xFFFB_FAFA));
        assert!((0xF6..=0xFB).contains(&mid.red()));
        assert!((0xF5..=0xFA).contains(&mid.green()));
        assert!((0xF4..=0xFA).contains(&mid.blue()));
    }

    #[test]
    fn background_color_applies_when_there_is_no_background_image() {
        let sheet = CompiledSheet::compile("button { background-color: #112233 }");
        let s = resolve(&sheet, &button(&[], PseudoStates::default()));
        assert_eq!(s.background, Background::Solid(Color(0xFF11_2233)));
    }

    #[test]
    fn background_image_none_falls_back_to_background_color() {
        let sheet =
            CompiledSheet::compile("button { background-color: #112233; background-image: none }");
        let s = resolve(&sheet, &button(&[], PseudoStates::default()));
        assert_eq!(s.background, Background::Solid(Color(0xFF11_2233)));
    }

    #[test]
    fn named_color_references_resolve_through_define_color() {
        let sheet = CompiledSheet::compile(
            "@define-color accent_color #3584E4;\nbutton { background-color: @accent_color }",
        );
        let s = resolve(&sheet, &button(&[], PseudoStates::default()));
        assert_eq!(s.background, Background::Solid(Color(0xFF35_84E4)));
    }

    #[test]
    fn defaults_apply_when_nothing_matches() {
        let sheet = CompiledSheet::compile("entry { color: red }");
        let s = resolve(&sheet, &button(&[], PseudoStates::default()));
        assert_eq!(s.background, Background::Transparent);
        assert_eq!(s.color, Color::BLACK);
        assert_eq!(s.border_width, 0.0);
        assert_eq!(s.border_radius, 0.0);
        assert_eq!(s.padding, [0.0; 4]);
        assert_eq!(s.font_size, ComputedStyle::DEFAULT_FONT_SIZE);
        // M1 had no initial value for `border-color` and defaulted it to
        // transparent. The registry's initial is `currentColor` (contract
        // §1.3), which resolves to the initial `color`: opaque black.
        assert_eq!(s.border_color, Color(0xFF00_0000));
    }

    #[test]
    fn per_side_padding_longhands_override_the_shorthand() {
        // E1: `padding-left`/`padding-right` were never read, so Adwaita's
        // `button.image-button` (padding-left/right: 5px over the base
        // `padding: 4px 9px`) computed as [4, 9, 4, 9].
        let s = resolve(
            &adwaita(),
            &button(&["image-button"], PseudoStates::default()),
        );
        assert_eq!(s.padding, [4.0, 5.0, 4.0, 5.0]);
        let t = resolve(
            &adwaita(),
            &button(&["text-button"], PseudoStates::default()),
        );
        assert_eq!(t.padding, [4.0, 16.0, 4.0, 16.0]);
    }

    #[test]
    fn border_none_zeroes_the_used_width() {
        // E1: `border: none` was a no-op, so a flat button kept a 1px border.
        let sheet = CompiledSheet::compile(
            "button { border: 1px solid; border-color: red }\n             button.flat { border: none }",
        );
        let s = resolve(&sheet, &button(&["flat"], PseudoStates::default()));
        assert_eq!(s.border_width, 0.0);
    }

    #[test]
    fn a_function_valued_border_colour_is_not_shredded() {
        // E1: splitting `border` on whitespace turned `rgb(0 0 0)` into
        // `0`, which hit the unitless-zero length path and zeroed the width.
        let sheet = CompiledSheet::compile("button { border: 1px solid rgb(0 0 0) }");
        let s = resolve(&sheet, &button(&[], PseudoStates::default()));
        assert_eq!(s.border_width, 1.0);
        assert_eq!(s.border_color, Color(0xFF00_0000));
    }

    #[test]
    fn the_background_shorthand_is_read() {
        // E1: `background` was never read at all.
        let sheet = CompiledSheet::compile("button { background: #112233 }");
        assert_eq!(
            resolve(&sheet, &button(&[], PseudoStates::default())).background,
            Background::Solid(Color(0xFF11_2233))
        );
        let sheet = CompiledSheet::compile("button { background: image(#dad6d2) }");
        assert_eq!(
            resolve(&sheet, &button(&[], PseudoStates::default())).background,
            Background::Solid(Color(0xFFDA_D6D2))
        );
        // The shorthand resets the longhands it does not name.
        let sheet = CompiledSheet::compile(
            "button { background-image: image(#dad6d2) }\nbutton { background: #112233 }",
        );
        assert_eq!(
            resolve(&sheet, &button(&[], PseudoStates::default())).background,
            Background::Solid(Color(0xFF11_2233))
        );
    }

    #[test]
    fn a_shorthand_and_a_longhand_are_ordered_by_the_cascade_not_by_property_name() {
        // E1: `from_declarations` applied `border` and then `border-width` in
        // a fixed order, so the source order between them was ignored.
        let later_shorthand = CompiledSheet::compile(
            "button { border-width: 5px }\nbutton { border: 1px solid red }",
        );
        assert_eq!(
            resolve(&later_shorthand, &button(&[], PseudoStates::default())).border_width,
            1.0
        );
        let later_longhand = CompiledSheet::compile(
            "button { border: 1px solid red }\nbutton { border-width: 5px }",
        );
        assert_eq!(
            resolve(&later_longhand, &button(&[], PseudoStates::default())).border_width,
            5.0
        );
    }

    #[test]
    fn an_invalid_winner_yields_the_initial_or_inherited_value() {
        // Decision 6 replaces M1's runner-up rule. Two halves:
        //
        // 1. Adwaita:1606 `button.sidebar-button { border-radius: 100% }` is
        //    *valid* now -- percentages are a first-class radius -- so the
        //    winner is kept as a percentage and resolved against the box at
        //    used-value time, not swapped for the base rule's 5px.
        // 2. A genuinely invalid winner (an unknown @name has nothing to
        //    resolve against) takes the initial value for a non-inherited
        //    property and the inherited value for an inherited one -- never
        //    the runner-up.
        let sheet = adwaita();
        let style = resolve(
            &sheet,
            &button(&["sidebar-button"], PseudoStates::default()),
        );
        assert_eq!(
            style.raw(Prop::BorderTopLeftRadius),
            &Value::Pair(std::rc::Rc::new((
                Value::Length(Length::Percent(1.0)),
                Value::Length(Length::Percent(1.0)),
            ))),
            "the percentage radius was thrown away instead of being kept"
        );

        let sheet = CompiledSheet::compile(
            "window { color: #ff0000 }\n\
             button { border-top-color: #00ff00; color: #0000ff }\n\
             button.bad { border-top-color: @nosuch; color: @nosuch }",
        );
        let style = resolve(&sheet, &button(&["bad"], PseudoStates::default()));
        assert_eq!(
            style.get::<Rgba>(Prop::Color).to_color32(),
            Color(0xFFFF_0000),
            "an invalid inherited property must inherit, not use the runner-up"
        );
        assert_eq!(
            style.get::<Rgba>(Prop::BorderTopColor).to_color32(),
            Color(0xFFFF_0000),
            "border-top-color's initial value is currentColor, i.e. the \
             inherited red -- not the #00ff00 runner-up"
        );
    }

    #[test]
    fn negative_lengths_clamp_to_zero() {
        // E10: a negative padding or radius painted outside the box.
        let sheet = CompiledSheet::compile(
            "button { padding: -4px; border: -1px solid red; border-radius: -5px; \
             min-width: -1px; min-height: -2px }",
        );
        let s = resolve(&sheet, &button(&[], PseudoStates::default()));
        assert_eq!(s.padding, [0.0; 4]);
        assert_eq!(s.border_width, 0.0);
        assert_eq!(s.border_radius, 0.0);
        assert_eq!(s.min_width, 0.0);
        assert_eq!(s.min_height, 0.0);
    }

    #[test]
    fn color_and_font_size_inherit_from_the_ancestor_chain() {
        // E2: `resolve` ignored ancestors entirely, so this button computed
        // black at 14px.
        let sheet = CompiledSheet::compile(
            "window { color: #ff0000; font-size: 20px }\n             button { background-color: #ffffff }",
        );
        let s = resolve(&sheet, &button(&[], PseudoStates::default()));
        assert_eq!(s.color, Color(0xFFFF_0000));
        assert_eq!(s.font_size, 20.0);
        // The node's own declaration still wins over the inherited value.
        let sheet = CompiledSheet::compile(
            "window { color: #ff0000; font-size: 20px }\n             button { color: #00ff00; font-size: 11px }",
        );
        let s = resolve(&sheet, &button(&[], PseudoStates::default()));
        assert_eq!(s.color, Color(0xFF00_FF00));
        assert_eq!(s.font_size, 11.0);
    }

    #[test]
    fn non_inherited_properties_do_not_leak_down() {
        let sheet = CompiledSheet::compile("window { padding: 7px; border-radius: 9px }");
        let s = resolve(&sheet, &button(&[], PseudoStates::default()));
        assert_eq!(s.padding, [0.0; 4]);
        assert_eq!(s.border_radius, 0.0);
    }

    #[test]
    fn current_color_resolves_against_the_elements_own_colour() {
        let sheet = CompiledSheet::compile(
            "window { color: #ff0000 }\n             button { border: 1px solid currentColor }",
        );
        let s = resolve(&sheet, &button(&[], PseudoStates::default()));
        assert_eq!(
            s.border_color,
            Color(0xFFFF_0000),
            "currentColor is the inherited colour"
        );

        let sheet = CompiledSheet::compile(
            "window { color: #ff0000 }\n             button { color: #0000ff; border: 1px solid currentColor;              background-color: currentColor }",
        );
        let s = resolve(&sheet, &button(&[], PseudoStates::default()));
        assert_eq!(
            s.border_color,
            Color(0xFF00_00FF),
            "the element's own colour wins"
        );
        assert_eq!(s.background, Background::Solid(Color(0xFF00_00FF)));
    }

    #[test]
    fn color_current_color_means_the_inherited_colour() {
        // Adwaita:898 `tab button.flat:hover { color: currentColor }`.
        let sheet =
            CompiledSheet::compile("window { color: #ff0000 }\nbutton { color: currentColor }");
        let s = resolve(&sheet, &button(&[], PseudoStates::default()));
        assert_eq!(s.color, Color(0xFFFF_0000));
    }

    #[test]
    fn border_line_width_keywords_resolve_to_pixels() {
        // Review round 1: `thin` was classified as the border *colour*, so
        // the width fell back to `medium` (3px) and the colour reset was lost.
        let sheet =
            CompiledSheet::compile("window { color: #ff0000 }\nbutton { border: thin solid }");
        let s = resolve(&sheet, &button(&[], PseudoStates::default()));
        assert_eq!(s.border_width, 1.0);
        assert_eq!(
            s.border_color,
            Color(0xFFFF_0000),
            "the omitted colour resets to currentColor, i.e. the inherited colour"
        );

        let sheet = CompiledSheet::compile("button { border: thick dotted red }");
        let s = resolve(&sheet, &button(&[], PseudoStates::default()));
        assert_eq!(s.border_width, 5.0);
        assert_eq!(s.border_color, Color(0xFFFF_0000));

        // ...and the reset is not undone by a runner-up the shorthand beat.
        let sheet = CompiledSheet::compile(
            "button { border-color: #00ff00 }\nbutton { border: thin solid }",
        );
        let s = resolve(&sheet, &button(&[], PseudoStates::default()));
        assert_eq!(s.border_width, 1.0);
        assert_eq!(
            s.border_color,
            Color::BLACK,
            "currentColor, not the reset colour"
        );
    }

    #[test]
    fn the_table_has_one_slot_per_longhand_and_starts_at_the_registry_initials() {
        let env = ResolveEnv::default();
        let initial = ComputedStyle::initial(&env);
        for prop in crate::css::registry::longhands() {
            // Every slot is readable and holds that row's initial value.
            let expected = match prop {
                // ResolveEnv seeds these two so it reaches the root of a chain.
                Prop::GtkDpi => Value::Number(env.dpi),
                Prop::FontSize => Value::Length(Length::px(env.root_font_size)),
                other => other.initial(),
            };
            assert_eq!(initial.raw(prop), &expected, "{}", prop.name());
        }
        let custom = ComputedStyle::initial(&ResolveEnv {
            dpi: 192.0,
            root_font_size: 16.0,
        });
        assert_eq!(custom.raw(Prop::GtkDpi), &Value::Number(192.0));
        assert_eq!(custom.raw(Prop::FontSize), &Value::Length(Length::px(16.0)));
    }

    #[test]
    fn get_reads_the_table_typed_and_a_mismatch_is_not_a_panic() {
        let sheet = CompiledSheet::compile(
            "button { color: #123456; opacity: 0.25; border-top-width: 3px; \
             background-repeat: no-repeat }",
        );
        let style = resolve(&sheet, &button(&[], PseudoStates::default()));
        assert_eq!(
            style.get::<Rgba>(Prop::Color).to_color32(),
            Color(0xFF12_3456)
        );
        assert_eq!(style.get::<f32>(Prop::Opacity), 0.25);
        assert_eq!(style.get::<f32>(Prop::BorderTopWidth), 3.0);
        // Asking for the wrong type yields that type's initial-equivalent.
        assert_eq!(style.get::<Rgba>(Prop::Opacity), Rgba::TRANSPARENT);
        assert_eq!(style.get::<f32>(Prop::BackgroundRepeat), 0.0);
    }

    #[test]
    fn inherited_properties_inherit_and_non_inherited_ones_do_not() {
        let sheet = CompiledSheet::compile(
            "window { color: #ff0000; letter-spacing: 2px; padding: 7px; opacity: 0.5 }",
        );
        let style = resolve(&sheet, &button(&[], PseudoStates::default()));
        assert_eq!(
            style.get::<Rgba>(Prop::Color).to_color32(),
            Color(0xFFFF_0000)
        );
        assert_eq!(style.get::<f32>(Prop::LetterSpacing), 2.0);
        assert_eq!(style.raw(Prop::PaddingTop), &Prop::PaddingTop.initial());
        assert_eq!(style.get::<f32>(Prop::Opacity), 1.0);
    }

    #[test]
    fn the_wide_keywords_do_what_the_cascade_spec_says() {
        let sheet = CompiledSheet::compile(
            "window { color: #ff0000; padding: 7px }\n\
             button.a { color: initial; padding: inherit }\n\
             button.b { color: unset; padding: unset }\n\
             button.c { color: inherit }",
        );
        let a = resolve(&sheet, &button(&["a"], PseudoStates::default()));
        assert_eq!(
            a.get::<Rgba>(Prop::Color),
            Rgba::from_value(&Prop::Color.initial())
        );
        assert_eq!(
            a.get::<f32>(Prop::PaddingTop),
            7.0,
            "inherit on a non-inherited property still inherits"
        );

        let b = resolve(&sheet, &button(&["b"], PseudoStates::default()));
        assert_eq!(
            b.get::<Rgba>(Prop::Color).to_color32(),
            Color(0xFFFF_0000),
            "unset on an inherited property inherits"
        );
        assert_eq!(
            b.raw(Prop::PaddingTop),
            &Prop::PaddingTop.initial(),
            "unset on a non-inherited property is initial"
        );

        let c = resolve(&sheet, &button(&["c"], PseudoStates::default()));
        assert_eq!(c.get::<Rgba>(Prop::Color).to_color32(), Color(0xFFFF_0000));
    }

    #[test]
    fn a_wide_keyword_at_the_root_of_the_chain_lands_on_the_initial_value() {
        let sheet = CompiledSheet::compile("window { color: inherit; padding: inherit }");
        let window = Node::with_classes("window", &["background"]);
        let style = resolve(&sheet, &window);
        assert_eq!(
            style.get::<Rgba>(Prop::Color),
            Rgba::from_value(&Prop::Color.initial())
        );
        assert_eq!(style.raw(Prop::PaddingTop), &Prop::PaddingTop.initial());
    }

    #[test]
    fn em_rem_and_pt_resolve_at_computed_time_against_the_right_font_size() {
        // `em` on font-size itself resolves against the *parent's* size; every
        // other property resolves against the element's own. `rem` uses the
        // initial font size, not the root node's (GTK's documented divergence).
        // `pt` converts through `-gtk-dpi`, not CSS's fixed 96.
        let sheet = CompiledSheet::compile(
            "window { font-size: 20px }\n\
             button { font-size: 2em; padding-top: 0.5em; padding-right: 1rem; \
                      padding-bottom: 12pt; -gtk-dpi: 144 }",
        );
        let style = resolve(&sheet, &button(&[], PseudoStates::default()));
        assert_eq!(
            style.get::<f32>(Prop::FontSize),
            40.0,
            "2em of the parent's 20px"
        );
        assert_eq!(
            style.get::<f32>(Prop::PaddingTop),
            20.0,
            "0.5em of its own 40px"
        );
        assert_eq!(
            style.get::<f32>(Prop::PaddingRight),
            14.0,
            "1rem is the initial 14px, not the root node's 20px"
        );
        assert_eq!(
            style.get::<f32>(Prop::PaddingBottom),
            24.0,
            "12pt at 144 dpi is 12 * 144 / 72 == 24px"
        );
    }

    #[test]
    fn a_percentage_survives_computed_time_unresolved() {
        // Only `font-size` has its basis at computed time. Everything else
        // keeps the percentage so the used-value accessors can resolve it
        // against the real box.
        let sheet = CompiledSheet::compile(
            "window { font-size: 20px }\nbutton { font-size: 150%; padding-top: 50% }",
        );
        let style = resolve(&sheet, &button(&[], PseudoStates::default()));
        assert_eq!(style.get::<f32>(Prop::FontSize), 30.0);
        assert_eq!(
            style.raw(Prop::PaddingTop),
            &Value::Length(Length::Percent(0.5))
        );
    }

    #[test]
    fn named_colours_and_current_color_are_absolute_in_the_table() {
        // Interpolators see resolved colours only (contract §2.10), so the
        // table must never hold a `@name` or `currentColor`.
        let sheet = CompiledSheet::compile(
            "@define-color accent_color #3584E4;\n\
             button { color: @accent_color; border-top-color: currentColor }",
        );
        let style = resolve(&sheet, &button(&[], PseudoStates::default()));
        assert_eq!(
            style.raw(Prop::BorderTopColor),
            &Value::Color(crate::css::value::color::ColorValue::Absolute(
                Rgba::from_color32(Color(0xFF35_84E4))
            ))
        );
    }

    #[test]
    fn resolve_with_an_explicit_parent_matches_resolving_the_whole_chain() {
        let sheet = CompiledSheet::compile("window { color: #ff0000 }\nbutton { padding: 2px }");
        let window = Node::with_classes("window", &["background"]);
        let node = Node::new("button");
        window.append_child(&node);
        let mut cx = MatchCx::new();
        let env = ResolveEnv::default();
        let window_style = ComputedStyle::resolve_chain(&sheet, &window, &env, &mut cx);
        assert_eq!(
            ComputedStyle::resolve(&sheet, &node, Some(&window_style), &env, &mut cx),
            ComputedStyle::resolve_chain(&sheet, &node, &env, &mut cx)
        );
    }

    #[test]
    fn the_box_accessors_read_adwaitas_button_in_used_pixels() {
        let style = resolve(&adwaita(), &button(&[], PseudoStates::default()));
        assert_eq!(style.font_size_px(), 14.0);
        assert_eq!(style.dpi(), 96.0);
        assert_eq!(style.color().to_color32(), Color(0xFF2E_3436));
        assert_eq!(style.opacity(), 1.0);
        assert_eq!(style.padding(0.0), [4.0, 9.0, 4.0, 9.0]);
        assert_eq!(style.margin(0.0), [Some(0.0); 4]);
        assert_eq!(style.min_size((0.0, 0.0)), (16.0, 24.0));
    }

    #[test]
    fn percentage_box_values_resolve_against_the_used_basis() {
        let sheet = CompiledSheet::compile(
            "button { padding: 25%; min-width: 50%; min-height: 10%; margin-left: 5% }",
        );
        let style = resolve(&sheet, &button(&[], PseudoStates::default()));
        // CSS resolves *every* padding side against the containing block's
        // *width*; the accessor takes that one basis.
        assert_eq!(style.padding(200.0), [50.0, 50.0, 50.0, 50.0]);
        assert_eq!(style.min_size((200.0, 80.0)), (100.0, 8.0));
        assert_eq!(style.margin(200.0)[3], Some(10.0));
    }

    #[test]
    fn auto_margins_are_none_and_everything_else_clamps_sanely() {
        let sheet = CompiledSheet::compile(
            "button { margin: auto; padding: -4px; min-width: -1px; min-height: -2px; \
             opacity: 4 }",
        );
        let style = resolve(&sheet, &button(&[], PseudoStates::default()));
        assert_eq!(style.margin(0.0), [None; 4]);
        assert_eq!(style.padding(0.0), [0.0; 4], "negative padding clamps to 0");
        assert_eq!(style.min_size((0.0, 0.0)), (0.0, 0.0));
        assert_eq!(style.opacity(), 1.0, "opacity clamps into 0..=1");
    }

    #[test]
    fn the_length_context_carries_the_styles_own_font_size_and_dpi() {
        let sheet = CompiledSheet::compile("button { font-size: 21px; -gtk-dpi: 192 }");
        let style = resolve(&sheet, &button(&[], PseudoStates::default()));
        let ctx = style.length_ctx(&ResolveEnv::default(), Some(64.0));
        assert_eq!(ctx.font_size_px, 21.0);
        assert_eq!(ctx.dpi, 192.0);
        assert_eq!(ctx.root_font_size_px, 14.0);
        assert_eq!(ctx.percent_basis, Some(64.0));
    }

    #[test]
    fn opacity_and_colour_survive_inheritance_without_leaking() {
        let sheet = CompiledSheet::compile("window { opacity: 0.5; color: #ff0000 }");
        let style = resolve(&sheet, &button(&[], PseudoStates::default()));
        assert_eq!(style.opacity(), 1.0, "opacity is not inherited");
        assert_eq!(style.color().to_color32(), Color(0xFFFF_0000));
    }
}
