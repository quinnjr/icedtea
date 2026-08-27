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

use std::rc::Rc;

/// Where a `steps()` function jumps.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum StepPosition {
    /// `jump-start` (alias `start`).
    JumpStart,
    /// `jump-end` (alias `end`, the default).
    JumpEnd,
    /// `jump-none`.
    JumpNone,
    /// `jump-both`.
    JumpBoth,
}

/// A CSS `<easing-function>`.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum TimingFunction {
    /// The identity.
    Linear,
    /// `cubic-bezier(x1, y1, x2, y2)`.
    CubicBezier(f32, f32, f32, f32),
    /// `steps(n, <position>)`.
    Steps(u32, StepPosition),
}

impl TimingFunction {
    /// `ease`.
    pub const EASE: TimingFunction = TimingFunction::CubicBezier(0.25, 0.1, 0.25, 1.0);
    /// `ease-in`.
    pub const EASE_IN: TimingFunction = TimingFunction::CubicBezier(0.42, 0.0, 1.0, 1.0);
    /// `ease-out`.
    pub const EASE_OUT: TimingFunction = TimingFunction::CubicBezier(0.0, 0.0, 0.58, 1.0);
    /// `ease-in-out`.
    pub const EASE_IN_OUT: TimingFunction = TimingFunction::CubicBezier(0.42, 0.0, 0.58, 1.0);

    /// Parse an `<easing-function>`.
    pub fn parse(input: &mut Parser<'_, '_>) -> Result<TimingFunction, ()> {
        let state = input.state();
        if let Ok(name) = input.expect_ident() {
            let name = name.as_ref().to_ascii_lowercase();
            return match name.as_str() {
                "linear" => Ok(TimingFunction::Linear),
                "ease" => Ok(TimingFunction::EASE),
                "ease-in" => Ok(TimingFunction::EASE_IN),
                "ease-out" => Ok(TimingFunction::EASE_OUT),
                "ease-in-out" => Ok(TimingFunction::EASE_IN_OUT),
                "step-start" => Ok(TimingFunction::Steps(1, StepPosition::JumpStart)),
                "step-end" => Ok(TimingFunction::Steps(1, StepPosition::JumpEnd)),
                _ => Err(()),
            };
        }
        input.reset(&state);
        let name = input
            .expect_function()
            .map_err(|_| ())?
            .as_ref()
            .to_ascii_lowercase();
        input
            .parse_nested_block(|inner| match parse_timing_body(&name, inner) {
                Ok(function) => Ok(function),
                Err(()) => Err(inner.new_custom_error::<(), ()>(())),
            })
            .map_err(|_: cssparser::ParseError<'_, ()>| ())
    }

    /// Evaluate the function at input progress `t`.
    ///
    /// Cubic beziers are solved for `t` on x by Newton-Raphson (8
    /// iterations) with a bisection fallback, and are exact at 0 and 1.
    #[must_use]
    pub fn eval(self, t: f32) -> f32 {
        match self {
            TimingFunction::Linear => t,
            TimingFunction::CubicBezier(x1, y1, x2, y2) => {
                if t <= 0.0 {
                    return 0.0;
                }
                if t >= 1.0 {
                    return 1.0;
                }
                let bezier = |a: f32, b: f32, u: f32| {
                    let v = 1.0 - u;
                    3.0 * v * v * u * a + 3.0 * v * u * u * b + u * u * u
                };
                let slope = |a: f32, b: f32, u: f32| {
                    let v = 1.0 - u;
                    3.0 * v * v * (a) + 6.0 * v * u * (b - a) + 3.0 * u * u * (1.0 - b)
                };
                let mut guess = t;
                for _ in 0..8 {
                    let x = bezier(x1, x2, guess) - t;
                    if x.abs() < 1e-6 {
                        return bezier(y1, y2, guess);
                    }
                    let derivative = slope(x1, x2, guess);
                    if derivative.abs() < 1e-6 {
                        break;
                    }
                    guess -= x / derivative;
                }
                let (mut low, mut high) = (0.0_f32, 1.0_f32);
                let mut guess = t;
                for _ in 0..32 {
                    let x = bezier(x1, x2, guess);
                    if (x - t).abs() < 1e-6 {
                        break;
                    }
                    if x > t {
                        high = guess;
                    } else {
                        low = guess;
                    }
                    guess = f32::midpoint(low, high);
                }
                bezier(y1, y2, guess)
            }
            TimingFunction::Steps(count, position) => {
                if count == 0 {
                    return t;
                }
                let steps = count as f32;
                let mut step = (t * steps).floor();
                if matches!(position, StepPosition::JumpStart | StepPosition::JumpBoth) {
                    step += 1.0;
                }
                let denominator = match position {
                    StepPosition::JumpNone => (steps - 1.0).max(1.0),
                    StepPosition::JumpBoth => steps + 1.0,
                    _ => steps,
                };
                if matches!(position, StepPosition::JumpNone) && t >= 1.0 {
                    return 1.0;
                }
                (step / denominator).clamp(0.0, 1.0)
            }
        }
    }
}

fn parse_timing_body(name: &str, input: &mut Parser<'_, '_>) -> Result<TimingFunction, ()> {
    let function = match name {
        "cubic-bezier" => {
            let mut values = [0.0_f32; 4];
            for (index, slot) in values.iter_mut().enumerate() {
                if index > 0 {
                    input.expect_comma().map_err(|_| ())?;
                }
                let value = input.expect_number().map_err(|_| ())?;
                if !value.is_finite() {
                    return Err(());
                }
                *slot = value;
            }
            if !(0.0..=1.0).contains(&values[0]) || !(0.0..=1.0).contains(&values[2]) {
                return Err(());
            }
            TimingFunction::CubicBezier(values[0], values[1], values[2], values[3])
        }
        "steps" => {
            let count = input.expect_integer().map_err(|_| ())?;
            if count < 1 {
                return Err(());
            }
            let position = if input.expect_comma().is_ok() {
                let keyword = input
                    .expect_ident()
                    .map_err(|_| ())?
                    .as_ref()
                    .to_ascii_lowercase();
                match keyword.as_str() {
                    "jump-start" | "start" => StepPosition::JumpStart,
                    "jump-end" | "end" => StepPosition::JumpEnd,
                    "jump-none" => StepPosition::JumpNone,
                    "jump-both" => StepPosition::JumpBoth,
                    _ => return Err(()),
                }
            } else {
                StepPosition::JumpEnd
            };
            let count = u32::try_from(count).map_err(|_| ())?;
            TimingFunction::Steps(count, position)
        }
        _ => return Err(()),
    };
    input.skip_whitespace();
    if input.is_exhausted() {
        Ok(function)
    } else {
        Err(())
    }
}

/// `animation-name`, and the ident carrier for `transition-property`.
#[derive(Clone, Debug, PartialEq)]
pub enum AnimationName {
    /// `none`.
    None,
    /// A `<custom-ident>` or quoted name.
    Named(Rc<str>),
}

impl AnimationName {
    /// `none | <keyframes-name>`.
    pub fn parse(input: &mut Parser<'_, '_>) -> Result<AnimationName, ()> {
        let state = input.state();
        if let Ok(quoted) = input.expect_string() {
            return Ok(AnimationName::Named(Rc::from(quoted.as_ref())));
        }
        input.reset(&state);
        let name = input.expect_ident().map_err(|_| ())?.as_ref().to_string();
        if name.eq_ignore_ascii_case("none") {
            Ok(AnimationName::None)
        } else {
            Ok(AnimationName::Named(Rc::from(name.as_str())))
        }
    }
}

/// `animation-iteration-count`.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum IterationCount {
    /// `infinite`.
    Infinite,
    /// A finite count.
    Count(f32),
}

impl IterationCount {
    /// `infinite | <number>`.
    pub fn parse(input: &mut Parser<'_, '_>) -> Result<IterationCount, ()> {
        let token = input.next().map_err(|_| ())?.clone();
        match token {
            Token::Ident(ref name) if name.eq_ignore_ascii_case("infinite") => {
                Ok(IterationCount::Infinite)
            }
            Token::Number { value, .. } if value.is_finite() && value >= 0.0 => {
                Ok(IterationCount::Count(value))
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

    use super::{AnimationName, IterationCount, StepPosition, TimingFunction};

    fn timing(text: &str) -> Option<TimingFunction> {
        parse_entirely_with(text, TimingFunction::parse).ok()
    }

    #[test]
    fn the_named_easings_are_their_cubic_beziers() {
        assert_eq!(timing("linear"), Some(TimingFunction::Linear));
        assert_eq!(timing("EASE"), Some(TimingFunction::EASE));
        assert_eq!(timing("ease-in"), Some(TimingFunction::EASE_IN));
        assert_eq!(timing("ease-out"), Some(TimingFunction::EASE_OUT));
        assert_eq!(timing("ease-in-out"), Some(TimingFunction::EASE_IN_OUT));
        assert_eq!(
            TimingFunction::EASE,
            TimingFunction::CubicBezier(0.25, 0.1, 0.25, 1.0)
        );
        assert_eq!(
            timing("step-start"),
            Some(TimingFunction::Steps(1, StepPosition::JumpStart))
        );
        assert_eq!(
            timing("step-end"),
            Some(TimingFunction::Steps(1, StepPosition::JumpEnd))
        );
        assert_eq!(
            timing("steps(4)"),
            Some(TimingFunction::Steps(4, StepPosition::JumpEnd))
        );
        assert_eq!(
            timing("steps(4, jump-both)"),
            Some(TimingFunction::Steps(4, StepPosition::JumpBoth))
        );
        assert_eq!(
            timing("steps(4, start)"),
            Some(TimingFunction::Steps(4, StepPosition::JumpStart))
        );
        assert_eq!(timing("steps(0, end)"), None);
        // A cubic-bezier's x controls must stay in 0..=1.
        assert_eq!(timing("cubic-bezier(2, 0, 0, 1)"), None);
        assert!(timing("cubic-bezier(0.4, -0.5, 0.6, 1.5)").is_some());
    }

    #[test]
    fn easing_evaluation_is_exact_at_the_endpoints_and_correct_in_between() {
        assert_eq!(TimingFunction::Linear.eval(0.0), 0.0);
        assert_eq!(TimingFunction::Linear.eval(1.0), 1.0);
        assert_eq!(TimingFunction::Linear.eval(0.5), 0.5);
        assert_eq!(TimingFunction::EASE.eval(0.0), 0.0);
        assert_eq!(TimingFunction::EASE.eval(1.0), 1.0);
        // `ease` at t = 0.5 is 0.8024 to four places (CSS Easing L1).
        let mid = TimingFunction::EASE.eval(0.5);
        assert!((mid - 0.802_4).abs() < 1e-3, "ease(0.5) == {mid}");
        // A symmetric bezier is the identity.
        let symmetric = TimingFunction::CubicBezier(0.5, 0.5, 0.5, 0.5);
        assert!((symmetric.eval(0.3) - 0.3).abs() < 1e-4);
    }

    #[test]
    fn step_functions_jump_where_their_position_says() {
        let end = TimingFunction::Steps(2, StepPosition::JumpEnd);
        assert_eq!(end.eval(0.0), 0.0);
        assert_eq!(end.eval(0.49), 0.0);
        assert_eq!(end.eval(0.51), 0.5);
        assert_eq!(end.eval(1.0), 1.0);
        let start = TimingFunction::Steps(2, StepPosition::JumpStart);
        assert_eq!(start.eval(0.0), 0.5);
        assert_eq!(start.eval(1.0), 1.0);
        let none = TimingFunction::Steps(2, StepPosition::JumpNone);
        assert_eq!(none.eval(0.0), 0.0);
        assert_eq!(none.eval(1.0), 1.0);
        let both = TimingFunction::Steps(2, StepPosition::JumpBoth);
        assert!((both.eval(0.0) - 1.0 / 3.0).abs() < 1e-6);
        assert_eq!(both.eval(1.0), 1.0);
    }

    #[test]
    fn animation_names_and_iteration_counts_parse() {
        assert_eq!(
            parse_entirely_with("none", AnimationName::parse).ok(),
            Some(AnimationName::None)
        );
        assert_eq!(
            parse_entirely_with("needs_attention", AnimationName::parse).ok(),
            Some(AnimationName::Named("needs_attention".into()))
        );
        assert_eq!(
            parse_entirely_with("\"spin\"", AnimationName::parse).ok(),
            Some(AnimationName::Named("spin".into()))
        );
        assert_eq!(
            parse_entirely_with("infinite", IterationCount::parse).ok(),
            Some(IterationCount::Infinite)
        );
        assert_eq!(
            parse_entirely_with("2.5", IterationCount::parse).ok(),
            Some(IterationCount::Count(2.5))
        );
        assert_eq!(parse_entirely_with("-1", IterationCount::parse).ok(), None);
    }

    #[test]
    fn timing_parsing_never_panics() {
        for input in crate::css::value::FUZZ_INPUTS {
            if let Ok(function) = parse_entirely_with(input, TimingFunction::parse) {
                for step in -2..=12 {
                    let _ = function.eval(step as f32 / 10.0);
                }
            }
            let _ = parse_entirely_with(input, AnimationName::parse);
            let _ = parse_entirely_with(input, IterationCount::parse);
        }
    }
}
