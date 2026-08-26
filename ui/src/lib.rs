//! `icedtea-ui` — a pure-Rust, GTK4-theme-compatible widget layer.
//!
//! M1 proving slice: one themed `button`, from a real GTK4 `gtk.css`
//! through selector matching, cascade, `taffy` layout and `skia-rs` paint,
//! onto a `zwlr_layer_shell_v1` surface. See
//! `docs/superpowers/specs/2026-08-20-pure-rust-gtk-ui-design.md`.

pub mod css;
pub mod layout;
pub mod text;

/// GTK 4's default light theme, vendored so tests are hermetic.
///
/// See `ui/themes/README.md` for provenance and the LGPL-2.1-or-later
/// note that covers it.
pub const BUNDLED_ADWAITA_LIGHT: &str = include_str!("../themes/adwaita-light.css");

#[cfg(test)]
mod tests {
    use super::BUNDLED_ADWAITA_LIGHT;

    #[test]
    fn bundled_adwaita_is_the_extracted_gtk4_theme() {
        assert_eq!(
            BUNDLED_ADWAITA_LIGHT.lines().count(),
            1941,
            "vendored Adwaita is not the 1,941-line GTK4 Default-light.css this crate's \
             expected values were derived from"
        );
        assert_eq!(
            BUNDLED_ADWAITA_LIGHT.matches("@define-color").count(),
            37,
            "vendored Adwaita does not carry GTK4's 37 @define-color declarations"
        );
        assert!(
            BUNDLED_ADWAITA_LIGHT
                .contains("background-image: linear-gradient(to top, #f6f5f4 2px, #fbfafa)"),
            "vendored Adwaita is missing the base `button` background this crate asserts on"
        );
    }
}
