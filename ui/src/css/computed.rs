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

use cssparser::{Parser, ParserInput, Token};
use skia_rs_safe::core::Color;

use super::cascade::{CascadedValues, CompiledSheet, cascade};
use super::colors::{ColorTable, parse_color_value};
use super::select::CssNode;
use super::value::{comma_groups, component_values};

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
        }
    }
}

/// Parse a `<length>` in px. Unitless `0` is accepted; other units are not
/// (M1 has no font/viewport context to resolve `em`/`%` against).
///
/// Tokenized rather than string-sliced, so the unit is matched
/// ASCII-case-insensitively (`5PX`) and `NaNpx`/`infpx` -- which are
/// identifiers, not dimensions -- cannot slip through.
fn parse_px(value: &str) -> Option<f32> {
    let mut input = ParserInput::new(value);
    let mut parser = Parser::new(&mut input);
    let token = parser.next().ok()?.clone();
    if parser.expect_exhausted().is_err() {
        return None;
    }
    match token {
        Token::Dimension {
            value, ref unit, ..
        } if unit.eq_ignore_ascii_case("px") => value.is_finite().then_some(value),
        Token::Number { value: 0.0, .. } => Some(0.0),
        _ => None,
    }
}

/// A `<line-width>`: a length, or one of CSS's three width keywords.
fn parse_border_width(value: &str) -> Option<f32> {
    for (keyword, width) in [("thin", 1.0), ("medium", 3.0), ("thick", 5.0)] {
        if value.eq_ignore_ascii_case(keyword) {
            return Some(width);
        }
    }
    parse_px(value)
}

/// Parse one `<color> [<length>]` gradient stop from its components.
fn parse_stop(components: &[String], colors: &ColorTable) -> Option<GradientStop> {
    match components {
        [color] => Some(GradientStop {
            color: parse_color_value(color, colors)?,
            position_px: None,
        }),
        [color, position] => Some(GradientStop {
            color: parse_color_value(color, colors)?,
            position_px: Some(parse_px(position)?),
        }),
        _ => None,
    }
}

/// Parse a `background-image` value into a [`Background`].
///
/// Supports GTK's `image(<color>)` flat fill and the two-stop
/// `linear-gradient(to top, ...)` form Adwaita's buttons use. Anything else
/// -- radial gradients, `url()`, `-gtk-*` image functions, more than two
/// stops -- yields `None`, which leaves the `background-color` value (or the
/// transparent default) in place rather than painting something invented.
///
/// Function names and the `to top` keyword are matched
/// ASCII-case-insensitively, and the direction is compared component-wise so
/// `to  top` (or a newline between the two words) still matches.
fn parse_background_image(value: &str, colors: &ColorTable) -> Option<Background> {
    let components = component_values(value);
    let [single] = components.as_slice() else {
        return None;
    };
    let (name, rest) = single.split_once('(')?;
    let args = rest.strip_suffix(')')?;
    if name.eq_ignore_ascii_case("image") {
        return Some(Background::Solid(parse_color_value(args, colors)?));
    }
    if !name.eq_ignore_ascii_case("linear-gradient") {
        return None;
    }
    let groups = comma_groups(args);
    let [direction, first, second] = groups.as_slice() else {
        return None;
    };
    let [to, top] = direction.as_slice() else {
        return None;
    };
    if !to.eq_ignore_ascii_case("to") || !top.eq_ignore_ascii_case("top") {
        return None;
    }
    Some(Background::LinearGradientToTop {
        from: parse_stop(first, colors)?,
        to: parse_stop(second, colors)?,
    })
}

impl ComputedStyle {
    /// Resolve `node`'s style against `sheet`.
    #[must_use]
    pub fn resolve(sheet: &CompiledSheet, node: &CssNode) -> Self {
        Self::from_declarations(&cascade(sheet, node), &sheet.colors)
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
    pub fn from_declarations(values: &CascadedValues, colors: &ColorTable) -> Self {
        let mut style = Self::default();

        if let Some(color) = pick(values, "color", |v| parse_color_value(v, colors)) {
            style.color = color;
        }

        // `background-color` first, then `background-image` on top: CSS
        // paints the image over the color, and every background M1 supports
        // is fully opaque where it paints at all.
        if let Some(color) = pick(values, "background-color", |v| parse_color_value(v, colors)) {
            style.background = Background::Solid(color);
        }
        if let Some(background) = pick(values, "background-image", |value| {
            if value.eq_ignore_ascii_case("none") {
                // `none` is interpretable -- it just leaves the colour alone,
                // so it must not fall through to a runner-up image.
                Some(None)
            } else {
                parse_background_image(value, colors).map(Some)
            }
        })
        .flatten()
        {
            style.background = background;
        }

        // Borders are cascaded per side (`super::shorthand`); M1 paints a
        // uniform border, so the top side stands for all four. M2 widens
        // `ComputedStyle` to four sides.
        if let Some(width) = pick(values, "border-top-width", parse_border_width) {
            style.border_width = width.max(0.0);
        }
        // A `none`/`hidden` border style forces a used width of 0 -- that is
        // what makes `border: none` undo an earlier `border: 1px solid`.
        if let Some(style_keyword) = values.winner("border-top-style")
            && (style_keyword.eq_ignore_ascii_case("none")
                || style_keyword.eq_ignore_ascii_case("hidden"))
        {
            style.border_width = 0.0;
        }
        if let Some(color) = pick(values, "border-top-color", |v| parse_color_value(v, colors)) {
            style.border_color = color;
        }
        if let Some(radius) = pick(values, "border-radius", parse_px) {
            style.border_radius = radius.max(0.0);
        }
        for (index, name) in [
            "padding-top",
            "padding-right",
            "padding-bottom",
            "padding-left",
        ]
        .into_iter()
        .enumerate()
        {
            if let Some(padding) = pick(values, name, parse_px) {
                style.padding[index] = padding.max(0.0);
            }
        }
        if let Some(min_width) = pick(values, "min-width", parse_px) {
            style.min_width = min_width.max(0.0);
        }
        if let Some(min_height) = pick(values, "min-height", parse_px) {
            style.min_height = min_height.max(0.0);
        }
        if let Some(font_size) = pick(values, "font-size", parse_px) {
            style.font_size = font_size.max(0.0);
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
fn pick<T>(
    values: &CascadedValues,
    name: &str,
    mut parse: impl FnMut(&str) -> Option<T>,
) -> Option<T> {
    for (rank, candidate) in values.candidates(name).iter().enumerate() {
        if let Some(parsed) = parse(&candidate.value) {
            return Some(parsed);
        }
        tracing::debug!(
            property = name,
            value = %candidate.value,
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
    use crate::css::select::{CssNode, PseudoStates};
    use skia_rs_safe::core::Color;

    fn adwaita() -> CompiledSheet {
        CompiledSheet::compile(crate::BUNDLED_ADWAITA_LIGHT)
    }

    fn button(classes: &[&str], states: PseudoStates) -> CssNode {
        let window = CssNode::new("window", &["background"], PseudoStates::default(), None);
        CssNode::new("button", classes, states, Some(window))
    }

    fn stop(color: u32, position_px: Option<f32>) -> GradientStop {
        GradientStop {
            color: Color(color),
            position_px,
        }
    }

    #[test]
    fn non_finite_lengths_are_rejected() {
        assert_eq!(super::parse_px("NaNpx"), None);
        assert_eq!(super::parse_px("infpx"), None);
        assert_eq!(super::parse_px("1e40px"), None);
        assert_eq!(super::parse_px("4px"), Some(4.0));
        assert_eq!(super::parse_px("0"), Some(0.0));
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
        let hovered = ComputedStyle::resolve(
            &sheet,
            &button(
                &[],
                PseudoStates {
                    hover: true,
                    ..PseudoStates::default()
                },
            ),
        );
        assert_eq!(
            hovered.background,
            Background::LinearGradientToTop {
                from: stop(0xFFD6_D1CD, None),
                to: stop(0xFFE8_E6E3, Some(1.0)),
            }
        );
        assert_eq!(hovered.border_color, Color(0xFFCD_C7C2));

        let active = ComputedStyle::resolve(
            &sheet,
            &button(
                &[],
                PseudoStates {
                    active: true,
                    ..PseudoStates::default()
                },
            ),
        );
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

        let pressed = ComputedStyle::resolve(
            &sheet,
            &button(
                &["suggested-action"],
                PseudoStates {
                    active: true,
                    ..PseudoStates::default()
                },
            ),
        );
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
}
