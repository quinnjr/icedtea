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

/// Load config, falling back to defaults for every individual field that is
/// missing or unparsable. Never returns Err and never panics: a file that
/// opens cleanly (see `open`) but has corrupt page/table data can still trip
/// an internal redb assert during `begin_read`, `open_table`, or a table
/// read/cursor call, so the entire read path below is wrapped in
/// `catch_unwind_silently` in addition to `open`'s own protection.
pub fn load_or_default(db_path: &Path) -> Config {
    let default = default_config();
    let db = match open(db_path) {
        Ok(db) => db,
        Err(e) => {
            tracing::warn!("config db unavailable ({e}), using defaults");
            return default;
        }
    };

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
        let table = read_txn.open_table(DB_APPEARANCE)?;

        let mut cfg = default_for_closure;
        if let Some(appearance) = read_json::<Appearance>(&table, KEY_APPEARANCE) {
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
            let mut keybindings = write_txn.open_table(DB_KEYBINDINGS)?;
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
