//! `calc()`, `min()`, `max()` and `clamp()`.
//!
//! `cssparser` 0.37 has no calc AST -- a math function arrives as an
//! ordinary `Token::Function` -- so the expression tree, the parser and the
//! evaluator here are entirely project code.

use std::rc::Rc;

use cssparser::{ParseError, Parser, Token};

use super::length::{Length, LengthCtx, LengthUnit};
use super::timing::Time;
use crate::css::depth_guard::DepthGuard;

/// A parsed math expression. Nothing is resolved: units, percentages and
/// relative-colour channel variables all survive until a context exists.
#[derive(Clone, Debug, PartialEq)]
pub enum CalcNode {
    /// A bare `<number>`.
    Number(f32),
    /// A `<length>` (which may itself be a percentage or nested calc).
    Length(Length),
    /// A `<percentage>`, stored as a fraction (`50%` == `0.5`).
    Percent(f32),
    /// An `<angle>` in degrees.
    Angle(f32),
    /// A `<time>`.
    Time(Time),
    /// `a + b`.
    Sum(Rc<CalcNode>, Rc<CalcNode>),
    /// `a - b`.
    Difference(Rc<CalcNode>, Rc<CalcNode>),
    /// `a * k` (the bare number is always the right operand after parsing).
    Product(Rc<CalcNode>, f32),
    /// `a / k`.
    Quotient(Rc<CalcNode>, f32),
    /// `min(a, b, ...)`.
    Min(Rc<[CalcNode]>),
    /// `max(a, b, ...)`.
    Max(Rc<[CalcNode]>),
    /// `clamp(lo, value, hi)`.
    Clamp(Rc<CalcNode>, Rc<CalcNode>, Rc<CalcNode>),
    /// A relative-colour channel variable: `0..=2` are the origin colour's
    /// three channels in the function's own space, `3` is its alpha.
    ///
    /// Contract deviation 4: the contract's `ChannelExpr::Calc` cannot
    /// otherwise express `calc(s * 1.2)`, which 8 of Adwaita's 37
    /// `@define-color`s need.
    Var(u8),
}

/// What an expression evaluates to. Kinds never mix.
#[derive(Copy, Clone, Debug, PartialEq)]
enum CalcValue {
    Number(f32),
    Px(f32),
    Angle(f32),
    Seconds(f32),
}

impl CalcNode {
    /// Parse a math expression -- the *contents* of a `calc()` and the body
    /// of each `min()`/`max()`/`clamp()` argument.
    pub fn parse(input: &mut Parser<'_, '_>) -> Result<CalcNode, ()> {
        parse_sum(input)
    }

    /// Resolve to device pixels. `None` == invalid at computed-value time.
    #[must_use]
    pub fn resolve_length(&self, ctx: &LengthCtx) -> Option<f32> {
        match self.eval(Some(ctx), None)? {
            CalcValue::Px(px) => finite_opt(px),
            _ => None,
        }
    }

    /// Resolve to a bare number.
    #[must_use]
    pub fn resolve_number(&self) -> Option<f32> {
        match self.eval(None, None)? {
            CalcValue::Number(n) => finite_opt(n),
            _ => None,
        }
    }

    /// Resolve to a bare number with relative-colour channels bound:
    /// `[c0, c1, c2, alpha]` in the colour function's own space.
    #[must_use]
    pub fn resolve_number_with(&self, vars: &[f32; 4]) -> Option<f32> {
        match self.eval(None, Some(vars))? {
            CalcValue::Number(n) => finite_opt(n),
            _ => None,
        }
    }

    /// Resolve to degrees.
    #[must_use]
    pub fn resolve_angle(&self) -> Option<f32> {
        match self.eval(None, None)? {
            CalcValue::Angle(deg) => finite_opt(deg),
            _ => None,
        }
    }

    /// Resolve to a time.
    #[must_use]
    pub fn resolve_time(&self) -> Option<Time> {
        match self.eval(None, None)? {
            CalcValue::Seconds(s) => finite_opt(s).map(Time),
            _ => None,
        }
    }

    fn eval(&self, ctx: Option<&LengthCtx>, vars: Option<&[f32; 4]>) -> Option<CalcValue> {
        match self {
            CalcNode::Number(n) => Some(CalcValue::Number(*n)),
            // With a length context in hand a percentage *must* find a basis
            // (contract §`LengthCtx::percent_basis`: `None` => percentages are
            // invalid here). Only the context-free resolvers -- number, angle,
            // time -- read a percentage as its bare fraction.
            CalcNode::Percent(f) => match ctx {
                Some(ctx) => ctx.percent_basis.map(|basis| CalcValue::Px(f * basis)),
                None => Some(CalcValue::Number(*f)),
            },
            CalcNode::Angle(deg) => Some(CalcValue::Angle(*deg)),
            CalcNode::Time(t) => Some(CalcValue::Seconds(t.0)),
            CalcNode::Length(length) => length.resolve(ctx?).map(CalcValue::Px),
            CalcNode::Var(index) => vars
                .and_then(|v| v.get(usize::from(*index)).copied())
                .map(CalcValue::Number),
            CalcNode::Sum(a, b) => combine(a.eval(ctx, vars)?, b.eval(ctx, vars)?, |x, y| x + y),
            CalcNode::Difference(a, b) => {
                combine(a.eval(ctx, vars)?, b.eval(ctx, vars)?, |x, y| x - y)
            }
            CalcNode::Product(a, k) => Some(scale(a.eval(ctx, vars)?, *k)),
            CalcNode::Quotient(a, k) => {
                if *k == 0.0 || !k.is_finite() {
                    return None;
                }
                Some(scale(a.eval(ctx, vars)?, 1.0 / *k))
            }
            CalcNode::Min(list) => fold(list, ctx, vars, f32::min),
            CalcNode::Max(list) => fold(list, ctx, vars, f32::max),
            CalcNode::Clamp(lo, value, hi) => {
                let lo = lo.eval(ctx, vars)?;
                let value = value.eval(ctx, vars)?;
                let hi = hi.eval(ctx, vars)?;
                let clamped = combine(value, lo, f32::max)?;
                combine(clamped, hi, f32::min)
            }
        }
    }
}

fn finite_opt(value: f32) -> Option<f32> {
    value.is_finite().then_some(value)
}

fn scalar(value: CalcValue) -> f32 {
    match value {
        CalcValue::Number(n) | CalcValue::Px(n) | CalcValue::Angle(n) | CalcValue::Seconds(n) => n,
    }
}

fn rewrap(kind: CalcValue, scalar: f32) -> CalcValue {
    match kind {
        CalcValue::Number(_) => CalcValue::Number(scalar),
        CalcValue::Px(_) => CalcValue::Px(scalar),
        CalcValue::Angle(_) => CalcValue::Angle(scalar),
        CalcValue::Seconds(_) => CalcValue::Seconds(scalar),
    }
}

/// Combine two values of the *same* kind. A number combines with anything
/// (CSS's `<number>` is unit-neutral only for `*`/`/`, but a `calc(2 + 3)`
/// inside a length context must still work), and any other mismatch fails.
fn combine(a: CalcValue, b: CalcValue, op: fn(f32, f32) -> f32) -> Option<CalcValue> {
    let kind = match (a, b) {
        (CalcValue::Number(_), other) => other,
        (other, CalcValue::Number(_)) => other,
        (x, y) if std::mem::discriminant(&x) == std::mem::discriminant(&y) => x,
        _ => return None,
    };
    Some(rewrap(kind, op(scalar(a), scalar(b))))
}

fn scale(value: CalcValue, k: f32) -> CalcValue {
    rewrap(value, scalar(value) * k)
}

fn fold(
    list: &[CalcNode],
    ctx: Option<&LengthCtx>,
    vars: Option<&[f32; 4]>,
    op: fn(f32, f32) -> f32,
) -> Option<CalcValue> {
    let mut iter = list.iter();
    let mut acc = iter.next()?.eval(ctx, vars)?;
    for node in iter {
        acc = combine(acc, node.eval(ctx, vars)?, op)?;
    }
    Some(acc)
}

/// Parse a math function whose `Token::Function` name has already been
/// consumed. Returns `Err(())` for anything that is not `calc`, `min`,
/// `max` or `clamp`.
///
/// This is the single choke point every `calc()`/`min()`/`max()`/`clamp()`
/// body passes through, whether reached through [`parse_unary`]'s own
/// `Function` arm or directly from another value type's parser (`Length`,
/// `Time`, colour channels, ...); guarding it here bounds recursion for both
/// without needing a guard at every call site.
pub fn parse_math_function(name: &str, input: &mut Parser<'_, '_>) -> Result<CalcNode, ()> {
    let _guard = DepthGuard::enter().ok_or(())?;
    if name.eq_ignore_ascii_case("calc") {
        input.parse_nested_block(parse_sum_entirely).map_err(|_| ())
    } else if name.eq_ignore_ascii_case("min") || name.eq_ignore_ascii_case("max") {
        let is_min = name.eq_ignore_ascii_case("min");
        let args = input
            .parse_nested_block(parse_argument_list)
            .map_err(|_| ())?;
        if args.is_empty() {
            return Err(());
        }
        let args: Rc<[CalcNode]> = args.into();
        Ok(if is_min {
            CalcNode::Min(args)
        } else {
            CalcNode::Max(args)
        })
    } else if name.eq_ignore_ascii_case("clamp") {
        let args = input
            .parse_nested_block(parse_argument_list)
            .map_err(|_| ())?;
        if args.len() != 3 {
            return Err(());
        }
        let mut iter = args.into_iter();
        let lo = iter.next().ok_or(())?;
        let value = iter.next().ok_or(())?;
        let hi = iter.next().ok_or(())?;
        Ok(CalcNode::Clamp(Rc::new(lo), Rc::new(value), Rc::new(hi)))
    } else {
        Err(())
    }
}

fn parse_sum_entirely<'i>(inner: &mut Parser<'i, '_>) -> Result<CalcNode, ParseError<'i, ()>> {
    let node = parse_sum(inner).map_err(|()| inner.new_custom_error::<(), ()>(()))?;
    inner.skip_whitespace();
    if inner.is_exhausted() {
        Ok(node)
    } else {
        Err(inner.new_custom_error::<(), ()>(()))
    }
}

fn parse_argument_list<'i>(
    inner: &mut Parser<'i, '_>,
) -> Result<Vec<CalcNode>, ParseError<'i, ()>> {
    inner.parse_comma_separated(|arg| {
        let node = parse_sum(arg).map_err(|()| arg.new_custom_error::<(), ()>(()))?;
        Ok(node)
    })
}

fn parse_sum(input: &mut Parser<'_, '_>) -> Result<CalcNode, ()> {
    let mut left = parse_product(input)?;
    loop {
        let state = input.state();
        let token = match input.next() {
            Ok(token) => token.clone(),
            Err(_) => {
                input.reset(&state);
                return Ok(left);
            }
        };
        match token {
            Token::Delim('+') => {
                let right = parse_product(input)?;
                left = CalcNode::Sum(Rc::new(left), Rc::new(right));
            }
            Token::Delim('-') => {
                let right = parse_product(input)?;
                left = CalcNode::Difference(Rc::new(left), Rc::new(right));
            }
            other => {
                // `calc(10px -2px)` tokenizes the second operand as a signed
                // dimension, with no delim token in between: a signed operand
                // straight after an operand is an implicit addition.
                let signed = matches!(
                    other,
                    Token::Number { has_sign: true, .. }
                        | Token::Percentage { has_sign: true, .. }
                        | Token::Dimension { has_sign: true, .. }
                );
                input.reset(&state);
                if !signed {
                    return Ok(left);
                }
                let right = parse_product(input)?;
                left = CalcNode::Sum(Rc::new(left), Rc::new(right));
            }
        }
    }
}

fn parse_product(input: &mut Parser<'_, '_>) -> Result<CalcNode, ()> {
    let mut left = parse_unary(input)?;
    loop {
        let state = input.state();
        let token = match input.next() {
            Ok(token) => token.clone(),
            Err(_) => {
                input.reset(&state);
                return Ok(left);
            }
        };
        match token {
            Token::Delim('*') => {
                let right = parse_unary(input)?;
                left = match (right.resolve_number(), left.resolve_number()) {
                    (Some(k), _) => CalcNode::Product(Rc::new(left), k),
                    (None, Some(k)) => CalcNode::Product(Rc::new(right), k),
                    (None, None) => return Err(()),
                };
            }
            Token::Delim('/') => {
                let right = parse_unary(input)?;
                let k = right.resolve_number().ok_or(())?;
                if k == 0.0 || !k.is_finite() {
                    return Err(());
                }
                left = CalcNode::Quotient(Rc::new(left), k);
            }
            _ => {
                input.reset(&state);
                return Ok(left);
            }
        }
    }
}

fn parse_unary(input: &mut Parser<'_, '_>) -> Result<CalcNode, ()> {
    let token = input.next().map_err(|_| ())?.clone();
    match token {
        Token::Number { value, .. } => {
            if value.is_finite() {
                Ok(CalcNode::Number(value))
            } else {
                Err(())
            }
        }
        Token::Percentage { unit_value, .. } => {
            if unit_value.is_finite() {
                Ok(CalcNode::Percent(unit_value))
            } else {
                Err(())
            }
        }
        Token::Dimension {
            value, ref unit, ..
        } => {
            if !value.is_finite() {
                return Err(());
            }
            if let Some(length_unit) = LengthUnit::from_str_ascii_ci(unit) {
                Ok(CalcNode::Length(Length::Abs {
                    value,
                    unit: length_unit,
                }))
            } else if let Some(degrees) = angle_to_degrees(value, unit) {
                Ok(CalcNode::Angle(degrees))
            } else if unit.eq_ignore_ascii_case("s") {
                Ok(CalcNode::Time(Time(value)))
            } else if unit.eq_ignore_ascii_case("ms") {
                Ok(CalcNode::Time(Time(value / 1000.0)))
            } else {
                Err(())
            }
        }
        Token::Ident(ref name) => channel_var(name).map(CalcNode::Var).ok_or(()),
        Token::ParenthesisBlock => {
            // A bare grouping `(...)`, not a function call, so it does not
            // pass through `parse_math_function`'s guard -- it needs its own.
            let _guard = DepthGuard::enter().ok_or(())?;
            input.parse_nested_block(parse_sum_entirely).map_err(|_| ())
        }
        Token::Function(ref name) => {
            let name = name.clone();
            parse_math_function(&name, input)
        }
        _ => Err(()),
    }
}

/// The only bare idents a math expression accepts are the relative-colour
/// channel names of the two spaces GTK themes actually use, `srgb` and
/// `hsl` (`rgb(from ...)` / `hsl(from ...)`). Index `3` is always alpha.
fn channel_var(name: &str) -> Option<u8> {
    if name.eq_ignore_ascii_case("r") || name.eq_ignore_ascii_case("h") {
        Some(0)
    } else if name.eq_ignore_ascii_case("g") || name.eq_ignore_ascii_case("s") {
        Some(1)
    } else if name.eq_ignore_ascii_case("b") || name.eq_ignore_ascii_case("l") {
        Some(2)
    } else if name.eq_ignore_ascii_case("alpha") {
        Some(3)
    } else {
        None
    }
}

fn angle_to_degrees(value: f32, unit: &str) -> Option<f32> {
    if unit.eq_ignore_ascii_case("deg") {
        Some(value)
    } else if unit.eq_ignore_ascii_case("grad") {
        Some(value * 360.0 / 400.0)
    } else if unit.eq_ignore_ascii_case("rad") {
        Some(value.to_degrees())
    } else if unit.eq_ignore_ascii_case("turn") {
        Some(value * 360.0)
    } else {
        None
    }
}

/// Parse an `<angle>` -- any CSS angle unit, a unitless `0`, or a math
/// function that resolves to one. Always returns degrees.
pub fn parse_angle(input: &mut Parser<'_, '_>) -> Result<f32, ()> {
    let token = input.next().map_err(|_| ())?.clone();
    match token {
        Token::Dimension {
            value, ref unit, ..
        } => {
            if !value.is_finite() {
                return Err(());
            }
            angle_to_degrees(value, unit).ok_or(())
        }
        Token::Number { value, .. } => {
            // A unitless zero is the one bare number that is an angle.
            if value == 0.0 { Ok(0.0) } else { Err(()) }
        }
        Token::Function(ref name) => {
            let name = name.clone();
            parse_math_function(&name, input)?.resolve_angle().ok_or(())
        }
        _ => Err(()),
    }
}

#[cfg(test)]
mod tests {
    use super::{CalcNode, parse_angle};
    use crate::css::value::length::{Length, LengthCtx};
    use crate::css::value::parse_entirely_with;
    use crate::css::value::timing::Time;

    fn calc(text: &str) -> Option<Length> {
        parse_entirely_with(text, Length::parse).ok()
    }

    fn calc_px(text: &str, ctx: &LengthCtx) -> Option<f32> {
        calc(text)?.resolve(ctx)
    }

    #[test]
    fn sums_and_differences_of_compatible_units_resolve() {
        let ctx = LengthCtx {
            font_size_px: 10.0,
            ..LengthCtx::default()
        };
        assert_eq!(calc_px("calc(1px + 2px)", &ctx), Some(3.0));
        assert_eq!(calc_px("calc(10px - 4px)", &ctx), Some(6.0));
        assert_eq!(calc_px("calc(1em + 5px)", &ctx), Some(15.0));
        // A signed operand with no operator between it and the previous one
        // is an implicit addition -- `1px -2px` tokenizes that way.
        assert_eq!(calc_px("calc(10px -2px)", &ctx), Some(8.0));
    }

    #[test]
    fn products_and_quotients_require_a_bare_number_on_one_side() {
        let ctx = LengthCtx::default();
        assert_eq!(calc_px("calc(3px * 4)", &ctx), Some(12.0));
        assert_eq!(calc_px("calc(4 * 3px)", &ctx), Some(12.0));
        assert_eq!(calc_px("calc(12px / 4)", &ctx), Some(3.0));
        assert_eq!(calc("calc(3px * 4px)"), None);
        assert_eq!(calc("calc(12px / 4px)"), None);
        assert_eq!(calc("calc(12px / 0)"), None);
    }

    #[test]
    fn nested_parentheses_and_math_functions_compose() {
        let ctx = LengthCtx::default();
        assert_eq!(calc_px("calc((1px + 2px) * 3)", &ctx), Some(9.0));
        assert_eq!(calc_px("min(4px, 9px, 2px)", &ctx), Some(2.0));
        assert_eq!(calc_px("max(4px, 9px, 2px)", &ctx), Some(9.0));
        assert_eq!(calc_px("clamp(2px, 9px, 5px)", &ctx), Some(5.0));
        assert_eq!(calc_px("clamp(2px, 1px, 5px)", &ctx), Some(2.0));
        assert_eq!(calc_px("calc(min(4px, 9px) + 1px)", &ctx), Some(5.0));
    }

    #[test]
    fn mixed_kinds_are_invalid_at_computed_value_time() {
        // Parses (it is a well-formed expression), never resolves.
        let ctx = LengthCtx::default();
        assert!(calc("calc(1px + 1s)").is_some());
        assert_eq!(calc_px("calc(1px + 1s)", &ctx), None);
    }

    #[test]
    fn percentages_inside_calc_use_the_context_basis() {
        let ctx = LengthCtx {
            percent_basis: Some(200.0),
            ..LengthCtx::default()
        };
        assert_eq!(calc_px("calc(50% + 10px)", &ctx), Some(110.0));
        assert_eq!(calc_px("calc(50% + 10px)", &LengthCtx::default()), None);
    }

    #[test]
    fn numbers_angles_and_times_have_their_own_resolvers() {
        let node = parse_entirely_with("calc(2 * 3)", |input| {
            let name = input.expect_function().map_err(|_| ())?.clone();
            super::parse_math_function(&name, input)
        })
        .expect("calc(2 * 3) parses");
        assert_eq!(node.resolve_number(), Some(6.0));
        assert_eq!(node.resolve_length(&LengthCtx::default()), None);

        let angle = parse_entirely_with("calc(0.5turn)", |input| {
            let name = input.expect_function().map_err(|_| ())?.clone();
            super::parse_math_function(&name, input)
        })
        .expect("calc(0.5turn) parses");
        assert_eq!(angle.resolve_angle(), Some(180.0));

        let time = parse_entirely_with("calc(200ms * 2)", |input| {
            let name = input.expect_function().map_err(|_| ())?.clone();
            super::parse_math_function(&name, input)
        })
        .expect("calc(200ms * 2) parses");
        assert_eq!(time.resolve_time(), Some(Time(0.4)));
    }

    #[test]
    fn channel_variables_resolve_only_when_bound() {
        // `hsl(from #f6f5f4 h calc(s * 1.2) calc(l * 1.2))` -- 8 of Adwaita's
        // 37 @define-colors need this.
        let node = parse_entirely_with("calc(s * 1.5)", |input| {
            let name = input.expect_function().map_err(|_| ())?.clone();
            super::parse_math_function(&name, input)
        })
        .expect("calc(s * 1.5) parses");
        assert_eq!(node.resolve_number(), None);
        assert_eq!(node.resolve_number_with(&[0.0, 0.4, 0.0, 1.0]), Some(0.6));

        let alpha = parse_entirely_with("calc(alpha * 0.35)", |input| {
            let name = input.expect_function().map_err(|_| ())?.clone();
            super::parse_math_function(&name, input)
        })
        .expect("calc(alpha * 0.35) parses");
        assert_eq!(alpha.resolve_number_with(&[0.0, 0.0, 0.0, 1.0]), Some(0.35));
    }

    #[test]
    fn angles_accept_every_css_unit() {
        assert_eq!(parse_entirely_with("90deg", parse_angle).ok(), Some(90.0));
        assert_eq!(
            parse_entirely_with("0.25turn", parse_angle).ok(),
            Some(90.0)
        );
        assert_eq!(parse_entirely_with("100grad", parse_angle).ok(), Some(90.0));
        assert_eq!(parse_entirely_with("0", parse_angle).ok(), Some(0.0));
        let radians = parse_entirely_with("1rad", parse_angle).expect("1rad parses");
        assert!((radians - 57.295_78).abs() < 1e-3, "{radians}");
        assert_eq!(parse_entirely_with("1px", parse_angle).ok(), None);
    }

    #[test]
    fn calc_parsing_never_panics() {
        for input in crate::css::value::FUZZ_INPUTS {
            let _ = parse_entirely_with(input, |p| {
                let name = p.expect_function().map_err(|_| ())?.clone();
                super::parse_math_function(&name, p)
            });
            let _ = parse_entirely_with(input, CalcNode::parse);
            let _ = parse_entirely_with(input, parse_angle);
        }
    }

    #[test]
    fn deeply_nested_calc_returns_err_instead_of_overflowing_the_stack() {
        // Mutation check: remove either `DepthGuard::enter()` call added to
        // `parse_math_function`/`parse_unary`'s `ParenthesisBlock` arm and
        // this input overflows the stack (an abort, not something a
        // `#[test]` can observe as a failure) instead of returning `Err`.
        let mut nested = "calc(".repeat(2000);
        nested.push_str("1px");
        nested.push_str(&")".repeat(2000));
        assert_eq!(calc(&nested), None);

        let mut parens = "(".repeat(2000);
        parens.push_str("1px");
        parens.push_str(&")".repeat(2000));
        let wrapped = format!("calc{parens}");
        assert_eq!(calc(&wrapped), None);
    }
}
