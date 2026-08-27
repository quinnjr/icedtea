//! Transitions and animations.
//!
//! P3 lands only the three types the computed style *produces*: the per-node
//! [`Overrides`] a sample is applied through, and the flattened
//! [`TransitionSpec`]/[`AnimationSpec`] the engine is driven by. The clock, the
//! transition/animation state machines and `AnimationState` are P5's.

use crate::css::registry::Prop;
use crate::css::value::{AnimationName, IterationCount, Keyword, Time, TimingFunction, Value};

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
