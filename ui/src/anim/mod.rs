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

use crate::anim::transition::{Transition, combined_ms, transitioned_props};
use crate::css::cascade::CompiledSheet;
use crate::css::computed::ComputedStyle;
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

/// Everything animating on one node.
///
/// Fed by [`restyle`](AnimationState::restyle) on every style recomputation
/// and read by [`sample`](AnimationState::sample) once per frame. It holds no
/// clock of its own: the caller passes the time in, which is what makes the
/// whole engine testable to the millisecond.
pub struct AnimationState {
    transitions: Vec<Transition>,
}

impl Default for AnimationState {
    fn default() -> Self {
        Self::new()
    }
}

impl AnimationState {
    /// Nothing running.
    #[must_use]
    pub fn new() -> AnimationState {
        AnimationState {
            transitions: Vec::new(),
        }
    }

    /// How many transitions are currently running.
    ///
    /// Observability for this part's own tests (deviation 7); nothing outside
    /// `anim` reads it.
    #[must_use]
    pub fn transition_count(&self) -> usize {
        self.transitions.len()
    }

    /// Take a new computed style.
    ///
    /// `old` is the style this node had before the change; `None` means this
    /// is the node's first style, which is not a change and therefore starts
    /// no transitions (CSS Transitions: there is no before-change style).
    ///
    /// `sheet` carries the `@keyframes` rules `animation-name` binds to; it
    /// is unused until Task 7b wires animations in, and is present now
    /// because the contract freezes this signature.
    pub fn restyle(
        &mut self,
        old: Option<&ComputedStyle>,
        new: &ComputedStyle,
        now: Duration,
        sheet: &CompiledSheet,
    ) {
        let _ = sheet;
        let now_ms = millis(now);
        self.update_transitions(old, new, now_ms);
    }

    /// CSS Transitions §3.1, applied to every longhand the new style says it
    /// transitions.
    fn update_transitions(
        &mut self,
        old: Option<&ComputedStyle>,
        new: &ComputedStyle,
        now_ms: f64,
    ) {
        let Some(old) = old else {
            self.transitions.clear();
            return;
        };
        let governed = transitioned_props(&new.transition_specs());

        // A property that dropped out of `transition-property` stops
        // transitioning immediately -- it is not left to finish.
        self.transitions.retain(|running| {
            governed
                .binary_search_by_key(&running.prop, |(p, _)| *p)
                .is_ok()
        });

        for (prop, spec) in &governed {
            let after = new.raw(*prop);
            let Some(index) = self.transitions.iter().position(|t| t.prop == *prop) else {
                // No transition running: start one if the value changed.
                if old.raw(*prop) != after {
                    self.transitions.push(Transition::start(
                        *prop,
                        old.raw(*prop).clone(),
                        after.clone(),
                        spec,
                        now_ms,
                    ));
                }
                continue;
            };

            if &self.transitions[index].to == after {
                // Already heading there: leave it alone (§3.1's "do nothing").
                continue;
            }
            let current = self.transitions[index].value_at(now_ms);
            if &current == after {
                // Already there: cancel (§3.1 step 1).
                self.transitions.remove(index);
            } else if &self.transitions[index].reversing_adjusted_start == after
                && combined_ms(spec) > 0.0
            {
                // Going back where it came from: shorten proportionally
                // (§3.1 step 3) so an interrupted hover-out returns in the
                // time it had actually spent leaving.
                let reversed = self.transitions[index].reverse(after.clone(), spec, now_ms);
                self.transitions[index] = reversed;
            } else {
                // Anything else: retarget from the value on screen right now.
                self.transitions[index] =
                    Transition::start(*prop, current, after.clone(), spec, now_ms);
            }
        }
        self.transitions.sort_by_key(|t| t.prop);
    }

    /// Every animated value for this frame.
    pub fn sample(&mut self, now: Duration) -> Overrides {
        let now_ms = millis(now);
        // A finished transition's end value *is* the computed style's value,
        // so keeping it would override the style with itself.
        self.transitions.retain(|t| !t.is_finished(now_ms));

        let mut out = Overrides::default();
        for transition in &self.transitions {
            out.set(transition.prop, transition.value_at(now_ms));
        }
        out
    }

    /// Whether another frame is needed.
    #[must_use]
    pub fn is_active(&self, now: Duration) -> bool {
        let now_ms = millis(now);
        self.transitions.iter().any(|t| !t.is_finished(now_ms))
    }

    /// The earliest instant at which [`sample`](Self::sample) could return
    /// something different; `None` when nothing is running.
    ///
    /// This is a *lower bound*, not the exact instant: an interpolating
    /// transition changes continuously, so the honest answer is `now`, and a
    /// `steps()` transition's true next change is later than that. Reporting
    /// early can only make the frame pump ask for a frame it did not strictly
    /// need; reporting late would drop frames.
    #[must_use]
    pub fn next_deadline(&self, now: Duration) -> Option<Duration> {
        if !self.is_active(now) {
            return None;
        }
        let now_ms = millis(now);
        let mut deadline = f64::INFINITY;
        for transition in &self.transitions {
            if transition.is_finished(now_ms) {
                continue;
            }
            deadline = deadline.min(if now_ms < transition.start_ms {
                transition.start_ms
            } else {
                now_ms
            });
        }
        if !deadline.is_finite() {
            return None;
        }
        Some(Duration::from_secs_f64(deadline.max(now_ms) / 1000.0))
    }
}

#[cfg(test)]
mod tests {
    use super::{AnimationState, Clock, ManualClock, Overrides, interpolate_prop};
    use crate::css::cascade::CompiledSheet;
    use crate::css::computed::{ComputedStyle, ResolveEnv};
    use crate::css::node::{Node, PseudoStates};
    use crate::css::registry::Prop;
    use crate::css::select::MatchCx;
    use crate::css::value::Value;
    use std::time::Duration;

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

    /// The computed style of a `button` inside a `window.background`, with
    /// `states` applied. Built from CSS rather than from field literals so the
    /// fixture exercises the same path the widget does.
    fn style_of(sheet: &CompiledSheet, states: PseudoStates) -> ComputedStyle {
        let window = Node::with_classes("window", &["background"]);
        let button = Node::new("button");
        window.append_child(&button);
        button.set_states(states);
        let mut cx = MatchCx::new();
        ComputedStyle::resolve_chain(sheet, &button, &ResolveEnv::default(), &mut cx)
    }

    fn number(overrides: &Overrides, prop: Prop) -> Option<f32> {
        match overrides.get(prop) {
            Some(Value::Number(v)) => Some(*v),
            Some(other) => panic!("{prop:?} sampled as {other:?}, expected a number"),
            None => None,
        }
    }

    const FADE_ON_HOVER: &str = "\
window { background-color: rgb(255 255 255); }
button { opacity: 1; color: rgb(0 0 0); transition: opacity 200ms linear; }
button:hover { opacity: 0; color: rgb(255 0 0); }
";

    // THE GATE (spec §7: "0/100/200 ms linear midpoint").
    // Mutation check: start transitions from `new` instead of `old` in
    // `update_transitions` and the 100 ms sample becomes 0.0.
    #[test]
    fn a_hover_change_transitions_from_the_old_value_to_the_new_one() {
        let sheet = CompiledSheet::compile(FADE_ON_HOVER);
        let normal = style_of(&sheet, PseudoStates::default());
        let hover = style_of(&sheet, PseudoStates::HOVER);
        assert_eq!(normal.raw(Prop::Opacity), &Value::Number(1.0));
        assert_eq!(hover.raw(Prop::Opacity), &Value::Number(0.0));

        let clock = ManualClock::new();
        let mut state = AnimationState::new();
        state.restyle(None, &normal, clock.now(), &sheet);
        assert!(
            state.sample(clock.now()).is_empty(),
            "the first style is not a change, so nothing transitions"
        );

        state.restyle(Some(&normal), &hover, clock.now(), &sheet);
        assert_eq!(state.transition_count(), 1);

        assert_eq!(
            number(&state.sample(Duration::from_millis(0)), Prop::Opacity),
            Some(1.0)
        );
        assert_eq!(
            number(&state.sample(Duration::from_millis(100)), Prop::Opacity),
            Some(0.5)
        );
        assert!(state.is_active(Duration::from_millis(100)));

        let finished = state.sample(Duration::from_millis(200));
        assert!(
            finished.is_empty(),
            "at the end the transition is dropped: the computed style already \
             carries the new value, so an override would be redundant"
        );
        assert!(!state.is_active(Duration::from_millis(200)));
        assert_eq!(state.transition_count(), 0);
    }

    // Mutation check: expand every spec as `all` and `color` -- which changes
    // on hover but is not in `transition-property` -- starts appearing in the
    // overrides.
    #[test]
    fn a_property_the_style_does_not_transition_is_never_overridden() {
        let sheet = CompiledSheet::compile(FADE_ON_HOVER);
        let normal = style_of(&sheet, PseudoStates::default());
        let hover = style_of(&sheet, PseudoStates::HOVER);
        let mut state = AnimationState::new();
        state.restyle(None, &normal, Duration::ZERO, &sheet);
        state.restyle(Some(&normal), &hover, Duration::ZERO, &sheet);
        let sampled = state.sample(Duration::from_millis(100));
        assert!(sampled.get(Prop::Color).is_none());
        assert!(sampled.get(Prop::Opacity).is_some());
    }

    // Mutation check: return `Some(now)` unconditionally from `next_deadline`
    // and the idle case stops returning None, so the frame pump never stops.
    #[test]
    fn next_deadline_is_none_only_when_nothing_is_running() {
        let sheet = CompiledSheet::compile(FADE_ON_HOVER);
        let normal = style_of(&sheet, PseudoStates::default());
        let hover = style_of(&sheet, PseudoStates::HOVER);
        let mut state = AnimationState::new();
        state.restyle(None, &normal, Duration::ZERO, &sheet);
        assert_eq!(state.next_deadline(Duration::ZERO), None);

        state.restyle(Some(&normal), &hover, Duration::ZERO, &sheet);
        assert_eq!(
            state.next_deadline(Duration::from_millis(100)),
            Some(Duration::from_millis(100)),
            "a continuously interpolating transition changes on the very next \
             frame, so the deadline is now"
        );
        let _ = state.sample(Duration::from_millis(200));
        assert_eq!(state.next_deadline(Duration::from_millis(200)), None);
    }
}
