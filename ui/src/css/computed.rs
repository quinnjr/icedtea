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

use skia_rs_safe::core::Color;

use super::cascade::{CascadedValues, CompiledSheet, cascade};
use super::node::Node;
use super::registry::Prop;
use super::select::MatchCx;
use super::value::Value;
use super::value::color::ColorTable;

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

/// The properties the M1 slice needs, fully resolved.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ComputedStyle {
    /// Resolved background.
    pub background: Background,
    /// Foreground (text) color.
    pub color: Color,
    /// Uniform border width in px.
    pub border_width: f32,
    /// Border color.
    pub border_color: Color,
    /// Uniform corner radius in px.
    pub border_radius: f32,
    /// `[top, right, bottom, left]` in px.
    pub padding: [f32; 4],
    /// `min-width` in px.
    pub min_width: f32,
    /// `min-height` in px.
    pub min_height: f32,
    /// Font size in px.
    pub font_size: f32,
    /// Which box the background is clipped to.
    pub background_clip: BackgroundClip,
}

impl ComputedStyle {
    /// The font size used when no rule sets one.
    ///
    /// No Adwaita `button` rule declares `font-size`; GTK inherits it from
    /// the desktop's font setting, which M1 does not read.
    pub const DEFAULT_FONT_SIZE: f32 = 14.0;
}

impl Default for ComputedStyle {
    fn default() -> Self {
        Self {
            background: Background::Transparent,
            color: Color::BLACK,
            border_width: 0.0,
            border_color: Color::TRANSPARENT,
            border_radius: 0.0,
            padding: [0.0; 4],
            min_width: 0.0,
            min_height: 0.0,
            font_size: Self::DEFAULT_FONT_SIZE,
            background_clip: BackgroundClip::default(),
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
    /// Resolve `node`'s style against `sheet`, resolving its whole ancestor
    /// chain first so inherited properties (`color`, `font-size`) and
    /// `currentColor` have something to inherit *from*.
    ///
    /// A caller that already holds the parent's computed style should use
    /// [`Self::resolve_with_parent`] and skip the walk.
    #[must_use]
    pub fn resolve(sheet: &CompiledSheet, node: &Node) -> Self {
        let mut cx = MatchCx::new();
        let mut chain = vec![node.clone()];
        while let Some(parent) = chain.last().and_then(Node::parent) {
            chain.push(parent);
        }
        let mut style: Option<Self> = None;
        for ancestor in chain.iter().rev() {
            style = Some(Self::from_declarations(
                &cascade(sheet, ancestor, &mut cx),
                &sheet.colors,
                style.as_ref(),
            ));
        }
        style.unwrap_or_default()
    }

    /// Resolve `node`'s style given its parent's already-computed style.
    #[must_use]
    pub fn resolve_with_parent(sheet: &CompiledSheet, node: &Node, parent: Option<&Self>) -> Self {
        let mut cx = MatchCx::new();
        Self::from_declarations(&cascade(sheet, node, &mut cx), &sheet.colors, parent)
    }

    /// Resolve a node's cascaded declarations. Separated from
    /// [`Self::resolve`] so the property logic is testable without a node
    /// tree.
    ///
    /// Every property is read through [`pick`], which walks the cascade's
    /// runner-ups: a winner this milestone cannot interpret
    /// (`border-radius: 100%`) yields to the next declaration that applied
    /// instead of reverting to the initial value.
    #[must_use]
    pub fn from_declarations(
        values: &CascadedValues,
        colors: &ColorTable,
        parent: Option<&Self>,
    ) -> Self {
        let mut style = Self::default();
        let inherited_color = parent.map_or(Color::BLACK, |parent| parent.color);
        let parent_font_size = parent.map_or(Self::DEFAULT_FONT_SIZE, |parent| parent.font_size);
        let font_ctx = m1_bridge::ctx(parent_font_size, Some(parent_font_size));

        style.color = pick(values, Prop::Color, |value| {
            m1_bridge::color(value, colors, inherited_color)
        })
        .unwrap_or(inherited_color);
        style.font_size = pick(values, Prop::FontSize, |value| {
            m1_bridge::px(value, &font_ctx)
        })
        .map(|size| size.max(0.0))
        .unwrap_or(parent_font_size);

        let current = style.color;
        let ctx = m1_bridge::ctx(style.font_size, None);

        if let Some(color) = pick(values, Prop::BackgroundColor, |value| {
            m1_bridge::color(value, colors, current)
        }) {
            style.background = Background::Solid(color);
        }
        if let Some(background) = pick(values, Prop::BackgroundImage, |value| {
            m1_bridge::background(value, colors, current, &ctx)
        })
        .flatten()
        {
            style.background = background;
        }

        // Borders are cascaded per side; M1 paints a uniform border, so the top
        // side stands for all four. Task 3 widens `ComputedStyle` to four sides.
        if let Some(width) = pick(values, Prop::BorderTopWidth, |value| {
            m1_bridge::px(value, &ctx)
        }) {
            style.border_width = width.max(0.0);
        }
        if pick(
            values,
            Prop::BorderTopStyle,
            m1_bridge::border_style_is_none,
        ) == Some(true)
        {
            style.border_width = 0.0;
        }
        if let Some(color) = pick(values, Prop::BorderTopColor, |value| {
            m1_bridge::color(value, colors, current)
        }) {
            style.border_color = color;
        }
        if let Some(radius) = pick(values, Prop::BorderTopLeftRadius, |value| {
            m1_bridge::radius(value, &ctx)
        }) {
            style.border_radius = radius.max(0.0);
        }
        for (index, prop) in [
            Prop::PaddingTop,
            Prop::PaddingRight,
            Prop::PaddingBottom,
            Prop::PaddingLeft,
        ]
        .into_iter()
        .enumerate()
        {
            if let Some(padding) = pick(values, prop, |value| m1_bridge::px(value, &ctx)) {
                style.padding[index] = padding.max(0.0);
            }
        }
        if let Some(clip) = pick(values, Prop::BackgroundClip, m1_bridge::clip) {
            style.background_clip = clip;
        }
        if let Some(min_width) = pick(values, Prop::MinWidth, |value| m1_bridge::px(value, &ctx)) {
            style.min_width = min_width.max(0.0);
        }
        if let Some(min_height) = pick(values, Prop::MinHeight, |value| m1_bridge::px(value, &ctx))
        {
            style.min_height = min_height.max(0.0);
        }
        style
    }
}

/// Read one property, stepping past declarations this engine cannot
/// interpret.
///
/// CSS proper calls an uninterpretable computed value "invalid at
/// computed-value time" and falls back to the inherited or initial value.
/// While M1's property coverage is this narrow that would throw away a
/// perfectly applicable declaration -- Adwaita's `.sidebar-button` would
/// lose its 5px radius to a `100%` this engine has no percentage context
/// for -- so the next declaration in cascade order is used instead. The
/// divergence is logged.
///
/// Known consequence, both directions: an uninterpretable *winner* does not
/// leave the property at its initial value, it leaves it at whatever an
/// earlier rule declared. Task 3 replaces this with CSS's own rule.
fn pick<T>(
    values: &CascadedValues,
    prop: Prop,
    mut parse: impl FnMut(&Value) -> Option<T>,
) -> Option<T> {
    for (rank, candidate) in values.candidates(prop).iter().enumerate() {
        if let Some(parsed) = parse(&candidate.value) {
            return Some(parsed);
        }
        tracing::debug!(
            property = prop.name(),
            value = ?candidate.value,
            rank,
            "uninterpretable declaration; falling back to the next in cascade order"
        );
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{Background, ComputedStyle, GradientStop};
    use crate::css::cascade::CompiledSheet;
    use crate::css::node::{Node, PseudoStates};
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
        use crate::css::registry::{Prop, parse_declaration_value};
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
        use crate::css::value::color::Rgba;
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
        let s = ComputedStyle::resolve(&adwaita(), &button(&[], PseudoStates::default()));
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
        let hovered = ComputedStyle::resolve(&sheet, &button(&[], PseudoStates::HOVER));
        assert_eq!(
            hovered.background,
            Background::LinearGradientToTop {
                from: stop(0xFFD6_D1CD, None),
                to: stop(0xFFE8_E6E3, Some(1.0)),
            }
        );
        assert_eq!(hovered.border_color, Color(0xFFCD_C7C2));

        let active = ComputedStyle::resolve(&sheet, &button(&[], PseudoStates::ACTIVE));
        assert_eq!(active.background, Background::Solid(Color(0xFFDA_D6D2)));
        assert_eq!(
            active.border_radius, 5.0,
            "radius must survive the state change"
        );
    }

    #[test]
    fn adwaita_suggested_action_button() {
        let sheet = adwaita();
        let s = ComputedStyle::resolve(
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

        let pressed =
            ComputedStyle::resolve(&sheet, &button(&["suggested-action"], PseudoStates::ACTIVE));
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
        let s = ComputedStyle::resolve(&sheet, &button(&[], PseudoStates::default()));
        assert_eq!(s.background, Background::Solid(Color(0xFF11_2233)));
    }

    #[test]
    fn background_image_none_falls_back_to_background_color() {
        let sheet =
            CompiledSheet::compile("button { background-color: #112233; background-image: none }");
        let s = ComputedStyle::resolve(&sheet, &button(&[], PseudoStates::default()));
        assert_eq!(s.background, Background::Solid(Color(0xFF11_2233)));
    }

    #[test]
    fn named_color_references_resolve_through_define_color() {
        let sheet = CompiledSheet::compile(
            "@define-color accent_color #3584E4;\nbutton { background-color: @accent_color }",
        );
        let s = ComputedStyle::resolve(&sheet, &button(&[], PseudoStates::default()));
        assert_eq!(s.background, Background::Solid(Color(0xFF35_84E4)));
    }

    #[test]
    fn defaults_apply_when_nothing_matches() {
        let sheet = CompiledSheet::compile("entry { color: red }");
        let s = ComputedStyle::resolve(&sheet, &button(&[], PseudoStates::default()));
        assert_eq!(s.background, Background::Transparent);
        assert_eq!(s.color, Color::BLACK);
        assert_eq!(s.border_width, 0.0);
        assert_eq!(s.border_radius, 0.0);
        assert_eq!(s.padding, [0.0; 4]);
        assert_eq!(s.font_size, ComputedStyle::DEFAULT_FONT_SIZE);
    }

    #[test]
    fn per_side_padding_longhands_override_the_shorthand() {
        // E1: `padding-left`/`padding-right` were never read, so Adwaita's
        // `button.image-button` (padding-left/right: 5px over the base
        // `padding: 4px 9px`) computed as [4, 9, 4, 9].
        let s = ComputedStyle::resolve(
            &adwaita(),
            &button(&["image-button"], PseudoStates::default()),
        );
        assert_eq!(s.padding, [4.0, 5.0, 4.0, 5.0]);
        let t = ComputedStyle::resolve(
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
        let s = ComputedStyle::resolve(&sheet, &button(&["flat"], PseudoStates::default()));
        assert_eq!(s.border_width, 0.0);
    }

    #[test]
    fn a_function_valued_border_colour_is_not_shredded() {
        // E1: splitting `border` on whitespace turned `rgb(0 0 0)` into
        // `0`, which hit the unitless-zero length path and zeroed the width.
        let sheet = CompiledSheet::compile("button { border: 1px solid rgb(0 0 0) }");
        let s = ComputedStyle::resolve(&sheet, &button(&[], PseudoStates::default()));
        assert_eq!(s.border_width, 1.0);
        assert_eq!(s.border_color, Color(0xFF00_0000));
    }

    #[test]
    fn the_background_shorthand_is_read() {
        // E1: `background` was never read at all.
        let sheet = CompiledSheet::compile("button { background: #112233 }");
        assert_eq!(
            ComputedStyle::resolve(&sheet, &button(&[], PseudoStates::default())).background,
            Background::Solid(Color(0xFF11_2233))
        );
        let sheet = CompiledSheet::compile("button { background: image(#dad6d2) }");
        assert_eq!(
            ComputedStyle::resolve(&sheet, &button(&[], PseudoStates::default())).background,
            Background::Solid(Color(0xFFDA_D6D2))
        );
        // The shorthand resets the longhands it does not name.
        let sheet = CompiledSheet::compile(
            "button { background-image: image(#dad6d2) }\nbutton { background: #112233 }",
        );
        assert_eq!(
            ComputedStyle::resolve(&sheet, &button(&[], PseudoStates::default())).background,
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
            ComputedStyle::resolve(&later_shorthand, &button(&[], PseudoStates::default()))
                .border_width,
            1.0
        );
        let later_longhand = CompiledSheet::compile(
            "button { border: 1px solid red }\nbutton { border-width: 5px }",
        );
        assert_eq!(
            ComputedStyle::resolve(&later_longhand, &button(&[], PseudoStates::default()))
                .border_width,
            5.0
        );
    }

    #[test]
    fn an_uninterpretable_winner_falls_back_to_the_runner_up() {
        // E3: Adwaita:1606 `button.sidebar-button { border-radius: 100% }`.
        // M1 has no percentage lengths, so the winner is uninterpretable and
        // the next applicable declaration -- the base button's 5px -- is used
        // instead of silently reverting to the 0 default.
        let s = ComputedStyle::resolve(
            &adwaita(),
            &button(&["sidebar-button"], PseudoStates::default()),
        );
        assert_eq!(s.border_radius, 5.0);

        let sheet = CompiledSheet::compile(
            "button { border-radius: 5px }\nbutton.round { border-radius: 100% }",
        );
        assert_eq!(
            ComputedStyle::resolve(&sheet, &button(&["round"], PseudoStates::default()))
                .border_radius,
            5.0
        );
    }

    #[test]
    fn negative_lengths_clamp_to_zero() {
        // E10: a negative padding or radius painted outside the box.
        let sheet = CompiledSheet::compile(
            "button { padding: -4px; border: -1px solid red; border-radius: -5px; \
             min-width: -1px; min-height: -2px }",
        );
        let s = ComputedStyle::resolve(&sheet, &button(&[], PseudoStates::default()));
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
        let s = ComputedStyle::resolve(&sheet, &button(&[], PseudoStates::default()));
        assert_eq!(s.color, Color(0xFFFF_0000));
        assert_eq!(s.font_size, 20.0);
        // The node's own declaration still wins over the inherited value.
        let sheet = CompiledSheet::compile(
            "window { color: #ff0000; font-size: 20px }\n             button { color: #00ff00; font-size: 11px }",
        );
        let s = ComputedStyle::resolve(&sheet, &button(&[], PseudoStates::default()));
        assert_eq!(s.color, Color(0xFF00_FF00));
        assert_eq!(s.font_size, 11.0);
    }

    #[test]
    fn non_inherited_properties_do_not_leak_down() {
        let sheet = CompiledSheet::compile("window { padding: 7px; border-radius: 9px }");
        let s = ComputedStyle::resolve(&sheet, &button(&[], PseudoStates::default()));
        assert_eq!(s.padding, [0.0; 4]);
        assert_eq!(s.border_radius, 0.0);
    }

    #[test]
    fn current_color_resolves_against_the_elements_own_colour() {
        let sheet = CompiledSheet::compile(
            "window { color: #ff0000 }\n             button { border: 1px solid currentColor }",
        );
        let s = ComputedStyle::resolve(&sheet, &button(&[], PseudoStates::default()));
        assert_eq!(
            s.border_color,
            Color(0xFFFF_0000),
            "currentColor is the inherited colour"
        );

        let sheet = CompiledSheet::compile(
            "window { color: #ff0000 }\n             button { color: #0000ff; border: 1px solid currentColor;              background-color: currentColor }",
        );
        let s = ComputedStyle::resolve(&sheet, &button(&[], PseudoStates::default()));
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
        let s = ComputedStyle::resolve(&sheet, &button(&[], PseudoStates::default()));
        assert_eq!(s.color, Color(0xFFFF_0000));
    }

    #[test]
    fn resolve_with_parent_matches_resolving_the_whole_chain() {
        let sheet = CompiledSheet::compile("window { color: #ff0000 }\nbutton { padding: 2px }");
        let window = Node::with_classes("window", &["background"]);
        let node = Node::new("button");
        window.append_child(&node);
        let window_style = ComputedStyle::resolve(&sheet, &window);
        assert_eq!(
            ComputedStyle::resolve_with_parent(&sheet, &node, Some(&window_style)),
            ComputedStyle::resolve(&sheet, &node)
        );
    }

    #[test]
    fn border_line_width_keywords_resolve_to_pixels() {
        // Review round 1: `thin` was classified as the border *colour*, so
        // the width fell back to `medium` (3px) and the colour reset was lost.
        let sheet =
            CompiledSheet::compile("window { color: #ff0000 }\nbutton { border: thin solid }");
        let s = ComputedStyle::resolve(&sheet, &button(&[], PseudoStates::default()));
        assert_eq!(s.border_width, 1.0);
        assert_eq!(
            s.border_color,
            Color(0xFFFF_0000),
            "the omitted colour resets to currentColor, i.e. the inherited colour"
        );

        let sheet = CompiledSheet::compile("button { border: thick dotted red }");
        let s = ComputedStyle::resolve(&sheet, &button(&[], PseudoStates::default()));
        assert_eq!(s.border_width, 5.0);
        assert_eq!(s.border_color, Color(0xFFFF_0000));

        // ...and the reset is not undone by a runner-up the shorthand beat.
        let sheet = CompiledSheet::compile(
            "button { border-color: #00ff00 }\nbutton { border: thin solid }",
        );
        let s = ComputedStyle::resolve(&sheet, &button(&[], PseudoStates::default()));
        assert_eq!(s.border_width, 1.0);
        assert_eq!(
            s.border_color,
            Color::BLACK,
            "currentColor, not the reset colour"
        );
    }
}
