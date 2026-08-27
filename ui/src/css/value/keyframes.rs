//! Compiled `@keyframes`.

use std::rc::Rc;

use cssparser::{Parser, ParserInput};

use crate::css::parse::KeyframesRule;
use crate::css::registry::{Prop, lookup, parse_declaration_value};

use super::Value;
use super::timing::TimingFunction;

/// One keyframe: the offsets it applies at, its longhand declarations, and
/// the timing function that governs the segment starting at it.
#[derive(Clone, Debug)]
pub struct Keyframe {
    /// Offsets in `0..=1`; `from` == `0.0`, `to` == `1.0`.
    pub offsets: Rc<[f32]>,
    /// Longhands only -- shorthands are pre-expanded.
    pub declarations: Rc<[(Prop, Value)]>,
    /// Per-keyframe `animation-timing-function`.
    pub timing: Option<TimingFunction>,
}

/// A compiled `@keyframes` rule.
#[derive(Clone, Debug)]
pub struct Keyframes {
    /// The animation name.
    pub name: Rc<str>,
    /// The frames, in ascending offset order.
    pub frames: Rc<[Keyframe]>,
}

impl Keyframes {
    /// Compile a parsed rule: expand shorthands, parse values, drop
    /// unknown properties and unparseable declarations (logged), and sort
    /// the frames by offset.
    #[must_use]
    pub fn compile(rule: &KeyframesRule) -> Keyframes {
        let mut frames: Vec<Keyframe> = Vec::with_capacity(rule.frames.len());
        for (offsets, declarations) in &rule.frames {
            let mut resolved: Vec<(Prop, Value)> = Vec::new();
            let mut timing = None;
            for declaration in declarations {
                let Some(prop) = lookup(&declaration.name) else {
                    tracing::debug!(
                        name = %declaration.name,
                        "unknown property in @keyframes; dropping"
                    );
                    continue;
                };
                if prop == Prop::AnimationTimingFunction {
                    let mut source = ParserInput::new(&declaration.value);
                    let mut parser = Parser::new(&mut source);
                    if let Ok(parsed) = TimingFunction::parse(&mut parser) {
                        timing = Some(parsed);
                    }
                    continue;
                }
                if prop.is_longhand() {
                    match parse_declaration_value(prop, &declaration.value) {
                        Ok(value) => resolved.push((prop, value)),
                        Err(()) => tracing::debug!(
                            name = %declaration.name,
                            value = %declaration.value,
                            "unparseable declaration in @keyframes; dropping"
                        ),
                    }
                    continue;
                }
                let mut source = ParserInput::new(&declaration.value);
                let mut parser = Parser::new(&mut source);
                let mut sink = |longhand: Prop, value: Value| resolved.push((longhand, value));
                if prop.expand_into(&mut parser, &mut sink).is_err() {
                    tracing::debug!(
                        name = %declaration.name,
                        value = %declaration.value,
                        "unexpandable shorthand in @keyframes; dropping"
                    );
                }
            }
            // CSS Animations 1: a keyframe selector outside 0%..100% is
            // invalid and the *keyframe* is ignored. Clamping it instead let
            // a bogus `150%` land on 1.0 and, being later in source order,
            // replace the real 100% endpoint in the sampler's dedupe-by-
            // offset, so the animation ended at the wrong value.
            if offsets
                .iter()
                .any(|offset| !offset.is_finite() || !(0.0..=1.0).contains(offset))
            {
                tracing::debug!(
                    ?offsets,
                    "keyframe selector out of 0%..100%; dropping keyframe"
                );
                continue;
            }
            let mut offsets: Vec<f32> = offsets.clone();
            offsets.sort_by(f32::total_cmp);
            if offsets.is_empty() {
                continue;
            }
            frames.push(Keyframe {
                offsets: offsets.into(),
                declarations: resolved.into(),
                timing,
            });
        }
        frames.sort_by(|a, b| a.offsets[0].total_cmp(&b.offsets[0]));
        Keyframes {
            name: Rc::from(rule.name.as_str()),
            frames: frames.into(),
        }
    }

    /// Every longhand any frame mentions, deduped, in registry order.
    #[must_use]
    pub fn properties(&self) -> Vec<Prop> {
        let mut props: Vec<Prop> = self
            .frames
            .iter()
            .flat_map(|frame| frame.declarations.iter().map(|(prop, _)| *prop))
            .collect();
        props.sort_unstable();
        props.dedup();
        props
    }

    /// The two frames bracketing `t` for `prop`, the local `0..1` progress
    /// between them, and the segment's *own* timing function.
    ///
    /// The timing is `None` when the starting keyframe declares no
    /// `animation-timing-function`, because the fallback is the animation's
    /// own value (CSS initial `ease`), which only the caller knows. Reporting
    /// `Linear` here silently discarded an `animation: spin 1s ease-in-out`
    /// and disagreed with `anim::keyframes::resolve_segment`, which has
    /// always returned an `Option` for exactly this reason.
    ///
    /// `None` for the whole tuple when fewer than two frames declare `prop`.
    #[must_use]
    pub fn segment(
        &self,
        prop: Prop,
        t: f32,
    ) -> Option<(&Value, &Value, f32, Option<TimingFunction>)> {
        // Flatten to (offset, value, timing) for every offset a frame that
        // declares `prop` applies at.
        let mut points: Vec<(f32, &Value, Option<TimingFunction>)> = Vec::new();
        for frame in self.frames.iter() {
            // Last-wins inside one keyframe block, as everywhere else in CSS
            // and as `anim::keyframes::resolve_segment` already does: a
            // `padding: 0px` followed by `padding-left: 10px` starts at 10px.
            let Some((_, value)) = frame
                .declarations
                .iter()
                .rev()
                .find(|(name, _)| *name == prop)
            else {
                continue;
            };
            for offset in frame.offsets.iter() {
                points.push((*offset, value, frame.timing));
            }
        }
        points.sort_by(|a, b| a.0.total_cmp(&b.0));
        if points.len() < 2 {
            return None;
        }
        let t = t.clamp(points[0].0, points[points.len() - 1].0);
        for window in 0..points.len() - 1 {
            let (start, from, timing) = points[window];
            let (end, to, _) = points[window + 1];
            if t >= start && t <= end {
                let local = if (end - start).abs() <= f32::EPSILON {
                    1.0
                } else {
                    (t - start) / (end - start)
                };
                return Some((from, to, local, timing));
            }
        }
        let last = points.len() - 1;
        Some((points[last - 1].1, points[last].1, 1.0, points[last - 1].2))
    }
}

#[cfg(test)]
mod tests {
    use super::Keyframes;
    use crate::css::parse::parse_stylesheet;
    use crate::css::registry::Prop;
    use crate::css::value::Value;
    use crate::css::value::timing::TimingFunction;

    fn compiled(css: &str) -> Keyframes {
        let sheet = parse_stylesheet(css);
        Keyframes::compile(&sheet.keyframes[0])
    }

    #[test]
    fn compilation_expands_shorthands_and_drops_unknown_properties() {
        let frames =
            compiled("@keyframes fade { from { padding: 4px; nonsense: 1 } to { padding: 8px } }");
        assert_eq!(frames.name.as_ref(), "fade");
        assert_eq!(frames.frames.len(), 2);
        let properties = frames.properties();
        assert!(properties.contains(&Prop::PaddingTop));
        assert!(properties.contains(&Prop::PaddingLeft));
        assert!(!properties.iter().any(|prop| prop.name() == "nonsense"));
        // properties() is deduped and in registry order.
        let mut sorted = properties.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(properties, sorted);
    }

    #[test]
    fn a_segment_brackets_the_requested_progress() {
        let frames = compiled(
            "@keyframes fade { 0% { opacity: 0 } 50% { opacity: 1 } 100% { opacity: 0 } }",
        );
        let (from, to, local, timing) = frames
            .segment(Prop::Opacity, 0.25)
            .expect("0.25 lies inside the first segment");
        assert_eq!(*from, Value::Number(0.0));
        assert_eq!(*to, Value::Number(1.0));
        assert!((local - 0.5).abs() < 1e-6, "{local}");
        // No per-frame `animation-timing-function`, so the segment reports
        // `None` and the caller substitutes the animation's own value; it
        // does *not* claim `linear` (F48).
        assert_eq!(timing, None);
        let (from, to, local, _) = frames.segment(Prop::Opacity, 0.75).expect("second segment");
        assert_eq!(*from, Value::Number(1.0));
        assert_eq!(*to, Value::Number(0.0));
        assert!((local - 0.5).abs() < 1e-6, "{local}");
        // A property no frame mentions has no segment.
        assert!(frames.segment(Prop::Color, 0.5).is_none());
        // Clamped at the ends.
        assert!(frames.segment(Prop::Opacity, 0.0).is_some());
        assert!(frames.segment(Prop::Opacity, 1.0).is_some());
    }

    #[test]
    fn a_per_keyframe_timing_function_wins_for_its_segment() {
        let frames = compiled(
            "@keyframes fade { 0% { opacity: 0; animation-timing-function: ease-in } 100% { opacity: 1 } }",
        );
        let (_, _, _, timing) = frames.segment(Prop::Opacity, 0.5).expect("segment exists");
        assert_eq!(timing, Some(TimingFunction::EASE_IN));
        // The timing declaration is not itself an animated property.
        assert!(!frames.properties().contains(&Prop::AnimationTimingFunction));
    }

    #[test]
    fn adwaitas_own_keyframes_compile() {
        let sheet = parse_stylesheet(crate::BUNDLED_ADWAITA_LIGHT);
        for rule in &sheet.keyframes {
            let compiled = Keyframes::compile(rule);
            assert!(
                !compiled.frames.is_empty(),
                "{} compiled to nothing",
                rule.name
            );
        }
    }

    #[test]
    fn compilation_never_panics_on_hostile_frames() {
        for input in crate::css::value::FUZZ_INPUTS {
            let css = format!("@keyframes k {{ {input} {{ {input}: {input} }} }}");
            let sheet = parse_stylesheet(&css);
            for rule in &sheet.keyframes {
                let compiled = Keyframes::compile(rule);
                let _ = compiled.properties();
                for t in [-1.0_f32, 0.0, 0.5, 1.0, 2.0] {
                    let _ = compiled.segment(Prop::Opacity, t);
                }
            }
        }
    }
}

#[cfg(test)]
mod review_tests {
    use super::Keyframes;
    use crate::css::parse::parse_stylesheet;
    use crate::css::registry::Prop;
    use crate::css::value::{Length, Value};

    fn compiled(css: &str) -> Keyframes {
        Keyframes::compile(&parse_stylesheet(css).keyframes[0])
    }

    // F45: CSS Animations 1 makes a keyframe selector outside 0%..100%
    // invalid and ignores the keyframe. Clamping it let the bogus frame
    // replace the real endpoint.
    #[test]
    fn an_out_of_range_keyframe_selector_drops_its_keyframe() {
        let frames = compiled(
            "@keyframes k { 0% { opacity: 0 } 100% { opacity: 1 } 150% { opacity: 0.5 } }",
        );
        let (_, to, _, _) = frames
            .segment(Prop::Opacity, 1.0)
            .expect("two real frames remain");
        assert_eq!(
            *to,
            Value::Number(1.0),
            "the 150% frame must not have replaced the 100% endpoint"
        );
        assert_eq!(frames.frames.len(), 2);

        // Negative selectors collapse onto 0% the same way, and are dropped.
        let negative =
            compiled("@keyframes k { -50% { opacity: 0.5 } 0% { opacity: 0 } to { opacity: 1 } }");
        let (from, _, _, _) = negative
            .segment(Prop::Opacity, 0.0)
            .expect("the two valid frames remain");
        assert_eq!(*from, Value::Number(0.0));
        assert_eq!(negative.frames.len(), 2);
    }

    // F46/F47: last declaration wins inside a keyframe block, as it does
    // everywhere else and as the production sampler already assumed.
    #[test]
    fn the_last_declaration_of_a_longhand_in_a_keyframe_wins() {
        let frames = compiled(
            "@keyframes k { from { padding: 0px; padding-left: 10px } to { padding-left: 20px } }",
        );
        let (from, to, _, _) = frames
            .segment(Prop::PaddingLeft, 0.0)
            .expect("both frames declare padding-left");
        assert_eq!(*from, Value::Length(Length::px(10.0)));
        assert_eq!(*to, Value::Length(Length::px(20.0)));
        // The shorthand's other sides are unaffected: `padding: 0px` still
        // sets padding-top, and only `to` omits it, so there is no segment.
        assert!(frames.segment(Prop::PaddingTop, 0.0).is_none());
    }

    // F48/F49: a frame with no `animation-timing-function` reports `None`,
    // so the caller can substitute the animation's own value; claiming
    // `linear` silently discarded an `animation: spin 1s ease-in-out`.
    #[test]
    fn a_frame_without_its_own_timing_reports_none_not_linear() {
        let frames = compiled("@keyframes spin { from { opacity: 0 } to { opacity: 1 } }");
        let (_, _, _, timing) = frames.segment(Prop::Opacity, 0.5).expect("segment exists");
        assert_eq!(timing, None);
    }
}
