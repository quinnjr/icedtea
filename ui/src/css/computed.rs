//! Computed values: a registry-indexed table of every longhand, plus the
//! typed used-value accessors that layout, paint, the button widget and the
//! app wiring read it through.
//!
//! `ComputedStyle` has no public fields -- the table (`values`) and its
//! accessors (`get`, `raw`, `padding`, `border_widths`, `background_layers`,
//! ...) are the whole surface.

use std::borrow::Cow;
use std::rc::Rc;

use crate::anim::{AnimationSpec, Overrides, TransitionSpec};

use super::cascade::{CascadedValues, CompiledSheet, cascade};
use super::node::Node;
use super::registry::{self, N_LONGHANDS, Prop};
use super::select::MatchCx;
use super::value::color::{ColorCtx, ColorValue, Rgba};
use super::value::image::ColorStop;
use super::value::{
    AnimationName, BgSize, CalcNode, FilterFn, FontFamily, FontStyle, FontWeight, Gradient, Image,
    IterationCount, Keyword, Length, LengthCtx, LengthUnit, LineHeight, Position, RepeatStyle,
    Shadow, Time, TimingFunction, TransformFn, Value, Wide,
};

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
    /// x-height / em for the face the styled element ends up using: the basis
    /// the `ex` unit resolves against. A widget that has matched a face feeds
    /// [`crate::text::FontDatabase::ex_ratio`] back in here; until then it is
    /// [`EX_RATIO`], the CSS-recommended default.
    pub ex_ratio: f32,
}

impl Default for ResolveEnv {
    fn default() -> Self {
        Self {
            dpi: 96.0,
            root_font_size: ComputedStyle::DEFAULT_FONT_SIZE,
            ex_ratio: EX_RATIO,
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

/// The x-height/em ratio used before a face is resolved -- CSS's recommended
/// default. The real ratio comes from the matched face, through
/// [`ResolveEnv::ex_ratio`]; `ex` appears in no GTK theme this crate ships.
pub const EX_RATIO: f32 = 0.5;

/// Every longhand's computed value, indexed by `Prop::slot()`.
///
/// No public fields: the table and its accessors (below) are the whole
/// surface.
#[derive(Clone, Debug, PartialEq)]
pub struct ComputedStyle {
    values: Box<[Value; N_LONGHANDS]>,
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
        Rc::new(Self { values })
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
            ex_ratio: env.ex_ratio,
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

    /// Used border widths in px, `[top, right, bottom, left]`.
    ///
    /// A `none`/`hidden` style forces its side's used width to zero -- that is
    /// what makes `border: none` undo an earlier `border: 1px solid`.
    #[must_use]
    pub fn border_widths(&self) -> [f32; 4] {
        const WIDTHS: [Prop; 4] = [
            Prop::BorderTopWidth,
            Prop::BorderRightWidth,
            Prop::BorderBottomWidth,
            Prop::BorderLeftWidth,
        ];
        let styles = self.border_styles();
        let mut widths = [0.0; 4];
        for (slot, prop) in WIDTHS.into_iter().enumerate() {
            if matches!(styles[slot], Keyword::None | Keyword::Hidden) {
                continue;
            }
            widths[slot] = self.length_px(prop, None).unwrap_or(0.0).max(0.0);
        }
        widths
    }

    /// Border colours, `[top, right, bottom, left]`.
    #[must_use]
    pub fn border_colors(&self) -> [Rgba; 4] {
        [
            self.get::<Rgba>(Prop::BorderTopColor),
            self.get::<Rgba>(Prop::BorderRightColor),
            self.get::<Rgba>(Prop::BorderBottomColor),
            self.get::<Rgba>(Prop::BorderLeftColor),
        ]
    }

    /// Border styles, `[top, right, bottom, left]`.
    #[must_use]
    pub fn border_styles(&self) -> [Keyword; 4] {
        [
            self.get::<Keyword>(Prop::BorderTopStyle),
            self.get::<Keyword>(Prop::BorderRightStyle),
            self.get::<Keyword>(Prop::BorderBottomStyle),
            self.get::<Keyword>(Prop::BorderLeftStyle),
        ]
    }

    /// Corner radii in px, `[top-left, top-right, bottom-right, bottom-left]`,
    /// each `[rx, ry]`, already scaled by CSS's overlapping-curves rule.
    ///
    /// Horizontal radii resolve percentages against `w`, vertical ones against
    /// `h`, per CSS Backgrounds and Borders L3 §border-radius.
    #[must_use]
    pub fn border_radii(&self, w: f32, h: f32) -> [[f32; 2]; 4] {
        const CORNERS: [Prop; 4] = [
            Prop::BorderTopLeftRadius,
            Prop::BorderTopRightRadius,
            Prop::BorderBottomRightRadius,
            Prop::BorderBottomLeftRadius,
        ];
        let horizontal = self.used_ctx(Some(w));
        let vertical = self.used_ctx(Some(h));
        let mut radii = [[0.0f32; 2]; 4];
        for (slot, prop) in CORNERS.into_iter().enumerate() {
            let (rx, ry) = match self.raw(prop) {
                Value::Pair(pair) => (pair.0.clone(), pair.1.clone()),
                other => (other.clone(), other.clone()),
            };
            radii[slot][0] = used_length(&rx, &horizontal);
            radii[slot][1] = used_length(&ry, &vertical);
        }

        // f = min over the four sides of side / (sum of its two radii).
        let mut factor = f32::INFINITY;
        for (side, sum) in [
            (w, radii[0][0] + radii[1][0]),
            (h, radii[1][1] + radii[2][1]),
            (w, radii[3][0] + radii[2][0]),
            (h, radii[0][1] + radii[3][1]),
        ] {
            if sum > 0.0 {
                factor = factor.min(side / sum);
            }
        }
        if factor.is_finite() && factor < 1.0 {
            for corner in &mut radii {
                corner[0] *= factor;
                corner[1] *= factor;
            }
        }
        radii
    }
}

/// One half of a corner radius, in px. Non-lengths and non-finite results are
/// zero: a radius is never negative and never `auto`.
fn used_length(value: &Value, ctx: &LengthCtx) -> f32 {
    match value {
        Value::Length(length) => length
            .resolve(ctx)
            .filter(|px| px.is_finite())
            .unwrap_or(0.0)
            .max(0.0),
        Value::Number(number) if *number == 0.0 => 0.0,
        _ => 0.0,
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

        let parent_font_size = parent.font_size_px();
        let parent_color = parent.color();

        // 1. -gtk-dpi.
        let seed_ctx = LengthCtx {
            font_size_px: parent_font_size,
            root_font_size_px: env.root_font_size,
            ex_ratio: env.ex_ratio,
            dpi: env.dpi,
            percent_basis: None,
        };
        let seed_colors = ColorCtx {
            table: &sheet.colors,
            current: parent_color,
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
            font_size_px: parent_font_size,
            root_font_size_px: env.root_font_size,
            ex_ratio: env.ex_ratio,
            dpi,
            percent_basis: Some(parent_font_size),
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
            _ => parent_font_size,
        };
        values[Prop::FontSize.slot()] = Value::Length(Length::px(font_size));

        // 3. color: `currentColor` on `color` itself is the inherited colour.
        let length_ctx = LengthCtx {
            font_size_px: font_size,
            root_font_size_px: env.root_font_size,
            ex_ratio: env.ex_ratio,
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

        ComputedStyle { values }
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

/// One resolved `background-*` layer.
///
/// Layers are **top-first**: CSS paints the first listed layer closest to the
/// viewer, with `background-color` behind all of them. Each layer carries its
/// own position/size/repeat/origin/clip/blend, shorter lists having been
/// repeated to the `background-image` list's length.
#[derive(Clone, Debug)]
pub struct BackgroundLayer {
    /// The layer's image.
    pub image: Image,
    /// `background-position`.
    pub position: Position,
    /// `background-size`.
    pub size: BgSize,
    /// `background-repeat`.
    pub repeat: RepeatStyle,
    /// `background-origin`: the box the image is positioned and sized against.
    pub origin: Keyword,
    /// `background-clip`: the box the painted result is clipped to.
    pub clip: Keyword,
    /// `background-blend-mode`, blending this layer with the ones below it.
    pub blend: Keyword,
}

impl ComputedStyle {
    /// Every painted background layer, top-first.
    ///
    /// `background-image: none` contributes no layer; `background-color` is not
    /// a layer at all -- it is one colour, painted once, behind everything.
    #[must_use]
    pub fn background_layers(&self) -> Vec<BackgroundLayer> {
        let images = comma_list(self.raw(Prop::BackgroundImage));
        let positions = comma_list(self.raw(Prop::BackgroundPosition));
        let sizes = comma_list(self.raw(Prop::BackgroundSize));
        let repeats = comma_list(self.raw(Prop::BackgroundRepeat));
        let origins = comma_list(self.raw(Prop::BackgroundOrigin));
        let clips = comma_list(self.raw(Prop::BackgroundClip));
        let blends = comma_list(self.raw(Prop::BackgroundBlendMode));

        let mut layers = Vec::with_capacity(images.len());
        for (index, image) in images.iter().enumerate() {
            let Value::Image(image) = image else {
                continue;
            };
            if matches!(image, Image::None) {
                continue;
            }
            layers.push(BackgroundLayer {
                image: image.clone(),
                position: match cycle(&positions, index) {
                    Some(Value::Position(position)) => position.clone(),
                    _ => Position::center(),
                },
                size: match cycle(&sizes, index) {
                    Some(Value::BgSize(size)) => size.clone(),
                    _ => BgSize::Auto,
                },
                repeat: match cycle(&repeats, index) {
                    Some(Value::Repeat(repeat)) => *repeat,
                    _ => RepeatStyle {
                        x: Keyword::Repeat,
                        y: Keyword::Repeat,
                    },
                },
                origin: keyword_or(cycle(&origins, index), Keyword::PaddingBox),
                clip: keyword_or(cycle(&clips, index), Keyword::BorderBox),
                blend: keyword_or(cycle(&blends, index), Keyword::Normal),
            });
        }
        layers
    }

    /// The `box-shadow` list, first listed on top. Empty for `none`.
    #[must_use]
    pub fn box_shadows(&self) -> Rc<[Shadow]> {
        self.get::<Rc<[Shadow]>>(Prop::BoxShadow)
    }
}

impl ComputedStyle {
    /// The `transition-*` longhands, flattened into one spec per entry of
    /// `transition-property`. An entry naming an unknown property, and
    /// `transition-property: none`, contribute nothing.
    #[must_use]
    pub fn transition_specs(&self) -> Vec<TransitionSpec> {
        let properties = comma_list(self.raw(Prop::TransitionProperty));
        let durations = comma_list(self.raw(Prop::TransitionDuration));
        let timings = comma_list(self.raw(Prop::TransitionTimingFunction));
        let delays = comma_list(self.raw(Prop::TransitionDelay));

        let mut specs = Vec::with_capacity(properties.len());
        for (index, property) in properties.iter().enumerate() {
            let (prop, all) = match property {
                Value::Keyword(Keyword::All) => (None, true),
                Value::Keyword(Keyword::None) => continue,
                Value::AnimationName(AnimationName::Named(name)) => match registry::lookup(name) {
                    Some(prop) if prop.is_longhand() => (Some(prop), false),
                    _ => {
                        tracing::debug!(%name, "transition-property names no longhand");
                        continue;
                    }
                },
                _ => continue,
            };
            specs.push(TransitionSpec {
                prop,
                all,
                duration: time_at(&durations, index),
                delay: time_at(&delays, index),
                timing: timing_at(&timings, index),
            });
        }
        specs
    }

    /// The `animation-*` longhands, flattened into one spec per entry of
    /// `animation-name`. `animation-name: none` contributes nothing.
    #[must_use]
    pub fn animation_specs(&self) -> Vec<AnimationSpec> {
        let names = comma_list(self.raw(Prop::AnimationName));
        let durations = comma_list(self.raw(Prop::AnimationDuration));
        let timings = comma_list(self.raw(Prop::AnimationTimingFunction));
        let delays = comma_list(self.raw(Prop::AnimationDelay));
        let iterations = comma_list(self.raw(Prop::AnimationIterationCount));
        let directions = comma_list(self.raw(Prop::AnimationDirection));
        let fills = comma_list(self.raw(Prop::AnimationFillMode));
        let states = comma_list(self.raw(Prop::AnimationPlayState));

        let mut specs = Vec::with_capacity(names.len());
        for (index, name) in names.iter().enumerate() {
            let Value::AnimationName(AnimationName::Named(name)) = name else {
                continue;
            };
            specs.push(AnimationSpec {
                name: AnimationName::Named(Rc::clone(name)),
                duration: time_at(&durations, index),
                delay: time_at(&delays, index),
                timing: timing_at(&timings, index),
                iterations: match cycle(&iterations, index) {
                    Some(value) => IterationCount::from_value(value),
                    None => IterationCount::Count(1.0),
                },
                direction: keyword_or(cycle(&directions, index), Keyword::Normal),
                fill: keyword_or(cycle(&fills, index), Keyword::None),
                play_state: keyword_or(cycle(&states, index), Keyword::Running),
            });
        }
        specs
    }

    /// This style with `overrides` layered on top. Borrowed unchanged when
    /// `overrides` is empty -- the common case, every frame nothing animates.
    #[must_use]
    pub fn with_overrides<'a>(&'a self, overrides: &Overrides) -> Cow<'a, ComputedStyle> {
        if overrides.is_empty() {
            return Cow::Borrowed(self);
        }
        let mut style = self.clone();
        for (prop, value) in overrides.iter() {
            style.values[prop.slot()] = value.clone();
        }
        Cow::Owned(style)
    }
}

fn time_at(list: &[&Value], index: usize) -> Time {
    match cycle(list, index) {
        Some(value) => Time::from_value(value),
        None => Time(0.0),
    }
}

fn timing_at(list: &[&Value], index: usize) -> TimingFunction {
    match cycle(list, index) {
        Some(value) => TimingFunction::from_value(value),
        None => TimingFunction::EASE,
    }
}

/// A comma-multiplied value as a slice of its entries. A non-list value is one
/// entry; `none` is still one entry, and the caller decides what that means.
fn comma_list(value: &Value) -> Vec<&Value> {
    match value {
        Value::List(items) => items.iter().collect(),
        other => vec![other],
    }
}

/// CSS's repeat-to-longest rule: shorter `background-*` lists cycle.
fn cycle<'a>(list: &[&'a Value], index: usize) -> Option<&'a Value> {
    if list.is_empty() {
        return None;
    }
    Some(list[index % list.len()])
}

fn keyword_or(value: Option<&Value>, fallback: Keyword) -> Keyword {
    match value {
        Some(Value::Keyword(keyword)) => *keyword,
        _ => fallback,
    }
}

#[cfg(test)]
mod tests {
    use super::ComputedStyle;
    use crate::css::cascade::CompiledSheet;
    use crate::css::computed::{FromValue, ResolveEnv};
    use crate::css::node::{Node, PseudoStates};
    use crate::css::registry::Prop;
    use crate::css::select::MatchCx;
    use crate::css::value::color::{ColorValue, Rgba};
    use crate::css::value::{
        AnimationName, Image, IterationCount, Keyword, Length, LengthCtx, Time, TimingFunction,
        Value,
    };
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

    /// The topmost background layer's gradient stops, as `(colour, position px)`.
    fn gradient_stops(style: &ComputedStyle) -> Vec<(Color, Option<f32>)> {
        let layers = style.background_layers();
        let Some(Image::Gradient(gradient)) = layers.first().map(|layer| layer.image.clone())
        else {
            panic!("the topmost background layer is not a gradient: {layers:?}");
        };
        gradient
            .stops
            .iter()
            .map(|stop| {
                let ColorValue::Absolute(rgba) = &stop.color else {
                    panic!("a computed gradient stop must carry a resolved colour");
                };
                let position = stop.position.as_ref().map(|length| match length {
                    Length::Abs { value, .. } => *value,
                    other => panic!("a computed stop position must be absolute: {other:?}"),
                });
                (rgba.to_color32(), position)
            })
            .collect()
    }

    /// The topmost background layer as a flat fill -- GTK's `image(<color>)`.
    fn solid_background(style: &ComputedStyle) -> Color {
        let layers = style.background_layers();
        let Some(Image::Solid(ColorValue::Absolute(rgba))) =
            layers.first().map(|layer| layer.image.clone())
        else {
            panic!("the topmost background layer is not a flat fill: {layers:?}");
        };
        rgba.to_color32()
    }

    #[test]
    fn non_finite_lengths_are_rejected() {
        // M1's `parse_px` is now the registry's `<length>` parser plus
        // `Length::resolve`; `NaNpx`/`infpx` are identifiers, not dimensions,
        // so they are invalid at parse time, and `1e40px` overflows f32.
        use crate::css::registry::parse_declaration_value;
        let ctx = LengthCtx {
            font_size_px: ComputedStyle::DEFAULT_FONT_SIZE,
            root_font_size_px: ComputedStyle::DEFAULT_FONT_SIZE,
            ex_ratio: 0.5,
            dpi: 96.0,
            percent_basis: None,
        };
        let px = |text: &str| {
            parse_declaration_value(Prop::PaddingTop, text)
                .ok()
                .and_then(|value| match value {
                    Value::Length(length) => length.resolve(&ctx).filter(|px| px.is_finite()),
                    Value::Number(0.0) => Some(0.0),
                    _ => None,
                })
        };
        assert_eq!(px("NaNpx"), None);
        assert_eq!(px("infpx"), None);
        assert_eq!(px("1e40px"), None);
        assert_eq!(px("4px"), Some(4.0));
        assert_eq!(px("0"), Some(0.0));
    }

    #[test]
    fn ex_lengths_resolve_against_the_environments_x_height_ratio() {
        // Mutation check: put `EX_RATIO` back in `resolve`'s length contexts
        // and both assertions collapse onto the same 10px.
        let sheet = CompiledSheet::compile("button { padding-top: 1ex; font-size: 20px }");
        let node = Node::new("button");
        let mut cx = MatchCx::new();
        let mut at = |ratio: f32| {
            let env = ResolveEnv {
                ex_ratio: ratio,
                ..ResolveEnv::default()
            };
            ComputedStyle::resolve_chain(&sheet, &node, &env, &mut cx).padding(0.0)[0]
        };
        assert!(
            (at(super::EX_RATIO) - 10.0).abs() < 1e-4,
            "the default is 0.5 em"
        );
        assert!(
            (at(0.25) - 5.0).abs() < 1e-4,
            "a face-derived ratio is used"
        );
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
            gradient_stops(&s),
            vec![(Color(0xFFF6_F5F4), Some(2.0)), (Color(0xFFFB_FAFA), None)]
        );
        assert_eq!(s.color().to_color32(), Color(0xFF2E_3436));
        assert_eq!(s.border_widths()[0], 1.0);
        assert_eq!(s.border_colors()[0].to_color32(), Color(0xFFCD_C7C2));
        assert_eq!(s.border_radii(100.0, 100.0)[0][0], 5.0);
        assert_eq!(s.padding(0.0), [4.0, 9.0, 4.0, 9.0]);
        assert_eq!(s.min_size((0.0, 0.0)), (16.0, 24.0));
        assert_eq!(s.font_size_px(), ComputedStyle::DEFAULT_FONT_SIZE);
    }

    #[test]
    fn adwaita_hover_and_active_button() {
        let sheet = adwaita();
        let hovered = resolve(&sheet, &button(&[], PseudoStates::HOVER));
        assert_eq!(
            gradient_stops(&hovered),
            vec![(Color(0xFFD6_D1CD), None), (Color(0xFFE8_E6E3), Some(1.0))]
        );
        assert_eq!(hovered.border_colors()[0].to_color32(), Color(0xFFCD_C7C2));

        let active = resolve(&sheet, &button(&[], PseudoStates::ACTIVE));
        assert_eq!(solid_background(&active), Color(0xFFDA_D6D2));
        assert_eq!(
            active.border_radii(100.0, 100.0)[0][0],
            5.0,
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
            gradient_stops(&s),
            vec![(Color(0xFF2C_7FE3), Some(2.0)), (Color(0xFF35_84E4), None)]
        );
        assert_eq!(s.color().to_color32(), Color(0xFFFF_FFFF));
        assert_eq!(s.border_colors()[0].to_color32(), Color(0xFF15_539E));

        let pressed = resolve(&sheet, &button(&["suggested-action"], PseudoStates::ACTIVE));
        assert_eq!(solid_background(&pressed), Color(0xFF19_61B9));
    }

    #[test]
    fn background_color_applies_when_there_is_no_background_image() {
        let sheet = CompiledSheet::compile("button { background-color: #112233 }");
        let s = resolve(&sheet, &button(&[], PseudoStates::default()));
        assert!(s.background_layers().is_empty());
        assert_eq!(
            s.get::<Rgba>(Prop::BackgroundColor).to_color32(),
            Color(0xFF11_2233)
        );
    }

    #[test]
    fn background_image_none_falls_back_to_background_color() {
        let sheet =
            CompiledSheet::compile("button { background-color: #112233; background-image: none }");
        let s = resolve(&sheet, &button(&[], PseudoStates::default()));
        assert!(s.background_layers().is_empty());
        assert_eq!(
            s.get::<Rgba>(Prop::BackgroundColor).to_color32(),
            Color(0xFF11_2233)
        );
    }

    #[test]
    fn named_color_references_resolve_through_define_color() {
        let sheet = CompiledSheet::compile(
            "@define-color accent_color #3584E4;\nbutton { background-color: @accent_color }",
        );
        let s = resolve(&sheet, &button(&[], PseudoStates::default()));
        assert!(s.background_layers().is_empty());
        assert_eq!(
            s.get::<Rgba>(Prop::BackgroundColor).to_color32(),
            Color(0xFF35_84E4)
        );
    }

    #[test]
    fn defaults_apply_when_nothing_matches() {
        let sheet = CompiledSheet::compile("entry { color: red }");
        let s = resolve(&sheet, &button(&[], PseudoStates::default()));
        assert!(s.background_layers().is_empty());
        assert_eq!(s.color().to_color32(), Color::BLACK);
        assert_eq!(s.border_widths()[0], 0.0);
        assert_eq!(s.border_radii(0.0, 0.0)[0][0], 0.0);
        assert_eq!(s.padding(0.0), [0.0; 4]);
        assert_eq!(s.font_size_px(), ComputedStyle::DEFAULT_FONT_SIZE);
        // M1 had no initial value for `border-color` and defaulted it to
        // transparent. The registry's initial is `currentColor` (contract
        // §1.3), which resolves to the initial `color`: opaque black.
        assert_eq!(s.border_colors()[0].to_color32(), Color(0xFF00_0000));
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
        assert_eq!(s.padding(0.0), [4.0, 5.0, 4.0, 5.0]);
        let t = resolve(
            &adwaita(),
            &button(&["text-button"], PseudoStates::default()),
        );
        assert_eq!(t.padding(0.0), [4.0, 16.0, 4.0, 16.0]);
    }

    #[test]
    fn border_none_zeroes_the_used_width() {
        // E1: `border: none` was a no-op, so a flat button kept a 1px border.
        let sheet = CompiledSheet::compile(
            "button { border: 1px solid; border-color: red }\n             button.flat { border: none }",
        );
        let s = resolve(&sheet, &button(&["flat"], PseudoStates::default()));
        assert_eq!(s.border_widths()[0], 0.0);
    }

    #[test]
    fn a_function_valued_border_colour_is_not_shredded() {
        // E1: splitting `border` on whitespace turned `rgb(0 0 0)` into
        // `0`, which hit the unitless-zero length path and zeroed the width.
        let sheet = CompiledSheet::compile("button { border: 1px solid rgb(0 0 0) }");
        let s = resolve(&sheet, &button(&[], PseudoStates::default()));
        assert_eq!(s.border_widths()[0], 1.0);
        assert_eq!(s.border_colors()[0].to_color32(), Color(0xFF00_0000));
    }

    #[test]
    fn the_background_shorthand_is_read() {
        // E1: `background` was never read at all.
        let sheet = CompiledSheet::compile("button { background: #112233 }");
        let s = resolve(&sheet, &button(&[], PseudoStates::default()));
        assert!(s.background_layers().is_empty());
        assert_eq!(
            s.get::<Rgba>(Prop::BackgroundColor).to_color32(),
            Color(0xFF11_2233)
        );

        let sheet = CompiledSheet::compile("button { background: image(#dad6d2) }");
        let s = resolve(&sheet, &button(&[], PseudoStates::default()));
        assert_eq!(solid_background(&s), Color(0xFFDA_D6D2));

        // The shorthand resets the longhands it does not name.
        let sheet = CompiledSheet::compile(
            "button { background-image: image(#dad6d2) }\nbutton { background: #112233 }",
        );
        let s = resolve(&sheet, &button(&[], PseudoStates::default()));
        assert!(
            s.background_layers().is_empty(),
            "the shorthand must reset background-image to none"
        );
        assert_eq!(
            s.get::<Rgba>(Prop::BackgroundColor).to_color32(),
            Color(0xFF11_2233)
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
            resolve(&later_shorthand, &button(&[], PseudoStates::default())).border_widths()[0],
            1.0
        );
        let later_longhand = CompiledSheet::compile(
            "button { border: 1px solid red }\nbutton { border-width: 5px }",
        );
        assert_eq!(
            resolve(&later_longhand, &button(&[], PseudoStates::default())).border_widths()[0],
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
        assert_eq!(s.padding(0.0), [0.0; 4]);
        assert_eq!(s.border_widths()[0], 0.0);
        assert_eq!(s.border_radii(0.0, 0.0)[0][0], 0.0);
        assert_eq!(s.min_size((0.0, 0.0)), (0.0, 0.0));
    }

    #[test]
    fn color_and_font_size_inherit_from_the_ancestor_chain() {
        // E2: `resolve` ignored ancestors entirely, so this button computed
        // black at 14px.
        let sheet = CompiledSheet::compile(
            "window { color: #ff0000; font-size: 20px }\n             button { background-color: #ffffff }",
        );
        let s = resolve(&sheet, &button(&[], PseudoStates::default()));
        assert_eq!(s.color().to_color32(), Color(0xFFFF_0000));
        assert_eq!(s.font_size_px(), 20.0);
        // The node's own declaration still wins over the inherited value.
        let sheet = CompiledSheet::compile(
            "window { color: #ff0000; font-size: 20px }\n             button { color: #00ff00; font-size: 11px }",
        );
        let s = resolve(&sheet, &button(&[], PseudoStates::default()));
        assert_eq!(s.color().to_color32(), Color(0xFF00_FF00));
        assert_eq!(s.font_size_px(), 11.0);
    }

    #[test]
    fn non_inherited_properties_do_not_leak_down() {
        let sheet = CompiledSheet::compile("window { padding: 7px; border-radius: 9px }");
        let s = resolve(&sheet, &button(&[], PseudoStates::default()));
        assert_eq!(s.padding(0.0), [0.0; 4]);
        assert_eq!(s.border_radii(0.0, 0.0)[0][0], 0.0);
    }

    #[test]
    fn current_color_resolves_against_the_elements_own_colour() {
        let sheet = CompiledSheet::compile(
            "window { color: #ff0000 }\n             button { border: 1px solid currentColor }",
        );
        let s = resolve(&sheet, &button(&[], PseudoStates::default()));
        assert_eq!(
            s.border_colors()[0].to_color32(),
            Color(0xFFFF_0000),
            "currentColor is the inherited colour"
        );

        let sheet = CompiledSheet::compile(
            "window { color: #ff0000 }\n             button { color: #0000ff; border: 1px solid currentColor;              background-color: currentColor }",
        );
        let s = resolve(&sheet, &button(&[], PseudoStates::default()));
        assert_eq!(
            s.border_colors()[0].to_color32(),
            Color(0xFF00_00FF),
            "the element's own colour wins"
        );
        assert!(s.background_layers().is_empty());
        assert_eq!(
            s.get::<Rgba>(Prop::BackgroundColor).to_color32(),
            Color(0xFF00_00FF)
        );
    }

    #[test]
    fn color_current_color_means_the_inherited_colour() {
        // Adwaita:898 `tab button.flat:hover { color: currentColor }`.
        let sheet =
            CompiledSheet::compile("window { color: #ff0000 }\nbutton { color: currentColor }");
        let s = resolve(&sheet, &button(&[], PseudoStates::default()));
        assert_eq!(s.color().to_color32(), Color(0xFFFF_0000));
    }

    #[test]
    fn border_line_width_keywords_resolve_to_pixels() {
        // Review round 1: `thin` was classified as the border *colour*, so
        // the width fell back to `medium` (3px) and the colour reset was lost.
        let sheet =
            CompiledSheet::compile("window { color: #ff0000 }\nbutton { border: thin solid }");
        let s = resolve(&sheet, &button(&[], PseudoStates::default()));
        assert_eq!(s.border_widths()[0], 1.0);
        assert_eq!(
            s.border_colors()[0].to_color32(),
            Color(0xFFFF_0000),
            "the omitted colour resets to currentColor, i.e. the inherited colour"
        );

        let sheet = CompiledSheet::compile("button { border: thick dotted red }");
        let s = resolve(&sheet, &button(&[], PseudoStates::default()));
        assert_eq!(s.border_widths()[0], 5.0);
        assert_eq!(s.border_colors()[0].to_color32(), Color(0xFFFF_0000));

        // ...and the reset is not undone by a runner-up the shorthand beat.
        let sheet = CompiledSheet::compile(
            "button { border-color: #00ff00 }\nbutton { border: thin solid }",
        );
        let s = resolve(&sheet, &button(&[], PseudoStates::default()));
        assert_eq!(s.border_widths()[0], 1.0);
        assert_eq!(
            s.border_colors()[0].to_color32(),
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
            ex_ratio: 0.5,
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

    #[test]
    fn mixed_per_side_borders_are_first_class() {
        // The headline M1 gap: `border_width`/`border_color` were one scalar
        // each, so the top side stood for all four.
        let sheet = CompiledSheet::compile(
            "button { border-width: 1px 2px 3px 4px; \
             border-color: red green blue white; \
             border-style: solid dashed dotted double }",
        );
        let style = resolve(&sheet, &button(&[], PseudoStates::default()));
        assert_eq!(style.border_widths(), [1.0, 2.0, 3.0, 4.0]);
        assert_eq!(
            style.border_colors().map(Rgba::to_color32),
            [
                Color(0xFFFF_0000),
                Color(0xFF00_8000),
                Color(0xFF00_00FF),
                Color(0xFFFF_FFFF),
            ]
        );
        assert_eq!(
            style.border_styles(),
            [
                Keyword::Solid,
                Keyword::Dashed,
                Keyword::Dotted,
                Keyword::Double
            ]
        );
    }

    #[test]
    fn a_none_or_hidden_border_style_forces_a_used_width_of_zero() {
        // E1: `border: none` was a no-op, so a flat button kept a 1px border.
        let sheet = CompiledSheet::compile(
            "button { border: 5px solid red }\n\
             button.flat { border-top-style: none; border-right-style: hidden }",
        );
        let style = resolve(&sheet, &button(&["flat"], PseudoStates::default()));
        assert_eq!(style.border_widths(), [0.0, 0.0, 5.0, 5.0]);
    }

    #[test]
    fn per_corner_radii_carry_both_axes_and_percentages() {
        let sheet = CompiledSheet::compile(
            "button { border-radius: 4px 8px 12px 16px / 2px 4px 6px 8px }\n\
             button.round { border-radius: 50% }",
        );
        let style = resolve(&sheet, &button(&[], PseudoStates::default()));
        assert_eq!(
            style.border_radii(200.0, 100.0),
            [[4.0, 2.0], [8.0, 4.0], [12.0, 6.0], [16.0, 8.0]]
        );

        let round = resolve(&sheet, &button(&["round"], PseudoStates::default()));
        assert_eq!(
            round.border_radii(200.0, 100.0),
            [[100.0, 50.0]; 4],
            "a percentage radius resolves against the box's own width and height"
        );
    }

    #[test]
    fn overlapping_corner_radii_are_scaled_down_together() {
        // CSS Backgrounds L3 §corner overlap: if two adjacent radii exceed the
        // side they share, every radius on the box scales by the same factor.
        let sheet = CompiledSheet::compile("button { border-radius: 60px }");
        let style = resolve(&sheet, &button(&[], PseudoStates::default()));
        // Top edge is 100 wide but wants 60 + 60: f = 100 / 120.
        assert_eq!(style.border_radii(100.0, 100.0), [[50.0, 50.0]; 4]);
        // Big enough box: no scaling at all.
        assert_eq!(style.border_radii(400.0, 400.0), [[60.0, 60.0]; 4]);
    }

    #[test]
    fn adwaitas_button_is_one_layer_clipped_to_the_border_box() {
        let style = resolve(&adwaita(), &button(&[], PseudoStates::default()));
        let layers = style.background_layers();
        assert_eq!(layers.len(), 1);
        assert!(matches!(layers[0].image, Image::Gradient(_)));
        // Adwaita sets no `background-clip`/`-origin`, so both are at their
        // CSS initials -- and they differ, which is why a translucent border
        // shows the element's own background.
        assert_eq!(layers[0].clip, Keyword::BorderBox);
        assert_eq!(layers[0].origin, Keyword::PaddingBox);
        assert_eq!(layers[0].blend, Keyword::Normal);
    }

    #[test]
    fn multiple_layers_are_top_first_and_the_shorter_lists_cycle() {
        let sheet = CompiledSheet::compile(
            "button { background-image: image(#ff0000), image(#00ff00), image(#0000ff); \
             background-clip: content-box, padding-box; \
             background-repeat: no-repeat }",
        );
        let style = resolve(&sheet, &button(&[], PseudoStates::default()));
        let layers = style.background_layers();
        assert_eq!(layers.len(), 3, "the image list drives the layer count");
        assert_eq!(
            layers[0].image,
            Image::Solid(crate::css::value::color::ColorValue::Absolute(
                Rgba::from_color32(Color(0xFFFF_0000))
            )),
            "the first listed layer is the topmost"
        );
        // CSS repeats the shorter lists to the image list's length.
        assert_eq!(layers[0].clip, Keyword::ContentBox);
        assert_eq!(layers[1].clip, Keyword::PaddingBox);
        assert_eq!(layers[2].clip, Keyword::ContentBox);
        assert_eq!(layers[2].repeat.x, Keyword::NoRepeat);
    }

    #[test]
    fn a_style_with_no_background_image_still_reports_no_layers() {
        let sheet = CompiledSheet::compile("button { background-color: #112233 }");
        let style = resolve(&sheet, &button(&[], PseudoStates::default()));
        assert!(
            style.background_layers().is_empty(),
            "`background-image: none` paints no layer; the colour is not a layer"
        );
        assert_eq!(
            style.get::<Rgba>(Prop::BackgroundColor).to_color32(),
            Color(0xFF11_2233)
        );
    }

    #[test]
    fn box_shadows_keep_their_order_and_resolve_their_colours() {
        let sheet = CompiledSheet::compile(
            "button { color: #ff0000; \
             box-shadow: 1px 2px 3px 4px #00ff00, inset 5px 6px }",
        );
        let style = resolve(&sheet, &button(&[], PseudoStates::default()));
        let shadows = style.box_shadows();
        assert_eq!(shadows.len(), 2);
        assert_eq!(shadows[0].offset_x, Length::px(1.0));
        assert_eq!(shadows[0].offset_y, Length::px(2.0));
        assert_eq!(shadows[0].blur, Length::px(3.0));
        assert_eq!(shadows[0].spread, Length::px(4.0));
        assert!(!shadows[0].inset);
        assert!(shadows[1].inset);
        // An omitted shadow colour is `currentColor`, resolved at computed time
        // so an interpolator never needs a paint context.
        assert_eq!(
            shadows[1].color,
            Some(crate::css::value::color::ColorValue::Absolute(
                Rgba::from_color32(Color(0xFFFF_0000))
            ))
        );
        // Reconciliation (Task 6): the plan's fixture assumed Adwaita's plain
        // button has no `box-shadow`, but the vendored sheet gives it a real
        // one-layer drop shadow -- `box-shadow: 0 1px 2px rgba(0, 0, 0, 0.07)`
        // on the shared `button` rule (adwaita-light.css:215). `box_shadows()`
        // must report it, not swallow it.
        let adwaita_shadows =
            resolve(&adwaita(), &button(&[], PseudoStates::default())).box_shadows();
        assert_eq!(adwaita_shadows.len(), 1);
        assert!(!adwaita_shadows[0].inset);
        assert_eq!(adwaita_shadows[0].offset_x, Length::zero());
        assert_eq!(adwaita_shadows[0].offset_y, Length::px(1.0));
        assert_eq!(adwaita_shadows[0].blur, Length::px(2.0));
        assert_eq!(adwaita_shadows[0].spread, Length::zero());
        assert!(matches!(
            adwaita_shadows[0].color,
            Some(crate::css::value::color::ColorValue::Absolute(_))
        ));
    }

    #[test]
    fn adwaitas_button_transition_flattens_to_one_spec() {
        // Adwaita animates buttons with a plain `transition: <time>`; the
        // property defaults to `all`.
        let sheet = CompiledSheet::compile("button { transition: 200ms ease-out 50ms }");
        let style = resolve(&sheet, &button(&[], PseudoStates::default()));
        let specs = style.transition_specs();
        assert_eq!(specs.len(), 1);
        assert!(specs[0].all);
        assert_eq!(specs[0].prop, None);
        assert_eq!(specs[0].duration, Time::from_ms(200.0));
        assert_eq!(specs[0].delay, Time::from_ms(50.0));
        assert_eq!(specs[0].timing, TimingFunction::EASE_OUT);
    }

    #[test]
    fn named_transition_properties_resolve_through_the_registry() {
        let sheet = CompiledSheet::compile(
            "button { transition-property: background-color, nosuch-property, color; \
             transition-duration: 100ms, 200ms }",
        );
        let style = resolve(&sheet, &button(&[], PseudoStates::default()));
        let specs = style.transition_specs();
        assert_eq!(
            specs.iter().map(|spec| spec.prop).collect::<Vec<_>>(),
            vec![Some(Prop::BackgroundColor), Some(Prop::Color)],
            "an unknown property name contributes no spec"
        );
        // Durations cycle, and the dropped entry does not shift the pairing:
        // `color` is the third property, so it takes the first duration again.
        assert_eq!(specs[0].duration, Time::from_ms(100.0));
        assert_eq!(specs[1].duration, Time::from_ms(100.0));
        assert!(
            resolve(
                &CompiledSheet::compile("button { transition-property: none }"),
                &button(&[], PseudoStates::default())
            )
            .transition_specs()
            .is_empty()
        );
    }

    #[test]
    fn animation_specs_carry_every_animation_longhand() {
        let sheet = CompiledSheet::compile(
            "button { animation: pulse 2s linear 1s infinite alternate both paused }",
        );
        let style = resolve(&sheet, &button(&[], PseudoStates::default()));
        let specs = style.animation_specs();
        assert_eq!(specs.len(), 1);
        assert_eq!(
            specs[0].name,
            AnimationName::Named(std::rc::Rc::from("pulse"))
        );
        assert_eq!(specs[0].duration, Time(2.0));
        assert_eq!(specs[0].delay, Time(1.0));
        assert_eq!(specs[0].timing, TimingFunction::Linear);
        assert_eq!(specs[0].iterations, IterationCount::Infinite);
        assert_eq!(specs[0].direction, Keyword::Alternate);
        assert_eq!(specs[0].fill, Keyword::Both);
        assert_eq!(specs[0].play_state, Keyword::Paused);
        assert!(
            resolve(&adwaita(), &button(&[], PseudoStates::default()))
                .animation_specs()
                .is_empty(),
            "`animation-name: none` produces no spec"
        );
    }

    #[test]
    fn overrides_layer_over_the_computed_style_without_cloning_when_empty() {
        use crate::anim::Overrides;
        let style = resolve(&adwaita(), &button(&[], PseudoStates::default()));
        let empty = Overrides::default();
        let borrowed = style.with_overrides(&empty);
        assert!(matches!(borrowed, std::borrow::Cow::Borrowed(_)));

        let mut overrides = Overrides::default();
        overrides.set(
            Prop::Color,
            Value::Color(ColorValue::Absolute(Rgba::from_color32(Color(0xFF00_FF00)))),
        );
        // A later `set` of the same property wins: animation over transition.
        overrides.set(
            Prop::Color,
            Value::Color(ColorValue::Absolute(Rgba::from_color32(Color(0xFF00_00FF)))),
        );
        let animated = style.with_overrides(&overrides);
        assert!(matches!(animated, std::borrow::Cow::Owned(_)));
        assert_eq!(animated.color().to_color32(), Color(0xFF00_00FF));
        assert_eq!(
            style.color().to_color32(),
            Color(0xFF2E_3436),
            "the base style must not be mutated"
        );
        assert_eq!(overrides.iter().count(), 1);
        assert!(overrides.get(Prop::Opacity).is_none());
    }

    #[test]
    fn the_computed_style_has_no_public_fields_left() {
        // A compile-time assertion in test form: the only way in is the table.
        let style = resolve(&adwaita(), &button(&[], PseudoStates::default()));
        let cloned = style.clone();
        assert_eq!(style, cloned);
        assert_eq!(style.raw(Prop::MinHeight), &Value::Length(Length::px(24.0)));
    }

    #[test]
    fn decision_six_goes_both_ways() {
        // An invalid winner takes the *inherited* value on an inherited
        // property and the *initial* value on a non-inherited one. Never the
        // runner-up, which is what M1 did.
        let sheet = CompiledSheet::compile(
            "window { color: #ff0000; letter-spacing: 3px }\n\
             button { color: #00ff00; letter-spacing: 9px; background-color: #0000ff }\n\
             button.bad { color: @nosuch; letter-spacing: calc(1px + 1s); \
                          background-color: @nosuch }",
        );
        let style = resolve(&sheet, &button(&["bad"], PseudoStates::default()));

        // inherited -> the parent's computed value
        assert_eq!(
            style.color().to_color32(),
            Color(0xFFFF_0000),
            "an invalid inherited colour must inherit"
        );
        assert_eq!(
            style.get::<f32>(Prop::LetterSpacing),
            3.0,
            "an invalid inherited length must inherit"
        );
        // not inherited -> the registry initial (transparent), not #0000ff
        assert_eq!(
            style.get::<Rgba>(Prop::BackgroundColor),
            Rgba::from_value(&Prop::BackgroundColor.initial()),
            "an invalid non-inherited colour must take its initial value"
        );
    }

    #[test]
    fn every_adwaita_node_resolves_without_panicking_and_every_accessor_reads() {
        // The whole vendored theme, across the node shapes it actually styles:
        // resolve each, then read every accessor. This is the crate-level
        // never-panic battery for computed values and used values.
        let sheet = adwaita();
        let mut cx = MatchCx::new();
        let env = ResolveEnv::default();
        for name in [
            "window",
            "headerbar",
            "button",
            "entry",
            "label",
            "notebook",
            "menuitem",
            "scrollbar",
            "check",
            "radio",
            "popover",
            "tooltip",
            "levelbar",
            "progressbar",
        ] {
            for classes in [
                &[][..],
                &["flat"][..],
                &["suggested-action"][..],
                &["osd"][..],
            ] {
                for states in [
                    PseudoStates::default(),
                    PseudoStates::HOVER,
                    PseudoStates::ACTIVE,
                    PseudoStates::DISABLED,
                    PseudoStates::CHECKED | PseudoStates::FOCUS,
                ] {
                    let window = Node::with_classes("window", &["background"]);
                    let node = Node::with_classes(name, classes);
                    window.append_child(&node);
                    node.set_states(states);
                    let style = ComputedStyle::resolve_chain(&sheet, &node, &env, &mut cx);

                    // Every longhand slot is readable...
                    for prop in crate::css::registry::longhands() {
                        let _ = style.raw(prop);
                        let _ = style.get::<f32>(prop);
                        let _ = style.get::<Keyword>(prop);
                        let _ = style.get::<Rgba>(prop);
                    }
                    // ...and every used-value accessor, at a plausible box and
                    // at a degenerate one.
                    for (w, h) in [(0.0, 0.0), (120.0, 40.0), (f32::MAX, 1.0)] {
                        let _ = style.padding(w);
                        let _ = style.margin(w);
                        let _ = style.min_size((w, h));
                        let _ = style.border_widths();
                        let _ = style.border_colors();
                        let _ = style.border_styles();
                        let _ = style.border_radii(w, h);
                        let _ = style.background_layers();
                        let _ = style.box_shadows();
                        let _ = style.transition_specs();
                        let _ = style.animation_specs();
                        let _ = style.opacity();
                        let _ = style.color();
                        let _ = style.font_size_px();
                    }
                }
            }
        }
    }

    #[test]
    fn resolving_hostile_declarations_never_panics_and_never_invents_a_value() {
        // The cascade's own hostile battery covers parse time; this covers
        // computed and used-value time, where the failure mode is a panic in a
        // resolver or a NaN escaping into layout.
        const HOSTILE: &[&str] = &[
            "button { padding: calc(100% - 100%) }",
            "button { padding: calc(1px / 0) }",
            "button { min-width: calc(1s + 1px) }",
            "button { border-radius: calc(1px * 1e30) / 1px }",
            "button { font-size: calc(-1em) }",
            "button { font-size: 0 }",
            "button { color: color-mix(in srgb, @nosuch, red) }",
            "@define-color a @b;\n@define-color b @a;\nbutton { color: @a }",
            "button { box-shadow: 1px 2px 3px 4px @nosuch }",
            "button { background-image: linear-gradient(to top, @nosuch, red) }",
            "button { letter-spacing: 1e38em }",
            "button { -gtk-dpi: 0 }",
            "button { -gtk-dpi: -5 }",
            "button { transform: translate(50%, calc(1px + 1%)) }",
            "button { opacity: calc(1 / 0) }",
        ];
        let mut cx = MatchCx::new();
        for css in HOSTILE {
            let sheet = CompiledSheet::compile(css);
            let style = ComputedStyle::resolve_chain(
                &sheet,
                &button(&[], PseudoStates::default()),
                &ResolveEnv::default(),
                &mut cx,
            );
            assert!(
                style.font_size_px().is_finite() && style.font_size_px() >= 0.0,
                "{css} produced a non-finite font size"
            );
            assert!(
                style.dpi().is_finite() && style.dpi() > 0.0,
                "{css} broke the dpi"
            );
            for value in style.padding(100.0) {
                assert!(value.is_finite(), "{css} produced a non-finite padding");
            }
            for corner in style.border_radii(100.0, 50.0) {
                assert!(
                    corner[0].is_finite() && corner[1].is_finite(),
                    "{css} produced a non-finite radius"
                );
            }
            assert!(
                (0.0..=1.0).contains(&style.opacity()),
                "{css} broke opacity"
            );
        }
    }

    #[test]
    fn a_restyle_pass_reuses_one_match_context_across_the_whole_tree() {
        // `MatchCx` is caller-owned precisely so a pass keeps the bloom filter
        // and nth-index caches warm; a reused context must not change answers.
        let sheet = adwaita();
        let env = ResolveEnv::default();
        let node = button(&["suggested-action"], PseudoStates::default());

        let mut shared = MatchCx::new();
        let first = ComputedStyle::resolve_chain(&sheet, &node, &env, &mut shared);
        let hovered_node = button(&[], PseudoStates::HOVER);
        let _ = ComputedStyle::resolve_chain(&sheet, &hovered_node, &env, &mut shared);
        let again = ComputedStyle::resolve_chain(&sheet, &node, &env, &mut shared);
        assert_eq!(first, again, "a reused MatchCx leaked state between nodes");

        let fresh = ComputedStyle::resolve_chain(&sheet, &node, &env, &mut MatchCx::new());
        assert_eq!(first, fresh, "a warm MatchCx disagrees with a cold one");
    }
}
