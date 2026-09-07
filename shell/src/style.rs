//! The panel's own style sheet, layered over the bundled Adwaita stack.
//!
//! GTK registered `style.css` display-wide with a `CssProvider` at
//! `STYLE_PROVIDER_PRIORITY_APPLICATION`. `icedtea-ui` has no display-wide
//! provider registry: a sheet belongs to a `Window`, and an app sheet layers
//! by being compiled *after* the theme in one source, which is exactly the
//! cascade origin the theme override uses.

use icedtea_ui::css::cascade::CompiledSheet;
use icedtea_ui::css::parse::parse_stylesheet_with_base;
use icedtea_ui::gallery::Theme;

/// `shell/style.css`, verbatim.
const PANEL_CSS: &str = include_str!("../style.css");

/// The theme `$ICEDTEA_THEME` names, defaulting to [`Theme::Dark`].
///
/// Dark is the default because the panel's own colours are dark
/// (`#bar { background-color: rgba(20, 20, 24, 0.92); color: #e6e6e6; }`); an
/// unreadable value is a default, never an error, per the contract's
/// untrusted-input rule.
#[must_use]
pub fn theme_from_env() -> Theme {
    std::env::var("ICEDTEA_THEME")
        .ok()
        .and_then(|name| Theme::parse(&name))
        .unwrap_or(Theme::Dark)
}

/// Compile the bundled panel sheet on top of the Adwaita stack for `theme`.
#[must_use]
pub fn sheet_for(theme: Theme) -> CompiledSheet {
    let css = format!("{}\n{PANEL_CSS}", theme.sheet());
    CompiledSheet::compile_with_env(&parse_stylesheet_with_base(&css, None), &theme.media_env())
}

/// [`sheet_for`] the theme [`theme_from_env`] resolves.
#[must_use]
pub fn sheet() -> CompiledSheet {
    sheet_for(theme_from_env())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Finding F10's accent stripe is a `background-image: linear-gradient`,
    /// chosen so the focused/attention state reads without reflowing the
    /// button's metrics. If M2's engine dropped it, the two states would be
    /// invisible again and nothing else would notice.
    #[test]
    fn the_panel_sheet_keeps_the_focus_and_attention_gradients() {
        for theme in [Theme::Light, Theme::Dark, Theme::HighContrast] {
            let compiled = sheet_for(theme);
            let text = format!("{compiled:?}");
            assert!(
                text.contains("89b4fa") || text.contains("137, 180, 250"),
                "{theme:?}: the #bar button.focused accent is missing"
            );
            assert!(
                text.contains("f38ba8") || text.contains("243, 139, 168"),
                "{theme:?}: the #bar button.attention accent is missing"
            );
        }
    }
}
