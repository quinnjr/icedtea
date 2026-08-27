//! Token-level handling of declaration values (M1's `css::value`, relocated).
//!
//! `css::value` is now the *typed* value module; this file keeps the
//! token-stream serializers that turn a declaration's tokens into the
//! normalized string `css::parse` stores. Bodies and tests are unchanged
//! from M1.
//!
//! Every value in this engine is a *serialization of a token stream*, never
//! a slice of the source text. That is what makes `padding: 4px /* x */ 9px`
//! come out as `4px 9px` instead of carrying the comment into the length
//! parser, and it is what lets `border: 1px solid rgb(0 0 0)` be split into
//! three components without shredding the `rgb()` call.

use cssparser::{Parser, ParserInput, ToCss, Token};

use super::depth_guard::DepthGuard;

/// Append a single already-consumed `token` -- recursing into blocks -- to `out`.
///
/// Recursion depth is bounded by [`DepthGuard`]: past the limit a nested
/// block's tokens are consumed (so the outer parser stays synchronised) but
/// not serialized, rather than growing the stack further. Ordinary CSS never
/// gets close to the limit; only a pathological, deliberately deep input
/// does, and for that input an under-serialized value is a correct outcome
/// (the declaration ends up unparseable downstream), not a panic.
fn write_token(token: &Token<'_>, input: &mut Parser<'_, '_>, out: &mut String) {
    match token {
        Token::Function(name) => {
            out.push_str(name);
            out.push('(');
            // A function's block must be consumed through `parse_nested_block`
            // or the outer parser desynchronises.
            match DepthGuard::enter() {
                Some(_guard) => {
                    let _ = input.parse_nested_block(|inner| {
                        write_component_values(inner, out);
                        Ok::<(), cssparser::ParseError<'_, ()>>(())
                    });
                }
                None => {
                    let _ = input
                        .parse_nested_block(|_inner| Ok::<(), cssparser::ParseError<'_, ()>>(()));
                }
            }
            trim_trailing_space(out);
            out.push(')');
        }
        Token::ParenthesisBlock | Token::SquareBracketBlock | Token::CurlyBracketBlock => {
            let (open, close) = match token {
                Token::ParenthesisBlock => ('(', ')'),
                Token::SquareBracketBlock => ('[', ']'),
                _ => ('{', '}'),
            };
            out.push(open);
            match DepthGuard::enter() {
                Some(_guard) => {
                    let _ = input.parse_nested_block(|inner| {
                        write_component_values(inner, out);
                        Ok::<(), cssparser::ParseError<'_, ()>>(())
                    });
                }
                None => {
                    let _ = input
                        .parse_nested_block(|_inner| Ok::<(), cssparser::ParseError<'_, ()>>(()));
                }
            }
            trim_trailing_space(out);
            out.push(close);
        }
        other => {
            let _ = other.to_css(out);
        }
    }
}

fn trim_trailing_space(out: &mut String) {
    while out.ends_with(' ') {
        out.pop();
    }
}

/// Serialize every remaining token in `input`, dropping comments and
/// collapsing runs of whitespace to a single space.
///
/// A comment *is* a separator (`4px/*x*/9px` is two tokens), so it
/// serializes as one space rather than disappearing entirely.
pub fn write_component_values(input: &mut Parser<'_, '_>, out: &mut String) {
    loop {
        let before = input.position();
        write_one_component(input, out);
        if input.position() == before {
            return;
        }
    }
}

/// Serialize exactly one token (recursing into a block) from `input`.
///
/// Split out from [`write_component_values`] so a caller can interleave its
/// own checks -- `!important`, say -- between tokens.
pub fn write_one_component(input: &mut Parser<'_, '_>, out: &mut String) {
    let token = match input.next_including_whitespace_and_comments() {
        Ok(token) => token.clone(),
        Err(_) => return,
    };
    match token {
        Token::WhiteSpace(_) | Token::Comment(_) => {
            if !out.is_empty()
                && !out.ends_with(' ')
                && !out.ends_with('(')
                && !out.ends_with('[')
                && !out.ends_with('{')
            {
                out.push(' ');
            }
        }
        token => write_token(&token, input, out),
    }
}

/// Serialize every remaining token in `input` into a trimmed value string.
#[must_use]
pub fn serialize_remaining(input: &mut Parser<'_, '_>) -> String {
    let mut out = String::new();
    write_component_values(input, &mut out);
    trim_trailing_space(&mut out);
    out
}

/// Split an already-serialized value into its top-level component values.
///
/// One entry per top-level token: a function call and its arguments are a
/// single component (`rgb(0 0 0)`), and a top-level comma is its own
/// component (`","`).
#[must_use]
pub fn component_values(value: &str) -> Vec<String> {
    let mut input = ParserInput::new(value);
    let mut parser = Parser::new(&mut input);
    let mut parts = Vec::new();
    loop {
        let token = match parser.next() {
            Ok(token) => token.clone(),
            Err(_) => return parts,
        };
        let mut part = String::new();
        write_token(&token, &mut parser, &mut part);
        if !part.is_empty() {
            parts.push(part);
        }
    }
}

/// Split an already-serialized value on its top-level commas, returning the
/// component values of each group.
#[must_use]
pub fn comma_groups(value: &str) -> Vec<Vec<String>> {
    let mut groups: Vec<Vec<String>> = vec![Vec::new()];
    for component in component_values(value) {
        if component == "," {
            groups.push(Vec::new());
        } else {
            groups
                .last_mut()
                .expect("groups is never empty")
                .push(component);
        }
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::{comma_groups, component_values};

    #[test]
    fn a_function_call_is_one_component() {
        assert_eq!(
            component_values("1px solid rgb(0 0 0)"),
            vec!["1px", "solid", "rgb(0 0 0)"]
        );
    }

    #[test]
    fn top_level_commas_are_their_own_component() {
        assert_eq!(
            comma_groups("to top, #f6f5f4 2px, #fbfafa"),
            vec![
                vec!["to".to_string(), "top".to_string()],
                vec!["#f6f5f4".to_string(), "2px".to_string()],
                vec!["#fbfafa".to_string()],
            ]
        );
    }

    #[test]
    fn nested_functions_survive_splitting() {
        assert_eq!(
            component_values("calc(1px + calc(2px * 3)) red"),
            vec!["calc(1px + calc(2px * 3))", "red"]
        );
    }

    #[test]
    fn an_empty_value_has_no_components() {
        assert!(component_values("").is_empty());
        assert!(component_values("   ").is_empty());
    }

    #[test]
    fn deeply_nested_functions_never_overflow_the_stack() {
        // Mutation check: remove either `DepthGuard::enter()` call in
        // `write_token` and this input overflows the stack (an abort, not a
        // panic `#[test]` can catch) instead of returning a value.
        let mut nested = String::new();
        for _ in 0..2000 {
            nested.push_str("calc(");
        }
        nested.push_str("1px");
        for _ in 0..2000 {
            nested.push(')');
        }
        nested.push_str(" red");
        // Must return, not overflow. What it returns past the depth limit is
        // secondary to that.
        let parts = component_values(&nested);
        assert!(!parts.is_empty());
    }
}
