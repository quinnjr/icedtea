//! Wiring: theme discovery plus one themed button on a layer surface.

use std::path::PathBuf;

use crate::BUNDLED_ADWAITA_LIGHT;
use crate::css::cascade::CompiledSheet;
use crate::css::select::{CssNode, PseudoStates};
use crate::text::FontStack;
use crate::wayland::{LayerWindow, LayerWindowError};
use crate::widget::button::Button;

/// Where to read the GTK4 theme from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThemeSource {
    /// The vendored Adwaita copy. Hermetic; what tests use.
    Bundled,
    /// A specific `gtk.css`.
    File(PathBuf),
    /// The user's own theme if one is installed, else [`Self::Bundled`].
    UserPreferred,
}

/// The paths `ThemeSource::UserPreferred` probes, in order.
fn user_theme_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(config) = std::env::var("XDG_CONFIG_HOME") {
        candidates.push(PathBuf::from(config).join("gtk-4.0/gtk.css"));
    } else if let Ok(home) = std::env::var("HOME") {
        candidates.push(PathBuf::from(home).join(".config/gtk-4.0/gtk.css"));
    }
    if let Ok(name) = std::env::var("GTK_THEME") {
        let name = name.split(':').next().unwrap_or(&name).to_string();
        candidates.push(PathBuf::from(format!(
            "/usr/share/themes/{name}/gtk-4.0/gtk.css"
        )));
    }
    candidates.push(PathBuf::from("/usr/share/themes/Adwaita/gtk-4.0/gtk.css"));
    candidates
}

/// Read the CSS for `source`, falling back to the bundled copy.
#[must_use]
pub fn load_theme(source: &ThemeSource) -> String {
    match source {
        ThemeSource::Bundled => BUNDLED_ADWAITA_LIGHT.to_string(),
        ThemeSource::File(path) => match std::fs::read_to_string(path) {
            Ok(css) => css,
            Err(err) => {
                tracing::warn!(path = %path.display(), %err, "cannot read theme; using bundled Adwaita");
                BUNDLED_ADWAITA_LIGHT.to_string()
            }
        },
        ThemeSource::UserPreferred => {
            for candidate in user_theme_candidates() {
                if let Ok(css) = std::fs::read_to_string(&candidate) {
                    tracing::info!(path = %candidate.display(), "loaded user GTK4 theme");
                    return css;
                }
            }
            tracing::info!("no installed GTK4 theme found; using bundled Adwaita");
            BUNDLED_ADWAITA_LIGHT.to_string()
        }
    }
}

/// Show one themed button on a layer surface until the compositor closes it.
///
/// # Errors
///
/// [`LayerWindowError`] if no usable typeface is installed, or if the
/// compositor connection, the layer surface or a repaint fails.
pub fn run_themed_button(
    label: &str,
    classes: &[&str],
    theme: &ThemeSource,
) -> Result<(), LayerWindowError> {
    let sheet = CompiledSheet::compile(&load_theme(theme));
    let fonts = FontStack::system().ok_or_else(|| {
        LayerWindowError::Io(std::io::Error::other(
            "no UI typeface found; install dejavu, liberation or noto sans",
        ))
    })?;
    let window_node = CssNode::new("window", &["background"], PseudoStates::default(), None);
    let button = Button::new(label, classes, window_node);
    let mut window = LayerWindow::open(sheet, fonts, button)?;
    window.run()
}
