//! Typed CSS values.
//!
//! Every parser here takes a `cssparser::Parser`, is ASCII-case- and
//! whitespace-insensitive, resolves nothing (`@name`, `currentColor`,
//! `em`/`rem`/`%` and `calc()` all survive unresolved into the value), and
//! never panics on any token stream. `Err(())` means *invalid at parse
//! time*: the declaration is dropped, CSS-style.

// Every value parser in this module tree returns `Result<T, ()>`: the
// contract freezes that signature, and `Err(())` carries all the meaning
// CSS gives it -- *invalid at parse time*, drop the declaration.
#![allow(clippy::result_unit_err)]

pub mod border;
pub mod calc;
pub mod color;
pub mod filter;
pub mod font;
pub mod image;
pub mod interpolate;
pub mod keyword;
pub mod length;
pub mod shadow;
pub mod text;
pub mod timing;
pub mod transform;

pub use border::{BgSize, BorderImageSlice, BorderImageWidthSide, NumberOrPercent, RepeatStyle};
pub use calc::CalcNode;
pub use color::{ColorCtx, ColorSpace, ColorTable, ColorValue, Rgba};
pub use filter::FilterFn;
pub use font::{
    FeatureSetting, FontFamily, FontStyle, FontVariantFlags, FontWeight, GenericFamily, LineHeight,
    VariationSetting,
};
pub use image::{Gradient, IconRef, Image, Position};
pub use keyword::{Keyword, Wide};
pub use length::{Length, LengthCtx, LengthUnit};
pub use shadow::Shadow;
pub use text::TextDecorationLines;
pub use timing::{AnimationName, IterationCount, StepPosition, Time, TimingFunction};
pub use transform::{Decomposed2d, TransformFn};

/// Odd inputs every value family's never-panic battery runs.
///
/// One corpus, shared, so a new hostile input added for one family
/// immediately covers all of them.
#[cfg(test)]
pub(crate) const FUZZ_INPUTS: &[&str] = &[
    "",
    " ",
    "\t\n",
    "/**/",
    "0",
    "-",
    "+",
    ".",
    "e",
    "e10",
    "1e999",
    "-1e999",
    "nan",
    "inf",
    "-0",
    "#",
    "#z",
    "#\u{e9}",
    "#\u{1f600}\u{1f600}\u{1f600}",
    "\u{e9}",
    "\u{0}\u{1}\u{2}",
    "@",
    "@\u{e9}",
    "(",
    ")",
    "()",
    "[",
    "{",
    "{}",
    ",",
    ",,,",
    "/",
    "//",
    "!",
    "!important",
    "url(",
    "url()",
    "calc(",
    "calc()",
    "calc(1px",
    "calc(1px +)",
    "calc(1px + )",
    "calc(* 2)",
    "calc(1px * 2px)",
    "calc(1 / 0)",
    "calc(1px + 1s)",
    "min()",
    "max()",
    "clamp(1px)",
    "rgb(",
    "rgb()",
    "rgb(1,2)",
    "rgb(1,2,3,4,5)",
    "rgb(from)",
    "hsl(\u{e9})",
    "color-mix(",
    "color-mix(in)",
    "linear-gradient(",
    "linear-gradient()",
    "linear-gradient(red)",
    "radial-gradient(at)",
    "conic-gradient(from)",
    "cross-fade()",
    "-gtk-icontheme()",
    "-gtk-scaled(a)",
    "image()",
    "steps()",
    "steps(0)",
    "steps(-1, end)",
    "cubic-bezier()",
    "cubic-bezier(1,2,3)",
    "cubic-bezier(2,0,0,1)",
    "matrix()",
    "matrix(1)",
    "translate()",
    "rotate()",
    "blur()",
    "drop-shadow()",
    "inset",
    "1px 2px 3px 4px 5px 6px",
    "1px/",
    "/1px",
    "solid solid solid",
    "\u{202e}",
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
];

/// Parse `text` with `f`, requiring the whole input to be consumed.
///
/// The test-side mirror of the registry's whole-value rule: a trailing
/// token is an error.
#[cfg(test)]
pub(crate) fn parse_entirely_with<T>(
    text: &str,
    f: impl FnOnce(&mut cssparser::Parser<'_, '_>) -> Result<T, ()>,
) -> Result<T, ()> {
    let mut input = cssparser::ParserInput::new(text);
    let mut parser = cssparser::Parser::new(&mut input);
    let value = f(&mut parser)?;
    parser.skip_whitespace();
    if parser.is_exhausted() {
        Ok(value)
    } else {
        Err(())
    }
}

use std::rc::Rc;

use cssparser::Parser;

use crate::css::registry::ParseFn;

/// A parsed, unresolved property value.
///
/// Discrete keyword values all share [`Value::Keyword`], so the registry
/// can give them one interpolator and one equality rule.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// `inherit` / `initial` / `unset`.
    Wide(Wide),
    /// Any discrete keyword.
    Keyword(Keyword),
    /// A bare number.
    Number(f32),
    /// A percentage, as a fraction (`100%` == `1.0`).
    Percentage(f32),
    /// A length.
    Length(Length),
    /// An angle, always in degrees.
    Angle(f32),
    /// A time.
    Time(Time),
    /// A colour.
    Color(ColorValue),
    /// An image.
    Image(Image),
    /// One shadow.
    Shadow(Shadow),
    /// A transform list; `none` is the empty slice.
    Transform(Rc<[TransformFn]>),
    /// A filter list; `none` is the empty slice.
    Filter(Rc<[FilterFn]>),
    /// A position (`background-position`, `transform-origin`).
    Position(Position),
    /// A background size.
    BgSize(BgSize),
    /// A repeat style.
    Repeat(RepeatStyle),
    /// A font-family list.
    FontFamilies(Rc<[FontFamily]>),
    /// A font weight.
    FontWeight(FontWeight),
    /// A font style.
    FontStyle(FontStyle),
    /// Font-variant flags.
    FontVariant(FontVariantFlags),
    /// OpenType feature settings.
    FontFeatures(Rc<[FeatureSetting]>),
    /// OpenType variation settings.
    FontVariations(Rc<[VariationSetting]>),
    /// A line height.
    LineHeight(LineHeight),
    /// Text-decoration lines.
    TextDecorationLines(TextDecorationLines),
    /// An easing function.
    Timing(TimingFunction),
    /// An animation name, and `transition-property`'s ident carrier.
    AnimationName(AnimationName),
    /// An animation iteration count.
    IterationCount(IterationCount),
    /// A `border-image-slice`.
    Slice(BorderImageSlice),
    /// A `border-image-width`, TRBL.
    BorderImageWidths([BorderImageWidthSide; 4]),
    /// An icon palette: `name colour` pairs.
    IconPalette(Rc<[(Rc<str>, ColorValue)]>),
    /// A pair: corner radii `(h, v)` and `border-spacing`.
    Pair(Rc<(Value, Value)>),
    /// Every comma-multiplied (`#`) property's value.
    List(Rc<[Value]>),
}

impl Value {
    /// Parse `inherit | initial | unset` as a whole value.
    pub fn parse_wide(input: &mut Parser<'_, '_>) -> Result<Value, ()> {
        let name = input.expect_ident().map_err(|_| ())?.as_ref().to_string();
        let wide = Wide::from_str_ascii_ci(&name).ok_or(())?;
        input.skip_whitespace();
        if input.is_exhausted() {
            Ok(Value::Wide(wide))
        } else {
            Err(())
        }
    }

    /// Try the wide keywords first, then `f`. Every registry `ParseFn`
    /// that accepts a value goes through this.
    pub fn parse_wide_or(input: &mut Parser<'_, '_>, f: ParseFn) -> Result<Value, ()> {
        let state = input.state();
        match Value::parse_wide(input) {
            Ok(value) => Ok(value),
            Err(()) => {
                input.reset(&state);
                f(input)
            }
        }
    }

    /// Whether this is a CSS-wide keyword.
    #[must_use]
    pub fn is_wide(&self) -> bool {
        matches!(self, Value::Wide(_))
    }

    /// Parse a comma-separated list with `f`, always producing a
    /// [`Value::List`] -- even for a single item.
    pub fn parse_list(input: &mut Parser<'_, '_>, f: ParseFn) -> Result<Value, ()> {
        let items = input
            .parse_comma_separated(|argument| match f(argument) {
                Ok(value) => Ok(value),
                Err(()) => Err(argument.new_custom_error::<(), ()>(())),
            })
            .map_err(|_: cssparser::ParseError<'_, ()>| ())?;
        if items.is_empty() {
            return Err(());
        }
        Ok(Value::List(items.into()))
    }
}

#[cfg(test)]
mod tests {
    use super::{Keyword, Length, Value, Wide, parse_entirely_with};

    fn keyword_parser(input: &mut cssparser::Parser<'_, '_>) -> Result<Value, ()> {
        let name = input.expect_ident().map_err(|_| ())?.as_ref().to_string();
        Keyword::from_str_ascii_ci(&name)
            .map(Value::Keyword)
            .ok_or(())
    }

    fn length_parser(input: &mut cssparser::Parser<'_, '_>) -> Result<Value, ()> {
        Length::parse(input).map(Value::Length)
    }

    #[test]
    fn the_wide_keywords_are_recognised_on_any_property() {
        for (text, expected) in [
            ("inherit", Wide::Inherit),
            ("INITIAL", Wide::Initial),
            ("unset", Wide::Unset),
        ] {
            let value =
                parse_entirely_with(text, |input| Value::parse_wide_or(input, keyword_parser))
                    .expect("wide keyword parses");
            assert_eq!(value, Value::Wide(expected));
            assert!(value.is_wide());
        }
        // The inner parser still runs for anything else.
        assert_eq!(
            parse_entirely_with("none", |input| Value::parse_wide_or(input, keyword_parser)).ok(),
            Some(Value::Keyword(Keyword::None))
        );
        assert!(!Value::Keyword(Keyword::None).is_wide());
    }

    #[test]
    fn a_wide_keyword_with_trailing_tokens_is_not_a_wide_keyword() {
        // `inherit 4px` is invalid, not "inherit plus junk".
        assert!(
            parse_entirely_with("inherit 4px", |input| Value::parse_wide_or(
                input,
                length_parser
            ))
            .is_err()
        );
    }

    #[test]
    fn parse_list_splits_on_top_level_commas() {
        let value = parse_entirely_with("1px, 2px, 3px", |input| {
            Value::parse_list(input, length_parser)
        })
        .expect("list parses");
        match value {
            Value::List(items) => {
                assert_eq!(items.len(), 3);
                assert_eq!(items[0], Value::Length(Length::px(1.0)));
                assert_eq!(items[2], Value::Length(Length::px(3.0)));
            }
            other => panic!("expected a list, got {other:?}"),
        }
        // A single item is still a one-element list, so consumers never
        // have to handle two shapes.
        let single = parse_entirely_with("1px", |input| Value::parse_list(input, length_parser))
            .expect("single item parses");
        assert_eq!(
            single,
            Value::List(vec![Value::Length(Length::px(1.0))].into())
        );
        // One bad item invalidates the whole declaration.
        assert!(
            parse_entirely_with("1px, red", |input| Value::parse_list(input, length_parser))
                .is_err()
        );
    }
}
