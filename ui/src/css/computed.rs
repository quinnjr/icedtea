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

use std::collections::HashMap;

use skia_rs_safe::core::Color;

use super::cascade::{CompiledSheet, cascade};
use super::colors::{ColorTable, parse_color_value};
use super::select::CssNode;

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
fn parse_px(value: &str) -> Option<f32> {
    let value = value.trim();
    if let Some(number) = value.strip_suffix("px") {
        return number.trim().parse::<f32>().ok().filter(|n| n.is_finite());
    }
    let number = value.parse::<f32>().ok().filter(|n| n.is_finite())?;
    (number == 0.0).then_some(0.0)
}

/// Split a function call `name(args)` into `(name, args)`.
fn split_function(value: &str) -> Option<(&str, &str)> {
    let value = value.trim();
    let open = value.find('(')?;
    let inner = value.strip_suffix(')')?;
    Some((value[..open].trim(), &inner[open + 1..]))
}

/// Split `args` on top-level commas (parenthesised commas stay together).
fn split_top_level_commas(args: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut current = String::new();
    for ch in args.chars() {
        match ch {
            '(' => {
                depth += 1;
                current.push(ch);
            }
            ')' => {
                depth = depth.saturating_sub(1);
                current.push(ch);
            }
            ',' if depth == 0 => {
                parts.push(current.trim().to_string());
                current = String::new();
            }
            _ => current.push(ch),
        }
    }
    if !current.trim().is_empty() {
        parts.push(current.trim().to_string());
    }
    parts
}

/// Parse one `<color> [<length>]` gradient stop.
fn parse_stop(text: &str, colors: &ColorTable) -> Option<GradientStop> {
    let text = text.trim();
    if let Some((color_text, position_text)) = text.rsplit_once(char::is_whitespace)
        && let Some(position) = parse_px(position_text)
    {
        return Some(GradientStop {
            color: parse_color_value(color_text, colors)?,
            position_px: Some(position),
        });
    }
    Some(GradientStop {
        color: parse_color_value(text, colors)?,
        position_px: None,
    })
}

/// Parse a `background-image` value into a [`Background`].
///
/// Supports GTK's `image(<color>)` flat fill and the two-stop
/// `linear-gradient(to top, ...)` form Adwaita's buttons use. Anything else
/// -- radial gradients, `url()`, `-gtk-*` image functions, more than two
/// stops -- yields `None`, which leaves the `background-color` value (or the
/// transparent default) in place rather than painting something invented.
fn parse_background_image(value: &str, colors: &ColorTable) -> Option<Background> {
    let (name, args) = split_function(value)?;
    match name {
        "image" => Some(Background::Solid(parse_color_value(args, colors)?)),
        "linear-gradient" => {
            let parts = split_top_level_commas(args);
            let [direction, first, second] = parts.as_slice() else {
                return None;
            };
            if direction.trim() != "to top" {
                return None;
            }
            Some(Background::LinearGradientToTop {
                from: parse_stop(first, colors)?,
                to: parse_stop(second, colors)?,
            })
        }
        _ => None,
    }
}

/// Parse a CSS 1-to-4-value `padding` shorthand into `[top, right, bottom, left]`.
fn parse_padding(value: &str) -> Option<[f32; 4]> {
    let parts: Vec<f32> = value
        .split_whitespace()
        .map(parse_px)
        .collect::<Option<Vec<f32>>>()?;
    Some(match parts.as_slice() {
        [all] => [*all; 4],
        [tb, lr] => [*tb, *lr, *tb, *lr],
        [t, lr, b] => [*t, *lr, *b, *lr],
        [t, r, b, l] => [*t, *r, *b, *l],
        _ => return None,
    })
}

impl ComputedStyle {
    /// Resolve `node`'s style against `sheet`.
    #[must_use]
    pub fn resolve(sheet: &CompiledSheet, node: &CssNode) -> Self {
        Self::from_declarations(&cascade(sheet, node), &sheet.colors)
    }

    /// Resolve a set of winning declarations. Separated from [`Self::resolve`]
    /// so the property logic is testable without a node tree.
    #[must_use]
    pub fn from_declarations(declarations: &HashMap<String, String>, colors: &ColorTable) -> Self {
        let mut style = Self::default();
        let get = |name: &str| declarations.get(name).map(String::as_str);

        if let Some(color) = get("color").and_then(|v| parse_color_value(v, colors)) {
            style.color = color;
        }

        // `background-color` first, then `background-image` on top: CSS
        // paints the image over the color, and every background M1 supports
        // is fully opaque where it paints at all.
        if let Some(color) = get("background-color").and_then(|v| parse_color_value(v, colors)) {
            style.background = Background::Solid(color);
        }
        if let Some(value) = get("background-image")
            && value.trim() != "none"
            && let Some(background) = parse_background_image(value, colors)
        {
            style.background = background;
        }

        // `border: <width> <style> [<color>]` -- Adwaita writes `1px solid`
        // with no color and sets `border-color` separately.
        if let Some(value) = get("border") {
            for part in value.split_whitespace() {
                if let Some(width) = parse_px(part) {
                    style.border_width = width;
                } else if let Some(color) = parse_color_value(part, colors) {
                    style.border_color = color;
                }
            }
        }
        if let Some(width) = get("border-width").and_then(parse_px) {
            style.border_width = width;
        }
        if let Some(color) = get("border-color").and_then(|v| parse_color_value(v, colors)) {
            style.border_color = color;
        }
        if let Some(radius) = get("border-radius").and_then(parse_px) {
            style.border_radius = radius;
        }
        if let Some(padding) = get("padding").and_then(parse_padding) {
            style.padding = padding;
        }
        if let Some(min_width) = get("min-width").and_then(parse_px) {
            style.min_width = min_width;
        }
        if let Some(min_height) = get("min-height").and_then(parse_px) {
            style.min_height = min_height;
        }
        if let Some(font_size) = get("font-size").and_then(parse_px) {
            style.font_size = font_size;
        }

        style
    }
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
}
