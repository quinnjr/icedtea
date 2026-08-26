# Pure-Rust GTK-themed UI — M2 Part 5: Transitions, keyframe animations, clock, interpolation — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `ui/src/anim/` — a deterministic, clock-driven animation engine that turns CSS `transition-*` and `animation-*` declarations plus `@keyframes` into per-node `Overrides` layered over the computed style, and wire it into the button widget and the Wayland frame-callback pump.

**Architecture:** A `Clock` trait (`MonotonicClock` in production, `ManualClock` in tests) supplies the only notion of time. `AnimationState::restyle(old, new, now, sheet)` diffs two `ComputedStyle`s over the properties named by the new style's `transition-*` longhands and starts/retargets/cancels `Transition`s with CSS Transitions §3.1 reversal continuity; it also binds `animation-name` to the compiled sheet's `@keyframes` as `ActiveAnimation`s. `AnimationState::sample(now)` produces an `Overrides` table — transitions first, animations second, so animations win. Interpolation is never hand-written per property: it is always the registry row's `Prop::interpolator()`, falling back to `interpolate::discrete`. Nothing in this part paints; it produces `Overrides` and nothing more.

**Tech Stack:** Rust edition 2024 (rust-version 1.94), `cssparser` 0.37, `selectors` 0.40, `taffy` 0.14, `skia-rs-safe` 0.4.0, `wayland-client` 0.31, `bitflags` 2, `fontconfig` 0.11, `tracing`. No new dependency is introduced by this part.

**Spec:** `docs/superpowers/specs/2026-08-26-pure-rust-gtk-m2-css-engine-design.md` (§5 Transitions & animations; §7 Testing strategy).
**Contract:** `docs/superpowers/plans/2026-08-26-m2-part0-contract.md` (§6 Animation is this part's normative interface; §11 P5 names the boundaries).
**Parent spec:** `docs/superpowers/specs/2026-08-20-pure-rust-gtk-ui-design.md`.
**Research notes:** `.superpowers/m2-plan-notes/gtk-css-semantics.md` §10 (transition/animation defaults, `@keyframes` grammar), §11 (timing function names); `.superpowers/m2-plan-notes/current-crate.md` (`wayland.rs`, `widget/button.rs`, `tests/` inventory).

---

## Contract deviations

Each line is a place where the frozen contract does not, on its own, let this
part compile — stated up front rather than diverged from silently. Nothing
below changes a signature the contract does freeze.

1. **`ui/src/anim/mod.rs` must exist before P3 lands, not P5.** The contract
   puts `TransitionSpec`/`AnimationSpec` in `§6 anim [P5]`, but
   `ComputedStyle::transition_specs()`/`animation_specs()` (§5, P3) return
   `Vec<TransitionSpec>`/`Vec<AnimationSpec>`, so P3 cannot compile without
   them. **Ruling:** whichever part runs first creates
   `ui/src/anim/mod.rs` + `pub mod anim;` in `ui/src/lib.rs` with the two
   spec structs **exactly as the contract prints them**. Task 1 below is
   written to be idempotent: if the file already declares them, it keeps
   those declarations byte-for-byte and adds only the missing items.
2. **The contract does not freeze `Button`'s or `AppState`'s post-P3 API**,
   yet P5 is told to add "the `Overrides` hand-off in `widget/button.rs` /
   `app.rs`". Tasks 13 and 14 therefore specify **only the additions** —
   new fields and new methods whose names and signatures are fixed here —
   and touch existing code at exactly two call sites (the end of
   `Button::restyle`, and the `overrides` argument `Button::render` passes to
   `paint::paint_node`). If P3/P4 named those differently, adapt the call
   site only; never the `anim` API.
3. **`app.rs` needs no change.** The hand-off is entirely inside `Button`
   (which owns the `AnimationState`) and `LayerWindow`/`AppState` (which owns
   the frame pump). `app.rs`'s `themed_button`/`run_themed_button` pass a
   `Button` through unchanged. This part therefore does not modify `app.rs`,
   contrary to §11's "and `app.rs`".
4. **`AnimationState::next_deadline` returns a conservative lower bound.**
   The contract says "earliest instant `sample` would change". For an output
   that changes continuously (any running interpolation) the true answer is
   "now"; for a `steps()` transition the true answer is the next step
   boundary. This part returns `now` in both cases — never later than the
   true instant, so the frame pump can only over-request, never under-request.
   Documented on the method.
5. **`transition-property` may name a shorthand.** Adwaita's base `button`
   rule says `transition-property: outline, outline-width, outline-offset,
   outline-color`, and `outline` is a shorthand. The contract's
   `TransitionSpec { prop: Option<Prop>, .. }` does not say what a shorthand
   there means. **Ruling:** `transitioned_props` expands a shorthand `Prop`
   through `Prop::longhands()`; `prop: None, all: false` (i.e. `none`, or an
   unknown ident P3 could not resolve) contributes no properties.
6. **Zero-duration transitions are constructed, per the contract**, rather
   than gated behind CSS Transitions' "combined duration > 0" start
   condition. They complete on the frame they are created, and `sample`
   prunes them; the observable result is identical to CSS's, and the contract
   asks for the construction explicitly.
7. **Three `pub` items in `anim` are additive beyond contract §6.** The
   contract's preamble permits unrestricted additions only for *private*
   items, and §6 freezes `Overrides` at `is_empty`/`get`/`iter`/`set` and
   `AnimationState` at `new`/`restyle`/`sample`/`is_active`/`next_deadline`.
   This part needs three more, all read only by this part's own tests:
   `Overrides::len`, `AnimationState::transition_count`, and
   `AnimationState::animation_count`. **Ruling:** they are kept, because a
   test module in the same crate is the only consumer and a `pub(crate)`
   spelling would trip `dead_code` in the non-test build that
   `--all-targets` clippy also compiles; each carries a doc line saying it is
   test observability and that nothing outside `anim` reads it, and each is
   listed in the §3 type-consistency table below as an addition rather than
   folded in silently. A fourth addition an earlier draft carried,
   `Overrides::clear`, had **no** caller anywhere in the plan and is deleted
   rather than kept as unused surface: `Overrides::default()` is how every
   frame starts a fresh table.

---

## Global Constraints

Every task's requirements implicitly include this section.

- **Crate:** `ui/` (`icedtea-ui`) only. Branch `rebuild/pure-rust-gtk-m2`.
- **Crate pins (do not change, do not add):** `cssparser = "0.37"`,
  `selectors = "0.40"`, `taffy = "0.14"`, `skia-rs-safe = "0.4.0"`,
  `wayland-client = "0.31"`, `wayland-protocols-wlr = "0.3"`,
  `precomputed-hash = "0.1"`, `rustix = "1"`, `bitflags = "2"`,
  `fontconfig = "0.11"`. **P5 adds no dependency of its own.**
- **No `smithay`, no `gtk4`/`gio`/`glib`/`pango`/`cairo`/`gdk`.** These are
  Wayland *clients*; wlroots is the server side and lives in another crate.
- **Edition 2024, `rust-version = 1.94`** (inherited from the workspace).
- **Gates — every task's final "run" step must leave these green:**
  - `cargo test -p icedtea-ui`
  - `cargo clippy -p icedtea-ui --all-targets -- -D warnings`
  - `cargo fmt --all --check`
- **Commit trailer** on every commit:
  `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`
- **The M1 pixel gate `ui/tests/themed_button_offscreen.rs` must stay green
  and byte-identical** unless the contract's §10.3 migration row for that file
  says otherwise. P5 has no migration row for it: **P5 must not edit it at
  all.** The same holds for `ui/tests/layer_shell_screencopy.rs` and
  `ui/tests/support/mod.rs` — P5 adds a *new* test file instead.
- **Parts execute IN ORDER 1 → 6 on one branch.** By the time P5 starts, P1's
  registry/values, P2's `Node`/`MatchCx`, P3's `ComputedStyle`, and P4's
  `LayoutTree`/`paint_node` all exist. This part may consume any of their
  Produces, and must not modify `css/**`, `layout.rs`, `paint/**`, or
  `text.rs`.
- **No property strings outside `css::registry`.** Everything in `anim/`
  names properties as `Prop::*`.
- **Never panic on hostile numbers.** Durations, delays, iteration counts and
  keyframe offsets can all arrive as NaN or ±∞ from a `calc()`; every public
  entry point in `anim/` must survive them (Task 11's battery).
- **Every load-bearing test states its mutation check** in a comment: the
  single-line source change that must make it fail.

---

## File Structure

| File | Create/Modify | Responsibility |
|---|---|---|
| `ui/src/anim/mod.rs` | Create (idempotent — see deviation 1) | Module root. `Overrides`, `TransitionSpec`, `AnimationSpec`, `AnimationState`, `interpolate_prop`, `millis`. Re-exports `Clock`/`ManualClock`/`MonotonicClock`. |
| `ui/src/anim/clock.rs` | Create | `Clock` trait, `MonotonicClock` (production), `ManualClock` (tests). Nothing else in the crate reads wall-clock time. |
| `ui/src/anim/transition.rs` | Create | `Transition` (one running property transition, incl. reversal bookkeeping) and `transitioned_props` (which longhands a style's `transition-*` govern). |
| `ui/src/anim/keyframes.rs` | Create | `ActiveAnimation` (one running `@keyframes` animation: phases, iterations, direction, fill, play-state) and `resolve_segment` (keyframe bracketing with endpoints synthesised from the underlying style). |
| `ui/src/lib.rs` | Modify (`pub mod anim;`, one line) | Expose the module. |
| `ui/src/widget/button.rs` | Modify (fields + 4 methods + 2 call sites) | The `Overrides` hand-off: the widget owns a clock and an `AnimationState`, and paints through the sampled overrides. |
| `ui/src/wayland.rs` | Modify (2 fields, 1 `Dispatch` impl, 1 helper, 2 call sites) | The frame-callback pump: request `wl_surface.frame` while anything animates; never busy-loop. |
| `ui/tests/transition_screencopy.rs` | Create | The Wayland proof: a `transition: background-color` really animates on a real compositor's output — start and end captured, intermediates not asserted. |

---

## Task 1: The `anim` module, the clock, and the spec structs

**Files:**
- Create: `ui/src/anim/mod.rs`
- Create: `ui/src/anim/clock.rs`
- Modify: `ui/src/lib.rs` (add `pub mod anim;` to the module list at lines 8-15)

**Interfaces:**
- Consumes:
  - `crate::css::registry::Prop` — `#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)] #[repr(u8)] pub enum Prop` (contract §1.1).
  - `crate::css::value::Value` — `#[derive(Clone, Debug, PartialEq)] pub enum Value` (contract §2.1).
  - `crate::css::value::keyframes::Keyframes` and `crate::css::value::timing::{TimingFunction, Time, AnimationName, IterationCount}` (contract §2.9), re-exported from `crate::css::value`.
  - `crate::css::value::Keyword` (contract §2.2).
- Produces:
  - `pub trait Clock { fn now(&self) -> std::time::Duration; }`
  - `pub struct MonotonicClock(std::time::Instant);` + `pub fn new() -> Self` + `impl Default` + `impl Clock`
  - `pub struct ManualClock(std::cell::Cell<std::time::Duration>);` + `pub fn new() -> Self`, `pub fn advance_ms(&self, ms: u64)`, `pub fn set_ms(&self, ms: u64)` + `impl Default` + `impl Clock`
  - `#[derive(Clone, Debug, PartialEq)] pub struct TransitionSpec { pub prop: Option<Prop>, pub all: bool, pub duration: Time, pub delay: Time, pub timing: TimingFunction }`
  - `#[derive(Clone, Debug, PartialEq)] pub struct AnimationSpec { pub name: AnimationName, pub duration: Time, pub delay: Time, pub timing: TimingFunction, pub iterations: IterationCount, pub direction: Keyword, pub fill: Keyword, pub play_state: Keyword }`
  - `pub fn millis(d: std::time::Duration) -> f64`

**Idempotence note (deviation 1):** if `ui/src/anim/mod.rs` already exists
because P3 created it for `ComputedStyle::transition_specs()`, keep its
`TransitionSpec`/`AnimationSpec` declarations byte-for-byte and add only the
`pub mod` lines, `millis`, and the re-exports below. Both spellings are the
contract's, so they will already match.

- [ ] **Step 1: Write the failing test**

Create `ui/src/anim/clock.rs` containing only its test module for now:

```rust
//! The animation clock.
//!
//! Every time value in `anim` comes through a [`Clock`]. Production code
//! uses [`MonotonicClock`]; tests use [`ManualClock`], which is the only
//! reason animation behaviour is assertable to the millisecond instead of
//! being sampled against a wall clock and hedged with tolerances.

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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib anim::clock`
Expected: FAIL to compile with `error[E0433]: failed to resolve: could not find `anim` in the crate root` (or, once `mod.rs` exists but `clock.rs` has no items, `error[E0432]: unresolved imports `super::Clock`, `super::ManualClock`, `super::MonotonicClock``).

- [ ] **Step 3: Write minimal implementation**

Prepend to `ui/src/anim/clock.rs`, above the test module:

```rust
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
```

Create `ui/src/anim/mod.rs`:

```rust
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
use crate::css::value::{AnimationName, IterationCount, Keyword, Time, TimingFunction};

/// A `Duration` as milliseconds.
///
/// The engine works in `f64` milliseconds internally rather than `Duration`
/// because a negative `animation-delay`/`transition-delay` puts a start time
/// *before* the clock's epoch, which `Duration` cannot represent.
#[must_use]
pub fn millis(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// One flattened entry of the computed `transition-*` longhands.
///
/// `all == true` means `transition-property: all`; otherwise `prop` names the
/// property (which may be a shorthand — Adwaita's base `button` rule lists
/// `outline`). `prop: None` with `all: false` is `none` or an unresolvable
/// ident and governs nothing.
#[derive(Clone, Debug, PartialEq)]
pub struct TransitionSpec {
    pub prop: Option<Prop>,
    pub all: bool,
    pub duration: Time,
    pub delay: Time,
    pub timing: TimingFunction,
}

/// One flattened entry of the computed `animation-*` longhands.
#[derive(Clone, Debug, PartialEq)]
pub struct AnimationSpec {
    pub name: AnimationName,
    pub duration: Time,
    pub delay: Time,
    pub timing: TimingFunction,
    pub iterations: IterationCount,
    pub direction: Keyword,
    pub fill: Keyword,
    pub play_state: Keyword,
}
```

Create placeholder-free stubs so the module tree compiles — `ui/src/anim/transition.rs` and `ui/src/anim/keyframes.rs`, each containing only their module doc comment for now:

`ui/src/anim/transition.rs`:

```rust
//! One running property transition, and the rule that decides which
//! longhands a style's `transition-*` declarations govern.
```

`ui/src/anim/keyframes.rs`:

```rust
//! One running `@keyframes` animation: phases, iterations, direction,
//! fill mode, play state, and keyframe bracketing.
```

Add to `ui/src/lib.rs`, in the module list (alphabetical, before `pub mod app;`):

```rust
pub mod anim;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib anim::clock`
Expected: PASS — 3 tests.

Run: `cargo clippy -p icedtea-ui --all-targets -- -D warnings && cargo fmt --all --check`
Expected: no output, exit 0.

- [ ] **Step 5: Commit**

```bash
git add ui/src/anim/mod.rs ui/src/anim/clock.rs ui/src/anim/transition.rs ui/src/anim/keyframes.rs ui/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(ui): add the anim module, the Clock trait and its two clocks

Production time is monotonic and frame-callback driven; test time is a
ManualClock, which is what makes every later animation assertion exact to
the millisecond instead of wall-clock-sampled with a tolerance.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: `Overrides` and the registry-driven interpolator

**Files:**
- Modify: `ui/src/anim/mod.rs` (append after `AnimationSpec`)

**Interfaces:**
- Consumes:
  - `Prop::interpolator(self) -> Option<Interpolate>` where `pub type Interpolate = fn(&Value, &Value, f32) -> Value` (contract §1.2/§1.3).
  - `crate::css::value::interpolate::discrete(a: &Value, b: &Value, t: f32) -> Value` (contract §2.10).
  - `crate::css::registry::Prop`, `crate::css::value::Value`.
- Produces:
  - `#[derive(Clone, Debug, Default, PartialEq)] pub struct Overrides`
  - `pub fn is_empty(&self) -> bool`
  - `pub fn len(&self) -> usize` (test observability — see deviation 7)
  - `pub fn get(&self, prop: Prop) -> Option<&Value>`
  - `pub fn iter(&self) -> impl Iterator<Item = (Prop, &Value)>`
  - `pub fn set(&mut self, prop: Prop, value: Value)`
  - `pub fn interpolate_prop(prop: Prop, a: &Value, b: &Value, t: f32) -> Value`

- [ ] **Step 1: Write the failing test**

Append to `ui/src/anim/mod.rs`:

```rust
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
        let mid = interpolate_prop(
            Prop::Opacity,
            &Value::Number(0.0),
            &Value::Number(1.0),
            0.5,
        );
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib anim::tests`
Expected: FAIL to compile with `error[E0432]: unresolved imports `super::Overrides`, `super::interpolate_prop``.

- [ ] **Step 3: Write minimal implementation**

Insert into `ui/src/anim/mod.rs`, after the `AnimationSpec` declaration and
before the test module:

```rust
/// Per-node animation output: the values that replace the computed style's
/// for this frame, sorted by `Prop` so lookups are a binary search and
/// iteration is in registry order.
///
/// Written twice per frame — transitions first, then animations — so a later
/// [`set`](Overrides::set) is exactly CSS's "animation overrides transition".
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Overrides {
    entries: Vec<(Prop, Value)>,
}

impl Overrides {
    /// Whether anything is being overridden this frame.
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

    /// This frame's value for `prop`, if it is being animated.
    #[must_use]
    pub fn get(&self, prop: Prop) -> Option<&Value> {
        self.entries
            .binary_search_by_key(&prop, |(p, _)| *p)
            .ok()
            .map(|index| &self.entries[index].1)
    }

    /// Every override, in registry order.
    pub fn iter(&self) -> impl Iterator<Item = (Prop, &Value)> {
        self.entries.iter().map(|(prop, value)| (*prop, value))
    }

    /// Override `prop`, replacing any value already set for it.
    pub fn set(&mut self, prop: Prop, value: Value) {
        match self.entries.binary_search_by_key(&prop, |(p, _)| *p) {
            Ok(index) => self.entries[index].1 = value,
            Err(index) => self.entries.insert(index, (prop, value)),
        }
    }
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
```

Add `use crate::css::value::Value;` to `mod.rs`'s import block (it becomes
`use crate::css::value::{AnimationName, IterationCount, Keyword, Time, TimingFunction, Value};`).

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib anim::tests`
Expected: PASS — 2 tests.

Run: `cargo clippy -p icedtea-ui --all-targets -- -D warnings && cargo fmt --all --check`
Expected: no output, exit 0.

- [ ] **Step 5: Commit**

```bash
git add ui/src/anim/mod.rs
git commit -m "$(cat <<'EOF'
feat(ui): add Overrides and the registry-driven interpolator

Overrides is a Prop-sorted table so "animation overrides transition" is just
the write order. interpolate_prop never hand-writes per-property maths: it
dispatches to the registry row, falling back to discrete for a property the
registry marks non-animatable.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: `Transition` and timing-function exactness

**Files:**
- Modify: `ui/src/anim/transition.rs` (append below the module doc comment)

**Interfaces:**
- Consumes:
  - `crate::anim::{TransitionSpec, interpolate_prop, millis}` (Tasks 1-2).
  - `crate::css::value::timing::TimingFunction` with `pub const EASE`,
    `EASE_IN`, `EASE_OUT`, `EASE_IN_OUT`, `pub fn eval(self, t: f32) -> f32`,
    and variants `Linear`, `CubicBezier(f32, f32, f32, f32)`,
    `Steps(u32, StepPosition)`; `pub enum StepPosition { JumpStart, JumpEnd, JumpNone, JumpBoth }` (contract §2.9).
  - `crate::css::value::timing::Time(pub f32)` with `pub fn from_ms(ms: f32) -> Time` and `pub fn as_secs_f32(self) -> f32`.
- Produces:
  - `#[derive(Clone, Debug)] pub struct Transition { pub prop: Prop, pub from: Value, pub to: Value, pub reversing_adjusted_start: Value, pub reversing_shortening_factor: f32, pub start_ms: f64, pub duration_ms: f64, pub timing: TimingFunction }`
  - `pub fn start(prop: Prop, from: Value, to: Value, spec: &TransitionSpec, now_ms: f64) -> Transition`
  - `pub fn progress(&self, now_ms: f64) -> f32`
  - `pub fn eased(&self, now_ms: f64) -> f32`
  - `pub fn value_at(&self, now_ms: f64) -> Value`
  - `pub fn end_ms(&self) -> f64`
  - `pub fn is_finished(&self, now_ms: f64) -> bool`
  - `pub fn combined_ms(spec: &TransitionSpec) -> f64`
  - `pub fn duration_ms_of(spec: &TransitionSpec) -> f64`
  - `pub fn delay_ms_of(spec: &TransitionSpec) -> f64`

- [ ] **Step 1: Write the failing test**

Append to `ui/src/anim/transition.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::{Transition, combined_ms, delay_ms_of, duration_ms_of};
    use crate::anim::TransitionSpec;
    use crate::css::registry::Prop;
    use crate::css::value::timing::{StepPosition, Time, TimingFunction};
    use crate::css::value::Value;

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
        assert_eq!(duration_ms_of(&spec(200.0, 0.0, TimingFunction::Linear)), 200.0);
        assert_eq!(delay_ms_of(&spec(200.0, -50.0, TimingFunction::Linear)), -50.0);
        assert_eq!(combined_ms(&spec(200.0, -50.0, TimingFunction::Linear)), 150.0);
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib anim::transition`
Expected: FAIL to compile with `error[E0432]: unresolved imports `super::Transition`, `super::combined_ms`, `super::delay_ms_of`, `super::duration_ms_of``.

- [ ] **Step 3: Write minimal implementation**

Insert into `ui/src/anim/transition.rs`, between the module doc comment and
the test module:

```rust
use crate::anim::{TransitionSpec, interpolate_prop};
use crate::css::registry::Prop;
use crate::css::value::Value;
use crate::css::value::timing::TimingFunction;

/// `spec`'s `transition-duration` in milliseconds, clamped non-negative and
/// finite. A `calc()` can produce NaN; NaN durations are treated as zero so
/// the transition completes instantly rather than never.
#[must_use]
pub fn duration_ms_of(spec: &TransitionSpec) -> f64 {
    let ms = f64::from(spec.duration.as_secs_f32()) * 1000.0;
    if ms.is_finite() { ms.max(0.0) } else { 0.0 }
}

/// `spec`'s `transition-delay` in milliseconds. May be negative (CSS allows
/// it: the transition starts already part-way through). NaN becomes zero.
#[must_use]
pub fn delay_ms_of(spec: &TransitionSpec) -> f64 {
    let ms = f64::from(spec.delay.as_secs_f32()) * 1000.0;
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
```

Note on `if !(self.duration_ms > 0.0)`: written this way rather than
`if self.duration_ms <= 0.0` so that a NaN duration takes the
instant-completion branch instead of falling through to a division that
yields NaN. Add `#[allow(clippy::neg_cmp_op_on_partial_ord)]` above the
`if` if clippy objects, with that reason as the comment.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib anim::transition`
Expected: PASS — 5 tests.

Run: `cargo clippy -p icedtea-ui --all-targets -- -D warnings && cargo fmt --all --check`
Expected: no output, exit 0.

- [ ] **Step 5: Commit**

```bash
git add ui/src/anim/transition.rs
git commit -m "$(cat <<'EOF'
feat(ui): add Transition and pin the timing functions at known points

Linear is exactly half way at half its duration; `ease` reports 0.8024 at
t = 0.5, the number every browser agrees on, which is only true if the bezier
is solved on x rather than evaluated parametrically. NaN and negative
durations complete instantly instead of dividing to NaN.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: `transitioned_props` — which longhands a style transitions

**Files:**
- Modify: `ui/src/anim/transition.rs` (append `transitioned_props` above the test module; extend the test module)

**Interfaces:**
- Consumes:
  - `crate::css::registry::animatable_longhands() -> impl Iterator<Item = Prop>` (contract §1.3).
  - `Prop::is_longhand(self) -> bool`, `Prop::longhands(self) -> &'static [Prop]` (contract §1.3).
  - `crate::anim::TransitionSpec` (Task 1).
- Produces:
  - `pub fn transitioned_props(specs: &[TransitionSpec]) -> Vec<(Prop, TransitionSpec)>` — every longhand the specs govern, sorted by `Prop`, each paired with the spec that wins for it (the last spec listing it).

- [ ] **Step 1: Write the failing test**

Append to `ui/src/anim/transition.rs`'s test module:

```rust
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib anim::transition`
Expected: FAIL to compile with `error[E0425]: cannot find function `transitioned_props` in module `super``.

- [ ] **Step 3: Write minimal implementation**

Insert into `ui/src/anim/transition.rs`, after `impl Transition` and before
the test module:

```rust
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
```

The sorted insert keeps the table ascending, so the final sort the test names
is the insertion discipline itself — no separate sort call is needed. Keep the
test's window assertion: it is what pins that discipline.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib anim::transition`
Expected: PASS — 10 tests.

Run: `cargo clippy -p icedtea-ui --all-targets -- -D warnings && cargo fmt --all --check`
Expected: no output, exit 0.

- [ ] **Step 5: Commit**

```bash
git add ui/src/anim/transition.rs
git commit -m "$(cat <<'EOF'
feat(ui): resolve which longhands a style's transition-* declarations govern

`all` expands through animatable_longhands, so it never puts a discrete flip
on a non-animatable property. A shorthand named in transition-property
expands to its longhands -- Adwaita's base button rule lists `outline`, which
would otherwise transition nothing at all.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: `resolve_segment` — bracketing a property inside `@keyframes`

**Files:**
- Modify: `ui/src/anim/keyframes.rs` (append below the module doc comment)

**Interfaces:**
- Consumes:
  - `crate::css::value::keyframes::{Keyframe, Keyframes}` (contract §2.9):
    `pub struct Keyframe { pub offsets: Rc<[f32]>, pub declarations: Rc<[(Prop, Value)]>, pub timing: Option<TimingFunction> }`,
    `pub struct Keyframes { pub name: Rc<str>, pub frames: Rc<[Keyframe]> }`, and
    `pub fn properties(&self) -> Vec<Prop>`.
  - `crate::css::computed::{ComputedStyle, ResolveEnv}` with
    `pub fn raw(&self, prop: Prop) -> &Value` and
    `pub fn initial(env: &ResolveEnv) -> Rc<ComputedStyle>` (contract §5).
  - `crate::css::registry::Prop`, `crate::css::value::Value`,
    `crate::css::value::timing::TimingFunction`.
- Produces:
  - `pub fn resolve_segment(frames: &[Keyframe], prop: Prop, q: f32, base: &ComputedStyle) -> Option<(Value, Value, f32, Option<TimingFunction>)>`

**Why this rather than `Keyframes::segment`** (contract §2.9): `segment` has no
access to the underlying computed style, so it cannot synthesise the missing
`0%`/`100%` endpoints CSS Animations L1 requires ("if `from` is missing, the
value at animation start is the underlying value"). `resolve_segment` takes
`base` for exactly that. `Keyframes::segment` stays available for callers that
do not need synthesis; this part does not use it.

- [ ] **Step 1: Write the failing test**

Append to `ui/src/anim/keyframes.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::resolve_segment;
    use crate::css::computed::{ComputedStyle, ResolveEnv};
    use crate::css::registry::Prop;
    use crate::css::value::Value;
    use crate::css::value::keyframes::Keyframe;
    use crate::css::value::timing::TimingFunction;
    use std::rc::Rc;

    fn frame(offsets: &[f32], prop: Prop, value: Value) -> Keyframe {
        Keyframe {
            offsets: Rc::from(offsets),
            declarations: Rc::from(vec![(prop, value)]),
            timing: None,
        }
    }

    fn timed_frame(
        offsets: &[f32],
        prop: Prop,
        value: Value,
        timing: TimingFunction,
    ) -> Keyframe {
        Keyframe {
            offsets: Rc::from(offsets),
            declarations: Rc::from(vec![(prop, value)]),
            timing: Some(timing),
        }
    }

    fn base() -> Rc<ComputedStyle> {
        ComputedStyle::initial(&ResolveEnv::default())
    }

    // Mutation check: divide by `hi.offset` instead of `hi.offset -
    // lo.offset` and the 0.25-of-a-0.5-span sample becomes 0.5 instead of 1.0.
    #[test]
    fn a_property_between_two_keyframes_reports_its_local_progress() {
        let base = base();
        let frames = [
            frame(&[0.0], Prop::Opacity, Value::Number(0.0)),
            frame(&[0.5], Prop::Opacity, Value::Number(0.4)),
            frame(&[1.0], Prop::Opacity, Value::Number(1.0)),
        ];
        let (from, to, t, timing) =
            resolve_segment(&frames, Prop::Opacity, 0.25, &base).expect("bracketed");
        assert_eq!(from, Value::Number(0.0));
        assert_eq!(to, Value::Number(0.4));
        assert!((t - 0.5).abs() < 1e-6, "0.25 is half way through the 0..0.5 span, got {t}");
        assert_eq!(timing, None);

        let (from, to, t, _) =
            resolve_segment(&frames, Prop::Opacity, 0.75, &base).expect("bracketed");
        assert_eq!(from, Value::Number(0.4));
        assert_eq!(to, Value::Number(1.0));
        assert!((t - 0.5).abs() < 1e-6, "got {t}");
    }

    // Mutation check: drop the endpoint synthesis and this returns the 0.6
    // keyframe's own value on both sides (a flat hold), so the `from`
    // assertion against the underlying opacity fails.
    #[test]
    fn a_missing_from_keyframe_is_synthesised_from_the_underlying_style() {
        let base = base();
        let underlying = base.raw(Prop::Opacity).clone();
        let frames = [frame(&[1.0], Prop::Opacity, Value::Number(0.0))];
        let (from, to, t, _) =
            resolve_segment(&frames, Prop::Opacity, 0.5, &base).expect("bracketed");
        assert_eq!(
            from, underlying,
            "with no 0% keyframe the segment starts at the underlying value"
        );
        assert_eq!(to, Value::Number(0.0));
        assert!((t - 0.5).abs() < 1e-6, "got {t}");
    }

    // Mutation check: synthesise only the leading endpoint and the `to`
    // assertion falls back to the 0% keyframe's own 0.0.
    #[test]
    fn a_missing_to_keyframe_is_synthesised_from_the_underlying_style() {
        let base = base();
        let underlying = base.raw(Prop::Opacity).clone();
        let frames = [frame(&[0.0], Prop::Opacity, Value::Number(0.0))];
        let (from, to, _, _) =
            resolve_segment(&frames, Prop::Opacity, 0.5, &base).expect("bracketed");
        assert_eq!(from, Value::Number(0.0));
        assert_eq!(to, underlying);
    }

    // Mutation check: keep the first value at a duplicated offset instead of
    // the last and `to` becomes 0.2.
    #[test]
    fn a_later_keyframe_at_the_same_offset_wins() {
        let base = base();
        let frames = [
            frame(&[0.0], Prop::Opacity, Value::Number(0.0)),
            frame(&[1.0], Prop::Opacity, Value::Number(0.2)),
            frame(&[1.0], Prop::Opacity, Value::Number(0.9)),
        ];
        let (_, to, _, _) =
            resolve_segment(&frames, Prop::Opacity, 0.5, &base).expect("bracketed");
        assert_eq!(to, Value::Number(0.9));
    }

    // Mutation check: read only `offsets[0]` and the 0.25 sample brackets
    // 0.0..1.0 instead of 0.0..0.5, changing `to` to 1.0.
    #[test]
    fn one_keyframe_block_can_carry_several_offsets() {
        let base = base();
        let frames = [
            frame(&[0.0, 0.5], Prop::Opacity, Value::Number(0.3)),
            frame(&[1.0], Prop::Opacity, Value::Number(1.0)),
        ];
        let (from, to, _, _) =
            resolve_segment(&frames, Prop::Opacity, 0.25, &base).expect("bracketed");
        assert_eq!(from, Value::Number(0.3));
        assert_eq!(
            to,
            Value::Number(0.3),
            "0.25 lies between the two offsets of the same block, so both ends \
             are that block's value"
        );
        let (_, to, _, _) =
            resolve_segment(&frames, Prop::Opacity, 0.75, &base).expect("bracketed");
        assert_eq!(to, Value::Number(1.0));
    }

    // Mutation check: return `Some` with the underlying value on both sides
    // when the property is absent, and an animation would pin every property
    // in the style rather than only the ones its keyframes mention.
    #[test]
    fn a_property_no_keyframe_mentions_has_no_segment() {
        let base = base();
        let frames = [frame(&[0.0], Prop::Opacity, Value::Number(0.0))];
        assert!(resolve_segment(&frames, Prop::Color, 0.5, &base).is_none());
        assert!(resolve_segment(&[], Prop::Opacity, 0.5, &base).is_none());
    }

    // Mutation check: clamp `q` before bracketing instead of returning a flat
    // hold, and the t reported at q = 1.5 becomes 0.0 rather than 1.0.
    #[test]
    fn progress_outside_the_keyframe_range_holds_the_nearest_value() {
        let base = base();
        let frames = [
            frame(&[0.2], Prop::Opacity, Value::Number(0.1)),
            frame(&[0.8], Prop::Opacity, Value::Number(0.9)),
        ];
        // The synthesised 0.0 and 1.0 endpoints take the underlying value,
        // so below 0.2 the segment is underlying -> 0.1, not a flat hold.
        let (from, to, _, _) =
            resolve_segment(&frames, Prop::Opacity, 0.1, &base).expect("bracketed");
        assert_eq!(from, *base.raw(Prop::Opacity));
        assert_eq!(to, Value::Number(0.1));
    }

    // Mutation check: return the *upper* keyframe's timing instead of the
    // lower one's and this reports EASE_IN.
    #[test]
    fn the_segments_timing_comes_from_the_keyframe_it_starts_at() {
        let base = base();
        let frames = [
            timed_frame(&[0.0], Prop::Opacity, Value::Number(0.0), TimingFunction::EASE_OUT),
            timed_frame(&[1.0], Prop::Opacity, Value::Number(1.0), TimingFunction::EASE_IN),
        ];
        let (_, _, _, timing) =
            resolve_segment(&frames, Prop::Opacity, 0.5, &base).expect("bracketed");
        assert_eq!(timing, Some(TimingFunction::EASE_OUT));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib anim::keyframes`
Expected: FAIL to compile with `error[E0432]: unresolved import `super::resolve_segment``.

- [ ] **Step 3: Write minimal implementation**

Insert into `ui/src/anim/keyframes.rs`, between the module doc comment and the
test module:

```rust
use crate::css::computed::ComputedStyle;
use crate::css::registry::Prop;
use crate::css::value::Value;
use crate::css::value::keyframes::Keyframe;
use crate::css::value::timing::TimingFunction;

/// The two keyframe values bracketing `q` for `prop`, the local `0..=1`
/// progress between them, and the timing function declared on the keyframe
/// the segment *starts* at (CSS Animations L1: a keyframe's
/// `animation-timing-function` applies to the segment leaving it).
///
/// Missing `0%`/`100%` keyframes are synthesised from `base` — the
/// underlying, non-animated computed value — which is why this exists
/// alongside `Keyframes::segment`, which has no `base` to synthesise from.
///
/// `None` when no keyframe mentions `prop` at all.
#[must_use]
pub fn resolve_segment(
    frames: &[Keyframe],
    prop: Prop,
    q: f32,
    base: &ComputedStyle,
) -> Option<(Value, Value, f32, Option<TimingFunction>)> {
    // Collect one point per distinct offset, in source order, letting a later
    // keyframe at the same offset replace an earlier one (CSS declaration
    // order inside a single `@keyframes` rule).
    let mut points: Vec<(f32, Value, Option<TimingFunction>)> = Vec::new();
    for frame in frames {
        let Some((_, value)) = frame.declarations.iter().rev().find(|(p, _)| *p == prop) else {
            continue;
        };
        for &offset in frame.offsets.iter() {
            if !offset.is_finite() {
                continue;
            }
            let offset = offset.clamp(0.0, 1.0);
            match points.iter().position(|(o, _, _)| *o == offset) {
                Some(index) => points[index] = (offset, value.clone(), frame.timing),
                None => points.push((offset, value.clone(), frame.timing)),
            }
        }
    }
    if points.is_empty() {
        return None;
    }
    points.sort_by(|a, b| a.0.total_cmp(&b.0));

    // Synthesise the endpoints CSS says come from the underlying value.
    if points[0].0 > 0.0 {
        points.insert(0, (0.0, base.raw(prop).clone(), None));
    }
    let last = points.len() - 1;
    if points[last].0 < 1.0 {
        points.push((1.0, base.raw(prop).clone(), None));
    }

    let last = points.len() - 1;
    let q = if q.is_finite() { q } else { 0.0 };
    if q <= points[0].0 {
        let (_, value, timing) = &points[0];
        return Some((value.clone(), value.clone(), 0.0, *timing));
    }
    if q >= points[last].0 {
        let (_, value, timing) = &points[last];
        return Some((value.clone(), value.clone(), 1.0, *timing));
    }

    let hi = points.iter().position(|(o, _, _)| *o > q).unwrap_or(last);
    let lo = hi - 1;
    let span = points[hi].0 - points[lo].0;
    let t = if span > 0.0 { (q - points[lo].0) / span } else { 1.0 };
    Some((
        points[lo].1.clone(),
        points[hi].1.clone(),
        t,
        points[lo].2,
    ))
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib anim::keyframes`
Expected: PASS — 8 tests.

Run: `cargo clippy -p icedtea-ui --all-targets -- -D warnings && cargo fmt --all --check`
Expected: no output, exit 0.

- [ ] **Step 5: Commit**

```bash
git add ui/src/anim/keyframes.rs
git commit -m "$(cat <<'EOF'
feat(ui): bracket a property inside @keyframes, synthesising endpoints

CSS says a missing `from`/`to` keyframe takes the underlying, non-animated
computed value; Keyframes::segment has no access to that style, so this
takes the base style and synthesises the endpoints itself. A later keyframe
at the same offset replaces an earlier one.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 6a: `ActiveAnimation` — phases, iterations and direction

> Tasks 6a and 6b together are what an earlier draft called "Task 6". They are
> split so each stays inside this plan's ~400-line-per-task sizing ceiling:
> 6a builds the running animation and its timeline (phase, iteration index,
> local progress, direction); 6b adds the two features that only *gate* that
> timeline — `animation-fill-mode` and `animation-play-state`. Later tasks
> Every cross-reference elsewhere in this plan has been repointed at whichever
> half it actually means, so no task refers to a bare "Task 6".

**Files:**
- Modify: `ui/src/anim/keyframes.rs` (append above the test module; extend the test module)

**Interfaces:**
- Consumes:
  - `crate::anim::{AnimationSpec, Overrides, interpolate_prop}` (Tasks 1-2).
  - `resolve_segment` (Task 5).
  - `crate::css::value::keyframes::Keyframes` with `pub fn properties(&self) -> Vec<Prop>`.
  - `crate::css::value::{Keyword, IterationCount, Time, TimingFunction, Value}`.
  - `crate::css::computed::ComputedStyle`.
- Produces:
  - `#[derive(Copy, Clone, Debug, PartialEq, Eq)] pub enum Phase { Before, Active, After }`
  - `#[derive(Clone, Debug)] pub struct ActiveAnimation { pub spec: AnimationSpec, pub name: Rc<str>, .. }`
  - `pub fn start(spec: AnimationSpec, name: Rc<str>, keyframes: Rc<Keyframes>, now_ms: f64) -> ActiveAnimation`
  - `pub fn phase(&self, now_ms: f64) -> Phase`
  - `pub fn iteration_progress(&self, now_ms: f64) -> Option<(u64, f32)>` (fill-blind here; Task 6b widens it)
  - `pub fn sample(&self, now_ms: f64, base: &ComputedStyle, out: &mut Overrides)`
  - `pub fn is_active(&self, now_ms: f64) -> bool`
  - `pub fn next_change_ms(&self, now_ms: f64) -> f64`
  - `pub fn props(&self) -> &[Prop]`
  - `pub fn direction_progress(direction: Keyword, iteration: u64, q: f32) -> f32`

- [ ] **Step 1: Write the failing test**

Append to `ui/src/anim/keyframes.rs`'s test module (the fixtures below are
also used by Task 6b, so they go in once, here):

```rust
    use super::{ActiveAnimation, Phase, direction_progress};
    use crate::anim::{AnimationSpec, Overrides};
    use crate::css::value::keyframes::Keyframes;
    use crate::css::value::{AnimationName, IterationCount, Keyword, Time};

    fn fade() -> Rc<Keyframes> {
        Rc::new(Keyframes {
            name: Rc::from("fade"),
            frames: Rc::from(vec![
                frame(&[0.0], Prop::Opacity, Value::Number(0.0)),
                frame(&[1.0], Prop::Opacity, Value::Number(1.0)),
            ]),
        })
    }

    fn spec(
        duration_ms: f32,
        delay_ms: f32,
        iterations: IterationCount,
        direction: Keyword,
        fill: Keyword,
    ) -> AnimationSpec {
        AnimationSpec {
            name: AnimationName::Named(Rc::from("fade")),
            duration: Time(duration_ms / 1000.0),
            delay: Time(delay_ms / 1000.0),
            timing: TimingFunction::Linear,
            iterations,
            direction,
            fill,
            play_state: Keyword::Running,
        }
    }

    fn opacity_at(anim: &ActiveAnimation, now_ms: f64, base: &ComputedStyle) -> Option<f32> {
        let mut out = Overrides::default();
        anim.sample(now_ms, base, &mut out);
        match out.get(Prop::Opacity) {
            Some(Value::Number(v)) => Some(*v),
            Some(other) => panic!("opacity sampled as {other:?}"),
            None => None,
        }
    }

    // THE GATE (spec §7: manual-clock exactness).
    // Mutation check: floor the iteration index with `round` and the 250 ms
    // sample of a 1000 ms animation stays 0.25 but the 1250 ms sample of
    // the `alternate` test below flips -- so the local tripwire here is the
    // 1000 ms boundary, which must be iteration 1 at q = 0, i.e. 0.0.
    #[test]
    fn a_linear_animation_reports_its_local_progress_each_iteration() {
        let base = base();
        let anim = ActiveAnimation::start(
            spec(1000.0, 0.0, IterationCount::Count(3.0), Keyword::Normal, Keyword::None),
            Rc::from("fade"),
            fade(),
            0.0,
        );
        assert_eq!(opacity_at(&anim, 0.0, &base), Some(0.0));
        assert_eq!(opacity_at(&anim, 250.0, &base), Some(0.25));
        assert_eq!(opacity_at(&anim, 750.0, &base), Some(0.75));
        assert_eq!(
            opacity_at(&anim, 1000.0, &base),
            Some(0.0),
            "1000 ms is the start of iteration 1, not the end of iteration 0"
        );
        assert_eq!(opacity_at(&anim, 1250.0, &base), Some(0.25));
        assert_eq!(anim.phase(500.0), Phase::Active);
        assert_eq!(anim.phase(3000.0), Phase::After);
    }

    // THE GATE (spec §7: `alternate`).
    // Mutation check: alternate on `iteration % 2 == 0` instead of `== 1`
    // and the first and second iterations swap, failing both samples.
    #[test]
    fn alternate_runs_every_other_iteration_backwards() {
        let base = base();
        let anim = ActiveAnimation::start(
            spec(1000.0, 0.0, IterationCount::Count(4.0), Keyword::Alternate, Keyword::None),
            Rc::from("fade"),
            fade(),
            0.0,
        );
        assert_eq!(opacity_at(&anim, 250.0, &base), Some(0.25), "iteration 0 forwards");
        assert_eq!(opacity_at(&anim, 1250.0, &base), Some(0.75), "iteration 1 backwards");
        assert_eq!(opacity_at(&anim, 2250.0, &base), Some(0.25), "iteration 2 forwards");
    }

    // Mutation check: map `Reverse` to `q` and both assertions collapse onto
    // the `Normal` values.
    #[test]
    fn direction_maps_iteration_progress_the_way_css_says() {
        assert!((direction_progress(Keyword::Normal, 0, 0.25) - 0.25).abs() < 1e-6);
        assert!((direction_progress(Keyword::Reverse, 0, 0.25) - 0.75).abs() < 1e-6);
        assert!((direction_progress(Keyword::Alternate, 0, 0.25) - 0.25).abs() < 1e-6);
        assert!((direction_progress(Keyword::Alternate, 1, 0.25) - 0.75).abs() < 1e-6);
        assert!((direction_progress(Keyword::AlternateReverse, 0, 0.25) - 0.75).abs() < 1e-6);
        assert!((direction_progress(Keyword::AlternateReverse, 1, 0.25) - 0.25).abs() < 1e-6);
    }

    // Mutation check: make `total_ms` finite for IterationCount::Infinite and
    // `is_active` goes false past the first iteration, so the spinner stops.
    #[test]
    fn an_infinite_animation_never_reaches_its_after_phase() {
        let base = base();
        let anim = ActiveAnimation::start(
            spec(1000.0, 0.0, IterationCount::Infinite, Keyword::Normal, Keyword::None),
            Rc::from("fade"),
            fade(),
            0.0,
        );
        assert!(anim.is_active(1_000_000.0));
        assert_eq!(anim.phase(1_000_000.0), Phase::Active);
        assert_eq!(opacity_at(&anim, 1_000_250.0, &base), Some(0.25));
    }

```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib anim::keyframes`
Expected: FAIL to compile with `error[E0432]: unresolved imports `super::ActiveAnimation`, `super::Phase`, `super::direction_progress``.

- [ ] **Step 3: Write minimal implementation**

Insert into `ui/src/anim/keyframes.rs`, after `resolve_segment` and before the
test module (and extend the file's `use` block to
`use crate::anim::{AnimationSpec, Overrides, interpolate_prop};`,
`use crate::css::value::keyframes::{Keyframe, Keyframes};`,
`use crate::css::value::{IterationCount, Keyword, Value};`,
`use std::rc::Rc;`):

```rust
/// Which side of its active interval an animation is on.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Still inside `animation-delay`.
    Before,
    /// Running.
    Active,
    /// Every iteration finished.
    After,
}

/// One running `@keyframes` animation bound to one node.
#[derive(Clone, Debug)]
pub struct ActiveAnimation {
    /// The computed `animation-*` longhands driving it.
    pub spec: AnimationSpec,
    /// The `@keyframes` name, kept so a restyle can recognise the same
    /// animation and let it keep running instead of restarting it.
    pub name: Rc<str>,
    keyframes: Rc<Keyframes>,
    props: Vec<Prop>,
    /// Clock milliseconds at which this animation's delay began.
    start_ms: f64,
}

/// CSS's `animation-direction` applied to one iteration's progress.
#[must_use]
pub fn direction_progress(direction: Keyword, iteration: u64, q: f32) -> f32 {
    match direction {
        Keyword::Reverse => 1.0 - q,
        Keyword::Alternate => {
            if iteration % 2 == 1 {
                1.0 - q
            } else {
                q
            }
        }
        Keyword::AlternateReverse => {
            if iteration % 2 == 1 {
                q
            } else {
                1.0 - q
            }
        }
        // `normal`, and anything the cascade could not make sense of.
        _ => q,
    }
}

impl ActiveAnimation {
    /// Bind `keyframes` to `spec` and start the clock at `now_ms`.
    #[must_use]
    pub fn start(
        spec: AnimationSpec,
        name: Rc<str>,
        keyframes: Rc<Keyframes>,
        now_ms: f64,
    ) -> ActiveAnimation {
        let props = keyframes.properties();
        ActiveAnimation {
            spec,
            name,
            keyframes,
            props,
            start_ms: now_ms,
        }
    }

    /// The longhands this animation writes.
    #[must_use]
    pub fn props(&self) -> &[Prop] {
        &self.props
    }

    fn duration_ms(&self) -> f64 {
        let ms = f64::from(self.spec.duration.as_secs_f32()) * 1000.0;
        if ms.is_finite() { ms.max(0.0) } else { 0.0 }
    }

    fn delay_ms(&self) -> f64 {
        let ms = f64::from(self.spec.delay.as_secs_f32()) * 1000.0;
        if ms.is_finite() { ms } else { 0.0 }
    }

    fn iterations(&self) -> f64 {
        match self.spec.iterations {
            IterationCount::Infinite => f64::INFINITY,
            IterationCount::Count(n) => {
                let n = f64::from(n);
                if n.is_nan() { 0.0 } else { n.max(0.0) }
            }
        }
    }

    fn total_ms(&self) -> f64 {
        let iterations = self.iterations();
        if iterations.is_infinite() {
            f64::INFINITY
        } else {
            self.duration_ms() * iterations
        }
    }

    /// Milliseconds since this animation's delay began.
    ///
    /// Task 6b makes this freeze while `animation-play-state: paused`.
    fn local_ms(&self, now_ms: f64) -> f64 {
        now_ms - self.start_ms
    }

    /// Milliseconds since the *active* interval began; negative during delay.
    fn active_ms(&self, now_ms: f64) -> f64 {
        self.local_ms(now_ms) - self.delay_ms()
    }

    /// Which side of its active interval this animation is on at `now_ms`.
    #[must_use]
    pub fn phase(&self, now_ms: f64) -> Phase {
        let active = self.active_ms(now_ms);
        if !active.is_finite() || active < 0.0 {
            return Phase::Before;
        }
        let duration = self.duration_ms();
        if duration <= 0.0 || active >= self.total_ms() {
            Phase::After
        } else {
            Phase::Active
        }
    }

    /// `(iteration index, progress through it)` at `now_ms`, or `None` when
    /// the animation is not affecting the style at all.
    ///
    /// Task 6b widens the `Before`/`After` arms, which fill modes turn from
    /// "no contribution" into "hold the first/last value".
    #[must_use]
    pub fn iteration_progress(&self, now_ms: f64) -> Option<(u64, f32)> {
        match self.phase(now_ms) {
            Phase::Before | Phase::After => None,
            Phase::Active => {
                let duration = self.duration_ms();
                let active = self.active_ms(now_ms);
                let index = (active / duration).floor();
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let index = if index.is_finite() { index.max(0.0) as u64 } else { 0 };
                #[allow(clippy::cast_possible_truncation)]
                let progress =
                    (((active - (index as f64) * duration) / duration).clamp(0.0, 1.0)) as f32;
                Some((index, progress))
            }
        }
    }

    /// Write this animation's contribution for `now_ms` into `out`.
    ///
    /// `base` is the underlying, non-animated computed style: it supplies the
    /// values for keyframe endpoints the `@keyframes` rule omits.
    pub fn sample(&self, now_ms: f64, base: &ComputedStyle, out: &mut Overrides) {
        let Some((iteration, progress)) = self.iteration_progress(now_ms) else {
            return;
        };
        let q = direction_progress(self.spec.direction, iteration, progress);
        for &prop in &self.props {
            let Some((from, to, t, timing)) =
                resolve_segment(&self.keyframes.frames, prop, q, base)
            else {
                continue;
            };
            let timing = timing.unwrap_or(self.spec.timing);
            out.set(prop, interpolate_prop(prop, &from, &to, timing.eval(t)));
        }
    }

    /// Whether this animation still needs frames.
    #[must_use]
    pub fn is_active(&self, now_ms: f64) -> bool {
        if self.props.is_empty() {
            return false;
        }
        !matches!(self.phase(now_ms), Phase::After)
    }

    /// The earliest instant at which [`sample`](Self::sample) could differ.
    /// `f64::INFINITY` when this animation will never change again.
    #[must_use]
    pub fn next_change_ms(&self, now_ms: f64) -> f64 {
        match self.phase(now_ms) {
            Phase::Before => self.start_ms + self.delay_ms(),
            Phase::Active => now_ms,
            Phase::After => f64::INFINITY,
        }
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib anim::keyframes`
Expected: PASS — 12 tests (Task 5's 8 plus these 4).

Run: `cargo clippy -p icedtea-ui --all-targets -- -D warnings && cargo fmt --all --check`
Expected: no output, exit 0.

- [ ] **Step 5: Commit**

```bash
git add ui/src/anim/keyframes.rs
git commit -m "$(cat <<'EOF'
feat(ui): run one @keyframes animation with phases, iterations and direction

Iteration index and local progress come off the manual clock exactly, so
`alternate` at 1250 ms of a 1000 ms animation is 0.75 by arithmetic rather
than by tolerance, and infinite animations never reach the after phase.
Fill modes and play state land next.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 6b: `ActiveAnimation` — fill modes, play state, rebinding

**Files:**
- Modify: `ui/src/anim/keyframes.rs` (widen `iteration_progress`, add one field and four methods; extend the test module)

**Interfaces:**
- Consumes: everything Task 6a produced, plus `crate::css::value::Keyword`
  (`Forwards`, `Backwards`, `Both`, `None`, `Running`, `Paused`) and
  `IterationCount`.
- Produces:
  - `pub fn rebind(&mut self, spec: AnimationSpec, keyframes: Rc<Keyframes>)`
  - `pub fn apply_play_state(&mut self, now_ms: f64)`
  - a `paused_at: Option<f64>` field on `ActiveAnimation` (private)
  - `fn end_iteration(&self) -> (u64, f32)` (private)
  - fill-aware behaviour for the existing `iteration_progress`, and
    pause-aware behaviour for the existing `local_ms`, `is_active` and
    `next_change_ms`. **No signature changes.**

- [ ] **Step 1: Write the failing test**

Append to `ui/src/anim/keyframes.rs`'s test module, below Task 6a's tests
(they share `fade()`, `spec()`, `opacity_at()` and `base()`):

```rust
    // THE GATE (spec §7: `fill-mode: forwards`).
    // Mutation check: treat every fill mode as filling and the
    // `Keyword::None` case starts reporting 1.0 after the end.
    #[test]
    fn fill_mode_decides_whether_the_end_value_sticks() {
        let base = base();
        let none = ActiveAnimation::start(
            spec(200.0, 0.0, IterationCount::Count(1.0), Keyword::Normal, Keyword::None),
            Rc::from("fade"),
            fade(),
            0.0,
        );
        assert_eq!(opacity_at(&none, 100.0, &base), Some(0.5));
        assert_eq!(
            opacity_at(&none, 500.0, &base),
            None,
            "`fill-mode: none` stops overriding once the animation ends"
        );

        let forwards = ActiveAnimation::start(
            spec(200.0, 0.0, IterationCount::Count(1.0), Keyword::Normal, Keyword::Forwards),
            Rc::from("fade"),
            fade(),
            0.0,
        );
        assert_eq!(opacity_at(&forwards, 500.0, &base), Some(1.0));
    }

    // Mutation check: apply the backwards fill during the delay unconditionally
    // and the `Keyword::None` case reports 0.0 instead of None at 50 ms.
    #[test]
    fn fill_mode_backwards_applies_the_start_value_during_the_delay() {
        let base = base();
        let none = ActiveAnimation::start(
            spec(200.0, 100.0, IterationCount::Count(1.0), Keyword::Normal, Keyword::None),
            Rc::from("fade"),
            fade(),
            0.0,
        );
        assert_eq!(opacity_at(&none, 50.0, &base), None);
        assert_eq!(opacity_at(&none, 200.0, &base), Some(0.5), "100 ms into a 200 ms run");

        let backwards = ActiveAnimation::start(
            spec(200.0, 100.0, IterationCount::Count(1.0), Keyword::Normal, Keyword::Backwards),
            Rc::from("fade"),
            fade(),
            0.0,
        );
        assert_eq!(opacity_at(&backwards, 50.0, &base), Some(0.0));
        assert_eq!(backwards.phase(50.0), Phase::Before);
    }

    // Mutation check: round the fractional iteration count up and the
    // end progress becomes 1.0 instead of 0.5.
    #[test]
    fn a_fractional_iteration_count_ends_part_way_through_its_last_iteration() {
        let base = base();
        let anim = ActiveAnimation::start(
            spec(1000.0, 0.0, IterationCount::Count(2.5), Keyword::Normal, Keyword::Forwards),
            Rc::from("fade"),
            fade(),
            0.0,
        );
        assert_eq!(opacity_at(&anim, 5000.0, &base), Some(0.5));
    }

    // Mutation check: ignore `paused_at` in `local_ms` and the paused sample
    // advances with the clock instead of freezing.
    #[test]
    fn play_state_paused_freezes_the_animation_where_it_stood() {
        let base = base();
        let mut anim = ActiveAnimation::start(
            spec(1000.0, 0.0, IterationCount::Infinite, Keyword::Normal, Keyword::None),
            Rc::from("fade"),
            fade(),
            0.0,
        );
        assert_eq!(opacity_at(&anim, 250.0, &base), Some(0.25));

        anim.spec.play_state = Keyword::Paused;
        anim.apply_play_state(250.0);
        assert_eq!(opacity_at(&anim, 900.0, &base), Some(0.25), "frozen while paused");
        assert!(!anim.is_active(900.0), "a paused animation asks for no frames");

        anim.spec.play_state = Keyword::Running;
        anim.apply_play_state(900.0);
        assert_eq!(
            opacity_at(&anim, 1150.0, &base),
            Some(0.5),
            "resuming continues from 250 ms of local time, not from 900 ms"
        );
    }

    // Mutation check: have `rebind` reset `start_ms` to the restyle time and
    // the resumed sample restarts at 0.0 instead of continuing at 0.5.
    #[test]
    fn rebinding_an_animation_keeps_it_running_from_where_it_was() {
        let base = base();
        let mut anim = ActiveAnimation::start(
            spec(1000.0, 0.0, IterationCount::Infinite, Keyword::Normal, Keyword::None),
            Rc::from("fade"),
            fade(),
            0.0,
        );
        anim.rebind(
            spec(1000.0, 0.0, IterationCount::Infinite, Keyword::Normal, Keyword::None),
            fade(),
        );
        assert_eq!(opacity_at(&anim, 500.0, &base), Some(0.5));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib anim::keyframes`
Expected: FAIL to compile with
``error[E0599]: no method named `apply_play_state` found for struct `ActiveAnimation` ``
and ``error[E0599]: no method named `rebind` found for struct `ActiveAnimation` ``.
Once those two methods exist, `fill_mode_decides_whether_the_end_value_sticks`
fails on `Some(1.0)`, because Task 6a's `iteration_progress` returns `None` in
the `After` phase regardless of fill mode.

- [ ] **Step 3: Write minimal implementation**

Three edits inside `ui/src/anim/keyframes.rs`, all within Task 6a's items.

**(a) Add the pause bookkeeping field** to `struct ActiveAnimation`, after
`start_ms`, and `paused_at: None,` to the struct literal in
`ActiveAnimation::start`:

```rust
    /// Local milliseconds at which `animation-play-state: paused` froze it.
    ///
    /// `Some` also means "asks for no more frames": a paused animation's
    /// sample cannot change until something un-pauses it.
    paused_at: Option<f64>,
```

**(b) Replace** `local_ms`, `iteration_progress`, `is_active` and
`next_change_ms` with these pause- and fill-aware versions, and add
`end_iteration` next to them:

```rust
    /// Milliseconds since this animation's delay began, frozen while paused.
    fn local_ms(&self, now_ms: f64) -> f64 {
        self.paused_at.unwrap_or(now_ms - self.start_ms)
    }

    /// The final `(iteration, progress)` an animation rests at.
    fn end_iteration(&self) -> (u64, f32) {
        let iterations = self.iterations();
        if !iterations.is_finite() {
            // Unreachable in practice: an infinite animation is never `After`.
            return (u64::MAX, 1.0);
        }
        if iterations <= 0.0 {
            return (0, 0.0);
        }
        let whole = iterations.floor();
        let fraction = iterations - whole;
        if fraction <= 0.0 {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let index = (whole as u64).saturating_sub(1);
            (index, 1.0)
        } else {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let index = whole as u64;
            #[allow(clippy::cast_possible_truncation)]
            (index, fraction as f32)
        }
    }

    /// `(iteration index, progress through it)` at `now_ms`, or `None` when
    /// the animation is not affecting the style at all (outside its active
    /// interval with an `animation-fill-mode` that does not fill that side).
    #[must_use]
    pub fn iteration_progress(&self, now_ms: f64) -> Option<(u64, f32)> {
        match self.phase(now_ms) {
            Phase::Before => matches!(self.spec.fill, Keyword::Backwards | Keyword::Both)
                .then_some((0, 0.0)),
            Phase::After => matches!(self.spec.fill, Keyword::Forwards | Keyword::Both)
                .then(|| self.end_iteration()),
            Phase::Active => {
                let duration = self.duration_ms();
                let active = self.active_ms(now_ms);
                let index = (active / duration).floor();
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let index = if index.is_finite() { index.max(0.0) as u64 } else { 0 };
                #[allow(clippy::cast_possible_truncation)]
                let progress =
                    (((active - (index as f64) * duration) / duration).clamp(0.0, 1.0)) as f32;
                Some((index, progress))
            }
        }
    }

    /// Whether this animation still needs frames.
    #[must_use]
    pub fn is_active(&self, now_ms: f64) -> bool {
        if self.paused_at.is_some() || self.props.is_empty() {
            return false;
        }
        !matches!(self.phase(now_ms), Phase::After)
    }

    /// The earliest instant at which [`sample`](Self::sample) could differ.
    /// `f64::INFINITY` when this animation will never change again.
    #[must_use]
    pub fn next_change_ms(&self, now_ms: f64) -> f64 {
        if self.paused_at.is_some() {
            return f64::INFINITY;
        }
        match self.phase(now_ms) {
            Phase::Before => self.start_ms + self.delay_ms(),
            Phase::Active => now_ms,
            Phase::After => f64::INFINITY,
        }
    }
```

**(c) Add** `rebind` and `apply_play_state`, at the end of the
`impl ActiveAnimation` block:

```rust
    /// Adopt a new spec and keyframe set without restarting: a restyle that
    /// leaves `animation-name` alone must not rewind a running animation.
    pub fn rebind(&mut self, spec: AnimationSpec, keyframes: Rc<Keyframes>) {
        self.props = keyframes.properties();
        self.keyframes = keyframes;
        self.spec = spec;
    }

    /// Freeze or resume according to the spec's `animation-play-state`.
    ///
    /// Resuming shifts `start_ms` so the animation continues from the local
    /// time it was frozen at rather than jumping forward by the pause.
    pub fn apply_play_state(&mut self, now_ms: f64) {
        if self.spec.play_state == Keyword::Paused {
            if self.paused_at.is_none() {
                self.paused_at = Some(now_ms - self.start_ms);
            }
        } else if let Some(local) = self.paused_at.take() {
            self.start_ms = now_ms - local;
        }
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib anim::keyframes`
Expected: PASS — 17 tests.

Run: `cargo clippy -p icedtea-ui --all-targets -- -D warnings && cargo fmt --all --check`
Expected: no output, exit 0.

- [ ] **Step 5: Commit**

```bash
git add ui/src/anim/keyframes.rs
git commit -m "$(cat <<'EOF'
feat(ui): honour animation-fill-mode, animation-play-state and rebinding

fill-mode decides whether the first and last keyframe values hold outside the
active interval, including the part-way rest point of a fractional
iteration-count. A paused animation freezes at the local time it stopped,
asks for no frames, and resumes from there rather than jumping the pause.
rebind adopts a new spec and keyframe set without rewinding, which is what a
restyle that leaves animation-name alone must do.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 7a: `AnimationState` — the restyle diff over transitions

> Tasks 7a and 7b together are what an earlier draft called "Task 7". They are
> split so each stays inside this plan's ~400-line-per-task sizing ceiling:
> 7a builds `AnimationState` and the transition half of `restyle`/`sample`;
> 7b adds the `@keyframes` binding and the animation-over-transition
> precedence. The shared test fixtures (`style_of`, `number`,
> `FADE_ON_HOVER`) land here in 7a, and every cross-reference elsewhere in
> this plan has been repointed at whichever half it actually means, so no task
> refers to a bare "Task 7".

**Files:**
- Modify: `ui/src/anim/mod.rs` (append `AnimationState` above the test module; extend the test module)

**Interfaces:**
- Consumes:
  - `crate::anim::transition::{Transition, transitioned_props, combined_ms}` (Tasks 3-4).
  - `crate::css::cascade::CompiledSheet` with `pub fn compile(css: &str) -> Self` (contract §4).
  - `crate::css::computed::{ComputedStyle, ResolveEnv}` with `pub fn raw(&self, prop: Prop) -> &Value`, `pub fn resolve_chain(sheet: &CompiledSheet, node: &Node, env: &ResolveEnv, cx: &mut MatchCx) -> ComputedStyle`, `pub fn transition_specs(&self) -> Vec<TransitionSpec>` (contract §5).
  - `crate::css::node::{Node, PseudoStates}` and `crate::css::select::MatchCx` (contract §3).
- Produces:
  - `pub struct AnimationState`
  - `pub fn new() -> AnimationState` + `impl Default for AnimationState`
  - `pub fn restyle(&mut self, old: Option<&ComputedStyle>, new: &ComputedStyle, now: Duration, sheet: &CompiledSheet)` (the `sheet` argument is unused until Task 7b — it is in the contract's frozen signature, so it is present from the start)
  - `pub fn sample(&mut self, now: Duration) -> Overrides`
  - `pub fn is_active(&self, now: Duration) -> bool`
  - `pub fn next_deadline(&self, now: Duration) -> Option<Duration>`
  - `pub fn transition_count(&self) -> usize` (test observability — see deviation 7)

- [ ] **Step 1: Write the failing test**

Append to `ui/src/anim/mod.rs`'s test module (keeping the two tests already
there), together with these shared fixtures which Task 7b and later tasks
also use:

```rust
    use super::{AnimationState, ManualClock, Clock};
    use crate::css::cascade::CompiledSheet;
    use crate::css::computed::{ComputedStyle, ResolveEnv};
    use crate::css::node::{Node, PseudoStates};
    use crate::css::select::MatchCx;

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

        assert_eq!(number(&state.sample(Duration::from_millis(0)), Prop::Opacity), Some(1.0));
        assert_eq!(number(&state.sample(Duration::from_millis(100)), Prop::Opacity), Some(0.5));
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
```

Also add `use std::time::Duration;` and `use crate::css::value::Value;` /
`use crate::css::registry::Prop;` to the test module's imports if they are not
already there from Task 2.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib anim::tests`
Expected: FAIL to compile with `error[E0432]: unresolved import `super::AnimationState``.

- [ ] **Step 3: Write minimal implementation**

Insert into `ui/src/anim/mod.rs`, after `interpolate_prop` and before the test
module, and extend the file's imports with
`use crate::anim::transition::{Transition, combined_ms, transitioned_props};`,
`use crate::css::cascade::CompiledSheet;`,
`use crate::css::computed::ComputedStyle;`:

```rust
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
        self.transitions
            .retain(|running| governed.binary_search_by_key(&running.prop, |(p, _)| *p).is_ok());

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
```

`Transition::reverse` does not exist yet — Task 8 adds it. Until then this
task's build fails on that one call. **Write Task 8's `reverse` method now**
(the code is in Task 8, Step 3) so this task compiles, and let Task 8's tests
be the ones that pin its behaviour. This is the single place in this plan
where an implementation lands one task before its test; the alternative is a
stub, which this plan does not permit.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib anim::`
Expected: PASS — all `anim::` tests (22 by this point).

Run: `cargo clippy -p icedtea-ui --all-targets -- -D warnings && cargo fmt --all --check`
Expected: no output, exit 0.

- [ ] **Step 5: Commit**

```bash
git add ui/src/anim/mod.rs ui/src/anim/transition.rs
git commit -m "$(cat <<'EOF'
feat(ui): add AnimationState -- the restyle diff over transitions

restyle diffs the old and new computed styles over exactly the longhands the
new style says it transitions, starting, retargeting or cancelling one
Transition per property; sample prunes the finished ones so a completed
transition never overrides the computed style with its own value.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 7b: `AnimationState` — `@keyframes` binding and precedence

**Files:**
- Modify: `ui/src/anim/mod.rs` (add two fields, one private method, and the animation arms of `restyle`/`sample`/`is_active`/`next_deadline`; extend the test module)

**Interfaces:**
- Consumes:
  - Everything Task 7a produced.
  - `crate::anim::keyframes::ActiveAnimation` (Tasks 6a-6b).
  - `CompiledSheet::keyframes(&self, name: &str) -> Option<&Rc<Keyframes>>` (contract §4).
  - `ComputedStyle::animation_specs(&self) -> Vec<AnimationSpec>` (contract §5).
  - `crate::css::value::AnimationName`.
- Produces:
  - `pub fn animation_count(&self) -> usize` (test observability — see deviation 7)
  - `fn update_animations(&mut self, new: &ComputedStyle, now_ms: f64, sheet: &CompiledSheet)` (private)
  - two private `AnimationState` fields: `animations: Vec<ActiveAnimation>`, `base: Option<ComputedStyle>`
  - **No signature changes** to `restyle`, `sample`, `is_active` or
    `next_deadline`.

- [ ] **Step 1: Write the failing test**

Append to `ui/src/anim/mod.rs`'s test module, below Task 7a's tests (they
share `style_of` and `number`):

```rust
    // Mutation check: sample animations before transitions and the 100 ms
    // reading becomes the transition's 0.5.
    #[test]
    fn an_animation_overrides_a_transition_on_the_same_property() {
        let css = "\
@keyframes fade { from { opacity: 0; } to { opacity: 1; } }
window { background-color: rgb(255 255 255); }
button { opacity: 1; transition: opacity 200ms linear; animation: fade 1000ms linear; }
button:hover { opacity: 0; }
";
        let sheet = CompiledSheet::compile(css);
        assert!(sheet.keyframes("fade").is_some(), "the sheet must carry @keyframes fade");
        let normal = style_of(&sheet, PseudoStates::default());
        let hover = style_of(&sheet, PseudoStates::HOVER);

        let mut state = AnimationState::new();
        state.restyle(None, &normal, Duration::ZERO, &sheet);
        assert_eq!(state.animation_count(), 1);
        state.restyle(Some(&normal), &hover, Duration::ZERO, &sheet);

        let sampled = state.sample(Duration::from_millis(100));
        assert_eq!(
            number(&sampled, Prop::Opacity),
            Some(0.1),
            "0.1 is the animation's value at 100 ms of 1000 ms; 0.5 would be \
             the transition's, which the animation must override"
        );
    }

    // Mutation check: rebuild `animations` unconditionally in
    // `update_animations` and the resumed sample restarts at 0.0.
    #[test]
    fn a_restyle_that_keeps_the_animation_name_does_not_restart_it() {
        let css = "\
@keyframes fade { from { opacity: 0; } to { opacity: 1; } }
window { background-color: rgb(255 255 255); }
button { opacity: 1; animation: fade 1000ms linear; }
button:hover { color: rgb(255 0 0); }
";
        let sheet = CompiledSheet::compile(css);
        let normal = style_of(&sheet, PseudoStates::default());
        let hover = style_of(&sheet, PseudoStates::HOVER);
        let mut state = AnimationState::new();
        state.restyle(None, &normal, Duration::ZERO, &sheet);
        state.restyle(Some(&normal), &hover, Duration::from_millis(500), &sheet);
        assert_eq!(number(&state.sample(Duration::from_millis(500)), Prop::Opacity), Some(0.5));
    }

    // Mutation check: keep animations whose name is gone from the new style
    // and the second sample still reports the animation's value.
    #[test]
    fn dropping_animation_name_stops_the_animation() {
        let css = "\
@keyframes fade { from { opacity: 0; } to { opacity: 1; } }
window { background-color: rgb(255 255 255); }
button { opacity: 1; animation: fade 1000ms linear; }
button:hover { animation-name: none; }
";
        let sheet = CompiledSheet::compile(css);
        let normal = style_of(&sheet, PseudoStates::default());
        let hover = style_of(&sheet, PseudoStates::HOVER);
        let mut state = AnimationState::new();
        state.restyle(None, &normal, Duration::ZERO, &sheet);
        assert_eq!(state.animation_count(), 1);
        state.restyle(Some(&normal), &hover, Duration::from_millis(100), &sheet);
        assert_eq!(state.animation_count(), 0);
        assert!(state.sample(Duration::from_millis(200)).is_empty());
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib anim::tests`
Expected: FAIL to compile with
``error[E0599]: no method named `animation_count` found for struct `AnimationState` ``.

- [ ] **Step 3: Write minimal implementation**

Four edits inside `ui/src/anim/mod.rs`, all within Task 7a's `AnimationState`.
Extend the file's imports with `use crate::anim::keyframes::ActiveAnimation;`
(`AnimationName` is already imported from Task 1).

**(a) Add the two fields** to `struct AnimationState`, after `transitions`,
and initialise them in `AnimationState::new` with `animations: Vec::new(),`
and `base: None,`:

```rust
    animations: Vec<ActiveAnimation>,
    /// The most recent computed style. Keyframe endpoints the `@keyframes`
    /// rule omits are taken from here.
    base: Option<ComputedStyle>,
```

**(b) Add** `animation_count` next to `transition_count`, and
`update_animations` next to `update_transitions`:

```rust
    /// How many `@keyframes` animations are currently bound.
    ///
    /// Observability for this part's own tests (deviation 7); nothing outside
    /// `anim` reads it.
    #[must_use]
    pub fn animation_count(&self) -> usize {
        self.animations.len()
    }

    /// Bind `animation-name` to the sheet's `@keyframes`, keeping animations
    /// that survive the restyle running rather than rewinding them.
    fn update_animations(&mut self, new: &ComputedStyle, now_ms: f64, sheet: &CompiledSheet) {
        let specs = new.animation_specs();
        let mut kept: Vec<ActiveAnimation> = Vec::with_capacity(specs.len());
        for spec in specs {
            let AnimationName::Named(name) = spec.name.clone() else {
                continue;
            };
            let Some(keyframes) = sheet.keyframes(&name).cloned() else {
                tracing::debug!(%name, "animation-name has no matching @keyframes rule");
                continue;
            };
            let mut animation = match self.animations.iter().position(|a| a.name == name) {
                Some(index) => {
                    let mut existing = self.animations.remove(index);
                    existing.rebind(spec, keyframes);
                    existing
                }
                None => ActiveAnimation::start(spec, name, keyframes, now_ms),
            };
            animation.apply_play_state(now_ms);
            kept.push(animation);
        }
        self.animations = kept;
    }
```

**(c) Replace** `restyle`'s body (its signature is unchanged, and `sheet` is
now genuinely used, so drop Task 7a's `let _ = sheet;` and its doc paragraph
about the unused argument):

```rust
        let now_ms = millis(now);
        self.update_transitions(old, new, now_ms);
        self.update_animations(new, now_ms, sheet);
        self.base = Some(new.clone());
```

**(d) Add the animation arms** to `sample`, `is_active` and `next_deadline`.
In `sample`, after the transition loop and before `out` is returned — and
extend its doc comment to record why the order matters:

```rust
    /// Every animated value for this frame.
    ///
    /// Transitions are written first and animations second, so an animation
    /// wins on a property both touch — CSS's precedence, expressed as write
    /// order rather than as a conditional.
```

```rust
        if let Some(base) = &self.base {
            for animation in &self.animations {
                animation.sample(now_ms, base, &mut out);
            }
        }
```

In `is_active`, extend the expression:

```rust
        self.transitions.iter().any(|t| !t.is_finished(now_ms))
            || self.animations.iter().any(|a| a.is_active(now_ms))
```

In `next_deadline`, after the transition loop and before the
`deadline.is_finite()` check:

```rust
        for animation in &self.animations {
            if animation.is_active(now_ms) {
                deadline = deadline.min(animation.next_change_ms(now_ms));
            }
        }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib anim::`
Expected: PASS — all `anim::` tests (25 by this point).

Run: `cargo clippy -p icedtea-ui --all-targets -- -D warnings && cargo fmt --all --check`
Expected: no output, exit 0.

- [ ] **Step 5: Commit**

```bash
git add ui/src/anim/mod.rs
git commit -m "$(cat <<'EOF'
feat(ui): bind animation-name to @keyframes and rank it over transitions

restyle binds animation-name to the sheet's @keyframes without rewinding an
animation that survived the restyle, and drops one whose name is gone. sample
writes transitions then animations, so "animation overrides transition" is
the write order rather than a special case.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: Reversal continuity (CSS Transitions §3.1)

**Files:**
- Modify: `ui/src/anim/transition.rs` (add `Transition::reverse`; extend the test module)

**Interfaces:**
- Consumes: `Transition`, `duration_ms_of`, `delay_ms_of` (Task 3); `TransitionSpec` (Task 1).
- Produces:
  - `pub fn reverse(&self, to: Value, spec: &TransitionSpec, now_ms: f64) -> Transition`

- [ ] **Step 1: Write the failing test**

Append to `ui/src/anim/transition.rs`'s test module:

```rust
    // THE GATE (spec §7: "reversal continuity").
    // Mutation check: set the reversed transition's `duration_ms` to the
    // spec's full duration instead of scaling it by the shortening factor,
    // and the reversed run finishes at 300 ms instead of 200 ms -- the
    // `is_finished(200.0)` assertion fails.
    #[test]
    fn reversing_half_way_through_takes_half_as_long_to_come_back() {
        let forward = opacity_transition(200.0, TimingFunction::Linear);
        // Half way: the value on screen is 0.5.
        assert_eq!(forward.value_at(100.0), Value::Number(0.5));

        // The style flips back to the value the transition started from.
        let back = forward.reverse(
            Value::Number(0.0),
            &spec(200.0, 0.0, TimingFunction::Linear),
            100.0,
        );
        assert_eq!(back.from, Value::Number(0.5), "it reverses from what is on screen");
        assert_eq!(back.to, Value::Number(0.0));
        assert_eq!(
            back.reversing_adjusted_start,
            Value::Number(1.0),
            "the anchor for a further reversal is the run's own end value"
        );
        assert!(
            (back.reversing_shortening_factor - 0.5).abs() < 1e-6,
            "half-eased progress gives a 0.5 shortening factor, got {}",
            back.reversing_shortening_factor
        );
        assert!((back.duration_ms - 100.0).abs() < 1e-6, "got {}", back.duration_ms);
        assert_eq!(back.value_at(150.0), Value::Number(0.25), "half way back");
        assert!(back.is_finished(200.0), "it must be home 100 ms after reversing");
        assert_eq!(back.value_at(200.0), Value::Number(0.0));
    }

    // Mutation check: use raw linear progress instead of the eased output in
    // the shortening factor and this factor becomes 0.5 rather than ~0.8024.
    #[test]
    fn the_shortening_factor_uses_the_eased_output_not_linear_progress() {
        let forward = opacity_transition(200.0, TimingFunction::EASE);
        let back = forward.reverse(
            Value::Number(0.0),
            &spec(200.0, 0.0, TimingFunction::EASE),
            100.0,
        );
        assert!(
            (0.79..0.81).contains(&back.reversing_shortening_factor),
            "`ease` reports 0.8024 at t = 0.5, got {}",
            back.reversing_shortening_factor
        );
    }

    // Mutation check: scale a *positive* delay by the shortening factor and
    // the start time moves from 150 to 125.
    #[test]
    fn a_positive_delay_is_not_shortened_but_a_negative_one_is() {
        let forward = opacity_transition(200.0, TimingFunction::Linear);
        let positive = forward.reverse(
            Value::Number(0.0),
            &spec(200.0, 50.0, TimingFunction::Linear),
            100.0,
        );
        assert!((positive.start_ms - 150.0).abs() < 1e-6, "got {}", positive.start_ms);

        let negative = forward.reverse(
            Value::Number(0.0),
            &spec(200.0, -50.0, TimingFunction::Linear),
            100.0,
        );
        assert!(
            (negative.start_ms - 75.0).abs() < 1e-6,
            "a negative delay is scaled by the 0.5 shortening factor, got {}",
            negative.start_ms
        );
    }

    // Mutation check: drop the clamp and a factor computed from an
    // overshooting cubic-bezier leaves 0..=1, producing a negative duration.
    #[test]
    fn the_shortening_factor_is_clamped_into_zero_to_one() {
        // cubic-bezier with y2 = 2.0 overshoots above 1 before settling.
        let overshoot = Transition::start(
            Prop::Opacity,
            Value::Number(0.0),
            Value::Number(1.0),
            &spec(200.0, 0.0, TimingFunction::CubicBezier(0.0, 0.0, 0.5, 2.0)),
            0.0,
        );
        let back = overshoot.reverse(
            Value::Number(0.0),
            &spec(200.0, 0.0, TimingFunction::CubicBezier(0.0, 0.0, 0.5, 2.0)),
            100.0,
        );
        assert!((0.0..=1.0).contains(&back.reversing_shortening_factor));
        assert!(back.duration_ms >= 0.0);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib anim::transition::tests::reversing_half_way_through_takes_half_as_long_to_come_back`
Expected: after Task 7a already added `reverse`, this passes. If `reverse` is
absent (Task 7a not yet applied), FAIL to compile with
`error[E0599]: no method named `reverse` found for struct `Transition``.

- [ ] **Step 3: Write minimal implementation**

Add to `impl Transition` in `ui/src/anim/transition.rs` (this is the method
Task 7a's `update_transitions` calls):

```rust
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
            if raw.is_finite() { raw.clamp(0.0, 1.0) } else { 1.0 }
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
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib anim::transition`
Expected: PASS — 14 tests.

Run: `cargo clippy -p icedtea-ui --all-targets -- -D warnings && cargo fmt --all --check`
Expected: no output, exit 0.

- [ ] **Step 5: Commit**

```bash
git add ui/src/anim/transition.rs
git commit -m "$(cat <<'EOF'
test(ui): pin CSS Transitions 3.1 reversal continuity

Reversing half way through a 200 ms transition comes back in 100 ms, from
the value actually on screen, with the run's own end value as the anchor for
a further reversal. The shortening factor uses the eased output, not linear
progress -- `ease` reversed at t = 0.5 shortens to 0.8024, not 0.5.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 9: Delays, zero durations, discrete switches, cancellation, retargeting

**Files:**
- Modify: `ui/src/anim/mod.rs` (test module only — the behaviour is already implemented by Tasks 3, 7a and 8; this task proves it and fixes whatever it catches)

**Interfaces:**
- Consumes: `AnimationState`, `Overrides`, `style_of`, `number` (Task 7a);
  `crate::css::registry::Prop`; `crate::css::value::Value`.
- Produces: no new public items. If any assertion fails, the fix goes into
  `update_transitions` / `Transition` and is committed with the test.

- [ ] **Step 1: Write the failing test**

Append to `ui/src/anim/mod.rs`'s test module:

```rust
    // Mutation check: gate transition construction behind
    // `combined_ms(spec) > 0.0` and `transition_count` reports 0 here --
    // the contract asks for the transition to be *constructed* and complete
    // instantly, not special-cased away.
    #[test]
    fn a_zero_duration_transition_is_constructed_and_completes_at_once() {
        let css = "\
window { background-color: rgb(255 255 255); }
button { opacity: 1; transition: opacity 0s linear; }
button:hover { opacity: 0; }
";
        let sheet = CompiledSheet::compile(css);
        let normal = style_of(&sheet, PseudoStates::default());
        let hover = style_of(&sheet, PseudoStates::HOVER);
        let mut state = AnimationState::new();
        state.restyle(None, &normal, Duration::ZERO, &sheet);
        state.restyle(Some(&normal), &hover, Duration::ZERO, &sheet);
        assert_eq!(state.transition_count(), 1, "it is constructed");
        assert!(
            state.sample(Duration::ZERO).is_empty(),
            "and finished on the frame it was created"
        );
        assert!(!state.is_active(Duration::ZERO));
    }

    // Mutation check: start interpolating at the style change instead of at
    // `start_ms` and the 50 ms sample becomes 0.75 rather than 1.0.
    #[test]
    fn a_transition_delay_holds_the_old_value_before_it_starts() {
        let css = "\
window { background-color: rgb(255 255 255); }
button { opacity: 1; transition: opacity 200ms linear 100ms; }
button:hover { opacity: 0; }
";
        let sheet = CompiledSheet::compile(css);
        let normal = style_of(&sheet, PseudoStates::default());
        let hover = style_of(&sheet, PseudoStates::HOVER);
        let mut state = AnimationState::new();
        state.restyle(None, &normal, Duration::ZERO, &sheet);
        state.restyle(Some(&normal), &hover, Duration::ZERO, &sheet);

        assert_eq!(number(&state.sample(Duration::from_millis(50)), Prop::Opacity), Some(1.0));
        assert_eq!(number(&state.sample(Duration::from_millis(200)), Prop::Opacity), Some(0.5));
        assert!(state.sample(Duration::from_millis(300)).is_empty());
        assert_eq!(
            state.next_deadline(Duration::from_millis(50)),
            Some(Duration::from_millis(100)),
            "inside the delay the next interesting instant is the start, not now"
        );
    }

    // Mutation check: clamp the delay non-negative in `delay_ms_of` and the
    // t = 0 sample becomes 1.0 instead of 0.5.
    #[test]
    fn a_negative_transition_delay_starts_part_way_through() {
        let css = "\
window { background-color: rgb(255 255 255); }
button { opacity: 1; transition: opacity 200ms linear -100ms; }
button:hover { opacity: 0; }
";
        let sheet = CompiledSheet::compile(css);
        let normal = style_of(&sheet, PseudoStates::default());
        let hover = style_of(&sheet, PseudoStates::HOVER);
        let mut state = AnimationState::new();
        state.restyle(None, &normal, Duration::ZERO, &sheet);
        state.restyle(Some(&normal), &hover, Duration::ZERO, &sheet);
        assert_eq!(number(&state.sample(Duration::ZERO), Prop::Opacity), Some(0.5));
        assert!(state.sample(Duration::from_millis(100)).is_empty());
    }

    // Mutation check: make `interpolate_prop` fall back to the *new* value
    // rather than to `discrete`, and the 99 ms sample flips early.
    #[test]
    fn a_non_animatable_property_named_explicitly_switches_at_the_half_way_point() {
        let css = "\
window { background-color: rgb(255 255 255); }
button { font-family: \"Alpha\"; transition: font-family 200ms linear; }
button:hover { font-family: \"Beta\"; }
";
        let sheet = CompiledSheet::compile(css);
        let normal = style_of(&sheet, PseudoStates::default());
        let hover = style_of(&sheet, PseudoStates::HOVER);
        assert_ne!(normal.raw(Prop::FontFamily), hover.raw(Prop::FontFamily));

        let mut state = AnimationState::new();
        state.restyle(None, &normal, Duration::ZERO, &sheet);
        state.restyle(Some(&normal), &hover, Duration::ZERO, &sheet);

        let early = state.sample(Duration::from_millis(99));
        assert_eq!(early.get(Prop::FontFamily), Some(normal.raw(Prop::FontFamily)));
        let late = state.sample(Duration::from_millis(100));
        assert_eq!(late.get(Prop::FontFamily), Some(hover.raw(Prop::FontFamily)));
    }

    // Mutation check: drop the `retain` that prunes transitions no longer
    // governed and the third style leaves the opacity transition running.
    #[test]
    fn a_property_leaving_transition_property_stops_transitioning_at_once() {
        let css = "\
window { background-color: rgb(255 255 255); }
button { opacity: 1; transition: opacity 200ms linear; }
button:hover { opacity: 0; }
button:active { opacity: 0.5; transition-property: none; }
";
        let sheet = CompiledSheet::compile(css);
        let normal = style_of(&sheet, PseudoStates::default());
        let hover = style_of(&sheet, PseudoStates::HOVER);
        let active = style_of(&sheet, PseudoStates::ACTIVE);
        let mut state = AnimationState::new();
        state.restyle(None, &normal, Duration::ZERO, &sheet);
        state.restyle(Some(&normal), &hover, Duration::ZERO, &sheet);
        assert_eq!(state.transition_count(), 1);
        state.restyle(Some(&hover), &active, Duration::from_millis(100), &sheet);
        assert_eq!(state.transition_count(), 0);
        assert!(state.sample(Duration::from_millis(100)).is_empty());
    }

    // Mutation check: treat every mid-flight change as a reversal and this
    // run comes back shortened (finishing at 200 ms), failing the 250 ms
    // assertion which expects it still in flight.
    #[test]
    fn retargeting_to_a_third_value_restarts_from_what_is_on_screen() {
        let css = "\
window { background-color: rgb(255 255 255); }
button { opacity: 1; transition: opacity 200ms linear; }
button:hover { opacity: 0; }
button:active { opacity: 0.25; }
";
        let sheet = CompiledSheet::compile(css);
        let normal = style_of(&sheet, PseudoStates::default());
        let hover = style_of(&sheet, PseudoStates::HOVER);
        let active = style_of(&sheet, PseudoStates::ACTIVE);
        let mut state = AnimationState::new();
        state.restyle(None, &normal, Duration::ZERO, &sheet);
        state.restyle(Some(&normal), &hover, Duration::ZERO, &sheet);
        assert_eq!(number(&state.sample(Duration::from_millis(100)), Prop::Opacity), Some(0.5));

        // 0.25 is neither the running transition's end (0.0) nor its
        // reversing anchor (1.0), so this is a retarget, not a reversal.
        state.restyle(Some(&hover), &active, Duration::from_millis(100), &sheet);
        assert_eq!(state.transition_count(), 1);
        assert_eq!(
            number(&state.sample(Duration::from_millis(200)), Prop::Opacity),
            Some(0.375),
            "half way from the 0.5 on screen to 0.25, over a fresh 200 ms"
        );
        assert!(
            state.is_active(Duration::from_millis(250)),
            "a retarget runs the full duration; it is not shortened"
        );
        assert_eq!(number(&state.sample(Duration::from_millis(300)), Prop::Opacity), None);
    }

    // Mutation check: skip the "already heading there" check in
    // `update_transitions` and a repeated restyle to the same target restarts
    // the run, so the 150 ms sample becomes 0.25 instead of 0.25's successor
    // 0.5 -- i.e. the run visibly stretches.
    #[test]
    fn restyling_to_the_target_it_is_already_heading_for_does_not_restart_it() {
        let sheet = CompiledSheet::compile(FADE_ON_HOVER);
        let normal = style_of(&sheet, PseudoStates::default());
        let hover = style_of(&sheet, PseudoStates::HOVER);
        let mut state = AnimationState::new();
        state.restyle(None, &normal, Duration::ZERO, &sheet);
        state.restyle(Some(&normal), &hover, Duration::ZERO, &sheet);
        // A restyle that changes nothing about opacity's target.
        state.restyle(Some(&hover), &hover, Duration::from_millis(100), &sheet);
        assert_eq!(
            number(&state.sample(Duration::from_millis(150)), Prop::Opacity),
            Some(0.25),
            "still on the original 0..200 ms timeline"
        );
        assert!(state.sample(Duration::from_millis(200)).is_empty());
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib anim::tests`
Expected: at least
`a_non_animatable_property_named_explicitly_switches_at_the_half_way_point`
and `a_transition_delay_holds_the_old_value_before_it_starts` must be run and
seen to pass or fail. If everything already passes, **deliberately break one
line** to confirm the test is wired: change `Transition::progress`'s clamp to
`.clamp(0.0, 0.5)`, re-run, and see
`a_hover_change_transitions_from_the_old_value_to_the_new_one` fail with
`assertion `left == right` failed: left: Some(0.25), right: Some(0.5)`, then
revert.

- [ ] **Step 3: Write minimal implementation**

No new implementation is expected: Tasks 3, 7 and 8 already cover these paths.
If an assertion fails, fix it in `ui/src/anim/transition.rs` or
`AnimationState::update_transitions` — the likely candidates are the
delay-phase branch of `Transition::progress` (must return `0.0`, not a
negative fraction) and the ordering of the "already heading there" check in
`update_transitions` (it must come *before* `value_at` is consulted).

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib anim::`
Expected: PASS — all `anim::` tests (39 by this point).

Run: `cargo clippy -p icedtea-ui --all-targets -- -D warnings && cargo fmt --all --check`
Expected: no output, exit 0.

- [ ] **Step 5: Commit**

```bash
git add ui/src/anim/mod.rs ui/src/anim/transition.rs
git commit -m "$(cat <<'EOF'
test(ui): pin delays, zero durations, discrete switches and retargeting

A zero-duration transition is constructed and finishes on the frame it was
made, rather than being special-cased away. A positive delay holds the old
value; a negative one starts part-way through. A non-animatable property
named explicitly in transition-property flips at the half-way point, and a
property that leaves transition-property stops immediately instead of being
left to finish.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 10: The never-panic battery

**Files:**
- Modify: `ui/src/anim/mod.rs` (test module only)

**Interfaces:**
- Consumes: everything produced by Tasks 1-9.
- Produces: no new public items.

**Note on the fuzz obligation:** P5 introduces no `ParseFn` — every value
parser belongs to P1, which carries its own never-panic battery. The
equivalent hostile-input surface here is *numeric*: durations, delays,
iteration counts and keyframe offsets all reach this module as `f32`s that a
`calc()` can make NaN or infinite. This battery is the discharge of that
obligation.

- [ ] **Step 1: Write the failing test**

Append to `ui/src/anim/mod.rs`'s test module:

```rust
    use crate::anim::keyframes::ActiveAnimation;
    use crate::anim::transition::Transition;
    use crate::css::value::keyframes::{Keyframe, Keyframes};
    use crate::css::value::timing::{StepPosition, Time, TimingFunction};
    use crate::css::value::{AnimationName, IterationCount, Keyword};
    use std::rc::Rc;

    /// Every time value the engine can be handed, including the ones only a
    /// `calc()` can produce.
    const HOSTILE_TIMES: &[f32] = &[
        0.0,
        -0.0,
        1e-9,
        -1e-9,
        1.0,
        -1.0,
        1e30,
        -1e30,
        f32::MAX,
        f32::MIN,
        f32::EPSILON,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::NAN,
    ];

    const HOSTILE_PROGRESS: &[f32] = &[
        0.0,
        1.0,
        0.5,
        -1.0,
        2.0,
        1e30,
        -1e30,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::NAN,
    ];

    const HOSTILE_CLOCKS: &[f64] = &[
        0.0,
        -1.0,
        1.0,
        1e15,
        -1e15,
        f64::MAX,
        f64::MIN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NAN,
    ];

    fn every_timing() -> Vec<TimingFunction> {
        vec![
            TimingFunction::Linear,
            TimingFunction::EASE,
            TimingFunction::EASE_IN,
            TimingFunction::EASE_OUT,
            TimingFunction::EASE_IN_OUT,
            TimingFunction::CubicBezier(0.0, 0.0, 1.0, 1.0),
            TimingFunction::CubicBezier(0.25, 2.0, 0.75, -1.0),
            TimingFunction::CubicBezier(f32::NAN, 0.0, 1.0, f32::NAN),
            TimingFunction::Steps(1, StepPosition::JumpStart),
            TimingFunction::Steps(1, StepPosition::JumpEnd),
            TimingFunction::Steps(2, StepPosition::JumpNone),
            TimingFunction::Steps(2, StepPosition::JumpBoth),
            TimingFunction::Steps(0, StepPosition::JumpEnd),
            TimingFunction::Steps(u32::MAX, StepPosition::JumpEnd),
        ]
    }

    // Mutation check: remove the `is_finite` guard from `duration_ms_of` and
    // a NaN duration makes `progress` return NaN, which `interpolate::number`
    // turns into a NaN opacity -- the "every sample is finite" assertion
    // fails.
    #[test]
    fn no_combination_of_hostile_times_progress_and_timing_can_panic() {
        for &duration in HOSTILE_TIMES {
            for &delay in HOSTILE_TIMES {
                for timing in every_timing() {
                    let spec = TransitionSpec {
                        prop: Some(Prop::Opacity),
                        all: false,
                        duration: Time(duration),
                        delay: Time(delay),
                        timing,
                    };
                    let transition = Transition::start(
                        Prop::Opacity,
                        Value::Number(0.0),
                        Value::Number(1.0),
                        &spec,
                        0.0,
                    );
                    for &now in HOSTILE_CLOCKS {
                        let progress = transition.progress(now);
                        assert!(
                            progress.is_finite() && (0.0..=1.0).contains(&progress),
                            "progress {progress} for duration {duration} delay {delay} \
                             at {now} left 0..=1"
                        );
                        let _ = transition.eased(now);
                        let _ = transition.value_at(now);
                        let _ = transition.is_finished(now);
                        let _ = transition.reverse(Value::Number(0.5), &spec, now);
                    }
                }
            }
        }
    }

    // Mutation check: drop the NaN guard in `TimingFunction::eval`'s caller
    // and a NaN progress propagates into the sample, failing the finiteness
    // assertion below.
    #[test]
    fn every_timing_function_survives_progress_outside_zero_to_one() {
        for timing in every_timing() {
            for &t in HOSTILE_PROGRESS {
                let y = timing.eval(t);
                assert!(
                    !y.is_nan(),
                    "{timing:?}.eval({t}) returned NaN; an animated length would \
                     become NaN and poison layout"
                );
            }
        }
    }

    // Mutation check: remove the `offset.is_finite()` filter in
    // `resolve_segment` and a NaN keyframe offset makes `sort_by` see an
    // inconsistent ordering, which `total_cmp` tolerates but the bracketing
    // search does not -- the sampled value stops being finite.
    #[test]
    fn hostile_keyframe_offsets_and_iteration_counts_never_panic() {
        let base = ComputedStyle::initial(&ResolveEnv::default());
        let frames: Rc<[Keyframe]> = Rc::from(vec![
            Keyframe {
                offsets: Rc::from(&[f32::NAN, 0.0, -5.0][..]),
                declarations: Rc::from(vec![(Prop::Opacity, Value::Number(0.0))]),
                timing: None,
            },
            Keyframe {
                offsets: Rc::from(&[f32::INFINITY, 1.0, 42.0][..]),
                declarations: Rc::from(vec![(Prop::Opacity, Value::Number(1.0))]),
                timing: Some(TimingFunction::Steps(0, StepPosition::JumpEnd)),
            },
        ]);
        let keyframes = Rc::new(Keyframes {
            name: Rc::from("hostile"),
            frames,
        });

        let counts = [
            IterationCount::Infinite,
            IterationCount::Count(0.0),
            IterationCount::Count(-3.0),
            IterationCount::Count(2.5),
            IterationCount::Count(f32::NAN),
            IterationCount::Count(f32::INFINITY),
            IterationCount::Count(1e30),
        ];
        let directions = [
            Keyword::Normal,
            Keyword::Reverse,
            Keyword::Alternate,
            Keyword::AlternateReverse,
            Keyword::None,
        ];
        let fills = [
            Keyword::None,
            Keyword::Forwards,
            Keyword::Backwards,
            Keyword::Both,
            Keyword::Normal,
        ];

        for &duration in HOSTILE_TIMES {
            for &iterations in &counts {
                for &direction in &directions {
                    for &fill in &fills {
                        let spec = AnimationSpec {
                            name: AnimationName::Named(Rc::from("hostile")),
                            duration: Time(duration),
                            delay: Time(-duration),
                            timing: TimingFunction::EASE,
                            iterations,
                            direction,
                            fill,
                            play_state: Keyword::Running,
                        };
                        let animation = ActiveAnimation::start(
                            spec,
                            Rc::from("hostile"),
                            Rc::clone(&keyframes),
                            0.0,
                        );
                        for &now in HOSTILE_CLOCKS {
                            let mut out = Overrides::default();
                            animation.sample(now, &base, &mut out);
                            if let Some(Value::Number(v)) = out.get(Prop::Opacity) {
                                assert!(
                                    v.is_finite(),
                                    "sampled a non-finite opacity {v} at {now} for \
                                     duration {duration}"
                                );
                            }
                            let _ = animation.is_active(now);
                            let _ = animation.next_change_ms(now);
                            let _ = animation.phase(now);
                        }
                    }
                }
            }
        }
    }

    // Mutation check: build the deadline with `Duration::from_secs_f64`
    // without the finiteness guard and an infinite deadline panics with
    // "can not convert float seconds to Duration: value is either too big or NaN".
    #[test]
    fn next_deadline_never_panics_on_an_unreachable_deadline() {
        let css = "\
@keyframes fade { from { opacity: 0; } to { opacity: 1; } }
window { background-color: rgb(255 255 255); }
button { opacity: 1; animation: fade 1000ms linear infinite; }
";
        let sheet = CompiledSheet::compile(css);
        let normal = style_of(&sheet, PseudoStates::default());
        let mut state = AnimationState::new();
        state.restyle(None, &normal, Duration::ZERO, &sheet);
        for &now_ms in &[0u64, 1, 999, 1000, 86_400_000] {
            let now = Duration::from_millis(now_ms);
            let _ = state.next_deadline(now);
            let _ = state.is_active(now);
            let _ = state.sample(now);
        }
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib anim::tests::no_combination_of_hostile_times_progress_and_timing_can_panic`
Expected: initially FAIL — either a panic (`attempt to ...`, or
`can not convert float seconds to Duration`) or the finiteness assertion,
depending on which guard is missing.

- [ ] **Step 3: Write minimal implementation**

Harden whatever the battery catches. The guards this plan already specifies
and that must be present:

- `duration_ms_of` / `delay_ms_of` / `ActiveAnimation::duration_ms` /
  `ActiveAnimation::delay_ms`: `if ms.is_finite() { .. } else { 0.0 }`.
- `Transition::progress`: the `!(self.duration_ms > 0.0)` branch (NaN takes
  the instant-completion path) plus the explicit `fraction.is_nan()` check.
- `Transition::reverse`: `raw.is_finite()` before the clamp.
- `ActiveAnimation::iterations`: NaN becomes `0.0`, negatives clamp to `0.0`.
- `ActiveAnimation::phase`: `!active.is_finite() || active < 0.0` → `Before`.
- `resolve_segment`: `if !offset.is_finite() { continue; }` and the
  `q.is_finite()` normalisation.
- `AnimationState::next_deadline`: `if !deadline.is_finite() { return None; }`
  before `Duration::from_secs_f64`.

If the battery shows `TimingFunction::eval` itself returning NaN for a NaN
input, that is P1's code, not P5's — clamp at this module's boundary instead
of editing `css/`: in `Transition::eased`, wrap as

```rust
    #[must_use]
    pub fn eased(&self, now_ms: f64) -> f32 {
        let y = self.timing.eval(self.progress(now_ms));
        if y.is_finite() { y } else { self.progress(now_ms) }
    }
```

with a comment naming `css::value::timing` as the owner of the degenerate
input (a `cubic-bezier()` with NaN control points, which P1 parses but cannot
reject at parse time without inventing a rule the CSS spec does not state).
Mirror the same guard in `ActiveAnimation::sample` around
`timing.eval(t)`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib anim::`
Expected: PASS — all `anim::` tests (43 by this point).

Run: `cargo clippy -p icedtea-ui --all-targets -- -D warnings && cargo fmt --all --check`
Expected: no output, exit 0.

- [ ] **Step 5: Commit**

```bash
git add ui/src/anim/
git commit -m "$(cat <<'EOF'
test(ui): never-panic battery over hostile times, progress and offsets

calc() can hand this module NaN and infinite durations, delays, iteration
counts and keyframe offsets. Every public entry point survives them and
never emits a non-finite animated value: a NaN opacity or length would
poison layout several layers away from the cause.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 11: The Adwaita gate — the real theme's button transition and spinner animation

**Files:**
- Modify: `ui/src/anim/mod.rs` (test module only)

**Interfaces:**
- Consumes: `crate::BUNDLED_ADWAITA_LIGHT`; `AnimationState`, `style_of`
  (Task 7a); `crate::css::computed::FromValue`; `crate::css::value::color::Rgba`;
  `crate::css::node::{Node, PseudoStates}`.
- Produces: no new public items.

**The values this test pins, and where they come from.** In the vendored
`ui/themes/adwaita-light.css`:

- the base `button` rule (line 215) declares, in this order,
  `transition: all 200ms cubic-bezier(0.25, 0.46, 0.45, 0.94)` and then, later
  in the same block, `transition-property: outline, outline-width,
  outline-offset, outline-color; transition-duration: 300ms;` — so the
  longhands win on property and duration, the shorthand's timing function
  survives, and the computed transition is **the four `outline` longhands
  (via the `outline` shorthand's expansion), 300 ms,
  `cubic-bezier(0.25, 0.46, 0.45, 0.94)`, no delay**;
- the following rule (line 217) declares `outline: 0 solid transparent;
  outline-offset: 4px;`;
- `button:focus:focus-visible` (line 219) declares
  `outline-color: rgba(53, 132, 228, 0.5); outline-width: 2px;
  outline-offset: -2px;`.

Solving `cubic-bezier(0.25, 0.46, 0.45, 0.94)` for x = 100/300 gives the
Bézier parameter u ≈ 0.4380, and y(u) ≈ **0.5790**. So at 100 ms:

- `outline-width`: `0 + 2 × 0.5790` = **1.158 px**;
- `outline-offset`: `4 + (−6) × 0.5790` = **0.526 px**;
- `outline-color`: `transparent` → `rgba(53, 132, 228, 0.5)` interpolated in
  **premultiplied** sRGB, which is GTK's rule: alpha `0 + 0.5 × 0.5790` =
  **0.2895**, and the unpremultiplied RGB is exactly the accent's
  `(53, 132, 228)/255`. A naive *un*premultiplied lerp from transparent black
  would give red `0.5790 × 53/255 = 0.120` instead of `0.208`, which is the
  discrimination this assertion buys.

- [ ] **Step 1: Write the failing test**

Append to `ui/src/anim/mod.rs`'s test module:

```rust
    use crate::BUNDLED_ADWAITA_LIGHT;
    use crate::css::computed::FromValue;
    use crate::css::value::color::Rgba;

    /// The computed style of a `button` under Adwaita, with `states` applied.
    fn adwaita_button(sheet: &CompiledSheet, states: PseudoStates) -> ComputedStyle {
        let window = Node::with_classes("window", &["background"]);
        let button = Node::new("button");
        window.append_child(&button);
        for flag in [PseudoStates::FOCUS, PseudoStates::HOVER, PseudoStates::ACTIVE] {
            button.set_state(flag, states.contains(flag));
        }
        let mut cx = MatchCx::new();
        ComputedStyle::resolve_chain(sheet, &button, &ResolveEnv::default(), &mut cx)
    }

    // THE GATE (spec §7: "Adwaita's button `transition` sampled at 100 ms").
    //
    // Mutation check: change `transitioned_props` so a shorthand is pushed
    // verbatim instead of expanded, and `outline-width` (which Adwaita lists
    // separately) still transitions but `outline-style` does not -- so the
    // sharper mutation is to make `transitioned_props` keep the *first* spec
    // rather than the last: the duration reverts to the shorthand's 200 ms
    // and the width at 100 ms becomes ~1.44 px, outside the window below.
    #[test]
    fn adwaitas_button_outline_transition_is_exact_at_100ms() {
        let sheet = CompiledSheet::compile(BUNDLED_ADWAITA_LIGHT);
        let normal = adwaita_button(&sheet, PseudoStates::default());
        let focused = adwaita_button(&sheet, PseudoStates::FOCUS);

        // The theme's declared start and end values.
        assert_eq!(f32::from_value(normal.raw(Prop::OutlineWidth)), 0.0);
        assert_eq!(f32::from_value(focused.raw(Prop::OutlineWidth)), 2.0);
        assert_eq!(f32::from_value(normal.raw(Prop::OutlineOffset)), 4.0);
        assert_eq!(f32::from_value(focused.raw(Prop::OutlineOffset)), -2.0);

        let mut state = AnimationState::new();
        state.restyle(None, &normal, Duration::ZERO, &sheet);
        state.restyle(Some(&normal), &focused, Duration::ZERO, &sheet);

        let sampled = state.sample(Duration::from_millis(100));

        // 2 px * cubic-bezier(0.25, 0.46, 0.45, 0.94) at x = 1/3 (= 0.5790).
        let width = f32::from_value(
            sampled
                .get(Prop::OutlineWidth)
                .expect("outline-width is one of Adwaita's transitioned properties"),
        );
        assert!(
            (1.10..1.21).contains(&width),
            "outline-width at 100 ms of Adwaita's 300 ms outline transition is \
             1.158 px, got {width}"
        );

        // 4 px + (-6 px) * 0.5790 = 0.526 px.
        let offset = f32::from_value(
            sampled
                .get(Prop::OutlineOffset)
                .expect("outline-offset is transitioned"),
        );
        assert!(
            (0.45..0.62).contains(&offset),
            "outline-offset at 100 ms is 0.526 px, got {offset}"
        );

        // transparent -> rgba(53, 132, 228, 0.5), premultiplied (GTK's rule).
        let color = Rgba::from_value(
            sampled
                .get(Prop::OutlineColor)
                .expect("outline-color is transitioned"),
        );
        assert!(
            (0.27..0.31).contains(&color.a),
            "alpha at 100 ms is 0.5 * 0.5790 = 0.2895, got {}",
            color.a
        );
        assert!(
            (color.r - 53.0 / 255.0).abs() < 0.02
                && (color.g - 132.0 / 255.0).abs() < 0.02
                && (color.b - 228.0 / 255.0).abs() < 0.02,
            "premultiplied interpolation keeps the accent hue at every alpha; \
             got {color:?}, and an unpremultiplied lerp from transparent black \
             would have given r = 0.120"
        );

        // Adwaita does not transition its background, so hovering paints
        // instantly even though `transition: all` appears earlier in the rule.
        assert!(
            sampled.get(Prop::BackgroundImage).is_none(),
            "background-image is not in Adwaita's transition-property list"
        );

        assert!(state.is_active(Duration::from_millis(299)));
        assert!(state.sample(Duration::from_millis(300)).is_empty());
    }

    // Mutation check: make `update_animations` skip specs whose iteration
    // count is Infinite (an easy "guard against runaway work" mistake) and
    // `animation_count` reports 0 -- the spinner would never spin.
    #[test]
    fn adwaitas_spinner_binds_its_infinite_keyframes_animation() {
        let sheet = CompiledSheet::compile(BUNDLED_ADWAITA_LIGHT);
        assert!(
            sheet.keyframes("spin").is_some(),
            "Adwaita declares `@keyframes spin`"
        );

        let window = Node::with_classes("window", &["background"]);
        let spinner = Node::new("spinner");
        window.append_child(&spinner);
        spinner.set_state(PseudoStates::CHECKED, true);
        let mut cx = MatchCx::new();
        let style =
            ComputedStyle::resolve_chain(&sheet, &spinner, &ResolveEnv::default(), &mut cx);

        let mut state = AnimationState::new();
        state.restyle(None, &style, Duration::ZERO, &sheet);
        assert_eq!(state.animation_count(), 1);

        let sampled = state.sample(Duration::from_millis(250));
        assert!(
            sampled.get(Prop::Transform).is_some(),
            "`@keyframes spin` animates `transform`, so a quarter of the way \
             through its 1 s iteration the transform must be overridden"
        );
        assert!(
            state.is_active(Duration::from_secs(600)),
            "`infinite` means the spinner still asks for frames ten minutes in"
        );
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib anim::tests::adwaitas`
Expected: FAIL. The most likely first failure is
`outline-width is one of Adwaita's transitioned properties` panicking on the
`expect`, because `transitioned_props` has not expanded the `outline`
shorthand — which is the behaviour Task 4 added and this test proves against
the real sheet rather than a synthetic one.

- [ ] **Step 3: Write minimal implementation**

No new implementation. If a value is outside its window, work backwards in
this order before touching `anim/`:

1. print `normal.transition_specs()` and check the duration is `300 ms` and
   the timing is `CubicBezier(0.25, 0.46, 0.45, 0.94)` — a wrong duration
   means P3's list-repetition rule for `transition-duration` is off, which is
   P3's bug, not this part's;
2. print the raw start/end `Value`s — a wrong `outline-offset` start means
   the `outline: 0 solid transparent` reset in the *later* rule did not beat
   the earlier `outline-color`, again P3's;
3. only then look at `transitioned_props` and `Transition::eased`.

Record whichever of these you find in the commit message; do not "fix" it by
widening the assertion windows.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib anim::`
Expected: PASS — all `anim::` tests (45 by this point).

Run: `cargo test -p icedtea-ui`
Expected: PASS — the whole crate, including
`tests/themed_button_offscreen.rs` unchanged.

Run: `cargo clippy -p icedtea-ui --all-targets -- -D warnings && cargo fmt --all --check`
Expected: no output, exit 0.

- [ ] **Step 5: Commit**

```bash
git add ui/src/anim/mod.rs
git commit -m "$(cat <<'EOF'
test(ui): pin Adwaita's real button transition and spinner animation

The theme's base button rule sets `transition: all 200ms` and then overrides
it with `transition-property: outline, ...; transition-duration: 300ms` in
the same block, so the engine must take the later longhands and expand the
`outline` shorthand. At 100 ms the outline is 1.158 px wide at 0.526 px
offset with alpha 0.2895 -- all three derived from the cubic-bezier, and the
colour only lands on the accent hue if the interpolation is premultiplied.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 12: The `Overrides` hand-off in `Button`

**Files:**
- Modify: `ui/src/widget/button.rs` (add 4 fields to `struct Button` at ~line 22; add 4 initialisers in `Button::new` at ~line 59; add ~6 lines at the end of `Button::restyle` at ~line 80; add 4 methods after `Button::allocation` at ~line 150; change one argument in `Button::render` at ~line 155; append tests)

**Interfaces:**
- Consumes (post-P3/P4 shapes — adapt the *call sites* only if P3/P4 named
  them differently, never the `anim` API; see contract deviation 2):
  - `Button::new(label: &str, classes: &[&str], parent: Node) -> Button`
  - `Button::restyle(&mut self, sheet: &CompiledSheet, fonts: &mut FontDatabase)` —
    computes `self.style` from the cascade, reshapes the label, relayouts.
    (P3 leaves this `fonts: &FontStack`; **P4 changes it to `&mut FontDatabase`**,
    and P4 runs first, so every call site in this part's tasks and tests passes
    `&mut fonts`.)
  - `Button::render(&mut self, surface: &mut Surface, origin: (f32, f32),
    sheet: &CompiledSheet, fonts: &mut FontDatabase, overrides: Option<&Overrides>)`
    — delegates to `paint::paint_node(canvas, node, style, alloc, overrides, cx)`
    (contract §8). **P4 leaves `overrides` as a caller-supplied parameter that
    every call site passes `None` for.** Per the contract's Execution note E5, this
    part *removes* that parameter and reads `self.overrides` instead, so `render`
    becomes `render(&mut self, surface, origin, sheet, fonts)`; the two call sites
    (`wayland.rs::LayerWindow::repaint` and this part's tests) drop the argument.
    Every `button.render(&mut surface, (0.0, 0.0))` written in Tasks 12-14 below
    is shorthand for `button.render(&mut surface, (0.0, 0.0), &sheet, &mut fonts)`.
  - The test module's existing `fixture(css: &str, label: &str)` helper, which
    returns a compiled sheet, a font handle, and a restyled `Button`.
  - `crate::anim::{AnimationState, Clock, MonotonicClock, Overrides}` (Tasks 1-7).
- Produces:
  - `pub fn set_clock(&mut self, clock: Rc<dyn Clock>)`
  - `pub fn tick(&mut self) -> bool`
  - `pub fn is_animating(&self) -> bool`
  - `pub fn overrides(&self) -> &Overrides`

**Scope note.** `tick` resamples and repaints; it does **not** relayout. A
property that both animates and affects layout (`min-width`, `margin`) will
therefore animate its paint but not its allocation in M2. That is deliberate:
the widget/event model is M3's, and this part is contractually forbidden from
touching `layout.rs`. It is documented on `tick` so the limitation is visible
where someone would hit it.

- [ ] **Step 1: Write the failing test**

Append to `ui/src/widget/button.rs`'s test module:

```rust
    use crate::anim::{Clock, ManualClock};
    use std::rc::Rc;

    const FADE_BUTTON: &str = "\
window { background-color: rgb(255 255 255); }
button { opacity: 1; transition: opacity 200ms linear; }
button:hover { opacity: 0; }
";

    // Mutation check: drop the `self.anim.restyle(..)` call at the end of
    // `Button::restyle` and the hover change snaps -- `overrides()` is empty
    // and the 100 ms assertion fails.
    #[test]
    fn a_state_change_animates_through_the_widgets_own_clock() {
        let (sheet, fonts, mut button) = fixture(FADE_BUTTON, "Click me");
        let clock = Rc::new(ManualClock::new());
        button.set_clock(Rc::clone(&clock) as Rc<dyn Clock>);
        // Re-style once on the manual clock so the *next* restyle has a
        // before-change style recorded against t = 0.
        button.restyle(&sheet, &fonts);
        assert!(button.overrides().is_empty());
        assert!(!button.is_animating());

        button.set_states(PseudoStates::HOVER, &sheet, &fonts);
        assert!(button.is_animating(), "hovering starts the opacity transition");

        clock.set_ms(100);
        assert!(button.tick(), "a mid-transition tick changes the painted values");
        assert_eq!(
            button.overrides().get(Prop::Opacity),
            Some(&Value::Number(0.5))
        );

        clock.set_ms(200);
        assert!(button.tick(), "the final tick clears the override");
        assert!(button.overrides().is_empty());
        assert!(!button.is_animating());
    }

    // Mutation check: have `tick` return `true` unconditionally and the
    // Wayland pump repaints forever -- this assertion is the tripwire.
    #[test]
    fn ticking_an_idle_button_reports_no_change() {
        let (sheet, fonts, mut button) = fixture(FADE_BUTTON, "Click me");
        let clock = Rc::new(ManualClock::new());
        button.set_clock(Rc::clone(&clock) as Rc<dyn Clock>);
        button.restyle(&sheet, &fonts);
        clock.set_ms(5_000);
        assert!(!button.tick());
        assert!(!button.is_animating());
    }

    // Mutation check: keep passing `None` for `overrides` in
    // `Button::render`'s call to `paint_node` and the two surfaces come out
    // identical, failing the inequality.
    #[test]
    fn the_painted_pixels_follow_the_animated_value_not_the_computed_one() {
        let css = "\
window { background-color: rgb(255 255 255); }
button { min-width: 40px; min-height: 40px; padding: 0; border: 0 solid transparent; \
border-radius: 0; background-color: rgb(255 0 0); background-image: none; \
transition: background-color 200ms linear; }
button:hover { background-color: rgb(0 0 255); }
";
        let (sheet, fonts, mut button) = fixture(css, "");
        let clock = Rc::new(ManualClock::new());
        button.set_clock(Rc::clone(&clock) as Rc<dyn Clock>);
        button.restyle(&sheet, &fonts);
        button.set_states(PseudoStates::HOVER, &sheet, &fonts);

        let mut start = Surface::new_raster_n32_premul(60, 60).expect("surface");
        button.render(&mut start, (0.0, 0.0));

        clock.set_ms(100);
        button.tick();
        let mut middle = Surface::new_raster_n32_premul(60, 60).expect("surface");
        button.render(&mut middle, (0.0, 0.0));

        assert_ne!(
            read_pixel(&mut start, 5, 5),
            read_pixel(&mut middle, 5, 5),
            "half way through a 200 ms background-color transition the painted \
             pixel must differ from the one painted at t = 0"
        );
    }
```

If the test module has no `read_pixel` helper, add this one next to
`fixture` (it mirrors what `tests/themed_button_offscreen.rs` already does,
without touching that file):

```rust
    /// The premultiplied RGBA bytes at `(x, y)` of `surface`.
    fn read_pixel(surface: &mut Surface, x: i32, y: i32) -> [u8; 4] {
        let mut pixels = vec![0u8; 4];
        assert!(
            surface.read_pixels_rgba8(x, y, 1, 1, &mut pixels),
            "reading pixel ({x}, {y}) off the surface"
        );
        [pixels[0], pixels[1], pixels[2], pixels[3]]
    }
```

Use whatever pixel-reading helper `ui/src/paint/`'s own tests already use if
one exists — P4 owns that seam and will have settled the exact spelling of
`read_pixels_rgba8`; match it rather than inventing a second spelling.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib widget::button`
Expected: FAIL to compile with
`error[E0599]: no method named `set_clock` found for struct `Button``.

- [ ] **Step 3: Write minimal implementation**

Add to `struct Button` in `ui/src/widget/button.rs`, after the existing
cache fields:

```rust
    /// The clock this widget's transitions and animations run on. Shared with
    /// the `LayerWindow` that drives its frame callbacks, and swapped for a
    /// `ManualClock` in tests.
    clock: Rc<dyn Clock>,
    /// Everything currently animating on this widget.
    anim: AnimationState,
    /// The values `render` paints instead of the computed ones this frame.
    overrides: Overrides,
    /// Whether `restyle` has ever run. The first style is not a change, so it
    /// starts no transitions.
    styled: bool,
```

Add to `Button::new`'s struct literal:

```rust
            clock: Rc::new(MonotonicClock::new()),
            anim: AnimationState::new(),
            overrides: Overrides::default(),
            styled: false,
```

At the **end** of `Button::restyle`, after `self.style` has been assigned and
before the layout call (so the layout sees the animated values on the frame a
transition starts), insert:

```rust
        let previous = if self.styled {
            Some(std::mem::replace(&mut self.style, computed))
        } else {
            self.style = computed;
            None
        };
        self.styled = true;
        let now = self.clock.now();
        self.anim.restyle(previous.as_ref(), &self.style, now, sheet);
        self.overrides = self.anim.sample(now);
```

where `computed` is whatever local `restyle` already builds before assigning
it to `self.style`; if `restyle` assigns `self.style` directly, hoist that
expression into a `let computed = ...;` first. Nothing else in `restyle`
changes.

Add these methods after `Button::allocation`:

```rust
    /// Drive this widget's transitions and animations from `clock`.
    ///
    /// The `LayerWindow` passes its own clock in so the widget and the frame
    /// pump agree on what "now" means; tests pass a `ManualClock`.
    pub fn set_clock(&mut self, clock: Rc<dyn Clock>) {
        self.clock = clock;
    }

    /// Resample the animation clock.
    ///
    /// Returns `true` when the animated values changed and the widget must be
    /// repainted. Paint only: `tick` deliberately does not relayout, so a
    /// property that animates *and* affects layout animates its appearance
    /// but not its allocation in M2. The widget/event model that would make
    /// per-frame relayout sensible is M3's.
    pub fn tick(&mut self) -> bool {
        let sampled = self.anim.sample(self.clock.now());
        if sampled == self.overrides {
            return false;
        }
        self.overrides = sampled;
        true
    }

    /// Whether this widget still needs frames.
    #[must_use]
    pub fn is_animating(&self) -> bool {
        self.anim.is_active(self.clock.now())
    }

    /// This frame's animated values, layered over the computed style by
    /// `paint_node`.
    #[must_use]
    pub fn overrides(&self) -> &Overrides {
        &self.overrides
    }
```

In `Button::render`, drop P4's caller-supplied `overrides: Option<&Overrides>`
parameter and pass `Some(&self.overrides)` to `paint::paint_node` instead of the
parameter (contract Execution note E5). Those are the only edits to `render`;
update its two call sites (`wayland.rs::LayerWindow::repaint`, Task 13, and this
part's tests) to drop the now-absent argument.

Add to the file's imports:

```rust
use std::rc::Rc;

use crate::anim::{AnimationState, Clock, MonotonicClock, Overrides};
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib widget::button`
Expected: PASS — P3's 4 rewritten tests plus these 3.

Run: `cargo test -p icedtea-ui`
Expected: PASS, `tests/themed_button_offscreen.rs` included and unedited.

Run: `cargo clippy -p icedtea-ui --all-targets -- -D warnings && cargo fmt --all --check`
Expected: no output, exit 0.

- [ ] **Step 5: Commit**

```bash
git add ui/src/widget/button.rs
git commit -m "$(cat <<'EOF'
feat(ui): hand Overrides from the widget's AnimationState to paint

The button owns a clock and an AnimationState; restyle records the
before-change style and starts whatever the theme says should transition,
and render paints through the sampled overrides rather than the raw computed
style. tick is paint-only by design -- per-frame relayout needs the M3
widget model.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 13: The `wl_surface.frame` pump

**Files:**
- Modify: `ui/src/wayland.rs` (imports ~line 10-27; `AppState` fields ~line 93-116; `AppState::new` ~line 120; `LayerWindow::repaint` ~line 395-440; new `Dispatch` impl near the other `Dispatch` impls ~line 500; new pure helper next to `select_paint_slot` ~line 483; append tests)

**Interfaces:**
- Consumes:
  - `Button::{set_clock, tick, is_animating}` (Task 12).
  - `crate::anim::{Clock, MonotonicClock}` (Task 1).
  - `wayland_client::protocol::wl_callback` and `wl_surface::WlSurface::frame(&self, qh: &QueueHandle<D>, udata: U) -> wl_callback::WlCallback`.
- Produces:
  - `fn should_request_frame(animating: bool, frame_pending: bool) -> bool` (private, Wayland-object-free — the same pattern `select_paint_slot` already follows so the decision is unit-testable without a compositor)
  - `impl Dispatch<wl_callback::WlCallback, ()> for AppState`
  - Two new private `AppState` fields: `clock: Rc<MonotonicClock>`, `frame_pending: bool`

- [ ] **Step 1: Write the failing test**

Append to `ui/src/wayland.rs`'s test module:

```rust
    // Mutation check: drop the `!frame_pending` term and every repaint stacks
    // another frame callback on the surface, so the compositor delivers one
    // `done` per outstanding request and the client repaints in a storm.
    #[test]
    fn a_frame_callback_is_requested_only_while_animating_and_never_twice() {
        assert!(super::should_request_frame(true, false));
        assert!(
            !super::should_request_frame(true, true),
            "a callback is already outstanding: asking again multiplies the \
             `done` events the compositor will send"
        );
        assert!(
            !super::should_request_frame(false, false),
            "an idle widget must not ask for another frame -- that is the \
             whole difference between this and a busy loop"
        );
        assert!(!super::should_request_frame(false, true));
    }

    // Mutation check: initialise `frame_pending` to `true` and the very first
    // animating repaint never requests a callback, so nothing ever animates.
    #[test]
    fn a_fresh_client_state_has_no_frame_callback_outstanding() {
        let state = state();
        assert!(!state.frame_pending);
        assert!(
            state.clock.now() < std::time::Duration::from_secs(1),
            "the client's clock is created with the state, so it starts near zero"
        );
    }
```

`state()` is the `AppState` constructor M1's `wayland.rs` test module already
has — verified present on this branch, verbatim:

```rust
    fn state() -> AppState {
        let sheet = CompiledSheet::compile(BUNDLED_ADWAITA_LIGHT);
        let fonts = FontStack::system().expect("system font");
        let window = CssNode::new("window", &["background"], PseudoStates::default(), None);
        let mut button = Button::new("Click me", &[], window);
        button.restyle(&sheet, &fonts);
        AppState::new(sheet, fonts, button)
    }
```

Reuse it; do not add a second constructor. `let state = state();` shadows the
function with the binding, which is legal and is what the surrounding M1
tests already do. Only the *name* `state` is load-bearing here: if P2/P3/P4
renamed `CssNode` or `Button::restyle`, they will already have adapted this
body, and the two tests above still compile against it unchanged.

`state.clock.now()` calls a [`Clock`] trait method, so add `Clock` — and only
`Clock` — to the test module's existing `use super::{ ... };` list (the one
that already imports `AppState`, `BTN_LEFT`, `select_paint_slot`,
`wait_bounded`). `should_request_frame` stays `super::`-qualified at its call
sites above, so importing it too would be an unused import under
`-D warnings`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib wayland`
Expected: FAIL to compile with
`error[E0425]: cannot find function `should_request_frame` in module `super``
and `error[E0609]: no field `frame_pending` on type `AppState``.

- [ ] **Step 3: Write minimal implementation**

Add to the `wayland_client::protocol` import list: `wl_callback`. Add:

```rust
use std::rc::Rc;

use crate::anim::{Clock, MonotonicClock};
```

Add to `struct AppState`, after `released`:

```rust
    /// The clock the widget's transitions and animations run on. Shared with
    /// the `Button` so the pump and the widget agree on "now".
    clock: Rc<MonotonicClock>,
    /// A `wl_surface.frame` callback is outstanding.
    ///
    /// A frame callback fires once per commit that requested it, so asking
    /// again while one is pending multiplies the `done` events the compositor
    /// sends and turns a smooth animation into a repaint storm.
    frame_pending: bool,
```

In `AppState::new`, take the button by value as `mut button`, and before
building the struct:

```rust
        let clock = Rc::new(MonotonicClock::new());
        button.set_clock(Rc::clone(&clock) as Rc<dyn Clock>);
```

then add `clock,` and `frame_pending: false,` to the struct literal.

Add next to `select_paint_slot`:

```rust
/// Whether this commit should carry a `wl_surface.frame` request.
///
/// Wayland-object-free on purpose, like [`select_paint_slot`]: the decision
/// that separates "animate at the compositor's pace" from "busy loop" is
/// worth testing without a live compositor.
fn should_request_frame(animating: bool, frame_pending: bool) -> bool {
    animating && !frame_pending
}
```

In `LayerWindow::repaint`, immediately **before** `self.surface.commit();`
(the frame request is part of the surface state the commit applies):

```rust
        if should_request_frame(self.state.button.is_animating(), self.state.frame_pending) {
            self.surface.frame(&self.qh, ());
            self.state.frame_pending = true;
            tracing::trace!("requested a frame callback: the widget is animating");
        }
```

Add the dispatch, next to the other `Dispatch` impls:

```rust
impl Dispatch<wl_callback::WlCallback, ()> for AppState {
    /// A frame callback is the compositor saying "now is a good time to draw
    /// the next frame". Resample the animation clock, and keep asking for
    /// frames while anything is still running.
    ///
    /// The repaint is marked dirty even when the sampled values did not
    /// change, because a frame callback only fires for a commit that asked
    /// for one: without a repaint there is no commit, and without a commit
    /// there is no next callback, so the animation would stall.
    fn event(
        state: &mut Self,
        _callback: &wl_callback::WlCallback,
        event: wl_callback::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event {
            state.frame_pending = false;
            let changed = state.button.tick();
            if changed || state.button.is_animating() {
                state.dirty = true;
            }
        }
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib wayland`
Expected: PASS — M1's 11 tests (byte-identical, per contract §10.2) plus
these 2.

Run: `cargo test -p icedtea-ui`
Expected: PASS.

Run: `cargo clippy -p icedtea-ui --all-targets -- -D warnings && cargo fmt --all --check`
Expected: no output, exit 0.

- [ ] **Step 5: Commit**

```bash
git add ui/src/wayland.rs
git commit -m "$(cat <<'EOF'
feat(ui): drive animations from wl_surface.frame callbacks

A commit carries a frame request only while the widget is animating and only
when none is outstanding, so an idle window blocks in dispatch exactly as
before and an animating one advances at the compositor's pace rather than in
a busy loop. The decision itself is a pure function, testable without a
compositor, following select_paint_slot's precedent.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 14: The Wayland proof, and the full gate sweep

**Files:**
- Create: `ui/tests/transition_screencopy.rs`

**Interfaces:**
- Consumes:
  - `icedtea_harness::{CapturedFrame, Compositor, ScreencopyClient, VirtualPointerClient}` — `Compositor::spawn() -> Compositor` with a `socket: String` field and `output_size() -> (i32, i32)`; `ScreencopyClient::spawn(socket: &str)` with `capture() -> CapturedFrame`; `CapturedFrame { format, bytes, stride, width, height }`; `VirtualPointerClient::spawn(socket: &str)` with `motion_absolute(x: f64, y: f64, output_w: u32, output_h: u32)`, `frame()`, `pump()`.
  - `icedtea_ui::shm::pixel_rgb(format, bytes, stride, x, y) -> Option<(u8, u8, u8)>`.
  - The `themed-button` binary via `env!("CARGO_BIN_EXE_themed-button")`, with
    `ICEDTEA_UI_THEME` (a path = a whole theme file), `ICEDTEA_UI_LABEL`,
    `ICEDTEA_UI_CLASSES`, `WAYLAND_DISPLAY`.
  - `tempfile` (already a dev-dependency).
- Produces: no library items. This file is **self-contained** — it does not
  declare `mod support;`, because `support::allocation_of` parses M1's
  four-number `Allocation` and this part must not touch that file.

**Why the elapsed-time assertion.** Capturing "red before, blue after" alone
would also pass if the transition snapped instantly, since a snap also goes
red → blue. The load-bearing assertion is therefore *how long* it took: a
2000 ms `linear` transition cannot reach blue in under a second, and a snap
cannot take more than one. This asserts lateness, not an intermediate colour —
the spec's "intermediate frames not asserted on screen" rule is kept.

- [ ] **Step 1: Write the failing test**

Create `ui/tests/transition_screencopy.rs`:

```rust
//! The animation seam, end to end: a `transition: background-color` really
//! animates on a real compositor's output.
//!
//! Start and end colours are asserted; intermediate frames are not (the M2
//! spec's rule — on-screen timing is not reproducible enough to pin a colour
//! to a millisecond). What *is* asserted is that reaching the end took
//! roughly the declared duration, which is the only thing that distinguishes
//! an animation from a snap when both begin red and end blue.
//!
//! Self-contained on purpose: it does not share `tests/support/mod.rs`,
//! whose `allocation_of` helper belongs to the offscreen pixel gate.

use std::io::Write;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use icedtea_harness::{CapturedFrame, Compositor, ScreencopyClient, VirtualPointerClient};

/// A whole theme (never layered under Adwaita) declaring one long, linear,
/// unmistakable colour transition on hover.
///
/// Geometry is pinned so the sample point can be a constant: no border, no
/// radius, no padding, an 80x60 content-box minimum, so the border box starts
/// at the output origin and (10, 10) is inside it and clear of the centred
/// label.
const THEME: &str = "\
window { background-color: rgba(0, 0, 0, 0); }
button {
  min-width: 80px;
  min-height: 60px;
  padding: 0;
  border: 0 solid transparent;
  border-radius: 0;
  background-color: rgb(255 0 0);
  background-image: none;
  color: rgb(0 0 0);
  transition: background-color 2000ms linear;
}
button:hover { background-color: rgb(0 0 255); }
";

/// Where the background is sampled: inside the button, outside the label.
const SAMPLE: (u32, u32) = (10, 10);
/// Where the pointer is put to arm `:hover`: the button's centre.
const CENTRE: (f64, f64) = (40.0, 30.0);
/// The declared transition duration.
const DURATION: Duration = Duration::from_millis(2000);

struct Reaper(Child);

impl Drop for Reaper {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn spawn_button(socket: &str, theme_path: &std::path::Path) -> Reaper {
    Reaper(
        Command::new(env!("CARGO_BIN_EXE_themed-button"))
            .env("WAYLAND_DISPLAY", socket)
            .env("ICEDTEA_UI_THEME", theme_path)
            .env("ICEDTEA_UI_LABEL", "Click me")
            .env("ICEDTEA_UI_CLASSES", "")
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to spawn themed-button"),
    )
}

fn pixel(frame: &CapturedFrame) -> (u8, u8, u8) {
    icedtea_ui::shm::pixel_rgb(frame.format, &frame.bytes, frame.stride, SAMPLE.0, SAMPLE.1)
        .expect("sample pixel inside the captured frame")
}

fn close(a: u8, b: u8) -> bool {
    i32::from(a).abs_diff(i32::from(b)) <= 12
}

/// `rgb(255 0 0)` — the button's resting background.
fn is_start(px: (u8, u8, u8)) -> bool {
    close(px.0, 0xFF) && close(px.1, 0x00) && close(px.2, 0x00)
}

/// `rgb(0 0 255)` — `button:hover`'s background.
fn is_end(px: (u8, u8, u8)) -> bool {
    close(px.0, 0x00) && close(px.1, 0x00) && close(px.2, 0xFF)
}

/// Capture until `ready` accepts the sample pixel, pumping `pointer` so the
/// compositor keeps delivering, or until `timeout` passes.
fn capture_until(
    screencopy: &mut ScreencopyClient,
    mut pointer: Option<&mut VirtualPointerClient>,
    timeout: Duration,
    ready: impl Fn((u8, u8, u8)) -> bool,
) -> ((u8, u8, u8), Duration) {
    let started = Instant::now();
    let mut px = pixel(&screencopy.capture());
    while !ready(px) && started.elapsed() < timeout {
        std::thread::sleep(Duration::from_millis(25));
        if let Some(pointer) = pointer.as_deref_mut() {
            pointer.pump();
        }
        px = pixel(&screencopy.capture());
    }
    (px, started.elapsed())
}

/// The one on-screen animation proof: hovering starts a 2 s transition that
/// really takes about 2 s to arrive.
///
/// Mutation check: delete the `self.surface.frame(&self.qh, ())` request in
/// `LayerWindow::repaint` and the button stays red forever -- the "reached the
/// hover colour" assertion times out. Delete the `self.anim.restyle(..)` call
/// in `Button::restyle` and it snaps to blue in well under a second, failing
/// the elapsed-time assertion instead.
#[test]
fn a_background_color_transition_animates_on_a_real_compositor() {
    let mut theme = tempfile::NamedTempFile::new().expect("temp theme file");
    theme.write_all(THEME.as_bytes()).expect("writing the theme");
    theme.flush().expect("flushing the theme");

    let comp = Compositor::spawn();
    let (output_w, output_h) = comp.output_size();
    let (output_w, output_h) = (output_w as u32, output_h as u32);
    let _child = spawn_button(&comp.socket, theme.path());

    let mut screencopy = ScreencopyClient::spawn(&comp.socket);
    let (before, _) = capture_until(&mut screencopy, None, Duration::from_secs(10), is_start);
    assert!(
        is_start(before),
        "the button never reached the screen in its resting rgb(255 0 0): \
         sampled {before:?} at {SAMPLE:?}"
    );

    let mut pointer = VirtualPointerClient::spawn(&comp.socket);
    pointer.motion_absolute(CENTRE.0, CENTRE.1, output_w, output_h);
    pointer.frame();
    pointer.pump();

    let (after, elapsed) = capture_until(
        &mut screencopy,
        Some(&mut pointer),
        Duration::from_secs(10),
        is_end,
    );
    assert!(
        is_end(after),
        "the button never reached `button:hover`'s rgb(0 0 255): sampled \
         {after:?} after {elapsed:?}. Without a wl_surface.frame pump the \
         first repaint paints t = 0 and nothing ever advances the clock."
    );
    assert!(
        elapsed >= DURATION / 2,
        "the hover colour arrived after only {elapsed:?}: a {DURATION:?} \
         linear transition cannot finish that fast, so the change snapped \
         instead of animating"
    );
    assert!(
        elapsed <= Duration::from_secs(9),
        "the hover colour took {elapsed:?}, far longer than the declared \
         {DURATION:?}: frames are being dropped or the clock is not advancing"
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --test transition_screencopy -- --nocapture`
Expected: before Tasks 12-13 are applied, FAIL with
`the button never reached `button:hover`'s rgb(0 0 255)`. After them, PASS.
If it fails at the *first* assertion instead, the theme file is not being
read: check `ICEDTEA_UI_THEME` reaches `ThemeSource::File` (any non-empty,
non-`bundled` value does) and that the temp file outlives the child — the
`NamedTempFile` binding must stay alive for the whole test, which it does
here because it is a `let` in the test body.

- [ ] **Step 3: Write minimal implementation**

None: Tasks 12 and 13 are the implementation this test proves. If it fails,
the fix is in `LayerWindow::repaint`'s frame request or in
`Dispatch<wl_callback::WlCallback>`'s `dirty` handling — both from Task 13 —
not in the test's tolerances.

- [ ] **Step 4: Run the full gate sweep**

Run each of these and confirm the stated expectation before committing.

```bash
cargo test -p icedtea-ui
```
Expected: PASS, every test in the crate, including
`tests/themed_button_offscreen.rs` and `tests/layer_shell_screencopy.rs`.

```bash
cargo test --workspace
```
Expected: PASS.

```bash
cargo clippy -p icedtea-ui --all-targets -- -D warnings
```
Expected: no output, exit 0.

```bash
cargo fmt --all --check
```
Expected: no output, exit 0.

```bash
git diff --stat rebuild/pure-rust-gtk-m2 -- \
  ui/tests/themed_button_offscreen.rs \
  ui/tests/layer_shell_screencopy.rs \
  ui/tests/support/mod.rs
```
Expected: **empty output** — P5 must not have touched any of the three. If a
name P5 depends on forced a change here, that is a contract violation: revert
the edit and adapt `anim/`'s call site instead.

```bash
git log --oneline rebuild/pure-rust-gtk-m2..HEAD
```
Expected: 13 commits, each with the
`Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>` trailer
(`git log --format='%(trailers:key=Co-Authored-By)' | sort -u` should show
exactly that one line plus a blank).

- [ ] **Step 5: Commit**

```bash
git add ui/tests/transition_screencopy.rs
git commit -m "$(cat <<'EOF'
test(ui): prove a background-color transition animates on a real compositor

Start and end colours are captured through screencopy; intermediate frames
are not asserted. What separates an animation from a snap here is elapsed
time: a 2 s linear transition cannot reach the hover colour in under a
second, and a snap cannot take longer than one.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Self-Review

### 1. Spec coverage

Every bullet of the M2 spec's §5 (Transitions & animations) and the animation
half of §7 (Testing strategy), plus every item of contract §6 and §11's P5
boundary, mapped to the task that implements it.

| Spec / contract requirement | Task |
|---|---|
| §5 Clock: `anim::Clock` trait, `now() -> Duration` | 1 |
| §5 Clock: production = monotonic time driven by `wl_surface.frame`, no busy loop | 1 (clock), 13 (pump) |
| §5 Clock: tests = `ManualClock::advance(ms)` | 1 (`advance_ms`/`set_ms`) |
| §5 Transitions: on restyle, diff old vs new computed values | 7a |
| §5 Transitions: over the properties in `transition-property` | 4, 7a |
| §5 Transitions: `all` = every animatable row | 4 |
| §5 Transitions: `Transition { from, to, duration, delay, timing }` | 3 |
| §5 Transitions: reversal continuity (CSS Transitions §3.1) | 8 |
| §5 Transitions: `transition-*` values come from the new style | 7a (`new.transition_specs()`) |
| §5 Transitions: non-animatable changes switch at 50% | 2 (`interpolate_prop` → `discrete`), 9 (proof) |
| §5 Animations: `animation-name` → `@keyframes` from the compiled sheet | 7b (`update_animations`) |
| §5 Animations: duration, delay | 6a |
| §5 Animations: iteration-count incl. `infinite` | 6a |
| §5 Animations: direction normal/reverse/alternate/alternate-reverse | 6a |
| §5 Animations: fill-mode | 6b |
| §5 Animations: play-state | 6b |
| §5 Animations: per-span timing functions | 5 (`resolve_segment` returns the keyframe's), 6a (applies it) |
| §5 Precedence: animation over transition over base cascade | 2 (`Overrides::set`), 7b (write order + test) |
| §5 Interpolation is the registry row's `animatable` fn — lengths, numbers, colours, shadows, gradients, transforms, opacity, per-side values | 2 (`interpolate_prop` dispatches to `Prop::interpolator`; the per-type maths is P1's `value/interpolate.rs`, contract §2.10) |
| §5 Output: `AnimationState::sample(now) -> Overrides` layered before layout/paint | 7a-7b (produce), 12 (hand-off to `paint_node`) |
| §5 Output: `is_active()` requests the next frame | 7a-7b (produce), 13 (consume) |
| §5 Timing functions: `linear`, `ease*`, `cubic-bezier` (Newton–Raphson on x), `steps(n, start\|end\|jump-*)` | 3 (pinned at known points; the solver itself is P1's `TimingFunction::eval`, contract §2.9) |
| §7 Animation tests: 0/100/200 ms linear midpoint | 3, 7a |
| §7 Animation tests: `ease` at t = 0.5 | 3 |
| §7 Animation tests: reversal continuity | 8 |
| §7 Animation tests: `alternate` | 6a |
| §7 Animation tests: `fill-mode: forwards` | 6b |
| §7 Animation tests: Adwaita's button `transition` sampled at 100 ms | 11 |
| §7 Wayland proof: one `transition: background-color` start/end capture | 14 |
| §7 Mutation discipline: every load-bearing test records a mutation check | every test in 1-14 (16 tasks after the 6a/6b and 7a/7b splits) |
| §7 Gates: `cargo test -p icedtea-ui`, clippy `-D warnings`, `cargo fmt --all --check` | every task's step 4; swept in 14 |
| Contract §6 `Clock`, `MonotonicClock`, `ManualClock` | 1 |
| Contract §6 `Overrides` (`is_empty`/`get`/`iter`/`set`) | 2 |
| Contract §6 `TransitionSpec`, `AnimationSpec` | 1 |
| Contract §6 `AnimationState::{new, restyle, sample, is_active, next_deadline}` | 7a (signatures + transitions), 7b (animation arms) |
| Contract §6 zero-duration transitions constructed, not special-cased | 9 |
| Contract §11 P5: `Value::interpolate` wiring through `Prop::interpolator` | 2 |
| Contract §11 P5: frame-callback pump in `wayland.rs` | 13 |
| Contract §11 P5: `Overrides` hand-off in `widget/button.rs` | 12 |
| Contract §11 P5: must not touch paint internals | enforced — no task modifies `paint/**`; 12 changes one argument at the call site |
| Global: never-panic obligation | 10 |

**Gaps found and closed while writing:** the contract's `app.rs` mention has
no work behind it (deviation 3); `TransitionSpec`/`AnimationSpec` are needed
by P3 before P5 runs (deviation 1); `transition-property` naming a shorthand
is unspecified (deviation 5) and is load-bearing for the Adwaita gate, so
Task 4 rules on it and Task 11 proves it against the real sheet.

### 2. Placeholder scan

Searched the plan for `TBD`, `TODO`, `FIXME`, `implement later`, `fill in`,
`similar to Task`, `and so on`, `etc.`, `appropriate error handling`,
`handle edge cases`, and bare "write tests for the above".

- No occurrences. Every code step is complete, compilable Rust against the
  contract's signatures.
- The two `.rs` files created empty in Task 1 (`transition.rs`,
  `keyframes.rs`) carry only their module doc comments and are filled in
  Tasks 3-6b; they are module-tree scaffolding, not stubbed functions, and no
  task refers to a function that does not exist by the time it runs.
- One ordering exception is called out explicitly rather than papered over:
  Task 7a's `update_transitions` calls `Transition::reverse`, whose code is
  printed in Task 8. Task 7a's step 3 says to write it there and names Task 8
  as the source; Task 8's step 2 accounts for both orders. No stub is
  involved.
- Task 12's `read_pixel` helper and Task 13's `state()` fixture are both
  "reuse the one the file's test module already has", and both print the
  body they are reusing: `read_pixel` as a fallback, and `state()` verbatim
  from `ui/src/wayland.rs`'s test module on this branch, so neither leaves an
  undefined symbol for the executor to guess at. Every function named in a
  printed code block resolves to something this plan or the current crate
  defines — re-checked after the 6a/6b and 7a/7b splits.

### 2a. Task sizing

No task exceeds this plan's ~400-line header-to-header ceiling. The two that
did in an earlier draft were split along their natural seams and keep their
old numbers as suffixes so every cross-reference in the plan stays valid:

| Task | Lines | Was |
|---|---|---|
| 6a `ActiveAnimation` — phases, iterations and direction | ~392 | Task 6 (547) |
| 6b `ActiveAnimation` — fill modes, play state, rebinding | ~300 | Task 6 (547) |
| 7a `AnimationState` — the restyle diff over transitions | ~362 | Task 7 (475) |
| 7b `AnimationState` — `@keyframes` binding and precedence | ~235 | Task 7 (475) |

Neither split introduces a stub: 6a's `iteration_progress` and 7a's `restyle`
are complete, correct implementations of a narrower behaviour (fill-blind,
transition-only), each with tests that pass at that width, and 6b/7b widen
them by replacing whole named items whose new bodies are printed in full.

### 3. Type consistency against the contract

| Name | Contract §6 spelling | This plan | Match |
|---|---|---|---|
| `Clock::now` | `fn now(&self) -> Duration` | identical | yes |
| `MonotonicClock::new` | `pub fn new() -> Self` | identical (+`Default`, required by clippy's `new_without_default`) | yes |
| `ManualClock::{new, advance_ms, set_ms}` | `new()`, `advance_ms(&self, ms: u64)`, `set_ms(&self, ms: u64)` | identical (+`Default`) | yes |
| `Overrides` | `{ entries: Vec<(Prop, Value)> }`, `is_empty`, `get`, `iter`, `set` | identical (+`len`, additive `pub`, ruled on in deviation 7) | yes |
| `TransitionSpec` | `{ prop: Option<Prop>, all: bool, duration: Time, delay: Time, timing: TimingFunction }` | identical | yes |
| `AnimationSpec` | `{ name: AnimationName, duration: Time, delay: Time, timing: TimingFunction, iterations: IterationCount, direction: Keyword, fill: Keyword, play_state: Keyword }` | identical | yes |
| `AnimationState::new` | `pub fn new() -> Self` | identical (+`Default`; +`transition_count`/`animation_count`, additive `pub`, ruled on in deviation 7) | yes |
| `AnimationState::restyle` | `(&mut self, old: Option<&ComputedStyle>, new: &ComputedStyle, now: Duration, sheet: &CompiledSheet)` | identical | yes |
| `AnimationState::sample` | `(&mut self, now: Duration) -> Overrides` | identical | yes |
| `AnimationState::is_active` | `(&self, now: Duration) -> bool` | identical | yes |
| `AnimationState::next_deadline` | `(&self, now: Duration) -> Option<Duration>` | identical (semantics narrowed to a lower bound — deviation 4) | yes |

Consumed-from-other-parts names, checked against the contract text:

| Name | Source | Used in |
|---|---|---|
| `Prop::{interpolator, is_longhand, longhands, initial}` | §1.3 [P1] | 2, 4 |
| `registry::animatable_longhands()` | §1.3 [P1] | 4 |
| `Value` / `Value::Number` / `PartialEq` | §2.1 [P1] | 2, 3, 5-11 |
| `interpolate::discrete` | §2.10 [P1] | 2 |
| `Time(pub f32)`, `Time::as_secs_f32` | §2.9 [P1] | 3, 6a |
| `TimingFunction::{Linear, CubicBezier, Steps, EASE, EASE_IN, EASE_OUT, EASE_IN_OUT, eval}`, `StepPosition` | §2.9 [P1] | 3, 6a, 10 |
| `AnimationName::{None, Named}`, `IterationCount::{Infinite, Count}` | §2.9 [P1] | 6a, 6b, 7b |
| `Keyframe { offsets, declarations, timing }`, `Keyframes { name, frames }`, `Keyframes::properties` | §2.9 [P1] | 5, 6a, 6b |
| `Keyword::{Normal, Reverse, Alternate, AlternateReverse, Forwards, Backwards, Both, Running, Paused, None}` | §2.2 [P1] | 6a, 6b |
| `Rgba { r, g, b, a }` | §2.4 [P1] | 11 |
| `Node::{new, with_classes, append_child, set_states, set_state}`, `PseudoStates::{HOVER, ACTIVE, FOCUS, CHECKED}` | §3 [P2] | 7a, 9, 11 |
| `MatchCx::new` | §3.2 [P2] | 7a, 11 |
| `CompiledSheet::{compile, keyframes}` | §4 [P1/P3] | 7a, 7b, 9, 10, 11 |
| `ComputedStyle::{initial, raw, resolve_chain, transition_specs, animation_specs}`, `ResolveEnv::default`, `FromValue::from_value` | §5 [P3] | 5, 7a, 7b, 9, 11 |
| `paint::paint_node(.., overrides: Option<&Overrides>, ..)` | §8 [P4] | 12 |
| `shm::pixel_rgb` | unchanged from M1 | 14 |

No name is used in a later task under a different spelling than the task that
introduced it: `transitioned_props`, `resolve_segment`, `direction_progress`,
`interpolate_prop`, `millis`, `combined_ms`, `duration_ms_of`, `delay_ms_of`,
`Transition::{start, progress, eased, value_at, end_ms, is_finished, reverse}`,
`ActiveAnimation::{start, rebind, apply_play_state, phase, iteration_progress,
sample, is_active, next_change_ms, props}`,
`Button::{set_clock, tick, is_animating, overrides}`, and
`should_request_frame` each appear with one spelling throughout.
