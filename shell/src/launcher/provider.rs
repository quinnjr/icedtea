//! The launcher's search-provider seam: one ranked list from many sources.
//!
//! Apps, files and settings each implement [`SearchProvider`] and return
//! [`SearchResult`]s; [`rank_results`] merges them with one deterministic
//! total order. The pure core keeps no I/O on the hot path: the files
//! provider is built once per open ([`FilesProvider::scan`] /
//! [`FilesProvider::from_recent`]) and `search` only reads the cached index.
//!
//! No Wayland and no D-Bus here: selecting a result is the view's job, and
//! the compositor spawn path is app-id-only (see the B1 spec's "Deferred
//! work").

use std::path::PathBuf;
use std::time::SystemTime;

use super::{DesktopIndex, RecencyStore, entry_score, field_score};

/// The settings app id the launcher asks the compositor to spawn. Mirrors
/// `settings/src/main.rs`'s `APP_ID` and the desktop file's stem.
pub const SETTINGS_APP_ID: &str = "org.icedtea.Settings";

/// The settings pages offered as search results, in switcher order.
///
/// SYNC: mirrors `icedtea_settings::pages::PageId::ALL` (name, title). The
/// shell crate does not depend on `icedtea-settings`, so the list is
/// duplicated here; keep both in step when a page is added.
pub const SETTINGS_PAGES: &[(&str, &str)] = &[
    ("appearance", "Appearance"),
    ("behavior", "Behavior"),
    ("workspaces", "Workspaces"),
    ("keybindings", "Keybindings"),
    ("displays", "Displays"),
];

/// Cap on files one [`FilesProvider`] holds (and can return). Keeps the
/// per-open scan bounded on a large home directory.
pub const MAX_FILE_RESULTS: usize = 200;

/// Which provider a result came from. Declaration order is the tie-rank
/// [`rank_results`] applies on an exact score/weight tie: apps, then
/// settings, then files.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ProviderKind {
    /// A `.desktop` app from the index.
    App,
    /// A settings page.
    Settings,
    /// A file from a bounded user-dir/recent index.
    File,
}

/// One ranked search result, independent of the pane that renders it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchResult {
    /// The originating provider.
    pub kind: ProviderKind,
    /// Launch key: an app id, a settings page name, or an absolute file path.
    pub id: String,
    /// Display name.
    pub title: String,
    /// Secondary line (the exec, "Settings", the parent dir).
    pub subtitle: Option<String>,
    /// Matcher score (0..=300).
    pub score: u32,
    /// Tie weight: apps use [`RecencyStore::adaptive_score`]; files use
    /// newest-first rank; settings use 0.
    pub weight: u64,
}

/// A source of ranked search results.
pub trait SearchProvider {
    /// Which provider this is.
    fn kind(&self) -> ProviderKind;

    /// Results for `query`, unranked across providers. An empty query
    /// returns nothing: the launcher's own empty-query view is app-only.
    fn search(&self, query: &str) -> Vec<SearchResult>;
}

/// Merge and rank results from every provider: score desc, weight desc,
/// kind tie-rank ([`ProviderKind`] order), case-insensitive title, id.
pub fn rank_results(mut results: Vec<SearchResult>) -> Vec<SearchResult> {
    results.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then(b.weight.cmp(&a.weight))
            .then(a.kind.cmp(&b.kind))
            .then(a.title.to_lowercase().cmp(&b.title.to_lowercase()))
            .then(a.id.cmp(&b.id))
    });
    results
}

/// Apps from the `.desktop` index, scored by [`entry_score`] and weighted
/// by [`RecencyStore::adaptive_score`].
pub struct AppsProvider<'a> {
    /// The scanned app index.
    pub index: &'a DesktopIndex,
    /// Launch recency/frequency for the adaptive weight.
    pub recency: &'a RecencyStore,
}

impl SearchProvider for AppsProvider<'_> {
    fn kind(&self) -> ProviderKind {
        ProviderKind::App
    }

    fn search(&self, query: &str) -> Vec<SearchResult> {
        let normalized = query.trim().to_lowercase();
        if normalized.is_empty() {
            return Vec::new();
        }
        self.index
            .apps()
            .iter()
            .filter_map(|app| {
                let score = entry_score(app, &normalized);
                (score > 0).then(|| SearchResult {
                    kind: ProviderKind::App,
                    id: app.id.clone(),
                    title: app.name.clone(),
                    subtitle: Some(app.display_exec()),
                    score,
                    weight: self.recency.adaptive_score(&app.id),
                })
            })
            .collect()
    }
}

/// The settings pages, matched by title, page name, or the word "settings".
pub struct SettingsProvider;

impl SearchProvider for SettingsProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Settings
    }

    fn search(&self, query: &str) -> Vec<SearchResult> {
        let normalized = query.trim().to_lowercase();
        if normalized.is_empty() {
            return Vec::new();
        }
        let generic = field_score("settings", &normalized);
        SETTINGS_PAGES
            .iter()
            .filter_map(|(name, title)| {
                let mut score = field_score(&title.to_lowercase(), &normalized);
                score = score.max(field_score(name, &normalized));
                if score == 0 && generic > 0 {
                    score = 25;
                }
                (score > 0).then(|| SearchResult {
                    kind: ProviderKind::Settings,
                    id: (*name).to_string(),
                    title: (*title).to_string(),
                    subtitle: Some("Settings".to_string()),
                    score,
                    weight: 0,
                })
            })
            .collect()
    }
}

/// One file the provider can offer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    /// Absolute path.
    pub path: PathBuf,
    /// File name (last path component).
    pub name: String,
    /// Last modification time, when the platform reports one.
    pub modified: Option<SystemTime>,
}

/// A bounded, read-only index of files from the user's XDG dirs and the
/// recent-files list. `search` is pure over the cached entries.
#[derive(Debug, Clone, Default)]
pub struct FilesProvider {
    entries: Vec<FileEntry>,
}

impl FilesProvider {
    /// Index every regular, non-hidden file directly under `dirs`, newest
    /// first, capped at `limit`. Unreadable dirs and entries are skipped.
    pub fn scan(dirs: &[PathBuf], limit: usize) -> Self {
        let mut entries = Vec::new();
        for dir in dirs {
            let Ok(read) = std::fs::read_dir(dir) else {
                continue;
            };
            for dirent in read.flatten() {
                let path = dirent.path();
                let Some(name) = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(str::to_string)
                else {
                    continue;
                };
                if name.starts_with('.') {
                    continue;
                }
                let Ok(meta) = dirent.metadata() else {
                    continue;
                };
                if !meta.is_file() {
                    continue;
                }
                entries.push(FileEntry {
                    path,
                    name,
                    modified: meta.modified().ok(),
                });
            }
        }
        Self::from_entries(entries, limit)
    }

    /// Index the paths in a recent-files (`recently-used.xbel`) document,
    /// newest first as the document lists them, capped at `limit`.
    pub fn from_recent(xbel: &str, limit: usize) -> Self {
        let entries = parse_recent(xbel, limit)
            .into_iter()
            .filter_map(|path| {
                let name = path.file_name()?.to_str()?.to_string();
                let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
                Some(FileEntry {
                    path,
                    name,
                    modified,
                })
            })
            .collect();
        Self::from_entries(entries, limit)
    }

    /// The production index: the XDG user dirs plus the recent list, merged
    /// and capped. Read-only; failures degrade to an empty provider.
    pub fn from_env() -> Self {
        let mut merged = Self::scan(&default_file_dirs(), MAX_FILE_RESULTS);
        if let Ok(xbel) = std::fs::read_to_string(default_recent_path()) {
            merged.merge(Self::from_recent(&xbel, MAX_FILE_RESULTS));
        }
        merged
    }

    /// Dedupe by path, sort newest-mtime first (path breaks ties), cap.
    fn from_entries(mut entries: Vec<FileEntry>, limit: usize) -> Self {
        entries.sort_by(|a, b| b.modified.cmp(&a.modified).then(a.path.cmp(&b.path)));
        entries.dedup_by(|a, b| a.path == b.path);
        entries.truncate(limit);
        Self { entries }
    }

    fn merge(&mut self, other: Self) {
        let mut all = self.entries.clone();
        all.extend(other.entries);
        *self = Self::from_entries(all, MAX_FILE_RESULTS);
    }

    /// How many files are indexed.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when nothing is indexed.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl SearchProvider for FilesProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::File
    }

    fn search(&self, query: &str) -> Vec<SearchResult> {
        let normalized = query.trim().to_lowercase();
        if normalized.is_empty() {
            return Vec::new();
        }
        let total = self.entries.len();
        self.entries
            .iter()
            .enumerate()
            .filter_map(|(rank, file)| {
                let mut score = field_score(&file.name.to_lowercase(), &normalized);
                if score == 0 {
                    score = field_score(&file.path.to_string_lossy().to_lowercase(), &normalized);
                }
                (score > 0).then(|| SearchResult {
                    kind: ProviderKind::File,
                    id: file.path.to_string_lossy().into_owned(),
                    title: file.name.clone(),
                    subtitle: file.path.parent().map(|p| p.display().to_string()),
                    score,
                    weight: u64::try_from(total - rank).unwrap_or(u64::MAX),
                })
            })
            .collect()
    }
}

/// The default file-index roots: the XDG user dirs, falling back to the
/// conventional `$HOME` names, keeping only those that exist.
pub fn default_file_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("~"));
    [
        ("XDG_DESKTOP_DIR", "Desktop"),
        ("XDG_DOCUMENTS_DIR", "Documents"),
        ("XDG_DOWNLOAD_DIR", "Downloads"),
    ]
    .into_iter()
    .filter_map(|(var, fallback)| {
        let dir = std::env::var_os(var)
            .map(PathBuf::from)
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| home.join(fallback));
        dir.is_dir().then_some(dir)
    })
    .collect()
}

/// The recent-files document path: `$XDG_DATA_HOME/recently-used.xbel`.
pub fn default_recent_path() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("~"));
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| home.join(".local/share"))
        .join("recently-used.xbel")
}

/// Extract up to `limit` local paths from a `recently-used.xbel`, in
/// document order (newest first in the real file). Non-`file://` schemes,
/// malformed escapes and duplicates are dropped.
pub fn parse_recent(xbel: &str, limit: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for line in xbel.lines() {
        if out.len() >= limit {
            break;
        }
        let Some(start) = line.find("href=\"") else {
            continue;
        };
        let rest = &line[start + "href=\"".len()..];
        let Some(end) = rest.find('"') else {
            continue;
        };
        let Some(path) = decode_file_uri(&rest[..end]) else {
            continue;
        };
        if !out.contains(&path) {
            out.push(path);
        }
    }
    out
}

/// Decode a `file://` URI to a local path. `None` for any other scheme, a
/// non-localhost authority, a malformed escape, non-UTF-8 bytes, or a NUL.
fn decode_file_uri(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let path_part = match rest.find('/') {
        Some(0) => rest,
        Some(slash) if &rest[..slash] == "localhost" => &rest[slash..],
        _ => return None,
    };
    let bytes = percent_decode(path_part)?;
    if bytes.contains(&0) {
        return None;
    }
    let decoded = String::from_utf8(bytes).ok()?;
    (!decoded.is_empty()).then(|| PathBuf::from(decoded))
}

/// `%XX` decoding. `None` on a truncated or non-hex escape.
fn percent_decode(s: &str) -> Option<Vec<u8>> {
    let raw = s.as_bytes();
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'%' {
            let hex = raw.get(i + 1..i + 3)?;
            let text = std::str::from_utf8(hex).ok()?;
            out.push(u8::from_str_radix(text, 16).ok()?);
            i += 3;
        } else {
            out.push(raw[i]);
            i += 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn tmpdir() -> PathBuf {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "icedtea-files-provider-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn parse_recent_decodes_local_uris_and_skips_other_schemes() {
        let xbel = r#"<?xml version="1.0"?>
<bookmarks>
  <bookmark href="file:///home/u/My%20Docs/notes.txt" modified="2026-01-01T00:00:00Z"/>
  <bookmark href="https://example.com/page"/>
  <bookmark href="file:///home/u/a.txt"/>
  <bookmark href="file:///home/u/%ZZ.txt"/>
  <bookmark href="file:///home/u/a.txt"/>
</bookmarks>"#;
        let paths = parse_recent(xbel, 10);
        assert_eq!(
            paths,
            vec![
                PathBuf::from("/home/u/My Docs/notes.txt"),
                PathBuf::from("/home/u/a.txt"),
            ],
            "decoded, deduped, non-file schemes and bad escapes dropped"
        );
    }

    #[test]
    fn parse_recent_caps_at_the_limit() {
        let mut xbel = String::new();
        for i in 0..10 {
            xbel.push_str(&format!("<bookmark href=\"file:///tmp/f{i}\"/>\n"));
        }
        assert_eq!(parse_recent(&xbel, 3).len(), 3);
    }

    #[test]
    fn files_scan_is_bounded_and_ignores_hidden_and_dirs() {
        let dir = tmpdir();
        for i in 0..5 {
            std::fs::write(dir.join(format!("file{i}.txt")), b"x").unwrap();
        }
        std::fs::write(dir.join(".hidden"), b"x").unwrap();
        std::fs::create_dir_all(dir.join("subdir")).unwrap();
        let provider = FilesProvider::scan(std::slice::from_ref(&dir), 3);
        assert_eq!(provider.len(), 3, "the limit caps the index");
        assert!(
            !provider
                .search("hidden")
                .iter()
                .any(|r| r.title == ".hidden"),
            "hidden files stay out"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn files_search_matches_name_and_path_and_ranks_newer_first() {
        use std::time::Duration;

        let provider = FilesProvider::from_entries(
            vec![
                FileEntry {
                    path: PathBuf::from("/home/u/old-report.txt"),
                    name: "old-report.txt".to_string(),
                    modified: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(10)),
                },
                FileEntry {
                    path: PathBuf::from("/home/u/report.txt"),
                    name: "report.txt".to_string(),
                    modified: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(20)),
                },
            ],
            10,
        );
        let hits = provider.search("report");
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].title, "report.txt", "newest first by weight");
        assert!(hits[0].weight > hits[1].weight);
        assert_eq!(hits[0].kind, ProviderKind::File);
        assert!(hits[0].id.ends_with("report.txt"));
        // A path-component query matches even when the file name does not.
        let by_dir = provider.search("home");
        assert_eq!(by_dir.len(), 2, "the directory component is searchable");
    }

    #[test]
    fn files_search_empty_query_returns_nothing() {
        let provider = FilesProvider {
            entries: vec![FileEntry {
                path: PathBuf::from("/tmp/a"),
                name: "a".to_string(),
                modified: None,
            }],
        };
        assert!(provider.search("   ").is_empty());
    }

    #[test]
    fn settings_search_matches_titles_names_and_the_generic_word() {
        let settings = SettingsProvider;
        assert_eq!(settings.search("display")[0].id, "displays", "title match");
        assert_eq!(
            settings.search("keybind")[0].id,
            "keybindings",
            "partial title match"
        );
        assert_eq!(
            settings.search("settings").len(),
            SETTINGS_PAGES.len(),
            "the generic word reaches every page"
        );
        assert!(settings.search("zzzz").is_empty());
        assert_eq!(
            settings.search("appearance")[0].kind,
            ProviderKind::Settings
        );
    }

    #[test]
    fn apps_search_scores_and_weights_from_the_index() {
        use super::super::{DesktopEntry, Matcher};

        let index = DesktopIndex::from_entries(vec![
            DesktopEntry {
                id: "firefox".to_string(),
                name: "Firefox".to_string(),
                exec: "firefox".to_string(),
                icon: None,
                categories: Vec::new(),
                keywords: Vec::new(),
                nodisplay: false,
                only_show_in: Vec::new(),
                not_show_in: Vec::new(),
            },
            DesktopEntry {
                id: "firetrap".to_string(),
                name: "Fire Trap".to_string(),
                exec: "firetrap".to_string(),
                icon: None,
                categories: Vec::new(),
                keywords: Vec::new(),
                nodisplay: false,
                only_show_in: Vec::new(),
                not_show_in: Vec::new(),
            },
        ]);
        let mut recency = RecencyStore::default();
        recency.record("firetrap");
        let provider = AppsProvider {
            index: &index,
            recency: &recency,
        };
        let results = rank_results(provider.search("fire"));
        assert_eq!(results.len(), 2);
        // Both prefix-match; the adaptive weight lifts the launched one.
        assert_eq!(results[0].id, "firetrap");
        // And the provider agrees with the matcher over the same data.
        let matcher_top = Matcher::new(&index).with_recency(&recency).rank("fire");
        assert_eq!(matcher_top[0].id, results[0].id);
    }

    #[test]
    fn rank_results_pins_the_cross_provider_order() {
        let results = vec![
            SearchResult {
                kind: ProviderKind::File,
                id: "file".to_string(),
                title: "Zebra".to_string(),
                subtitle: None,
                score: 200,
                weight: 1,
            },
            SearchResult {
                kind: ProviderKind::App,
                id: "app".to_string(),
                title: "Zebra".to_string(),
                subtitle: None,
                score: 200,
                weight: 1,
            },
            SearchResult {
                kind: ProviderKind::Settings,
                id: "settings".to_string(),
                title: "Zebra".to_string(),
                subtitle: None,
                score: 200,
                weight: 1,
            },
            SearchResult {
                kind: ProviderKind::File,
                id: "stronger".to_string(),
                title: "Zebra".to_string(),
                subtitle: None,
                score: 300,
                weight: 0,
            },
        ];
        let ranked = rank_results(results);
        let ids: Vec<&str> = ranked.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["stronger", "app", "settings", "file"],
            "score, then kind rank (App, Settings, File), then title/id"
        );
    }

    #[test]
    fn default_file_dirs_keeps_only_existing_dirs() {
        // Hermetic: clear the XDG vars so the conventional fallbacks apply,
        // then it must be a subset of $HOME and only existing dirs.
        for dir in default_file_dirs() {
            assert!(dir.is_dir(), "only existing dirs are offered");
        }
    }
}
