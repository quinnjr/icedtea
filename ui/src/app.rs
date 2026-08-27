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

/// GTK 4's default *dark* theme, vendored alongside the light one.
///
/// GTK4 carries Adwaita internally rather than on disk, so on a stock system
/// `/usr/share/themes/Adwaita*/` holds only `gtk-3.0` files and every
/// [`ThemeEnv::base_theme_candidates`] entry misses. Without this,
/// `GTK_THEME=Adwaita:dark` compiled the *light* sheet under a dark
/// [`MediaEnv`] and rendered `#f6f5f4` backgrounds.
///
/// See `ui/themes/README.md` for provenance and the LGPL-2.1-or-later note.
pub const BUNDLED_ADWAITA_DARK: &str = include_str!("../themes/adwaita-dark.css");

/// GTK 4's high-contrast theme, vendored for the same reason.
///
/// `GTK_THEME=Adwaita:hc` / `HighContrast` used to get `Contrast::More` over
/// the light sheet.
pub const BUNDLED_ADWAITA_HC: &str = include_str!("../themes/adwaita-hc.css");

/// The bundled sheet that matches `env`.
///
/// High contrast wins over the colour scheme: GTK ships high contrast as its
/// own theme, and the vendored copy is the light one, which is what
/// `Adwaita:hc` and `HighContrast` both name. `HighContrastInverse` has no
/// vendored copy, so it falls to the plain dark sheet -- the closer of the
/// two.
#[must_use]
pub fn bundled_sheet_for(env: &MediaEnv) -> &'static str {
    match (env.contrast, env.color_scheme) {
        (Contrast::More, ColorScheme::Light) => BUNDLED_ADWAITA_HC,
        (_, ColorScheme::Dark) => BUNDLED_ADWAITA_DARK,
        _ => BUNDLED_ADWAITA_LIGHT,
    }
}
use crate::css::cascade::CompiledSheet;
use crate::css::node::Node;
use crate::css::parse::{ColorScheme, Contrast, MediaEnv, Stylesheet, parse_stylesheet_with_base};
use crate::layout::Allocation;
use crate::text::FontDatabase;
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

    /// `$GTK_THEME` split into its theme name and its `:`-separated
    /// variants, or `None` when it is unset or names an empty theme.
    fn theme_spec(&self) -> Option<(&str, impl Iterator<Item = &str>)> {
        let spec = self.gtk_theme.as_ref()?;
        let mut parts = spec.split(':');
        let name = parts.next().unwrap_or(spec.as_str());
        if name.is_empty() {
            return None;
        }
        Some((name, parts))
    }

    /// The `@media` environment this theme selection implies.
    ///
    /// GTK does not expose a separate colour-scheme or contrast setting to a
    /// client this far down the stack: the *theme spelling* is the setting.
    /// `Name:dark` asks for the dark variant, so
    /// `(prefers-color-scheme: dark)` must match -- without this, a theme's
    /// dark `gtk-dark.css` loaded and then had every one of its
    /// `@media (prefers-color-scheme: dark)` blocks thrown away.
    ///
    /// Contrast comes from the same string: GTK ships high contrast as its
    /// own theme (`HighContrast`, and `HighContrastInverse` for the dark
    /// one), and a `Name:hc` variant is accepted for the themes that spell
    /// it that way. Everything else is `no-preference`, which is
    /// [`MediaEnv`]'s default.
    ///
    /// A `-dark` or `-hc` *name* suffix counts too, because that is what
    /// people actually type: `GTK_THEME=Adwaita-dark` names a whole theme
    /// directory, which [`base_theme_candidates`](Self::base_theme_candidates)
    /// already resolves to the right files -- so the media environment has to
    /// agree with it, or a `-dark` theme's dark `@media` blocks get dropped.
    ///
    /// Names and variants are matched case-insensitively, as GTK matches
    /// them, and by character rather than by byte: `$GTK_THEME` is
    /// user-controlled text and need not be ASCII.
    #[must_use]
    pub fn media_env(&self) -> MediaEnv {
        let Some((name, variants)) = self.theme_spec() else {
            return MediaEnv::default();
        };
        let variants: Vec<&str> = variants.collect();
        let has_variant = |wanted: &[&str]| {
            variants
                .iter()
                .any(|v| wanted.iter().any(|w| v.eq_ignore_ascii_case(w)))
        };
        // `$GTK_THEME` is user-controlled text and need not be ASCII, so both
        // affix tests go through `str::get`, which returns `None` on a byte
        // index that lands inside a character rather than panicking. A plain
        // `name[..n]` here aborted startup on a theme called, say,
        // `AAAAAAAAAAAéTheme`.
        let starts_with = |prefix: &str| {
            name.get(..prefix.len())
                .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
        };
        let ends_with = |suffix: &str| {
            name.len() > suffix.len()
                && name
                    .get(name.len() - suffix.len()..)
                    .is_some_and(|tail| tail.eq_ignore_ascii_case(suffix))
        };
        MediaEnv {
            color_scheme: if has_variant(&["dark"])
                || ends_with("-dark")
                || name.eq_ignore_ascii_case("HighContrastInverse")
            {
                ColorScheme::Dark
            } else {
                ColorScheme::Light
            },
            contrast: if starts_with("HighContrast")
                || ends_with("-hc")
                || ends_with("-highcontrast")
                || ends_with("-high-contrast")
                || has_variant(&["hc", "highcontrast", "high-contrast"])
            {
                Contrast::More
            } else {
                Contrast::NoPreference
            },
        }
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
        let Some((name, mut variants)) = self.theme_spec() else {
            return Vec::new();
        };
        let dark = variants.any(|variant| variant.eq_ignore_ascii_case("dark"));

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
    let media = env.media_env();
    let mut sheet = sheet.unwrap_or_else(|| {
        // The bundled fallback has to match the media environment the sheet
        // will be *compiled* under, or `GTK_THEME=Adwaita:dark` renders the
        // light theme's colours with the dark sheet's `@media` blocks
        // applied to it.
        let bundled = bundled_sheet_for(&media);
        tracing::info!(
            base = "bundled Adwaita",
            color_scheme = ?media.color_scheme,
            contrast = ?media.contrast,
            "no installed GTK4 theme found"
        );
        parse_stylesheet_with_base(bundled, None)
    });

    if let Some(path) = env.user_override_path()
        && let Some(overrides) = parse_file(&path)
    {
        tracing::info!(
            overrides = %path.display(),
            rules = overrides.rules.len(),
            "layering the user's gtk.css over the theme"
        );
        sheet.append_layer(overrides);
    }
    sheet
}

/// GTK's theme stack for `env`, compiled under the `@media` environment
/// `env` itself implies (see [`ThemeEnv::media_env`]).
///
/// Split out from [`compile_theme`] so the pairing of the loaded sheet with
/// its media environment is testable without mutating the process
/// environment.
#[must_use]
pub fn compile_for_theme_env(env: &ThemeEnv) -> CompiledSheet {
    CompiledSheet::compile_with_env(&load_layered_stylesheet(env), &env.media_env())
}

/// Compile the stylesheet `source` describes.
///
/// [`ThemeSource::Bundled`] and [`ThemeSource::File`] compile under
/// [`MediaEnv::default`] (light, no contrast preference): neither carries a
/// theme *spelling* to derive a preference from -- the bundled sheet is
/// Adwaita's light one, and an explicit file is whatever the caller pointed
/// at. Only [`ThemeSource::UserPreferred`] has `$GTK_THEME` to read.
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
        ThemeSource::UserPreferred => compile_for_theme_env(&ThemeEnv::from_env()),
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
) -> Result<(CompiledSheet, FontDatabase, Button), LayerWindowError> {
    let sheet = compile_theme(theme);
    let mut fonts = FontDatabase::new();
    let mut button = button_node(label, classes);
    button.restyle(&sheet, &mut fonts);
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
    Ok(themed_button_allocations(label, classes, theme)?.0)
}

/// The button's allocation *and* its label's, both tree-absolute.
///
/// `themed-button --print-allocation` prints the label's origin relative to
/// the button's border box. It used to print the button's own border +
/// padding inset instead and call it `label_x label_y`, which happens to
/// coincide on `x` for Adwaita and is wrong on `y` by the centring offset:
/// a 24px content box holding an 18px label puts the label 3px lower than
/// the padding edge, so a test sampling a glyph row from the printed `y`
/// sampled the padding gutter.
///
/// # Errors
///
/// [`LayerWindowError::NoFont`] if no usable typeface is installed.
pub fn themed_button_allocations(
    label: &str,
    classes: &[&str],
    theme: &ThemeSource,
) -> Result<(Allocation, Allocation), LayerWindowError> {
    let (_sheet, _fonts, button) = themed_button(label, classes, theme)?;
    Ok((button.allocation(), button.label_allocation()))
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
    use super::{
        ThemeEnv, ThemeSource, compile_for_theme_env, compile_theme, load_layered_stylesheet,
    };
    use crate::css::computed::{ComputedStyle, ResolveEnv};
    use crate::css::node::Node;
    use crate::css::parse::MediaEnv;
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

    fn button_min_height(sheet: &crate::css::cascade::CompiledSheet) -> f32 {
        button_style(sheet).min_size((0.0, 0.0)).1
    }

    fn button_min_width(sheet: &crate::css::cascade::CompiledSheet) -> f32 {
        button_style(sheet).min_size((0.0, 0.0)).0
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
        .min_size((0.0, 0.0))
        .1
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
            style.border_colors()[0].to_color32(),
            Color(0xFFCD_C7C2),
            "the override replaced the base theme instead of layering onto it"
        );
        assert_eq!(style.padding(0.0), [4.0, 9.0, 4.0, 9.0]);
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
            button_min_height(&sheet),
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
        assert_eq!(button_min_height(&sheet), 43.0);
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
        assert_eq!(button_min_height(&sheet), 77.0);
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
        assert_eq!(button_min_height(&sheet), 11.0);
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
        assert_eq!(button_min_height(&sheet), 11.0);
    }

    #[test]
    fn a_dark_gtk_theme_evaluates_prefers_color_scheme_dark() {
        // `MediaEnv` used to be left at its default (light) for every
        // production compile, so `$GTK_THEME=Foo:dark` loaded the dark
        // *file* and then threw away every `@media (prefers-color-scheme:
        // dark)` block inside it.
        let tmp = tempfile::tempdir().expect("tempdir");
        let theme = tmp.path().join(".themes/Foo/gtk-4.0");
        write(&theme.join("gtk.css"), "button { min-width: 11px }\n");
        write(
            &theme.join("gtk-dark.css"),
            "button { min-width: 11px }\n\
             @media (prefers-color-scheme: dark) { button { min-width: 77px } }\n",
        );
        let dark = ThemeEnv {
            gtk_theme: Some("Foo:dark".to_string()),
            home: Some(tmp.path().display().to_string()),
            ..ThemeEnv::default()
        };
        assert_eq!(
            button_min_width(&compile_for_theme_env(&dark)),
            77.0,
            "the dark variant's @media block never reached the cascade"
        );

        let light = ThemeEnv {
            gtk_theme: Some("Foo".to_string()),
            ..dark.clone()
        };
        assert_eq!(
            button_min_width(&compile_for_theme_env(&light)),
            11.0,
            "the light variant must not match prefers-color-scheme: dark"
        );
    }

    /// `$GTK_THEME` is user-controlled text, not ASCII by construction. The
    /// high-contrast check used to byte-slice `name[..12]`, which panics when
    /// byte 12 lands inside a multi-byte character -- on the live
    /// `UserPreferred` startup path, before anything is drawn.
    #[test]
    fn a_non_ascii_gtk_theme_name_does_not_panic() {
        // Byte 12 of this name is the middle of the `é`.
        let spec = "AAAAAAAAAAA\u{e9}Theme";
        assert!(!spec.is_char_boundary("HighContrast".len()));
        let env = ThemeEnv {
            gtk_theme: Some(spec.to_string()),
            ..ThemeEnv::default()
        };
        assert_eq!(env.media_env(), MediaEnv::default());

        // The same hazard at every other length a check could look at, plus a
        // name that is *shorter* than every affix.
        for spec in [
            "\u{e9}",
            "\u{1f600}",
            "d\u{e9}",
            "\u{e9}-dark",
            "-\u{e9}",
            "HighContrast\u{e9}",
            "\u{e9}:dark",
            "\u{4e2d}\u{6587}\u{4e3b}\u{9898}-Dark",
        ] {
            let env = ThemeEnv {
                gtk_theme: Some(spec.to_string()),
                ..ThemeEnv::default()
            };
            let _ = env.media_env();
            let _ = env.base_theme_candidates();
        }
    }

    /// The `-dark` / `-hc` spellings users actually type. GTK treats these as
    /// whole theme *names* (`~/.themes/Adwaita-dark/gtk-4.0/gtk.css`), which
    /// `base_theme_candidates` already resolves correctly -- but the media
    /// environment read them as plain light themes, so a `-dark` theme's
    /// `@media (prefers-color-scheme: dark)` blocks were dropped.
    #[test]
    fn a_dark_or_hc_theme_name_suffix_is_read_like_the_variant() {
        let env = |spec: &str| {
            ThemeEnv {
                gtk_theme: Some(spec.to_string()),
                ..ThemeEnv::default()
            }
            .media_env()
        };
        for spec in ["Adwaita-dark", "Foo-Dark", "Foo-DARK"] {
            assert_eq!(
                env(spec).color_scheme,
                crate::css::parse::ColorScheme::Dark,
                "`{spec}` is a dark theme"
            );
        }
        for spec in ["Foo-hc", "Foo-HC", "Foo-highcontrast", "Foo-high-contrast"] {
            assert_eq!(
                env(spec).contrast,
                crate::css::parse::Contrast::More,
                "`{spec}` is a high-contrast theme"
            );
        }
        // Both at once, in either spelling.
        assert_eq!(
            env("Adwaita-dark:hc"),
            MediaEnv {
                color_scheme: crate::css::parse::ColorScheme::Dark,
                contrast: crate::css::parse::Contrast::More,
            }
        );
        // And a name that merely *contains* the word is not a suffix match.
        assert_eq!(env("Darkroom"), MediaEnv::default());
        assert_eq!(env("dark"), MediaEnv::default(), "the affix needs a stem");
        assert_eq!(env("-dark"), MediaEnv::default(), "so does the hyphen form");
    }

    #[test]
    fn a_high_contrast_gtk_theme_evaluates_prefers_contrast_more() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(
            &tmp.path().join(".themes/HighContrast/gtk-4.0/gtk.css"),
            "button { min-width: 11px }\n\
             @media (prefers-contrast: more) { button { min-width: 79px } }\n",
        );
        let env = ThemeEnv {
            gtk_theme: Some("HighContrast".to_string()),
            home: Some(tmp.path().display().to_string()),
            ..ThemeEnv::default()
        };
        assert_eq!(button_min_width(&compile_for_theme_env(&env)), 79.0);
    }

    #[test]
    fn the_media_env_is_derived_from_the_gtk_theme_spelling() {
        let env = |spec: &str| {
            ThemeEnv {
                gtk_theme: Some(spec.to_string()),
                ..ThemeEnv::default()
            }
            .media_env()
        };
        assert_eq!(ThemeEnv::default().media_env(), MediaEnv::default());
        assert_eq!(env("Adwaita"), MediaEnv::default());
        assert_eq!(
            env("Adwaita:dark").color_scheme,
            crate::css::parse::ColorScheme::Dark
        );
        assert_eq!(
            env("Adwaita:DARK").color_scheme,
            crate::css::parse::ColorScheme::Dark,
            "GTK matches the variant case-insensitively"
        );
        assert_eq!(
            env("HighContrast").contrast,
            crate::css::parse::Contrast::More
        );
        assert_eq!(
            env("HighContrastInverse").color_scheme,
            crate::css::parse::ColorScheme::Dark,
            "the inverse high-contrast theme is the dark one"
        );
        assert_eq!(
            env("HighContrastInverse").contrast,
            crate::css::parse::Contrast::More
        );
        assert_eq!(env("Foo:hc").contrast, crate::css::parse::Contrast::More);
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
        assert_eq!(button_min_height(&sheet), 13.0);
        let border = button_style(&sheet).border_colors()[0].to_color32();
        // M2's initial `border-color` is `currentColor`, which resolves to the
        // initial `color` -- opaque black. (M1 had no initial and defaulted to
        // transparent.) What the test is really pinning is that Adwaita's
        // #cdc7c2 never got layered underneath.
        assert_eq!(border, Color(0xFF00_0000));
        assert_ne!(
            border,
            Color(0xFFCD_C7C2),
            "an explicit file must not be layered onto Adwaita"
        );
    }

    #[test]
    fn the_bundled_source_is_exactly_the_vendored_sheet() {
        let sheet = compile_theme(&ThemeSource::Bundled);
        assert_eq!(
            button_style(&sheet).border_colors()[0].to_color32(),
            Color(0xFFCD_C7C2)
        );
        assert_eq!(sheet.rules.len(), 900);
    }
    #[test]
    fn a_dark_or_high_contrast_theme_falls_back_to_the_matching_bundled_sheet() {
        // F1. `media_env()` derives `ColorScheme::Dark` from `$GTK_THEME`,
        // but the loader only ever fell back to the *light* bundled sheet.
        // GTK4 carries Adwaita internally, so on a stock system every
        // `base_theme_candidates` entry misses and `GTK_THEME=Adwaita:dark`
        // rendered `#f6f5f4` backgrounds under a dark media environment.
        //
        // Mutation check: return BUNDLED_ADWAITA_LIGHT unconditionally from
        // `bundled_sheet_for` and the dark and hc assertions fail. (Compared
        // by content, not by pointer: these are `const` items, so each use
        // site may get its own copy of the literal.)
        use super::{BUNDLED_ADWAITA_DARK, BUNDLED_ADWAITA_HC, bundled_sheet_for};
        use crate::BUNDLED_ADWAITA_LIGHT;
        use crate::css::parse::{ColorScheme, Contrast};

        let dark = MediaEnv {
            color_scheme: ColorScheme::Dark,
            contrast: Contrast::NoPreference,
        };
        assert_eq!(bundled_sheet_for(&dark), BUNDLED_ADWAITA_DARK);

        let hc = MediaEnv {
            color_scheme: ColorScheme::Light,
            contrast: Contrast::More,
        };
        assert_eq!(bundled_sheet_for(&hc), BUNDLED_ADWAITA_HC);

        assert_eq!(
            bundled_sheet_for(&MediaEnv::default()),
            BUNDLED_ADWAITA_LIGHT
        );
    }

    #[test]
    fn a_dark_gtk_theme_with_nothing_on_disk_compiles_dark_colours() {
        // The end-to-end half of F1: no theme directory exists at all, so
        // the loader takes the bundled fallback, and the compiled button's
        // background must be a dark colour rather than Adwaita light's
        // #f6f5f4 family.
        let home = tempfile::tempdir().expect("tempdir");
        let env = ThemeEnv {
            gtk_theme: Some("Adwaita:dark".to_string()),
            xdg_config_home: Some(home.path().join("config").display().to_string()),
            xdg_data_home: Some(home.path().join("data").display().to_string()),
            home: Some(home.path().display().to_string()),
        };
        let sheet = compile_for_theme_env(&env);
        let color = button_style(&sheet).color();
        // Adwaita dark's foreground is near-white; light's is near-black.
        assert!(
            color.r > 0.7 && color.g > 0.7 && color.b > 0.7,
            "expected a dark theme's light foreground, got {color:?}"
        );
    }
}
