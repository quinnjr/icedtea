//! M1's Wayland client, kept at its original path.
//!
//! The implementation moved to [`crate::window::layer`] when M3 grew three
//! surface roles; this module is the compatibility surface contract §8.1
//! promises, so `app.rs`, the `themed-button` binary and the byte-identical
//! gate `ui/tests/layer_shell_screencopy.rs` (which imports [`BTN_LEFT`] from
//! here) keep their M1 paths.

pub use crate::window::layer::{
    AppState, BTN_LEFT, CONFIGURE_TIMEOUT, LayerWindow, LayerWindowError, MARGIN,
};

#[cfg(test)]
mod tests {
    /// The M1 import paths still resolve to the relocated items.
    ///
    /// `ui/tests/layer_shell_screencopy.rs` -- a byte-identical gate file --
    /// imports `icedtea_ui::wayland::BTN_LEFT`, and `app.rs` calls
    /// `LayerWindow::open`/`run` through this module. A rename that broke
    /// either would otherwise only show up in a test binary that needs a live
    /// compositor.
    #[test]
    fn the_m1_import_paths_still_resolve() {
        assert_eq!(super::BTN_LEFT, 0x110);
        assert_eq!(super::MARGIN, 0);
        assert_eq!(super::CONFIGURE_TIMEOUT, std::time::Duration::from_secs(5));
        let error: super::LayerWindowError = super::LayerWindowError::Closed;
        assert!(error.to_string().contains("closed"));
        // `LayerWindow` is nameable through the old path (the type check is
        // the assertion; there is no compositor here to open one against).
        fn _accepts(
            _: fn(
                crate::css::cascade::CompiledSheet,
                crate::text::FontDatabase,
                crate::widget::button::Button,
            ) -> Result<super::LayerWindow, super::LayerWindowError>,
        ) {
        }
        _accepts(super::LayerWindow::open);
    }
}
