//! A shared recursion-depth guard for every hand-written recursive-descent
//! parser in `css::tokens`, `css::value::calc` and `css::value::image`.
//!
//! `cssparser::Parser::parse_nested_block` recurses through Rust's call
//! stack with no depth limit of its own: a value like `((((((...1px...))))))`
//! or `cross-fade(cross-fade(cross-fade(...)))` nested a few thousand levels
//! deep will overflow the stack and abort the process rather than fail a
//! parse. [`DepthGuard::enter`] turns that abort into an ordinary `Err` by
//! counting live recursive entries in a thread-local and refusing to nest
//! past [`MAX_DEPTH`].
//!
//! The counter is process-wide per thread and shared across all three
//! modules on purpose: a `calc()` nested inside a `linear-gradient()` nested
//! inside another `calc()` still counts against the same budget, because
//! that is the actual call-stack depth that matters.

use std::cell::Cell;

thread_local! {
    static DEPTH: Cell<u32> = const { Cell::new(0) };
}

/// Recursive-descent entries deeper than this fail the parse instead of
/// growing the stack further. Ordinary CSS -- including anything a human
/// could plausibly author or a real GTK theme ships -- nests at most a
/// handful of levels; 256 is generous headroom above that with a wide
/// margin below where a debug-build stack actually overflows.
const MAX_DEPTH: u32 = 256;

/// An open recursive-parse entry. Dropping it returns the budget.
pub(crate) struct DepthGuard(());

impl DepthGuard {
    /// Enter one more level of recursion, or `None` past [`MAX_DEPTH`].
    ///
    /// Callers at every recursive entry point (a parenthesized group, a
    /// nested function call, a nested `<image>`) must hold the guard for
    /// exactly the duration of that one recursive call and propagate `None`
    /// as an ordinary parse failure -- never unwrap it.
    #[must_use]
    pub(crate) fn enter() -> Option<DepthGuard> {
        DEPTH.with(|depth| {
            let current = depth.get();
            if current >= MAX_DEPTH {
                None
            } else {
                depth.set(current + 1);
                Some(DepthGuard(()))
            }
        })
    }
}

impl Drop for DepthGuard {
    fn drop(&mut self) {
        DEPTH.with(|depth| depth.set(depth.get().saturating_sub(1)));
    }
}

#[cfg(test)]
mod tests {
    use super::{DepthGuard, MAX_DEPTH};

    #[test]
    fn nesting_up_to_the_limit_succeeds_and_unwinds_cleanly() {
        fn recurse(remaining: u32) -> u32 {
            let Some(_guard) = DepthGuard::enter() else {
                return remaining;
            };
            if remaining == 0 {
                0
            } else {
                recurse(remaining - 1)
            }
        }
        // Exactly MAX_DEPTH entries must all succeed (each `enter()` call is
        // itself one level), and the guard must fully release afterwards so
        // a later, unrelated parse is not left permanently refused.
        assert_eq!(recurse(MAX_DEPTH - 1), 0);
        let Some(_guard) = DepthGuard::enter() else {
            panic!("the counter must have unwound back to zero");
        };
    }

    #[test]
    fn nesting_past_the_limit_refuses_rather_than_recursing_further() {
        fn recurse(depth: u32) -> u32 {
            match DepthGuard::enter() {
                Some(_guard) => recurse(depth + 1),
                None => depth,
            }
        }
        let reached = recurse(0);
        assert_eq!(reached, MAX_DEPTH);
    }
}
