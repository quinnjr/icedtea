//! Pure launcher core: `.desktop` index, fuzzy matcher, pin/tile/recency stores.
//!
//! No Wayland imports here by design — the view renders what this module
//! returns, and every behavior is unit-testable without a compositor.

pub mod entry;

pub use entry::{DesktopEntry, parse_entry, parse_entry_with_id};

use std::collections::HashMap;
use std::path::PathBuf;

/// Desktop name this launcher shows up as for `OnlyShowIn`/`NotShowIn`.
pub const SHOW_IN_ENV: &str = "icedtea";

/// Default XDG application directories scanned for `.desktop` files.
pub fn default_dirs() -> Vec<PathBuf> {
    let user = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("~"))
        .join(".local/share/applications");
    vec![PathBuf::from("/usr/share/applications"), user]
}

/// Cap on distinct apps tracked by [`RecencyStore`]; oldest prune first.
pub const MAX_RECENCY_ENTRIES: usize = 200;

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
    /// unparseable content are skipped, never fatal.
    pub fn scan(dirs: &[PathBuf]) -> Vec<DesktopEntry> {
        let mut apps = Vec::new();
        for dir in dirs {
            let dir_entries = match std::fs::read_dir(dir) {
                Ok(entries) => entries,
                Err(_) => continue,
            };
            for dir_entry in dir_entries.flatten() {
                let path = dir_entry.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("desktop") {
                    continue;
                }
                let text = match std::fs::read_to_string(&path) {
                    Ok(text) => text,
                    Err(_) => continue,
                };
                let id = path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .unwrap_or("");
                if id.is_empty() {
                    continue;
                }
                let visible =
                    parse_entry_with_id(id, &text).filter(|entry| entry.visible_in(SHOW_IN_ENV));
                if let Some(entry) = visible {
                    apps.push(entry);
                }
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
/// Ties break on recency count, then case-insensitive name, then id.
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
            let mut best = field_score(&app.name.to_lowercase(), &normalized);
            for keyword in &app.keywords {
                best = best.max(field_score(&keyword.to_lowercase(), &normalized));
            }
            let basename = app.exec_basename().to_lowercase();
            if !basename.is_empty() {
                best = best.max(field_score(&basename, &normalized));
            }
            if best > 0 {
                let recency = self.recency.map(|store| store.count(&app.id)).unwrap_or(0);
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

/// Standard 1x1 tile span assigned to newly created groups.
pub fn default_tile_size() -> u32 {
    1
}

/// One named tile group holding ordered app ids.
///
/// Mirrored by `config::TileGroup` for persistence (the config crate cannot
/// depend on this crate; conversion happens at the shell boundary in
/// `config_ext.rs`). Keep the two shapes in sync when either changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TileGroup {
    /// Group display name.
    pub name: String,
    /// Member app ids in tile order.
    pub ids: Vec<String>,
    /// Tile span in grid units (`1` = standard 1x1 tile).
    pub size: u32,
}

impl Default for TileGroup {
    fn default() -> Self {
        Self {
            name: String::new(),
            ids: Vec::new(),
            size: default_tile_size(),
        }
    }
}

/// Named tile groups (ordered) with ordered member ids.
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

/// The exact shell-out a [`PowerAction`] runs: program plus its args.
///
/// `Logout` is `None`: it takes the compositor quit path (no shell-out).
/// Every other action is the spec §Spawn+power shell-out until A3 logind
/// replaces it.
#[must_use]
pub fn power_argv(action: PowerAction) -> Option<(&'static str, &'static [&'static str])> {
    match action {
        PowerAction::Lock => Some(("loginctl", &["lock-session"])),
        PowerAction::Logout => None,
        PowerAction::Suspend => Some(("systemctl", &["suspend"])),
        PowerAction::Reboot => Some(("systemctl", &["reboot"])),
        PowerAction::PowerOff => Some(("systemctl", &["poweroff"])),
    }
}

/// The launcher status line for a failed [`PowerAction`]: it names the
/// action and carries the underlying cause, so a shell-out failure is never
/// silent (spec §Spawn+power). Tested as a pure mapping, not via exec.
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
    fn power_argv_matches_the_shell_out_contract() {
        assert_eq!(
            power_argv(PowerAction::Lock),
            Some(("loginctl", ["lock-session"].as_slice()))
        );
        assert_eq!(
            power_argv(PowerAction::Suspend),
            Some(("systemctl", ["suspend"].as_slice()))
        );
        assert_eq!(
            power_argv(PowerAction::Reboot),
            Some(("systemctl", ["reboot"].as_slice()))
        );
        assert_eq!(
            power_argv(PowerAction::PowerOff),
            Some(("systemctl", ["poweroff"].as_slice()))
        );
        assert_eq!(
            power_argv(PowerAction::Logout),
            None,
            "logout takes the compositor quit path, never a shell-out"
        );
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
    fn default_dirs_lists_the_two_xdg_locations() {
        let dirs = default_dirs();
        assert_eq!(dirs.len(), 2);
        assert!(dirs[0].ends_with("usr/share/applications"));
        assert!(dirs[1].ends_with(".local/share/applications"));
    }
}
