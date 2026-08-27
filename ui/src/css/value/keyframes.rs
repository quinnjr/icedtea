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
            let mut offsets: Vec<f32> = offsets
                .iter()
                .copied()
                .filter(|offset| offset.is_finite())
                .map(|offset| offset.clamp(0.0, 1.0))
                .collect();
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
    /// between them, and the segment's timing function.
    ///
    /// `None` when fewer than two frames declare `prop`.
    #[must_use]
    pub fn segment(&self, prop: Prop, t: f32) -> Option<(&Value, &Value, f32, TimingFunction)> {
        // Flatten to (offset, value, timing) for every offset a frame that
        // declares `prop` applies at.
        let mut points: Vec<(f32, &Value, Option<TimingFunction>)> = Vec::new();
        for frame in self.frames.iter() {
            let Some((_, value)) = frame.declarations.iter().find(|(name, _)| *name == prop) else {
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
                return Some((from, to, local, timing.unwrap_or(TimingFunction::Linear)));
            }
        }
        let last = points.len() - 1;
        Some((
            points[last - 1].1,
            points[last].1,
            1.0,
            points[last - 1].2.unwrap_or(TimingFunction::Linear),
        ))
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
        assert_eq!(timing, TimingFunction::Linear);
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
        assert_eq!(timing, TimingFunction::EASE_IN);
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
