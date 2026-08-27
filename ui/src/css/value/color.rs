//! Colours: the grammar, the lazily-resolved value, and resolution.
//!
//! Nothing resolves at parse time. `@name` survives as
//! [`ColorValue::Named`], `currentColor` as [`ColorValue::CurrentColor`],
//! and `color-mix()`/relative/legacy expressions survive as trees, so one
//! parsed sheet can be resolved against several colour tables and several
//! `color` values.

use std::collections::HashMap;
use std::rc::Rc;

use cssparser::{Parser, ParserInput, Token};
use skia_rs_safe::core::{Color, hsl_to_rgb, rgb_to_hsl};

use super::calc::{CalcNode, parse_angle, parse_math_function};

/// An unpremultiplied sRGB colour with components in `0..=1`.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Rgba {
    /// Red.
    pub r: f32,
    /// Green.
    pub g: f32,
    /// Blue.
    pub b: f32,
    /// Alpha.
    pub a: f32,
}

impl Rgba {
    /// Fully transparent black.
    pub const TRANSPARENT: Rgba = Rgba {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 0.0,
    };

    /// Convert to skia's packed `0xAARRGGBB`, rounding each channel to the
    /// nearest byte. Round-to-nearest is load-bearing: the offscreen pixel
    /// gate compares bytes against the theme's declared hex.
    #[must_use]
    pub fn to_color32(self) -> Color {
        fn byte(value: f32) -> u8 {
            if value.is_nan() {
                return 0;
            }
            (value.clamp(0.0, 1.0) * 255.0).round() as u8
        }
        Color::from_argb(byte(self.a), byte(self.r), byte(self.g), byte(self.b))
    }

    /// Convert from skia's packed `0xAARRGGBB`.
    #[must_use]
    pub fn from_color32(color: Color) -> Rgba {
        Rgba {
            r: f32::from(color.red()) / 255.0,
            g: f32::from(color.green()) / 255.0,
            b: f32::from(color.blue()) / 255.0,
            a: f32::from(color.alpha()) / 255.0,
        }
    }

    /// Clamp every channel into `0..=1`, mapping NaN to `0`.
    #[must_use]
    pub fn clamped(self) -> Rgba {
        fn unit(value: f32) -> f32 {
            if value.is_nan() {
                0.0
            } else {
                value.clamp(0.0, 1.0)
            }
        }
        Rgba {
            r: unit(self.r),
            g: unit(self.g),
            b: unit(self.b),
            a: unit(self.a),
        }
    }

    /// Premultiplied `[r, g, b, a]` -- the space GTK interpolates colours in.
    #[must_use]
    pub fn premultiplied(self) -> [f32; 4] {
        [self.r * self.a, self.g * self.a, self.b * self.a, self.a]
    }

    /// Inverse of [`Rgba::premultiplied`].
    #[must_use]
    pub fn from_premultiplied(components: [f32; 4]) -> Rgba {
        let a = components[3];
        if a <= 0.0 {
            return Rgba::TRANSPARENT;
        }
        Rgba {
            r: components[0] / a,
            g: components[1] / a,
            b: components[2] / a,
            a,
        }
    }
}

/// The colour spaces `color-mix()` and relative syntax name.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ColorSpace {
    /// Gamma-encoded sRGB.
    Srgb,
    /// Linear-light sRGB.
    SrgbLinear,
    /// HSL.
    Hsl,
    /// HWB.
    Hwb,
    /// CIE Lab.
    Lab,
    /// CIE LCh.
    Lch,
    /// Oklab.
    Oklab,
    /// Oklch.
    Oklch,
}

impl ColorSpace {
    /// ASCII-case-insensitive lookup of a space name.
    #[must_use]
    pub fn from_str_ascii_ci(name: &str) -> Option<ColorSpace> {
        const SPACES: &[(&str, ColorSpace)] = &[
            ("srgb", ColorSpace::Srgb),
            ("srgb-linear", ColorSpace::SrgbLinear),
            ("hsl", ColorSpace::Hsl),
            ("hwb", ColorSpace::Hwb),
            ("lab", ColorSpace::Lab),
            ("lch", ColorSpace::Lch),
            ("oklab", ColorSpace::Oklab),
            ("oklch", ColorSpace::Oklch),
        ];
        SPACES
            .iter()
            .find(|(spelling, _)| name.eq_ignore_ascii_case(spelling))
            .map(|(_, space)| *space)
    }
}

/// An unresolved colour.
#[derive(Clone, Debug, PartialEq)]
pub enum ColorValue {
    /// A literal colour.
    Absolute(Rgba),
    /// `currentColor` -- the element's own computed `color`.
    CurrentColor,
    /// `@name` -- resolved lazily against the [`ColorTable`].
    Named(Rc<str>),
    /// `color-mix(in <space>, a <wa>?, b <wb>?)`.
    Mix {
        /// Interpolation space.
        space: ColorSpace,
        /// First colour.
        a: Rc<ColorValue>,
        /// First weight, as a fraction.
        wa: Option<f32>,
        /// Second colour.
        b: Rc<ColorValue>,
        /// Second weight, as a fraction.
        wb: Option<f32>,
    },
    /// `rgb(from <origin> ...)` / `hsl(from <origin> ...)`.
    Relative {
        /// The function's space.
        space: ColorSpace,
        /// The origin colour.
        origin: Rc<ColorValue>,
        /// The three channel expressions.
        channels: [ChannelExpr; 3],
        /// The alpha expression; `None` keeps the origin's alpha.
        alpha: Option<ChannelExpr>,
    },
    /// GTK's deprecated colour expressions.
    Legacy(Rc<LegacyColorFn>),
}

/// One channel of a relative colour.
#[derive(Clone, Debug, PartialEq)]
pub enum ChannelExpr {
    /// A bare channel name: keep the origin's value for that channel.
    Keep(u8),
    /// A literal number, in the space's own units.
    Number(f32),
    /// A literal percentage, as a fraction.
    Percent(f32),
    /// A math expression over the channel variables.
    Calc(Rc<CalcNode>),
}

impl ChannelExpr {
    fn resolve(&self, vars: &[f32; 4], space: ColorSpace, index: usize) -> Option<f32> {
        match self {
            ChannelExpr::Keep(channel) => vars.get(usize::from(*channel)).copied(),
            ChannelExpr::Number(value) => Some(match (space, index) {
                // The alpha channel is always a bare 0..1 fraction, in any
                // space -- it is never sRGB-byte- or degree-scaled.
                (_, 3) => *value,
                // In an HSL-space relative colour, hue is degrees and
                // saturation/lightness are already fractions.
                (ColorSpace::Hsl, _) => *value,
                // Everywhere else a bare number is an 0..255 sRGB channel.
                _ => value / 255.0,
            }),
            ChannelExpr::Percent(fraction) => Some(match (space, index) {
                (ColorSpace::Hsl, 0) => fraction * 360.0,
                _ => *fraction,
            }),
            ChannelExpr::Calc(node) => node.resolve_number_with(vars),
        }
    }
}

/// GTK's deprecated colour expressions, still used by real themes.
#[derive(Clone, Debug, PartialEq)]
pub enum LegacyColorFn {
    /// `alpha(c, k)` -- multiplies alpha by `k`.
    Alpha(ColorValue, f32),
    /// `shade(c, k)` -- scales HSL saturation and lightness by `k`
    /// (`0` == black, `2` == white), matching GTK's `_gtk_hsla_shade`.
    Shade(ColorValue, f32),
    /// `mix(a, b, k)` -- interpolates, `0` == `a`, `1` == `b`.
    Mix(ColorValue, ColorValue, f32),
    /// `lighter(c)` == `shade(c, 1.3)`.
    Lighter(ColorValue),
    /// `darker(c)` == `shade(c, 0.7)`.
    Darker(ColorValue),
}

/// `@define-color` definitions, **unresolved**.
pub type ColorTable = HashMap<String, ColorValue>;

/// Everything colour resolution needs.
#[derive(Copy, Clone, Debug)]
pub struct ColorCtx<'a> {
    /// The `@define-color` table.
    pub table: &'a ColorTable,
    /// The element's own computed `color`, for `currentColor`.
    pub current: Rgba,
    /// Recursion depth; the cycle guard.
    pub depth: u8,
}

impl ColorCtx<'_> {
    /// Hard cap on `@name` / nested-expression recursion.
    pub const MAX_DEPTH: u8 = 32;

    fn deeper(&self) -> Option<ColorCtx<'_>> {
        (self.depth < ColorCtx::MAX_DEPTH).then(|| ColorCtx {
            table: self.table,
            current: self.current,
            depth: self.depth + 1,
        })
    }
}

impl ColorValue {
    /// Resolve to a concrete colour. `None` == invalid at computed-value
    /// time: an unknown `@name`, a definition cycle, or a non-finite channel.
    #[must_use]
    pub fn resolve(&self, ctx: &ColorCtx<'_>) -> Option<Rgba> {
        match self {
            ColorValue::Absolute(rgba) => Some(*rgba),
            ColorValue::CurrentColor => Some(ctx.current),
            ColorValue::Named(name) => {
                let deeper = ctx.deeper()?;
                ctx.table.get(name.as_ref())?.resolve(&deeper)
            }
            ColorValue::Mix {
                space,
                a,
                wa,
                b,
                wb,
            } => {
                let deeper = ctx.deeper()?;
                let a_rgba = a.resolve(&deeper)?;
                let b_rgba = b.resolve(&deeper)?;
                let (pa, pb) = normalize_weights(*wa, *wb)?;
                Some(mix_in_space(*space, a_rgba, b_rgba, pb / (pa + pb)))
            }
            ColorValue::Relative {
                space,
                origin,
                channels,
                alpha,
            } => {
                let deeper = ctx.deeper()?;
                let origin = origin.resolve(&deeper)?.clamped();
                let vars = match space {
                    ColorSpace::Hsl => {
                        let (h, s, l) = rgb_to_hsl(origin.r, origin.g, origin.b);
                        [h, s, l, origin.a]
                    }
                    _ => [origin.r, origin.g, origin.b, origin.a],
                };
                let mut out = [0.0_f32; 3];
                for (index, expr) in channels.iter().enumerate() {
                    out[index] = expr.resolve(&vars, *space, index)?;
                }
                let a = match alpha {
                    Some(expr) => expr.resolve(&vars, *space, 3)?,
                    None => vars[3],
                };
                let rgba = match space {
                    ColorSpace::Hsl => {
                        let (r, g, b) = hsl_to_rgb(
                            out[0].rem_euclid(360.0),
                            out[1].clamp(0.0, 1.0),
                            out[2].clamp(0.0, 1.0),
                        );
                        Rgba { r, g, b, a }
                    }
                    _ => Rgba {
                        r: out[0],
                        g: out[1],
                        b: out[2],
                        a,
                    },
                };
                finite(rgba).map(Rgba::clamped)
            }
            ColorValue::Legacy(function) => {
                let deeper = ctx.deeper()?;
                match function.as_ref() {
                    LegacyColorFn::Alpha(color, factor) => {
                        let base = color.resolve(&deeper)?;
                        finite(Rgba {
                            a: base.a * factor,
                            ..base
                        })
                        .map(Rgba::clamped)
                    }
                    LegacyColorFn::Shade(color, factor) => shade(color.resolve(&deeper)?, *factor),
                    LegacyColorFn::Lighter(color) => shade(color.resolve(&deeper)?, 1.3),
                    LegacyColorFn::Darker(color) => shade(color.resolve(&deeper)?, 0.7),
                    LegacyColorFn::Mix(a, b, factor) => {
                        let a = a.resolve(&deeper)?;
                        let b = b.resolve(&deeper)?;
                        finite(mix_in_space(ColorSpace::Srgb, a, b, *factor)).map(Rgba::clamped)
                    }
                }
            }
        }
    }

    /// Parse a whole `<color>`.
    pub fn parse(input: &mut Parser<'_, '_>) -> Result<ColorValue, ()> {
        let token = input.next().map_err(|_| ())?.clone();
        match token {
            Token::Hash(ref digits) | Token::IDHash(ref digits) => {
                parse_hex(digits).map(ColorValue::Absolute).ok_or(())
            }
            Token::AtKeyword(ref name) => Ok(ColorValue::Named(Rc::from(name.as_ref()))),
            Token::Ident(ref name) => {
                if name.eq_ignore_ascii_case("currentcolor") {
                    Ok(ColorValue::CurrentColor)
                } else if name.eq_ignore_ascii_case("transparent") {
                    Ok(ColorValue::Absolute(Rgba::TRANSPARENT))
                } else {
                    named_color(name).map(ColorValue::Absolute).ok_or(())
                }
            }
            Token::Function(ref name) => {
                let name = name.clone();
                parse_color_function(&name, input)
            }
            _ => Err(()),
        }
    }
}

fn finite(rgba: Rgba) -> Option<Rgba> {
    (rgba.r.is_finite() && rgba.g.is_finite() && rgba.b.is_finite() && rgba.a.is_finite())
        .then_some(rgba)
}

/// GTK's `_gtk_hsla_shade`: scale HSL saturation *and* lightness by `factor`.
fn shade(base: Rgba, factor: f32) -> Option<Rgba> {
    if !factor.is_finite() {
        return None;
    }
    let base = base.clamped();
    let (h, s, l) = rgb_to_hsl(base.r, base.g, base.b);
    let (r, g, b) = hsl_to_rgb(
        h,
        (s * factor).clamp(0.0, 1.0),
        (l * factor).clamp(0.0, 1.0),
    );
    finite(Rgba { r, g, b, a: base.a }).map(Rgba::clamped)
}

/// `t == 0` is `a`, `t == 1` is `b`. Premultiplied, as GTK interpolates.
fn mix_in_space(space: ColorSpace, a: Rgba, b: Rgba, t: f32) -> Rgba {
    if space == ColorSpace::Hsl {
        let (ha, sa, la) = rgb_to_hsl(a.r, a.g, a.b);
        let (hb, sb, lb) = rgb_to_hsl(b.r, b.g, b.b);
        // Shorter hue arc, per CSS Color 4's default `shorter hue`.
        let mut delta = hb - ha;
        if delta > 180.0 {
            delta -= 360.0;
        } else if delta < -180.0 {
            delta += 360.0;
        }
        let (r, g, blue) = hsl_to_rgb(
            (ha + delta * t).rem_euclid(360.0),
            sa + (sb - sa) * t,
            la + (lb - la) * t,
        );
        return Rgba {
            r,
            g,
            b: blue,
            a: a.a + (b.a - a.a) * t,
        }
        .clamped();
    }
    // Every non-HSL space (including the Lab/Oklab family, which this
    // engine does not model channel-wise) mixes in premultiplied sRGB.
    let pa = a.premultiplied();
    let pb = b.premultiplied();
    let mut out = [0.0_f32; 4];
    for (index, slot) in out.iter_mut().enumerate() {
        *slot = pa[index] + (pb[index] - pa[index]) * t;
    }
    Rgba::from_premultiplied(out).clamped()
}

/// CSS `color-mix()` weight normalization: both omitted == 50/50; one given
/// == the other is its complement; both given and summing to 0 is invalid.
fn normalize_weights(wa: Option<f32>, wb: Option<f32>) -> Option<(f32, f32)> {
    let (a, b) = match (wa, wb) {
        (None, None) => (0.5, 0.5),
        (Some(a), None) => (a, 1.0 - a),
        (None, Some(b)) => (1.0 - b, b),
        (Some(a), Some(b)) => (a, b),
    };
    if !a.is_finite() || !b.is_finite() || a < 0.0 || b < 0.0 || a + b <= 0.0 {
        return None;
    }
    Some((a, b))
}

fn parse_hex(digits: &str) -> Option<Rgba> {
    if !digits.is_ascii() || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    fn nibble(byte: u8) -> f32 {
        f32::from(char::from(byte).to_digit(16).unwrap_or(0) as u8)
    }
    let bytes = digits.as_bytes();
    let channel = |high: u8, low: u8| (nibble(high) * 16.0 + nibble(low)) / 255.0;
    let short = |value: u8| (nibble(value) * 17.0) / 255.0;
    match bytes.len() {
        3 => Some(Rgba {
            r: short(bytes[0]),
            g: short(bytes[1]),
            b: short(bytes[2]),
            a: 1.0,
        }),
        4 => Some(Rgba {
            r: short(bytes[0]),
            g: short(bytes[1]),
            b: short(bytes[2]),
            a: short(bytes[3]),
        }),
        6 => Some(Rgba {
            r: channel(bytes[0], bytes[1]),
            g: channel(bytes[2], bytes[3]),
            b: channel(bytes[4], bytes[5]),
            a: 1.0,
        }),
        8 => Some(Rgba {
            r: channel(bytes[0], bytes[1]),
            g: channel(bytes[2], bytes[3]),
            b: channel(bytes[4], bytes[5]),
            a: channel(bytes[6], bytes[7]),
        }),
        _ => None,
    }
}

/// CSS Color 3 named colours, via skia's own table.
fn named_color(name: &str) -> Option<Rgba> {
    if !name.is_ascii() {
        return None;
    }
    Color::from_css(&name.to_ascii_lowercase()).map(Rgba::from_color32)
}

/// One channel component of a colour function.
#[derive(Copy, Clone, Debug)]
enum Channel {
    Number(f32),
    Percent(f32),
}

fn parse_channel(input: &mut Parser<'_, '_>) -> Result<Channel, ()> {
    let state = input.state();
    let token = match input.next() {
        Ok(token) => token.clone(),
        Err(_) => {
            input.reset(&state);
            return Err(());
        }
    };
    match token {
        Token::Number { value, .. } if value.is_finite() => Ok(Channel::Number(value)),
        Token::Percentage { unit_value, .. } if unit_value.is_finite() => {
            Ok(Channel::Percent(unit_value))
        }
        Token::Function(ref name) => {
            let name = name.clone();
            let number = parse_math_function(&name, input)?
                .resolve_number()
                .ok_or(())?;
            Ok(Channel::Number(number))
        }
        _ => {
            input.reset(&state);
            Err(())
        }
    }
}

/// Consume an optional comma -- colour functions accept both the CSS 3
/// comma syntax and the CSS 4 space syntax.
fn eat_comma(input: &mut Parser<'_, '_>) {
    let state = input.state();
    let is_comma = matches!(input.next(), Ok(Token::Comma));
    if !is_comma {
        input.reset(&state);
    }
}

/// Consume `, <alpha>` or `/ <alpha>` if present.
fn parse_optional_alpha(input: &mut Parser<'_, '_>) -> Result<Option<Channel>, ()> {
    let state = input.state();
    let token = match input.next() {
        Ok(token) => token.clone(),
        Err(_) => {
            input.reset(&state);
            return Ok(None);
        }
    };
    match token {
        Token::Comma | Token::Delim('/') => parse_channel(input).map(Some),
        _ => {
            input.reset(&state);
            Ok(None)
        }
    }
}

fn rgb_channel(channel: Channel) -> f32 {
    match channel {
        Channel::Number(value) => value / 255.0,
        Channel::Percent(fraction) => fraction,
    }
}

fn unit_channel(channel: Channel) -> f32 {
    match channel {
        Channel::Number(value) => value,
        Channel::Percent(fraction) => fraction,
    }
}

/// Dispatch a colour function whose name token has already been consumed.
///
/// `color-mix()`, the relative `from` forms and GTK's legacy expressions
/// are added in Task 5; the Level 4 spaces route through skia's parser
/// because this engine models only sRGB and HSL channel-wise.
fn parse_color_function(name: &str, input: &mut Parser<'_, '_>) -> Result<ColorValue, ()> {
    if name.eq_ignore_ascii_case("rgb") || name.eq_ignore_ascii_case("rgba") {
        nested(input, parse_rgb_body)
    } else if name.eq_ignore_ascii_case("hsl") || name.eq_ignore_ascii_case("hsla") {
        nested(input, parse_hsl_body)
    } else if name.eq_ignore_ascii_case("hwb") {
        nested(input, parse_hwb_body)
    } else if name.eq_ignore_ascii_case("lab")
        || name.eq_ignore_ascii_case("lch")
        || name.eq_ignore_ascii_case("oklab")
        || name.eq_ignore_ascii_case("oklch")
        || name.eq_ignore_ascii_case("color")
    {
        let arguments = input
            .parse_nested_block(|inner| {
                Ok::<String, cssparser::ParseError<'_, ()>>(
                    crate::css::tokens::serialize_remaining(inner),
                )
            })
            .map_err(|_| ())?;
        let text = format!("{}({})", name.to_ascii_lowercase(), arguments);
        Color::from_css(&text)
            .map(|color| ColorValue::Absolute(Rgba::from_color32(color)))
            .ok_or(())
    } else if name.eq_ignore_ascii_case("color-mix") {
        nested(input, parse_color_mix_body)
    } else if name.eq_ignore_ascii_case("alpha") {
        nested(input, |inner| {
            let color = ColorValue::parse(inner)?;
            eat_comma(inner);
            let factor = parse_number(inner)?;
            require_exhausted(inner)?;
            Ok(ColorValue::Legacy(Rc::new(LegacyColorFn::Alpha(
                color, factor,
            ))))
        })
    } else if name.eq_ignore_ascii_case("shade") {
        nested(input, |inner| {
            let color = ColorValue::parse(inner)?;
            eat_comma(inner);
            let factor = parse_number(inner)?;
            require_exhausted(inner)?;
            Ok(ColorValue::Legacy(Rc::new(LegacyColorFn::Shade(
                color, factor,
            ))))
        })
    } else if name.eq_ignore_ascii_case("mix") {
        nested(input, |inner| {
            let a = ColorValue::parse(inner)?;
            eat_comma(inner);
            let b = ColorValue::parse(inner)?;
            eat_comma(inner);
            let factor = parse_number(inner)?;
            require_exhausted(inner)?;
            Ok(ColorValue::Legacy(Rc::new(LegacyColorFn::Mix(
                a, b, factor,
            ))))
        })
    } else if name.eq_ignore_ascii_case("lighter") {
        nested(input, |inner| {
            let color = ColorValue::parse(inner)?;
            require_exhausted(inner)?;
            Ok(ColorValue::Legacy(Rc::new(LegacyColorFn::Lighter(color))))
        })
    } else if name.eq_ignore_ascii_case("darker") {
        nested(input, |inner| {
            let color = ColorValue::parse(inner)?;
            require_exhausted(inner)?;
            Ok(ColorValue::Legacy(Rc::new(LegacyColorFn::Darker(color))))
        })
    } else {
        Err(())
    }
}

/// Run `body` inside a function's block, translating its `()` error.
fn nested<T>(
    input: &mut Parser<'_, '_>,
    body: fn(&mut Parser<'_, '_>) -> Result<T, ()>,
) -> Result<T, ()> {
    input
        .parse_nested_block(|inner| match body(inner) {
            Ok(value) => Ok(value),
            Err(()) => Err(inner.new_custom_error::<(), ()>(())),
        })
        .map_err(|_: cssparser::ParseError<'_, ()>| ())
}

fn require_exhausted(input: &mut Parser<'_, '_>) -> Result<(), ()> {
    input.skip_whitespace();
    if input.is_exhausted() {
        Ok(())
    } else {
        Err(())
    }
}

fn parse_rgb_body(input: &mut Parser<'_, '_>) -> Result<ColorValue, ()> {
    if let Some(relative) = try_parse_relative(input, ColorSpace::Srgb)? {
        return Ok(relative);
    }
    let r = parse_channel(input)?;
    eat_comma(input);
    let g = parse_channel(input)?;
    eat_comma(input);
    let b = parse_channel(input)?;
    let alpha = parse_optional_alpha(input)?;
    require_exhausted(input)?;
    Ok(ColorValue::Absolute(
        Rgba {
            r: rgb_channel(r),
            g: rgb_channel(g),
            b: rgb_channel(b),
            a: alpha.map_or(1.0, unit_channel),
        }
        .clamped(),
    ))
}

fn parse_hsl_body(input: &mut Parser<'_, '_>) -> Result<ColorValue, ()> {
    if let Some(relative) = try_parse_relative(input, ColorSpace::Hsl)? {
        return Ok(relative);
    }
    let hue = parse_angle_or_number(input)?;
    eat_comma(input);
    let saturation = parse_channel(input)?;
    eat_comma(input);
    let lightness = parse_channel(input)?;
    let alpha = parse_optional_alpha(input)?;
    require_exhausted(input)?;
    let (r, g, b) = hsl_to_rgb(
        hue.rem_euclid(360.0),
        unit_channel(saturation).clamp(0.0, 1.0),
        unit_channel(lightness).clamp(0.0, 1.0),
    );
    Ok(ColorValue::Absolute(
        Rgba {
            r,
            g,
            b,
            a: alpha.map_or(1.0, unit_channel),
        }
        .clamped(),
    ))
}

fn parse_hwb_body(input: &mut Parser<'_, '_>) -> Result<ColorValue, ()> {
    let hue = parse_angle_or_number(input)?;
    eat_comma(input);
    let whiteness = unit_channel(parse_channel(input)?).clamp(0.0, 1.0);
    eat_comma(input);
    let blackness = unit_channel(parse_channel(input)?).clamp(0.0, 1.0);
    let alpha = parse_optional_alpha(input)?;
    require_exhausted(input)?;
    let a = alpha.map_or(1.0, unit_channel);
    if whiteness + blackness >= 1.0 {
        let grey = whiteness / (whiteness + blackness);
        return Ok(ColorValue::Absolute(
            Rgba {
                r: grey,
                g: grey,
                b: grey,
                a,
            }
            .clamped(),
        ));
    }
    let (r, g, b) = hsl_to_rgb(hue.rem_euclid(360.0), 1.0, 0.5);
    let apply = |channel: f32| channel * (1.0 - whiteness - blackness) + whiteness;
    Ok(ColorValue::Absolute(
        Rgba {
            r: apply(r),
            g: apply(g),
            b: apply(b),
            a,
        }
        .clamped(),
    ))
}

/// A hue: any angle unit, or a bare number in degrees.
fn parse_angle_or_number(input: &mut Parser<'_, '_>) -> Result<f32, ()> {
    let state = input.state();
    if let Ok(angle) = parse_angle(input) {
        return Ok(angle);
    }
    input.reset(&state);
    match input.next().map_err(|_| ())?.clone() {
        Token::Number { value, .. } if value.is_finite() => Ok(value),
        _ => Err(()),
    }
}

/// `<number>` / `<percentage>` / math function, as a bare number.
fn parse_number(input: &mut Parser<'_, '_>) -> Result<f32, ()> {
    let token = input.next().map_err(|_| ())?.clone();
    match token {
        Token::Number { value, .. } if value.is_finite() => Ok(value),
        Token::Percentage { unit_value, .. } if unit_value.is_finite() => Ok(unit_value),
        Token::Function(ref name) => {
            let name = name.clone();
            parse_math_function(&name, input)?
                .resolve_number()
                .ok_or(())
        }
        _ => Err(()),
    }
}

/// The channel names each space binds. Only sRGB and HSL are modelled
/// channel-wise; a relative colour in any other space is rejected at parse
/// time rather than silently mis-bound.
fn channel_index(name: &str, space: ColorSpace) -> Option<u8> {
    let names: [&str; 3] = match space {
        ColorSpace::Srgb | ColorSpace::SrgbLinear => ["r", "g", "b"],
        ColorSpace::Hsl => ["h", "s", "l"],
        _ => return None,
    };
    if name.eq_ignore_ascii_case("alpha") {
        return Some(3);
    }
    names
        .iter()
        .position(|channel| name.eq_ignore_ascii_case(channel))
        .and_then(|index| u8::try_from(index).ok())
}

fn parse_channel_expr(
    input: &mut Parser<'_, '_>,
    space: ColorSpace,
    index: usize,
) -> Result<ChannelExpr, ()> {
    let state = input.state();
    let token = input.next().map_err(|_| ())?.clone();
    match token {
        Token::Number { value, .. } if value.is_finite() => Ok(ChannelExpr::Number(value)),
        Token::Percentage { unit_value, .. } if unit_value.is_finite() => {
            Ok(ChannelExpr::Percent(unit_value))
        }
        Token::Dimension { .. } if space == ColorSpace::Hsl && index == 0 => {
            input.reset(&state);
            parse_angle(input).map(ChannelExpr::Number)
        }
        Token::Ident(ref name) => channel_index(name, space).map(ChannelExpr::Keep).ok_or(()),
        Token::Function(ref name) => {
            let name = name.clone();
            Ok(ChannelExpr::Calc(Rc::new(parse_math_function(
                &name, input,
            )?)))
        }
        _ => {
            input.reset(&state);
            Err(())
        }
    }
}

/// `from <origin> c0 c1 c2 [/ alpha]` inside `rgb()`/`hsl()`.
/// `Ok(None)` means "this is not a relative colour"; the caller falls
/// through to the ordinary grammar with the parser rewound.
fn try_parse_relative(
    input: &mut Parser<'_, '_>,
    space: ColorSpace,
) -> Result<Option<ColorValue>, ()> {
    let state = input.state();
    let is_from =
        matches!(input.next(), Ok(Token::Ident(name)) if name.eq_ignore_ascii_case("from"));
    if !is_from {
        input.reset(&state);
        return Ok(None);
    }
    let origin = ColorValue::parse(input)?;
    let channels = [
        parse_channel_expr(input, space, 0)?,
        parse_channel_expr(input, space, 1)?,
        parse_channel_expr(input, space, 2)?,
    ];
    let alpha_state = input.state();
    let has_alpha = matches!(input.next(), Ok(Token::Delim('/')) | Ok(Token::Comma));
    let alpha = if has_alpha {
        Some(parse_channel_expr(input, space, 3)?)
    } else {
        input.reset(&alpha_state);
        None
    };
    require_exhausted(input)?;
    Ok(Some(ColorValue::Relative {
        space,
        origin: Rc::new(origin),
        channels,
        alpha,
    }))
}

fn parse_color_mix_body(input: &mut Parser<'_, '_>) -> Result<ColorValue, ()> {
    let in_keyword =
        matches!(input.next(), Ok(Token::Ident(name)) if name.eq_ignore_ascii_case("in"));
    if !in_keyword {
        return Err(());
    }
    let space_name = input.expect_ident().map_err(|_| ())?.as_ref().to_string();
    let space = ColorSpace::from_str_ascii_ci(&space_name).ok_or(())?;
    // Skip the optional hue-interpolation clause (`shorter hue`, ...) up to
    // the comma that ends the space specifier.
    loop {
        let state = input.state();
        let token = input.next().map_err(|_| ())?.clone();
        match token {
            Token::Comma => break,
            Token::Ident(_) => continue,
            _ => {
                input.reset(&state);
                return Err(());
            }
        }
    }
    let (a, wa) = parse_mix_operand(input)?;
    input.expect_comma().map_err(|_| ())?;
    let (b, wb) = parse_mix_operand(input)?;
    require_exhausted(input)?;
    Ok(ColorValue::Mix {
        space,
        a: Rc::new(a),
        wa,
        b: Rc::new(b),
        wb,
    })
}

/// `<color> <percentage>?` or `<percentage> <color>`.
fn parse_mix_operand(input: &mut Parser<'_, '_>) -> Result<(ColorValue, Option<f32>), ()> {
    let state = input.state();
    let leading = match input.next() {
        Ok(Token::Percentage { unit_value, .. }) if unit_value.is_finite() => Some(*unit_value),
        _ => None,
    };
    if leading.is_none() {
        input.reset(&state);
    }
    let color = ColorValue::parse(input)?;
    if leading.is_some() {
        return Ok((color, leading));
    }
    let state = input.state();
    let trailing = match input.next() {
        Ok(Token::Percentage { unit_value, .. }) if unit_value.is_finite() => Some(*unit_value),
        _ => None,
    };
    if trailing.is_none() {
        input.reset(&state);
    }
    Ok((color, trailing))
}

/// Build the `@define-color` table.
///
/// Order-preserving and **lazy**: every definition is parsed but nothing is
/// resolved, so a definition may reference a name defined later in the
/// sheet (M1 required earlier-only). Cycles are broken by
/// [`ColorCtx::MAX_DEPTH`] at resolution time. A later definition of the
/// same name replaces an earlier one, as GTK does.
#[must_use]
pub fn build_color_table(definitions: &[(String, String)]) -> ColorTable {
    let mut table = ColorTable::with_capacity(definitions.len());
    for (name, text) in definitions {
        let mut source = ParserInput::new(text);
        let mut parser = Parser::new(&mut source);
        let parsed = ColorValue::parse(&mut parser).and_then(|value| {
            parser.skip_whitespace();
            if parser.is_exhausted() {
                Ok(value)
            } else {
                Err(())
            }
        });
        match parsed {
            Ok(value) => {
                table.insert(name.clone(), value);
            }
            Err(()) => {
                tracing::debug!(%name, value = %text, "unparseable @define-color; skipping");
            }
        }
    }
    table
}

#[cfg(test)]
mod tests {
    use super::{ColorCtx, ColorTable, ColorValue, Rgba};
    use crate::css::value::parse_entirely_with;
    use skia_rs_safe::core::Color;

    fn table_with(name: &str, value: &str) -> ColorTable {
        let mut table = ColorTable::new();
        table.insert(
            name.to_string(),
            parse_entirely_with(value, ColorValue::parse).expect("fixture colour parses"),
        );
        table
    }

    /// The M1 `parse_color_value` shape: a whole value, resolved against a
    /// table, with `currentColor` reported as "no value".
    fn color_value(text: &str, table: &ColorTable) -> Option<Color> {
        let value = parse_entirely_with(text, ColorValue::parse).ok()?;
        if value == ColorValue::CurrentColor {
            return None;
        }
        let ctx = ColorCtx {
            table,
            current: Rgba::TRANSPARENT,
            depth: 0,
        };
        value.resolve(&ctx).map(Rgba::to_color32)
    }

    /// The M1 `parse_color_ref` shape: the unresolved value.
    fn color_ref(text: &str, _table: &ColorTable) -> Option<ColorValue> {
        parse_entirely_with(text, ColorValue::parse).ok()
    }

    #[test]
    fn literal_forms_delegate_to_skias_css_parser() {
        let table = ColorTable::new();
        assert_eq!(color_value("#2e3436", &table), Some(Color(0xFF2E_3436)));
        assert_eq!(color_value("white", &table), Some(Color(0xFFFF_FFFF)));
        assert_eq!(
            color_value("rgb(53, 132, 228)", &table),
            Some(Color(0xFF35_84E4))
        );
        assert_eq!(color_value("transparent", &table), Some(Color(0x0000_0000)));
        assert_eq!(
            color_value("linear-gradient(to top, red, blue)", &table),
            None
        );
        // CSS Color 4 spaces route through skia's own parser.
        assert!(color_value("oklch(0.5 0 0)", &table).is_some());
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
            let _ = color_value(input, &table);
        }
    }

    #[test]
    fn colour_syntax_is_ascii_case_insensitive() {
        let table = table_with("borders", "#CDC7C2");
        assert_eq!(color_value("#2E3436", &table), Some(Color(0xFF2E_3436)));
        assert_eq!(color_value("WHITE", &table), Some(Color(0xFFFF_FFFF)));
        assert_eq!(
            color_value("RGB(53, 132, 228)", &table),
            Some(Color(0xFF35_84E4))
        );
        assert_eq!(
            color_value("RGBA(255, 255, 255, 0.8)", &table),
            Some(Color(0xCCFF_FFFF))
        );
        assert_eq!(color_value("TRANSPARENT", &table), Some(Color(0x0000_0000)));
        assert_eq!(color_value("@borders", &table), Some(Color(0xFFCD_C7C2)));
    }

    #[test]
    fn css_color_4_space_separated_syntax_parses() {
        // E7: `rgb(53 132 228)` -- the syntax GTK 4.14+ themes emit --
        // silently dropped, because the old parser split on commas only.
        let table = ColorTable::new();
        assert_eq!(
            color_value("rgb(53 132 228)", &table),
            Some(Color(0xFF35_84E4))
        );
        assert_eq!(
            color_value("rgb(53 132 228 / 0.5)", &table),
            Some(Color(0x8035_84E4))
        );
        assert_eq!(
            color_value("rgba(53 132 228 / 50%)", &table),
            Some(Color(0x8035_84E4))
        );
        assert_eq!(
            color_value("rgb(0% 100% 0%)", &table),
            Some(Color(0xFF00_FF00))
        );
        assert_eq!(
            color_value("hsl(120 100% 50%)", &table),
            Some(Color(0xFF00_FF00))
        );
        assert_eq!(
            color_value("hsl(120, 100%, 50%)", &table),
            Some(Color(0xFF00_FF00))
        );
        assert_eq!(
            color_value("hsla(120 100% 50% / 0.5)", &table),
            Some(Color(0x8000_FF00))
        );
        // Out-of-range channels clamp rather than wrapping or failing.
        assert_eq!(
            color_value("rgb(300 -20 0)", &table),
            Some(Color(0xFFFF_0000))
        );
    }

    #[test]
    fn four_and_eight_digit_hex_carry_alpha() {
        let table = ColorTable::new();
        assert_eq!(color_value("#f00f", &table), Some(Color(0xFFFF_0000)));
        assert_eq!(color_value("#FF000080", &table), Some(Color(0x80FF_0000)));
    }

    #[test]
    fn current_color_is_a_marker_not_a_colour() {
        let table = ColorTable::new();
        assert_eq!(
            color_ref("currentColor", &table),
            Some(ColorValue::CurrentColor)
        );
        assert_eq!(
            color_ref("CURRENTCOLOR", &table),
            Some(ColorValue::CurrentColor)
        );
        // The plain accessor cannot invent one, so it reports no value.
        assert_eq!(color_value("currentColor", &table), None);
        let ctx = ColorCtx {
            table: &table,
            current: Rgba::TRANSPARENT,
            depth: 0,
        };
        assert_eq!(
            color_ref("#2e3436", &table)
                .and_then(|value| value.resolve(&ctx))
                .map(Rgba::to_color32),
            Some(Color(0xFF2E_3436))
        );
    }

    #[test]
    fn trailing_junk_is_rejected() {
        let table = ColorTable::new();
        assert_eq!(color_value("#2e3436 red", &table), None);
        assert_eq!(color_value("red blue", &table), None);
    }

    #[test]
    fn current_color_resolves_against_the_context_not_a_constant() {
        let table = ColorTable::new();
        let ctx = ColorCtx {
            table: &table,
            current: Rgba {
                r: 1.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            },
            depth: 0,
        };
        assert_eq!(
            ColorValue::CurrentColor.resolve(&ctx).map(Rgba::to_color32),
            Some(Color(0xFFFF_0000))
        );
    }

    #[test]
    fn an_unknown_name_and_a_cycle_are_both_invalid_at_computed_value_time() {
        let mut table = ColorTable::new();
        // a -> b -> a
        table.insert("a".to_string(), ColorValue::Named("b".into()));
        table.insert("b".to_string(), ColorValue::Named("a".into()));
        let ctx = ColorCtx {
            table: &table,
            current: Rgba::TRANSPARENT,
            depth: 0,
        };
        assert_eq!(ColorValue::Named("a".into()).resolve(&ctx), None);
        assert_eq!(ColorValue::Named("nope".into()).resolve(&ctx), None);
    }

    #[test]
    fn byte_conversion_rounds_to_nearest() {
        // The offscreen pixel gate compares bytes, so 0.8 * 255 must be 204,
        // not 203 (truncation).
        assert_eq!(
            Rgba {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 0.8
            }
            .to_color32(),
            Color(0xCCFF_FFFF)
        );
        assert_eq!(
            Rgba::from_color32(Color(0xFF35_84E4)).to_color32(),
            Color(0xFF35_84E4)
        );
    }

    #[test]
    fn colour_parsing_never_panics() {
        let table = ColorTable::new();
        for input in crate::css::value::FUZZ_INPUTS {
            let _ = color_value(input, &table);
        }
    }

    use super::build_color_table;
    use crate::css::parse::parse_stylesheet;

    fn adwaita_table() -> ColorTable {
        build_color_table(&parse_stylesheet(crate::BUNDLED_ADWAITA_LIGHT).color_definitions)
    }

    fn resolved(table: &ColorTable, name: &str) -> Option<Color> {
        let ctx = ColorCtx {
            table,
            current: Rgba::TRANSPARENT,
            depth: 0,
        };
        table.get(name)?.resolve(&ctx).map(Rgba::to_color32)
    }

    #[test]
    fn resolves_adwaita_named_colors() {
        let table = adwaita_table();
        assert_eq!(resolved(&table, "theme_fg_color"), Some(Color(0xFF2E_3436)));
        assert_eq!(resolved(&table, "borders"), Some(Color(0xFFCD_C7C2)));
        assert_eq!(resolved(&table, "accent_color"), Some(Color(0xFF35_84E4)));
        assert_eq!(
            resolved(&table, "theme_text_color"),
            Some(Color(0xFF00_0000))
        );
        // rgba(255, 255, 255, 0.8) -> alpha 204 (0.8 * 255, rounded).
        assert_eq!(resolved(&table, "wm_highlight"), Some(Color(0xCCFF_FFFF)));
    }

    #[test]
    fn relative_color_syntax_resolves() {
        // M1 recorded these as unresolved and pinned the table at 29 of 37.
        // Modelling CSS Color 5 relative syntax is an M2 deliverable: the
        // fingerprint moves to 37, and this test inverts.
        let table = adwaita_table();
        assert_eq!(table.len(), 37);
        // `rgb(from black r g b / calc(alpha * 0.35))`
        // -> black at 35% alpha; 0.35 * 255 rounds to 89 (0x59).
        assert_eq!(resolved(&table, "wm_shadow"), Some(Color(0x5900_0000)));
        // `rgb(from black r g b / calc(alpha * 0.18))` -> 0.18 * 255 -> 46.
        assert_eq!(resolved(&table, "wm_border"), Some(Color(0x2E00_0000)));
        // The five `hsl(from ... h calc(s * k) calc(l * k))` entries all
        // resolve to opaque colours.
        for name in [
            "wm_title",
            "wm_bg_a",
            "wm_button_hover_color_a",
            "wm_button_active_color_a",
            "wm_button_active_color_b",
            "wm_button_active_color_c",
        ] {
            let color = resolved(&table, name).unwrap_or_else(|| panic!("{name} did not resolve"));
            assert_eq!(color.alpha(), 0xFF, "{name} lost its alpha");
        }
    }

    #[test]
    fn at_name_references_resolve_through_the_table() {
        let table = build_color_table(&[
            ("borders".to_string(), "#cdc7c2".to_string()),
            ("edge".to_string(), "@borders".to_string()),
        ]);
        assert_eq!(resolved(&table, "edge"), Some(Color(0xFFCD_C7C2)));
        assert_eq!(color_value("@edge", &table), Some(Color(0xFFCD_C7C2)));
        assert_eq!(color_value("@nope", &table), None);
    }

    #[test]
    fn a_definition_may_reference_a_name_defined_later() {
        // M1's table was built incrementally, so a forward reference was
        // silently dropped. Lazy resolution makes source order irrelevant.
        let table = build_color_table(&[
            ("edge".to_string(), "@borders".to_string()),
            ("borders".to_string(), "#cdc7c2".to_string()),
        ]);
        assert_eq!(resolved(&table, "edge"), Some(Color(0xFFCD_C7C2)));
    }

    #[test]
    fn gtk_legacy_colour_functions_resolve() {
        let table = ColorTable::new();
        // alpha() multiplies alpha: 0.5 * 255 -> 128 (0x80).
        assert_eq!(
            color_value("alpha(#ff0000, 0.5)", &table),
            Some(Color(0x80FF_0000))
        );
        // shade(c, 0) drives lightness (and saturation) to zero: black.
        assert_eq!(
            color_value("shade(#ff0000, 0)", &table),
            Some(Color(0xFF00_0000))
        );
        // shade(c, 1) is the identity.
        assert_eq!(
            color_value("shade(#ff0000, 1)", &table),
            Some(Color(0xFFFF_0000))
        );
        // mix(a, b, 0) is a, mix(a, b, 1) is b, and 0.5 is the midpoint.
        assert_eq!(
            color_value("mix(#000000, #ffffff, 0)", &table),
            Some(Color(0xFF00_0000))
        );
        assert_eq!(
            color_value("mix(#000000, #ffffff, 1)", &table),
            Some(Color(0xFFFF_FFFF))
        );
        assert_eq!(
            color_value("mix(#000000, #ffffff, 0.5)", &table),
            Some(Color(0xFF80_8080))
        );
        // lighter/darker are shade(c, 1.3) / shade(c, 0.7); nesting works.
        assert!(color_value("lighter(#808080)", &table).is_some());
        assert_eq!(
            color_value("darker(#808080)", &table),
            color_value("shade(#808080, 0.7)", &table)
        );
        assert!(color_value("alpha(shade(@nope, 0.5), 0.5)", &table).is_none());
    }

    #[test]
    fn color_mix_normalizes_its_weights() {
        let table = ColorTable::new();
        assert_eq!(
            color_value("color-mix(in srgb, #000000, #ffffff)", &table),
            Some(Color(0xFF80_8080))
        );
        assert_eq!(
            color_value("color-mix(in srgb, #000000 100%, #ffffff)", &table),
            Some(Color(0xFF00_0000))
        );
        assert_eq!(
            color_value("color-mix(in srgb, #000000 0%, #ffffff)", &table),
            Some(Color(0xFFFF_FFFF))
        );
        // Both weights given and not summing to 100% still normalizes.
        assert_eq!(
            color_value("color-mix(in srgb, #000000 25%, #ffffff 25%)", &table),
            Some(Color(0xFF80_8080))
        );
        assert_eq!(
            color_value("color-mix(in srgb, #000000 0%, #ffffff 0%)", &table),
            None
        );
        assert_eq!(color_value("color-mix(#000000, #ffffff)", &table), None);
    }

    #[test]
    fn relative_syntax_survives_parse_unresolved() {
        // A relative colour over an @name must not be resolved at parse time.
        let value = parse_entirely_with("rgb(from @accent r g b / 0.5)", ColorValue::parse)
            .expect("relative syntax parses");
        assert!(matches!(value, ColorValue::Relative { .. }));
        let empty = ColorTable::new();
        let ctx = ColorCtx {
            table: &empty,
            current: Rgba::TRANSPARENT,
            depth: 0,
        };
        assert_eq!(value.resolve(&ctx), None);
        let table = table_with("accent", "#3584e4");
        let ctx = ColorCtx {
            table: &table,
            current: Rgba::TRANSPARENT,
            depth: 0,
        };
        assert_eq!(
            value.resolve(&ctx).map(Rgba::to_color32),
            Some(Color(0x8035_84E4))
        );
    }

    #[test]
    fn the_colour_table_never_panics_on_hostile_definitions() {
        let definitions: Vec<(String, String)> = crate::css::value::FUZZ_INPUTS
            .iter()
            .map(|input| ((*input).to_string(), (*input).to_string()))
            .collect();
        let table = build_color_table(&definitions);
        let ctx = ColorCtx {
            table: &table,
            current: Rgba::TRANSPARENT,
            depth: 0,
        };
        for value in table.values() {
            let _ = value.resolve(&ctx);
        }
    }
}
