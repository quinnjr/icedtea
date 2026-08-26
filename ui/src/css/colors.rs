//! GTK's `@define-color` table and the colour-value grammar the engine needs.
//!
//! Colour values are parsed from a **token stream**, never from string
//! slicing: `Color::from_css` panics on `#a<non-ascii>` (it slices a hex
//! literal by byte index) and is exact-case throughout, so a theme carrying
//! `RGB(53 132 228)` or one stray non-ASCII byte would silently drop a
//! colour or take the process down. This module therefore implements the
//! grammar itself -- hex 3/4/6/8, `rgb()`/`rgba()`/`hsl()`/`hsla()` in both
//! CSS Color 3 comma syntax and CSS Color 4 space syntax with `/ <alpha>`
//! and percentages, `transparent`, `currentColor`, and GTK's `@name`
//! references -- and delegates only *named* colours (a table lookup that
//! cannot panic) to `Color::from_css`.
//!
//! Not implemented (M2): GTK's `alpha()`, `shade()` and `mix()` functions,
//! and CSS Color Level 5 relative syntax (`rgb(from ...)`). Entries whose
//! value needs them are left unresolved rather than guessed at. Colour
//! functions this module does not model (`lab()`, `oklch()`, `hwb()`,
//! `color()`) are handed to `Color::from_css` as a whole -- safe, because
//! only its hex path can panic and a function value never reaches it.

use std::collections::HashMap;

use cssparser::{ParseError, Parser, ParserInput, Token};
use skia_rs_safe::core::Color;

/// Resolved `@define-color` names.
pub type ColorTable = HashMap<String, Color>;

/// A colour value that may not be knowable without the element's own colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorRef {
    /// A colour that stands on its own.
    Absolute(Color),
    /// `currentColor`: whatever the element's computed `color` resolves to.
    CurrentColor,
}

/// Parse one colour value against `table`, keeping `currentColor` as a marker.
///
/// Returns `None` for anything this milestone cannot interpret -- including
/// values that are not colours at all (`linear-gradient(...)`), which is how
/// `super::computed` distinguishes a flat background from a gradient.
#[must_use]
pub fn parse_color_ref(value: &str, table: &ColorTable) -> Option<ColorRef> {
    let mut input = ParserInput::new(value);
    let mut parser = Parser::new(&mut input);
    let parsed = parse_one(&mut parser, table);
    // A colour value is the *whole* value: `#2e3436 red` is not a colour.
    if parser.expect_exhausted().is_err() {
        return None;
    }
    if parsed.is_none() {
        return fallback_color_function(value).map(ColorRef::Absolute);
    }
    parsed
}

/// Colour functions this module does not model (`lab()`, `oklch()`,
/// `hwb()`, `color()`) still reach `Color::from_css`, which implements
/// them. Only a *function* value is handed over, never a hex literal, so
/// the panicking `parse_hex_color` path is unreachable.
fn fallback_color_function(value: &str) -> Option<Color> {
    let value = value.trim();
    let name = value.split('(').next()?;
    if value.starts_with('#') || value.starts_with('@') || !value.ends_with(')') {
        return None;
    }
    // The four this module owns must not be second-guessed: a failure there
    // means the value is genuinely out of scope (CSS Color 5 relative
    // syntax), not merely unmodelled.
    if ["rgb", "rgba", "hsl", "hsla"]
        .iter()
        .any(|owned| name.eq_ignore_ascii_case(owned))
    {
        return None;
    }
    Color::from_css(value)
}

/// Parse one colour value against `table`.
///
/// `currentColor` yields `None`: it cannot be resolved without an element.
/// Callers that can resolve it use [`parse_color_ref`].
#[must_use]
pub fn parse_color_value(value: &str, table: &ColorTable) -> Option<Color> {
    match parse_color_ref(value, table)? {
        ColorRef::Absolute(color) => Some(color),
        ColorRef::CurrentColor => None,
    }
}

fn parse_one(input: &mut Parser<'_, '_>, table: &ColorTable) -> Option<ColorRef> {
    let token = input.next().ok()?.clone();
    match token {
        // GTK's `@name` reference into the `@define-color` table.
        Token::AtKeyword(name) => table.get(name.as_ref()).copied().map(ColorRef::Absolute),
        Token::Hash(hex) | Token::IDHash(hex) => parse_hex(hex.as_ref()).map(ColorRef::Absolute),
        Token::Ident(name) => {
            if name.eq_ignore_ascii_case("currentcolor") {
                Some(ColorRef::CurrentColor)
            } else {
                // A bare identifier can only reach `from_css`'s named-colour
                // table (itself ASCII-case-insensitive): no hex slicing, so
                // no panic.
                Color::from_css(name.as_ref()).map(ColorRef::Absolute)
            }
        }
        Token::Function(name) => {
            let args = input
                .parse_nested_block(|inner| {
                    Ok::<Vec<Arg>, ParseError<'_, ()>>(collect_arguments(inner))
                })
                .ok()?;
            let color = if name.eq_ignore_ascii_case("rgb") || name.eq_ignore_ascii_case("rgba") {
                parse_rgb(&args)
            } else if name.eq_ignore_ascii_case("hsl") || name.eq_ignore_ascii_case("hsla") {
                parse_hsl(&args)
            } else {
                None
            };
            color.map(ColorRef::Absolute)
        }
        _ => None,
    }
}

/// `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`.
///
/// Every digit is checked to be ASCII hex *before* any slicing, which is the
/// whole point: `Color::from_css("#a\u{e9}")` panics on a char boundary.
fn parse_hex(hex: &str) -> Option<Color> {
    if !hex.is_ascii() || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let nibble = |i: usize| u8::from_str_radix(&hex[i..=i], 16).ok();
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some(match hex.len() {
        3 => Color::from_rgb(nibble(0)? * 17, nibble(1)? * 17, nibble(2)? * 17),
        4 => Color::from_argb(
            nibble(3)? * 17,
            nibble(0)? * 17,
            nibble(1)? * 17,
            nibble(2)? * 17,
        ),
        6 => Color::from_rgb(byte(0)?, byte(2)?, byte(4)?),
        8 => Color::from_argb(byte(6)?, byte(0)?, byte(2)?, byte(4)?),
        _ => return None,
    })
}

/// One argument token inside a colour function.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Arg {
    /// A plain number.
    Number(f32),
    /// A percentage, as a fraction (`50%` -> 0.5).
    Percent(f32),
    /// An angle, normalized to degrees.
    Angle(f32),
    /// The `/` that introduces the CSS Color 4 alpha component.
    Slash,
    /// A `,` separator.
    Comma,
    /// Anything else -- `from`, `calc(...)`, a stray identifier. Its
    /// presence makes the function uninterpretable.
    Other,
}

fn collect_arguments(input: &mut Parser<'_, '_>) -> Vec<Arg> {
    let mut args = Vec::new();
    while let Ok(token) = input.next() {
        args.push(match token {
            Token::Number { value, .. } => Arg::Number(*value),
            Token::Percentage { unit_value, .. } => Arg::Percent(*unit_value),
            Token::Dimension { value, unit, .. } => {
                angle_degrees(*value, unit).map_or(Arg::Other, Arg::Angle)
            }
            Token::Comma => Arg::Comma,
            Token::Delim('/') => Arg::Slash,
            _ => Arg::Other,
        });
    }
    args
}

fn angle_degrees(value: f32, unit: &str) -> Option<f32> {
    if unit.eq_ignore_ascii_case("deg") {
        Some(value)
    } else if unit.eq_ignore_ascii_case("grad") {
        Some(value * 360.0 / 400.0)
    } else if unit.eq_ignore_ascii_case("rad") {
        Some(value.to_degrees())
    } else if unit.eq_ignore_ascii_case("turn") {
        Some(value * 360.0)
    } else {
        None
    }
}

/// Drop comma separators and split the argument list at its `/`.
///
/// Returns `None` if the list carries anything uninterpretable, or more than
/// one `/`.
fn channels(args: &[Arg]) -> Option<(Vec<Arg>, Option<Arg>)> {
    let mut head = Vec::new();
    let mut alpha: Option<Arg> = None;
    let mut after_slash = false;
    for arg in args {
        match arg {
            Arg::Comma => {}
            Arg::Slash if !after_slash => after_slash = true,
            Arg::Other | Arg::Slash => return None,
            value if after_slash => {
                if alpha.replace(*value).is_some() {
                    return None;
                }
            }
            value => head.push(*value),
        }
    }
    if after_slash && alpha.is_none() {
        return None;
    }
    Some((head, alpha))
}

/// A 0-255 channel from a number (already 0-255) or a percentage.
fn channel_byte(arg: Arg) -> Option<u8> {
    let value = match arg {
        Arg::Number(number) => number,
        Arg::Percent(fraction) => fraction * 255.0,
        _ => return None,
    };
    (!value.is_nan()).then(|| value.round().clamp(0.0, 255.0) as u8)
}

/// The alpha byte for an explicit alpha argument, defaulting to opaque.
fn alpha_byte(arg: Option<Arg>) -> Option<u8> {
    let Some(arg) = arg else {
        return Some(255);
    };
    let value = match arg {
        Arg::Number(number) => number * 255.0,
        Arg::Percent(fraction) => fraction * 255.0,
        _ => return None,
    };
    (!value.is_nan()).then(|| value.round().clamp(0.0, 255.0) as u8)
}

fn parse_rgb(args: &[Arg]) -> Option<Color> {
    let (head, mut alpha) = channels(args)?;
    let head = match head.as_slice() {
        // Legacy `rgba(r, g, b, a)`: the alpha rides in the head.
        [r, g, b, a] if alpha.is_none() => {
            alpha = Some(*a);
            [*r, *g, *b]
        }
        [r, g, b] => [*r, *g, *b],
        _ => return None,
    };
    Some(Color::from_argb(
        alpha_byte(alpha)?,
        channel_byte(head[0])?,
        channel_byte(head[1])?,
        channel_byte(head[2])?,
    ))
}

fn parse_hsl(args: &[Arg]) -> Option<Color> {
    let (head, mut alpha) = channels(args)?;
    let head = match head.as_slice() {
        [h, s, l, a] if alpha.is_none() => {
            alpha = Some(*a);
            [*h, *s, *l]
        }
        [h, s, l] => [*h, *s, *l],
        _ => return None,
    };
    let hue = match head[0] {
        Arg::Number(number) => number,
        Arg::Angle(degrees) => degrees,
        Arg::Percent(_) | Arg::Slash | Arg::Comma | Arg::Other => return None,
    };
    // CSS Color 4 allows a bare number for saturation/lightness too.
    let fraction = |arg: Arg| match arg {
        Arg::Percent(fraction) => Some(fraction),
        Arg::Number(number) => Some(number / 100.0),
        _ => None,
    };
    let (red, green, blue) = hsl_to_rgb(hue, fraction(head[1])?, fraction(head[2])?)?;
    Some(Color::from_argb(alpha_byte(alpha)?, red, green, blue))
}

fn hsl_to_rgb(hue_degrees: f32, saturation: f32, lightness: f32) -> Option<(u8, u8, u8)> {
    if !hue_degrees.is_finite() || !saturation.is_finite() || !lightness.is_finite() {
        return None;
    }
    let saturation = saturation.clamp(0.0, 1.0);
    let lightness = lightness.clamp(0.0, 1.0);
    let hue = hue_degrees.rem_euclid(360.0) / 60.0;
    let chroma = (1.0 - (2.0f32.mul_add(lightness, -1.0)).abs()) * saturation;
    let second = chroma * (1.0 - (hue % 2.0 - 1.0).abs());
    let (r, g, b) = match hue as u32 {
        0 => (chroma, second, 0.0),
        1 => (second, chroma, 0.0),
        2 => (0.0, chroma, second),
        3 => (0.0, second, chroma),
        4 => (second, 0.0, chroma),
        _ => (chroma, 0.0, second),
    };
    let base = lightness - chroma / 2.0;
    let byte = |value: f32| ((value + base) * 255.0).round().clamp(0.0, 255.0) as u8;
    Some((byte(r), byte(g), byte(b)))
}

/// Build the colour table from `@define-color` pairs in source order.
///
/// Resolution is incremental and order-sensitive, exactly as GTK's is: a
/// definition may reference any name defined *above* it, and a later
/// redefinition of a name wins for everything below it.
#[must_use]
pub fn build_color_table(definitions: &[(String, String)]) -> ColorTable {
    let mut table = ColorTable::new();
    for (name, value) in definitions {
        match parse_color_value(value, &table) {
            Some(color) => {
                table.insert(name.clone(), color);
            }
            None => {
                tracing::debug!(%name, %value, "unresolved @define-color; skipping");
            }
        }
    }
    table
}

#[cfg(test)]
mod tests {
    use super::{ColorRef, ColorTable, build_color_table, parse_color_ref, parse_color_value};
    use crate::css::parse::parse_stylesheet;
    use skia_rs_safe::core::Color;

    fn adwaita_table() -> ColorTable {
        build_color_table(&parse_stylesheet(crate::BUNDLED_ADWAITA_LIGHT).color_definitions)
    }

    #[test]
    fn resolves_adwaita_named_colors() {
        let table = adwaita_table();
        assert_eq!(table.get("theme_fg_color"), Some(&Color(0xFF2E_3436)));
        assert_eq!(table.get("borders"), Some(&Color(0xFFCD_C7C2)));
        assert_eq!(table.get("accent_color"), Some(&Color(0xFF35_84E4)));
        assert_eq!(table.get("theme_text_color"), Some(&Color(0xFF00_0000)));
        // rgba(255, 255, 255, 0.8) -> alpha 204 (0.8 * 255, rounded).
        assert_eq!(table.get("wm_highlight"), Some(&Color(0xCCFF_FFFF)));
    }

    #[test]
    fn relative_color_syntax_is_recorded_as_unresolved_not_fabricated() {
        let table = adwaita_table();
        // `@define-color wm_shadow rgb(from black r g b / calc(alpha * 0.35))`
        // is CSS Color 5 relative syntax, out of scope for M1.
        assert!(!table.contains_key("wm_shadow"));
        // 37 declared, 8 of them relative-color syntax.
        assert_eq!(table.len(), 29);
    }

    #[test]
    fn at_name_references_resolve_through_the_table() {
        let table = build_color_table(&[
            ("borders".to_string(), "#cdc7c2".to_string()),
            ("edge".to_string(), "@borders".to_string()),
        ]);
        assert_eq!(table.get("edge"), Some(&Color(0xFFCD_C7C2)));
        assert_eq!(parse_color_value("@edge", &table), Some(Color(0xFFCD_C7C2)));
        assert_eq!(parse_color_value("@nope", &table), None);
    }

    #[test]
    fn literal_forms_delegate_to_skias_css_parser() {
        let table = ColorTable::new();
        assert_eq!(
            parse_color_value("#2e3436", &table),
            Some(Color(0xFF2E_3436))
        );
        assert_eq!(parse_color_value("white", &table), Some(Color(0xFFFF_FFFF)));
        assert_eq!(
            parse_color_value("rgb(53, 132, 228)", &table),
            Some(Color(0xFF35_84E4))
        );
        assert_eq!(
            parse_color_value("transparent", &table),
            Some(Color(0x0000_0000))
        );
        assert_eq!(
            parse_color_value("linear-gradient(to top, red, blue)", &table),
            None
        );
    }

    #[test]
    fn short_hex_with_non_ascii_bytes_does_not_panic() {
        // E8: `Color::from_css("#a\u{e9}")` panicked -- "byte index 2 is not a
        // char boundary" -- so one odd byte in a theme took the process down.
        let table = ColorTable::new();
        for input in [
            "#a\u{e9}",
            "#\u{e9}\u{e9}\u{e9}",
            "#\u{1f600}\u{1f600}\u{1f600}",
            "#",
            "#1",
            "#12",
            "#12345",
            "#1234567",
            "#123456789",
            "#zzz",
            "#zzzzzz",
            "rgb(",
            "rgb()",
            "rgb(1,2)",
            "rgb(1,2,3,4,5)",
            "hsl(\u{e9})",
            "\u{e9}",
            "@",
            "@\u{e9}",
            "",
            "   ",
            "()",
            "image(#a\u{e9})",
        ] {
            let _ = parse_color_value(input, &table);
        }
    }

    #[test]
    fn colour_syntax_is_ascii_case_insensitive() {
        let table = build_color_table(&[("borders".to_string(), "#CDC7C2".to_string())]);
        assert_eq!(
            parse_color_value("#2E3436", &table),
            Some(Color(0xFF2E_3436))
        );
        assert_eq!(parse_color_value("WHITE", &table), Some(Color(0xFFFF_FFFF)));
        assert_eq!(
            parse_color_value("RGB(53, 132, 228)", &table),
            Some(Color(0xFF35_84E4))
        );
        assert_eq!(
            parse_color_value("RGBA(255, 255, 255, 0.8)", &table),
            Some(Color(0xCCFF_FFFF))
        );
        assert_eq!(
            parse_color_value("TRANSPARENT", &table),
            Some(Color(0x0000_0000))
        );
        assert_eq!(
            parse_color_value("@borders", &table),
            Some(Color(0xFFCD_C7C2))
        );
    }

    #[test]
    fn css_color_4_space_separated_syntax_parses() {
        // E7: `rgb(53 132 228)` -- the syntax GTK 4.14+ themes emit --
        // silently dropped, because the old parser split on commas only.
        let table = ColorTable::new();
        assert_eq!(
            parse_color_value("rgb(53 132 228)", &table),
            Some(Color(0xFF35_84E4))
        );
        assert_eq!(
            parse_color_value("rgb(53 132 228 / 0.5)", &table),
            Some(Color(0x8035_84E4))
        );
        assert_eq!(
            parse_color_value("rgba(53 132 228 / 50%)", &table),
            Some(Color(0x8035_84E4))
        );
        assert_eq!(
            parse_color_value("rgb(0% 100% 0%)", &table),
            Some(Color(0xFF00_FF00))
        );
        assert_eq!(
            parse_color_value("hsl(120 100% 50%)", &table),
            Some(Color(0xFF00_FF00))
        );
        assert_eq!(
            parse_color_value("hsl(120, 100%, 50%)", &table),
            Some(Color(0xFF00_FF00))
        );
        assert_eq!(
            parse_color_value("hsla(120 100% 50% / 0.5)", &table),
            Some(Color(0x8000_FF00))
        );
        // Out-of-range channels clamp rather than wrapping or failing.
        assert_eq!(
            parse_color_value("rgb(300 -20 0)", &table),
            Some(Color(0xFFFF_0000))
        );
    }

    #[test]
    fn four_and_eight_digit_hex_carry_alpha() {
        let table = ColorTable::new();
        assert_eq!(parse_color_value("#f00f", &table), Some(Color(0xFFFF_0000)));
        assert_eq!(
            parse_color_value("#FF000080", &table),
            Some(Color(0x80FF_0000))
        );
    }

    #[test]
    fn current_color_is_a_marker_not_a_colour() {
        let table = ColorTable::new();
        assert_eq!(
            parse_color_ref("currentColor", &table),
            Some(ColorRef::CurrentColor)
        );
        assert_eq!(
            parse_color_ref("CURRENTCOLOR", &table),
            Some(ColorRef::CurrentColor)
        );
        // The plain accessor cannot invent one, so it reports no value.
        assert_eq!(parse_color_value("currentColor", &table), None);
        assert_eq!(
            parse_color_ref("#2e3436", &table),
            Some(ColorRef::Absolute(Color(0xFF2E_3436)))
        );
    }

    #[test]
    fn trailing_junk_is_rejected() {
        let table = ColorTable::new();
        assert_eq!(parse_color_value("#2e3436 red", &table), None);
        assert_eq!(parse_color_value("red blue", &table), None);
    }
}
