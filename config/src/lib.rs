use std::collections::HashMap;
use std::path::{Path, PathBuf};

use redb::{Database, ReadableDatabase, ReadableTable};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

pub mod defaults;
pub mod schema;

use schema::*;

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
pub fn open(db_path: &Path) -> Result<Database, redb::Error> {
    if let Some(dir) = db_path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    Database::create(db_path).map_err(Into::into)
}

fn read_json<T: DeserializeOwned>(
    table: &impl ReadableTable<&'static str, &'static [u8]>,
    key: &'static str,
) -> Option<T> {
    table.get(key).ok().flatten().and_then(|v| serde_json::from_slice(v.value()).ok())
}

/// Load config, falling back to defaults for every individual field that is
/// missing or unparsable. Never returns Err.
pub fn load_or_default(db_path: &Path) -> Config {
    let default = default_config();
    let db = match open(db_path) {
        Ok(db) => db,
        Err(e) => {
            tracing::warn!("config db unavailable ({e}), using defaults");
            return default;
        }
    };
    let Ok(read_txn) = db.begin_read() else { return default };
    let Ok(table) = read_txn.open_table(DB_APPEARANCE) else { return default };

    let mut cfg = default;
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
    cfg
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
