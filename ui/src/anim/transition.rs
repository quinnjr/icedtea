//! One running property transition, and the rule that decides which
//! longhands a style's `transition-*` declarations govern.

use crate::anim::{TransitionSpec, interpolate_prop};
use crate::css::registry::Prop;
use crate::css::value::Value;
use crate::css::value::timing::TimingFunction;

/// `spec`'s `transition-duration` in milliseconds, clamped non-negative and
/// finite. A `calc()` can produce NaN; NaN durations are treated as zero so
/// the transition completes instantly rather than never.
#[must_use]
pub fn duration_ms_of(spec: &TransitionSpec) -> f64 {
    // Multiply in f32 before widening: `Time` is stored in seconds as f32
    // (e.g. `Time(0.2)` from a literal 200 ms), and `seconds * 1000.0` done
    // in f32 round-trips exactly for the values the CSS parser and tests
    // construct, whereas widening to f64 first then multiplying exposes the
    // f32 rounding of the stored seconds value as sub-millisecond noise
    // (200.00000298023224 instead of 200.0).
    let ms = f64::from(spec.duration.as_secs_f32() * 1000.0);
    if ms.is_finite() { ms.max(0.0) } else { 0.0 }
}

/// `spec`'s `transition-delay` in milliseconds. May be negative (CSS allows
/// it: the transition starts already part-way through). NaN becomes zero.
#[must_use]
pub fn delay_ms_of(spec: &TransitionSpec) -> f64 {
    // Same f32-then-widen ordering as `duration_ms_of`, and for the same
    // reason.
    let ms = f64::from(spec.delay.as_secs_f32() * 1000.0);
    if ms.is_finite() { ms } else { 0.0 }
}

/// CSS Transitions' "combined duration": duration plus delay.
#[must_use]
pub fn combined_ms(spec: &TransitionSpec) -> f64 {
    duration_ms_of(spec) + delay_ms_of(spec)
}

/// One running transition of one longhand.
///
/// `reversing_adjusted_start` and `reversing_shortening_factor` are CSS
/// Transitions §3.1's reversal bookkeeping: they are what makes interrupting
/// a half-finished hover-out and going back to hover-in take half the time
/// rather than the full duration.
#[derive(Clone, Debug)]
pub struct Transition {
    /// The longhand being transitioned.
    pub prop: Prop,
    /// The value at `start_ms`.
    pub from: Value,
    /// The value at `end_ms()`.
    pub to: Value,
    /// The value a *later* change must equal for this to count as a reversal.
    pub reversing_adjusted_start: Value,
    /// How much of the specified duration this transition actually runs for.
    pub reversing_shortening_factor: f32,
    /// Clock milliseconds at which interpolation begins. May be in the past
    /// (a negative `transition-delay`).
    pub start_ms: f64,
    /// How long interpolation runs, in milliseconds. Never negative.
    pub duration_ms: f64,
    /// The easing applied to linear progress.
    pub timing: TimingFunction,
}

impl Transition {
    /// Start a fresh transition of `prop` from `from` to `to` at `now_ms`.
    #[must_use]
    pub fn start(
        prop: Prop,
        from: Value,
        to: Value,
        spec: &TransitionSpec,
        now_ms: f64,
    ) -> Transition {
        Transition {
            prop,
            reversing_adjusted_start: from.clone(),
            from,
            to,
            reversing_shortening_factor: 1.0,
            start_ms: now_ms + delay_ms_of(spec),
            duration_ms: duration_ms_of(spec),
            timing: spec.timing,
        }
    }

    /// Linear progress through the transition, clamped to `0..=1`.
    ///
    /// Before `start_ms` (i.e. during a positive delay) this is `0.0`, which
    /// is CSS's rule: the property holds its before-change value throughout
    /// the delay.
    #[must_use]
    pub fn progress(&self, now_ms: f64) -> f32 {
        // NaN must take this branch rather than fall through to a division
        // that yields NaN, so this is written as a negated `>` rather than
        // `<=` on purpose.
        #[allow(clippy::neg_cmp_op_on_partial_ord)]
        if !(self.duration_ms > 0.0) {
            // Zero (or NaN) duration: complete the instant it starts.
            return if now_ms >= self.start_ms { 1.0 } else { 0.0 };
        }
        let fraction = (now_ms - self.start_ms) / self.duration_ms;
        if fraction.is_nan() {
            return 0.0;
        }
        fraction.clamp(0.0, 1.0) as f32
    }

    /// Eased progress. May leave `0..=1` for an overshooting `cubic-bezier`.
    #[must_use]
    pub fn eased(&self, now_ms: f64) -> f32 {
        self.timing.eval(self.progress(now_ms))
    }

    /// This transition's contribution to the frame at `now_ms`.
    #[must_use]
    pub fn value_at(&self, now_ms: f64) -> Value {
        interpolate_prop(self.prop, &self.from, &self.to, self.eased(now_ms))
    }

    /// Clock milliseconds at which this transition reaches `to`.
    #[must_use]
    pub fn end_ms(&self) -> f64 {
        self.start_ms + self.duration_ms
    }

    /// Whether this transition has reached `to` and can be dropped.
    #[must_use]
    pub fn is_finished(&self, now_ms: f64) -> bool {
        now_ms >= self.end_ms()
    }
}

#[cfg(test)]
mod tests {
    use super::{Transition, combined_ms, delay_ms_of, duration_ms_of};
    use crate::anim::TransitionSpec;
    use crate::css::registry::Prop;
    use crate::css::value::Value;
    use crate::css::value::timing::{StepPosition, Time, TimingFunction};

    fn spec(duration_ms: f32, delay_ms: f32, timing: TimingFunction) -> TransitionSpec {
        TransitionSpec {
            prop: Some(Prop::Opacity),
            all: false,
            duration: Time(duration_ms / 1000.0),
            delay: Time(delay_ms / 1000.0),
            timing,
        }
    }

    fn opacity_transition(duration_ms: f32, timing: TimingFunction) -> Transition {
        Transition::start(
            Prop::Opacity,
            Value::Number(0.0),
            Value::Number(1.0),
            &spec(duration_ms, 0.0, timing),
            0.0,
        )
    }

    // THE GATE (spec §7 "0/100/200 ms linear midpoint").
    // Mutation check: change `progress` to divide by `end_ms()` instead of
    // `duration_ms` and the 100 ms sample becomes 0.5 of a 200 ms window
    // measured from zero -- identical here -- so instead mutate the clamp to
    // `.clamp(0.0, 2.0)` and the 300 ms sample stops being exactly 1.0.
    #[test]
    fn a_linear_transition_is_exactly_half_way_at_half_its_duration() {
        let t = opacity_transition(200.0, TimingFunction::Linear);
        assert_eq!(t.value_at(0.0), Value::Number(0.0));
        assert_eq!(t.value_at(100.0), Value::Number(0.5));
        assert_eq!(t.value_at(200.0), Value::Number(1.0));
        assert_eq!(
            t.value_at(300.0),
            Value::Number(1.0),
            "past the end a transition holds its end value, it does not overshoot"
        );
        assert!(!t.is_finished(199.0));
        assert!(t.is_finished(200.0));
    }

    // THE GATE (spec §7 "`ease` at t=0.5").
    // `ease` is cubic-bezier(0.25, 0.1, 0.25, 1.0). Solving x(u) = 0.5 gives
    // u = 0.5 by the curve's symmetry in x about u = 0.5
    // (x(u) = 0.75u - 0.75u^2 + ... is not symmetric in general, so the value
    // is solved numerically): y(0.5) = 0.8024 to four places, the number every
    // browser reports for `ease` at half its duration.
    // Mutation check: swap EASE's control points to (0.42, 0, 0.58, 1)
    // (`ease-in-out`) and the sample becomes 0.5, far outside the window.
    #[test]
    fn ease_at_half_its_duration_is_the_bezier_value_not_the_midpoint() {
        let t = opacity_transition(200.0, TimingFunction::EASE);
        let Value::Number(v) = t.value_at(100.0) else {
            panic!("opacity interpolates to a number");
        };
        assert!(
            (0.79..0.81).contains(&v),
            "`ease` at t = 0.5 is 0.8024, got {v}: the timing function is not \
             being solved on x"
        );
        assert_eq!(t.value_at(0.0), Value::Number(0.0), "exact at t = 0");
        assert_eq!(t.value_at(200.0), Value::Number(1.0), "exact at t = 1");
    }

    // Mutation check: make `Steps` round instead of floor and the 0.49
    // sample jumps to 0.5 instead of staying at 0.0.
    #[test]
    fn steps_hold_their_value_between_jumps() {
        let t = opacity_transition(200.0, TimingFunction::Steps(2, StepPosition::JumpEnd));
        assert_eq!(t.value_at(0.0), Value::Number(0.0));
        assert_eq!(t.value_at(98.0), Value::Number(0.0));
        assert_eq!(t.value_at(100.0), Value::Number(0.5));
        assert_eq!(t.value_at(198.0), Value::Number(0.5));
        assert_eq!(t.value_at(200.0), Value::Number(1.0));
    }

    // Mutation check: drop the `.max(0.0)` in `duration_ms_of` and a negative
    // duration makes `progress` return a negative fraction rather than 1.0.
    #[test]
    fn spec_times_convert_to_milliseconds_and_a_negative_duration_is_zero() {
        assert_eq!(
            duration_ms_of(&spec(200.0, 0.0, TimingFunction::Linear)),
            200.0
        );
        assert_eq!(
            delay_ms_of(&spec(200.0, -50.0, TimingFunction::Linear)),
            -50.0
        );
        assert_eq!(
            combined_ms(&spec(200.0, -50.0, TimingFunction::Linear)),
            150.0
        );
        assert_eq!(
            duration_ms_of(&spec(-200.0, 0.0, TimingFunction::Linear)),
            0.0,
            "a negative transition-duration is clamped to zero, not run backwards"
        );
    }

    // Mutation check: initialise `reversing_adjusted_start` from `to` instead
    // of `from` in `start`, and Task 8's reversal test starts reversing on the
    // wrong value -- this assertion is the local tripwire for that.
    #[test]
    fn a_fresh_transition_records_its_start_value_as_the_reversing_anchor() {
        let t = opacity_transition(200.0, TimingFunction::Linear);
        assert_eq!(t.reversing_adjusted_start, Value::Number(0.0));
        assert_eq!(t.reversing_shortening_factor, 1.0);
        assert_eq!(t.start_ms, 0.0);
        assert_eq!(t.end_ms(), 200.0);
    }
}
