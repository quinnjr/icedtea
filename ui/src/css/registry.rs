//! The property registry. Filled in by Tasks 14 and 16.

use crate::css::value::Value;

/// Whole-value parser. Consumes the entire input; a trailing token is an
/// error. Never panics. `Err(())` == invalid at parse time, so the
/// declaration is dropped, CSS-style.
pub type ParseFn = fn(&mut cssparser::Parser<'_, '_>) -> Result<Value, ()>;
