//! `.desktop` entry model and file parser.
//!
//! Pure parsing with no I/O and no Wayland imports: every malformed input
//! yields `None` (total parse, never panics).
//!
//! Accepted keys inside the `[Desktop Entry]` group: `Name`, `Name[xx]`
//! (any locale variant wins over plain `Name`), `Exec`, `Icon`,
//! `Categories`, `Keywords`, `NoDisplay`, `OnlyShowIn`, `NotShowIn`.
//! Everything else (including `Type`) is ignored leniently: an entry with a
//! non-empty `Name` and `Exec` parses regardless of `Type`.

use std::collections::HashMap;

/// A single launchable application parsed from a `.desktop` file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopEntry {
    /// App id, normally the `.desktop` file stem (e.g. `"firefox"`).
    pub id: String,
    /// Display name (`Name[xx]` locale variant wins over `Name` when present).
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
    /// Human-readable `Exec`: `%%` unescaped to `%`, standalone field codes
    /// removed, remaining whitespace collapsed to single spaces.
    pub fn display_exec(&self) -> String {
        self.exec
            .replace("%%", "%")
            .split_whitespace()
            .filter(|tok| {
                let bytes = tok.as_bytes();
                !(bytes.len() == 2
                    && bytes[0] == b'%'
                    && DISPLAY_STRIP_CODES.contains(&(bytes[1] as char)))
            })
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

/// Parse one `.desktop` file's text with an explicit app id.
pub fn parse_entry_with_id(id: &str, text: &str) -> Option<DesktopEntry> {
    let mut in_entry_group = false;
    let mut seen_entry_group = false;
    let mut locale_name: Option<String> = None;
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
            if base == "Name" && locale_name.is_none() {
                locale_name = Some(value.to_string());
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
    let name = locale_name
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

/// Split a `;`-separated desktop list value, dropping empties.
fn split_list(value: &str) -> Vec<String> {
    value
        .split(';')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(ToString::to_string)
        .collect()
}
