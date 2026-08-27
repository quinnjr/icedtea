//! Typed CSS values.
//!
//! Every parser here takes a `cssparser::Parser`, is ASCII-case- and
//! whitespace-insensitive, resolves nothing (`@name`, `currentColor`,
//! `em`/`rem`/`%` and `calc()` all survive unresolved into the value), and
//! never panics on any token stream. `Err(())` means *invalid at parse
//! time*: the declaration is dropped, CSS-style.

// Every value parser in this module tree returns `Result<T, ()>`: the
// contract freezes that signature, and `Err(())` carries all the meaning
// CSS gives it -- *invalid at parse time*, drop the declaration.
#![allow(clippy::result_unit_err)]

pub mod border;
pub mod calc;
pub mod color;
pub mod image;
pub mod keyword;
pub mod length;
pub mod shadow;
pub mod timing;

pub use border::{BgSize, BorderImageSlice, BorderImageWidthSide, NumberOrPercent, RepeatStyle};
pub use calc::CalcNode;
pub use color::{ColorCtx, ColorSpace, ColorTable, ColorValue, Rgba};
pub use image::{Gradient, IconRef, Image, Position};
pub use keyword::{Keyword, Wide};
pub use length::{Length, LengthCtx, LengthUnit};
pub use shadow::Shadow;
pub use timing::Time;

/// Odd inputs every value family's never-panic battery runs.
///
/// One corpus, shared, so a new hostile input added for one family
/// immediately covers all of them.
#[cfg(test)]
pub(crate) const FUZZ_INPUTS: &[&str] = &[
    "",
    " ",
    "\t\n",
    "/**/",
    "0",
    "-",
    "+",
    ".",
    "e",
    "e10",
    "1e999",
    "-1e999",
    "nan",
    "inf",
    "-0",
    "#",
    "#z",
    "#\u{e9}",
    "#\u{1f600}\u{1f600}\u{1f600}",
    "\u{e9}",
    "\u{0}\u{1}\u{2}",
    "@",
    "@\u{e9}",
    "(",
    ")",
    "()",
    "[",
    "{",
    "{}",
    ",",
    ",,,",
    "/",
    "//",
    "!",
    "!important",
    "url(",
    "url()",
    "calc(",
    "calc()",
    "calc(1px",
    "calc(1px +)",
    "calc(1px + )",
    "calc(* 2)",
    "calc(1px * 2px)",
    "calc(1 / 0)",
    "calc(1px + 1s)",
    "min()",
    "max()",
    "clamp(1px)",
    "rgb(",
    "rgb()",
    "rgb(1,2)",
    "rgb(1,2,3,4,5)",
    "rgb(from)",
    "hsl(\u{e9})",
    "color-mix(",
    "color-mix(in)",
    "linear-gradient(",
    "linear-gradient()",
    "linear-gradient(red)",
    "radial-gradient(at)",
    "conic-gradient(from)",
    "cross-fade()",
    "-gtk-icontheme()",
    "-gtk-scaled(a)",
    "image()",
    "steps()",
    "steps(0)",
    "steps(-1, end)",
    "cubic-bezier()",
    "cubic-bezier(1,2,3)",
    "cubic-bezier(2,0,0,1)",
    "matrix()",
    "matrix(1)",
    "translate()",
    "rotate()",
    "blur()",
    "drop-shadow()",
    "inset",
    "1px 2px 3px 4px 5px 6px",
    "1px/",
    "/1px",
    "solid solid solid",
    "\u{202e}",
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
];

/// Parse `text` with `f`, requiring the whole input to be consumed.
///
/// The test-side mirror of the registry's whole-value rule: a trailing
/// token is an error.
///
/// Unused within this task; later value-family parsers (Task 3 onward)
/// call it from their own test modules.
#[cfg(test)]
#[allow(dead_code)]
pub(crate) fn parse_entirely_with<T>(
    text: &str,
    f: impl FnOnce(&mut cssparser::Parser<'_, '_>) -> Result<T, ()>,
) -> Result<T, ()> {
    let mut input = cssparser::ParserInput::new(text);
    let mut parser = cssparser::Parser::new(&mut input);
    let value = f(&mut parser)?;
    parser.skip_whitespace();
    if parser.is_exhausted() {
        Ok(value)
    } else {
        Err(())
    }
}
