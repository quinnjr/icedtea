//! `.desktop` entry model and file parser.
//!
//! Pure parsing with no I/O and no Wayland imports: every malformed input
//! yields `None` (total parse, never panics).
//!
//! Accepted keys inside the `[Desktop Entry]` group: `Name`, `Name[xx]`
//! (an exact `$LANG`/`$LANGUAGE` match wins, else the first variant, else
//! plain `Name`), `Exec`, `Icon`,
//! `Categories`, `Keywords`, `NoDisplay`, `OnlyShowIn`, `NotShowIn`.
//! Everything else (including `Type`) is ignored leniently: an entry with a
//! non-empty `Name` and `Exec` parses regardless of `Type`.

use std::collections::HashMap;

/// A single launchable application parsed from a `.desktop` file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopEntry {
    /// App id, normally the `.desktop` file stem (e.g. `"firefox"`).
    pub id: String,
    /// Display name (exact `$LANG`/`$LANGUAGE` `Name[xx]` match wins, else
    /// the first variant, else plain `Name`).
    pub name: String,
    /// Raw `Exec` value, field codes included. Kept verbatim for launch;
    /// use [`DesktopEntry::display_exec`] for a human-readable form.
    pub exec: String,
    /// Raw `Icon` value (theme name or absolute path).
    pub icon: Option<String>,
    /// `Categories` (`;`-separated in the file).
    pub categories: Vec<String>,
    /// `Keywords` (`;`-separated in the file).
    pub keywords: Vec<String>,
    /// Parsed `NoDisplay` value.
    pub nodisplay: bool,
    /// Parsed `OnlyShowIn` list (`;`-separated in the file).
    pub only_show_in: Vec<String>,
    /// Parsed `NotShowIn` list (`;`-separated in the file).
    pub not_show_in: Vec<String>,
}

/// Field codes stripped for display (kept raw in [`DesktopEntry::exec`]).
const DISPLAY_STRIP_CODES: &[char] = &['f', 'F', 'u', 'U', 'i', 'c', 'k'];

impl DesktopEntry {
    /// Human-readable `Exec`: field codes removed wherever they occur,
    /// `%%` unescaped to a literal `%`, remaining whitespace collapsed to
    /// single spaces.
    ///
    /// Rule: every `%x` with `x` in [`DISPLAY_STRIP_CODES`] is dropped
    /// whether standalone (`"foo %f bar"` → `"foo bar"`) or embedded
    /// (`"foo%fbar"` → `"foobar"`); `%%` becomes a literal `%` (`"%%f"`
    /// → `"%f"`); any other `%x` sequence and a lone `%` are kept
    /// verbatim. Stripping runs before unescaping so `"%%f"` reads as an
    /// escaped percent plus `f`, not as a field code.
    pub fn display_exec(&self) -> String {
        // Placeholder for an escaped percent: strip first, unescape after,
        // in a single scan so `"%%f"` cannot collapse into a field code.
        const ESCAPED: char = '\u{E000}';
        let mut stripped = String::with_capacity(self.exec.len());
        let mut chars = self.exec.chars().peekable();
        while let Some(c) = chars.next() {
            if c != '%' {
                stripped.push(c);
                continue;
            }
            match chars.peek() {
                Some('%') => {
                    chars.next();
                    stripped.push(ESCAPED);
                }
                Some(next) if DISPLAY_STRIP_CODES.contains(next) => {
                    chars.next();
                }
                _ => stripped.push('%'),
            }
        }
        stripped
            .replace(ESCAPED, "%")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// First whitespace-separated token of the display `Exec`, reduced to its
    /// file-name portion (e.g. `"/usr/bin/foo --bar"` → `"foo"`).
    /// Empty when there is no `Exec`.
    pub fn exec_basename(&self) -> String {
        self.display_exec()
            .split_whitespace()
            .next()
            .unwrap_or("")
            .rsplit('/')
            .next()
            .unwrap_or("")
            .to_string()
    }

    /// Visibility of this entry in the desktop named `env`
    /// (e.g. `"icedtea"`): false when `NoDisplay`, when a non-empty
    /// `OnlyShowIn` omits `env`, or when `NotShowIn` contains `env`.
    pub fn visible_in(&self, env: &str) -> bool {
        if self.nodisplay {
            return false;
        }
        if !self.only_show_in.is_empty() && !self.only_show_in.iter().any(|e| e == env) {
            return false;
        }
        if self.not_show_in.iter().any(|e| e == env) {
            return false;
        }
        true
    }
}

/// Parse one `.desktop` file's text. Returns `None` when there is no
/// `[Desktop Entry]` group or when `Name`/`Exec` is missing or empty.
/// The id is left empty; use [`parse_entry_with_id`] when it is known.
pub fn parse_entry(text: &str) -> Option<DesktopEntry> {
    parse_entry_with_id("", text)
}

/// Parse one `.desktop` file's text with an explicit app id, using the
/// process locale (`LANG`/`LANGUAGE`) for localized `Name` selection.
pub fn parse_entry_with_id(id: &str, text: &str) -> Option<DesktopEntry> {
    parse_entry_with_id_and_locale(id, text, locale_from_env().as_ref())
}

/// Parse one `.desktop` file's text with an explicit app id and an explicit
/// locale (hermetic: no environment reads — tests pass the locale in).
///
/// Localized-name rule: the first `Name[$candidate]` with an exact match
/// wins, where the candidates are `$LANG` (full value, then without the
/// codeset/modifier suffix, then the bare language) followed by each
/// colon-separated `$LANGUAGE` entry in order; with no exact match the
/// first `Name[xx]` variant in file order wins; with no variant at all the
/// plain `Name` is used.
pub fn parse_entry_with_id_and_locale(
    id: &str,
    text: &str,
    locale: Option<&Locale>,
) -> Option<DesktopEntry> {
    let mut in_entry_group = false;
    let mut seen_entry_group = false;
    // Every `Name[locale]` variant in file order: (locale, value).
    let mut locale_names: Vec<(String, String)> = Vec::new();
    // First occurrence of each plain key wins.
    let mut values: HashMap<String, String> = HashMap::new();

    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            in_entry_group = line == "[Desktop Entry]";
            seen_entry_group = seen_entry_group || in_entry_group;
            continue;
        }
        if !in_entry_group {
            continue;
        }
        let (raw_key, value) = match line.split_once('=') {
            Some(pair) => pair,
            None => continue,
        };
        let key = raw_key.trim();
        let value = value.trim();
        if let Some(base) = localized_base(key) {
            if base == "Name"
                && let Some(tag) = localized_tag(key)
                && !value.is_empty()
            {
                locale_names.push((tag.to_string(), value.to_string()));
            }
            continue;
        }
        values
            .entry(key.to_string())
            .or_insert_with(|| value.to_string());
    }

    if !seen_entry_group {
        return None;
    }
    let name = select_locale_name(&locale_names, locale)
        .or_else(|| values.get("Name").cloned())
        .filter(|name| !name.is_empty())?;
    let exec = values
        .get("Exec")
        .cloned()
        .filter(|exec| !exec.is_empty())?;
    let icon = values.get("Icon").cloned();
    let categories = values
        .get("Categories")
        .map(|value| split_list(value))
        .unwrap_or_default();
    let keywords = values
        .get("Keywords")
        .map(|value| split_list(value))
        .unwrap_or_default();
    let nodisplay = values
        .get("NoDisplay")
        .map(|value| value.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    let only_show_in = values
        .get("OnlyShowIn")
        .map(|value| split_list(value))
        .unwrap_or_default();
    let not_show_in = values
        .get("NotShowIn")
        .map(|value| split_list(value))
        .unwrap_or_default();

    Some(DesktopEntry {
        id: id.to_string(),
        name,
        exec,
        icon,
        categories,
        keywords,
        nodisplay,
        only_show_in,
        not_show_in,
    })
}

/// If `key` has the localized `Base[locale]` form, return the base name.
fn localized_base(key: &str) -> Option<&str> {
    let localized = key.strip_suffix(']')?;
    let open = localized.find('[')?;
    Some(&localized[..open])
}

/// If `key` has the localized `Base[locale]` form, return the locale tag.
fn localized_tag(key: &str) -> Option<&str> {
    let localized = key.strip_suffix(']')?;
    let open = localized.find('[')?;
    Some(&localized[open + 1..])
}

/// The process locale for localized-`Name` selection: `$LANG` plus the
/// colon-separated `$LANGUAGE` list, empty parts dropped.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Locale {
    /// Raw `$LANG` value (e.g. `"de_DE.UTF-8"`).
    pub lang: Option<String>,
    /// `$LANGUAGE` entries in priority order (e.g. `["de", "fr"]`).
    pub language: Vec<String>,
}

/// Read the process locale from the environment.
fn locale_from_env() -> Option<Locale> {
    let lang = std::env::var("LANG").ok().filter(|v| !v.is_empty());
    let language: Vec<String> = std::env::var("LANGUAGE")
        .map(|v| {
            v.split(':')
                .filter(|part| !part.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    if lang.is_none() && language.is_empty() {
        return None;
    }
    Some(Locale { lang, language })
}

/// Candidate tags for one `$LANG` value, most specific first: the full
/// value (`"de_DE.UTF-8"`), without the codeset/modifier (`"de_DE"`),
/// then the bare language (`"de"`).
fn lang_candidates(lang: &str) -> Vec<&str> {
    let mut out = vec![lang];
    let without_modifier = lang.split('@').next().unwrap_or(lang);
    let without_codeset = without_modifier
        .split('.')
        .next()
        .unwrap_or(without_modifier);
    if without_codeset != lang {
        out.push(without_codeset);
    }
    let bare = without_codeset.split('_').next().unwrap_or(without_codeset);
    if bare != without_codeset {
        out.push(bare);
    }
    out
}

/// Pick the display name from the file-order `Name[locale]` variants:
/// first exact candidate match wins, else the first variant.
fn select_locale_name(variants: &[(String, String)], locale: Option<&Locale>) -> Option<String> {
    if let Some(locale) = locale {
        let mut candidates: Vec<&str> = Vec::new();
        if let Some(lang) = &locale.lang {
            candidates.extend(lang_candidates(lang));
        }
        candidates.extend(locale.language.iter().map(String::as_str));
        for candidate in candidates {
            if let Some(hit) = variants.iter().find(|(tag, _)| tag == candidate) {
                return Some(hit.1.clone());
            }
        }
    }
    variants.first().map(|(_, value)| value.clone())
}

/// Split a `;`-separated desktop list value, dropping empties.
fn split_list(value: &str) -> Vec<String> {
    value
        .split(';')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(ToString::to_string)
        .collect()
}
