//! `transform` values, matrix flattening and 2-D decomposition.

use std::rc::Rc;

use cssparser::Parser;
use skia_rs_safe::core::Matrix;

use super::calc::parse_angle;
use super::length::{Length, LengthCtx};

/// One `<transform-function>`.
#[derive(Clone, Debug, PartialEq)]
pub enum TransformFn {
    /// `matrix(a, b, c, d, e, f)`.
    Matrix([f32; 6]),
    /// `matrix3d(...)`, column-major as CSS writes it.
    Matrix3d([f32; 16]),
    /// `translate(x, y)`.
    Translate(Length, Length),
    /// `translateX(x)`.
    TranslateX(Length),
    /// `translateY(y)`.
    TranslateY(Length),
    /// `translateZ(z)`.
    TranslateZ(Length),
    /// `translate3d(x, y, z)`.
    Translate3d(Length, Length, Length),
    /// `scale(x, y)`.
    Scale(f32, f32),
    /// `scaleX(x)`.
    ScaleX(f32),
    /// `scaleY(y)`.
    ScaleY(f32),
    /// `scaleZ(z)`.
    ScaleZ(f32),
    /// `scale3d(x, y, z)`.
    Scale3d(f32, f32, f32),
    /// `rotate(<angle>)`, degrees.
    Rotate(f32),
    /// `rotateX(<angle>)`.
    RotateX(f32),
    /// `rotateY(<angle>)`.
    RotateY(f32),
    /// `rotateZ(<angle>)`.
    RotateZ(f32),
    /// `rotate3d(x, y, z, <angle>)`.
    Rotate3d(f32, f32, f32, f32),
    /// `skew(ax, ay)`, degrees.
    Skew(f32, f32),
    /// `skewX(ax)`.
    SkewX(f32),
    /// `skewY(ay)`.
    SkewY(f32),
    /// `perspective(<length>)`.
    Perspective(Length),
}

fn matrix_of(scale_x: f32, skew_y: f32, skew_x: f32, scale_y: f32, tx: f32, ty: f32) -> Matrix {
    Matrix {
        values: [scale_x, skew_x, tx, skew_y, scale_y, ty, 0.0, 0.0, 1.0],
    }
}

impl TransformFn {
    /// The 3x3 matrix for this function. M2 paints in 2-D: the `*Z`/3-D
    /// forms collapse to their in-plane effect.
    #[must_use]
    pub fn to_matrix(&self, ctx: &LengthCtx, basis: (f32, f32)) -> Matrix {
        let horizontal = LengthCtx {
            percent_basis: Some(basis.0),
            ..*ctx
        };
        let vertical = LengthCtx {
            percent_basis: Some(basis.1),
            ..*ctx
        };
        let px = |length: &Length, ctx: &LengthCtx| length.resolve(ctx).unwrap_or(0.0);
        match self {
            TransformFn::Matrix(m) => matrix_of(m[0], m[1], m[2], m[3], m[4], m[5]),
            TransformFn::Matrix3d(m) => matrix_of(m[0], m[1], m[4], m[5], m[12], m[13]),
            TransformFn::Translate(x, y) => Matrix::translate(px(x, &horizontal), px(y, &vertical)),
            TransformFn::TranslateX(x) => Matrix::translate(px(x, &horizontal), 0.0),
            TransformFn::TranslateY(y) => Matrix::translate(0.0, px(y, &vertical)),
            TransformFn::TranslateZ(_) => Matrix::IDENTITY,
            TransformFn::Translate3d(x, y, _) => {
                Matrix::translate(px(x, &horizontal), px(y, &vertical))
            }
            TransformFn::Scale(x, y) => Matrix::scale(*x, *y),
            TransformFn::ScaleX(x) => Matrix::scale(*x, 1.0),
            TransformFn::ScaleY(y) => Matrix::scale(1.0, *y),
            TransformFn::ScaleZ(_) => Matrix::IDENTITY,
            TransformFn::Scale3d(x, y, _) => Matrix::scale(*x, *y),
            TransformFn::Rotate(degrees) | TransformFn::RotateZ(degrees) => {
                Matrix::rotate(degrees.to_radians())
            }
            TransformFn::RotateX(_) | TransformFn::RotateY(_) => Matrix::IDENTITY,
            TransformFn::Rotate3d(_, _, z, degrees) => {
                if *z == 0.0 {
                    Matrix::IDENTITY
                } else {
                    Matrix::rotate(degrees.to_radians() * z.signum())
                }
            }
            TransformFn::Skew(ax, ay) => matrix_of(
                1.0,
                ay.to_radians().tan(),
                ax.to_radians().tan(),
                1.0,
                0.0,
                0.0,
            ),
            TransformFn::SkewX(ax) => matrix_of(1.0, 0.0, ax.to_radians().tan(), 1.0, 0.0, 0.0),
            TransformFn::SkewY(ay) => matrix_of(1.0, ay.to_radians().tan(), 0.0, 1.0, 0.0, 0.0),
            TransformFn::Perspective(_) => Matrix::IDENTITY,
        }
    }
}

/// Flatten a list to one matrix, applied about `origin`.
#[must_use]
pub fn transform_list_matrix(
    list: &[TransformFn],
    ctx: &LengthCtx,
    basis: (f32, f32),
    origin: (f32, f32),
) -> Matrix {
    let mut matrix = Matrix::translate(origin.0, origin.1);
    for function in list {
        matrix = matrix.concat(&function.to_matrix(ctx, basis));
    }
    matrix.concat(&Matrix::translate(-origin.0, -origin.1))
}

/// A 2-D matrix's translate/rotate/scale/skew components.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Decomposed2d {
    /// Horizontal translation.
    pub tx: f32,
    /// Vertical translation.
    pub ty: f32,
    /// Rotation in degrees.
    pub rotate: f32,
    /// Horizontal scale.
    pub scale_x: f32,
    /// Vertical scale.
    pub scale_y: f32,
    /// The XY skew factor.
    pub skew_xy: f32,
}

/// Decompose an affine matrix. skia-rs has no decomposition of its own;
/// this is used only when interpolating structurally-mismatched lists.
#[must_use]
pub fn decompose_2d(matrix: &Matrix) -> Option<Decomposed2d> {
    let (a, c, e) = (matrix.values[0], matrix.values[1], matrix.values[2]);
    let (b, d, f) = (matrix.values[3], matrix.values[4], matrix.values[5]);
    let determinant = a * d - b * c;
    if !determinant.is_finite() || determinant == 0.0 {
        return None;
    }
    // A negative determinant is a reflection. Factoring it out of the *first
    // column* before the Gram-Schmidt step -- rather than negating `scale_y`
    // after it -- is what makes the decomposition round-trip: negating the
    // column is right-multiplying by `diag(-1, 1)`, whose inverse is folded
    // back into `scale_x` below, leaving the rotation and the skew alone.
    // Negating `scale_y` afterwards left a skew of the wrong sign, so
    // `scale(1,-1) skewX(45deg)` recomposed to a mirrored shear and the
    // element jumped at t = 0.
    let flipped = determinant < 0.0;
    let (a, b) = if flipped { (-a, -b) } else { (a, b) };
    let scale_x = (a * a + b * b).sqrt();
    if scale_x == 0.0 {
        return None;
    }
    let (a_n, b_n) = (a / scale_x, b / scale_x);
    let skew = a_n * c + b_n * d;
    let (c_s, d_s) = (c - a_n * skew, d - b_n * skew);
    let scale_y = (c_s * c_s + d_s * d_s).sqrt();
    if scale_y == 0.0 {
        return None;
    }
    let skew_xy = skew / scale_y;
    Some(Decomposed2d {
        tx: e,
        ty: f,
        rotate: b_n.atan2(a_n).to_degrees(),
        scale_x: if flipped { -scale_x } else { scale_x },
        scale_y,
        skew_xy,
    })
}

/// Rebuild a matrix from its components.
#[must_use]
pub fn recompose_2d(d: &Decomposed2d) -> Matrix {
    let (sin, cos) = d.rotate.to_radians().sin_cos();
    let rotation = matrix_of(cos, sin, -sin, cos, 0.0, 0.0);
    let skew = matrix_of(1.0, 0.0, d.skew_xy, 1.0, 0.0, 0.0);
    let scale = matrix_of(d.scale_x, 0.0, 0.0, d.scale_y, 0.0, 0.0);
    Matrix::translate(d.tx, d.ty)
        .concat(&rotation)
        .concat(&skew)
        .concat(&scale)
}

/// `none | <transform-list>`.
pub fn parse_transform_list(input: &mut Parser<'_, '_>) -> Result<Rc<[TransformFn]>, ()> {
    let state = input.state();
    if let Ok(name) = input.expect_ident()
        && name.eq_ignore_ascii_case("none")
    {
        return Ok(Rc::from(Vec::new()));
    }
    input.reset(&state);
    let mut functions: Vec<TransformFn> = Vec::new();
    loop {
        let state = input.state();
        let name = match input.expect_function() {
            Ok(name) => name.as_ref().to_ascii_lowercase(),
            Err(_) => {
                input.reset(&state);
                break;
            }
        };
        let function = input
            .parse_nested_block(|inner| match parse_transform_function(&name, inner) {
                Ok(function) => Ok(function),
                Err(()) => Err(inner.new_custom_error::<(), ()>(())),
            })
            .map_err(|_: cssparser::ParseError<'_, ()>| ())?;
        functions.push(function);
    }
    if functions.is_empty() {
        return Err(());
    }
    Ok(functions.into())
}

fn numbers<const N: usize>(input: &mut Parser<'_, '_>) -> Result<[f32; N], ()> {
    let mut out = [0.0_f32; N];
    for (index, slot) in out.iter_mut().enumerate() {
        if index > 0 {
            input.expect_comma().map_err(|_| ())?;
        }
        let value = input.expect_number().map_err(|_| ())?;
        if !value.is_finite() {
            return Err(());
        }
        *slot = value;
    }
    Ok(out)
}

fn parse_transform_function(name: &str, input: &mut Parser<'_, '_>) -> Result<TransformFn, ()> {
    let function = match name {
        "matrix" => TransformFn::Matrix(numbers::<6>(input)?),
        "matrix3d" => TransformFn::Matrix3d(numbers::<16>(input)?),
        "translate" => {
            let x = Length::parse(input)?;
            let y = if input.expect_comma().is_ok() {
                Length::parse(input)?
            } else {
                Length::zero()
            };
            TransformFn::Translate(x, y)
        }
        "translatex" => TransformFn::TranslateX(Length::parse(input)?),
        "translatey" => TransformFn::TranslateY(Length::parse(input)?),
        "translatez" => TransformFn::TranslateZ(Length::parse(input)?),
        "translate3d" => {
            let x = Length::parse(input)?;
            input.expect_comma().map_err(|_| ())?;
            let y = Length::parse(input)?;
            input.expect_comma().map_err(|_| ())?;
            let z = Length::parse(input)?;
            TransformFn::Translate3d(x, y, z)
        }
        "scale" => {
            let x = input.expect_number().map_err(|_| ())?;
            let y = if input.expect_comma().is_ok() {
                input.expect_number().map_err(|_| ())?
            } else {
                x
            };
            TransformFn::Scale(x, y)
        }
        "scalex" => TransformFn::ScaleX(input.expect_number().map_err(|_| ())?),
        "scaley" => TransformFn::ScaleY(input.expect_number().map_err(|_| ())?),
        "scalez" => TransformFn::ScaleZ(input.expect_number().map_err(|_| ())?),
        "scale3d" => {
            let [x, y, z] = numbers::<3>(input)?;
            TransformFn::Scale3d(x, y, z)
        }
        "rotate" => TransformFn::Rotate(parse_angle(input)?),
        "rotatex" => TransformFn::RotateX(parse_angle(input)?),
        "rotatey" => TransformFn::RotateY(parse_angle(input)?),
        "rotatez" => TransformFn::RotateZ(parse_angle(input)?),
        "rotate3d" => {
            let [x, y, z] = numbers::<3>(input)?;
            input.expect_comma().map_err(|_| ())?;
            TransformFn::Rotate3d(x, y, z, parse_angle(input)?)
        }
        "skew" => {
            let ax = parse_angle(input)?;
            let ay = if input.expect_comma().is_ok() {
                parse_angle(input)?
            } else {
                0.0
            };
            TransformFn::Skew(ax, ay)
        }
        "skewx" => TransformFn::SkewX(parse_angle(input)?),
        "skewy" => TransformFn::SkewY(parse_angle(input)?),
        "perspective" => TransformFn::Perspective(Length::parse(input)?),
        _ => return Err(()),
    };
    input.skip_whitespace();
    if input.is_exhausted() {
        Ok(function)
    } else {
        Err(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        TransformFn, decompose_2d, parse_transform_list, recompose_2d, transform_list_matrix,
    };
    use crate::css::value::length::{Length, LengthCtx};
    use crate::css::value::parse_entirely_with;

    fn list(text: &str) -> Option<Vec<TransformFn>> {
        parse_entirely_with(text, parse_transform_list)
            .ok()
            .map(|functions| functions.to_vec())
    }

    #[test]
    fn none_is_an_empty_list_and_every_function_parses() {
        assert_eq!(list("none"), Some(Vec::new()));
        assert_eq!(
            list("rotate(1turn)"),
            Some(vec![TransformFn::Rotate(360.0)])
        );
        assert_eq!(
            list("translate(1px, 2px) scale(2)"),
            Some(vec![
                TransformFn::Translate(Length::px(1.0), Length::px(2.0)),
                TransformFn::Scale(2.0, 2.0),
            ])
        );
        assert_eq!(list("skewX(10deg)"), Some(vec![TransformFn::SkewX(10.0)]));
        assert!(list("matrix(1, 0, 0, 1, 5, 6)").is_some());
        assert!(list("matrix3d(1,0,0,0, 0,1,0,0, 0,0,1,0, 0,0,0,1)").is_some());
        assert!(list("perspective(100px)").is_some());
        assert!(list("rotate3d(0, 0, 1, 90deg)").is_some());
        assert!(list("rotate()").is_none());
    }

    #[test]
    fn a_transform_list_flattens_around_its_origin() {
        let ctx = LengthCtx::default();
        let functions = parse_transform_list_of("translate(10px, 0)");
        let matrix = transform_list_matrix(&functions, &ctx, (100.0, 50.0), (0.0, 0.0));
        let point = matrix.map_point(skia_rs_safe::core::Point { x: 0.0, y: 0.0 });
        assert_eq!((point.x, point.y), (10.0, 0.0));

        // Scaling about a (50, 25) origin leaves that point fixed.
        let functions = parse_transform_list_of("scale(2)");
        let matrix = transform_list_matrix(&functions, &ctx, (100.0, 50.0), (50.0, 25.0));
        let point = matrix.map_point(skia_rs_safe::core::Point { x: 50.0, y: 25.0 });
        assert!((point.x - 50.0).abs() < 1e-4 && (point.y - 25.0).abs() < 1e-4);
    }

    fn parse_transform_list_of(text: &str) -> Vec<TransformFn> {
        parse_entirely_with(text, parse_transform_list)
            .expect("transform list parses")
            .to_vec()
    }

    #[test]
    fn percentage_translations_use_the_basis() {
        let ctx = LengthCtx::default();
        let functions = parse_transform_list_of("translate(50%, 100%)");
        let matrix = transform_list_matrix(&functions, &ctx, (100.0, 50.0), (0.0, 0.0));
        let point = matrix.map_point(skia_rs_safe::core::Point { x: 0.0, y: 0.0 });
        assert!((point.x - 50.0).abs() < 1e-4, "{point:?}");
        assert!((point.y - 50.0).abs() < 1e-4, "{point:?}");
    }

    #[test]
    fn decomposition_round_trips_a_2d_matrix() {
        // skia-rs has no matrix decomposition; this is project code, and
        // interpolating two structurally-mismatched lists depends on it.
        let ctx = LengthCtx::default();
        let functions = parse_transform_list_of("translate(5px, 7px) rotate(30deg) scale(2, 3)");
        let matrix = transform_list_matrix(&functions, &ctx, (10.0, 10.0), (0.0, 0.0));
        let decomposed = decompose_2d(&matrix).expect("an affine matrix decomposes");
        assert!((decomposed.tx - 5.0).abs() < 1e-3, "{decomposed:?}");
        assert!((decomposed.ty - 7.0).abs() < 1e-3, "{decomposed:?}");
        assert!((decomposed.rotate - 30.0).abs() < 1e-3, "{decomposed:?}");
        assert!((decomposed.scale_x - 2.0).abs() < 1e-3, "{decomposed:?}");
        assert!((decomposed.scale_y - 3.0).abs() < 1e-3, "{decomposed:?}");
        let rebuilt = recompose_2d(&decomposed);
        for index in 0..6 {
            assert!(
                (rebuilt.values[index] - matrix.values[index]).abs() < 1e-3,
                "component {index}: {rebuilt:?} vs {matrix:?}"
            );
        }
    }

    #[test]
    fn a_degenerate_matrix_does_not_decompose() {
        let flat = skia_rs_safe::core::Matrix {
            values: [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0],
        };
        assert_eq!(decompose_2d(&flat), None);
    }

    #[test]
    fn transform_parsing_never_panics() {
        for input in crate::css::value::FUZZ_INPUTS {
            if let Ok(functions) = parse_entirely_with(input, parse_transform_list) {
                let _ = transform_list_matrix(
                    &functions,
                    &LengthCtx::default(),
                    (1.0, 1.0),
                    (0.0, 0.0),
                );
            }
        }
    }
}
