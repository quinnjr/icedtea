//! Transitions, `@keyframes` animations, and the clock that drives them.
//!
//! The engine is deliberately property-agnostic: it never knows what
//! `background-color` *is*. It knows that a [`Prop`] changed, that the
//! registry row for that `Prop` carries an interpolator, and that a
//! [`Clock`] says how far along the change is. Everything property-specific
//! lives in `css::registry` and `css::value::interpolate`.
//!
//! Output is an [`Overrides`] table layered over the computed style by
//! `ComputedStyle::with_overrides` before layout and paint. Precedence is
//! CSS's: animation over transition over the base cascade.

pub mod clock;
pub mod keyframes;
pub mod transition;

use std::time::Duration;

pub use clock::{Clock, ManualClock, MonotonicClock};

use crate::css::registry::Prop;
use crate::css::value::{AnimationName, IterationCount, Keyword, Time, TimingFunction, Value};

/// A `Duration` as milliseconds.
///
/// The engine works in `f64` milliseconds internally rather than `Duration`
/// because a negative `animation-delay`/`transition-delay` puts a start time
/// *before* the clock's epoch, which `Duration` cannot represent.
#[must_use]
pub fn millis(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// Per-node animation output, sorted by [`Prop`], applied over the computed
/// style.
///
/// One slot per property: a later [`set`](Self::set) of the same property
/// replaces the earlier one, which is exactly the precedence rule (animation
/// overrides transition overrides the base cascade) expressed as write order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Overrides {
    entries: Vec<(Prop, Value)>,
}

impl Overrides {
    /// Whether nothing is being animated.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// How many longhands are being overridden this frame.
    ///
    /// Additive beyond contract §6 (deviation 7): observability for this
    /// part's own tests — nothing outside `anim` reads it.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// The overriding value for `prop`, if any.
    #[must_use]
    pub fn get(&self, prop: Prop) -> Option<&Value> {
        self.entries
            .binary_search_by_key(&prop, |(entry, _)| *entry)
            .ok()
            .map(|index| &self.entries[index].1)
    }

    /// Every override, in `Prop` order.
    pub fn iter(&self) -> impl Iterator<Item = (Prop, &Value)> {
        self.entries.iter().map(|(prop, value)| (*prop, value))
    }

    /// Set `prop`'s overriding value, replacing any earlier one.
    pub fn set(&mut self, prop: Prop, value: Value) {
        match self
            .entries
            .binary_search_by_key(&prop, |(entry, _)| *entry)
        {
            Ok(index) => self.entries[index].1 = value,
            Err(index) => self.entries.insert(index, (prop, value)),
        }
    }
}

/// One `transition-*` list entry, flattened from the four longhands.
#[derive(Clone, Debug, PartialEq)]
pub struct TransitionSpec {
    /// The property this entry transitions, if it names one.
    pub prop: Option<Prop>,
    /// Whether this entry is `transition-property: all`.
    pub all: bool,
    /// `transition-duration`.
    pub duration: Time,
    /// `transition-delay`.
    pub delay: Time,
    /// `transition-timing-function`.
    pub timing: TimingFunction,
}

/// One `animation-*` list entry, flattened from the eight longhands.
#[derive(Clone, Debug, PartialEq)]
pub struct AnimationSpec {
    /// `animation-name`.
    pub name: AnimationName,
    /// `animation-duration`.
    pub duration: Time,
    /// `animation-delay`.
    pub delay: Time,
    /// `animation-timing-function`.
    pub timing: TimingFunction,
    /// `animation-iteration-count`.
    pub iterations: IterationCount,
    /// `animation-direction`: `normal`/`reverse`/`alternate`/`alternate-reverse`.
    pub direction: Keyword,
    /// `animation-fill-mode`: `none`/`forwards`/`backwards`/`both`.
    pub fill: Keyword,
    /// `animation-play-state`: `running`/`paused`.
    pub play_state: Keyword,
}

/// Interpolate two computed values of `prop` at progress `t`.
///
/// The interpolator is always the registry row's — never hand-written per
/// property here. A property with no interpolator (`animatable: None`)
/// switches discretely at `t >= 0.5`, which is what the contract asks for
/// when a non-animatable property is named explicitly in
/// `transition-property`.
#[must_use]
pub fn interpolate_prop(prop: Prop, a: &Value, b: &Value, t: f32) -> Value {
    match prop.interpolator() {
        Some(interpolate) => interpolate(a, b, t),
        None => crate::css::value::interpolate::discrete(a, b, t),
    }
}

#[cfg(test)]
mod tests {
    use super::{Overrides, interpolate_prop};
    use crate::css::registry::Prop;
    use crate::css::value::Value;

    // Mutation check: make `set` push instead of doing a sorted
    // insert-or-replace and `overrides_are_sorted_by_prop_and_set_replaces`
    // fails on both the ordering and the length assertion.
    #[test]
    fn overrides_are_sorted_by_prop_and_set_replaces() {
        let mut o = Overrides::default();
        assert!(o.is_empty());

        o.set(Prop::Opacity, Value::Number(0.5));
        o.set(Prop::Color, Value::Number(1.0));
        o.set(Prop::Opacity, Value::Number(0.25));

        assert_eq!(o.len(), 2, "setting Opacity twice must replace, not append");
        assert_eq!(o.get(Prop::Opacity), Some(&Value::Number(0.25)));
        assert_eq!(o.get(Prop::Color), Some(&Value::Number(1.0)));
        assert_eq!(o.get(Prop::Filter), None);

        let props: Vec<Prop> = o.iter().map(|(p, _)| p).collect();
        assert_eq!(
            props,
            vec![Prop::Color, Prop::Opacity],
            "iteration order must be registry order (Color = 0 precedes Opacity = 1)"
        );
    }

    // Mutation check: make `interpolate_prop` ignore `prop.interpolator()`
    // and always call `discrete`, and the midpoint assertion returns 0.0
    // (a's value at t < 0.5) instead of 0.5.
    #[test]
    fn interpolate_prop_uses_the_registry_row_and_falls_back_to_discrete() {
        // `opacity` is animatable: a real number interpolation.
        let mid = interpolate_prop(Prop::Opacity, &Value::Number(0.0), &Value::Number(1.0), 0.5);
        assert_eq!(mid, Value::Number(0.5));

        // `font-family` is not animatable: discrete, so it flips at t >= 0.5.
        let a = Value::Number(0.0);
        let b = Value::Number(1.0);
        assert_eq!(
            interpolate_prop(Prop::FontFamily, &a, &b, 0.49),
            Value::Number(0.0)
        );
        assert_eq!(
            interpolate_prop(Prop::FontFamily, &a, &b, 0.5),
            Value::Number(1.0)
        );
        assert!(
            Prop::FontFamily.interpolator().is_none(),
            "this test's premise is that font-family has no registry interpolator"
        );
    }
}
