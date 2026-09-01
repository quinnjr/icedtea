//! The icon theme handle: which theme, where its roots are, and what it
//! inherits from.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use super::IconFormat;

/// The theme GTK falls back to when `settings.ini` names none.
const DEFAULT_THEME: &str = "Adwaita";

/// The spec-mandated backstop theme, where every chain terminates.
pub(crate) const HICOLOR: &str = "hicolor";

/// The most themes one chain may contain.
///
/// Adwaita's is three (`Adwaita`, `AdwaitaLegacy`, `hicolor`). The cap exists
/// because `Inherits=` is untrusted and a chain is walked on every miss.
pub(crate) const MAX_CHAIN: usize = 32;

/// The largest `Scale` a surface is ever asked for.
const MAX_SCALE: u32 = 4;

/// The most `(name, size, scale, symbolic)` lookups remembered at once.
pub(crate) const MAX_LOOKUP_CACHE: usize = 1024;

/// A lookup's cache key.
///
/// `chain` is the hash of the resolved inheritance chain. It is constant for
/// the lifetime of one `IconTheme` today, and is in the key anyway because
/// the contract puts it there and because the day a `set_theme` lands, a key
/// without it silently serves the previous theme's paths.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct LookupKey {
    chain: u64,
    name: Rc<str>,
    size: u32,
    scale: u32,
    symbolic: bool,
}

/// `true` if `name` is a usable theme directory name: non-empty, no path
/// separators, no `.`/`..`, no NUL.
///
/// The theme name comes from `settings.ini`, which is a file the user (or
/// anything running as them) writes. Joining `../../etc` onto a root would
/// walk out of it.
fn is_safe_theme_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains('\0')
}

/// One icon theme, its inheritance chain, its search roots and its caches.
///
/// Construct once per application and keep it: the caches are what make an
/// icon lookup cost a hash probe rather than a directory walk on every frame.
pub struct IconTheme {
    name: Rc<str>,
    roots: Vec<PathBuf>,
    // Read only by `pixmap_dirs` below, which is itself unused until Task 6's
    // lookup consumes the flat fallback — allowed here rather than deferred,
    // since the plan places the field in this task.
    #[allow(dead_code)]
    pixmaps: Vec<PathBuf>,
    chain: Vec<Rc<str>>,
    chain_hash: u64,
    indexes: HashMap<Rc<str>, ThemeIndex>,
    scale: u32,
    lookups: HashMap<LookupKey, Option<IconFile>>,
}

impl IconTheme {
    /// Hermetic construction for tests: no environment reads at all, and no
    /// flat `/usr/share/pixmaps` fallback (see
    /// [`with_name_roots_and_pixmaps`](Self::with_name_roots_and_pixmaps)).
    #[must_use]
    pub fn with_name_and_roots(name: &str, roots: Vec<PathBuf>) -> IconTheme {
        Self::with_name_roots_and_pixmaps(name, roots, Vec::new())
    }

    /// As [`with_name_and_roots`](Self::with_name_and_roots), plus the flat,
    /// non-themed directories searched as a last resort.
    ///
    /// Split out because a flat directory is not a themed root — it has no
    /// `index.theme` and no subdirectories — and because a hermetic test must
    /// be able to exercise the fallback without reading `/usr`.
    #[must_use]
    pub fn with_name_roots_and_pixmaps(
        name: &str,
        roots: Vec<PathBuf>,
        pixmaps: Vec<PathBuf>,
    ) -> IconTheme {
        let name: Rc<str> = if is_safe_theme_name(name) {
            Rc::from(name)
        } else {
            if !name.is_empty() {
                tracing::debug!(
                    theme = name,
                    "unusable icon theme name; falling back to hicolor"
                );
            }
            Rc::from(HICOLOR)
        };
        let mut theme = IconTheme {
            name,
            roots,
            pixmaps,
            chain: Vec::new(),
            chain_hash: 0,
            indexes: HashMap::new(),
            scale: 1,
            lookups: HashMap::new(),
        };
        theme.resolve_chain();
        theme
    }

    /// The active theme's directory name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The resolved inheritance chain, base theme first, `hicolor` last.
    ///
    /// `hicolor` is appended implicitly when the base theme neither is it nor
    /// names it, which is the convention every shipped theme relies on.
    /// A theme with no `index.theme` on disk still appears here — the chain
    /// is what was *declared*, and lookup simply finds nothing in it.
    #[must_use]
    pub fn chain(&self) -> &[Rc<str>] {
        &self.chain
    }

    /// The output scale icons are rasterised for, `1..=4`.
    ///
    /// Carried here rather than on `PaintCx` because contract §6 freezes
    /// `PaintCx`'s only new field as `icons` (deviation 3). The window sets
    /// it from `wl_surface.preferred_buffer_scale`.
    #[must_use]
    pub fn scale(&self) -> u32 {
        self.scale
    }

    /// Set the output scale. Zero and absurd values clamp into `1..=4`
    /// rather than producing a zero-pixel or gigapixel surface.
    pub fn set_scale(&mut self, scale: u32) {
        let clamped = scale.clamp(1, MAX_SCALE);
        if clamped != self.scale {
            self.scale = clamped;
            self.clear_caches();
        }
    }

    /// The themed roots, in search order.
    ///
    /// Unused outside tests until Task 6's lookup consumes it — allowed here
    /// rather than deferred, since the plan places the accessor in this task.
    #[allow(dead_code)]
    pub(crate) fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    /// The flat, non-themed last-resort directories.
    #[allow(dead_code)]
    pub(crate) fn pixmap_dirs(&self) -> &[PathBuf] {
        &self.pixmaps
    }

    /// The parsed `index.theme` for one theme in the chain, if it has one.
    #[allow(dead_code)]
    pub(crate) fn index(&self, theme: &str) -> Option<&ThemeIndex> {
        self.indexes.get(theme)
    }

    /// Read `<root>/<theme>/index.theme` from the first root that has one.
    fn read_index(&self, theme: &str) -> Option<ThemeIndex> {
        for root in &self.roots {
            let path = root.join(theme).join("index.theme");
            let Ok(text) = std::fs::read(&path) else {
                continue;
            };
            // An index.theme need not be valid UTF-8; a lossy read keeps the
            // ASCII keys intact, which is all the parser looks at.
            let text = String::from_utf8_lossy(&text);
            return Some(ThemeIndex::parse(theme, &text));
        }
        None
    }

    /// Walk `Inherits=` breadth-first into [`chain`](Self::chain), stopping
    /// at [`MAX_CHAIN`], never revisiting a theme, and appending `hicolor`.
    fn resolve_chain(&mut self) {
        let mut chain: Vec<Rc<str>> = Vec::new();
        let mut queue: Vec<Rc<str>> = vec![Rc::clone(&self.name)];
        while let Some(theme) = queue.first().cloned() {
            queue.remove(0);
            if chain.len() >= MAX_CHAIN {
                tracing::debug!(
                    theme = &*self.name,
                    "icon theme inheritance chain hit its cap; the tail is ignored"
                );
                break;
            }
            if chain.iter().any(|seen| **seen == *theme) {
                continue;
            }
            chain.push(Rc::clone(&theme));
            if let Some(index) = self.read_index(&theme) {
                for parent in &index.inherits {
                    if is_safe_theme_name(parent) {
                        queue.push(Rc::clone(parent));
                    }
                }
                self.indexes.insert(Rc::clone(&theme), index);
            }
        }
        if !chain.iter().any(|theme| &**theme == HICOLOR) && chain.len() < MAX_CHAIN {
            let hicolor: Rc<str> = Rc::from(HICOLOR);
            if let Some(index) = self.read_index(HICOLOR) {
                self.indexes.insert(Rc::clone(&hicolor), index);
            }
            chain.push(hicolor);
        }
        self.chain = chain;
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for theme in &self.chain {
            std::hash::Hash::hash(&**theme, &mut hasher);
        }
        self.chain_hash = std::hash::Hasher::finish(&hasher);
    }

    /// The spec's `FindIcon`: per theme in the chain, try each `Directories`
    /// (+ `ScaledDirectories`) subdir for `name.{png,svg,xpm}` where
    /// `DirectoryMatchesSize(subdir, size, scale)`; if none matched, take that
    /// theme's minimum `DirectorySizeDistance` candidate **before** moving to
    /// the next theme; finally `/usr/share/pixmaps`.
    /// `symbolic` prefers `<name>-symbolic` and falls back to `<name>`;
    /// a total miss falls back to `image-missing`, and only then to `None`.
    ///
    /// Memoised on `(chain, name, size, scale, symbolic)`, misses included:
    /// an application that asks for an icon it does not have every frame must
    /// not walk every directory of every theme every frame. The table is
    /// capped at [`MAX_LOOKUP_CACHE`] and emptied by
    /// [`clear_caches`](Self::clear_caches) — which is also how a theme
    /// edited on disk is picked up.
    pub fn lookup(
        &mut self,
        name: &str,
        size: u32,
        scale: u32,
        symbolic: bool,
    ) -> Option<IconFile> {
        let key = LookupKey {
            chain: self.chain_hash,
            name: Rc::from(name),
            size,
            scale,
            symbolic,
        };
        if let Some(found) = self.lookups.get(&key) {
            return found.clone();
        }
        let found = super::lookup::lookup_uncached(self, name, size, scale, symbolic);
        if self.lookups.len() >= MAX_LOOKUP_CACHE {
            // Whole-table eviction rather than an LRU: an icon set that
            // overflows this is one that changed wholesale (a file manager
            // scrolled to new MIME types), and the cost of rebuilding it is
            // one directory walk per icon actually on screen.
            self.lookups.clear();
        }
        self.lookups.insert(key, found.clone());
        found
    }

    /// Empty every memo table.
    ///
    /// The chain and the parsed indexes are *not* caches: they are the
    /// theme's identity, and re-reading them means constructing a new
    /// `IconTheme`.
    pub fn clear_caches(&mut self) {
        self.lookups.clear();
    }

    /// How many lookups are remembered. Test observability only; nothing
    /// outside this module's tests reads it.
    #[cfg(test)]
    pub(crate) fn lookup_cache_len(&self) -> usize {
        self.lookups.len()
    }

    /// `<root>/<theme>/<subdir>/<name>.<ext>` for every root, in order.
    ///
    /// Unused until Task 6's lookup consumes it — allowed here rather than
    /// deferred, since the plan places the accessor in this task.
    #[allow(dead_code)]
    pub(crate) fn candidate_paths(&self, theme: &str, subdir: &str, file: &str) -> Vec<PathBuf> {
        self.roots
            .iter()
            .map(|root| Path::new(root).join(theme).join(subdir).join(file))
            .collect()
    }
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

/// The most subdirectories one theme may declare.
///
/// Adwaita, the largest theme in common use, declares 33. The cap exists
/// because `index.theme` is untrusted: a file that names a million
/// directories must cost a bounded amount of work, not a bounded amount of
/// patience.
pub(crate) const MAX_SUBDIRS: usize = 512;

/// The most parent themes one `Inherits=` line may name.
pub(crate) const MAX_INHERITS: usize = 32;

/// The spec's default `Threshold`.
const DEFAULT_THRESHOLD: u32 = 2;

/// One subdirectory of an icon theme, as `index.theme` declares it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubDir {
    /// The path relative to the theme's own directory, e.g. `16x16/actions`.
    pub path: Rc<str>,
    /// The declared `Size`.
    pub size: u32,
    /// The declared `Scale`, defaulting to 1.
    pub scale: u32,
    /// `Type` plus the keys that type reads.
    pub kind: DirKind,
}

/// One theme's parsed `index.theme`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeIndex {
    /// The theme's directory name — **not** its localised `Name=` unless the
    /// file supplies one; lookup matches themes by directory name.
    pub name: Rc<str>,
    /// `Inherits=`, in order, with empty entries dropped.
    pub inherits: Vec<Rc<str>>,
    /// `Directories=` then `ScaledDirectories=`, in declaration order, with
    /// entries that have no group of their own dropped.
    pub dirs: Vec<SubDir>,
}

/// One `key=value` line, split on the *first* `=` and trimmed.
///
/// Localised keys (`Name[de]=`) keep their brackets here and are matched
/// exactly by the caller, so `Name[de]` never overwrites `Name`.
fn split_key_value(line: &str) -> Option<(&str, &str)> {
    let (key, value) = line.split_once('=')?;
    Some((key.trim(), value.trim()))
}

/// A `,`-separated list, trimmed, with empty entries dropped and at most
/// `cap` entries kept.
fn comma_list(value: &str, cap: usize) -> Vec<Rc<str>> {
    value
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .take(cap)
        .map(Rc::from)
        .collect()
}

/// A `u32` key, or `default` when the value is absent, empty, negative,
/// non-numeric or larger than a `u32`.
fn u32_key(groups: &[(String, Vec<(String, String)>)], group: &str, key: &str) -> Option<u32> {
    let entries = &groups.iter().find(|(name, _)| name == group)?.1;
    let value = entries
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())?;
    value.parse::<u32>().ok()
}

impl ThemeIndex {
    /// Parse an `index.theme`.
    ///
    /// Never fails and never panics: this is a file on disk that anything at
    /// all may have written. A line that does not parse is skipped, a key
    /// that does not parse takes the spec's default, and the result is
    /// bounded by [`MAX_SUBDIRS`]/[`MAX_INHERITS`]. `name` is the theme's
    /// directory name, used when the file declares no `Name=`.
    #[must_use]
    pub fn parse(name: &str, text: &str) -> ThemeIndex {
        // Group name -> ordered key/value pairs. A duplicate key keeps the
        // first, as the desktop-entry spec says.
        let mut groups: Vec<(String, Vec<(String, String)>)> = Vec::new();
        let mut current: Option<usize> = None;

        for raw in text.lines() {
            // `\r` survives `lines()` on a CRLF file read as bytes, and a
            // lone `\r` (classic-Mac line endings, or a truncated write)
            // never splits at all -- so trim it off both ends.
            let line = raw.trim_matches(|c: char| c == '\r' || c == '\u{0}').trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(rest) = line.strip_prefix('[') {
                let Some(group) = rest.strip_suffix(']') else {
                    // An unterminated group header is not a group.
                    continue;
                };
                let group = group.trim();
                if group.is_empty() {
                    continue;
                }
                current = Some(match groups.iter().position(|(n, _)| n == group) {
                    Some(existing) => existing,
                    None => {
                        groups.push((group.to_owned(), Vec::new()));
                        groups.len() - 1
                    }
                });
                continue;
            }
            let Some(index) = current else {
                // A key before any group header belongs to nothing.
                continue;
            };
            let Some((key, value)) = split_key_value(line) else {
                continue;
            };
            if key.is_empty() {
                continue;
            }
            let entries = &mut groups[index].1;
            if !entries.iter().any(|(k, _)| k == key) {
                entries.push((key.to_owned(), value.to_owned()));
            }
        }

        let header = groups
            .iter()
            .find(|(group, _)| group == "Icon Theme")
            .map(|(_, entries)| entries.as_slice())
            .unwrap_or(&[]);
        let get = |key: &str| {
            header
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.as_str())
        };

        let display_name: Rc<str> = get("Name")
            .filter(|value| !value.is_empty())
            .map_or_else(|| Rc::from(name), Rc::from);
        let inherits = get("Inherits").map_or_else(Vec::new, |v| comma_list(v, MAX_INHERITS));

        let mut declared = get("Directories").map_or_else(Vec::new, |v| comma_list(v, MAX_SUBDIRS));
        for scaled in get("ScaledDirectories").map_or_else(Vec::new, |v| comma_list(v, MAX_SUBDIRS))
        {
            if declared.len() >= MAX_SUBDIRS {
                break;
            }
            if !declared.contains(&scaled) {
                declared.push(scaled);
            }
        }

        let mut dirs = Vec::with_capacity(declared.len());
        for path in declared {
            // A directory with no group of its own has no `Size`, and `Size`
            // is required: the spec has nothing to match it against.
            let Some(size) = u32_key(&groups, &path, "Size") else {
                continue;
            };
            let scale = u32_key(&groups, &path, "Scale")
                .filter(|scale| *scale > 0)
                .unwrap_or(1);
            let type_key = groups
                .iter()
                .find(|(group, _)| *group == *path)
                .and_then(|(_, entries)| entries.iter().find(|(k, _)| k == "Type"))
                .map(|(_, v)| v.as_str())
                .unwrap_or("Threshold");
            let kind = if type_key.eq_ignore_ascii_case("Fixed") {
                DirKind::Fixed { size }
            } else if type_key.eq_ignore_ascii_case("Scalable") {
                DirKind::Scalable {
                    min: u32_key(&groups, &path, "MinSize").unwrap_or(size),
                    max: u32_key(&groups, &path, "MaxSize").unwrap_or(size),
                }
            } else {
                DirKind::Threshold {
                    size,
                    threshold: u32_key(&groups, &path, "Threshold").unwrap_or(DEFAULT_THRESHOLD),
                }
            };
            dirs.push(SubDir {
                path,
                size,
                scale,
                kind,
            });
        }

        ThemeIndex {
            name: display_name,
            inherits,
            dirs,
        }
    }
}

/// The environment `IconTheme::from_env` reads, as data.
///
/// A struct rather than direct `std::env::var` calls so the resolution rules
/// are testable without mutating process-global state — `std::env::set_var`
/// is `unsafe` in edition 2024 and racy across the test harness's threads.
/// Same shape [`crate::app::ThemeEnv`] uses for the same reason.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IconEnv {
    /// `$XDG_DATA_HOME`.
    pub xdg_data_home: Option<String>,
    /// `$XDG_DATA_DIRS`, `:`-separated.
    pub xdg_data_dirs: Option<String>,
    /// `$HOME`.
    pub home: Option<String>,
    /// `$XDG_CONFIG_HOME`.
    pub xdg_config_home: Option<String>,
}

/// Read `name`, treating an empty value as unset — which is what
/// `XDG_DATA_DIRS=""` means.
fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

/// One `key=value` from one group of an INI file.
///
/// Never fails and never panics: `settings.ini` is a file on disk that
/// anything may have written. Values keep their quotes, as GTK's own
/// key-file reader does for an unquoted-string key.
#[must_use]
pub(crate) fn ini_value(text: &str, group: &str, key: &str) -> Option<String> {
    let mut in_group = false;
    for raw in text.lines() {
        let line = raw.trim_matches(|c: char| c == '\r' || c == '\u{0}').trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix('[') {
            let Some(name) = rest.strip_suffix(']') else {
                continue;
            };
            in_group = name.trim() == group;
            continue;
        }
        if !in_group {
            continue;
        }
        let Some((found, value)) = line.split_once('=') else {
            continue;
        };
        if found.trim() == key {
            return Some(value.trim().to_owned());
        }
    }
    None
}

impl IconEnv {
    /// Read the four variables from the process environment.
    #[must_use]
    pub fn from_env() -> IconEnv {
        IconEnv {
            xdg_data_home: env_var("XDG_DATA_HOME"),
            xdg_data_dirs: env_var("XDG_DATA_DIRS"),
            home: env_var("HOME"),
            xdg_config_home: env_var("XDG_CONFIG_HOME"),
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

    /// The themed roots, in the spec's search order:
    /// `$XDG_DATA_HOME/icons` (default `~/.local/share/icons`),
    /// `$HOME/.icons`, then each `$XDG_DATA_DIRS` entry + `/icons`
    /// (default `/usr/local/share:/usr/share`).
    #[must_use]
    pub fn roots(&self) -> Vec<PathBuf> {
        let mut roots = Vec::new();
        match (&self.xdg_data_home, &self.home) {
            (Some(data), _) => roots.push(Path::new(data).join("icons")),
            (None, Some(home)) => roots.push(Path::new(home).join(".local/share/icons")),
            (None, None) => {}
        }
        if let Some(home) = &self.home {
            roots.push(Path::new(home).join(".icons"));
        }
        let dirs = self
            .xdg_data_dirs
            .clone()
            .unwrap_or_else(|| "/usr/local/share:/usr/share".to_string());
        for dir in dirs.split(':').map(str::trim).filter(|d| !d.is_empty()) {
            roots.push(Path::new(dir).join("icons"));
        }
        roots
    }

    /// The flat, non-themed last resort.
    #[must_use]
    pub fn pixmaps(&self) -> Vec<PathBuf> {
        vec![PathBuf::from("/usr/share/pixmaps")]
    }

    /// `gtk-icon-theme-name` from `<config>/gtk-4.0/settings.ini`'s
    /// `[Settings]` group, or `Adwaita`.
    ///
    /// A value that is not a usable directory name is ignored rather than
    /// joined onto a root; [`IconTheme::with_name_roots_and_pixmaps`] would
    /// reject it anyway, and rejecting it here keeps the log message about
    /// the setting rather than about the theme.
    #[must_use]
    pub fn theme_name(&self) -> String {
        let Some(config) = self.config_dir() else {
            return DEFAULT_THEME.to_string();
        };
        let path = config.join("gtk-4.0").join("settings.ini");
        let Ok(bytes) = std::fs::read(&path) else {
            return DEFAULT_THEME.to_string();
        };
        let text = String::from_utf8_lossy(&bytes);
        match ini_value(&text, "Settings", "gtk-icon-theme-name") {
            Some(name) if is_safe_theme_name(&name) => name,
            Some(name) => {
                tracing::debug!(
                    setting = %name,
                    path = %path.display(),
                    "gtk-icon-theme-name is not a usable theme directory name; using the default"
                );
                DEFAULT_THEME.to_string()
            }
            None => DEFAULT_THEME.to_string(),
        }
    }
}

impl IconTheme {
    /// The theme this environment selects, with the spec's roots.
    #[must_use]
    pub fn from_icon_env(env: &IconEnv) -> IconTheme {
        Self::with_name_roots_and_pixmaps(&env.theme_name(), env.roots(), env.pixmaps())
    }

    /// `gtk-icon-theme-name` from `$XDG_CONFIG_HOME/gtk-4.0/settings.ini`
    /// (`[Settings]`), default `Adwaita`. Roots, in order:
    /// `$XDG_DATA_HOME/icons`, `$HOME/.icons`, each `$XDG_DATA_DIRS/icons`,
    /// then `/usr/share/pixmaps` (flat, non-themed, last resort only).
    #[must_use]
    pub fn from_env() -> IconTheme {
        Self::from_icon_env(&IconEnv::from_env())
    }
}

/// One located icon file, with the geometry the directory it came from
/// declared for it.
///
/// `nominal_size`/`scale`/`kind` are the *directory's* numbers, not the
/// request's: they are what tells a renderer whether an upscale is expected
/// (a Scalable SVG) or a compromise (a Fixed PNG asked for at the wrong
/// size).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconFile {
    /// Absolute path to the file.
    pub path: PathBuf,
    /// The format its extension names.
    pub format: IconFormat,
    /// The subdir's declared `Size`.
    pub nominal_size: u32,
    /// The subdir's declared `Scale`.
    pub scale: u32,
    /// `true` for a `-symbolic` name or a file under a `symbolic/` subdir.
    pub symbolic: bool,
    /// The subdir's `Type` and the keys it reads.
    pub kind: DirKind,
}

use crate::css::computed::ComputedStyle;
use crate::css::registry::Prop;
use crate::css::value::{ColorCtx, ColorTable, ColorValue, Rgba, Value};

/// The vendored Adwaita `@success_color` (`themes/adwaita-light.css:1920`).
const DEFAULT_SUCCESS: Rgba = Rgba {
    r: 0x33 as f32 / 255.0,
    g: 0xd1 as f32 / 255.0,
    b: 0x7a as f32 / 255.0,
    a: 1.0,
};
/// The vendored Adwaita `@warning_color` (`themes/adwaita-light.css:1918`).
const DEFAULT_WARNING: Rgba = Rgba {
    r: 0xf5 as f32 / 255.0,
    g: 0x79 as f32 / 255.0,
    b: 0x00 as f32 / 255.0,
    a: 1.0,
};
/// The vendored Adwaita `@error_color` (`themes/adwaita-light.css:1919`).
const DEFAULT_ERROR: Rgba = Rgba {
    r: 0xcc as f32 / 255.0,
    g: 0x00 as f32 / 255.0,
    b: 0x00 as f32 / 255.0,
    a: 1.0,
};

/// The four logical slots GTK passes positionally as
/// `[foreground, success, warning, error]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    /// `colors[0]`: the icon's foreground, i.e. the painting node's `color`.
    pub foreground: Rgba,
    /// `colors[1]`.
    pub success: Rgba,
    /// `colors[2]`.
    pub warning: Rgba,
    /// `colors[3]`.
    pub error: Rgba,
}

impl Palette {
    /// A palette with `foreground` and the theme's default status colours.
    ///
    /// The three defaults are the vendored Adwaita sheet's own
    /// `@success_color`/`@warning_color`/`@error_color`, so an icon rendered
    /// without a sheet in hand matches one rendered with it.
    #[must_use]
    pub const fn for_color(foreground: Rgba) -> Palette {
        Palette {
            foreground,
            success: DEFAULT_SUCCESS,
            warning: DEFAULT_WARNING,
            error: DEFAULT_ERROR,
        }
    }

    /// `foreground` = the painting node's computed `color`; the other three
    /// come from `-gtk-icon-palette` on that node, falling back to the
    /// theme's `@success_color`/`@warning_color`/`@error_color`.
    ///
    /// `-gtk-icon-palette`'s entries may still carry `@name`s and
    /// `currentColor` here: `computed`'s resolution of the whole value fails
    /// as a unit if *any* entry's `@name` is unknown (`computed.rs:901-907`),
    /// so a sheet with two of the three colours defined leaves the property
    /// unresolved. Resolving per entry here is what makes the two that *are*
    /// defined still count.
    ///
    /// A slot name GTK does not know (anything but `success`, `warning`,
    /// `error`) is ignored: the property is a list of overrides, not a
    /// replacement.
    #[must_use]
    pub fn from_style(style: &ComputedStyle, colors: &ColorTable) -> Palette {
        let foreground = style.color();
        let mut palette = Palette::for_color(foreground);
        let Value::IconPalette(entries) = style.raw(Prop::GtkIconPalette) else {
            return palette;
        };
        let ctx = ColorCtx {
            table: colors,
            current: foreground,
            depth: 0,
        };
        for (name, value) in entries.iter() {
            let Some(resolved) = ColorValue::resolve(value, &ctx) else {
                continue;
            };
            if name.eq_ignore_ascii_case("success") {
                palette.success = resolved;
            } else if name.eq_ignore_ascii_case("warning") {
                palette.warning = resolved;
            } else if name.eq_ignore_ascii_case("error") {
                palette.error = resolved;
            }
        }
        palette
    }

    /// The four slots packed as `0xAARRGGBB`, for use as a cache key.
    ///
    /// `Palette` holds `f32`s and so is neither `Eq` nor `Hash`; the packed
    /// form is exactly what a rasterisation depends on, since that is what
    /// reaches the SVG's `fill`.
    #[must_use]
    #[allow(dead_code)]
    pub(crate) fn key(self) -> [u32; 4] {
        [
            self.foreground.to_color32().0,
            self.success.to_color32().0,
            self.warning.to_color32().0,
            self.error.to_color32().0,
        ]
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

    // Reconciliation (Task 4): P4's `is_safe_theme_name` predecessor trimmed
    // and treated an all-whitespace name as blank, defaulting to Adwaita.
    // Task 4's `is_safe_theme_name` does not trim -- it only rejects empty,
    // `.`/`..`, separators and NUL -- so three spaces is a literal (if
    // useless) theme name that simply matches nothing on disk. Updated to
    // assert the new, spec-driven behaviour rather than the old default.
    #[test]
    fn a_whitespace_theme_name_is_taken_literally_and_matches_nothing() {
        let theme = IconTheme::with_name_and_roots("   ", vec![]);
        assert_eq!(theme.name(), "   ");
        assert_eq!(
            theme.chain().iter().map(|s| &**s).collect::<Vec<_>>(),
            vec!["   ", "hicolor"]
        );
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

    use super::{MAX_INHERITS, MAX_SUBDIRS, ThemeIndex};

    const MINI: &str = "\
[Icon Theme]
Name=MiniTheme
Comment=A hermetic fixture
Inherits=MiniParent
Directories=16x16/actions,scalable/actions
ScaledDirectories=16x16@2/actions

[16x16/actions]
Size=16
Type=Fixed
Context=Actions

[scalable/actions]
Size=128
MinSize=8
MaxSize=512
Type=Scalable

[16x16@2/actions]
Size=16
Scale=2
Type=Fixed
";

    // Mutation check: stop appending `ScaledDirectories` to `dirs` and the
    // `16x16@2/actions` assertions fail -- which is the whole HiDPI path for
    // a theme that ships pre-rendered 2x assets.
    #[test]
    fn an_index_theme_parses_its_directories_inherits_and_types() {
        let index = ThemeIndex::parse("MiniTheme", MINI);
        assert_eq!(&*index.name, "MiniTheme");
        assert_eq!(index.inherits.len(), 1);
        assert_eq!(&*index.inherits[0], "MiniParent");
        assert_eq!(index.dirs.len(), 3);

        assert_eq!(&*index.dirs[0].path, "16x16/actions");
        assert_eq!(index.dirs[0].size, 16);
        assert_eq!(index.dirs[0].scale, 1);
        assert_eq!(index.dirs[0].kind, DirKind::Fixed { size: 16 });

        assert_eq!(&*index.dirs[1].path, "scalable/actions");
        assert_eq!(index.dirs[1].kind, DirKind::Scalable { min: 8, max: 512 });

        assert_eq!(&*index.dirs[2].path, "16x16@2/actions");
        assert_eq!(index.dirs[2].scale, 2);
    }

    // `Type` defaults to Threshold and `Threshold` to 2; `MinSize`/`MaxSize`
    // default to `Size`; `Scale` defaults to 1. All four are spec defaults
    // and all four are load-bearing for real themes.
    // Mutation check: default `Threshold` to 0 and the `matches(1, 22, 1)`
    // assertion fails.
    #[test]
    fn omitted_keys_take_the_specs_defaults() {
        let text = "\
[Icon Theme]
Directories=a,b
[a]
Size=24
[b]
Size=48
Type=Scalable
";
        let index = ThemeIndex::parse("T", text);
        assert_eq!(
            index.dirs[0].kind,
            DirKind::Threshold {
                size: 24,
                threshold: 2
            }
        );
        assert!(index.dirs[0].kind.matches(1, 22, 1));
        assert_eq!(index.dirs[0].scale, 1);
        assert_eq!(index.dirs[1].kind, DirKind::Scalable { min: 48, max: 48 });
    }

    // A directory named in `Directories=` with no group of its own is not a
    // directory: the spec requires `Size`, and GTK skips such entries rather
    // than inventing one.
    // Mutation check: emit a `SubDir` for a group-less name and `dirs.len()`
    // becomes 2.
    #[test]
    fn a_directory_without_a_group_is_dropped() {
        let text = "\
[Icon Theme]
Directories=ghost,real
[real]
Size=16
Type=Fixed
";
        let index = ThemeIndex::parse("T", text);
        assert_eq!(index.dirs.len(), 1);
        assert_eq!(&*index.dirs[0].path, "real");
    }

    // Comments, blank lines, CRLF line endings, whitespace around `=`, and
    // the localised `Name[de]=` form all appear in shipped index.theme files.
    // Mutation check: split on `'='` with `split('=')` and take the last
    // field, and the `Comment` line's second `=` corrupts nothing here but
    // the `Inherits` assertion fails once a value contains one.
    #[test]
    fn comments_crlf_and_localised_keys_are_tolerated() {
        let text = "# a comment\r\n\
                    [Icon Theme]\r\n\
                    Name[de]=Mini\r\n\
                    Name = MiniTheme \r\n\
                    Inherits = a , b ,, c \r\n\
                    Directories=d\r\n\
                    \r\n\
                    [d]\r\n\
                    Size=16\r\n";
        let index = ThemeIndex::parse("fallback", text);
        assert_eq!(&*index.name, "MiniTheme");
        let inherits: Vec<&str> = index.inherits.iter().map(|s| &**s).collect();
        assert_eq!(inherits, vec!["a", "b", "c"]);
        assert_eq!(index.dirs.len(), 1);
    }

    // Untrusted input: nothing below may panic, and every bound holds.
    // Mutation check: remove the MAX_SUBDIRS truncation and the
    // `dirs.len() <= MAX_SUBDIRS` assertion fails on the flood input.
    #[test]
    fn a_hostile_index_theme_never_panics_and_stays_bounded() {
        let flood_dirs: String = (0..MAX_SUBDIRS * 3)
            .map(|i| format!("d{i}"))
            .collect::<Vec<_>>()
            .join(",");
        let mut flood = format!("[Icon Theme]\nDirectories={flood_dirs}\n");
        for i in 0..MAX_SUBDIRS * 3 {
            flood.push_str(&format!("[d{i}]\nSize=16\n"));
        }
        let index = ThemeIndex::parse("T", &flood);
        assert!(index.dirs.len() <= MAX_SUBDIRS);

        let many_inherits: String = (0..MAX_INHERITS * 4)
            .map(|i| format!("p{i}"))
            .collect::<Vec<_>>()
            .join(",");
        let index = ThemeIndex::parse("T", &format!("[Icon Theme]\nInherits={many_inherits}\n"));
        assert!(index.inherits.len() <= MAX_INHERITS);

        for hostile in [
            "",
            "\0\0\0",
            "[",
            "[]",
            "[Icon Theme",
            "=",
            "Size=16",
            "[a]\nSize=\n",
            "[a]\nSize=-1\n",
            "[a]\nSize=99999999999999999999\n",
            "[Icon Theme]\nDirectories=,,,,\n",
            "[Icon Theme]\nInherits=\n",
            "[Icon Theme]\nDirectories=a\n[a]\nSize=4294967295\nType=Scalable\nMinSize=4294967295\nMaxSize=0\n",
            "[Icon Theme]\r\rDirectories=a\r[a]\rSize=16",
        ] {
            let index = ThemeIndex::parse("T", hostile);
            assert!(index.dirs.len() <= MAX_SUBDIRS, "input: {hostile:?}");
            assert!(index.inherits.len() <= MAX_INHERITS, "input: {hostile:?}");
        }

        // Pseudo-random bytes, deterministically generated: an index.theme
        // can be any file at all that happens to sit at that path.
        let mut state = 0x2545_F491_4F6C_DD1Du64;
        for _ in 0..256 {
            let mut bytes = Vec::with_capacity(512);
            for _ in 0..512 {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                bytes.push((state & 0xFF) as u8);
            }
            let text = String::from_utf8_lossy(&bytes);
            let index = ThemeIndex::parse("T", &text);
            assert!(index.dirs.len() <= MAX_SUBDIRS);
        }
    }

    use super::{HICOLOR, IconTheme};
    use crate::icons::test_support::{pixmaps, roots};

    // The spec's chain: the base theme, then everything it Inherits
    // transitively, then hicolor -- which is appended implicitly because a
    // theme that is not hicolor and does not name it still has to terminate
    // there.
    // Mutation check: stop appending hicolor and the last assertion fails,
    // taking `image-missing` (which only hicolor has) with it.
    #[test]
    fn the_chain_is_the_theme_its_parents_and_then_hicolor() {
        let theme = IconTheme::with_name_and_roots("MiniTheme", roots());
        let chain: Vec<&str> = theme.chain().iter().map(|s| &**s).collect();
        assert_eq!(chain, vec!["MiniTheme", "MiniParent", HICOLOR]);
        assert_eq!(theme.name(), "MiniTheme");
    }

    // hicolor is not appended to itself.
    // Mutation check: drop the `name != HICOLOR` guard and the chain becomes
    // ["hicolor", "hicolor"].
    #[test]
    fn hicolor_does_not_inherit_from_itself() {
        let theme = IconTheme::with_name_and_roots(HICOLOR, roots());
        let chain: Vec<&str> = theme.chain().iter().map(|s| &**s).collect();
        assert_eq!(chain, vec![HICOLOR]);
    }

    // A cycle in Inherits= is a file on disk saying so; it must terminate.
    // Mutation check: remove the `seen` set and this test hangs (which the
    // harness reports as a timeout, not a failure -- so run it alone if it
    // ever regresses).
    #[test]
    fn an_inheritance_cycle_terminates() {
        let theme = IconTheme::with_name_and_roots("MiniLoopA", roots());
        let chain: Vec<&str> = theme.chain().iter().map(|s| &**s).collect();
        assert_eq!(chain, vec!["MiniLoopA", "MiniLoopB", HICOLOR]);
    }

    // A theme name that resolves to nothing on disk still produces a usable
    // theme: the chain terminates at hicolor and every lookup misses.
    // Mutation check: `unwrap()` the missing index and this panics.
    #[test]
    fn a_missing_theme_still_yields_a_terminating_chain() {
        let theme = IconTheme::with_name_and_roots("NoSuchTheme", roots());
        let chain: Vec<&str> = theme.chain().iter().map(|s| &**s).collect();
        assert_eq!(chain, vec!["NoSuchTheme", HICOLOR]);
        assert!(theme.index("NoSuchTheme").is_none());
        assert!(theme.index("MiniParent").is_none());
        assert!(theme.index(HICOLOR).is_some());
    }

    // An empty name is not a theme; GTK's own default is Adwaita and the
    // spec's backstop is hicolor, and with nothing to go on hicolor is the
    // honest answer.
    // Mutation check: keep the empty name in the chain and the assertion
    // finds a leading "".
    #[test]
    fn an_empty_theme_name_falls_back_to_hicolor() {
        let theme = IconTheme::with_name_and_roots("", roots());
        assert_eq!(theme.name(), HICOLOR);
        let chain: Vec<&str> = theme.chain().iter().map(|s| &**s).collect();
        assert_eq!(chain, vec![HICOLOR]);
    }

    // A theme name is untrusted (it comes from settings.ini): it must not be
    // able to escape the roots.
    // Mutation check: drop the separator/`..` rejection and the chain
    // contains the traversal name, which then joins into a path outside the
    // root.
    #[test]
    fn a_traversing_theme_name_is_refused() {
        for hostile in ["../../etc", "a/b", "..", ".", "a\0b"] {
            let theme = IconTheme::with_name_and_roots(hostile, roots());
            assert_eq!(
                theme.name(),
                HICOLOR,
                "{hostile:?} was accepted as a theme name"
            );
        }
    }

    // The chain is bounded even against a pathological on-disk theme graph.
    // Mutation check: remove the MAX_CHAIN cap; this still passes on the
    // fixture, which is why the bound is also asserted directly.
    #[test]
    fn the_chain_is_bounded() {
        let theme = IconTheme::with_name_and_roots("MiniTheme", roots());
        assert!(theme.chain().len() <= super::MAX_CHAIN);
    }

    // The output scale is carried here because `PaintCx` gains exactly one
    // field and `ResolveEnv` has no scale (deviation 3).
    // Mutation check: drop the clamp and `set_scale(0)` makes every render
    // ask for a zero-pixel surface.
    #[test]
    fn the_output_scale_is_clamped_to_a_sane_range() {
        let mut theme = IconTheme::with_name_and_roots("MiniTheme", roots());
        assert_eq!(theme.scale(), 1);
        theme.set_scale(2);
        assert_eq!(theme.scale(), 2);
        theme.set_scale(0);
        assert_eq!(theme.scale(), 1);
        theme.set_scale(u32::MAX);
        assert_eq!(theme.scale(), 4);
    }

    // `with_name_and_roots` is the hermetic constructor: it reads no
    // environment and, per deviation 4, no /usr/share/pixmaps either.
    // Mutation check: have it fill `pixmaps` with the system path and the
    // first assertion fails.
    #[test]
    fn the_hermetic_constructor_has_no_flat_fallback_and_the_other_one_does() {
        let hermetic = IconTheme::with_name_and_roots("MiniTheme", roots());
        assert!(hermetic.pixmap_dirs().is_empty());
        let with_flat = IconTheme::with_name_roots_and_pixmaps("MiniTheme", roots(), pixmaps());
        assert_eq!(with_flat.pixmap_dirs(), pixmaps().as_slice());
        assert_eq!(with_flat.roots(), roots().as_slice());
        let _: &[PathBuf] = hermetic.roots();
    }

    use super::{IconEnv, ini_value};

    // The spec's root order: $XDG_DATA_HOME/icons, $HOME/.icons, then each
    // $XDG_DATA_DIRS entry + /icons. `/usr/share/pixmaps` is not a root: it
    // is the flat last resort, and it comes back from `pixmaps()`.
    // Mutation check: move `$HOME/.icons` ahead of `$XDG_DATA_HOME/icons`
    // and the vector comparison fails.
    #[test]
    fn the_search_roots_are_the_specs_in_the_specs_order() {
        let env = IconEnv {
            xdg_data_home: Some("/data".to_string()),
            xdg_data_dirs: Some("/one:/two".to_string()),
            home: Some("/home/u".to_string()),
            xdg_config_home: None,
        };
        assert_eq!(
            env.roots(),
            vec![
                PathBuf::from("/data/icons"),
                PathBuf::from("/home/u/.icons"),
                PathBuf::from("/one/icons"),
                PathBuf::from("/two/icons"),
            ]
        );
        assert_eq!(env.pixmaps(), vec![PathBuf::from("/usr/share/pixmaps")]);
    }

    // Unset variables take the XDG defaults: $HOME/.local/share for
    // XDG_DATA_HOME, /usr/local/share:/usr/share for XDG_DATA_DIRS.
    // Mutation check: drop the XDG_DATA_DIRS default and the last two
    // entries disappear -- taking /usr/share/icons, i.e. every installed
    // theme, with them.
    #[test]
    fn unset_variables_take_the_xdg_defaults() {
        let env = IconEnv {
            xdg_data_home: None,
            xdg_data_dirs: None,
            home: Some("/home/u".to_string()),
            xdg_config_home: None,
        };
        assert_eq!(
            env.roots(),
            vec![
                PathBuf::from("/home/u/.local/share/icons"),
                PathBuf::from("/home/u/.icons"),
                PathBuf::from("/usr/local/share/icons"),
                PathBuf::from("/usr/share/icons"),
            ]
        );
    }

    // With no $HOME at all there is still a usable system search path.
    // Mutation check: `unwrap()` the home and this panics.
    #[test]
    fn an_environment_with_nothing_set_still_searches_the_system_dirs() {
        let env = IconEnv::default();
        assert_eq!(
            env.roots(),
            vec![
                PathBuf::from("/usr/local/share/icons"),
                PathBuf::from("/usr/share/icons"),
            ]
        );
        assert_eq!(env.theme_name(), "Adwaita");
    }

    // GTK reads gtk-icon-theme-name from the [Settings] group of
    // $XDG_CONFIG_HOME/gtk-4.0/settings.ini, and defaults to Adwaita.
    // Mutation check: read the key from any group and the "wrong group"
    // case returns Papirus.
    #[test]
    fn the_theme_name_comes_from_gtk_fours_settings_ini() {
        let dir = tempfile::tempdir().expect("tempdir");
        let gtk4 = dir.path().join("gtk-4.0");
        std::fs::create_dir_all(&gtk4).expect("mkdir");
        std::fs::write(
            gtk4.join("settings.ini"),
            "# generated\n[Settings]\ngtk-theme-name=Adwaita\ngtk-icon-theme-name = Papirus \n",
        )
        .expect("write");
        let env = IconEnv {
            xdg_config_home: Some(dir.path().display().to_string()),
            ..IconEnv::default()
        };
        assert_eq!(env.theme_name(), "Papirus");

        std::fs::write(
            gtk4.join("settings.ini"),
            "[Other]\ngtk-icon-theme-name=Papirus\n",
        )
        .expect("write");
        assert_eq!(env.theme_name(), "Adwaita");
    }

    // settings.ini is untrusted, and so is the value in it.
    // Mutation check: stop validating the name and the traversal case comes
    // back as "../../etc" instead of Adwaita.
    #[test]
    fn a_hostile_settings_ini_never_panics_and_never_escapes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let gtk4 = dir.path().join("gtk-4.0");
        std::fs::create_dir_all(&gtk4).expect("mkdir");
        let env = IconEnv {
            xdg_config_home: Some(dir.path().display().to_string()),
            ..IconEnv::default()
        };
        for (text, expected) in [
            ("", "Adwaita"),
            ("[Settings]", "Adwaita"),
            ("[Settings]\ngtk-icon-theme-name=\n", "Adwaita"),
            ("[Settings]\ngtk-icon-theme-name=../../etc\n", "Adwaita"),
            ("[Settings]\ngtk-icon-theme-name=a/b\n", "Adwaita"),
            ("[Settings]\ngtk-icon-theme-name=Papirus\n", "Papirus"),
            ("\0\0\0[Settings]\0", "Adwaita"),
            ("[Settings]\r\ngtk-icon-theme-name=Papirus\r\n", "Papirus"),
        ] {
            std::fs::write(gtk4.join("settings.ini"), text).expect("write");
            let theme = IconTheme::from_icon_env(&env);
            assert_eq!(theme.name(), expected, "settings.ini: {text:?}");
        }
    }

    // The INI reader is shared with nothing and does one job.
    // Mutation check: return the value trimmed of quotes as well and the
    // quoted case comes back without them, which GTK does not do.
    #[test]
    fn the_ini_reader_finds_a_key_in_its_own_group_only() {
        let text = "[A]\nk=1\n[B]\nk = 2 \nj=\"q\"\n";
        assert_eq!(ini_value(text, "A", "k").as_deref(), Some("1"));
        assert_eq!(ini_value(text, "B", "k").as_deref(), Some("2"));
        assert_eq!(ini_value(text, "B", "j").as_deref(), Some("\"q\""));
        assert_eq!(ini_value(text, "C", "k"), None);
        assert_eq!(ini_value(text, "A", "j"), None);
        assert_eq!(ini_value("", "A", "k"), None);
    }

    // A cache is only a cache if a second call does not touch the disk. The
    // proof: look up, delete the file, look up again -- and still get it.
    // Mutation check: bypass the memo table in `lookup` and the second
    // assertion returns image-missing instead of the deleted file.
    #[test]
    fn a_second_lookup_does_not_touch_the_disk() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("root");
        let actions = root.join("T/16x16/actions");
        std::fs::create_dir_all(&actions).expect("mkdir");
        std::fs::write(
            root.join("T/index.theme"),
            "[Icon Theme]\nDirectories=16x16/actions\n[16x16/actions]\nSize=16\nType=Fixed\n",
        )
        .expect("write");
        let icon = actions.join("a.png");
        std::fs::write(&icon, b"not really a png, lookup never decodes").expect("write");

        let mut theme = IconTheme::with_name_and_roots("T", vec![root]);
        let first = theme.lookup("a", 16, 1, false).expect("found");
        assert_eq!(first.path, icon);

        std::fs::remove_file(&icon).expect("remove");
        let second = theme.lookup("a", 16, 1, false).expect("cached");
        assert_eq!(second.path, icon);

        theme.clear_caches();
        assert!(theme.lookup("a", 16, 1, false).is_none());
    }

    // Misses are cached too: an application that asks for a missing icon
    // every frame must not walk every directory of every theme every frame.
    // Mutation check: cache only the `Some` results and `lookup_cache_len`
    // stays 0 across the two calls.
    #[test]
    fn a_miss_is_cached_as_a_miss() {
        let mut theme = IconTheme::with_name_and_roots("MiniTheme", Vec::new());
        assert!(theme.lookup("nothing-here", 16, 1, false).is_none());
        assert_eq!(theme.lookup_cache_len(), 1);
        assert!(theme.lookup("nothing-here", 16, 1, false).is_none());
        assert_eq!(theme.lookup_cache_len(), 1);
    }

    // Every part of the key separates entries -- a size, a scale and the
    // symbolic flag all produce different files.
    // Mutation check: drop `symbolic` from the key and the fourth
    // assertion returns the PNG.
    #[test]
    fn the_cache_key_separates_size_scale_and_symbolic() {
        let mut theme = IconTheme::with_name_and_roots("MiniTheme", roots());
        let png = theme.lookup("document-open", 16, 1, false).expect("found");
        let svg = theme.lookup("document-open", 128, 1, false).expect("found");
        let sym = theme.lookup("document-open", 16, 1, true).expect("found");
        assert_ne!(png.path, svg.path);
        assert_ne!(png.path, sym.path);
        assert!(sym.symbolic);
        assert_eq!(theme.lookup_cache_len(), 3);
    }

    // The cache is bounded: an application that generates icon names (a
    // file manager showing MIME icons, say) must not grow it forever.
    // Mutation check: remove the eviction and the assertion fails.
    #[test]
    fn the_lookup_cache_is_bounded() {
        let mut theme = IconTheme::with_name_and_roots("MiniTheme", Vec::new());
        for i in 0..super::MAX_LOOKUP_CACHE * 2 {
            let _ = theme.lookup(&format!("icon-{i}"), 16, 1, false);
        }
        assert!(theme.lookup_cache_len() <= super::MAX_LOOKUP_CACHE);
    }

    // Changing the output scale invalidates every rasterisation decision
    // made under the old one.
    // Mutation check: drop the `clear_caches()` from `set_scale` and the
    // final assertion sees the stale entry count.
    #[test]
    fn setting_the_scale_clears_the_caches() {
        let mut theme = IconTheme::with_name_and_roots("MiniTheme", roots());
        let _ = theme.lookup("document-open", 16, 1, false);
        assert_eq!(theme.lookup_cache_len(), 1);
        theme.set_scale(2);
        assert_eq!(theme.lookup_cache_len(), 0);
    }

    use super::Palette;
    use crate::css::cascade::CompiledSheet;
    use crate::css::computed::{ComputedStyle, ResolveEnv};
    use crate::css::node::Node;
    use crate::css::select::MatchCx;
    use crate::css::value::Rgba;

    fn palette_for(css: &str) -> Palette {
        let sheet = CompiledSheet::compile(css);
        let node = Node::new("image");
        let env = ResolveEnv::default();
        let mut cx = MatchCx::new();
        let style = ComputedStyle::resolve_chain(&sheet, &node, &env, &mut cx);
        Palette::from_style(&style, &sheet.colors)
    }

    fn hex(color: Rgba) -> u32 {
        color.to_color32().0
    }

    // The foreground slot is the node's own `color`, which is what GTK
    // passes as `colors[0]` to `snapshot_symbolic`.
    // Mutation check: read `Prop::BackgroundColor` instead of `color()` and
    // the first assertion fails.
    #[test]
    fn the_foreground_slot_is_the_nodes_color() {
        let palette = palette_for("image { color: #3584e4 }");
        assert_eq!(hex(palette.foreground), 0xFF35_84E4);
    }

    // With no `@define-color`s in the sheet, the registry's initial
    // `-gtk-icon-palette` (`success @success_color, ...`) cannot resolve, so
    // the slots take the vendored Adwaita values.
    // Mutation check: return `Rgba::TRANSPARENT` for an unresolvable slot and
    // every symbolic status icon paints as nothing.
    #[test]
    fn unresolvable_slots_take_the_vendored_adwaita_defaults() {
        let palette = palette_for("image { color: #000000 }");
        assert_eq!(hex(palette.success), 0xFF33_D17A);
        assert_eq!(hex(palette.warning), 0xFFF5_7900);
        assert_eq!(hex(palette.error), 0xFFCC_0000);
    }

    // With them, the sheet's own colours win -- which is how a theme
    // restyles every status icon at once.
    // Mutation check: ignore the ColorTable and the assertions see the
    // defaults instead.
    #[test]
    fn defined_colors_feed_the_three_status_slots() {
        let palette = palette_for(
            "@define-color success_color #26a269; \
             @define-color warning_color #cd9309; \
             @define-color error_color #e01b24; \
             image { color: #000000 }",
        );
        assert_eq!(hex(palette.success), 0xFF26_A269);
        assert_eq!(hex(palette.warning), 0xFFCD_9309);
        assert_eq!(hex(palette.error), 0xFFE0_1B24);
    }

    // An explicit `-gtk-icon-palette` overrides slot by slot and leaves the
    // rest alone; `currentColor` in it means the node's own colour.
    // Mutation check: replace the whole palette from the declaration instead
    // of overriding named slots and `success` comes back transparent.
    #[test]
    fn gtk_icon_palette_overrides_named_slots_only() {
        let palette = palette_for(
            "image { color: #3584e4; -gtk-icon-palette: warning #ff0000, error currentColor }",
        );
        assert_eq!(hex(palette.warning), 0xFFFF_0000);
        assert_eq!(hex(palette.error), 0xFF35_84E4);
        assert_eq!(hex(palette.success), 0xFF33_D17A);
        assert_eq!(hex(palette.foreground), 0xFF35_84E4);
    }

    // A slot name GTK does not know is ignored, not fatal.
    // Mutation check: panic or clear the palette on an unknown name and this
    // fails.
    #[test]
    fn an_unknown_palette_slot_is_ignored() {
        let palette = palette_for("image { color: #000000; -gtk-icon-palette: bogus #ff0000 }");
        assert_eq!(hex(palette.success), 0xFF33_D17A);
        assert_eq!(hex(palette.foreground), 0xFF00_0000);
    }

    // The cache key is the four slots, packed -- `Palette` holds `f32`s and
    // so is not `Hash`/`Eq`.
    // Mutation check: key on the foreground alone and the last assertion
    // fails, so a palette change would serve a stale rasterisation.
    #[test]
    fn the_palette_key_separates_every_slot() {
        let base = Palette::for_color(Rgba {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        });
        let other = Palette {
            success: Rgba {
                r: 1.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            },
            ..base
        };
        assert_eq!(base.key(), base.key());
        assert_ne!(base.key(), other.key());
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
