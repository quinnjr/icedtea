//! One running `@keyframes` animation: phases, iterations, direction,
//! fill mode, play state, and keyframe bracketing.

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
    let t = if span > 0.0 {
        (q - points[lo].0) / span
    } else {
        1.0
    };
    Some((points[lo].1.clone(), points[hi].1.clone(), t, points[lo].2))
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
}
