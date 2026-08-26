//! Times and easing functions.

use cssparser::{Parser, Token};

use super::calc::parse_math_function;

/// A CSS `<time>`, always in seconds.
#[derive(Copy, Clone, Debug, PartialEq, PartialOrd)]
pub struct Time(pub f32);

impl Time {
    /// Zero seconds -- the initial value of every `*-duration`/`*-delay`.
    pub const ZERO: Time = Time(0.0);

    /// Build a time from milliseconds.
    #[must_use]
    pub fn from_ms(ms: f32) -> Time {
        Time(ms / 1000.0)
    }

    /// Seconds.
    #[must_use]
    pub fn as_secs_f32(self) -> f32 {
        self.0
    }

    /// Milliseconds.
    #[must_use]
    pub fn as_millis_f32(self) -> f32 {
        self.0 * 1000.0
    }

    /// Parse `<time>`: `s`, `ms`, a unitless `0`, or a math function.
    ///
    /// CSS proper rejects a unitless zero for `<time>`; GTK themes are
    /// written by hand and this engine accepts it, matching GTK's tolerance.
    pub fn parse(input: &mut Parser<'_, '_>) -> Result<Time, ()> {
        let token = input.next().map_err(|_| ())?.clone();
        match token {
            Token::Dimension {
                value, ref unit, ..
            } => {
                if !value.is_finite() {
                    return Err(());
                }
                if unit.eq_ignore_ascii_case("s") {
                    Ok(Time(value))
                } else if unit.eq_ignore_ascii_case("ms") {
                    Ok(Time(value / 1000.0))
                } else {
                    Err(())
                }
            }
            Token::Number { value, .. } => {
                // GTK tolerates a unitless zero where CSS proper would not.
                if value == 0.0 {
                    Ok(Time::ZERO)
                } else {
                    Err(())
                }
            }
            Token::Function(ref name) => {
                let name = name.clone();
                parse_math_function(&name, input)?.resolve_time().ok_or(())
            }
            _ => Err(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Time;
    use crate::css::value::parse_entirely_with;

    fn time(text: &str) -> Option<Time> {
        parse_entirely_with(text, Time::parse).ok()
    }

    #[test]
    fn seconds_and_milliseconds_both_land_in_seconds() {
        assert_eq!(time("2s"), Some(Time(2.0)));
        assert_eq!(time("200ms"), Some(Time(0.2)));
        assert_eq!(time("0S"), Some(Time(0.0)));
        assert_eq!(time("-1.5s"), Some(Time(-1.5)));
        assert_eq!(Time::from_ms(250.0), Time(0.25));
        assert_eq!(Time(0.25).as_millis_f32(), 250.0);
        assert_eq!(Time(0.25).as_secs_f32(), 0.25);
    }

    #[test]
    fn calc_reaches_time_values() {
        assert_eq!(time("calc(100ms * 3)"), Some(Time(0.3)));
        assert_eq!(time("calc(1s - 250ms)"), Some(Time(0.75)));
    }

    #[test]
    fn a_bare_number_other_than_zero_is_not_a_time() {
        assert_eq!(time("0"), Some(Time(0.0)));
        assert_eq!(time("5"), None);
        assert_eq!(time("5px"), None);
        assert_eq!(time("2s 1s"), None);
    }

    #[test]
    fn time_parsing_never_panics() {
        for input in crate::css::value::FUZZ_INPUTS {
            let _ = parse_entirely_with(input, Time::parse);
        }
    }
}
