//! Backend selection.
//!
//! Reduced to a single enum for this commit: the smithay winit path and the
//! DRM stub are gone, and the compositor library that replaces them arrives
//! in the next commit. `run()` in `lib.rs` currently has nothing to boot, and
//! says so.

/// Which backend the compositor was asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendChoice {
    /// Nested inside an existing Wayland session.
    Nested,
    /// Whatever the environment offers -- DRM on a TTY, X11 or Wayland under a
    /// session, headless in CI.
    Auto,
}

impl BackendChoice {
    /// `--nested` selects [`BackendChoice::Nested`]; anything else is
    /// [`BackendChoice::Auto`].
    pub fn from_args(args: impl IntoIterator<Item = String>) -> Self {
        if args.into_iter().any(|a| a == "--nested") {
            BackendChoice::Nested
        } else {
            BackendChoice::Auto
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_is_selected_only_by_its_own_flag() {
        assert_eq!(
            BackendChoice::from_args(["icedtea-compositor".into(), "--nested".into()]),
            BackendChoice::Nested
        );
        assert_eq!(BackendChoice::from_args(["icedtea-compositor".into()]), BackendChoice::Auto);
        assert_eq!(
            BackendChoice::from_args(["icedtea-compositor".into(), "--nestedish".into()]),
            BackendChoice::Auto
        );
    }
}
