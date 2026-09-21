//! The registration ingestion scanner (design §"Registrations").
//!
//! Reads `.desktop` files, autostart entries, and shared-MIME-info's `globs2`
//! index, and writes normalized records into the registry: `/apps/<id>`,
//! `/desktop/mime/<type>/handlers`, `/desktop/scheme/<scheme>/handlers`,
//! `/desktop/autostart/<id>`, plus the DE's own `/desktop/daemons/<name>` seed.
//!
//! Three properties are load-bearing and pinned by tests:
//!
//! * **Idempotent** — a second run over unchanged inputs writes nothing (no
//!   `seq` bump).
//! * **A user's default is never reordered** — existing handler entries keep
//!   their position; new vendor claims are appended.
//! * **A removed association is never resurrected** — an id in
//!   `/desktop/mime/<type>/removed` is neither re-added nor kept.
//!
//! Hostile input: `Exec` is parsed, never shell-evaluated, and the only MIME
//! source read is the line-based `globs2` text index — there is no XML parser
//! here, so entity-expansion/XXE surface simply does not exist. Every file is
//! size-capped and every malformed entry is skipped with a warning.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use icedtea_registry::{
    Registry, RegistryError, Value, mime_handlers_path, mime_path, mime_removed_path, scheme_path,
};

/// Largest `.desktop`/`globs2` file read, in bytes.
const MAX_FILE_BYTES: u64 = 1 << 20;
/// Largest accepted `Exec` argument list.
const MAX_EXEC_ARGS: usize = 64;
/// Largest accepted individual argument length.
const MAX_ARG_LEN: usize = 4096;

/// One normalized application, as parsed from a `.desktop` file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppEntry {
    /// The `.desktop` basename without the extension.
    pub id: String,
    pub name: String,
    pub icon: String,
    pub exec: Vec<String>,
    pub categories: Vec<String>,
    pub mime_types: Vec<String>,
}

/// One autostart entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutostartEntry {
    pub id: String,
    pub name: String,
    pub icon: String,
    pub exec: Vec<String>,
    pub enabled: bool,
}

/// What one ingest run did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScanSummary {
    pub apps: usize,
    pub handler_lists_changed: usize,
    pub autostart: usize,
    /// True when the run wrote nothing at all.
    pub unchanged: bool,
}

/// The XDG application directories, highest priority first.
pub fn default_app_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(data_home) = std::env::var_os("XDG_DATA_HOME") {
        dirs.push(PathBuf::from(data_home).join("applications"));
    } else if let Some(home) = std::env::home_dir() {
        dirs.push(home.join(".local/share/applications"));
    }
    let data_dirs = std::env::var_os("XDG_DATA_DIRS")
        .map(|v| v.to_string_lossy().into_owned())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".to_string());
    for dir in data_dirs.split(':').filter(|d| !d.is_empty()) {
        dirs.push(PathBuf::from(dir).join("applications"));
    }
    dirs
}

/// The XDG autostart directories, highest priority first.
pub fn default_autostart_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(config_home) = std::env::var_os("XDG_CONFIG_HOME") {
        dirs.push(PathBuf::from(config_home).join("autostart"));
    } else if let Some(home) = std::env::home_dir() {
        dirs.push(home.join(".config/autostart"));
    }
    dirs.push(PathBuf::from("/etc/xdg/autostart"));
    dirs
}

/// The shared-MIME-info `globs2` index files.
fn default_globs_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Some(data_home) = std::env::var_os("XDG_DATA_HOME") {
        files.push(PathBuf::from(data_home).join("mime/globs2"));
    }
    let data_dirs = std::env::var_os("XDG_DATA_DIRS")
        .map(|v| v.to_string_lossy().into_owned())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".to_string());
    for dir in data_dirs.split(':').filter(|d| !d.is_empty()) {
        files.push(PathBuf::from(dir).join("mime/globs2"));
    }
    files
}

/// A `[Desktop Entry]` group, the only group this scanner reads.
#[derive(Debug, Default, Clone)]
struct DesktopGroup {
    entry_type: String,
    name: String,
    icon: String,
    exec: String,
    categories: Vec<String>,
    mime_types: Vec<String>,
    no_display: bool,
    hidden: bool,
    autostart_enabled: Option<bool>,
}

/// Parse the `[Desktop Entry]` group of a `.desktop` file. Localized keys
/// (`Name[fr]`) are ignored in favour of the unlocalized form; a malformed line
/// is skipped rather than failing the whole file.
fn parse_desktop_entry(text: &str) -> Option<DesktopGroup> {
    let mut group: Option<DesktopGroup> = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            let in_entry = line == "[Desktop Entry]";
            if in_entry {
                group = Some(DesktopGroup::default());
            } else if group.is_some() {
                // A following group ends the one we care about.
                break;
            }
            continue;
        }
        let Some(g) = group.as_mut() else {
            continue;
        };
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        // Ignore localized keys like `Name[fr]`.
        if key.contains('[') {
            continue;
        }
        match key {
            "Type" => g.entry_type = value.to_string(),
            "Name" => g.name = value.to_string(),
            "Icon" => g.icon = value.to_string(),
            "Exec" => g.exec = value.to_string(),
            "Categories" => g.categories = split_list(value),
            "MimeType" => g.mime_types = split_list(value),
            "NoDisplay" => g.no_display = value.eq_ignore_ascii_case("true"),
            "Hidden" => g.hidden = value.eq_ignore_ascii_case("true"),
            "X-GNOME-Autostart-enabled" => {
                g.autostart_enabled = Some(!value.eq_ignore_ascii_case("false"))
            }
            _ => {}
        }
    }
    group
}

fn split_list(value: &str) -> Vec<String> {
    value
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Parse a desktop `Exec` value into argv: whitespace-separated, honoring
/// double quotes and backslash escapes, dropping field codes (`%f`, `%u`, …;
/// `%%` is a literal percent). Returns `None` on unbalanced quotes, an empty
/// command, or an over-long argument — the caller skips the entry.
///
/// This never invokes a shell: the result is an argv the caller execs directly.
pub fn parse_exec(exec: &str) -> Option<Vec<String>> {
    let mut args: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut has_current = false;
    let mut in_quotes = false;
    let mut chars = exec.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if let Some(next) = chars.next() {
                    current.push(next);
                    has_current = true;
                }
            }
            '"' => {
                in_quotes = !in_quotes;
                has_current = true;
            }
            c if c.is_whitespace() && !in_quotes => {
                if has_current {
                    push_arg(&mut args, &mut current)?;
                    has_current = false;
                }
            }
            '%' => match chars.next() {
                Some('%') => {
                    current.push('%');
                    has_current = true;
                }
                // A field code (`%f`, `%u`, `%i`, …) is dropped entirely.
                Some(_) => {}
                None => {}
            },
            c => {
                current.push(c);
                has_current = true;
            }
        }
    }
    if in_quotes {
        return None;
    }
    if has_current {
        push_arg(&mut args, &mut current)?;
    }
    if args.is_empty() || args.len() > MAX_EXEC_ARGS {
        return None;
    }
    Some(args)
}

fn push_arg(args: &mut Vec<String>, current: &mut String) -> Option<()> {
    if current.len() > MAX_ARG_LEN || current.contains('\0') {
        return None;
    }
    args.push(std::mem::take(current));
    Some(())
}

/// Scan application directories, highest priority first. A later directory
/// cannot shadow an id already found in an earlier one.
pub fn scan_applications(dirs: &[PathBuf]) -> Vec<AppEntry> {
    let mut seen: HashMap<String, ()> = HashMap::new();
    let mut apps = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("desktop") {
                continue;
            }
            let Some(id) = path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(str::to_string)
            else {
                continue;
            };
            if seen.contains_key(&id) {
                continue;
            }
            let Some(group) = read_desktop(&path) else {
                continue;
            };
            if group.entry_type != "Application" || group.no_display || group.hidden {
                continue;
            }
            let Some(exec) = parse_exec(&group.exec) else {
                tracing::debug!(path = %path.display(), "skipping .desktop with an unparsable Exec");
                continue;
            };
            seen.insert(id.clone(), ());
            apps.push(AppEntry {
                id,
                name: if group.name.is_empty() {
                    group.exec.clone()
                } else {
                    group.name
                },
                icon: group.icon,
                exec,
                categories: group.categories,
                mime_types: group.mime_types,
            });
        }
    }
    apps.sort_by(|a, b| a.id.cmp(&b.id));
    apps
}

/// Scan autostart directories, highest priority first.
pub fn scan_autostart(dirs: &[PathBuf]) -> Vec<AutostartEntry> {
    let mut seen: HashMap<String, ()> = HashMap::new();
    let mut out = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("desktop") {
                continue;
            }
            let Some(id) = path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(str::to_string)
            else {
                continue;
            };
            if seen.contains_key(&id) {
                continue;
            }
            let Some(group) = read_desktop(&path) else {
                continue;
            };
            let Some(exec) = parse_exec(&group.exec) else {
                continue;
            };
            seen.insert(id.clone(), ());
            out.push(AutostartEntry {
                id,
                name: group.name,
                icon: group.icon,
                exec,
                enabled: group.autostart_enabled.unwrap_or(true),
            });
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

fn read_desktop(path: &Path) -> Option<DesktopGroup> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > MAX_FILE_BYTES {
        tracing::warn!(path = %path.display(), "skipping oversized .desktop");
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    parse_desktop_entry(&text)
}

/// Parse the `globs2` index (`weight:glob:mime:flags` per line) into
/// `mime -> [glob]`. Malformed lines are skipped; the file is size-capped.
pub fn parse_globs2(text: &str) -> BTreeMap<String, Vec<String>> {
    let mut map: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for line in text.lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // `weight:mime:glob:flags`
        let mut parts = line.splitn(4, ':');
        let (_weight, mime, glob) = match (parts.next(), parts.next(), parts.next()) {
            (Some(w), Some(m), Some(g)) => (w, m, g),
            _ => continue,
        };
        if glob.is_empty() || mime.is_empty() {
            continue;
        }
        map.entry(mime.to_string())
            .or_default()
            .push(glob.to_string());
    }
    map
}

fn scan_globs() -> BTreeMap<String, Vec<String>> {
    let mut merged: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for file in default_globs_files() {
        let Ok(meta) = std::fs::metadata(&file) else {
            continue;
        };
        if meta.len() > MAX_FILE_BYTES {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        for (mime, globs) in parse_globs2(&text) {
            merged.entry(mime).or_default().extend(globs);
        }
    }
    merged
}

/// Where a `.desktop` `MimeType` claim points: a regular MIME handler list, or
/// a URI-scheme handler list for `x-scheme-handler/<scheme>`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum HandlerTarget {
    Mime(String),
    Scheme(String),
}

fn handler_target(claim: &str) -> Option<HandlerTarget> {
    let claim = claim.trim();
    if claim.is_empty() {
        return None;
    }
    if let Some(scheme) = claim.strip_prefix("x-scheme-handler/") {
        if scheme.is_empty() {
            return None;
        }
        return Some(HandlerTarget::Scheme(scheme.to_ascii_lowercase()));
    }
    if claim.contains('/') {
        return Some(HandlerTarget::Mime(claim.to_ascii_lowercase()));
    }
    None
}

fn handlers_path(target: &HandlerTarget) -> String {
    match target {
        HandlerTarget::Mime(mime) => mime_handlers_path(mime),
        HandlerTarget::Scheme(scheme) => icedtea_registry::scheme_handlers_path(scheme),
    }
}

fn removed_path(target: &HandlerTarget) -> String {
    match target {
        HandlerTarget::Mime(mime) => mime_removed_path(mime),
        HandlerTarget::Scheme(scheme) => format!("{}/removed", scheme_path(scheme)),
    }
}

fn record(fields: &[(&str, Value)]) -> Value {
    let mut row = BTreeMap::new();
    for (key, value) in fields {
        row.insert((*key).to_string(), value.clone());
    }
    Value::Record(row)
}

fn str_list(items: &[String]) -> Value {
    Value::StrList(items.to_vec())
}

fn row_str(row: &BTreeMap<String, Value>, key: &str) -> Option<String> {
    match row.get(key) {
        Some(Value::Str(s)) => Some(s.clone()),
        _ => None,
    }
}

#[cfg(test)]
fn row_str_list(row: &BTreeMap<String, Value>, key: &str) -> Vec<String> {
    match row.get(key) {
        Some(Value::StrList(list)) => list.clone(),
        _ => Vec::new(),
    }
}

fn app_record(app: &AppEntry) -> Value {
    record(&[
        ("name", Value::Str(app.name.clone())),
        ("icon", Value::Str(app.icon.clone())),
        ("exec", str_list(&app.exec)),
        ("categories", str_list(&app.categories)),
        ("mime_types", str_list(&app.mime_types)),
        ("source", Value::Str("vendor".into())),
    ])
}

fn autostart_record(entry: &AutostartEntry) -> Value {
    record(&[
        ("name", Value::Str(entry.name.clone())),
        ("icon", Value::Str(entry.icon.clone())),
        ("exec", str_list(&entry.exec)),
        ("enabled", Value::Bool(entry.enabled)),
        ("source", Value::Str("vendor".into())),
    ])
}

fn handler_record(id: &str, source: &str) -> Value {
    record(&[
        ("id", Value::Str(id.to_string())),
        ("source", Value::Str(source.to_string())),
    ])
}

/// Ingest `.desktop`, autostart, and `globs2` inputs into `reg`. Idempotent: a
/// second run over unchanged inputs writes nothing.
pub fn ingest(
    reg: &Registry,
    app_dirs: &[PathBuf],
    autostart_dirs: &[PathBuf],
) -> Result<ScanSummary, RegistryError> {
    let mut summary = ScanSummary::default();
    let apps = scan_applications(app_dirs);

    // Upsert each app record, and collect mime/scheme claims per handler list.
    let mut claims: BTreeMap<HandlerTarget, Vec<String>> = BTreeMap::new();
    let mut writes: Vec<(String, Value)> = Vec::new();
    for app in &apps {
        let path = icedtea_registry::app_path(&app.id);
        let desired = app_record(app);
        if reg.get(&path).ok().map(|(v, _)| v) != Some(desired.clone()) {
            writes.push((path, desired));
        }
        for claim in &app.mime_types {
            if let Some(target) = handler_target(claim) {
                claims.entry(target).or_default().push(app.id.clone());
            }
        }
    }
    // Remove vendor app records whose `.desktop` is gone.
    let present: std::collections::HashSet<&str> = apps.iter().map(|a| a.id.as_str()).collect();
    for (path, _) in reg.list("/apps", false).unwrap_or_default() {
        let Some(id) = path.strip_prefix("/apps/") else {
            continue;
        };
        if present.contains(id) {
            continue;
        }
        if let Ok(Some(Value::Record(row))) = reg.get_stored(&path)
            && row_str(&row, "source").as_deref() == Some("vendor")
        {
            writes.push((path, Value::Null));
        }
    }

    // The mime-type index from globs2 (informational; not a handler list).
    let mut glob_writes: Vec<(String, Value)> = Vec::new();
    for (mime, globs) in scan_globs() {
        let path = format!("{}/globs", mime_path(&mime));
        let desired = str_list(&globs);
        if reg.get(&path).ok().map(|(v, _)| v) != Some(desired.clone()) {
            glob_writes.push((path, desired));
        }
    }

    // Handler lists: merge, preserving order and the `removed` suppression.
    for (target, mut claimed) in claims {
        claimed.sort();
        claimed.dedup();
        if merge_handler_list(reg, &target, &claimed)? {
            summary.handler_lists_changed += 1;
        }
    }

    // Autostart.
    let autostart = scan_autostart(autostart_dirs);
    for entry in &autostart {
        let path = format!("/desktop/autostart/{}", entry.id);
        let desired = autostart_record(entry);
        if reg.get(&path).ok().map(|(v, _)| v) != Some(desired.clone()) {
            writes.push((path, desired));
        }
    }
    let present: std::collections::HashSet<&str> =
        autostart.iter().map(|a| a.id.as_str()).collect();
    for (path, _) in reg.list("/desktop/autostart", false).unwrap_or_default() {
        let Some(id) = path.strip_prefix("/desktop/autostart/") else {
            continue;
        };
        if present.contains(id) {
            continue;
        }
        if let Ok(Some(Value::Record(row))) = reg.get_stored(&path)
            && row_str(&row, "source").as_deref() == Some("vendor")
        {
            writes.push((path, Value::Null));
        }
    }

    seed_daemons(reg)?;

    let unchanged =
        writes.is_empty() && glob_writes.is_empty() && summary.handler_lists_changed == 0;
    apply_writes(reg, writes)?;
    apply_writes(reg, glob_writes)?;

    summary.apps = apps.len();
    summary.autostart = autostart.len();
    summary.unchanged = unchanged;
    Ok(summary)
}

/// Write a batch, where a `Value::Null` means "unset this path".
fn apply_writes(reg: &Registry, writes: Vec<(String, Value)>) -> Result<(), RegistryError> {
    let mut sets = Vec::new();
    let mut unsets = Vec::new();
    for (path, value) in writes {
        match value {
            Value::Null => unsets.push(path),
            value => sets.push((path, value)),
        }
    }
    if !sets.is_empty() {
        reg.set_many(&sets)?;
    }
    if !unsets.is_empty() {
        reg.unset_many(&unsets)?;
    }
    Ok(())
}

/// Merge vendor claims into one handler list. Existing entries keep their
/// position and source; a vendor entry no longer claimed (or now `removed`) is
/// dropped; newly claimed ids are appended. Returns whether anything changed.
fn merge_handler_list(
    reg: &Registry,
    target: &HandlerTarget,
    claimed: &[String],
) -> Result<bool, RegistryError> {
    let path = handlers_path(target);
    let removed: Vec<String> = reg
        .get(&removed_path(target))
        .ok()
        .and_then(|(v, _)| match v {
            Value::StrList(list) => Some(list),
            _ => None,
        })
        .unwrap_or_default();

    let existing: Vec<BTreeMap<String, Value>> = reg
        .get(&path)
        .ok()
        .and_then(|(v, _)| match v {
            Value::RecordList(rows) => Some(rows),
            _ => None,
        })
        .unwrap_or_default();

    let mut result: Vec<(String, String)> = existing
        .iter()
        .filter_map(|row| {
            let id = row_str(row, "id")?;
            let source = row_str(row, "source").unwrap_or_else(|| "user".to_string());
            Some((id, source))
        })
        .collect();

    result.retain(|(id, source)| {
        source != "vendor" || (claimed.contains(id) && !removed.contains(id))
    });
    for id in claimed {
        if removed.contains(id) {
            continue;
        }
        if !result.iter().any(|(existing, _)| existing == id) {
            result.push((id.clone(), "vendor".to_string()));
        }
    }

    let desired = Value::RecordList(
        result
            .iter()
            .map(|(id, source)| {
                let Value::Record(row) = handler_record(id, source) else {
                    unreachable!()
                };
                row
            })
            .collect(),
    );
    let current = Value::RecordList(existing);
    if desired == current {
        return Ok(false);
    }
    reg.set(&path, &desired)?;
    Ok(true)
}

/// Seed the DE's own service registry if a name is absent (never overwrites a
/// user edit).
fn seed_daemons(reg: &Registry) -> Result<(), RegistryError> {
    const DAEMONS: [(&str, &str); 4] = [
        ("clipboard", "org.icedtea.Clipboard"),
        ("notifications", "org.icedtea.Notifications"),
        ("session", "org.icedtea.Session"),
        ("registry", "org.icedtea.Registry"),
    ];
    let mut writes = Vec::new();
    for (name, bus_name) in DAEMONS {
        let path = format!("/desktop/daemons/{name}");
        if reg.get_stored(&path).ok().flatten().is_none() {
            writes.push((
                path,
                record(&[
                    ("bus_name", Value::Str(bus_name.to_string())),
                    ("enabled", Value::Bool(true)),
                ]),
            ));
        }
    }
    if !writes.is_empty() {
        reg.set_many(&writes)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn registry() -> Registry {
        let store = icedtea_registry::Store::in_memory(crate::schema()).expect("store");
        Registry::direct(Arc::new(Mutex::new(store)))
    }

    fn write_desktop(dir: &Path, id: &str, body: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(format!("{id}.desktop")), body).unwrap();
    }

    #[test]
    fn exec_parsing_quotes_escapes_and_field_codes() {
        assert_eq!(parse_exec("firefox %u"), Some(vec!["firefox".to_string()]));
        assert_eq!(
            parse_exec("sh -c \"echo hi\" %f"),
            Some(vec!["sh".into(), "-c".into(), "echo hi".into()])
        );
        assert_eq!(
            parse_exec("app --path=/a\\ b"),
            Some(vec!["app".into(), "--path=/a b".into()])
        );
        assert_eq!(
            parse_exec("app 100%%"),
            Some(vec!["app".into(), "100%".into()])
        );
        assert_eq!(parse_exec(""), None);
        assert_eq!(parse_exec("\"unbalanced"), None);
    }

    #[test]
    fn desktop_parsing_reads_the_entry_group_only() {
        let text = "\
[Desktop Entry]
Type=Application
Name=Firefox
Name[fr]=Firefox FR
Icon=firefox
Exec=firefox %u
Categories=Network;WebBrowser;
MimeType=text/html;x-scheme-handler/http;
NoDisplay=false

[Desktop Action new]
Exec=firefox --new
";
        let group = parse_desktop_entry(text).unwrap();
        assert_eq!(group.name, "Firefox");
        assert_eq!(group.icon, "firefox");
        assert_eq!(group.exec, "firefox %u");
        assert_eq!(group.categories, vec!["Network", "WebBrowser"]);
        assert_eq!(group.mime_types, vec!["text/html", "x-scheme-handler/http"]);
        assert!(!group.no_display);
    }

    #[test]
    fn handler_targets_split_mime_and_scheme() {
        assert_eq!(
            handler_target("text/html"),
            Some(HandlerTarget::Mime("text/html".into()))
        );
        assert_eq!(
            handler_target("x-scheme-handler/http"),
            Some(HandlerTarget::Scheme("http".into()))
        );
        assert_eq!(handler_target("nonsense"), None);
    }

    #[test]
    fn globs2_parsing_skips_malformed_lines() {
        let text = "50:text/html:*.html\n# comment\nbadline\n60:image/png:image/*\n";
        let map = parse_globs2(text);
        assert_eq!(map.get("text/html"), Some(&vec!["*.html".to_string()]));
        assert_eq!(map.get("image/png"), Some(&vec!["image/*".to_string()]));
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn ingest_is_idempotent_and_writes_handlers() {
        let dir = tempfile::tempdir().unwrap();
        let apps = dir.path().join("applications");
        write_desktop(
            &apps,
            "firefox",
            "[Desktop Entry]\nType=Application\nName=Firefox\nExec=firefox %u\nMimeType=text/html;x-scheme-handler/http;\n",
        );
        let reg = registry();

        let first = ingest(&reg, std::slice::from_ref(&apps), &[]).unwrap();
        assert_eq!(first.apps, 1);
        assert!(first.handler_lists_changed >= 1);
        assert!(!first.unchanged);

        let seq_after_first = reg.seq().unwrap();
        let second = ingest(&reg, std::slice::from_ref(&apps), &[]).unwrap();
        assert!(second.unchanged, "second run writes nothing");
        assert_eq!(reg.seq().unwrap(), seq_after_first, "no seq bump");

        // The handler lists are populated.
        let (mime, _) = reg.get(&mime_handlers_path("text/html")).unwrap();
        let Value::RecordList(rows) = mime else {
            panic!("expected a handler list");
        };
        assert_eq!(row_str(&rows[0], "id").as_deref(), Some("firefox"));
        let (scheme, _) = reg
            .get(&icedtea_registry::scheme_handlers_path("http"))
            .unwrap();
        assert!(matches!(scheme, Value::RecordList(rows) if !rows.is_empty()));
    }

    #[test]
    fn a_user_default_is_not_reordered() {
        let dir = tempfile::tempdir().unwrap();
        let apps = dir.path().join("applications");
        write_desktop(
            &apps,
            "chromium",
            "[Desktop Entry]\nType=Application\nName=Chromium\nExec=chromium\nMimeType=text/html;\n",
        );
        write_desktop(
            &apps,
            "firefox",
            "[Desktop Entry]\nType=Application\nName=Firefox\nExec=firefox\nMimeType=text/html;\n",
        );
        let reg = registry();
        // The user picked chromium as the default (it sorts after firefox, so a
        // naive sort would move firefox first).
        reg.set(
            &mime_handlers_path("text/html"),
            &Value::RecordList(vec![match handler_record("chromium", "user") {
                Value::Record(r) => r,
                _ => unreachable!(),
            }]),
        )
        .unwrap();

        ingest(&reg, &[apps], &[]).unwrap();

        let (value, _) = reg.get(&mime_handlers_path("text/html")).unwrap();
        let Value::RecordList(rows) = value else {
            panic!("expected a handler list");
        };
        assert_eq!(
            row_str(&rows[0], "id").as_deref(),
            Some("chromium"),
            "the user's default stays first"
        );
        assert_eq!(row_str(&rows[0], "source").as_deref(), Some("user"));
        assert_eq!(row_str(&rows[1], "id").as_deref(), Some("firefox"));
    }

    #[test]
    fn a_removed_association_is_not_resurrected() {
        let dir = tempfile::tempdir().unwrap();
        let apps = dir.path().join("applications");
        write_desktop(
            &apps,
            "firefox",
            "[Desktop Entry]\nType=Application\nName=Firefox\nExec=firefox\nMimeType=text/html;\n",
        );
        let reg = registry();
        reg.set(
            &mime_removed_path("text/html"),
            &Value::StrList(vec!["firefox".into()]),
        )
        .unwrap();

        let firefox_absent = |reg: &Registry| -> bool {
            match reg.get(&mime_handlers_path("text/html")) {
                Ok((Value::RecordList(rows), _)) => rows
                    .iter()
                    .all(|r| row_str(r, "id").as_deref() != Some("firefox")),
                // No handler list at all also means firefox is not present.
                _ => true,
            }
        };

        ingest(&reg, std::slice::from_ref(&apps), &[]).unwrap();
        assert!(
            firefox_absent(&reg),
            "a removed association must not come back"
        );

        // And a second run still does not add it.
        ingest(&reg, std::slice::from_ref(&apps), &[]).unwrap();
        assert!(firefox_absent(&reg), "still suppressed on a re-scan");
    }

    #[test]
    fn autostart_entries_are_written() {
        let dir = tempfile::tempdir().unwrap();
        let autostart = dir.path().join("autostart");
        write_desktop(
            &autostart,
            "foo",
            "[Desktop Entry]\nType=Application\nName=Foo\nExec=foo --start\n",
        );
        let reg = registry();
        ingest(&reg, &[], &[autostart]).unwrap();
        let (value, _) = reg.get("/desktop/autostart/foo").unwrap();
        let Value::Record(row) = value else {
            panic!("expected a record");
        };
        assert_eq!(row_str(&row, "name").as_deref(), Some("Foo"));
        assert_eq!(row_str_list(&row, "exec"), vec!["foo", "--start"]);
        assert_eq!(row.get("enabled"), Some(&Value::Bool(true)));
    }

    #[test]
    fn the_daemon_registry_is_seeded_without_overwriting() {
        let reg = registry();
        // A user edit first.
        reg.set(
            "/desktop/daemons/clipboard",
            &record(&[("bus_name", Value::Str("custom.Clip".into()))]),
        )
        .unwrap();
        ingest(&reg, &[], &[]).unwrap();
        let (value, _) = reg.get("/desktop/daemons/clipboard").unwrap();
        let Value::Record(row) = value else {
            panic!("expected a record");
        };
        assert_eq!(
            row_str(&row, "bus_name").as_deref(),
            Some("custom.Clip"),
            "a user edit is not overwritten"
        );
        assert!(reg.get("/desktop/daemons/registry").is_ok());
    }
}
