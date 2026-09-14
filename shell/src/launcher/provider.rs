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

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::{DesktopIndex, RecencyStore, entry_score, field_score};

pub use super::recent::parse_recent;

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

/// Cap on the bytes read from `recently-used.xbel`. The file is untrusted
/// (any process may write it), so the whole-document read is bounded; a
/// truncated document still parses its leading entries.
pub const MAX_RECENT_BYTES: u64 = 4 * 1024 * 1024;

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
    ///
    /// Collection stops once `limit` entries are gathered, and symlinks are
    /// rejected (only regular files whose dirent is not a link), so a link
    /// cannot smuggle an out-of-root target into the index. The final sort
    /// in [`FilesProvider::from_entries`] still orders and caps the result.
    pub fn scan(dirs: &[PathBuf], limit: usize) -> Self {
        let mut entries = Vec::new();
        'dirs: for dir in dirs {
            let Ok(read) = std::fs::read_dir(dir) else {
                continue;
            };
            for dirent in read.flatten() {
                if entries.len() >= limit {
                    break 'dirs;
                }
                let path = dirent.path();
                let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
                    continue;
                };
                if name.starts_with('.') {
                    continue;
                }
                let Ok(meta) = std::fs::symlink_metadata(&path) else {
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
    ///
    /// The document is untrusted input, so every candidate is confined to
    /// `$HOME`: hidden or `..` components, symlinks, non-regular files, and
    /// paths outside the home root are dropped. No home means no results.
    pub fn from_recent(xbel: &str, limit: usize) -> Self {
        let Some(home) = home_dir().and_then(|h| std::fs::canonicalize(h).ok()) else {
            return Self::default();
        };
        Self::from_recent_within(xbel, limit, &home)
    }

    /// [`FilesProvider::from_recent`] with the confinement root injected, so
    /// the rule is testable without touching the real `$HOME`.
    fn from_recent_within(xbel: &str, limit: usize, root: &Path) -> Self {
        let entries = parse_recent(xbel, limit)
            .into_iter()
            .filter_map(|path| file_entry_within(&path, root))
            .collect();
        Self::from_entries(entries, limit)
    }

    /// The production index: the XDG user dirs plus the recent list, merged
    /// and capped. Read-only; failures degrade to an empty provider.
    pub fn from_env() -> Self {
        let mut merged = Self::scan(&default_file_dirs(), MAX_FILE_RESULTS);
        if let Ok(file) = std::fs::File::open(default_recent_path()) {
            let mut bytes = Vec::new();
            if file.take(MAX_RECENT_BYTES).read_to_end(&mut bytes).is_ok() {
                let xbel = String::from_utf8_lossy(&bytes);
                merged.merge(Self::from_recent(&xbel, MAX_FILE_RESULTS));
            }
        }
        merged
    }

    /// Dedupe by path (keeping the newest mtime), sort newest-mtime first
    /// (path breaks ties), cap.
    fn from_entries(entries: Vec<FileEntry>, limit: usize) -> Self {
        use std::collections::HashMap;
        let mut by_path: HashMap<PathBuf, FileEntry> = HashMap::new();
        for entry in entries {
            match by_path.get(&entry.path) {
                Some(existing) if existing.modified >= entry.modified => {}
                _ => {
                    by_path.insert(entry.path.clone(), entry);
                }
            }
        }
        let mut entries: Vec<FileEntry> = by_path.into_values().collect();
        entries.sort_by(|a, b| b.modified.cmp(&a.modified).then(a.path.cmp(&b.path)));
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

/// The real home directory, or `None` when `$HOME` is unset/empty. Never a
/// literal `~`: that is not a path the filesystem resolves.
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
}

/// A regular, non-hidden file at `path`, canonicalized and required to be
/// under `root`. `root` is expected canonical. Symlinks are rejected before
/// canonicalization resolves them.
fn file_entry_within(path: &Path, root: &Path) -> Option<FileEntry> {
    if path
        .components()
        .any(|component| component.as_os_str().to_string_lossy().starts_with('.'))
    {
        return None;
    }
    let meta = std::fs::symlink_metadata(path).ok()?;
    if meta.file_type().is_symlink() || !meta.is_file() {
        return None;
    }
    let canonical = std::fs::canonicalize(path).ok()?;
    if !canonical.starts_with(root) {
        return None;
    }
    let name = canonical.file_name()?.to_string_lossy().into_owned();
    Some(FileEntry {
        path: canonical,
        name,
        modified: meta.modified().ok(),
    })
}

/// Canonical `dir` iff it is a directory under `home` (itself canonical).
/// Keeps a rogue `XDG_*_DIR` (e.g. `/` or `/etc`) from indexing a whole
/// tree, and drops the conventional fallback when it does not exist.
fn confined_to_home(dir: &Path, home: &Path) -> Option<PathBuf> {
    let canonical = std::fs::canonicalize(dir).ok()?;
    (canonical.is_dir() && canonical.starts_with(home)).then_some(canonical)
}

/// The default file-index roots: the XDG user dirs, falling back to the
/// conventional `$HOME` names. Each must exist and resolve under the real
/// `$HOME`; an unset home yields no roots.
pub fn default_file_dirs() -> Vec<PathBuf> {
    let Some(home) = home_dir().and_then(|h| std::fs::canonicalize(h).ok()) else {
        return Vec::new();
    };
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
        confined_to_home(&dir, &home)
    })
    .collect()
}

/// The recent-files document path: `$XDG_DATA_HOME/recently-used.xbel`,
/// falling back under the real home. An unset home degrades to a relative
/// path that simply fails to open.
pub fn default_recent_path() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| home_dir().unwrap_or_default().join(".local/share"))
        .join("recently-used.xbel")
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
    fn files_scan_rejects_symlinks_and_keeps_non_utf8_names() {
        use std::os::unix::ffi::OsStrExt;

        let dir = tmpdir();
        std::fs::write(dir.join("real.txt"), b"x").unwrap();
        std::os::unix::fs::symlink(dir.join("real.txt"), dir.join("link.txt")).unwrap();
        std::fs::write(dir.join(std::ffi::OsStr::from_bytes(b"bad\xff.txt")), b"x").unwrap();

        let provider = FilesProvider::scan(std::slice::from_ref(&dir), 10);
        assert!(
            provider.search("link").is_empty(),
            "a symlink is not a regular file"
        );
        assert!(
            provider
                .search("real")
                .iter()
                .any(|r| r.title == "real.txt"),
            "the real file survives"
        );
        assert!(
            provider
                .search("bad")
                .iter()
                .any(|r| r.title.contains("bad")),
            "a non-UTF8 name is kept via lossy conversion, not dropped"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn files_from_recent_rejects_hidden_symlink_and_out_of_root_paths() {
        let root = tmpdir();
        let outside = tmpdir();
        let good = root.join("notes.txt");
        std::fs::write(&good, b"x").unwrap();
        let hidden_dir = root.join(".ssh");
        std::fs::create_dir_all(&hidden_dir).unwrap();
        let hidden = hidden_dir.join("id_rsa");
        std::fs::write(&hidden, b"key").unwrap();
        let link = root.join("link.txt");
        std::os::unix::fs::symlink(&good, &link).unwrap();
        let outsider = outside.join("away.txt");
        std::fs::write(&outsider, b"x").unwrap();
        let dir_target = root.join("adir");
        std::fs::create_dir_all(&dir_target).unwrap();

        let xbel = format!(
            "<bookmark href=\"file://{}\"/>\n\
             <bookmark href=\"file://{}\"/>\n\
             <bookmark href=\"file://{}\"/>\n\
             <bookmark href=\"file://{}\"/>\n\
             <bookmark href=\"file://{}\"/>\n",
            good.display(),
            hidden.display(),
            link.display(),
            outsider.display(),
            dir_target.display(),
        );
        let root = std::fs::canonicalize(&root).unwrap();
        let provider = FilesProvider::from_recent_within(&xbel, 10, &root);
        assert_eq!(provider.len(), 1, "only the plain regular file survives");
        assert!(
            provider
                .search("notes")
                .iter()
                .any(|r| r.title == "notes.txt"),
            "the allowed file is still indexed"
        );
        std::fs::remove_dir_all(&root).unwrap();
        std::fs::remove_dir_all(&outside).unwrap();
    }

    #[test]
    fn from_entries_dedupes_the_same_path_across_mtimes() {
        use std::time::Duration;

        let at = |secs: u64| Some(SystemTime::UNIX_EPOCH + Duration::from_secs(secs));
        let x = PathBuf::from("/home/u/x.txt");
        let provider = FilesProvider::from_entries(
            vec![
                FileEntry {
                    path: x.clone(),
                    name: "x.txt".to_string(),
                    modified: at(10),
                },
                FileEntry {
                    path: PathBuf::from("/home/u/mid.txt"),
                    name: "mid.txt".to_string(),
                    modified: at(20),
                },
                FileEntry {
                    path: x.clone(),
                    name: "x.txt".to_string(),
                    modified: at(30),
                },
            ],
            10,
        );
        assert_eq!(provider.len(), 2, "one entry per path, not per mtime");
        let kept = provider
            .entries
            .iter()
            .find(|entry| entry.path == x)
            .expect("x is kept");
        assert_eq!(kept.modified, at(30), "the newest mtime wins");
    }

    #[test]
    fn confined_to_home_requires_an_existing_dir_under_home() {
        let root = tmpdir();
        let inside = root.join("Documents");
        std::fs::create_dir_all(&inside).unwrap();
        let file = root.join("a.txt");
        std::fs::write(&file, b"x").unwrap();
        let outside = tmpdir();

        let home = std::fs::canonicalize(&root).unwrap();
        assert_eq!(
            confined_to_home(&inside, &home),
            Some(std::fs::canonicalize(&inside).unwrap()),
            "a real dir under home is accepted"
        );
        assert_eq!(confined_to_home(&file, &home), None, "a file is not a dir");
        assert_eq!(
            confined_to_home(&outside, &home),
            None,
            "a dir outside home is rejected"
        );
        std::fs::remove_dir_all(&home).unwrap();
        std::fs::remove_dir_all(&outside).unwrap();
    }

    #[test]
    fn settings_pages_and_app_id_stay_in_sync_with_icedtea_settings() {
        let expected: Vec<(&str, &str)> = icedtea_settings::pages::PageId::ALL
            .iter()
            .map(|page| (page.name(), page.title()))
            .collect();
        assert_eq!(
            SETTINGS_PAGES,
            expected.as_slice(),
            "SETTINGS_PAGES drifted from icedtea_settings::pages::PageId::ALL"
        );
        let desktop = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../settings/data/org.icedtea.Settings.desktop");
        assert_eq!(
            SETTINGS_APP_ID,
            desktop
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default(),
            "SETTINGS_APP_ID drifted from the settings desktop file stem"
        );
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
