//! `box-shadow`, `text-shadow` and `-gtk-icon-shadow` values.

use cssparser::Parser;

use super::color::ColorValue;
use super::length::Length;

/// One shadow. `text-shadow` and `-gtk-icon-shadow` always have
/// `spread == 0` and `inset == false`.
#[derive(Clone, Debug, PartialEq)]
pub struct Shadow {
    /// `None` == `currentColor` at used-value time.
    pub color: Option<ColorValue>,
    /// Horizontal offset.
    pub offset_x: Length,
    /// Vertical offset.
    pub offset_y: Length,
    /// Blur radius.
    pub blur: Length,
    /// Spread radius.
    pub spread: Length,
    /// Drawn inside the border edge.
    pub inset: bool,
}

impl Shadow {
    /// `<shadow> = inset? && <length>{2,4} && <color>?`.
    pub fn parse(input: &mut Parser<'_, '_>) -> Result<Shadow, ()> {
        parse_shadow(input, true)
    }

    /// `text-shadow`'s form: `<color>? && <length>{2,3}`, no `inset`.
    pub fn parse_text(input: &mut Parser<'_, '_>) -> Result<Shadow, ()> {
        parse_shadow(input, false)
    }
}

fn parse_shadow(input: &mut Parser<'_, '_>, box_form: bool) -> Result<Shadow, ()> {
    let mut lengths: Vec<Length> = Vec::new();
    let mut color: Option<ColorValue> = None;
    let mut inset = false;
    loop {
        let state = input.state();
        if box_form && !inset {
            let is_inset = matches!(
                input.next(),
                Ok(cssparser::Token::Ident(name)) if name.eq_ignore_ascii_case("inset")
            );
            if is_inset {
                inset = true;
                continue;
            }
            input.reset(&state);
        }
        if lengths.len() < if box_form { 4 } else { 3 } {
            if let Ok(length) = Length::parse(input) {
                lengths.push(length);
                continue;
            }
            input.reset(&state);
        }
        if color.is_none() {
            if let Ok(parsed) = ColorValue::parse(input) {
                color = Some(parsed);
                continue;
            }
            input.reset(&state);
        }
        break;
    }
    if lengths.len() < 2 {
        return Err(());
    }
    Ok(Shadow {
        color,
        offset_x: lengths[0].clone(),
        offset_y: lengths[1].clone(),
        blur: lengths.get(2).cloned().unwrap_or_else(Length::zero),
        spread: lengths.get(3).cloned().unwrap_or_else(Length::zero),
        inset,
    })
}

#[cfg(test)]
mod tests {
    use super::Shadow;
    use crate::css::value::color::ColorValue;
    use crate::css::value::length::Length;
    use crate::css::value::parse_entirely_with;

    #[test]
    fn a_box_shadow_reads_offsets_blur_spread_colour_and_inset_in_any_order() {
        let shadow = parse_entirely_with("inset 1px 2px 3px 4px #ff0000", Shadow::parse)
            .expect("shadow parses");
        assert!(shadow.inset);
        assert_eq!(shadow.offset_x, Length::px(1.0));
        assert_eq!(shadow.offset_y, Length::px(2.0));
        assert_eq!(shadow.blur, Length::px(3.0));
        assert_eq!(shadow.spread, Length::px(4.0));
        assert!(matches!(shadow.color, Some(ColorValue::Absolute(_))));

        let reordered =
            parse_entirely_with("#ff0000 1px 2px inset", Shadow::parse).expect("shadow parses");
        assert!(reordered.inset);
        assert_eq!(reordered.blur, Length::zero());
        assert_eq!(reordered.spread, Length::zero());
    }

    #[test]
    fn an_omitted_colour_stays_none_so_it_can_become_current_colour_later() {
        let shadow = parse_entirely_with("0 1px 2px", Shadow::parse).expect("shadow parses");
        assert_eq!(shadow.color, None);
        assert_eq!(shadow.offset_x, Length::zero());
    }

    #[test]
    fn a_text_shadow_takes_no_spread_and_no_inset() {
        let shadow = parse_entirely_with("1px 2px 3px #000000", Shadow::parse_text)
            .expect("text-shadow parses");
        assert_eq!(shadow.spread, Length::zero());
        assert!(!shadow.inset);
        assert!(parse_entirely_with("1px 2px 3px 4px", Shadow::parse_text).is_err());
        assert!(parse_entirely_with("inset 1px 2px", Shadow::parse_text).is_err());
    }

    #[test]
    fn two_offsets_are_mandatory() {
        assert!(parse_entirely_with("1px", Shadow::parse).is_err());
        assert!(parse_entirely_with("red", Shadow::parse).is_err());
    }

    #[test]
    fn shadow_parsing_never_panics() {
        for input in crate::css::value::FUZZ_INPUTS {
            let _ = parse_entirely_with(input, Shadow::parse);
            let _ = parse_entirely_with(input, Shadow::parse_text);
        }
    }
}
