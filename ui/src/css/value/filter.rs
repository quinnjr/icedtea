//! `filter` and `-gtk-icon-filter` values.

use std::rc::Rc;

use cssparser::{Parser, Token};

use super::calc::parse_angle;
use super::length::Length;
use super::shadow::Shadow;

/// One `<filter-function>` -- exactly the set GTK documents.
#[derive(Clone, Debug, PartialEq)]
pub enum FilterFn {
    /// `blur(<length>)`.
    Blur(Length),
    /// `brightness(<number-percentage>)`.
    Brightness(f32),
    /// `contrast(<number-percentage>)`.
    Contrast(f32),
    /// `drop-shadow(<shadow>)`.
    DropShadow(Shadow),
    /// `grayscale(<number-percentage>)`.
    Grayscale(f32),
    /// `hue-rotate(<angle>)`, degrees.
    HueRotate(f32),
    /// `invert(<number-percentage>)`.
    Invert(f32),
    /// `opacity(<number-percentage>)`.
    Opacity(f32),
    /// `saturate(<number-percentage>)`.
    Saturate(f32),
    /// `sepia(<number-percentage>)`.
    Sepia(f32),
}

fn amount(input: &mut Parser<'_, '_>, default: f32) -> Result<f32, ()> {
    let state = input.state();
    let token = match input.next() {
        Ok(token) => token.clone(),
        Err(_) => {
            input.reset(&state);
            return Ok(default);
        }
    };
    match token {
        Token::Number { value, .. } if value.is_finite() => Ok(value),
        Token::Percentage { unit_value, .. } if unit_value.is_finite() => Ok(unit_value),
        _ => {
            input.reset(&state);
            Err(())
        }
    }
}

/// `none | <filter-function>+`.
pub fn parse_filter_list(input: &mut Parser<'_, '_>) -> Result<Rc<[FilterFn]>, ()> {
    super::parse_function_list(input, parse_filter_function)
}

fn parse_filter_function(name: &str, input: &mut Parser<'_, '_>) -> Result<FilterFn, ()> {
    let function = match name {
        "blur" => {
            let state = input.state();
            match Length::parse(input) {
                Ok(length) => FilterFn::Blur(length),
                Err(()) => {
                    input.reset(&state);
                    return Err(());
                }
            }
        }
        "brightness" => FilterFn::Brightness(amount(input, 1.0)?),
        "contrast" => FilterFn::Contrast(amount(input, 1.0)?),
        "grayscale" => FilterFn::Grayscale(amount(input, 1.0)?),
        "invert" => FilterFn::Invert(amount(input, 1.0)?),
        "opacity" => FilterFn::Opacity(amount(input, 1.0)?),
        "saturate" => FilterFn::Saturate(amount(input, 1.0)?),
        "sepia" => FilterFn::Sepia(amount(input, 1.0)?),
        "hue-rotate" => {
            let state = input.state();
            match parse_angle(input) {
                Ok(angle) => FilterFn::HueRotate(angle),
                Err(()) => {
                    input.reset(&state);
                    FilterFn::HueRotate(0.0)
                }
            }
        }
        "drop-shadow" => FilterFn::DropShadow(Shadow::parse_text(input)?),
        _ => return Err(()),
    };
    input.skip_whitespace();
    if input.is_exhausted() {
        Ok(function)
    } else {
        Err(())
    }
}

#[cfg(test)]
mod tests {
    use super::{FilterFn, parse_filter_list};
    use crate::css::value::length::Length;
    use crate::css::value::parse_entirely_with;

    fn list(text: &str) -> Option<Vec<FilterFn>> {
        parse_entirely_with(text, parse_filter_list)
            .ok()
            .map(|functions| functions.to_vec())
    }

    #[test]
    fn every_gtk_filter_function_parses() {
        assert_eq!(list("none"), Some(Vec::new()));
        assert_eq!(
            list("blur(3px)"),
            Some(vec![FilterFn::Blur(Length::px(3.0))])
        );
        assert_eq!(
            list("brightness(50%)"),
            Some(vec![FilterFn::Brightness(0.5)])
        );
        assert_eq!(list("contrast(1.5)"), Some(vec![FilterFn::Contrast(1.5)]));
        assert_eq!(
            list("hue-rotate(90deg)"),
            Some(vec![FilterFn::HueRotate(90.0)])
        );
        assert!(list("grayscale(1) invert(1) opacity(0.5) saturate(2) sepia(1)").is_some());
        assert!(matches!(
            list("drop-shadow(1px 2px 3px red)").as_deref(),
            Some([FilterFn::DropShadow(_)])
        ));
        assert!(list("blur()").is_none());
        assert!(list("wobble(1)").is_none());
    }

    #[test]
    fn an_omitted_amount_defaults_per_function() {
        // CSS: brightness()/contrast()/saturate() default to 1, the rest to 1
        // except opacity(), which is also 1. A missing argument is only legal
        // for the amount-taking functions.
        assert_eq!(list("grayscale()"), Some(vec![FilterFn::Grayscale(1.0)]));
        assert_eq!(list("hue-rotate()"), Some(vec![FilterFn::HueRotate(0.0)]));
    }

    #[test]
    fn filter_parsing_never_panics() {
        for input in crate::css::value::FUZZ_INPUTS {
            let _ = parse_entirely_with(input, parse_filter_list);
        }
    }
}
