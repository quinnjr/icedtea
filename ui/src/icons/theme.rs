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

/// How a subdirectory of an icon theme relates requested sizes to the icons
/// it holds: the spec's `Type` key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirKind {
    /// `Type=Fixed`: a match only at exactly `Size`.
    Fixed {
        /// The directory's declared `Size`.
        size: u32,
    },
    /// `Type=Scalable`: a match anywhere in `[MinSize, MaxSize]`.
    Scalable {
        /// `MinSize`, defaulting to `Size`.
        min: u32,
        /// `MaxSize`, defaulting to `Size`.
        max: u32,
    },
    /// `Type=Threshold` (the spec's default): a match within
    /// `Size ± Threshold`.
    Threshold {
        /// The directory's declared `Size`.
        size: u32,
        /// `Threshold`, defaulting to 2.
        threshold: u32,
    },
}

/// `a * b` in `u64`, so a hostile `Size=4294967295` in an `index.theme`
/// cannot overflow the arithmetic below.
const fn mul(a: u32, b: u32) -> u64 {
    (a as u64) * (b as u64)
}

/// `|a - b|`, saturated into a `u32`.
const fn diff(a: u64, b: u64) -> u32 {
    let d = a.abs_diff(b);
    if d > u32::MAX as u64 {
        u32::MAX
    } else {
        d as u32
    }
}

impl DirKind {
    /// The spec's `DirectoryMatchesSize`.
    ///
    /// `dir_scale` is the subdirectory's own `Scale` key; a directory only
    /// ever matches a request at its own scale, which is why a `scale=2`
    /// lookup in a theme with no `Scale=2` directories (Adwaita, as
    /// installed) matches nothing and falls to [`distance`](Self::distance).
    #[must_use]
    pub const fn matches(self, dir_scale: u32, size: u32, scale: u32) -> bool {
        if dir_scale != scale {
            return false;
        }
        match self {
            DirKind::Fixed { size: fixed } => fixed == size,
            DirKind::Scalable { min, max } => min <= size && size <= max,
            DirKind::Threshold {
                size: nominal,
                threshold,
            } => {
                nominal.saturating_sub(threshold) <= size
                    && size <= nominal.saturating_add(threshold)
            }
        }
    }

    /// The spec's `DirectorySizeDistance`, in device pixels.
    ///
    /// Zero while the request is inside the directory's range: the caller
    /// only reaches this when [`matches`](Self::matches) already failed, so a
    /// zero here means "right size, wrong scale" and still competes.
    #[must_use]
    pub const fn distance(self, dir_scale: u32, size: u32, scale: u32) -> u32 {
        let want = mul(size, scale);
        match self {
            DirKind::Fixed { size: fixed } => diff(mul(fixed, dir_scale), want),
            DirKind::Scalable { min, max } => {
                let lo = mul(min, dir_scale);
                let hi = mul(max, dir_scale);
                if want < lo {
                    diff(lo, want)
                } else if want > hi {
                    diff(want, hi)
                } else {
                    0
                }
            }
            DirKind::Threshold {
                size: nominal,
                threshold,
            } => {
                let lo = mul(nominal.saturating_sub(threshold), dir_scale);
                let hi = mul(nominal.saturating_add(threshold), dir_scale);
                if want < lo {
                    diff(lo, want)
                } else if want > hi {
                    diff(want, hi)
                } else {
                    0
                }
            }
        }
    }
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

#[cfg(test)]
mod size_tests {
    use super::DirKind;

    // The spec's `DirectoryMatchesSize` for `Type=Fixed`: a match at exactly
    // `Size`, and only at the directory's own `Scale`.
    // Mutation check: relax the scale equality to `true` and the third
    // assertion passes when it must not.
    #[test]
    fn a_fixed_directory_matches_only_its_exact_size_and_scale() {
        let dir = DirKind::Fixed { size: 16 };
        assert!(dir.matches(1, 16, 1));
        assert!(!dir.matches(1, 17, 1));
        assert!(!dir.matches(1, 16, 2));
        assert!(dir.matches(2, 16, 2));
    }

    // `Type=Scalable` matches anywhere in `[MinSize, MaxSize]`, at its scale.
    // Mutation check: make the bounds exclusive (`min < size`) and the
    // `matches(1, 8, 1)` boundary assertion fails.
    #[test]
    fn a_scalable_directory_matches_its_whole_range_inclusive() {
        let dir = DirKind::Scalable { min: 8, max: 512 };
        assert!(dir.matches(1, 8, 1));
        assert!(dir.matches(1, 128, 1));
        assert!(dir.matches(1, 512, 1));
        assert!(!dir.matches(1, 7, 1));
        assert!(!dir.matches(1, 513, 1));
    }

    // `Type=Threshold` matches `Size ± Threshold`.
    // Mutation check: drop the `+ threshold` half and `matches(1, 26, 1)`
    // fails.
    #[test]
    fn a_threshold_directory_matches_size_plus_or_minus_threshold() {
        let dir = DirKind::Threshold {
            size: 24,
            threshold: 2,
        };
        assert!(dir.matches(1, 22, 1));
        assert!(dir.matches(1, 24, 1));
        assert!(dir.matches(1, 26, 1));
        assert!(!dir.matches(1, 21, 1));
        assert!(!dir.matches(1, 27, 1));
    }

    // `DirectorySizeDistance`, which is what picks the *closest* directory
    // once no directory matched exactly. Distances are in device pixels:
    // `size * scale` against the directory's `Size * Scale`.
    // Mutation check: compare `size` against `Size` without multiplying
    // either by its scale and the `(2, 16, 1)` assertion returns 0.
    #[test]
    fn size_distance_is_measured_in_device_pixels() {
        assert_eq!(DirKind::Fixed { size: 16 }.distance(1, 32, 1), 16);
        assert_eq!(DirKind::Fixed { size: 16 }.distance(1, 8, 1), 8);
        assert_eq!(DirKind::Fixed { size: 16 }.distance(2, 16, 1), 16);
        assert_eq!(
            DirKind::Scalable { min: 8, max: 512 }.distance(1, 600, 1),
            88
        );
        assert_eq!(DirKind::Scalable { min: 8, max: 512 }.distance(1, 4, 1), 4);
        assert_eq!(
            DirKind::Scalable { min: 8, max: 512 }.distance(1, 128, 1),
            0
        );
        assert_eq!(
            DirKind::Threshold {
                size: 24,
                threshold: 2
            }
            .distance(1, 30, 1),
            4
        );
        assert_eq!(
            DirKind::Threshold {
                size: 24,
                threshold: 2
            }
            .distance(1, 10, 1),
            12
        );
    }

    // `index.theme` is untrusted: `Size=4294967295` is a legal-looking line.
    // Mutation check: do the arithmetic in `u32` instead of `u64` and this
    // panics with "attempt to multiply with overflow" in a debug build.
    #[test]
    fn hostile_sizes_saturate_instead_of_overflowing() {
        let huge = DirKind::Fixed { size: u32::MAX };
        assert_eq!(huge.distance(u32::MAX, 1, 1), u32::MAX);
        // Reconciliation (Task 1): the plan's own assertion here negated
        // this call, but `Fixed { size: u32::MAX }.matches(u32::MAX,
        // u32::MAX, u32::MAX)` is an exact size-and-scale match by
        // `DirectoryMatchesSize`'s own rule (dir_scale == scale, and
        // fixed == size) — the correct read of a hostile-but-consistent
        // input is "matches", not "doesn't". Corrected to match the
        // (unmodified) implementation above, which is the spec-exact one.
        assert!(huge.matches(u32::MAX, u32::MAX, u32::MAX));
        let scalable = DirKind::Scalable {
            min: u32::MAX,
            max: 0,
        };
        assert_eq!(scalable.distance(u32::MAX, u32::MAX, u32::MAX), u32::MAX);
        assert!(!scalable.matches(1, u32::MAX, 1));
        let threshold = DirKind::Threshold {
            size: 0,
            threshold: u32::MAX,
        };
        // Reconciliation (Task 1): the plan's assertion here expected `0`,
        // but `want = size * scale = u32::MAX * u32::MAX` (computed in u64,
        // no overflow) vastly exceeds `hi = nominal.saturating_add(threshold)
        // = u32::MAX`, so the correctly-saturated distance is `u32::MAX`,
        // consistent with the `huge`/`scalable` cases just above — not `0`.
        assert_eq!(threshold.distance(1, u32::MAX, u32::MAX), u32::MAX);
        assert!(threshold.matches(1, 0, 1));
    }
}
