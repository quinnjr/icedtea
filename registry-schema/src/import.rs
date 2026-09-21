//! One-shot importer from the pre-registry `config.redb`.
//!
//! This reads the old redb file directly (its table and key names are frozen —
//! they are the *old* format and must never change) and produces the
//! `(path, Value)` entries the registry would hold. It does not depend on
//! `icedtea-config`, so that crate can be deleted the moment the cutover lands.
//!
//! Two semantics are carried over deliberately (design §"Migration"):
//!
//! * a corrupt or unopenable file yields `Err`, and the caller decides to start
//!   on schema defaults rather than lose data silently;
//! * the A3 power-key backfill runs once, gated on the **fixed** boundary the
//!   old config used (`2`), never on a live version — so a user who removed a
//!   power binding does not get it resurrected.

use std::path::Path;

use redb::{Database, ReadableDatabase, TableDefinition};

use icedtea_registry::{RegistryError, Value};

use crate::{appearance, behavior, displays, keybindings, launcher, power, workspaces};

const T_META: TableDefinition<&'static str, &'static [u8]> = TableDefinition::new("meta");
const T_APPEARANCE: TableDefinition<&'static str, &'static [u8]> =
    TableDefinition::new("appearance");
const T_BEHAVIOR: TableDefinition<&'static str, &'static [u8]> = TableDefinition::new("behavior");
const T_POWER: TableDefinition<&'static str, &'static [u8]> = TableDefinition::new("power");
const T_WORKSPACES: TableDefinition<&'static str, &'static [u8]> =
    TableDefinition::new("workspaces");
const T_DISPLAYS: TableDefinition<&'static str, &'static [u8]> = TableDefinition::new("displays");
const T_LAUNCHER: TableDefinition<&'static str, &'static [u8]> = TableDefinition::new("launcher");
const T_KEYBINDINGS: TableDefinition<&'static str, &'static [u8]> =
    TableDefinition::new("keybindings");

const K_SCHEMA_VERSION: &str = "schema_version";
const K_ACTION_COUNT: &str = "action_count";
const K_ACTION_PREFIX: &str = "action:";

/// The fixed A3 boundary from `icedtea-config`. Gating on this literal (not a
/// live version) is what makes the backfill one-time.
const A3_SCHEMA_VERSION: u64 = 2;

/// Read a whole `config.redb` into registry entries. A missing file is a fresh
/// install and yields no entries; a present-but-broken file is an error.
pub fn import_config_db(path: &Path) -> Result<Vec<(String, Value)>, RegistryError> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let db = Database::open(path).map_err(storage_err)?;
    let read = db.begin_read().map_err(storage_err)?;

    let mut entries = Vec::new();

    if let Some(a) = read_json::<appearance::Appearance>(&read, T_APPEARANCE, "appearance")? {
        entries.extend(a.entries());
    }
    if let Some(b) = read_json::<behavior::Behavior>(&read, T_BEHAVIOR, "behavior")? {
        entries.extend(b.entries());
    }
    if let Some(p) = read_json::<power::Power>(&read, T_POWER, "power")? {
        entries.extend(p.entries());
    }
    if let Some(names) = read_json::<Vec<String>>(&read, T_WORKSPACES, "workspaces")?
        && !names.is_empty()
    {
        entries.push((workspaces::NAMES.to_string(), Value::StrList(names)));
    }
    if let Some(displays) =
        read_json::<Vec<displays::DisplayConfig>>(&read, T_DISPLAYS, "displays")?
    {
        entries.extend(displays.iter().flat_map(|d| d.entries()));
    }
    if let Some(launcher) = read_json::<launcher::LauncherConfig>(&read, T_LAUNCHER, "launcher")? {
        entries.extend(launcher.entries());
    }

    // Keybindings: the stored set replaces the defaults wholesale, exactly as
    // the old read path did; an absent/empty table means the defaults.
    let stored_bindings = read_keybindings(&read)?;
    let mut bindings = if stored_bindings.is_empty() {
        keybindings::default_bindings()
    } else {
        stored_bindings
    };
    backfill_power_keys(&read, &mut bindings)?;
    for (action, combo) in &bindings {
        entries.push((keybindings::path_for(action), Value::Record(combo.record())));
    }

    Ok(entries)
}

fn read_keybindings(
    read: &redb::ReadTransaction,
) -> Result<Vec<(String, keybindings::KeyCombo)>, RegistryError> {
    let Ok(table) = read.open_table(T_KEYBINDINGS) else {
        return Ok(Vec::new());
    };
    let count = table
        .get(K_ACTION_COUNT)
        .map_err(storage_err)?
        .and_then(|v| v.value().try_into().ok().map(u64::from_le_bytes))
        .unwrap_or(0);
    if count == 0 {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    let cursor = table.range(K_ACTION_PREFIX..).map_err(storage_err)?;
    for entry in cursor {
        let (key, value) = entry.map_err(storage_err)?;
        let Some(action) = key.value().strip_prefix(K_ACTION_PREFIX) else {
            continue;
        };
        let Ok(combo) = serde_json::from_slice::<keybindings::KeyCombo>(value.value()) else {
            continue;
        };
        out.push((action.to_string(), combo));
    }
    Ok(out)
}

/// The A3 migration, carried over verbatim in intent: a pre-A3 config whose
/// stored keybindings predate the power keys regains them, unless the action is
/// already bound or its chord is taken by another action.
///
/// The chord guard here compares the *canonical* key string and normalized
/// modifiers, not resolved keysyms. `icedtea-config` resolved keysyms to catch
/// aliases like `XF86_PowerOff` vs `KEY_XF86PowerOff`; the settings app has
/// always written the canonical spelling, so the two agree on every value this
/// importer can actually encounter.
fn backfill_power_keys(
    read: &redb::ReadTransaction,
    bindings: &mut Vec<(String, keybindings::KeyCombo)>,
) -> Result<(), RegistryError> {
    let version = read
        .open_table(T_META)
        .ok()
        .and_then(|table| table.get(K_SCHEMA_VERSION).ok().flatten())
        .and_then(|v| serde_json::from_slice::<u64>(v.value()).ok());
    let pre_a3 = match version {
        None => true,
        Some(v) => v < A3_SCHEMA_VERSION,
    };
    if !pre_a3 {
        return Ok(());
    }
    for (action, key) in keybindings::POWER_KEY_BINDINGS {
        if bindings.iter().any(|(a, _)| a == action) {
            continue;
        }
        let candidate = keybindings::KeyCombo {
            modifiers: Vec::new(),
            key: key.to_string(),
        };
        let chord_taken = bindings.iter().any(|(_, combo)| {
            combo.key == candidate.key && normalized(combo) == normalized(&candidate)
        });
        if chord_taken {
            continue;
        }
        bindings.push((action.to_string(), candidate));
    }
    Ok(())
}

fn normalized(combo: &keybindings::KeyCombo) -> Vec<String> {
    let mut mods: Vec<String> = combo.modifiers.iter().map(|m| m.to_uppercase()).collect();
    mods.sort();
    mods.dedup();
    mods
}

fn read_json<T: serde::de::DeserializeOwned>(
    read: &redb::ReadTransaction,
    table: TableDefinition<&'static str, &'static [u8]>,
    key: &'static str,
) -> Result<Option<T>, RegistryError> {
    let Ok(table) = read.open_table(table) else {
        return Ok(None);
    };
    match table.get(key).map_err(storage_err)? {
        Some(bytes) => Ok(serde_json::from_slice(bytes.value()).ok()),
        None => Ok(None),
    }
}

fn storage_err<E: std::fmt::Display>(err: E) -> RegistryError {
    RegistryError::Storage(err.to_string())
}

/// Import and persist in one call, stamping the current schema version.
pub fn import_into(
    reg: &icedtea_registry::Registry,
    config_db: &Path,
) -> Result<usize, RegistryError> {
    let entries = import_config_db(config_db)?;
    if !entries.is_empty() {
        reg.set_many(&entries)?;
    }
    Ok(entries.len())
}
