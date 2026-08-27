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
        // `auto` is not a length and has no numeric value to blend toward,
        // so CSS makes it a *discrete* endpoint. Folding it into the calc
        // sum below produced a `CalcNode` that can never resolve, which the
        // used-value accessors then read as 0 for the whole transition --
        // an `auto` margin snapped hard against its edge and stayed there.
        (Length::Auto, _) | (_, Length::Auto) => {
            if t >= 0.5 {
                b.clone()
            } else {
                a.clone()
            }
        }
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

/// Two colours interpolated in premultiplied sRGB, the one definition
/// `color-mix()` and every colour transition share.
#[must_use]
pub fn lerp_rgba(a: Rgba, b: Rgba, t: f32) -> Rgba {
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

/// Component-wise interpolation of two transform primitives of the same
/// kind, per CSS Transforms 1 §interpolation of transform functions.
///
/// Every primitive the parser can produce is handled except `matrix()` and
/// `matrix3d()`, which CSS interpolates by decomposition rather than
/// component-wise, and which the caller's matrix path already covers.
/// Leaving the *Z*, `*3d` and axis-suffixed forms out meant a structurally
/// identical `rotateZ(0deg)` -> `rotateZ(360deg)` pair fell into that path,
/// where both endpoints flatten to the identity matrix and the element never
/// span at all.
fn lerp_transform_fn(a: &TransformFn, b: &TransformFn, t: f32) -> Option<TransformFn> {
    use TransformFn as F;
    Some(match (a, b) {
        (F::Translate(ax, ay), F::Translate(bx, by)) => {
            F::Translate(lerp_length(ax, bx, t), lerp_length(ay, by, t))
        }
        (F::TranslateX(x), F::TranslateX(y)) => F::TranslateX(lerp_length(x, y, t)),
        (F::TranslateY(x), F::TranslateY(y)) => F::TranslateY(lerp_length(x, y, t)),
        (F::TranslateZ(x), F::TranslateZ(y)) => F::TranslateZ(lerp_length(x, y, t)),
        (F::Translate3d(ax, ay, az), F::Translate3d(bx, by, bz)) => F::Translate3d(
            lerp_length(ax, bx, t),
            lerp_length(ay, by, t),
            lerp_length(az, bz, t),
        ),
        (F::Scale(ax, ay), F::Scale(bx, by)) => F::Scale(lerp(*ax, *bx, t), lerp(*ay, *by, t)),
        (F::ScaleX(x), F::ScaleX(y)) => F::ScaleX(lerp(*x, *y, t)),
        (F::ScaleY(x), F::ScaleY(y)) => F::ScaleY(lerp(*x, *y, t)),
        (F::ScaleZ(x), F::ScaleZ(y)) => F::ScaleZ(lerp(*x, *y, t)),
        (F::Scale3d(ax, ay, az), F::Scale3d(bx, by, bz)) => {
            F::Scale3d(lerp(*ax, *bx, t), lerp(*ay, *by, t), lerp(*az, *bz, t))
        }
        // `rotate()` and `rotateZ()` are the same primitive spelled two ways,
        // so a mixed pair interpolates as one; the result takes the 2-D
        // spelling, which is what the painter reads.
        (F::Rotate(x) | F::RotateZ(x), F::Rotate(y) | F::RotateZ(y)) => F::Rotate(lerp(*x, *y, t)),
        (F::RotateX(x), F::RotateX(y)) => F::RotateX(lerp(*x, *y, t)),
        (F::RotateY(x), F::RotateY(y)) => F::RotateY(lerp(*x, *y, t)),
        // Only a shared axis interpolates component-wise; a different axis is
        // a different primitive and falls through to the matrix path.
        (F::Rotate3d(ax, ay, az, angle_a), F::Rotate3d(bx, by, bz, angle_b))
            if (ax, ay, az) == (bx, by, bz) =>
        {
            F::Rotate3d(*ax, *ay, *az, lerp(*angle_a, *angle_b, t))
        }
        (F::SkewX(x), F::SkewX(y)) => F::SkewX(lerp(*x, *y, t)),
        (F::SkewY(x), F::SkewY(y)) => F::SkewY(lerp(*x, *y, t)),
        (F::Skew(ax, ay), F::Skew(bx, by)) => F::Skew(lerp(*ax, *bx, t), lerp(*ay, *by, t)),
        (F::Perspective(x), F::Perspective(y)) => F::Perspective(lerp_length(x, y, t)),
        _ => return None,
    })
}

/// The identity value of one transform primitive: what CSS Transforms 1
/// substitutes for a missing function when one endpoint's list is shorter --
/// which is what makes `transform: none` -> `rotate(1turn)` a real rotation
/// rather than a discrete flip.
///
/// `perspective()` has no finite identity (its neutral element is
/// `perspective(infinity)`), so a list containing one is not padded.
fn identity_like(function: &TransformFn) -> Option<TransformFn> {
    use TransformFn as F;
    Some(match function {
        F::Matrix(_) => F::Matrix([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]),
        F::Matrix3d(_) => {
            let mut identity = [0.0_f32; 16];
            for index in [0, 5, 10, 15] {
                identity[index] = 1.0;
            }
            F::Matrix3d(identity)
        }
        F::Translate(..) => F::Translate(Length::zero(), Length::zero()),
        F::TranslateX(_) => F::TranslateX(Length::zero()),
        F::TranslateY(_) => F::TranslateY(Length::zero()),
        F::TranslateZ(_) => F::TranslateZ(Length::zero()),
        F::Translate3d(..) => F::Translate3d(Length::zero(), Length::zero(), Length::zero()),
        F::Scale(..) => F::Scale(1.0, 1.0),
        F::ScaleX(_) => F::ScaleX(1.0),
        F::ScaleY(_) => F::ScaleY(1.0),
        F::ScaleZ(_) => F::ScaleZ(1.0),
        F::Scale3d(..) => F::Scale3d(1.0, 1.0, 1.0),
        F::Rotate(_) => F::Rotate(0.0),
        F::RotateX(_) => F::RotateX(0.0),
        F::RotateY(_) => F::RotateY(0.0),
        F::RotateZ(_) => F::RotateZ(0.0),
        F::Rotate3d(x, y, z, _) => F::Rotate3d(*x, *y, *z, 0.0),
        F::Skew(..) => F::Skew(0.0, 0.0),
        F::SkewX(_) => F::SkewX(0.0),
        F::SkewY(_) => F::SkewY(0.0),
        F::Perspective(_) => return None,
    })
}

/// Whether every length in `list` is already absolute.
///
/// `computed` absolutises transform lengths except percentages, so a
/// percentage is the only thing left that the matrix path cannot honour:
/// it flattens with a `(0, 0)` basis, which silently zeroes the translation.
fn is_matrix_safe(list: &[TransformFn]) -> bool {
    fn absolute(length: &Length) -> bool {
        matches!(length, Length::Abs { .. })
    }
    list.iter().all(|function| {
        use TransformFn as F;
        match function {
            F::Translate(x, y) => absolute(x) && absolute(y),
            F::TranslateX(x) | F::TranslateY(x) | F::TranslateZ(x) | F::Perspective(x) => {
                absolute(x)
            }
            F::Translate3d(x, y, z) => absolute(x) && absolute(y) && absolute(z),
            _ => true,
        }
    })
}

/// Transform lists: component-wise when the structures match, otherwise
/// via 2-D matrix decomposition.
#[must_use]
pub fn transform(a: &Value, b: &Value, t: f32) -> Value {
    let (Value::Transform(xs), Value::Transform(ys)) = (a, b) else {
        return discrete(a, b, t);
    };
    // `transform: none` is the empty list. CSS Transforms 1 pads the shorter
    // side with each missing function's identity value rather than falling
    // through to matrix decomposition, which is what makes a spinner's
    // `none` -> `rotate(1turn)` an actual rotation instead of two identity
    // matrices that decompose to the same zero angle.
    let padded: Option<Rc<[TransformFn]>> = match (xs.is_empty(), ys.is_empty()) {
        (true, false) => ys.iter().map(identity_like).collect(),
        (false, true) => xs.iter().map(identity_like).collect(),
        _ => None,
    };
    let (xs, ys) = match (&padded, xs.is_empty()) {
        (Some(identities), true) => (identities, ys),
        (Some(identities), false) => (xs, identities),
        (None, _) => (xs, ys),
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
    // The matrix fallback flattens both endpoints with a `(0, 0)` percentage
    // basis, so a percentage translation would silently become zero and the
    // element would snap untranslated at t = 0. A discrete flip is the honest
    // answer there; every other length is already absolute by computed time.
    if !is_matrix_safe(xs) || !is_matrix_safe(ys) {
        return discrete(a, b, t);
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

#[cfg(test)]
mod review_tests {
    use std::rc::Rc;

    use super::{Value, length, transform};
    use crate::css::value::length::{Length, LengthCtx};
    use crate::css::value::transform::{
        TransformFn, decompose_2d, recompose_2d, transform_list_matrix,
    };

    fn transforms(list: Vec<TransformFn>) -> Value {
        Value::Transform(Rc::from(list))
    }

    fn only(value: &Value) -> &[TransformFn] {
        match value {
            Value::Transform(list) => list,
            other => panic!("expected a transform list, got {other:?}"),
        }
    }

    // F18/F42: `auto` is a discrete endpoint. Blending it into a calc sum
    // produced a value that never resolves, read as 0 for the whole
    // transition.
    #[test]
    fn an_auto_length_endpoint_flips_discretely_instead_of_becoming_a_dead_calc() {
        let auto = Value::Length(Length::Auto);
        let twenty = Value::Length(Length::px(20.0));
        let ctx = LengthCtx {
            percent_basis: Some(100.0),
            ..LengthCtx::default()
        };
        let resolves = |value: &Value| match value {
            Value::Length(Length::Auto) => Some(f32::NAN),
            Value::Length(other) => other.resolve(&ctx),
            _ => None,
        };
        for t in [0.0_f32, 0.25, 0.49] {
            assert_eq!(length(&auto, &twenty, t), auto);
        }
        for t in [0.5_f32, 0.75, 1.0] {
            assert_eq!(length(&auto, &twenty, t), twenty);
        }
        // Whatever it flips to must actually resolve -- the old calc did not.
        assert!(resolves(&length(&auto, &twenty, 0.75)).is_some_and(f32::is_finite));
        // Two ordinary lengths still interpolate continuously.
        let ten = Value::Length(Length::px(10.0));
        assert_eq!(length(&ten, &twenty, 0.5), Value::Length(Length::px(15.0)));
    }

    // F43: `rotateZ` is `rotate` under another name, and every other
    // primitive interpolates component-wise too. Falling into matrix
    // decomposition lost whole turns.
    #[test]
    fn every_transform_primitive_interpolates_component_wise() {
        let spun = transform(
            &transforms(vec![TransformFn::RotateZ(0.0)]),
            &transforms(vec![TransformFn::RotateZ(360.0)]),
            0.25,
        );
        assert_eq!(only(&spun), &[TransformFn::Rotate(90.0)]);
        // A mixed `rotate`/`rotateZ` pair is one primitive.
        let mixed = transform(
            &transforms(vec![TransformFn::Rotate(0.0)]),
            &transforms(vec![TransformFn::RotateZ(360.0)]),
            0.5,
        );
        assert_eq!(only(&mixed), &[TransformFn::Rotate(180.0)]);

        let scaled = transform(
            &transforms(vec![TransformFn::Scale3d(1.0, 1.0, 1.0)]),
            &transforms(vec![TransformFn::Scale3d(3.0, 3.0, 3.0)]),
            0.5,
        );
        assert_eq!(only(&scaled), &[TransformFn::Scale3d(2.0, 2.0, 2.0)]);
        for (from, to, half) in [
            (
                TransformFn::ScaleX(1.0),
                TransformFn::ScaleX(3.0),
                TransformFn::ScaleX(2.0),
            ),
            (
                TransformFn::ScaleY(1.0),
                TransformFn::ScaleY(3.0),
                TransformFn::ScaleY(2.0),
            ),
            (
                TransformFn::RotateX(0.0),
                TransformFn::RotateX(90.0),
                TransformFn::RotateX(45.0),
            ),
            (
                TransformFn::RotateY(0.0),
                TransformFn::RotateY(90.0),
                TransformFn::RotateY(45.0),
            ),
            (
                TransformFn::TranslateZ(Length::px(0.0)),
                TransformFn::TranslateZ(Length::px(10.0)),
                TransformFn::TranslateZ(Length::px(5.0)),
            ),
            (
                TransformFn::Rotate3d(0.0, 1.0, 0.0, 0.0),
                TransformFn::Rotate3d(0.0, 1.0, 0.0, 90.0),
                TransformFn::Rotate3d(0.0, 1.0, 0.0, 45.0),
            ),
        ] {
            let blended = transform(&transforms(vec![from]), &transforms(vec![to]), 0.5);
            assert_eq!(only(&blended), &[half]);
        }
    }

    // F43, the `none` half: `transform: none` is the empty list and must be
    // padded with each missing function's identity, or a spinner's
    // `none` -> `rotate(1turn)` decomposes to two identical zero angles.
    #[test]
    fn none_pads_with_identities_so_a_full_turn_still_animates() {
        let quarter = transform(
            &transforms(Vec::new()),
            &transforms(vec![TransformFn::Rotate(360.0)]),
            0.25,
        );
        assert_eq!(only(&quarter), &[TransformFn::Rotate(90.0)]);
        // And in the other direction.
        let back = transform(
            &transforms(vec![TransformFn::Rotate(360.0)]),
            &transforms(Vec::new()),
            0.25,
        );
        assert_eq!(only(&back), &[TransformFn::Rotate(270.0)]);
        // A padded translate starts from zero, not from a decomposed matrix.
        let slid = transform(
            &transforms(Vec::new()),
            &transforms(vec![TransformFn::TranslateX(Length::px(40.0))]),
            0.5,
        );
        assert_eq!(only(&slid), &[TransformFn::TranslateX(Length::px(20.0))]);
    }

    // F44: the matrix fallback flattens with a (0, 0) percentage basis, so a
    // percentage translation would silently vanish. A discrete flip is the
    // honest answer; every other length is absolute by computed time.
    #[test]
    fn a_percentage_translation_never_takes_the_lossy_matrix_path() {
        let from = transforms(vec![TransformFn::Translate(
            Length::Percent(0.5),
            Length::px(0.0),
        )]);
        let to = transforms(vec![
            TransformFn::Translate(Length::Percent(0.5), Length::px(0.0)),
            TransformFn::Rotate(10.0),
        ]);
        // Structures differ, so the matrix path would be taken -- and would
        // zero the 50%. Discrete keeps the declared value instead.
        assert_eq!(transform(&from, &to, 0.25), from);
        assert_eq!(transform(&from, &to, 0.75), to);

        // With absolute lengths the matrix path still runs.
        let px_from = transforms(vec![TransformFn::Translate(
            Length::px(10.0),
            Length::px(0.0),
        )]);
        let px_to = transforms(vec![
            TransformFn::Translate(Length::px(10.0), Length::px(0.0)),
            TransformFn::Rotate(10.0),
        ]);
        assert!(matches!(
            only(&transform(&px_from, &px_to, 0.5)),
            [TransformFn::Matrix(_)]
        ));
    }

    // F57: a negative-determinant matrix must round-trip through
    // decompose/recompose; the skew used to come back with the wrong sign.
    #[test]
    fn a_reflecting_matrix_round_trips_through_decomposition() {
        let ctx = LengthCtx::default();
        for list in [
            vec![TransformFn::Scale(1.0, -1.0), TransformFn::SkewX(45.0)],
            vec![TransformFn::Scale(-1.0, 1.0), TransformFn::SkewY(30.0)],
            vec![TransformFn::Scale(1.0, -1.0), TransformFn::Rotate(30.0)],
            vec![TransformFn::Scale(2.0, 3.0), TransformFn::SkewX(20.0)],
        ] {
            let matrix = transform_list_matrix(&list, &ctx, (0.0, 0.0), (0.0, 0.0));
            let decomposed = decompose_2d(&matrix).expect("decomposes");
            let round_tripped = recompose_2d(&decomposed);
            for (index, (left, right)) in matrix
                .values
                .iter()
                .zip(round_tripped.values.iter())
                .enumerate()
            {
                assert!(
                    (left - right).abs() < 1e-5,
                    "{list:?} slot {index}: {left} != {right}"
                );
            }
        }
    }
}
