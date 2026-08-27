//! The animation clock.
//!
//! Every time value in `anim` comes through a [`Clock`]. Production code
//! uses [`MonotonicClock`]; tests use [`ManualClock`], which is the only
//! reason animation behaviour is assertable to the millisecond instead of
//! being sampled against a wall clock and hedged with tolerances.

use std::cell::Cell;
use std::time::{Duration, Instant};

/// The one source of time for transitions and animations.
///
/// `now()` is an offset from an implementation-defined epoch, not a wall
/// clock: only differences between two readings are meaningful.
pub trait Clock {
    /// Time elapsed since this clock's epoch.
    fn now(&self) -> Duration;
}

/// The production clock: monotonic time since the clock was created.
///
/// Driven forward by `wl_surface.frame` callbacks (see `wayland.rs`), so
/// nothing polls it in a loop.
pub struct MonotonicClock(Instant);

impl MonotonicClock {
    /// A clock whose epoch is now.
    #[must_use]
    pub fn new() -> Self {
        Self(Instant::now())
    }
}

impl Default for MonotonicClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for MonotonicClock {
    fn now(&self) -> Duration {
        self.0.elapsed()
    }
}

/// The test clock: time moves only when a test moves it.
///
/// `Cell` rather than a `&mut` API so a `ManualClock` can be shared through
/// an `Rc<dyn Clock>` with the widget under test and still be advanced from
/// the test body.
pub struct ManualClock(Cell<Duration>);

impl ManualClock {
    /// A clock reading zero.
    #[must_use]
    pub fn new() -> Self {
        Self(Cell::new(Duration::ZERO))
    }

    /// Move the clock forward by `ms` milliseconds.
    pub fn advance_ms(&self, ms: u64) {
        self.0.set(self.0.get() + Duration::from_millis(ms));
    }

    /// Move the clock to exactly `ms` milliseconds past its epoch.
    pub fn set_ms(&self, ms: u64) {
        self.0.set(Duration::from_millis(ms));
    }
}

impl Default for ManualClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Duration {
        self.0.get()
    }
}

#[cfg(test)]
mod tests {
    use super::{Clock, ManualClock, MonotonicClock};
    use std::time::Duration;

    // Mutation check: make `advance_ms` assign instead of add
    // (`self.0.set(Duration::from_millis(ms))`) and the third assertion fails.
    #[test]
    fn a_manual_clock_starts_at_zero_and_only_moves_when_told_to() {
        let clock = ManualClock::new();
        assert_eq!(clock.now(), Duration::ZERO);
        clock.advance_ms(100);
        assert_eq!(clock.now(), Duration::from_millis(100));
        clock.advance_ms(150);
        assert_eq!(clock.now(), Duration::from_millis(250));
        clock.set_ms(20);
        assert_eq!(clock.now(), Duration::from_millis(20));
    }

    // Mutation check: have `MonotonicClock::now` return `Duration::ZERO` and
    // the "later reading is not before the earlier one" assertion still holds
    // but the "starts near zero" one fails only if the epoch is wrong -- so
    // the load-bearing half here is that `now()` is measured from `new()`,
    // not from the process start or the Unix epoch.
    #[test]
    fn a_monotonic_clock_is_measured_from_its_own_creation_and_never_goes_back() {
        let clock = MonotonicClock::new();
        let first = clock.now();
        assert!(
            first < Duration::from_secs(1),
            "a freshly created MonotonicClock read {first:?}, so it is not \
             measuring from its own creation"
        );
        let second = clock.now();
        assert!(second >= first, "{second:?} is before {first:?}");
    }

    // Mutation check: drop the `dyn Clock` object-safety (e.g. add a generic
    // method to the trait) and this stops compiling -- which is the point:
    // `Button` and `AppState` both store an `Rc<dyn Clock>`.
    #[test]
    fn a_clock_is_usable_as_a_trait_object() {
        let clock: std::rc::Rc<dyn Clock> = std::rc::Rc::new(ManualClock::new());
        assert_eq!(clock.now(), Duration::ZERO);
    }
}
