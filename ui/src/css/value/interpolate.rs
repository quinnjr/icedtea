//! The registry's interpolators.
//!
//! Every one takes two *computed* values of the same property. A pair an
//! interpolator cannot handle falls back to [`discrete`] rather than
//! panicking, so a hostile theme cannot crash an animation.

use std::rc::Rc;

use super::Value;
use super::border::BgSize;
use super::color::{ColorValue, Rgba};
use super::font::{FontWeight, LineHeight};
use super::image::{Image, Position};
use super::length::Length;
use super::shadow::Shadow;
use super::transform::{TransformFn, decompose_2d, recompose_2d};

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// `b` once `t >= 0.5`, else `a`.
#[must_use]
pub fn discrete(a: &Value, b: &Value, t: f32) -> Value {
    if t >= 0.5 { b.clone() } else { a.clone() }
}

/// Numbers, percentages and angles.
#[must_use]
pub fn number(a: &Value, b: &Value, t: f32) -> Value {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => Value::Number(lerp(*x, *y, t)),
        (Value::Percentage(x), Value::Percentage(y)) => Value::Percentage(lerp(*x, *y, t)),
        (Value::Angle(x), Value::Angle(y)) => Value::Angle(lerp(*x, *y, t)),
        _ => discrete(a, b, t),
    }
}

/// Percentages.
#[must_use]
pub fn percentage(a: &Value, b: &Value, t: f32) -> Value {
    number(a, b, t)
}

fn lerp_length(a: &Length, b: &Length, t: f32) -> Length {
    match (a, b) {
        (Length::Abs { value: x, unit: ua }, Length::Abs { value: y, unit: ub }) if ua == ub => {
            Length::Abs {
                value: lerp(*x, *y, t),
                unit: *ua,
            }
        }
        (Length::Percent(x), Length::Percent(y)) => Length::Percent(lerp(*x, *y, t)),
        // Unit-incompatible endpoints become a calc sum, which resolves
        // once a LengthCtx exists (CSS Values L4 §interpolation).
        _ => {
            let scaled = |length: &Length, weight: f32| {
                Rc::new(super::calc::CalcNode::Product(
                    Rc::new(super::calc::CalcNode::Length(length.clone())),
                    weight,
                ))
            };
            Length::Calc(Rc::new(super::calc::CalcNode::Sum(
                scaled(a, 1.0 - t),
                scaled(b, t),
            )))
        }
    }
}

/// Lengths.
#[must_use]
pub fn length(a: &Value, b: &Value, t: f32) -> Value {
    match (a, b) {
        (Value::Length(x), Value::Length(y)) => Value::Length(lerp_length(x, y, t)),
        _ => discrete(a, b, t),
    }
}

fn lerp_rgba(a: Rgba, b: Rgba, t: f32) -> Rgba {
    let pa = a.premultiplied();
    let pb = b.premultiplied();
    let mut out = [0.0_f32; 4];
    for index in 0..4 {
        out[index] = lerp(pa[index], pb[index], t);
    }
    Rgba::from_premultiplied(out).clamped()
}

/// Colours, in premultiplied sRGB (as GTK does).
///
/// Only resolved colours reach here: `computed` resolves `@name` and
/// `currentColor` before an interpolator ever sees the value.
#[must_use]
pub fn color(a: &Value, b: &Value, t: f32) -> Value {
    match (a, b) {
        (Value::Color(ColorValue::Absolute(x)), Value::Color(ColorValue::Absolute(y))) => {
            Value::Color(ColorValue::Absolute(lerp_rgba(*x, *y, t)))
        }
        _ => discrete(a, b, t),
    }
}

/// Corner radii and `border-spacing`.
#[must_use]
pub fn pair(a: &Value, b: &Value, t: f32) -> Value {
    match (a, b) {
        (Value::Pair(x), Value::Pair(y)) => {
            Value::Pair(Rc::new((length(&x.0, &y.0, t), length(&x.1, &y.1, t))))
        }
        _ => length(a, b, t),
    }
}

fn transparent_shadow(model: &Shadow) -> Shadow {
    Shadow {
        color: Some(ColorValue::Absolute(Rgba::TRANSPARENT)),
        offset_x: Length::zero(),
        offset_y: Length::zero(),
        blur: Length::zero(),
        spread: Length::zero(),
        inset: model.inset,
    }
}

fn lerp_shadow(a: &Shadow, b: &Shadow, t: f32) -> Shadow {
    let color = match (&a.color, &b.color) {
        (Some(ColorValue::Absolute(x)), Some(ColorValue::Absolute(y))) => {
            Some(ColorValue::Absolute(lerp_rgba(*x, *y, t)))
        }
        _ => {
            if t >= 0.5 {
                b.color.clone()
            } else {
                a.color.clone()
            }
        }
    };
    Shadow {
        color,
        offset_x: lerp_length(&a.offset_x, &b.offset_x, t),
        offset_y: lerp_length(&a.offset_y, &b.offset_y, t),
        blur: lerp_length(&a.blur, &b.blur, t),
        spread: lerp_length(&a.spread, &b.spread, t),
        inset: if t >= 0.5 { b.inset } else { a.inset },
    }
}

/// Shadow lists, padded with transparent shadows when lengths differ.
#[must_use]
pub fn shadow_list(a: &Value, b: &Value, t: f32) -> Value {
    let (Value::List(xs), Value::List(ys)) = (a, b) else {
        return discrete(a, b, t);
    };
    let count = xs.len().max(ys.len());
    let mut out: Vec<Value> = Vec::with_capacity(count);
    for index in 0..count {
        let x = xs.get(index);
        let y = ys.get(index);
        match (x, y) {
            (Some(Value::Shadow(x)), Some(Value::Shadow(y))) => {
                out.push(Value::Shadow(lerp_shadow(x, y, t)));
            }
            (Some(Value::Shadow(x)), None) => {
                out.push(Value::Shadow(lerp_shadow(x, &transparent_shadow(x), t)));
            }
            (None, Some(Value::Shadow(y))) => {
                out.push(Value::Shadow(lerp_shadow(&transparent_shadow(y), y, t)));
            }
            _ => return discrete(a, b, t),
        }
    }
    Value::List(out.into())
}

/// Images: stop-wise when both are structurally identical gradients,
/// otherwise discrete.
#[must_use]
pub fn image(a: &Value, b: &Value, t: f32) -> Value {
    let (Value::Image(Image::Gradient(x)), Value::Image(Image::Gradient(y))) = (a, b) else {
        return discrete(a, b, t);
    };
    if x.kind != y.kind || x.repeating != y.repeating || x.stops.len() != y.stops.len() {
        return discrete(a, b, t);
    }
    let mut stops = Vec::with_capacity(x.stops.len());
    for (from, to) in x.stops.iter().zip(y.stops.iter()) {
        let color = match (&from.color, &to.color) {
            (ColorValue::Absolute(p), ColorValue::Absolute(q)) => {
                ColorValue::Absolute(lerp_rgba(*p, *q, t))
            }
            _ => return discrete(a, b, t),
        };
        let position = match (&from.position, &to.position) {
            (Some(p), Some(q)) => Some(lerp_length(p, q, t)),
            (None, None) => None,
            _ => return discrete(a, b, t),
        };
        stops.push(super::image::ColorStop {
            color,
            position,
            hint: from.hint.clone(),
        });
    }
    Value::Image(Image::Gradient(Rc::new(super::image::Gradient {
        kind: x.kind.clone(),
        repeating: x.repeating,
        stops: stops.into(),
        interpolation: x.interpolation,
    })))
}

fn lerp_transform_fn(a: &TransformFn, b: &TransformFn, t: f32) -> Option<TransformFn> {
    Some(match (a, b) {
        (TransformFn::Translate(ax, ay), TransformFn::Translate(bx, by)) => {
            TransformFn::Translate(lerp_length(ax, bx, t), lerp_length(ay, by, t))
        }
        (TransformFn::TranslateX(x), TransformFn::TranslateX(y)) => {
            TransformFn::TranslateX(lerp_length(x, y, t))
        }
        (TransformFn::TranslateY(x), TransformFn::TranslateY(y)) => {
            TransformFn::TranslateY(lerp_length(x, y, t))
        }
        (TransformFn::Scale(ax, ay), TransformFn::Scale(bx, by)) => {
            TransformFn::Scale(lerp(*ax, *bx, t), lerp(*ay, *by, t))
        }
        (TransformFn::Rotate(x), TransformFn::Rotate(y)) => TransformFn::Rotate(lerp(*x, *y, t)),
        (TransformFn::SkewX(x), TransformFn::SkewX(y)) => TransformFn::SkewX(lerp(*x, *y, t)),
        (TransformFn::SkewY(x), TransformFn::SkewY(y)) => TransformFn::SkewY(lerp(*x, *y, t)),
        (TransformFn::Skew(ax, ay), TransformFn::Skew(bx, by)) => {
            TransformFn::Skew(lerp(*ax, *bx, t), lerp(*ay, *by, t))
        }
        _ => return None,
    })
}

/// Transform lists: component-wise when the structures match, otherwise
/// via 2-D matrix decomposition.
#[must_use]
pub fn transform(a: &Value, b: &Value, t: f32) -> Value {
    let (Value::Transform(xs), Value::Transform(ys)) = (a, b) else {
        return discrete(a, b, t);
    };
    if xs.len() == ys.len() {
        let mut out = Vec::with_capacity(xs.len());
        let mut matched = true;
        for (x, y) in xs.iter().zip(ys.iter()) {
            match lerp_transform_fn(x, y, t) {
                Some(function) => out.push(function),
                None => {
                    matched = false;
                    break;
                }
            }
        }
        if matched {
            return Value::Transform(out.into());
        }
    }
    let ctx = super::length::LengthCtx::default();
    let from = super::transform::transform_list_matrix(xs, &ctx, (0.0, 0.0), (0.0, 0.0));
    let to = super::transform::transform_list_matrix(ys, &ctx, (0.0, 0.0), (0.0, 0.0));
    let (Some(from), Some(to)) = (decompose_2d(&from), decompose_2d(&to)) else {
        return discrete(a, b, t);
    };
    let blended = super::transform::Decomposed2d {
        tx: lerp(from.tx, to.tx, t),
        ty: lerp(from.ty, to.ty, t),
        rotate: lerp(from.rotate, to.rotate, t),
        scale_x: lerp(from.scale_x, to.scale_x, t),
        scale_y: lerp(from.scale_y, to.scale_y, t),
        skew_xy: lerp(from.skew_xy, to.skew_xy, t),
    };
    let matrix = recompose_2d(&blended);
    Value::Transform(
        vec![TransformFn::Matrix([
            matrix.values[0],
            matrix.values[3],
            matrix.values[1],
            matrix.values[4],
            matrix.values[2],
            matrix.values[5],
        ])]
        .into(),
    )
}

/// Filter lists: component-wise when the function sequences match.
#[must_use]
pub fn filter_list(a: &Value, b: &Value, t: f32) -> Value {
    let (Value::Filter(xs), Value::Filter(ys)) = (a, b) else {
        return discrete(a, b, t);
    };
    if xs.len() != ys.len() {
        return discrete(a, b, t);
    }
    use super::filter::FilterFn;
    let mut out = Vec::with_capacity(xs.len());
    for (x, y) in xs.iter().zip(ys.iter()) {
        let blended = match (x, y) {
            (FilterFn::Blur(p), FilterFn::Blur(q)) => FilterFn::Blur(lerp_length(p, q, t)),
            (FilterFn::Brightness(p), FilterFn::Brightness(q)) => {
                FilterFn::Brightness(lerp(*p, *q, t))
            }
            (FilterFn::Contrast(p), FilterFn::Contrast(q)) => FilterFn::Contrast(lerp(*p, *q, t)),
            (FilterFn::Grayscale(p), FilterFn::Grayscale(q)) => {
                FilterFn::Grayscale(lerp(*p, *q, t))
            }
            (FilterFn::HueRotate(p), FilterFn::HueRotate(q)) => {
                FilterFn::HueRotate(lerp(*p, *q, t))
            }
            (FilterFn::Invert(p), FilterFn::Invert(q)) => FilterFn::Invert(lerp(*p, *q, t)),
            (FilterFn::Opacity(p), FilterFn::Opacity(q)) => FilterFn::Opacity(lerp(*p, *q, t)),
            (FilterFn::Saturate(p), FilterFn::Saturate(q)) => FilterFn::Saturate(lerp(*p, *q, t)),
            (FilterFn::Sepia(p), FilterFn::Sepia(q)) => FilterFn::Sepia(lerp(*p, *q, t)),
            (FilterFn::DropShadow(p), FilterFn::DropShadow(q)) => {
                FilterFn::DropShadow(lerp_shadow(p, q, t))
            }
            _ => return discrete(a, b, t),
        };
        out.push(blended);
    }
    Value::Filter(out.into())
}

/// Comma lists: component-wise, repeating the shorter list to the longer
/// one (CSS Backgrounds L3's list-repetition rule).
#[must_use]
pub fn list(a: &Value, b: &Value, t: f32) -> Value {
    let (Value::List(xs), Value::List(ys)) = (a, b) else {
        return discrete(a, b, t);
    };
    if xs.is_empty() || ys.is_empty() {
        return discrete(a, b, t);
    }
    let count = xs.len().max(ys.len());
    let mut out = Vec::with_capacity(count);
    for index in 0..count {
        let x = &xs[index % xs.len()];
        let y = &ys[index % ys.len()];
        out.push(match (x, y) {
            (Value::Length(_), Value::Length(_)) => length(x, y, t),
            (Value::Number(_), Value::Number(_))
            | (Value::Percentage(_), Value::Percentage(_))
            | (Value::Angle(_), Value::Angle(_)) => number(x, y, t),
            (Value::Color(_), Value::Color(_)) => color(x, y, t),
            (Value::Image(_), Value::Image(_)) => image(x, y, t),
            (Value::Position(p), Value::Position(q)) => Value::Position(Position {
                x: lerp_length(&p.x, &q.x, t),
                y: lerp_length(&p.y, &q.y, t),
                z: None,
            }),
            (Value::BgSize(BgSize::Explicit(pw, ph)), Value::BgSize(BgSize::Explicit(qw, qh))) => {
                Value::BgSize(BgSize::Explicit(
                    lerp_length(pw, qw, t),
                    lerp_length(ph, qh, t),
                ))
            }
            _ => discrete(x, y, t),
        });
    }
    Value::List(out.into())
}

/// Font weights, which compute to `Absolute` before they reach here.
#[must_use]
pub fn font_weight(a: &Value, b: &Value, t: f32) -> Value {
    match (a, b) {
        (
            Value::FontWeight(FontWeight::Absolute(x)),
            Value::FontWeight(FontWeight::Absolute(y)),
        ) => Value::FontWeight(FontWeight::Absolute(lerp(*x, *y, t).clamp(1.0, 1000.0))),
        _ => discrete(a, b, t),
    }
}

/// Line heights, which interpolate only within the same form.
#[must_use]
pub fn line_height(a: &Value, b: &Value, t: f32) -> Value {
    match (a, b) {
        (Value::LineHeight(LineHeight::Number(x)), Value::LineHeight(LineHeight::Number(y))) => {
            Value::LineHeight(LineHeight::Number(lerp(*x, *y, t)))
        }
        (Value::LineHeight(LineHeight::Length(x)), Value::LineHeight(LineHeight::Length(y))) => {
            Value::LineHeight(LineHeight::Length(lerp_length(x, y, t)))
        }
        _ => discrete(a, b, t),
    }
}

#[cfg(test)]
mod tests {
    use super::{color, discrete, length, list, number, shadow_list, transform};
    use crate::css::value::Value;
    use crate::css::value::color::{ColorValue, Rgba};
    use crate::css::value::keyword::Keyword;
    use crate::css::value::length::Length;
    use crate::css::value::shadow::Shadow;
    use std::rc::Rc;

    fn rgba(r: f32, g: f32, b: f32, a: f32) -> Value {
        Value::Color(ColorValue::Absolute(Rgba { r, g, b, a }))
    }

    #[test]
    fn discrete_flips_at_the_halfway_point() {
        let a = Value::Keyword(Keyword::None);
        let b = Value::Keyword(Keyword::Solid);
        assert_eq!(discrete(&a, &b, 0.0), a);
        assert_eq!(discrete(&a, &b, 0.49), a);
        assert_eq!(discrete(&a, &b, 0.5), b);
        assert_eq!(discrete(&a, &b, 1.0), b);
    }

    #[test]
    fn numbers_and_same_unit_lengths_interpolate_componentwise() {
        assert_eq!(
            number(&Value::Number(0.0), &Value::Number(10.0), 0.25),
            Value::Number(2.5)
        );
        assert_eq!(
            length(
                &Value::Length(Length::px(0.0)),
                &Value::Length(Length::px(8.0)),
                0.5
            ),
            Value::Length(Length::px(4.0))
        );
        // Mismatched units fall back to a calc sum rather than panicking.
        let mixed = length(
            &Value::Length(Length::px(0.0)),
            &Value::Length(Length::Percent(1.0)),
            0.5,
        );
        assert!(matches!(mixed, Value::Length(Length::Calc(_))));
    }

    #[test]
    fn colours_interpolate_in_premultiplied_srgb() {
        // GTK interpolates premultiplied: halfway from transparent black to
        // opaque white is 50% alpha with premultiplied-white channels.
        let midpoint = color(&rgba(0.0, 0.0, 0.0, 0.0), &rgba(1.0, 1.0, 1.0, 1.0), 0.5);
        match midpoint {
            Value::Color(ColorValue::Absolute(rgba)) => {
                assert!((rgba.a - 0.5).abs() < 1e-6, "{rgba:?}");
                assert!((rgba.r - 1.0).abs() < 1e-6, "{rgba:?}");
            }
            other => panic!("expected an absolute colour, got {other:?}"),
        }
        assert_eq!(
            color(&rgba(0.0, 0.0, 0.0, 1.0), &rgba(1.0, 1.0, 1.0, 1.0), 0.0),
            rgba(0.0, 0.0, 0.0, 1.0)
        );
    }

    #[test]
    fn shadow_lists_pad_the_shorter_side_with_transparent_shadows() {
        let one = Value::List(
            vec![Value::Shadow(Shadow {
                color: Some(ColorValue::Absolute(Rgba {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 1.0,
                })),
                offset_x: Length::px(0.0),
                offset_y: Length::px(4.0),
                blur: Length::zero(),
                spread: Length::zero(),
                inset: false,
            })]
            .into(),
        );
        let none = Value::List(Rc::from(Vec::new()));
        let mixed = shadow_list(&none, &one, 0.5);
        match mixed {
            Value::List(items) => assert_eq!(items.len(), 1),
            other => panic!("expected a list, got {other:?}"),
        }
    }

    #[test]
    fn a_pair_the_interpolator_cannot_handle_falls_back_to_discrete() {
        let a = Value::Number(1.0);
        let b = Value::Keyword(Keyword::None);
        assert_eq!(number(&a, &b, 0.75), b);
        assert_eq!(transform(&a, &b, 0.75), b);
        assert_eq!(list(&a, &b, 0.1), a);
    }

    #[test]
    fn interpolators_never_panic_on_mismatched_values() {
        let samples = [
            Value::Number(1.0),
            Value::Percentage(0.5),
            Value::Length(Length::Percent(0.5)),
            Value::Keyword(Keyword::None),
            Value::List(Rc::from(Vec::new())),
            rgba(0.5, 0.5, 0.5, 0.5),
        ];
        for a in &samples {
            for b in &samples {
                for t in [-1.0_f32, 0.0, 0.5, 1.0, 2.0] {
                    let _ = super::discrete(a, b, t);
                    let _ = super::number(a, b, t);
                    let _ = super::length(a, b, t);
                    let _ = super::percentage(a, b, t);
                    let _ = super::color(a, b, t);
                    let _ = super::pair(a, b, t);
                    let _ = super::shadow_list(a, b, t);
                    let _ = super::image(a, b, t);
                    let _ = super::transform(a, b, t);
                    let _ = super::filter_list(a, b, t);
                    let _ = super::list(a, b, t);
                    let _ = super::font_weight(a, b, t);
                    let _ = super::line_height(a, b, t);
                }
            }
        }
    }
}
