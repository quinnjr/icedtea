//! One running `@keyframes` animation: phases, iterations, direction,
//! fill mode, play state, and keyframe bracketing.

use std::rc::Rc;

use crate::anim::{AnimationSpec, Overrides, interpolate_prop};
use crate::css::computed::ComputedStyle;
use crate::css::registry::Prop;
use crate::css::value::keyframes::{Keyframe, Keyframes};
use crate::css::value::timing::TimingFunction;
use crate::css::value::{IterationCount, Keyword, Value};

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
    let t = if span > 0.0 {
        (q - points[lo].0) / span
    } else {
        1.0
    };
    Some((points[lo].1.clone(), points[hi].1.clone(), t, points[lo].2))
}

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
    /// Local milliseconds at which `animation-play-state: paused` froze it.
    ///
    /// `Some` also means "asks for no more frames": a paused animation's
    /// sample cannot change until something un-pauses it.
    paused_at: Option<f64>,
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
            paused_at: None,
        }
    }

    /// The longhands this animation writes.
    #[must_use]
    pub fn props(&self) -> &[Prop] {
        &self.props
    }

    fn duration_ms(&self) -> f64 {
        // Multiply in f32 before widening -- same ordering as, and for the
        // same reason as, `transition::duration_ms_of`: `Time` stores
        // seconds as f32, and widening to f64 before multiplying by 1000.0
        // exposes the f32 rounding of the stored seconds value as
        // sub-millisecond noise (200.00000298023224 instead of 200.0).
        let ms = f64::from(self.spec.duration.as_secs_f32() * 1000.0);
        if ms.is_finite() { ms.max(0.0) } else { 0.0 }
    }

    fn delay_ms(&self) -> f64 {
        // Same f32-then-widen ordering as `duration_ms`, and as
        // `transition::delay_ms_of`.
        let ms = f64::from(self.spec.delay.as_secs_f32() * 1000.0);
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

    /// Milliseconds since this animation's delay began, frozen while paused.
    fn local_ms(&self, now_ms: f64) -> f64 {
        self.paused_at.unwrap_or(now_ms - self.start_ms)
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
            Phase::Before => {
                matches!(self.spec.fill, Keyword::Backwards | Keyword::Both).then_some((0, 0.0))
            }
            Phase::After => matches!(self.spec.fill, Keyword::Forwards | Keyword::Both)
                .then(|| self.end_iteration()),
            Phase::Active => {
                let duration = self.duration_ms();
                let active = self.active_ms(now_ms);
                let index = (active / duration).floor();
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let index = if index.is_finite() {
                    index.max(0.0) as u64
                } else {
                    0
                };
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
            // Mirrors `Transition::eased`'s guard: a `cubic-bezier()` with
            // NaN control points is P1's degenerate input to accept at parse
            // time, not reject; this module's boundary is where it is
            // clamped back to a finite value rather than poisoning the
            // sampled property.
            let eased = timing.eval(t);
            let eased = if eased.is_finite() { eased } else { t };
            out.set(prop, interpolate_prop(prop, &from, &to, eased));
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
}

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

    fn timed_frame(offsets: &[f32], prop: Prop, value: Value, timing: TimingFunction) -> Keyframe {
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
        assert!(
            (t - 0.5).abs() < 1e-6,
            "0.25 is half way through the 0..0.5 span, got {t}"
        );
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
        let (_, to, _, _) = resolve_segment(&frames, Prop::Opacity, 0.5, &base).expect("bracketed");
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
            timed_frame(
                &[0.0],
                Prop::Opacity,
                Value::Number(0.0),
                TimingFunction::EASE_OUT,
            ),
            timed_frame(
                &[1.0],
                Prop::Opacity,
                Value::Number(1.0),
                TimingFunction::EASE_IN,
            ),
        ];
        let (_, _, _, timing) =
            resolve_segment(&frames, Prop::Opacity, 0.5, &base).expect("bracketed");
        assert_eq!(timing, Some(TimingFunction::EASE_OUT));
    }

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
            spec(
                1000.0,
                0.0,
                IterationCount::Count(3.0),
                Keyword::Normal,
                Keyword::None,
            ),
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
            spec(
                1000.0,
                0.0,
                IterationCount::Count(4.0),
                Keyword::Alternate,
                Keyword::None,
            ),
            Rc::from("fade"),
            fade(),
            0.0,
        );
        assert_eq!(
            opacity_at(&anim, 250.0, &base),
            Some(0.25),
            "iteration 0 forwards"
        );
        assert_eq!(
            opacity_at(&anim, 1250.0, &base),
            Some(0.75),
            "iteration 1 backwards"
        );
        assert_eq!(
            opacity_at(&anim, 2250.0, &base),
            Some(0.25),
            "iteration 2 forwards"
        );
    }

    // Mutation check: revert `duration_ms`/`delay_ms` to widening before
    // multiplying (`f64::from(x.as_secs_f32()) * 1000.0`) and this fails --
    // `Time(0.2)` (from a literal `200ms`) round-trips to exactly `200.0` the
    // multiply-in-f32 way but to `200.00000298023224` the widen-first way.
    #[test]
    fn duration_and_delay_convert_ms_the_same_way_transitions_do() {
        use crate::anim::TransitionSpec;
        use crate::anim::transition::{delay_ms_of, duration_ms_of};

        let anim = ActiveAnimation::start(
            spec(
                200.0,
                75.0,
                IterationCount::Count(1.0),
                Keyword::Normal,
                Keyword::None,
            ),
            Rc::from("fade"),
            fade(),
            0.0,
        );
        let equivalent_transition = TransitionSpec {
            prop: None,
            all: false,
            duration: Time(200.0 / 1000.0),
            delay: Time(75.0 / 1000.0),
            timing: TimingFunction::Linear,
        };
        assert_eq!(anim.duration_ms(), duration_ms_of(&equivalent_transition));
        assert_eq!(anim.delay_ms(), delay_ms_of(&equivalent_transition));
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
            spec(
                1000.0,
                0.0,
                IterationCount::Infinite,
                Keyword::Normal,
                Keyword::None,
            ),
            Rc::from("fade"),
            fade(),
            0.0,
        );
        assert!(anim.is_active(1_000_000.0));
        assert_eq!(anim.phase(1_000_000.0), Phase::Active);
        assert_eq!(opacity_at(&anim, 1_000_250.0, &base), Some(0.25));
    }

    // THE GATE (spec §7: `fill-mode: forwards`).
    // Mutation check: treat every fill mode as filling and the
    // `Keyword::None` case starts reporting 1.0 after the end.
    #[test]
    fn fill_mode_decides_whether_the_end_value_sticks() {
        let base = base();
        let none = ActiveAnimation::start(
            spec(
                200.0,
                0.0,
                IterationCount::Count(1.0),
                Keyword::Normal,
                Keyword::None,
            ),
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
            spec(
                200.0,
                0.0,
                IterationCount::Count(1.0),
                Keyword::Normal,
                Keyword::Forwards,
            ),
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
            spec(
                200.0,
                100.0,
                IterationCount::Count(1.0),
                Keyword::Normal,
                Keyword::None,
            ),
            Rc::from("fade"),
            fade(),
            0.0,
        );
        assert_eq!(opacity_at(&none, 50.0, &base), None);
        assert_eq!(
            opacity_at(&none, 200.0, &base),
            Some(0.5),
            "100 ms into a 200 ms run"
        );

        let backwards = ActiveAnimation::start(
            spec(
                200.0,
                100.0,
                IterationCount::Count(1.0),
                Keyword::Normal,
                Keyword::Backwards,
            ),
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
            spec(
                1000.0,
                0.0,
                IterationCount::Count(2.5),
                Keyword::Normal,
                Keyword::Forwards,
            ),
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
            spec(
                1000.0,
                0.0,
                IterationCount::Infinite,
                Keyword::Normal,
                Keyword::None,
            ),
            Rc::from("fade"),
            fade(),
            0.0,
        );
        assert_eq!(opacity_at(&anim, 250.0, &base), Some(0.25));

        anim.spec.play_state = Keyword::Paused;
        anim.apply_play_state(250.0);
        assert_eq!(
            opacity_at(&anim, 900.0, &base),
            Some(0.25),
            "frozen while paused"
        );
        assert!(
            !anim.is_active(900.0),
            "a paused animation asks for no frames"
        );

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
            spec(
                1000.0,
                0.0,
                IterationCount::Infinite,
                Keyword::Normal,
                Keyword::None,
            ),
            Rc::from("fade"),
            fade(),
            0.0,
        );
        anim.rebind(
            spec(
                1000.0,
                0.0,
                IterationCount::Infinite,
                Keyword::Normal,
                Keyword::None,
            ),
            fade(),
        );
        assert_eq!(opacity_at(&anim, 500.0, &base), Some(0.5));
    }
}
