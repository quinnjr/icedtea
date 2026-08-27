//! `icedtea-ui` — a pure-Rust, GTK4-theme-compatible widget layer.
//!
//! M1 proving slice: one themed `button`, from a real GTK4 `gtk.css`
//! through selector matching, cascade, `taffy` layout and `skia-rs` paint,
//! onto a `zwlr_layer_shell_v1` surface. See
//! `docs/superpowers/specs/2026-08-20-pure-rust-gtk-ui-design.md`.

pub mod anim;
pub mod app;
pub mod css;
pub mod layout;
pub mod paint;
pub mod shm;
pub mod text;
pub mod wayland;
pub mod widget;

/// GTK 4's default light theme, vendored so tests are hermetic.
///
/// See `ui/themes/README.md` for provenance and the LGPL-2.1-or-later
/// note that covers it.
pub const BUNDLED_ADWAITA_LIGHT: &str = include_str!("../themes/adwaita-light.css");

/// GTK 4's default *dark* theme, vendored alongside the light one.
///
/// GTK4 carries Adwaita internally rather than on disk, so on a stock system
/// `/usr/share/themes/Adwaita*/` holds only `gtk-3.0` files and every
/// [`app::ThemeEnv::base_theme_candidates`] entry misses. Without this,
/// `GTK_THEME=Adwaita:dark` compiled the *light* sheet under a dark
/// [`css::parse::MediaEnv`] and rendered `#f6f5f4` backgrounds.
///
/// See `ui/themes/README.md` for provenance and the LGPL-2.1-or-later note.
pub const BUNDLED_ADWAITA_DARK: &str = include_str!("../themes/adwaita-dark.css");

/// GTK 4's high-contrast theme, vendored for the same reason.
///
/// `GTK_THEME=Adwaita:hc` / `HighContrast` used to get `Contrast::More` over
/// the light sheet.
pub const BUNDLED_ADWAITA_HC: &str = include_str!("../themes/adwaita-hc.css");

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
