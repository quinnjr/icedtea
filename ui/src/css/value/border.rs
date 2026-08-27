//! Small value types shared by the border, border-image and background
//! property families.

use cssparser::{Parser, Token};

use super::keyword::Keyword;
use super::length::Length;

/// `<number>` or `<percentage>`, as `border-image-slice` uses them.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum NumberOrPercent {
    /// A bare number.
    Number(f32),
    /// A percentage, as a fraction.
    Percent(f32),
}

/// `border-image-slice`.
#[derive(Clone, Debug, PartialEq)]
pub struct BorderImageSlice {
    /// Top, right, bottom, left.
    pub sides: [NumberOrPercent; 4],
    /// Whether the middle region is painted.
    pub fill: bool,
}

/// One side of `border-image-width`.
#[derive(Clone, Debug, PartialEq)]
pub enum BorderImageWidthSide {
    /// A `<length-percentage>`.
    Length(Length),
    /// A multiple of the corresponding `border-*-width`.
    Number(f32),
    /// Use the slice's intrinsic size.
    Auto,
}

/// `background-repeat` / `border-image-repeat`, per axis.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct RepeatStyle {
    /// Horizontal behaviour.
    pub x: Keyword,
    /// Vertical behaviour.
    pub y: Keyword,
}

/// `background-size`.
#[derive(Clone, Debug, PartialEq)]
pub enum BgSize {
    /// `auto auto`.
    Auto,
    /// `cover`.
    Cover,
    /// `contain`.
    Contain,
    /// Explicit width and height (`auto` on an axis becomes `Length::Auto`).
    Explicit(Length, Length),
}

/// CSS's 1-to-4-value box rule, expanded to `[top, right, bottom, left]`.
#[must_use]
pub fn four_sides<T: Clone>(values: Vec<T>) -> Option<[T; 4]> {
    match values.len() {
        1 => Some([
            values[0].clone(),
            values[0].clone(),
            values[0].clone(),
            values[0].clone(),
        ]),
        2 => Some([
            values[0].clone(),
            values[1].clone(),
            values[0].clone(),
            values[1].clone(),
        ]),
        3 => Some([
            values[0].clone(),
            values[1].clone(),
            values[2].clone(),
            values[1].clone(),
        ]),
        4 => Some([
            values[0].clone(),
            values[1].clone(),
            values[2].clone(),
            values[3].clone(),
        ]),
        _ => None,
    }
}

/// `<line-width> = thin | medium | thick | <length>`, always in pixels for
/// the keyword forms (M1's mapping: 1, 3, 5).
pub fn parse_line_width(input: &mut Parser<'_, '_>) -> Result<Length, ()> {
    let state = input.state();
    let token = input.next().map_err(|_| ())?.clone();
    if let Token::Ident(ref name) = token {
        if name.eq_ignore_ascii_case("thin") {
            return Ok(Length::px(1.0));
        }
        if name.eq_ignore_ascii_case("medium") {
            return Ok(Length::px(3.0));
        }
        if name.eq_ignore_ascii_case("thick") {
            return Ok(Length::px(5.0));
        }
        return Err(());
    }
    input.reset(&state);
    Length::parse(input)
}

/// `<line-style>`.
pub fn parse_line_style(input: &mut Parser<'_, '_>) -> Result<Keyword, ()> {
    const STYLES: &[Keyword] = &[
        Keyword::None,
        Keyword::Hidden,
        Keyword::Dotted,
        Keyword::Dashed,
        Keyword::Solid,
        Keyword::Double,
        Keyword::Groove,
        Keyword::Ridge,
        Keyword::Inset,
        Keyword::Outset,
    ];
    let name = input.expect_ident().map_err(|_| ())?.as_ref().to_string();
    STYLES
        .iter()
        .copied()
        .find(|style| name.eq_ignore_ascii_case(style.as_str()))
        .ok_or(())
}

fn parse_number_or_percent(input: &mut Parser<'_, '_>) -> Result<NumberOrPercent, ()> {
    let token = input.next().map_err(|_| ())?.clone();
    match token {
        Token::Number { value, .. } if value.is_finite() => Ok(NumberOrPercent::Number(value)),
        Token::Percentage { unit_value, .. } if unit_value.is_finite() => {
            Ok(NumberOrPercent::Percent(unit_value))
        }
        _ => Err(()),
    }
}

impl BorderImageSlice {
    /// `<number-percentage>{1,4} && fill?`.
    pub fn parse(input: &mut Parser<'_, '_>) -> Result<BorderImageSlice, ()> {
        let mut fill = false;
        let mut values: Vec<NumberOrPercent> = Vec::new();
        loop {
            let state = input.state();
            let is_fill = matches!(
                input.next(),
                Ok(Token::Ident(name)) if name.eq_ignore_ascii_case("fill")
            );
            if is_fill {
                if fill {
                    input.reset(&state);
                    break;
                }
                fill = true;
                continue;
            }
            input.reset(&state);
            if values.len() < 4 {
                if let Ok(value) = parse_number_or_percent(input) {
                    values.push(value);
                    continue;
                }
                input.reset(&state);
            }
            break;
        }
        let sides = four_sides(values).ok_or(())?;
        Ok(BorderImageSlice { sides, fill })
    }
}

impl BorderImageWidthSide {
    /// `<length-percentage> | <number> | auto`.
    pub fn parse(input: &mut Parser<'_, '_>) -> Result<BorderImageWidthSide, ()> {
        let state = input.state();
        let token = input.next().map_err(|_| ())?.clone();
        match token {
            Token::Ident(ref name) if name.eq_ignore_ascii_case("auto") => {
                Ok(BorderImageWidthSide::Auto)
            }
            Token::Number { value, .. } if value.is_finite() => {
                Ok(BorderImageWidthSide::Number(value))
            }
            _ => {
                input.reset(&state);
                Length::parse(input).map(BorderImageWidthSide::Length)
            }
        }
    }
}

impl RepeatStyle {
    /// `border-image-repeat`: `[stretch | repeat | round | space]{1,2}`.
    pub fn parse(input: &mut Parser<'_, '_>) -> Result<RepeatStyle, ()> {
        const AXES: &[Keyword] = &[
            Keyword::Stretch,
            Keyword::Repeat,
            Keyword::Round,
            Keyword::Space,
        ];
        let x = parse_one_of(input, AXES)?;
        let state = input.state();
        let y = match parse_one_of(input, AXES) {
            Ok(keyword) => keyword,
            Err(()) => {
                input.reset(&state);
                x
            }
        };
        Ok(RepeatStyle { x, y })
    }

    /// `background-repeat`: adds `repeat-x` / `repeat-y` / `no-repeat`.
    pub fn parse_background(input: &mut Parser<'_, '_>) -> Result<RepeatStyle, ()> {
        const AXES: &[Keyword] = &[
            Keyword::Repeat,
            Keyword::Round,
            Keyword::Space,
            Keyword::NoRepeat,
        ];
        let state = input.state();
        if let Ok(name) = input.expect_ident() {
            if name.eq_ignore_ascii_case("repeat-x") {
                return Ok(RepeatStyle {
                    x: Keyword::Repeat,
                    y: Keyword::NoRepeat,
                });
            }
            if name.eq_ignore_ascii_case("repeat-y") {
                return Ok(RepeatStyle {
                    x: Keyword::NoRepeat,
                    y: Keyword::Repeat,
                });
            }
        }
        input.reset(&state);
        let x = parse_one_of(input, AXES)?;
        let state = input.state();
        let y = match parse_one_of(input, AXES) {
            Ok(keyword) => keyword,
            Err(()) => {
                input.reset(&state);
                x
            }
        };
        Ok(RepeatStyle { x, y })
    }
}

fn parse_one_of(input: &mut Parser<'_, '_>, allowed: &[Keyword]) -> Result<Keyword, ()> {
    let name = input.expect_ident().map_err(|_| ())?.as_ref().to_string();
    allowed
        .iter()
        .copied()
        .find(|keyword| name.eq_ignore_ascii_case(keyword.as_str()))
        .ok_or(())
}

impl BgSize {
    /// `[<length-percentage> | auto]{1,2} | cover | contain`.
    pub fn parse(input: &mut Parser<'_, '_>) -> Result<BgSize, ()> {
        let state = input.state();
        if let Ok(name) = input.expect_ident() {
            if name.eq_ignore_ascii_case("cover") {
                return Ok(BgSize::Cover);
            }
            if name.eq_ignore_ascii_case("contain") {
                return Ok(BgSize::Contain);
            }
            if name.eq_ignore_ascii_case("auto") {
                let state = input.state();
                return match Length::parse_allowing_auto(input) {
                    Ok(height) => Ok(BgSize::Explicit(Length::Auto, height)),
                    Err(()) => {
                        input.reset(&state);
                        Ok(BgSize::Auto)
                    }
                };
            }
        }
        input.reset(&state);
        let width = Length::parse_allowing_auto(input)?;
        let state = input.state();
        let height = match Length::parse_allowing_auto(input) {
            Ok(height) => height,
            Err(()) => {
                input.reset(&state);
                Length::Auto
            }
        };
        Ok(BgSize::Explicit(width, height))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BgSize, BorderImageSlice, BorderImageWidthSide, NumberOrPercent, RepeatStyle, four_sides,
        parse_line_style, parse_line_width,
    };
    use crate::css::value::keyword::Keyword;
    use crate::css::value::length::Length;
    use crate::css::value::parse_entirely_with;

    #[test]
    fn line_width_keywords_resolve_to_pixels() {
        // M1's mapping, preserved: thin 1, medium 3, thick 5.
        assert_eq!(
            parse_entirely_with("thin", parse_line_width).ok(),
            Some(Length::px(1.0))
        );
        assert_eq!(
            parse_entirely_with("MEDIUM", parse_line_width).ok(),
            Some(Length::px(3.0))
        );
        assert_eq!(
            parse_entirely_with("thick", parse_line_width).ok(),
            Some(Length::px(5.0))
        );
        assert_eq!(
            parse_entirely_with("2px", parse_line_width).ok(),
            Some(Length::px(2.0))
        );
        assert_eq!(parse_entirely_with("red", parse_line_width).ok(), None);
    }

    #[test]
    fn every_border_style_keyword_parses_and_nothing_else_does() {
        for (text, expected) in [
            ("none", Keyword::None),
            ("hidden", Keyword::Hidden),
            ("dotted", Keyword::Dotted),
            ("dashed", Keyword::Dashed),
            ("SOLID", Keyword::Solid),
            ("double", Keyword::Double),
            ("groove", Keyword::Groove),
            ("ridge", Keyword::Ridge),
            ("inset", Keyword::Inset),
            ("outset", Keyword::Outset),
        ] {
            assert_eq!(
                parse_entirely_with(text, parse_line_style).ok(),
                Some(expected)
            );
        }
        assert_eq!(parse_entirely_with("thin", parse_line_style).ok(), None);
    }

    #[test]
    fn the_four_sides_rule_matches_css() {
        assert_eq!(four_sides(vec![1]), Some([1, 1, 1, 1]));
        assert_eq!(four_sides(vec![1, 2]), Some([1, 2, 1, 2]));
        assert_eq!(four_sides(vec![1, 2, 3]), Some([1, 2, 3, 2]));
        assert_eq!(four_sides(vec![1, 2, 3, 4]), Some([1, 2, 3, 4]));
        assert_eq!(four_sides(vec![1, 2, 3, 4, 5]), None);
        assert_eq!(four_sides(Vec::<i32>::new()), None);
    }

    #[test]
    fn border_image_slice_width_and_repeat_parse() {
        let slice = parse_entirely_with("30% fill", BorderImageSlice::parse).expect("parses");
        assert!(slice.fill);
        assert_eq!(slice.sides, [NumberOrPercent::Percent(0.3); 4]);
        assert_eq!(
            parse_entirely_with("auto", BorderImageWidthSide::parse).ok(),
            Some(BorderImageWidthSide::Auto)
        );
        assert_eq!(
            parse_entirely_with("2", BorderImageWidthSide::parse).ok(),
            Some(BorderImageWidthSide::Number(2.0))
        );
        assert_eq!(
            parse_entirely_with("round space", RepeatStyle::parse).ok(),
            Some(RepeatStyle {
                x: Keyword::Round,
                y: Keyword::Space
            })
        );
        // One value applies to both axes.
        assert_eq!(
            parse_entirely_with("stretch", RepeatStyle::parse).ok(),
            Some(RepeatStyle {
                x: Keyword::Stretch,
                y: Keyword::Stretch
            })
        );
        // `repeat-x`/`repeat-y` are background-repeat only, never here.
        assert_eq!(
            parse_entirely_with("repeat-x", RepeatStyle::parse).ok(),
            None
        );
    }

    #[test]
    fn background_repeat_accepts_the_single_keyword_shorthands() {
        assert_eq!(
            parse_entirely_with("repeat-x", RepeatStyle::parse_background).ok(),
            Some(RepeatStyle {
                x: Keyword::Repeat,
                y: Keyword::NoRepeat
            })
        );
        assert_eq!(
            parse_entirely_with("repeat-y", RepeatStyle::parse_background).ok(),
            Some(RepeatStyle {
                x: Keyword::NoRepeat,
                y: Keyword::Repeat
            })
        );
    }

    #[test]
    fn background_size_parses_its_three_forms() {
        assert_eq!(
            parse_entirely_with("cover", BgSize::parse).ok(),
            Some(BgSize::Cover)
        );
        assert_eq!(
            parse_entirely_with("contain", BgSize::parse).ok(),
            Some(BgSize::Contain)
        );
        assert_eq!(
            parse_entirely_with("auto", BgSize::parse).ok(),
            Some(BgSize::Auto)
        );
        assert_eq!(
            parse_entirely_with("10px 50%", BgSize::parse).ok(),
            Some(BgSize::Explicit(Length::px(10.0), Length::Percent(0.5)))
        );
    }

    #[test]
    fn border_value_parsing_never_panics() {
        for input in crate::css::value::FUZZ_INPUTS {
            let _ = parse_entirely_with(input, parse_line_width);
            let _ = parse_entirely_with(input, parse_line_style);
            let _ = parse_entirely_with(input, BorderImageSlice::parse);
            let _ = parse_entirely_with(input, BorderImageWidthSide::parse);
            let _ = parse_entirely_with(input, RepeatStyle::parse);
            let _ = parse_entirely_with(input, RepeatStyle::parse_background);
            let _ = parse_entirely_with(input, BgSize::parse);
        }
    }
}
