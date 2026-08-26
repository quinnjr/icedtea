//! GTK's `@define-color` table and the color-value grammar M1 needs.
//!
//! Literal syntax (`#rgb`/`#rrggbb`, `rgb()`, `rgba()`, `hsl()`, CSS named
//! colors) is delegated to `skia_rs_core::Color::from_css`, which already
//! implements CSS Color Level 3 and 4. On top of that this module adds the
//! one GTK extension the M1 slice needs: `@name` references into the
//! `@define-color` table.
//!
//! Not implemented here (M2): GTK's `alpha()`, `shade()` and `mix()`
//! functions, and CSS Color Level 5 relative syntax (`rgb(from ...)`,
//! `hsl(from ...)`). No Adwaita `button` rule uses any of them; entries in
//! the table whose value needs them are left unresolved rather than
//! guessed at.

use std::collections::HashMap;

use skia_rs_safe::core::Color;

/// Resolved `@define-color` names.
pub type ColorTable = HashMap<String, Color>;

/// Resolve one color value against `table`.
///
/// Returns `None` for anything this milestone cannot interpret -- including
/// values that are not colors at all (`linear-gradient(...)`), which is how
/// `super::computed` distinguishes a flat background from a gradient.
#[must_use]
pub fn parse_color_value(value: &str, table: &ColorTable) -> Option<Color> {
    let value = value.trim();
    if let Some(name) = value.strip_prefix('@') {
        return table.get(name.trim()).copied();
    }
    Color::from_css(value)
}

/// Build the color table from `@define-color` pairs in source order.
///
/// Resolution is incremental and order-sensitive, exactly as GTK's is: a
/// definition may reference any name defined *above* it, and a later
/// redefinition of a name wins for everything below it.
#[must_use]
pub fn build_color_table(definitions: &[(String, String)]) -> ColorTable {
    let mut table = ColorTable::new();
    for (name, value) in definitions {
        match parse_color_value(value, &table) {
            Some(color) => {
                table.insert(name.clone(), color);
            }
            None => {
                tracing::debug!(%name, %value, "unresolved @define-color; skipping");
            }
        }
    }
    table
}

#[cfg(test)]
mod tests {
    use super::{ColorTable, build_color_table, parse_color_value};
    use crate::css::parse::parse_stylesheet;
    use skia_rs_safe::core::Color;

    fn adwaita_table() -> ColorTable {
        build_color_table(&parse_stylesheet(crate::BUNDLED_ADWAITA_LIGHT).color_definitions)
    }

    #[test]
    fn resolves_adwaita_named_colors() {
        let table = adwaita_table();
        assert_eq!(table.get("theme_fg_color"), Some(&Color(0xFF2E_3436)));
        assert_eq!(table.get("borders"), Some(&Color(0xFFCD_C7C2)));
        assert_eq!(table.get("accent_color"), Some(&Color(0xFF35_84E4)));
        assert_eq!(table.get("theme_text_color"), Some(&Color(0xFF00_0000)));
        // rgba(255, 255, 255, 0.8) -> alpha 204 (0.8 * 255, rounded).
        assert_eq!(table.get("wm_highlight"), Some(&Color(0xCCFF_FFFF)));
    }

    #[test]
    fn relative_color_syntax_is_recorded_as_unresolved_not_fabricated() {
        let table = adwaita_table();
        // `@define-color wm_shadow rgb(from black r g b / calc(alpha * 0.35))`
        // is CSS Color 5 relative syntax, out of scope for M1.
        assert!(!table.contains_key("wm_shadow"));
        // 37 declared, 8 of them relative-color syntax.
        assert_eq!(table.len(), 29);
    }

    #[test]
    fn at_name_references_resolve_through_the_table() {
        let table = build_color_table(&[
            ("borders".to_string(), "#cdc7c2".to_string()),
            ("edge".to_string(), "@borders".to_string()),
        ]);
        assert_eq!(table.get("edge"), Some(&Color(0xFFCD_C7C2)));
        assert_eq!(parse_color_value("@edge", &table), Some(Color(0xFFCD_C7C2)));
        assert_eq!(parse_color_value("@nope", &table), None);
    }

    #[test]
    fn literal_forms_delegate_to_skias_css_parser() {
        let table = ColorTable::new();
        assert_eq!(
            parse_color_value("#2e3436", &table),
            Some(Color(0xFF2E_3436))
        );
        assert_eq!(parse_color_value("white", &table), Some(Color(0xFFFF_FFFF)));
        assert_eq!(
            parse_color_value("rgb(53, 132, 228)", &table),
            Some(Color(0xFF35_84E4))
        );
        assert_eq!(
            parse_color_value("transparent", &table),
            Some(Color(0x0000_0000))
        );
        assert_eq!(
            parse_color_value("linear-gradient(to top, red, blue)", &table),
            None
        );
    }
}
