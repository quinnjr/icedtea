use std::collections::HashMap;
use std::path::{Path, PathBuf};

use redb::{Database, ReadableDatabase, ReadableTable};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

pub mod defaults;
pub mod keys;
pub mod schema;

use schema::*;

pub use keys::{key_name_to_keysym, keysym_to_key_name, MODIFIER_TOKENS};

/// Written into `DB_META` on every `Config::save`, for the future settings
/// crate (the plan's single config writer) to use for migrations. It is
/// deliberately never read/checked here: `load_or_default` already falls
/// back per-field for anything missing or unparsable, so an unknown/older
/// on-disk layout degrades gracefully without needing a version check on
/// this read path.
pub const SCHEMA_VERSION: u64 = 1;

pub use defaults::default_config;
pub use contract::Appearance;
pub use contract::DisplayConfig;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KeyCombo {
    pub modifiers: Vec<String>,
    pub key: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Behavior {
    pub raise_on_focus: bool,
    pub hide_bar_on_fullscreen: bool,
    pub snap_enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    pub keybindings: HashMap<String, KeyCombo>,
    pub appearance: Appearance,
    pub behavior: Behavior,
    pub workspace_names: Vec<String>,
    pub displays: Vec<DisplayConfig>,
}

pub fn default_db_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::home_dir().expect("HOME set").join(".config"));
    base.join("icedtea").join("config.redb")
}

/// Open the config DB. Missing file -> creates it. Corrupt/IO errors bubble up
/// to the caller (which chooses defaults).
///
/// `redb::Database::create` is documented to return `Err` on a corrupt file,
/// but in practice (observed with redb 3.1.3, see
/// `config/tests/corruption_manual.rs::truncated_redb_file_returns_defaults`)
/// a truncated/corrupt file can instead trip an internal `assert!` in redb's
/// page manager and unwind as a panic rather than a `Result::Err`. We catch
/// that unwind here and turn it into an `Err` so `load_or_default` can fall
/// back to defaults, per the crate's never-panic contract. A file can also
/// open cleanly here yet still have corrupt page/table data that only
/// surfaces once it's actually read; `load_or_default` wraps its own read
/// path (`begin_read`, `open_table`, table gets, the keybindings cursor) in
/// the same `catch_unwind_silently` machinery for that case, so the
/// unwind-safety net covers the whole open-then-read path, not just this
/// function.
///
/// This is sound: the panic occurs entirely inside the (failed) construction
/// of the `Database` value being returned by this call — there is no
/// partially-initialized `Database` escaping the panic, no shared/static/lock
/// state that redb leaves poisoned across this boundary, and nothing else on
/// this thread holds a reference into the half-built object. The unwind is
/// caught, the payload is logged, and we return a plain `redb::Error::Io` so
/// the caller's `Result`-based fallback logic applies uniformly whether redb
/// panicked or returned `Err` normally.
///
/// We also suppress the default panic hook for the duration of the call.
/// Without that, `catch_unwind` still stops the unwind from propagating, but
/// Rust's default hook runs *before* unwinding starts and prints a raw
/// `thread '...' panicked at ...: assertion failed: ...` line straight to
/// stderr — indistinguishable, to anyone tailing logs, from an unhandled
/// crash, even though we go on to log a clean `tracing::warn!` and return
/// defaults. `std::panic::set_hook` is process-global, so the swap is guarded
/// by `PANIC_HOOK_LOCK` to serialize it against any other thread doing the
/// same swap (e.g. concurrent test threads exercising this same function),
/// and the previous hook is restored immediately after `catch_unwind`
/// returns, before the lock is released. The one accepted trade-off: any
/// *unrelated* panic on another thread that happens to land inside this
/// narrow window will also print nothing, since the hook is process-wide;
/// that window is a single `Database::create` call, so the exposure is
/// small.
pub fn open(db_path: &Path) -> Result<Database, redb::Error> {
    if let Some(dir) = db_path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let owned_path = db_path.to_path_buf();
    match catch_unwind_silently(move || Database::create(&owned_path)) {
        Ok(result) => result.map_err(Into::into),
        Err(payload) => {
            let msg = panic_payload_message(&payload);
            tracing::warn!("config db open panicked ({msg}), treating file as corrupt");
            Err(std::io::Error::other(format!("redb panicked while opening database: {msg}")).into())
        }
    }
}

/// Process-global lock serializing temporary panic-hook swaps (see
/// `catch_unwind_silently`) so concurrent callers don't clobber each other's
/// saved hook.
static PANIC_HOOK_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Like `std::panic::catch_unwind`, but additionally installs a no-op panic
/// hook for the duration of the call so an expected/handled panic doesn't
/// print a raw backtrace line to stderr. The previous hook is restored
/// before returning, whether `f` panicked or not.
fn catch_unwind_silently<F, R>(f: F) -> std::thread::Result<R>
where
    F: FnOnce() -> R + std::panic::UnwindSafe,
{
    let _guard = PANIC_HOOK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_info| {}));
    let result = std::panic::catch_unwind(f);
    std::panic::set_hook(previous_hook);
    result
}

fn panic_payload_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "non-string panic payload".to_string()
    }
}

fn read_json<T: DeserializeOwned>(
    table: &impl ReadableTable<&'static str, &'static [u8]>,
    key: &'static str,
) -> Option<T> {
    table.get(key).ok().flatten().and_then(|v| serde_json::from_slice(v.value()).ok())
}

/// Outcome of [`try_load`]: the load path that must NOT silently turn a live
/// config into defaults on a transient open lock (review finding #2).
///
/// A cross-process open collision -- the settings app has the DB open when the
/// compositor tries to reload -- surfaces as `redb::Error::DatabaseAlreadyOpen`
/// (redb takes a whole-file lock, so a second opener anywhere on the system,
/// or in this process, gets it). That is transient: the file is intact, only
/// momentarily unavailable. A caller that already holds a good in-memory config
/// must KEEP it and retry rather than adopt `default_config()` and wipe the
/// live appearance/keybindings/workspaces. `Locked` is that signal; every other
/// failure (missing file, corrupt pages, IO) is a legitimate reason to fall
/// back to defaults and is folded into `Loaded(default_config())`.
// `Loaded` carries a `Config` by value and `Locked` is empty, so clippy flags
// the size difference. Boxing the `Config` would add a heap allocation on the
// hot reload path purely to shrink a transient value that is destructured and
// discarded immediately; the size gap is harmless here.
#[allow(clippy::large_enum_variant)]
pub enum LoadOutcome {
    /// The config was read (with per-field defaults for anything missing or
    /// unparsable), OR the file is genuinely missing/corrupt so defaults are
    /// the correct answer. Either way it is safe to adopt this `Config`.
    Loaded(Config),
    /// The file exists but could not be opened because it is currently locked
    /// by another handle (`DatabaseAlreadyOpen`). A caller holding a live
    /// config must keep it and retry; only a caller with nothing to keep
    /// (fresh boot) should fall back to defaults.
    Locked,
}

/// Load config, distinguishing a transient open-lock (`Locked`, keep your
/// current config and retry) from every other outcome (`Loaded`, safe to
/// adopt -- a real read, or defaults for a missing/corrupt file). This is the
/// wipe-safe primitive behind [`load_or_default`]; the compositor's reload
/// worker uses it directly so a cross-process collision never downgrades the
/// live config to defaults (review finding #2).
pub fn try_load(db_path: &Path) -> LoadOutcome {
    match open(db_path) {
        Ok(db) => LoadOutcome::Loaded(read_config_from_db(db)),
        // Transient: another handle (e.g. the settings app) holds the file.
        // The caller decides whether to keep a live config or fall back.
        Err(redb::Error::DatabaseAlreadyOpen) => LoadOutcome::Locked,
        Err(e) => {
            tracing::warn!("config db unavailable ({e}), using defaults");
            LoadOutcome::Loaded(default_config())
        }
    }
}

/// Load config, falling back to defaults for every individual field that is
/// missing or unparsable. Never returns Err and never panics: a file that
/// opens cleanly (see `open`) but has corrupt page/table data can still trip
/// an internal redb assert during `begin_read`, `open_table`, or a table
/// read/cursor call, so the entire read path below is wrapped in
/// `catch_unwind_silently` in addition to `open`'s own protection.
///
/// This convenience wrapper treats a transient `Locked` the same as a missing
/// file -- it returns defaults -- and so is only appropriate for a caller with
/// no live config to preserve (a fresh boot load). Any caller that already
/// holds a config it must not clobber on a cross-process collision uses
/// [`try_load`] and keeps its current config on [`LoadOutcome::Locked`].
pub fn load_or_default(db_path: &Path) -> Config {
    match try_load(db_path) {
        LoadOutcome::Loaded(cfg) => cfg,
        LoadOutcome::Locked => {
            tracing::warn!("config db locked by another handle, using defaults");
            default_config()
        }
    }
}

/// Read a `Config` out of an already-opened database, falling back per-field to
/// [`default_config`] for anything missing or unparsable. The whole read path
/// is wrapped in `catch_unwind_silently` because a file that opened cleanly can
/// still trip an internal redb assert during `begin_read`/`open_table`/a table
/// read; on any such panic (or a read `Err`) the whole `Config` degrades to
/// defaults.
fn read_config_from_db(db: Database) -> Config {
    let default = default_config();

    // Soundness of the `AssertUnwindSafe`/`catch_unwind` pair below mirrors
    // the argument on `open`: if redb panics partway through, the panic
    // aborts construction of values local to this closure (`read_txn`,
    // `table`, `cfg`, ...). The closure builds a fresh `Config` starting
    // from a clone of `default` and only returns it on success, so nothing
    // partially-read or partially-mutated escapes the unwind boundary.
    // `db` itself is only read (never written) here, so even though the
    // closure borrows it across the boundary, a panic mid-read leaves no
    // outward-visible mutation for later callers to observe.
    let default_for_closure = default.clone();
    let result = catch_unwind_silently(std::panic::AssertUnwindSafe(move || {
        let read_txn = db.begin_read()?;

        let mut cfg = default_for_closure;
        // Open every section table gracefully (`if let Ok`), never with `?`:
        // the compositor's scoped `save_displays` (review findings #2/#5)
        // creates a file that has ONLY `DB_DISPLAYS`, so a hard `?` on the
        // appearance table would make the whole load bail to defaults and drop
        // the displays it DID write. A missing table simply means that section
        // keeps its default.
        if let Ok(table) = read_txn.open_table(DB_APPEARANCE)
            && let Some(appearance) = read_json::<Appearance>(&table, KEY_APPEARANCE)
        {
            cfg.appearance = appearance;
        }
        if let Ok(behavior_table) = read_txn.open_table(DB_BEHAVIOR)
            && let Some(b) = read_json::<Behavior>(&behavior_table, KEY_BEHAVIOR)
        {
            cfg.behavior = b;
        }
        if let Ok(ws_table) = read_txn.open_table(DB_WORKSPACES)
            && let Some(names) = read_json::<Vec<String>>(&ws_table, KEY_WORKSPACES)
            && !names.is_empty()
        {
            cfg.workspace_names = names;
        }
        if let Ok(displays_table) = read_txn.open_table(DB_DISPLAYS)
            && let Some(displays) = read_json::<Vec<DisplayConfig>>(&displays_table, KEY_DISPLAYS)
        {
            cfg.displays = displays;
        }
        if let Ok(kb_table) = read_txn.open_table(DB_KEYBINDINGS)
            && read_json::<u64>(&kb_table, KEY_ACTION_COUNT).unwrap_or(0) > 0
        {
            let mut keybindings = HashMap::new();
            let mut cursor = kb_table.range(KEY_ACTION..).ok();
            while let Some(Ok((k, v))) = cursor.as_mut().and_then(|c| c.next()) {
                if let Ok(action) = std::str::from_utf8(k.value().as_bytes())
                    && let Some(action) = action.strip_prefix(KEY_ACTION)
                    && let Ok(combo) = serde_json::from_slice::<KeyCombo>(v.value())
                {
                    keybindings.insert(action.to_string(), combo);
                }
            }
            if !keybindings.is_empty() {
                cfg.keybindings = keybindings;
            }
        }
        Ok::<Config, redb::Error>(cfg)
    }));

    match result {
        Ok(Ok(cfg)) => cfg,
        Ok(Err(e)) => {
            tracing::warn!("config db read failed ({e}), using defaults");
            default
        }
        Err(payload) => {
            let msg = panic_payload_message(&payload);
            tracing::warn!("config db read panicked ({msg}), treating as corrupt and using defaults");
            default
        }
    }
}

impl Config {
    pub fn save(&self, db: &Database) -> Result<(), redb::Error> {
        let write_txn = db.begin_write()?;
        {
            let mut meta = write_txn.open_table(DB_META)?;
            let version = serde_json::to_vec(&crate::SCHEMA_VERSION).unwrap();
            meta.insert(KEY_SCHEMA_VERSION, version.as_slice())?;
            let mut appearance = write_txn.open_table(DB_APPEARANCE)?;
            let appearance_bytes = serde_json::to_vec(&self.appearance).unwrap();
            appearance.insert(KEY_APPEARANCE, appearance_bytes.as_slice())?;
            let mut behavior = write_txn.open_table(DB_BEHAVIOR)?;
            let behavior_bytes = serde_json::to_vec(&self.behavior).unwrap();
            behavior.insert(KEY_BEHAVIOR, behavior_bytes.as_slice())?;
            let mut workspaces = write_txn.open_table(DB_WORKSPACES)?;
            let workspace_bytes = serde_json::to_vec(&self.workspace_names).unwrap();
            workspaces.insert(KEY_WORKSPACES, workspace_bytes.as_slice())?;
            let mut displays = write_txn.open_table(DB_DISPLAYS)?;
            let displays_bytes = serde_json::to_vec(&self.displays).unwrap();
            displays.insert(KEY_DISPLAYS, displays_bytes.as_slice())?;
            let mut keybindings = write_txn.open_table(DB_KEYBINDINGS)?;
            // Review finding #3: clear every existing row FIRST. `insert`
            // upserts, so without this a binding that was removed from
            // `self.keybindings` keeps its stale `action:*` row on disk and
            // `load_or_default` (which reads every `action:*` key) resurrects
            // it -- silently undoing `prune_orphaned_workspace_bindings` and
            // any user deletion. Wiping the table makes the write authoritative:
            // exactly the current set persists, nothing more.
            keybindings.retain(|_k, _v| false)?;
            let count = serde_json::to_vec(&(self.keybindings.len() as u64)).unwrap();
            keybindings.insert(KEY_ACTION_COUNT, count.as_slice())?;
            for (action, combo) in &self.keybindings {
                let key = format!("{KEY_ACTION}{action}");
                let combo_bytes = serde_json::to_vec(combo).unwrap();
                keybindings.insert(key.as_str(), combo_bytes.as_slice())?;
            }
        }
        Ok(write_txn.commit()?)
    }

    /// Persist ONLY the displays section, touching no other table (review
    /// findings #2/#5). The compositor is a *second* writer of the shared
    /// config DB -- it saves after every applied output configuration -- but it
    /// must never own the appearance/keybindings/workspaces sections, which the
    /// settings app writes. [`Config::save`] rewrites the entire file and would
    /// clobber a concurrent settings-app edit to those sections with the
    /// compositor's (possibly stale) in-memory copy. This scoped save writes
    /// `DB_DISPLAYS` alone, so a display apply can never revert an unrelated
    /// on-disk edit. It is the ONLY save the compositor's persist path calls.
    pub fn save_displays(&self, db: &Database) -> Result<(), redb::Error> {
        let write_txn = db.begin_write()?;
        {
            let mut displays = write_txn.open_table(DB_DISPLAYS)?;
            let displays_bytes = serde_json::to_vec(&self.displays).unwrap();
            displays.insert(KEY_DISPLAYS, displays_bytes.as_slice())?;
        }
        Ok(write_txn.commit()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn defaults_are_parseable() {
        let cfg = default_config();
        assert_eq!(cfg.workspace_names, vec!["1", "2", "3", "4"]);
        assert!(cfg.keybindings.contains_key("close"));
        assert!(cfg.behavior.snap_enabled);
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("cfg.redb");
        let db = open(&path).unwrap();
        let cfg = default_config();
        cfg.save(&db).unwrap();
        drop(db);

        let loaded = load_or_default(&path);
        assert_eq!(loaded, cfg);
    }

    #[test]
    fn missing_db_returns_defaults() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nope.redb");
        let cfg = load_or_default(&path);
        assert_eq!(cfg, default_config());
    }

    #[test]
    fn default_displays_is_empty() {
        let cfg = default_config();
        assert!(cfg.displays.is_empty());
    }

    fn sample_displays() -> Vec<DisplayConfig> {
        vec![
            DisplayConfig {
                name: "DP-1".into(),
                enabled: true,
                width: 1920,
                height: 1080,
                refresh_mhz: 60000,
                x: 0,
                y: 0,
                scale: 1.0,
                transform: 0,
            },
            DisplayConfig {
                name: "HDMI-A-1".into(),
                enabled: false,
                width: 2560,
                height: 1440,
                refresh_mhz: 144000,
                x: 1920,
                y: 0,
                scale: 1.5,
                transform: 1,
            },
        ]
    }

    #[test]
    fn save_then_load_round_trips_displays() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("cfg.redb");
        let db = open(&path).unwrap();
        let mut cfg = default_config();
        cfg.displays = sample_displays();
        cfg.save(&db).unwrap();
        drop(db);

        let loaded = load_or_default(&path);
        assert_eq!(loaded.displays, sample_displays());
    }

    #[test]
    fn missing_displays_table_loads_empty() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("cfg.redb");
        let db = open(&path).unwrap();
        {
            // Write an "old-format" DB that has appearance/behavior/workspaces
            // but never touched DB_DISPLAYS, simulating a file saved before
            // this field existed.
            let write_txn = db.begin_write().unwrap();
            {
                let mut table = write_txn.open_table(DB_APPEARANCE).unwrap();
                let bytes = serde_json::to_vec(&default_config().appearance).unwrap();
                table.insert(KEY_APPEARANCE, bytes.as_slice()).unwrap();
            }
            write_txn.commit().unwrap();
        }
        drop(db);
        let cfg = load_or_default(&path);
        assert_eq!(cfg.displays, Vec::<DisplayConfig>::new());
    }

    #[test]
    fn save_displays_leaves_other_sections_intact() {
        // Review findings #2/#5: the compositor persists displays via
        // `save_displays`, which must NEVER touch appearance/behavior/
        // workspaces/keybindings. Seed a full non-default config, then have a
        // "compositor" write only a changed displays list; every other section
        // must survive byte-for-byte.
        let dir = tempdir().unwrap();
        let path = dir.path().join("cfg.redb");

        let mut seeded = default_config();
        seeded.workspace_names = vec!["alpha".into(), "beta".into()];
        seeded.appearance.wallpaper = Some("/custom/wall.png".into());
        seeded.keybindings.insert(
            "custom_action".into(),
            KeyCombo { modifiers: vec!["Super".into()], key: "z".into() },
        );
        seeded.displays = sample_displays();
        {
            let db = open(&path).unwrap();
            seeded.save(&db).unwrap();
        }

        // A different in-memory config that agrees on nothing but is only
        // allowed to persist its displays.
        let mut compositor_view = default_config();
        compositor_view.workspace_names = vec!["SHOULD_NOT_PERSIST".into()];
        compositor_view.appearance.wallpaper = Some("/wrong.png".into());
        compositor_view.displays = vec![DisplayConfig {
            name: "eDP-1".into(),
            enabled: true,
            width: 3840,
            height: 2160,
            refresh_mhz: 120000,
            x: 0,
            y: 0,
            scale: 2.0,
            transform: 0,
        }];
        {
            let db = open(&path).unwrap();
            compositor_view.save_displays(&db).unwrap();
        }

        let loaded = load_or_default(&path);
        // Displays are the compositor's new list...
        assert_eq!(loaded.displays, compositor_view.displays);
        // ...but every other section is still the settings-app's seeded values.
        assert_eq!(loaded.workspace_names, seeded.workspace_names);
        assert_eq!(loaded.appearance, seeded.appearance);
        assert_eq!(loaded.keybindings, seeded.keybindings);
        assert_eq!(loaded.behavior, seeded.behavior);
    }

    #[test]
    fn removed_keybinding_stays_removed_after_reload() {
        // Review finding #3: a binding deleted from `self.keybindings` must not
        // resurrect on the next load. `save` clears the keybindings table
        // before writing, so a removed action leaves no stale `action:*` row.
        let dir = tempdir().unwrap();
        let path = dir.path().join("cfg.redb");

        let mut cfg = default_config();
        cfg.keybindings.insert(
            "doomed".into(),
            KeyCombo { modifiers: vec!["Super".into()], key: "q".into() },
        );
        {
            let db = open(&path).unwrap();
            cfg.save(&db).unwrap();
        }
        assert!(load_or_default(&path).keybindings.contains_key("doomed"));

        // Remove it and re-save.
        cfg.keybindings.remove("doomed");
        {
            let db = open(&path).unwrap();
            cfg.save(&db).unwrap();
        }

        let loaded = load_or_default(&path);
        assert!(
            !loaded.keybindings.contains_key("doomed"),
            "a removed binding must not resurrect after reload"
        );
        assert_eq!(loaded.keybindings, cfg.keybindings);
    }

    #[test]
    fn try_load_reports_locked_when_file_already_open() {
        // Review finding #2: a cross-process open collision (redb takes a
        // whole-file lock) must surface as `Locked`, NOT as a silent
        // default-config wipe. Holding a live handle and calling `try_load` on
        // the same path reproduces the collision within one process.
        let dir = tempdir().unwrap();
        let path = dir.path().join("cfg.redb");

        let mut seeded = default_config();
        seeded.workspace_names = vec!["keepme".into()];
        {
            let db = open(&path).unwrap();
            seeded.save(&db).unwrap();
        }

        // Hold the DB open so the next open sees DatabaseAlreadyOpen.
        let _held = open(&path).unwrap();
        match try_load(&path) {
            LoadOutcome::Locked => {}
            LoadOutcome::Loaded(_) => {
                panic!("a locked file must report Locked, not Loaded(defaults) -- that is the wipe")
            }
        }

        // And `load_or_default` (the fresh-boot wrapper) still degrades to
        // defaults on that same lock, as documented.
        assert_eq!(load_or_default(&path), default_config());
    }

    #[test]
    fn corrupt_appearance_falls_back_per_field() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("cfg.redb");
        let db = open(&path).unwrap();
        {
            let write_txn = db.begin_write().unwrap();
            {
                let mut table = write_txn.open_table(DB_APPEARANCE).unwrap();
                table.insert(KEY_APPEARANCE, b"not json".as_slice()).unwrap();
            }
            write_txn.commit().unwrap();
        }
        drop(db);
        let cfg = load_or_default(&path);
        // Entire appearance value unparsable -> whole Appearance defaults.
        assert_eq!(cfg.appearance, default_config().appearance);
    }
}
