//! Pure launcher core: `.desktop` index, fuzzy matcher, pin/tile/recency stores.
//!
//! No Wayland imports here by design — the view renders what this module
//! returns, and every behavior is unit-testable without a compositor.

pub mod entry;
pub mod provider;
pub mod recent;

pub use entry::{
    DesktopEntry, Locale, parse_entry, parse_entry_with_id, parse_entry_with_id_and_locale,
};
pub use provider::{
    AppsProvider, FileEntry, FilesProvider, ProviderKind, SearchProvider, SearchResult,
    SettingsProvider, default_file_dirs, rank_results,
};

use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use icedtea_registry_schema::{TileGroup, default_tile_size};

/// Desktop name this launcher shows up as for `OnlyShowIn`/`NotShowIn`.
// SYNC: mirrored in compositor/src/state.rs; launcher_parity test is the sync gate.
pub const SHOW_IN_ENV: &str = "icedtea";

/// Default XDG application directories scanned for `.desktop` files, in
/// precedence order: the user dir first, then the system dir.
/// [`DesktopIndex::scan`] keeps the first dir's copy of a duplicated app id,
/// so a user override shadows the system entry (same order as the
/// compositor's lookup).
pub fn default_dirs() -> Vec<PathBuf> {
    let user = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("~"))
        .join(".local/share/applications");
    vec![user, PathBuf::from("/usr/share/applications")]
}

/// Cap on distinct apps tracked by [`RecencyStore`]; oldest prune first.
pub const MAX_RECENCY_ENTRIES: usize = 200;

/// Half-life-style window for [`RecencyStore::adaptive_score`]: an entry
/// `DECAY_WINDOW` launches old scores about half of a fresh one of the same
/// launch count. Fixed and integer-only so the ordering is deterministic.
pub const DECAY_WINDOW: u64 = 32;

/// Every `*.desktop` path under `dirs`, in dir order. Unreadable dirs are
/// skipped, never fatal; `read_dir` errors on individual entries are
/// flattened away and only the `.desktop` extension is kept.
///
/// Split out (not inlined in [`DesktopIndex::scan`]) so the fingerprint
/// half of the contract can hash exactly the file set a scan would read.
pub(crate) fn desktop_paths(dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for dir in dirs {
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) == Some("desktop") {
                out.push(path);
            }
        }
    }
    out
}

/// Index of visible desktop entries.
#[derive(Debug, Default)]
pub struct DesktopIndex {
    apps: Vec<DesktopEntry>,
}

impl DesktopIndex {
    /// Build an index, keeping only entries visible in [`SHOW_IN_ENV`].
    pub fn from_entries(entries: Vec<DesktopEntry>) -> Self {
        Self {
            apps: entries
                .into_iter()
                .filter(|entry| entry.visible_in(SHOW_IN_ENV))
                .collect(),
        }
    }

    /// Scan `dirs` for `*.desktop` files and return the visible entries,
    /// sorted by app id for a deterministic order. Unreadable files and
    /// unparseable content are skipped, never fatal. An app id appearing in
    /// more than one dir is kept once — the first dir in `dirs` order wins
    /// (the user dir precedes the system dir in [`default_dirs`], so the
    /// user entry shadows the system's), matching the first-hit-wins
    /// compositor lookup.
    pub fn scan(dirs: &[PathBuf]) -> Vec<DesktopEntry> {
        use std::collections::HashSet;
        let mut seen: HashSet<String> = HashSet::new();
        let mut apps = Vec::new();
        for path in desktop_paths(dirs) {
            let text = match std::fs::read_to_string(&path) {
                Ok(text) => text,
                Err(_) => continue,
            };
            let id = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("");
            if id.is_empty() || seen.contains(id) {
                continue;
            }
            let visible =
                parse_entry_with_id(id, &text).filter(|entry| entry.visible_in(SHOW_IN_ENV));
            if let Some(entry) = visible {
                seen.insert(id.to_string());
                apps.push(entry);
            }
        }
        apps.sort_by(|a, b| a.id.cmp(&b.id));
        apps
    }

    /// Visible entries in index order.
    pub fn apps(&self) -> &[DesktopEntry] {
        &self.apps
    }

    /// Look up one entry by app id.
    pub fn find(&self, id: &str) -> Option<&DesktopEntry> {
        self.apps.iter().find(|entry| entry.id == id)
    }
}

/// Launch-frequency/recency counts: app id → (launch count, last-seen seq).
/// Capped at [`MAX_RECENCY_ENTRIES`] distinct ids; the stalest prune first.
#[derive(Debug, Default)]
pub struct RecencyStore {
    counts: HashMap<String, (u64, u64)>,
    seq: u64,
}

impl RecencyStore {
    /// Record one successful launch of `app_id`, pruning the stalest
    /// (least-recently-recorded) ids past [`MAX_RECENCY_ENTRIES`].
    pub fn record(&mut self, app_id: &str) {
        self.seq = self.seq.saturating_add(1);
        let seen = self.seq;
        let (count, _) = self.counts.get(app_id).copied().unwrap_or((0, 0));
        self.counts
            .insert(app_id.to_string(), (count.saturating_add(1), seen));
        while self.counts.len() > MAX_RECENCY_ENTRIES {
            let Some(stalest) = self
                .counts
                .iter()
                .min_by_key(|(_, (_, seen))| *seen)
                .map(|(id, _)| id.clone())
            else {
                break;
            };
            self.counts.remove(&stalest);
        }
    }

    /// Restore persisted `(count, last-seen)` pairs (config seeding).
    /// Later [`RecencyStore::record`] calls keep the restored counts and
    /// continue the sequence past the restored maximum, so ranking order
    /// survives a restart.
    pub fn restore(&mut self, entries: &HashMap<String, (u64, u64)>) {
        for (id, (count, seen)) in entries {
            self.counts.insert(id.clone(), (*count, *seen));
            self.seq = self.seq.max(*seen);
        }
    }

    /// The whole `(count, last-seen)` map: what the launcher's write-back
    /// persists through `Config::save`, and what [`RecencyStore::restore`]
    /// reads back on the next open.
    pub fn snapshot(&self) -> HashMap<String, (u64, u64)> {
        self.counts.clone()
    }

    /// Launch count for `app_id` (0 when never recorded).
    pub fn count(&self, app_id: &str) -> u64 {
        self.counts
            .get(app_id)
            .map(|(count, _)| *count)
            .unwrap_or(0)
    }

    /// Adaptive ordering weight for `app_id`: launch frequency decayed by
    /// staleness, the ordering signal beyond the raw count tiebreak.
    ///
    /// Formula (integer-only, pure, deterministic):
    ///
    /// ```text
    /// age    = seq - last_seen            (saturating)
    /// weight = count * (DECAY_WINDOW + 1) / (DECAY_WINDOW + age)
    /// ```
    ///
    /// A just-used entry scores its count; an entry `DECAY_WINDOW` launches
    /// old scores about half; a never-recorded id scores 0. Frequency raises
    /// the weight, staleness lowers it, so a frequent recent app outranks an
    /// equally-matching stale one.
    pub fn adaptive_score(&self, app_id: &str) -> u64 {
        let Some((count, seen)) = self.counts.get(app_id) else {
            return 0;
        };
        let age = self.seq.saturating_sub(*seen);
        count.saturating_mul(DECAY_WINDOW + 1) / (DECAY_WINDOW + age)
    }

    /// Number of distinct apps currently tracked.
    pub fn len(&self) -> usize {
        self.counts.len()
    }

    /// True when no launch has been recorded.
    pub fn is_empty(&self) -> bool {
        self.counts.is_empty()
    }
}

/// Fuzzy matcher over one index.
///
/// Scoring per field (name, each keyword, exec basename), best field wins:
/// exact 300 > prefix 200 > word-boundary 100 > substring 25.
/// Ties break on the adaptive recency weight
/// ([`RecencyStore::adaptive_score`]), then case-insensitive name, then id.
#[derive(Debug, Clone, Copy)]
pub struct Matcher<'a> {
    index: &'a DesktopIndex,
    recency: Option<&'a RecencyStore>,
}

impl<'a> Matcher<'a> {
    /// Match against `index` with no recency signal.
    pub fn new(index: &'a DesktopIndex) -> Self {
        Self {
            index,
            recency: None,
        }
    }

    /// Add the recency store used as a ranking tiebreak.
    pub fn with_recency(mut self, recency: &'a RecencyStore) -> Self {
        self.recency = Some(recency);
        self
    }

    /// Ranked matches for `query`. Empty/blank query returns every app in
    /// index order; queries with no match return an empty vec.
    pub fn rank(&self, query: &str) -> Vec<&'a DesktopEntry> {
        let normalized = query.trim().to_lowercase();
        if normalized.is_empty() {
            return self.index.apps.iter().collect();
        }
        let mut scored: Vec<(u32, u64, &DesktopEntry)> = Vec::new();
        for app in &self.index.apps {
            let best = entry_score(app, &normalized);
            if best > 0 {
                let recency = self
                    .recency
                    .map(|store| store.adaptive_score(&app.id))
                    .unwrap_or(0);
                scored.push((best, recency, app));
            }
        }
        scored.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then(b.1.cmp(&a.1))
                .then(a.2.name.to_lowercase().cmp(&b.2.name.to_lowercase()))
                .then(a.2.id.cmp(&b.2.id))
        });
        scored.into_iter().map(|(_, _, app)| app).collect()
    }
}

/// Score one lowercased field against a lowercased query:
/// exact 300 > prefix 200 > word-boundary 100 > substring 25 > no hit 0.
/// Words break on any non-alphanumeric character.
fn field_score(field: &str, query: &str) -> u32 {
    if field == query {
        300
    } else if field.starts_with(query) {
        200
    } else if field
        .split(|c: char| !c.is_alphanumeric())
        .any(|word| !word.is_empty() && word.starts_with(query))
    {
        100
    } else if field.contains(query) {
        25
    } else {
        0
    }
}

/// Best field score for one entry against a lowercased, non-empty query:
/// the maximum of the display name, every keyword, and the exec basename.
/// Shared by [`Matcher`] and the provider seam so app scoring stays one rule.
pub(crate) fn entry_score(entry: &DesktopEntry, normalized: &str) -> u32 {
    let mut best = field_score(&entry.name.to_lowercase(), normalized);
    for keyword in &entry.keywords {
        best = best.max(field_score(&keyword.to_lowercase(), normalized));
    }
    let basename = entry.exec_basename().to_lowercase();
    if !basename.is_empty() {
        best = best.max(field_score(&basename, normalized));
    }
    best
}

/// Rank `query` against `index` with no recency signal.
pub fn rank<'a>(index: &'a DesktopIndex, query: &str) -> Vec<&'a DesktopEntry> {
    Matcher::new(index).rank(query)
}

/// Ordered pinned app-id list.
#[derive(Debug, Default, Clone)]
pub struct PinStore {
    pinned: Vec<String>,
}

impl PinStore {
    /// Pin `app_id`. Already-pinned ids keep their position.
    pub fn pin(&mut self, app_id: &str) {
        if !self.is_pinned(app_id) {
            self.pinned.push(app_id.to_string());
        }
    }

    /// Unpin `app_id`. Unknown ids are a no-op.
    pub fn unpin(&mut self, app_id: &str) {
        self.pinned.retain(|pinned| pinned != app_id);
    }

    /// True when `app_id` is pinned.
    pub fn is_pinned(&self, app_id: &str) -> bool {
        self.pinned.iter().any(|pinned| pinned == app_id)
    }

    /// Pinned ids in pin order.
    pub fn pinned(&self) -> &[String] {
        &self.pinned
    }
}

/// Named tile groups (ordered) with ordered member ids.
///
/// The group shape is `icedtea_registry_schema::TileGroup` (single owner: the config
/// crate persists it, this core operates on it).
#[derive(Debug, Default, Clone)]
pub struct TileStore {
    groups: Vec<TileGroup>,
}

impl TileStore {
    /// Groups in order.
    pub fn groups(&self) -> &[TileGroup] {
        &self.groups
    }

    /// Assign `app_id` to `group`, creating the group at the end when it is
    /// new. Already-assigned ids keep their position.
    pub fn assign(&mut self, group: &str, app_id: &str) {
        if let Some(slot) = self.groups.iter_mut().find(|slot| slot.name == group) {
            if !slot.ids.iter().any(|id| id == app_id) {
                slot.ids.push(app_id.to_string());
            }
            return;
        }
        self.groups.push(TileGroup {
            name: group.to_string(),
            ids: vec![app_id.to_string()],
            size: default_tile_size(),
        });
    }

    /// Restore one persisted group whole (config seeding): replaces any
    /// group of the same name, keeping its stored tile `size`.
    pub fn ingest(&mut self, group: &str, ids: &[String], size: u32) {
        if let Some(slot) = self.groups.iter_mut().find(|slot| slot.name == group) {
            slot.ids = ids.to_vec();
            slot.size = size;
            return;
        }
        self.groups.push(TileGroup {
            name: group.to_string(),
            ids: ids.to_vec(),
            size,
        });
    }

    /// Move `source` to just before `target`, within a group or across
    /// groups (the target's group is the destination). Returns whether the
    /// store changed: unknown ids, `source == target`, or a move that is
    /// already satisfied (`source` immediately before `target` in the same
    /// group) are a no-op.
    ///
    /// A group the move empties is pruned, so a cross-group move never
    /// leaves a ghost group behind. The ordering this writes is exactly
    /// what [`LauncherModel::snapshot`](crate::launcher_view::LauncherModel)
    /// persists through `LauncherConfig::tile_groups`.
    pub fn reorder(&mut self, source: &str, target: &str) -> bool {
        if source == target {
            return false;
        }
        let Some((src_group, src_index)) = self.locate(source) else {
            return false;
        };
        let Some((mut dst_group, dst_index)) = self.locate(target) else {
            return false;
        };
        if src_group == dst_group && src_index + 1 == dst_index {
            return false;
        }
        let moved = self.groups[src_group].ids.remove(src_index);
        // Removing a tile before the target in the same group shifts the
        // target one slot left.
        let mut insert_at = dst_index;
        if src_group == dst_group && src_index < dst_index {
            insert_at -= 1;
        }
        if self.groups[src_group].ids.is_empty() && src_group != dst_group {
            self.groups.remove(src_group);
            if src_group < dst_group {
                dst_group -= 1;
            }
        }
        let insert_at = insert_at.min(self.groups[dst_group].ids.len());
        self.groups[dst_group].ids.insert(insert_at, moved);
        true
    }

    /// The `(group index, member index)` of `id`, if assigned anywhere.
    fn locate(&self, id: &str) -> Option<(usize, usize)> {
        self.groups
            .iter()
            .enumerate()
            .find_map(|(group, slot)| slot.ids.iter().position(|x| x == id).map(|i| (group, i)))
    }
}

/// The launcher's power/session actions (spec §Layout, bottom row).
///
/// Lives in the pure core so the argv contract is unit-testable without a
/// compositor; the view (`launcher_view`) renders the row and folds the
/// messages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerAction {
    Lock,
    Logout,
    Suspend,
    Reboot,
    PowerOff,
}

/// The session CLI's binary name, as configured in the default power-key
/// bindings and installed by the `session` crate.
const SESSION_CLI_NAME: &str = "icedtea-session";

/// Resolve [`SESSION_CLI_NAME`] to an absolute path once per process.
///
/// The launcher's power actions are privileged (lock/suspend/reboot/poweroff),
/// so they must not run whichever `icedtea-session` happens to be first on a
/// `PATH` that could be shadowed or mutated between load and click. The first
/// executable found is cached in a `OnceLock`; if the binary is genuinely not
/// installed the bare name is kept, so the spawn fails with `NotFound` and the
/// failure surfaces as a launcher status line rather than silently doing
/// nothing.
fn session_cli_program() -> &'static str {
    static RESOLVED: OnceLock<Option<PathBuf>> = OnceLock::new();
    RESOLVED
        .get_or_init(|| resolve_on_path(SESSION_CLI_NAME, std::env::var_os("PATH").as_deref()))
        .as_deref()
        .and_then(Path::to_str)
        .unwrap_or(SESSION_CLI_NAME)
}

/// The first executable named `name` in `path_env` (a `PATH` value), or `None`.
/// Pure over its arguments so the resolution rule is testable without touching
/// the real environment. Empty `PATH` entries (which mean "the current
/// directory") are skipped: a privileged action must not pick up a binary
/// dropped in the launcher's working directory.
fn resolve_on_path(name: &str, path_env: Option<&OsStr>) -> Option<PathBuf> {
    std::env::split_paths(path_env?)
        .filter(|dir| !dir.as_os_str().is_empty())
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable(candidate))
}

/// Is `path` a regular file with at least one execute bit set? Close enough to
/// `which` for a same-user daemon binary; an unreadable directory or missing
/// file simply means "not here, keep looking".
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// The exact CLI invocation a [`PowerAction`] runs: program plus subcommand.
///
/// `Logout` is `None`: it takes the compositor quit path (no CLI). Every
/// other action calls the A3 `icedtea-session` helper, which forwards to
/// `org.icedtea.Session` over the session bus. The program is the absolute
/// path resolved once by [`session_cli_program`] (falling back to the bare
/// name only when the binary is not installed), so a privileged action never
/// depends on the `PATH` in effect at click time.
#[must_use]
pub fn power_argv(action: PowerAction) -> Option<(&'static str, &'static [&'static str])> {
    let program = session_cli_program();
    match action {
        PowerAction::Lock => Some((program, &["lock"])),
        PowerAction::Logout => None,
        PowerAction::Suspend => Some((program, &["suspend"])),
        PowerAction::Reboot => Some((program, &["reboot"])),
        PowerAction::PowerOff => Some((program, &["poweroff"])),
    }
}

/// The launcher status line for a failed [`PowerAction`]: it names the
/// action and carries the underlying cause, so a failed session-CLI call is
/// never silent (spec §Spawn+power). Tested as a pure mapping, not via exec.
#[must_use]
pub fn power_error_message(action: PowerAction, detail: &str) -> String {
    let name = match action {
        PowerAction::Lock => "Lock",
        PowerAction::Logout => "Log out",
        PowerAction::Suspend => "Suspend",
        PowerAction::Reboot => "Reboot",
        PowerAction::PowerOff => "PowerOff",
    };
    format!("Could not {name}: {detail}")
}

/// Run one [`power_argv`] command with an internal ~30s bound, so a slow
/// or hung helper can never wedge the caller: the child is polled to exit,
/// killed (and reaped) on timeout, and a timed-out [`std::io::Error`] is
/// returned.
///
/// The sibling view calls this instead of spawning directly, so the
/// argv contract ([`power_argv`]) and the execution bound live together in
/// the unit-testable core.
pub(crate) fn run_power_command(
    program: &str,
    args: &[&str],
) -> std::io::Result<std::process::ExitStatus> {
    use std::time::{Duration, Instant};
    const BOUND: Duration = Duration::from_secs(30);
    const POLL: Duration = Duration::from_millis(50);
    let mut child = std::process::Command::new(program).args(args).spawn()?;
    let deadline = Instant::now() + BOUND;
    loop {
        match child.try_wait()? {
            Some(status) => return Ok(status),
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!("{program} did not exit within {}s", BOUND.as_secs()),
                ));
            }
            None => std::thread::sleep(POLL),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

    fn entry(name: &str, id: &str) -> DesktopEntry {
        DesktopEntry {
            id: id.into(),
            name: name.into(),
            exec: id.into(),
            icon: None,
            categories: Vec::new(),
            keywords: Vec::new(),
            nodisplay: false,
            only_show_in: Vec::new(),
            not_show_in: Vec::new(),
        }
    }

    fn tmpdir() -> PathBuf {
        let n = TMP_SEQ.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "icedtea-launcher-test-{}-{}",
            std::process::id(),
            n
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn parse_skips_nodisplay_and_honors_onlyshowin() {
        let e = parse_entry("[Desktop Entry]\nName=Foo\nExec=foo\nNoDisplay=true\n").unwrap();
        assert!(e.nodisplay);
        let idx = DesktopIndex::from_entries(vec![e]);
        assert!(idx.apps().is_empty());
    }

    #[test]
    fn rank_prefers_prefix_over_substring() {
        let idx = DesktopIndex::from_entries(vec![
            entry("x Firefox", "firefox"),
            entry("y Fire tools", "firetools"),
        ]);
        assert_eq!(rank(&idx, "fire")[0].name, "x Firefox");
    }

    #[test]
    fn parse_reads_all_key_types() {
        let e = parse_entry(
            "[Desktop Entry]\n\
             Name=Terminal\n\
             Exec=gnome-terminal %F\n\
             Icon=terminal\n\
             Categories=System;TerminalEmulator;\n\
             Keywords=shell;prompt;\n",
        )
        .unwrap();
        assert_eq!(e.name, "Terminal");
        assert_eq!(e.exec, "gnome-terminal %F");
        assert_eq!(e.icon.as_deref(), Some("terminal"));
        assert_eq!(e.categories, vec!["System", "TerminalEmulator"]);
        assert_eq!(e.keywords, vec!["shell", "prompt"]);
        assert!(!e.nodisplay);
    }

    #[test]
    fn parse_locale_name_wins_over_plain_name() {
        let e = parse_entry("[Desktop Entry]\nName=Files\nName[de]=Dateien\nExec=files\n").unwrap();
        assert_eq!(e.name, "Dateien");
        let e = parse_entry("[Desktop Entry]\nName=Files\nExec=files\n").unwrap();
        assert_eq!(e.name, "Files");
    }

    #[test]
    fn parse_exec_field_codes_stripped_for_display_raw_kept() {
        let e =
            parse_entry("[Desktop Entry]\nName=F\nExec=foo %f %F %u %U %i %c %k bar\n").unwrap();
        assert!(e.exec.contains("%f"), "raw Exec keeps field codes");
        assert_eq!(e.display_exec(), "foo bar");
    }

    #[test]
    fn display_exec_unescapes_escaped_percent_after_stripping() {
        // `%%` is an escaped literal percent, not a field code: stripping
        // runs first (no `%x` code here), then `%%` → `%`.
        let e = parse_entry("[Desktop Entry]\nName=F\nExec=foo %%f bar\n").unwrap();
        assert_eq!(e.display_exec(), "foo %f bar");
        let e = parse_entry("[Desktop Entry]\nName=F\nExec=100%% done\n").unwrap();
        assert_eq!(e.display_exec(), "100% done");
    }

    #[test]
    fn display_exec_keeps_a_literal_placeholder_char_verbatim() {
        // The old implementation used U+E000 as an internal `%%` stand-in
        // and rewrote it back afterwards, corrupting input that genuinely
        // contained it. The single-pass push has no stand-in to leak.
        let mut e = entry("F", "f");
        e.exec = "foo\u{e000} %f bar".to_string();
        assert_eq!(e.display_exec(), "foo\u{e000} bar");
    }

    #[test]
    fn display_exec_strips_embedded_codes_and_keeps_unknown_sequences() {
        // Rule: `%x` for a known field code is dropped wherever it occurs;
        // `%%` unescapes after; any other `%x` and a lone `%` stay verbatim.
        let e = parse_entry("[Desktop Entry]\nName=F\nExec=foo%fbar\n").unwrap();
        assert_eq!(e.display_exec(), "foobar");
        let e = parse_entry("[Desktop Entry]\nName=F\nExec=run %d %Q 50% off\n").unwrap();
        assert_eq!(e.display_exec(), "run %d %Q 50% off");
    }

    #[test]
    fn locale_exact_match_wins_over_first_variant() {
        let text = "[Desktop Entry]\nName=Files\nName[fr]=Fichiers\nName[de]=Dateien\nExec=files\n";
        let de = Locale {
            lang: Some("de_DE.UTF-8".into()),
            language: vec![],
        };
        let e = parse_entry_with_id_and_locale("files", text, Some(&de)).unwrap();
        assert_eq!(e.name, "Dateien");
        let fr = Locale {
            lang: None,
            language: vec!["fr".into(), "de".into()],
        };
        let e = parse_entry_with_id_and_locale("files", text, Some(&fr)).unwrap();
        assert_eq!(e.name, "Fichiers", "LANGUAGE order decides");
    }

    #[test]
    fn locale_selection_follows_the_documented_precedence_table() {
        // Rule (entry.rs): exact `$LANG` (full value, then without the
        // codeset/modifier, then the bare language) → each `$LANGUAGE`
        // entry in order → first `Name[xx]` in file order → plain `Name`.
        let text = "[Desktop Entry]\nName=Files\nName[fr]=Fichiers\nName[de]=Dateien\nExec=files\n";
        let plain = "[Desktop Entry]\nName=Files\nExec=files\n";
        let locale = |lang: Option<&str>, language: &[&str]| Locale {
            lang: lang.map(str::to_string),
            language: language.iter().map(|s| s.to_string()).collect(),
        };
        let cases: &[(&str, Option<Locale>, &str, &str)] = &[
            (
                "bare LANG beats a later variant",
                Some(locale(Some("de"), &[])),
                text,
                "Dateien",
            ),
            (
                "full LANG strips codeset/modifier to the bare language",
                Some(locale(Some("de_DE.UTF-8"), &[])),
                text,
                "Dateien",
            ),
            (
                "LANGUAGE order decides when LANG matches nothing",
                Some(locale(Some("es"), &["de", "fr"])),
                text,
                "Dateien",
            ),
            (
                "no exact match anywhere falls back to the first variant",
                Some(locale(Some("es"), &[])),
                text,
                "Fichiers",
            ),
            (
                "no locale falls back to the first variant",
                None,
                text,
                "Fichiers",
            ),
            (
                "no variant at all falls back to plain Name",
                Some(locale(Some("de"), &["fr"])),
                plain,
                "Files",
            ),
        ];
        for (desc, loc, input, expected) in cases {
            let e = parse_entry_with_id_and_locale("files", input, loc.as_ref()).unwrap();
            assert_eq!(e.name, *expected, "{desc}");
        }
    }

    #[test]
    fn locale_falls_back_to_first_variant_then_plain_name() {
        let text = "[Desktop Entry]\nName=Files\nName[fr]=Fichiers\nName[de]=Dateien\nExec=files\n";
        let es = Locale {
            lang: Some("es".into()),
            language: vec![],
        };
        let e = parse_entry_with_id_and_locale("files", text, Some(&es)).unwrap();
        assert_eq!(e.name, "Fichiers", "no exact match: first variant wins");
        let e = parse_entry_with_id_and_locale("files", text, None).unwrap();
        assert_eq!(e.name, "Fichiers", "no locale: first variant wins");
        let plain = "[Desktop Entry]\nName=Files\nExec=files\n";
        let e = parse_entry_with_id_and_locale("files", plain, Some(&es)).unwrap();
        assert_eq!(e.name, "Files");
    }

    #[test]
    fn parse_returns_none_without_name_or_exec() {
        assert!(parse_entry("[Desktop Entry]\nName=NoExec\n").is_none());
        assert!(parse_entry("[Desktop Entry]\nExec=no-name\n").is_none());
        assert!(parse_entry("[Other]\nName=X\nExec=x\n").is_none());
        assert!(parse_entry("not a desktop file").is_none());
        assert!(parse_entry("").is_none());
    }

    #[test]
    fn parse_ignores_comments_other_groups_and_blank_lines() {
        let e = parse_entry(
            "# comment\n\
             \n\
             [Desktop Entry]\n\
             Name=Foo\n\
             # Name=Wrong\n\
             Exec=foo\n\
             \n\
             [Desktop Action X]\n\
             Name=Shadow\n",
        )
        .unwrap();
        assert_eq!(e.name, "Foo");
        assert_eq!(e.exec, "foo");
    }

    #[test]
    fn showin_lists_gate_visibility() {
        let mut e = entry("G", "g");
        e.only_show_in = vec!["GNOME".into()];
        assert!(!e.visible_in(SHOW_IN_ENV));
        e.only_show_in = vec!["icedtea".into()];
        assert!(e.visible_in(SHOW_IN_ENV));
        e.only_show_in.clear();
        e.not_show_in = vec!["icedtea".into()];
        assert!(!e.visible_in(SHOW_IN_ENV));
        e.not_show_in.clear();
        assert!(e.visible_in(SHOW_IN_ENV));
    }

    #[test]
    fn onlyshowin_file_is_filtered_from_index() {
        let mut e = entry("G", "g");
        e.only_show_in = vec!["GNOME".into()];
        let idx = DesktopIndex::from_entries(vec![e]);
        assert!(idx.apps().is_empty());
    }

    #[test]
    fn scan_reads_desktop_files_and_skips_nodisplay() {
        let dir = tmpdir();
        std::fs::write(
            dir.join("a.desktop"),
            "[Desktop Entry]\nName=App A\nExec=a\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("hidden.desktop"),
            "[Desktop Entry]\nName=Hidden\nExec=h\nNoDisplay=true\n",
        )
        .unwrap();
        std::fs::write(dir.join("junk.txt"), "not a desktop entry\n").unwrap();
        let apps = DesktopIndex::scan(std::slice::from_ref(&dir));
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].id, "a");
        assert_eq!(apps[0].name, "App A");
    }

    #[test]
    fn scan_dedupes_same_app_id_across_dirs_first_dir_wins() {
        let sys = tmpdir();
        let user = tmpdir();
        std::fs::write(
            sys.join("dup.desktop"),
            "[Desktop Entry]\nName=System Copy\nExec=sys-dup\n",
        )
        .unwrap();
        std::fs::write(
            user.join("dup.desktop"),
            "[Desktop Entry]\nName=User Copy\nExec=user-dup\n",
        )
        .unwrap();
        std::fs::write(
            user.join("only-user.desktop"),
            "[Desktop Entry]\nName=U\nExec=u\n",
        )
        .unwrap();
        let apps = DesktopIndex::scan(&[sys.clone(), user.clone()]);
        std::fs::remove_dir_all(&sys).unwrap();
        std::fs::remove_dir_all(&user).unwrap();
        assert_eq!(apps.len(), 2);
        let dup = apps.iter().find(|e| e.id == "dup").expect("one dup entry");
        assert_eq!(dup.name, "System Copy", "first dir wins");
        assert_eq!(dup.exec, "sys-dup");
    }

    #[test]
    fn scan_missing_dir_yields_empty_without_panicking() {
        let apps = DesktopIndex::scan(&[PathBuf::from("/nonexistent-icedtea-dir")]);
        assert!(apps.is_empty());
    }

    #[test]
    fn rank_empty_query_returns_everything() {
        let idx = DesktopIndex::from_entries(vec![entry("B", "b"), entry("A", "a")]);
        assert_eq!(rank(&idx, "").len(), 2);
        assert_eq!(rank(&idx, "   ").len(), 2);
    }

    #[test]
    fn rank_no_match_returns_empty() {
        let idx = DesktopIndex::from_entries(vec![entry("Firefox", "firefox")]);
        assert!(rank(&idx, "zzzz").is_empty());
    }

    #[test]
    fn rank_matches_keywords_and_exec_basename() {
        let mut pdf = entry("Document Viewer", "evince");
        pdf.keywords = vec!["pdf".into()];
        let idx = DesktopIndex::from_entries(vec![pdf, entry("Music", "music")]);
        assert_eq!(rank(&idx, "pdf")[0].id, "evince");
        assert_eq!(rank(&idx, "evin")[0].id, "evince");
    }

    #[test]
    fn rank_recency_breaks_score_ties() {
        let idx = DesktopIndex::from_entries(vec![
            entry("Fire Alpha", "fire-alpha"),
            entry("Fire Beta", "fire-beta"),
        ]);
        assert_eq!(rank(&idx, "fire")[0].id, "fire-alpha");
        let mut rec = RecencyStore::default();
        rec.record("fire-beta");
        let m = Matcher::new(&idx).with_recency(&rec);
        assert_eq!(m.rank("fire")[0].id, "fire-beta");
    }

    #[test]
    fn pin_store_pins_unpins_in_order() {
        let mut pins = PinStore::default();
        pins.pin("b");
        pins.pin("a");
        pins.pin("a");
        assert!(pins.is_pinned("a"));
        assert_eq!(pins.pinned(), &["b".to_string(), "a".to_string()]);
        pins.unpin("b");
        assert!(!pins.is_pinned("b"));
        assert_eq!(pins.pinned(), &["a".to_string()]);
        pins.unpin("missing");
        assert_eq!(pins.pinned(), &["a".to_string()]);
    }

    #[test]
    fn tile_store_assigns_groups_in_order() {
        let mut tiles = TileStore::default();
        tiles.assign("Web", "firefox");
        tiles.assign("Web", "firefox");
        tiles.assign("Media", "music");
        let groups = tiles.groups();
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].name, "Web");
        assert_eq!(groups[0].ids, vec!["firefox".to_string()]);
        assert_eq!(groups[1].name, "Media");
    }

    #[test]
    fn tile_store_assign_uses_default_size() {
        let mut tiles = TileStore::default();
        tiles.assign("Web", "firefox");
        assert_eq!(tiles.groups().len(), 1);
        assert_eq!(tiles.groups()[0].size, default_tile_size());
    }

    #[test]
    fn tile_store_ingest_restores_a_group_with_its_size() {
        let mut tiles = TileStore::default();
        tiles.ingest("Web", &["firefox".to_string(), "epiphany".to_string()], 2);
        let groups = tiles.groups();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].name, "Web");
        assert_eq!(
            groups[0].ids,
            vec!["firefox".to_string(), "epiphany".to_string()]
        );
        assert_eq!(groups[0].size, 2);
    }

    #[test]
    fn tile_store_ingest_replaces_an_existing_group() {
        let mut tiles = TileStore::default();
        tiles.assign("Web", "firefox");
        tiles.ingest("Web", &["epiphany".to_string()], 3);
        let groups = tiles.groups();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].ids, vec!["epiphany".to_string()]);
        assert_eq!(groups[0].size, 3);
    }

    #[test]
    fn tile_store_reorder_moves_within_a_group() {
        let mut tiles = TileStore::default();
        tiles.ingest(
            "Web",
            &["a".to_string(), "b".to_string(), "c".to_string()],
            1,
        );
        assert!(tiles.reorder("c", "a"));
        assert_eq!(
            tiles.groups()[0].ids,
            vec!["c".to_string(), "a".to_string(), "b".to_string()]
        );
        assert!(
            !tiles.reorder("c", "a"),
            "already immediately before a: a no-op reports no change"
        );
        assert_eq!(
            tiles.groups()[0].ids,
            vec!["c".to_string(), "a".to_string(), "b".to_string()],
            "repeating the move leaves the order unchanged"
        );
    }

    #[test]
    fn tile_store_reorder_moves_across_groups_and_prunes_the_emptied_one() {
        let mut tiles = TileStore::default();
        tiles.ingest("Web", &["a".to_string()], 1);
        tiles.ingest("Media", &["b".to_string(), "c".to_string()], 2);
        assert!(tiles.reorder("a", "c"));
        assert_eq!(tiles.groups().len(), 1, "the emptied Web group is pruned");
        let media = &tiles.groups()[0];
        assert_eq!(media.name, "Media");
        assert_eq!(
            media.ids,
            vec!["b".to_string(), "a".to_string(), "c".to_string()]
        );
        assert_eq!(media.size, 2, "the destination group keeps its size");
    }

    #[test]
    fn tile_store_reorder_rejects_unknown_ids_and_self_moves() {
        let mut tiles = TileStore::default();
        tiles.ingest("Web", &["a".to_string(), "b".to_string()], 1);
        assert!(!tiles.reorder("a", "a"), "self move");
        assert!(!tiles.reorder("a", "missing"), "unknown target");
        assert!(!tiles.reorder("missing", "a"), "unknown source");
        assert_eq!(
            tiles.groups()[0].ids,
            vec!["a".to_string(), "b".to_string()],
            "no rejected call mutated the store"
        );
    }

    #[test]
    fn adaptive_score_is_frequency_weighted_and_decays_with_age() {
        let mut rec = RecencyStore::default();
        rec.record("twice");
        rec.record("twice");
        rec.record("once");
        assert_eq!(rec.adaptive_score("twice"), 2, "age 0: weight is the count");
        assert_eq!(rec.adaptive_score("once"), 1);
        assert_eq!(rec.adaptive_score("never"), 0);
        // Age the entries by recording other launches.
        for i in 0..DECAY_WINDOW {
            rec.record(&format!("filler-{i}"));
        }
        let aged_twice = rec.adaptive_score("twice");
        let aged_once = rec.adaptive_score("once");
        assert!(
            aged_twice < 2 && aged_once < 1,
            "staleness must decay the weight: twice={aged_twice} once={aged_once}"
        );
        assert!(
            aged_twice >= aged_once,
            "frequency still leads at equal age: {aged_twice} >= {aged_once}"
        );
    }

    #[test]
    fn adaptive_score_recent_beats_stale_at_equal_count() {
        let mut rec = RecencyStore::default();
        rec.record("stale");
        for i in 0..DECAY_WINDOW {
            rec.record(&format!("filler-{i}"));
        }
        rec.record("recent");
        assert!(
            rec.adaptive_score("recent") > rec.adaptive_score("stale"),
            "equal count, fresher must win: {} > {}",
            rec.adaptive_score("recent"),
            rec.adaptive_score("stale")
        );
    }

    #[test]
    fn matcher_adaptive_ordering_prefers_the_frequent_recent_app() {
        let idx = DesktopIndex::from_entries(vec![
            entry("Fire Alpha", "fire-alpha"),
            entry("Fire Beta", "fire-beta"),
        ]);
        let mut rec = RecencyStore::default();
        rec.record("fire-beta");
        rec.record("fire-beta");
        rec.record("fire-alpha");
        let ranked = Matcher::new(&idx).with_recency(&rec).rank("fire");
        assert_eq!(ranked[0].id, "fire-beta", "two fresh launches outrank one");
    }

    #[test]
    fn adaptive_score_snapshot_round_trips_through_restore() {
        let mut rec = RecencyStore::default();
        rec.record("a");
        rec.record("b");
        rec.record("a");
        let score = rec.adaptive_score("a");
        let mut back = RecencyStore::default();
        back.restore(&rec.snapshot());
        assert_eq!(back.adaptive_score("a"), score);
    }

    #[test]
    fn recency_record_counts_and_caps_at_200() {
        let mut rec = RecencyStore::default();
        rec.record("a");
        rec.record("a");
        assert_eq!(rec.count("a"), 2);
        assert_eq!(rec.count("missing"), 0);
        for i in 0..210 {
            rec.record(&format!("app-{i}"));
        }
        assert_eq!(rec.len(), MAX_RECENCY_ENTRIES);
        assert_eq!(rec.count("a"), 0, "stalest pruned first");
        assert_eq!(rec.count("app-209"), 1);
    }

    #[test]
    fn recency_restore_reloads_persisted_counts() {
        let mut saved = HashMap::new();
        saved.insert("firefox".to_string(), (4u64, 9u64));
        saved.insert("music".to_string(), (1u64, 3u64));
        let mut rec = RecencyStore::default();
        rec.restore(&saved);
        assert_eq!(rec.count("firefox"), 4);
        assert_eq!(rec.count("music"), 1);
        assert_eq!(rec.len(), 2);
        // A fresh record keeps the restored count and bumps it.
        rec.record("firefox");
        assert_eq!(rec.count("firefox"), 5);
    }

    #[test]
    fn power_argv_matches_the_session_cli_contract() {
        for (action, subcommand) in [
            (PowerAction::Lock, "lock"),
            (PowerAction::Suspend, "suspend"),
            (PowerAction::Reboot, "reboot"),
            (PowerAction::PowerOff, "poweroff"),
        ] {
            let (program, args) = power_argv(action).expect("a session-CLI invocation");
            assert_eq!(
                args,
                [subcommand].as_slice(),
                "{action:?} forwards to the `icedtea-session {subcommand}` CLI"
            );
            assert!(
                program.ends_with(SESSION_CLI_NAME),
                "{action:?}: program must be the session CLI, got {program:?}"
            );
            assert!(
                program == SESSION_CLI_NAME || program.starts_with('/'),
                "{action:?}: program must be an absolute path when installed, \
                 or the bare name when not, got {program:?}"
            );
        }
        assert_eq!(
            power_argv(PowerAction::Logout),
            None,
            "logout takes the compositor quit path, never a shell-out"
        );
    }

    /// The resolution rule: the first executable of the name wins, an
    /// empty `PATH` entry (cwd) is skipped, and a non-executable earlier
    /// candidate does not shadow a later executable one.
    #[test]
    fn resolve_on_path_prefers_the_first_executable() {
        use std::os::unix::fs::PermissionsExt;

        let first = tmpdir();
        let second = tmpdir();
        let no_exec = first.join(SESSION_CLI_NAME);
        std::fs::write(&no_exec, b"not executable").unwrap();
        std::fs::set_permissions(&no_exec, std::fs::Permissions::from_mode(0o644)).unwrap();
        let executable = second.join(SESSION_CLI_NAME);
        std::fs::write(&executable, b"#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();

        // The empty middle entry stands for "the current directory" and must
        // be ignored.
        let path_env =
            std::ffi::OsString::from(format!("{}::{}", first.display(), second.display()));
        assert_eq!(
            resolve_on_path(SESSION_CLI_NAME, Some(&path_env)),
            Some(executable)
        );
    }

    #[test]
    fn resolve_on_path_returns_none_when_absent_or_unset() {
        let dir = tmpdir();
        let path_env = dir.into_os_string();
        assert_eq!(resolve_on_path(SESSION_CLI_NAME, Some(&path_env)), None);
        assert_eq!(resolve_on_path(SESSION_CLI_NAME, None), None);
    }

    #[test]
    fn power_error_names_the_action_and_the_cause() {
        for (action, name) in [
            (PowerAction::Lock, "Lock"),
            (PowerAction::Logout, "Log out"),
            (PowerAction::Suspend, "Suspend"),
            (PowerAction::Reboot, "Reboot"),
            (PowerAction::PowerOff, "PowerOff"),
        ] {
            let msg = power_error_message(action, "permission denied");
            assert!(
                msg.contains(name) && msg.contains("permission denied"),
                "{action:?}: status line must name the action and the cause, got {msg:?}"
            );
        }
    }

    #[test]
    fn recency_snapshot_round_trips_through_restore() {
        let mut rec = RecencyStore::default();
        rec.record("a");
        rec.record("b");
        rec.record("a");
        let snap = rec.snapshot();
        assert_eq!(snap.get("a").map(|(count, _)| *count), Some(2));
        assert_eq!(snap.get("b").map(|(count, _)| *count), Some(1));
        // The snapshot feeds `Config::save` and comes back through
        // `restore` with counts and order intact.
        let mut back = RecencyStore::default();
        back.restore(&snap);
        assert_eq!(back.count("a"), 2);
        assert_eq!(back.count("b"), 1);
    }

    #[test]
    fn default_dirs_lists_the_two_xdg_locations_user_first() {
        let dirs = default_dirs();
        assert_eq!(dirs.len(), 2);
        assert!(dirs[0].ends_with(".local/share/applications"));
        assert!(dirs[1].ends_with("usr/share/applications"));
    }
}
