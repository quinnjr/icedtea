//! Wiring: theme discovery plus one themed button on a layer surface.
//!
//! GTK4 does not have *a* stylesheet; it has a **stack** of them, applied at
//! different priorities. The two that matter for a themed client are the
//! *theme* (priority 200) and the user's own `gtk.css` (priority 800). This
//! module reproduces that stack with the cascade's own source-order rule:
//! the base theme's rules first, the user override's rules after, so a
//! declaration in the override beats an equally-specific one in the theme.

use std::path::{Path, PathBuf};

use crate::BUNDLED_ADWAITA_LIGHT;
use crate::css::cascade::CompiledSheet;
use crate::css::node::Node;
use crate::css::parse::{Stylesheet, parse_stylesheet_with_base};
use crate::layout::Allocation;
use crate::text::FontStack;
use crate::wayland::{LayerWindow, LayerWindowError};
use crate::widget::button::Button;

/// Where to read the GTK4 theme from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThemeSource {
    /// The vendored Adwaita copy. Hermetic; what tests use.
    Bundled,
    /// A specific `gtk.css`, used **whole**: an explicit file is a complete
    /// theme, not an override, so no user `gtk.css` is layered onto it.
    File(PathBuf),
    /// GTK's own stack: the `$GTK_THEME` theme (or the vendored Adwaita),
    /// with the user's `gtk-4.0/gtk.css` layered over it.
    UserPreferred,
}

/// The `gtk-4.0` subdirectory every GTK4 theme and user override lives in.
const GTK4_DIR: &str = "gtk-4.0";

/// The environment [`ThemeSource::UserPreferred`] resolves against.
///
/// A struct rather than direct `std::env::var` calls so the resolution rules
/// are testable without mutating process-global state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ThemeEnv {
    /// `$GTK_THEME`, in GTK's `Name[:variant]` form.
    pub gtk_theme: Option<String>,
    /// `$XDG_CONFIG_HOME`.
    pub xdg_config_home: Option<String>,
    /// `$XDG_DATA_HOME`.
    pub xdg_data_home: Option<String>,
    /// `$HOME`.
    pub home: Option<String>,
}

/// Read `name`, treating an empty value as unset — which is what
/// `XDG_CONFIG_HOME=""` means, and what previously made the loader probe the
/// process's current working directory.
fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

impl ThemeEnv {
    /// Read the four variables from the process environment.
    #[must_use]
    pub fn from_env() -> Self {
        Self {
            gtk_theme: env_var("GTK_THEME"),
            xdg_config_home: env_var("XDG_CONFIG_HOME"),
            xdg_data_home: env_var("XDG_DATA_HOME"),
            home: env_var("HOME"),
        }
    }

    /// `$XDG_CONFIG_HOME`, falling back to `$HOME/.config`.
    #[must_use]
    pub fn config_dir(&self) -> Option<PathBuf> {
        self.xdg_config_home
            .as_ref()
            .map(PathBuf::from)
            .or_else(|| {
                self.home
                    .as_ref()
                    .map(|home| Path::new(home).join(".config"))
            })
    }

    /// The directories a GTK4 theme may be installed in, in search order.
    #[must_use]
    pub fn theme_dirs(&self) -> Vec<PathBuf> {
        let mut dirs = Vec::new();
        if let Some(home) = &self.home {
            dirs.push(Path::new(home).join(".themes"));
        }
        if let Some(data) = &self.xdg_data_home {
            dirs.push(Path::new(data).join("themes"));
        }
        if let Some(home) = &self.home {
            dirs.push(Path::new(home).join(".local/share/themes"));
        }
        dirs.push(PathBuf::from("/usr/share/themes"));
        dirs
    }

    /// Candidate base-theme files for `$GTK_THEME`, in search order.
    ///
    /// `Name:dark` asks for the theme's dark variant, which GTK4 ships as
    /// `gtk-dark.css`; the plain `gtk.css` of the same theme is kept as a
    /// fallback after every directory has been tried for the dark file, so a
    /// theme with no dark variant still loads instead of silently reverting
    /// to the vendored Adwaita.
    ///
    /// Empty when `$GTK_THEME` is unset: GTK's default theme is Adwaita,
    /// which GTK4 carries internally rather than on disk, so the vendored
    /// copy *is* the right answer and there is nothing to probe.
    #[must_use]
    pub fn base_theme_candidates(&self) -> Vec<PathBuf> {
        let Some(spec) = &self.gtk_theme else {
            return Vec::new();
        };
        let mut parts = spec.split(':');
        let name = parts.next().unwrap_or(spec);
        if name.is_empty() {
            return Vec::new();
        }
        let dark = parts.any(|variant| variant.eq_ignore_ascii_case("dark"));

        let dirs = self.theme_dirs();
        let mut candidates = Vec::with_capacity(dirs.len() * 2);
        if dark {
            for dir in &dirs {
                candidates.push(dir.join(name).join(GTK4_DIR).join("gtk-dark.css"));
            }
        }
        for dir in &dirs {
            candidates.push(dir.join(name).join(GTK4_DIR).join("gtk.css"));
        }
        candidates
    }

    /// The user's own override sheet, GTK's priority-800 layer.
    #[must_use]
    pub fn user_override_path(&self) -> Option<PathBuf> {
        self.config_dir()
            .map(|dir| dir.join(GTK4_DIR).join("gtk.css"))
    }
}

/// Parse `path`, resolving its `@import`s against its own directory.
fn parse_file(path: &Path) -> Option<Stylesheet> {
    let css = std::fs::read_to_string(path).ok()?;
    Some(parse_stylesheet_with_base(&css, path.parent()))
}

/// Append `layer`'s rules after `base`'s, renumbering source order so the
/// cascade sees one sheet in which the later layer wins ties.
fn append_layer(base: &mut Stylesheet, layer: Stylesheet) {
    let offset = base
        .rules
        .iter()
        .map(|rule| rule.source_order + 1)
        .max()
        .unwrap_or(0);
    for mut rule in layer.rules {
        rule.source_order += offset;
        base.rules.push(rule);
    }
    base.color_definitions.extend(layer.color_definitions);
}

/// GTK's theme stack for `env`: the base theme, then the user's override.
///
/// The base is the first readable [`ThemeEnv::base_theme_candidates`] entry,
/// or the vendored Adwaita. The override is
/// [`ThemeEnv::user_override_path`] if it is readable. Both are parsed with
/// their own directory as the `@import` base.
#[must_use]
pub fn load_layered_stylesheet(env: &ThemeEnv) -> Stylesheet {
    let mut sheet = None;
    for candidate in env.base_theme_candidates() {
        if let Some(parsed) = parse_file(&candidate) {
            tracing::info!(base = %candidate.display(), "loaded base GTK4 theme");
            sheet = Some(parsed);
            break;
        }
    }
    let mut sheet = sheet.unwrap_or_else(|| {
        tracing::info!(base = "bundled Adwaita", "no installed GTK4 theme found");
        parse_stylesheet_with_base(BUNDLED_ADWAITA_LIGHT, None)
    });

    if let Some(path) = env.user_override_path()
        && let Some(overrides) = parse_file(&path)
    {
        tracing::info!(
            overrides = %path.display(),
            rules = overrides.rules.len(),
            "layering the user's gtk.css over the theme"
        );
        append_layer(&mut sheet, overrides);
    }
    sheet
}

/// Compile the stylesheet `source` describes.
#[must_use]
pub fn compile_theme(source: &ThemeSource) -> CompiledSheet {
    match source {
        ThemeSource::Bundled => {
            CompiledSheet::from_stylesheet(parse_stylesheet_with_base(BUNDLED_ADWAITA_LIGHT, None))
        }
        ThemeSource::File(path) => {
            let sheet = parse_file(path).unwrap_or_else(|| {
                tracing::warn!(path = %path.display(), "cannot read theme; using bundled Adwaita");
                parse_stylesheet_with_base(BUNDLED_ADWAITA_LIGHT, None)
            });
            CompiledSheet::from_stylesheet(sheet)
        }
        ThemeSource::UserPreferred => {
            CompiledSheet::from_stylesheet(load_layered_stylesheet(&ThemeEnv::from_env()))
        }
    }
}

/// The `window > button` node tree the M1 slice styles.
fn button_node(label: &str, classes: &[&str]) -> Button {
    let window = Node::with_classes("window", &["background"]);
    Button::new(label, classes, window)
}

/// Build and style the M1 button, returning it with its font stack.
///
/// # Errors
///
/// [`LayerWindowError::NoFont`] if no usable typeface is installed.
pub fn themed_button(
    label: &str,
    classes: &[&str],
    theme: &ThemeSource,
) -> Result<(CompiledSheet, FontStack, Button), LayerWindowError> {
    let sheet = compile_theme(theme);
    let fonts = FontStack::system().ok_or(LayerWindowError::NoFont)?;
    let mut button = button_node(label, classes);
    button.restyle(&sheet, &fonts);
    Ok((sheet, fonts, button))
}

/// The allocation the M1 button would take, without opening a window.
///
/// This is what `themed-button --print-allocation` reports, so a test can
/// derive the on-screen geometry instead of hardcoding it.
///
/// # Errors
///
/// [`LayerWindowError::NoFont`] if no usable typeface is installed.
pub fn themed_button_allocation(
    label: &str,
    classes: &[&str],
    theme: &ThemeSource,
) -> Result<Allocation, LayerWindowError> {
    let (_sheet, _fonts, button) = themed_button(label, classes, theme)?;
    Ok(button.allocation())
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
    let (sheet, fonts, button) = themed_button(label, classes, theme)?;
    let mut window = LayerWindow::open(sheet, fonts, button)?;
    window.run()
}

#[cfg(test)]
mod tests {
    use super::{ThemeEnv, ThemeSource, compile_theme, load_layered_stylesheet};
    use crate::css::computed::{ComputedStyle, ResolveEnv};
    use crate::css::node::Node;
    use crate::css::select::MatchCx;
    use skia_rs_safe::core::Color;
    use std::path::Path;

    fn write(path: &Path, contents: &str) {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, contents).expect("write");
    }

    fn button_style(sheet: &crate::css::cascade::CompiledSheet) -> ComputedStyle {
        let window = Node::with_classes("window", &["background"]);
        let button = Node::new("button");
        window.append_child(&button);
        ComputedStyle::resolve_chain(sheet, &button, &ResolveEnv::default(), &mut MatchCx::new())
    }

    fn headerbar_min_height(sheet: &crate::css::cascade::CompiledSheet) -> f32 {
        let window = Node::with_classes("window", &["background"]);
        let headerbar = Node::new("headerbar");
        window.append_child(&headerbar);
        ComputedStyle::resolve_chain(
            sheet,
            &headerbar,
            &ResolveEnv::default(),
            &mut MatchCx::new(),
        )
        .min_height
    }

    #[test]
    fn a_user_override_layers_over_the_theme_instead_of_replacing_it() {
        // The reviewer's A1 scenario: `$XDG_CONFIG_HOME/gtk-4.0/gtk.css` is
        // GTK's priority-800 *override*, not the whole sheet. A file that
        // only restyles `headerbar` must leave Adwaita's button intact.
        let tmp = tempfile::tempdir().expect("tempdir");
        write(
            &tmp.path().join("config/gtk-4.0/gtk.css"),
            "headerbar { min-height: 32px }\n",
        );
        let env = ThemeEnv {
            xdg_config_home: Some(tmp.path().join("config").display().to_string()),
            ..ThemeEnv::default()
        };

        let sheet =
            crate::css::cascade::CompiledSheet::from_stylesheet(load_layered_stylesheet(&env));
        let style = button_style(&sheet);
        assert_eq!(
            style.border_color,
            Color(0xFFCD_C7C2),
            "the override replaced the base theme instead of layering onto it"
        );
        assert_eq!(style.padding, [4.0, 9.0, 4.0, 9.0]);
        assert_eq!(
            headerbar_min_height(&sheet),
            32.0,
            "the user's own override never reached the cascade"
        );
    }

    #[test]
    fn the_override_wins_ties_against_the_base_theme() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(
            &tmp.path().join("config/gtk-4.0/gtk.css"),
            "button { min-height: 41px }\n",
        );
        let env = ThemeEnv {
            xdg_config_home: Some(tmp.path().join("config").display().to_string()),
            ..ThemeEnv::default()
        };
        let sheet =
            crate::css::cascade::CompiledSheet::from_stylesheet(load_layered_stylesheet(&env));
        assert_eq!(
            button_style(&sheet).min_height,
            41.0,
            "an equally specific override rule must win on source order"
        );
    }

    #[test]
    fn the_override_can_import_relative_to_its_own_directory() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(
            &tmp.path().join("config/gtk-4.0/gtk.css"),
            "@import 'extra.css';\n",
        );
        write(
            &tmp.path().join("config/gtk-4.0/extra.css"),
            "button { min-height: 43px }\n",
        );
        let env = ThemeEnv {
            xdg_config_home: Some(tmp.path().join("config").display().to_string()),
            ..ThemeEnv::default()
        };
        let sheet =
            crate::css::cascade::CompiledSheet::from_stylesheet(load_layered_stylesheet(&env));
        assert_eq!(button_style(&sheet).min_height, 43.0);
    }

    #[test]
    fn a_dark_gtk_theme_picks_the_dark_variant_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let theme = tmp.path().join(".themes/Foo/gtk-4.0");
        write(&theme.join("gtk.css"), "button { min-height: 11px }\n");
        write(&theme.join("gtk-dark.css"), "button { min-height: 77px }\n");
        let env = ThemeEnv {
            gtk_theme: Some("Foo:dark".to_string()),
            home: Some(tmp.path().display().to_string()),
            ..ThemeEnv::default()
        };
        assert!(
            env.base_theme_candidates()[0].ends_with("Foo/gtk-4.0/gtk-dark.css"),
            "`Name:dark` dropped the variant: {:?}",
            env.base_theme_candidates()[0]
        );
        let sheet =
            crate::css::cascade::CompiledSheet::from_stylesheet(load_layered_stylesheet(&env));
        assert_eq!(button_style(&sheet).min_height, 77.0);
    }

    #[test]
    fn a_light_gtk_theme_picks_the_plain_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let theme = tmp.path().join(".themes/Foo/gtk-4.0");
        write(&theme.join("gtk.css"), "button { min-height: 11px }\n");
        write(&theme.join("gtk-dark.css"), "button { min-height: 77px }\n");
        let env = ThemeEnv {
            gtk_theme: Some("Foo".to_string()),
            home: Some(tmp.path().display().to_string()),
            ..ThemeEnv::default()
        };
        let sheet =
            crate::css::cascade::CompiledSheet::from_stylesheet(load_layered_stylesheet(&env));
        assert_eq!(button_style(&sheet).min_height, 11.0);
    }

    #[test]
    fn a_dark_theme_without_a_dark_file_falls_back_to_gtk_css() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(
            &tmp.path().join(".themes/Foo/gtk-4.0/gtk.css"),
            "button { min-height: 11px }\n",
        );
        let env = ThemeEnv {
            gtk_theme: Some("Foo:dark".to_string()),
            home: Some(tmp.path().display().to_string()),
            ..ThemeEnv::default()
        };
        let sheet =
            crate::css::cascade::CompiledSheet::from_stylesheet(load_layered_stylesheet(&env));
        assert_eq!(button_style(&sheet).min_height, 11.0);
    }

    #[test]
    fn an_empty_xdg_config_home_falls_back_to_home_dot_config() {
        // `XDG_CONFIG_HOME=""` previously made `PathBuf::from("")` join a
        // relative path, i.e. probe the process's working directory.
        let env = ThemeEnv {
            xdg_config_home: None,
            home: Some("/home/somebody".to_string()),
            ..ThemeEnv::default()
        };
        assert_eq!(
            env.user_override_path().expect("override path"),
            Path::new("/home/somebody/.config/gtk-4.0/gtk.css")
        );

        // And `ThemeEnv::from_env` maps the empty string to `None`, which is
        // what makes the fallback above reachable.
        assert_eq!(super::env_var("ICEDTEA_UI_DEFINITELY_UNSET_VAR"), None);
    }

    #[test]
    fn no_home_and_no_xdg_means_no_override_path() {
        assert_eq!(ThemeEnv::default().user_override_path(), None);
        assert!(ThemeEnv::default().base_theme_candidates().is_empty());
    }

    #[test]
    fn theme_dirs_are_searched_in_gtk_order() {
        let env = ThemeEnv {
            home: Some("/h".to_string()),
            xdg_data_home: Some("/d".to_string()),
            ..ThemeEnv::default()
        };
        assert_eq!(
            env.theme_dirs(),
            vec![
                Path::new("/h/.themes"),
                Path::new("/d/themes"),
                Path::new("/h/.local/share/themes"),
                Path::new("/usr/share/themes"),
            ]
        );
    }

    #[test]
    fn an_explicit_theme_file_is_a_whole_theme_not_an_override() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("only.css");
        std::fs::write(&path, "button { min-height: 13px }\n").expect("write");
        let sheet = compile_theme(&ThemeSource::File(path));
        let style = button_style(&sheet);
        assert_eq!(style.min_height, 13.0);
        // Contract deviation 4: `border-color`'s registry initial is
        // `currentColor`, which resolves to the initial `color` -- opaque
        // black. M1 had no initial and left it transparent. What the test
        // means is unchanged: Adwaita's #cdc7c2 is nowhere near it.
        assert_eq!(
            style.border_color,
            Color(0xFF00_0000),
            "an explicit file must not be layered onto Adwaita"
        );
        assert_ne!(
            style.border_color,
            Color(0xFFCD_C7C2),
            "Adwaita was layered under the explicit file"
        );
    }

    #[test]
    fn the_bundled_source_is_exactly_the_vendored_sheet() {
        let sheet = compile_theme(&ThemeSource::Bundled);
        assert_eq!(button_style(&sheet).border_color, Color(0xFFCD_C7C2));
        assert_eq!(sheet.rules.len(), 900);
    }
}
