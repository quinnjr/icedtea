//! The icon theme handle: which theme, where its roots are, and what it
//! inherits from.

use std::path::PathBuf;
use std::rc::Rc;

/// The theme GTK falls back to when `settings.ini` names none.
const DEFAULT_THEME: &str = "Adwaita";

/// The theme every chain ends at, per the Icon Theme Specification.
const FALLBACK_THEME: &str = "hicolor";

/// A resolved icon theme: its name, its search roots and its inheritance
/// chain.
///
/// P7 grows this with `lookup`, `render` and their caches; P4 needs only
/// enough for `BuildCx`/`EventCx` to carry one and for tests to build a
/// hermetic instance with [`IconTheme::with_name_and_roots`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconTheme {
    name: Rc<str>,
    roots: Vec<PathBuf>,
    chain: Vec<Rc<str>>,
}

impl IconTheme {
    /// Read the theme name from `$XDG_CONFIG_HOME/gtk-4.0/settings.ini`
    /// (falling back to `$HOME/.config/...`), defaulting to `Adwaita`, and
    /// build the standard root list: `$XDG_DATA_HOME/icons`, `$HOME/.icons`,
    /// each `$XDG_DATA_DIRS/icons`, then `/usr/share/pixmaps` last.
    ///
    /// A missing or unreadable `settings.ini` is not an error — it is the
    /// normal case on a machine with no GTK configuration.
    #[must_use]
    pub fn from_env() -> Self {
        let home = std::env::var("HOME").ok();
        let config_home = std::env::var("XDG_CONFIG_HOME")
            .ok()
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|h| PathBuf::from(h).join(".config")));

        let name = config_home
            .as_ref()
            .map(|dir| dir.join("gtk-4.0").join("settings.ini"))
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|text| theme_name_from_settings_ini(&text))
            .unwrap_or_else(|| DEFAULT_THEME.to_owned());

        let mut roots: Vec<PathBuf> = Vec::new();
        if let Some(data_home) = std::env::var("XDG_DATA_HOME")
            .ok()
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|h| PathBuf::from(h).join(".local/share")))
        {
            roots.push(data_home.join("icons"));
        }
        if let Some(home) = home.as_ref() {
            roots.push(PathBuf::from(home).join(".icons"));
        }
        let data_dirs = std::env::var("XDG_DATA_DIRS")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "/usr/local/share:/usr/share".to_owned());
        for dir in data_dirs.split(':').filter(|s| !s.is_empty()) {
            roots.push(PathBuf::from(dir).join("icons"));
        }
        // Flat, non-themed, last resort only.
        roots.push(PathBuf::from("/usr/share/pixmaps"));

        IconTheme::with_name_and_roots(&name, roots)
    }

    /// Hermetic construction: no environment is read at all.
    #[must_use]
    pub fn with_name_and_roots(name: &str, roots: Vec<PathBuf>) -> Self {
        let trimmed = name.trim();
        let name: Rc<str> = if trimmed.is_empty() {
            Rc::from(DEFAULT_THEME)
        } else {
            Rc::from(trimmed)
        };
        // P7 replaces this with the real `Inherits=` walk over each theme's
        // `index.theme`; the invariant it must preserve is the one pinned
        // here: the chain starts at `name` and ends at `hicolor`, once.
        let mut chain = vec![Rc::clone(&name)];
        if &*name != FALLBACK_THEME {
            chain.push(Rc::from(FALLBACK_THEME));
        }
        IconTheme { name, roots, chain }
    }

    /// The theme's own name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The resolved inheritance chain: this theme first, `hicolor` last.
    #[must_use]
    pub fn chain(&self) -> &[Rc<str>] {
        &self.chain
    }

    /// The search roots, in priority order.
    #[must_use]
    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    /// Drop every memoized lookup and render.
    ///
    /// P4 holds no caches yet, so this is a no-op that P7 fills in; it exists
    /// now because `App` calls it when the theme changes.
    pub fn clear_caches(&mut self) {}
}

/// `gtk-icon-theme-name` from a GTK `settings.ini`, or `None`.
///
/// A hand-edited, truncated or non-UTF-8-intentioned file is untrusted
/// input: every malformed shape yields `None` and none of them panics.
/// Only the `[Settings]` section is consulted, keys are matched
/// ASCII-case-insensitively, and both key and value are trimmed.
#[must_use]
pub fn theme_name_from_settings_ini(text: &str) -> Option<String> {
    let mut in_settings = false;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(rest) = line.strip_prefix('[') {
            // An unterminated section header is malformed; treat it as
            // "not the Settings section" rather than guessing.
            in_settings = rest
                .strip_suffix(']')
                .is_some_and(|name| name.trim().eq_ignore_ascii_case("Settings"));
            continue;
        }
        if !in_settings {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if !key
            .trim()
            .trim_start_matches('\u{feff}')
            .eq_ignore_ascii_case("gtk-icon-theme-name")
        {
            continue;
        }
        let value = value.trim().trim_matches('\u{0}').trim();
        if value.is_empty() {
            continue;
        }
        return Some(value.to_owned());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hermetic_theme_appends_hicolor_to_its_chain() {
        let theme = IconTheme::with_name_and_roots("Adwaita", vec![]);
        assert_eq!(theme.name(), "Adwaita");
        assert_eq!(
            theme.chain().iter().map(|s| &**s).collect::<Vec<_>>(),
            vec!["Adwaita", "hicolor"]
        );
        assert!(theme.roots().is_empty());
    }

    #[test]
    fn hicolor_is_never_appended_twice() {
        let theme = IconTheme::with_name_and_roots("hicolor", vec![]);
        assert_eq!(
            theme.chain().iter().map(|s| &**s).collect::<Vec<_>>(),
            vec!["hicolor"]
        );
    }

    #[test]
    fn a_blank_theme_name_falls_back_to_adwaita() {
        let theme = IconTheme::with_name_and_roots("   ", vec![]);
        assert_eq!(theme.name(), "Adwaita");
    }

    #[test]
    fn settings_ini_is_read_only_from_the_settings_section() {
        let ini = "[Other]\ngtk-icon-theme-name=Wrong\n\
                   [Settings]\ngtk-theme-name = Adwaita\n\
                   gtk-icon-theme-name = Papirus \n";
        assert_eq!(
            theme_name_from_settings_ini(ini).as_deref(),
            Some("Papirus")
        );
    }

    #[test]
    fn a_malformed_settings_ini_never_panics_and_yields_nothing() {
        // Untrusted input (contract "cross-cutting rules"): every one of
        // these is a real shape a hand-edited settings.ini can take.
        for text in [
            "",
            "\u{0}\u{1}\u{2}",
            "[Settings",
            "[Settings]\n=",
            "[Settings]\ngtk-icon-theme-name",
            "[Settings]\ngtk-icon-theme-name=",
            "[Settings]\ngtk-icon-theme-name=   ",
            "[Settings]\n=Papirus",
            "gtk-icon-theme-name=NoSection",
            "[Settings]\n\u{feff}gtk-icon-theme-name=Ok\u{0}",
            "[Settings]\ngtk-icon-theme-name=日本語のテーマ",
            &"[".repeat(10_000),
            &format!("[Settings]\ngtk-icon-theme-name={}", "x".repeat(100_000)),
        ] {
            let got = theme_name_from_settings_ini(text);
            match text {
                t if t.contains("=Ok") => assert_eq!(got.as_deref(), Some("Ok")),
                t if t.contains("日本語") => assert_eq!(got.as_deref(), Some("日本語のテーマ")),
                t if t.contains(&"x".repeat(64)) => assert_eq!(got.map(|s| s.len()), Some(100_000)),
                _ => assert_eq!(got, None, "unexpected name from {text:?}"),
            }
        }
    }

    #[test]
    fn clear_caches_keeps_the_identity_of_the_theme() {
        let mut theme =
            IconTheme::with_name_and_roots("Papirus", vec![std::path::PathBuf::from("/tmp/icons")]);
        theme.clear_caches();
        assert_eq!(theme.name(), "Papirus");
        assert_eq!(theme.roots().len(), 1);
    }
}
