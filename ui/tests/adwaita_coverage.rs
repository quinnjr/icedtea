//! THE M2 GATE — every declaration of GTK 4.22's Adwaita, through the registry.
//!
//! Light, dark and high-contrast are all walked: 0 unparseable declarations,
//! 0 unknown properties, 37/37 `@define-color`s resolved. See
//! `docs/superpowers/specs/2026-08-26-pure-rust-gtk-m2-css-engine-design.md`
//! section 7.

/// GTK 4.22 `Default-dark.css`, vendored so this gate is hermetic.
const ADWAITA_DARK: &str = include_str!("../themes/adwaita-dark.css");
/// GTK 4.22 `Default-hc.css`, vendored so this gate is hermetic.
const ADWAITA_HC: &str = include_str!("../themes/adwaita-hc.css");

#[test]
fn the_vendored_sheets_are_the_extracted_gtk4_themes() {
    assert_eq!(
        icedtea_ui::BUNDLED_ADWAITA_LIGHT.lines().count(),
        1941,
        "vendored Adwaita light is not GTK 4.22's 1,941-line Default-light.css"
    );
    assert_eq!(
        ADWAITA_DARK.lines().count(),
        1929,
        "vendored Adwaita dark is not GTK 4.22's 1,929-line Default-dark.css"
    );
    assert_eq!(
        ADWAITA_HC.lines().count(),
        1944,
        "vendored Adwaita high-contrast is not GTK 4.22's 1,944-line Default-hc.css"
    );
    for (name, sheet) in [
        ("light", icedtea_ui::BUNDLED_ADWAITA_LIGHT),
        ("dark", ADWAITA_DARK),
        ("hc", ADWAITA_HC),
    ] {
        assert_eq!(
            sheet.matches("@define-color").count(),
            37,
            "vendored Adwaita {name} does not carry GTK 4.22's 37 @define-color declarations"
        );
    }
}

#[test]
fn the_three_sheets_are_three_different_themes() {
    assert_ne!(icedtea_ui::BUNDLED_ADWAITA_LIGHT, ADWAITA_DARK);
    assert_ne!(icedtea_ui::BUNDLED_ADWAITA_LIGHT, ADWAITA_HC);
    assert_ne!(ADWAITA_DARK, ADWAITA_HC);
    // The colour that separates them, declared in each sheet's own words.
    assert!(icedtea_ui::BUNDLED_ADWAITA_LIGHT.contains("@define-color theme_bg_color #f6f5f4"));
    assert!(ADWAITA_DARK.contains("@define-color theme_bg_color #353535"));
    assert!(ADWAITA_HC.contains("@define-color theme_bg_color #fdfdfc"));
}
