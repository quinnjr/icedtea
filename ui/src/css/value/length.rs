//! Lengths and the context that turns them into device pixels.

use std::rc::Rc;

use cssparser::{Parser, Token};

use super::calc::{CalcNode, parse_math_function};

/// Every absolute or font-relative unit GTK accepts.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LengthUnit {
    /// CSS pixel.
    Px,
    /// Point.
    Pt,
    /// Pica.
    Pc,
    /// Inch.
    In,
    /// Centimetre.
    Cm,
    /// Millimetre.
    Mm,
    /// The element's own font size.
    Em,
    /// The resolved face's x-height.
    Ex,
    /// GTK's *initial* font size -- not the root node's (GTK note, §4).
    Rem,
}

impl LengthUnit {
    /// ASCII-case-insensitive unit lookup.
    #[must_use]
    pub fn from_str_ascii_ci(unit: &str) -> Option<LengthUnit> {
        const UNITS: &[(&str, LengthUnit)] = &[
            ("px", LengthUnit::Px),
            ("pt", LengthUnit::Pt),
            ("pc", LengthUnit::Pc),
            ("in", LengthUnit::In),
            ("cm", LengthUnit::Cm),
            ("mm", LengthUnit::Mm),
            ("em", LengthUnit::Em),
            ("ex", LengthUnit::Ex),
            ("rem", LengthUnit::Rem),
        ];
        UNITS
            .iter()
            .find(|(name, _)| unit.eq_ignore_ascii_case(name))
            .map(|(_, value)| *value)
    }

    /// The unit's CSS spelling.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            LengthUnit::Px => "px",
            LengthUnit::Pt => "pt",
            LengthUnit::Pc => "pc",
            LengthUnit::In => "in",
            LengthUnit::Cm => "cm",
            LengthUnit::Mm => "mm",
            LengthUnit::Em => "em",
            LengthUnit::Ex => "ex",
            LengthUnit::Rem => "rem",
        }
    }

    /// Physical units convert through `-gtk-dpi`, **not** CSS's fixed 96dpi.
    fn to_px(self, value: f32, ctx: &LengthCtx) -> f32 {
        match self {
            LengthUnit::Px => value,
            LengthUnit::Pt => value * ctx.dpi / 72.0,
            LengthUnit::Pc => value * ctx.dpi / 6.0,
            LengthUnit::In => value * ctx.dpi,
            LengthUnit::Cm => value * ctx.dpi / 2.54,
            LengthUnit::Mm => value * ctx.dpi / 25.4,
            LengthUnit::Em => value * ctx.font_size_px,
            LengthUnit::Ex => value * ctx.font_size_px * ctx.ex_ratio,
            LengthUnit::Rem => value * ctx.root_font_size_px,
        }
    }
}

/// A `<length>`, `<percentage>`, math expression or `auto`, unresolved.
#[derive(Clone, Debug, PartialEq)]
pub enum Length {
    /// A dimension with a unit.
    Abs {
        /// The numeric part.
        value: f32,
        /// The unit.
        unit: LengthUnit,
    },
    /// A percentage, stored as a fraction (`100%` == `1.0`).
    Percent(f32),
    /// A math expression.
    Calc(Rc<CalcNode>),
    /// `auto` -- valid on `margin-*` and `border-image-width` only.
    Auto,
}

/// Everything needed to turn a [`Length`] into device pixels.
#[derive(Copy, Clone, Debug)]
pub struct LengthCtx {
    /// This element's computed `font-size`.
    pub font_size_px: f32,
    /// GTK's *initial* `font-size` -- the `rem` base, not the root node's.
    pub root_font_size_px: f32,
    /// x-height / em of the resolved face; `0.5` when unknown.
    pub ex_ratio: f32,
    /// `-gtk-dpi`; `pt`/`pc`/`in`/`cm`/`mm` convert through this.
    pub dpi: f32,
    /// `None` == percentages are invalid in this position.
    pub percent_basis: Option<f32>,
}

impl Default for LengthCtx {
    fn default() -> Self {
        LengthCtx {
            font_size_px: 14.0,
            root_font_size_px: 14.0,
            ex_ratio: 0.5,
            dpi: 96.0,
            percent_basis: None,
        }
    }
}

impl Length {
    /// Zero pixels.
    #[must_use]
    pub fn zero() -> Length {
        Length::px(0.0)
    }

    /// `value` pixels.
    #[must_use]
    pub fn px(value: f32) -> Length {
        Length::Abs {
            value,
            unit: LengthUnit::Px,
        }
    }

    /// Parse a `<length-percentage>` or math function. `auto` is rejected --
    /// use [`Length::parse_allowing_auto`] on the two properties that take it.
    pub fn parse(input: &mut Parser<'_, '_>) -> Result<Length, ()> {
        let token = input.next().map_err(|_| ())?.clone();
        match token {
            Token::Dimension {
                value, ref unit, ..
            } => {
                if !value.is_finite() {
                    return Err(());
                }
                let unit = LengthUnit::from_str_ascii_ci(unit).ok_or(())?;
                Ok(Length::Abs { value, unit })
            }
            Token::Percentage { unit_value, .. } => {
                if unit_value.is_finite() {
                    Ok(Length::Percent(unit_value))
                } else {
                    Err(())
                }
            }
            Token::Number { value, .. } => {
                // A unitless zero is the one bare number that is a length.
                if value == 0.0 {
                    Ok(Length::zero())
                } else {
                    Err(())
                }
            }
            Token::Function(ref name) => {
                let name = name.clone();
                Ok(Length::Calc(Rc::new(parse_math_function(&name, input)?)))
            }
            _ => Err(()),
        }
    }

    /// [`Length::parse`], plus the `auto` keyword.
    pub fn parse_allowing_auto(input: &mut Parser<'_, '_>) -> Result<Length, ()> {
        let state = input.state();
        if let Ok(Token::Ident(name)) = input.next()
            && name.eq_ignore_ascii_case("auto")
        {
            return Ok(Length::Auto);
        }
        input.reset(&state);
        Length::parse(input)
    }

    /// Resolve to device pixels. `None` == invalid at computed-value time:
    /// `auto`, a percentage with no basis, or a non-finite result.
    #[must_use]
    pub fn resolve(&self, ctx: &LengthCtx) -> Option<f32> {
        let px = match self {
            Length::Abs { value, unit } => unit.to_px(*value, ctx),
            Length::Percent(fraction) => fraction * ctx.percent_basis?,
            Length::Calc(node) => node.resolve_length(ctx)?,
            Length::Auto => return None,
        };
        px.is_finite().then_some(px)
    }
}

#[cfg(test)]
mod tests {
    use super::{Length, LengthCtx, LengthUnit};
    use crate::css::value::parse_entirely_with;

    fn len(text: &str) -> Option<Length> {
        parse_entirely_with(text, Length::parse).ok()
    }

    fn px(text: &str, ctx: &LengthCtx) -> Option<f32> {
        len(text)?.resolve(ctx)
    }

    #[test]
    fn absolute_units_convert_through_gtk_dpi_not_a_fixed_96() {
        // GTK resolves pt/pc/in/cm/mm through `-gtk-dpi`, NOT CSS's fixed
        // 96dpi (research/gtk-css-semantics.md §4).
        let ctx = LengthCtx {
            dpi: 192.0,
            ..LengthCtx::default()
        };
        assert_eq!(px("12px", &ctx), Some(12.0));
        assert_eq!(px("72pt", &ctx), Some(192.0));
        assert_eq!(px("1in", &ctx), Some(192.0));
        assert_eq!(px("1pc", &ctx), Some(32.0));
        let ninety_six = LengthCtx {
            dpi: 96.0,
            ..LengthCtx::default()
        };
        assert_eq!(px("72pt", &ninety_six), Some(96.0));
    }

    #[test]
    fn font_relative_units_use_their_own_bases() {
        let ctx = LengthCtx {
            font_size_px: 20.0,
            root_font_size_px: 14.0,
            ex_ratio: 0.5,
            ..LengthCtx::default()
        };
        assert_eq!(px("2em", &ctx), Some(40.0));
        assert_eq!(px("2rem", &ctx), Some(28.0));
        assert_eq!(px("2ex", &ctx), Some(20.0));
    }

    #[test]
    fn percentages_need_a_basis_and_are_stored_as_fractions() {
        assert_eq!(len("50%"), Some(Length::Percent(0.5)));
        let with_basis = LengthCtx {
            percent_basis: Some(200.0),
            ..LengthCtx::default()
        };
        assert_eq!(px("50%", &with_basis), Some(100.0));
        // No basis == invalid at computed-value time, not a parse error.
        assert!(len("50%").is_some());
        assert_eq!(px("50%", &LengthCtx::default()), None);
    }

    #[test]
    fn unitless_zero_parses_but_other_bare_numbers_do_not() {
        assert_eq!(len("0"), Some(Length::zero()));
        assert_eq!(len("3"), None);
        assert_eq!(len("3.5"), None);
    }

    #[test]
    fn auto_is_only_accepted_by_the_auto_aware_entry_point() {
        assert_eq!(len("auto"), None);
        assert_eq!(
            parse_entirely_with("auto", Length::parse_allowing_auto).ok(),
            Some(Length::Auto)
        );
        assert_eq!(Length::Auto.resolve(&LengthCtx::default()), None);
    }

    #[test]
    fn units_are_ascii_case_insensitive_and_unknown_units_are_rejected() {
        assert_eq!(len("4PX"), Some(Length::px(4.0)));
        assert_eq!(
            len("4Em"),
            Some(Length::Abs {
                value: 4.0,
                unit: LengthUnit::Em
            })
        );
        assert_eq!(len("4vh"), None);
        assert_eq!(len("4"), None);
    }

    #[test]
    fn a_non_finite_length_never_resolves() {
        // A theme that manages to express an overflowing dimension must not
        // poison layout with a NaN.
        assert_eq!(px("1e40px", &LengthCtx::default()), None);
    }

    #[test]
    fn trailing_tokens_make_the_whole_value_invalid() {
        assert_eq!(len("4px 9px"), None);
        assert_eq!(len("4px!"), None);
    }

    #[test]
    fn length_parsing_never_panics() {
        for input in crate::css::value::FUZZ_INPUTS {
            let _ = parse_entirely_with(input, Length::parse);
            let _ = parse_entirely_with(input, Length::parse_allowing_auto);
            if let Ok(length) = parse_entirely_with(input, Length::parse_allowing_auto) {
                let _ = length.resolve(&LengthCtx::default());
                let _ = length.resolve(&LengthCtx {
                    percent_basis: Some(100.0),
                    ..LengthCtx::default()
                });
            }
        }
    }
}
