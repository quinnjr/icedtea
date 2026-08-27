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

    /// CSS Transitions §3.1 step 3: this transition is being sent back to the
    /// value it started from, so the new run is shortened in proportion to
    /// how far it actually got.
    ///
    /// Without this, tapping in and out of `:hover` faster than the duration
    /// makes each reversal take the full time from wherever it happened to
    /// be, which reads as lag that compounds.
    #[must_use]
    pub fn reverse(&self, to: Value, spec: &TransitionSpec, now_ms: f64) -> Transition {
        let eased = self.eased(now_ms);
        let old_factor = self.reversing_shortening_factor;
        let factor = {
            let raw = eased.mul_add(old_factor, 1.0 - old_factor).abs();
            if raw.is_finite() {
                raw.clamp(0.0, 1.0)
            } else {
                1.0
            }
        };
        let delay = delay_ms_of(spec);
        // A negative delay is scaled with the run; a positive one is not.
        let delay = if delay >= 0.0 {
            delay
        } else {
            delay * f64::from(factor)
        };
        Transition {
            prop: self.prop,
            from: self.value_at(now_ms),
            reversing_adjusted_start: self.to.clone(),
            to,
            reversing_shortening_factor: factor,
            start_ms: now_ms + delay,
            duration_ms: duration_ms_of(spec) * f64::from(factor),
            timing: spec.timing,
        }
    }
}

/// Every longhand the computed `transition-*` declarations govern, each
/// paired with the spec that applies to it, sorted by `Prop`.
///
/// Three expansions happen here, in this order of precedence (later specs
/// overwrite earlier ones, per CSS Transitions' "if a property is listed more
/// than once, the last entry wins"):
///
/// * `all` becomes every animatable longhand — never every longhand, so a
///   `transition: all` does not put a 100 ms discrete flip on `font-family`;
/// * a shorthand `Prop` becomes its longhands (Adwaita's base `button` rule
///   lists `outline`, a shorthand, alongside three of its own longhands);
/// * `prop: None` with `all: false` — `transition-property: none`, or an
///   ident the registry does not know — governs nothing.
#[must_use]
pub fn transitioned_props(specs: &[TransitionSpec]) -> Vec<(Prop, TransitionSpec)> {
    fn put(table: &mut Vec<(Prop, TransitionSpec)>, prop: Prop, spec: &TransitionSpec) {
        match table.binary_search_by_key(&prop, |(p, _)| *p) {
            Ok(index) => table[index].1 = spec.clone(),
            Err(index) => table.insert(index, (prop, spec.clone())),
        }
    }

    let mut table: Vec<(Prop, TransitionSpec)> = Vec::new();
    for spec in specs {
        if spec.all {
            for prop in crate::css::registry::animatable_longhands() {
                put(&mut table, prop, spec);
            }
        } else if let Some(prop) = spec.prop {
            if prop.is_longhand() {
                put(&mut table, prop, spec);
            } else {
                for &longhand in prop.longhands() {
                    put(&mut table, longhand, spec);
                }
            }
        }
    }
    table
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

    fn named(prop: Option<Prop>, all: bool, duration_ms: f32) -> TransitionSpec {
        TransitionSpec {
            prop,
            all,
            duration: Time(duration_ms / 1000.0),
            delay: Time(0.0),
            timing: TimingFunction::Linear,
        }
    }

    fn governed(specs: &[TransitionSpec]) -> Vec<Prop> {
        super::transitioned_props(specs)
            .into_iter()
            .map(|(prop, _)| prop)
            .collect()
    }

    // Mutation check: expand `all` through `registry::longhands()` instead of
    // `animatable_longhands()` and FontFamily (non-animatable) appears in the
    // list, failing the second assertion.
    #[test]
    fn transition_property_all_expands_to_every_animatable_longhand() {
        let props = governed(&[named(None, true, 200.0)]);
        assert!(props.contains(&Prop::BackgroundColor));
        assert!(
            !props.contains(&Prop::FontFamily),
            "`all` must not pick up a non-animatable longhand"
        );
        let expected: Vec<Prop> = crate::css::registry::animatable_longhands().collect();
        assert_eq!(props.len(), expected.len());
    }

    // Mutation check: drop the `is_longhand` branch so shorthands are pushed
    // verbatim, and Prop::Outline (a shorthand) appears where its four
    // longhands should -- the assertion on OutlineWidth then fails.
    #[test]
    fn a_shorthand_in_transition_property_expands_to_its_longhands() {
        let props = governed(&[named(Some(Prop::Outline), false, 300.0)]);
        for expected in Prop::Outline.longhands() {
            assert!(
                props.contains(expected),
                "{expected:?} is one of `outline`'s longhands and must be transitioned"
            );
        }
        assert!(
            !props.contains(&Prop::Outline),
            "the shorthand itself is never a transitioned property"
        );
        assert!(props.contains(&Prop::OutlineWidth));
    }

    // Mutation check: make `transitioned_props` insert-if-absent instead of
    // insert-or-replace and the winning duration stays 200 ms.
    #[test]
    fn the_last_spec_naming_a_property_wins() {
        let specs = vec![
            named(None, true, 200.0),
            named(Some(Prop::Opacity), false, 50.0),
        ];
        let table = super::transitioned_props(&specs);
        let (_, spec) = table
            .iter()
            .find(|(prop, _)| *prop == Prop::Opacity)
            .expect("opacity is governed");
        assert_eq!(
            duration_ms_of(spec),
            50.0,
            "the later, more specific spec must win for opacity"
        );
        let (_, other) = table
            .iter()
            .find(|(prop, _)| *prop == Prop::BackgroundColor)
            .expect("background-color is still governed by `all`");
        assert_eq!(duration_ms_of(other), 200.0);
    }

    // Mutation check: return every spec's props even when `prop` is None and
    // `all` is false, and this returns a non-empty list.
    #[test]
    fn transition_property_none_governs_nothing() {
        assert!(governed(&[named(None, false, 200.0)]).is_empty());
        assert!(governed(&[]).is_empty());
    }

    // Mutation check: drop the final sort and the pairs come back in spec
    // order, failing the sorted-window assertion.
    #[test]
    fn the_governed_table_is_sorted_by_prop_so_lookups_can_binary_search() {
        let table = super::transitioned_props(&[named(None, true, 200.0)]);
        assert!(
            table.windows(2).all(|w| w[0].0 < w[1].0),
            "transitioned_props must return a strictly ascending Prop table"
        );
    }
}
