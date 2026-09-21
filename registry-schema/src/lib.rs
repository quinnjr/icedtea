//! `icedtea-registry-schema` — the first-party schema union and the typed
//! domain structs the rest of the desktop uses.
//!
//! Two kinds of key live in the registry (design §"Per-field keys"):
//!
//! * **Fixed keys** — `appearance`, `behavior`, `power`, the workspace-name
//!   list, and the launcher's top-level stores. These are declared with
//!   [`KeySpec`]s, so they are type-checked, have schema defaults, and are
//!   reset/reverted per field.
//! * **Dynamic keys** — one per keybinding action, one subtree per display,
//!   one subtree per app for launch recency, one per MIME type's handler list.
//!   Their names are not known at build time, so they are *unregistered* and
//!   ride the open fallback (design Q8): still typed on the wire, still
//!   watchable, but without a declared default. This crate gives them typed
//!   helpers instead of specs.

pub mod appearance;
pub mod behavior;
pub mod displays;
pub mod import;
pub mod keybindings;
pub mod keys;
pub mod launcher;
pub mod power;
pub mod scan;
pub mod workspaces;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use icedtea_registry::{KeySpec, Registry, RegistryError, Schema, Value};

pub use appearance::{Appearance, Palette};
pub use behavior::Behavior;
pub use displays::DisplayConfig;
pub use keybindings::KeyCombo;
pub use keys::{MODIFIER_TOKENS, key_name_to_keysym, keysym_to_key_name};
pub use launcher::{LauncherConfig, TileGroup, default_tile_size};
pub use power::Power;

/// The whole settings surface in one value, with the same field names the old
/// `icedtea-config::Config` had. Consumers hold this in memory exactly as
/// before; what changed is where it is loaded from and saved to.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Config {
    pub keybindings: HashMap<String, KeyCombo>,
    pub appearance: Appearance,
    pub launcher: LauncherConfig,
    pub behavior: Behavior,
    pub power: Power,
    pub workspace_names: Vec<String>,
    pub displays: Vec<DisplayConfig>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            keybindings: keybindings::default_bindings().into_iter().collect(),
            appearance: Appearance {
                bar_position: "bottom".into(),
                bar_height: 42,
                corner_radius: 8,
                snap_gap: 8,
                palette: Palette {
                    background: "#1e1e2e".into(),
                    foreground: "#cdd6f4".into(),
                    accent: "#89b4fa".into(),
                },
                wallpaper: None,
            },
            launcher: LauncherConfig::default(),
            behavior: Behavior {
                raise_on_focus: true,
                hide_bar_on_fullscreen: true,
                snap_enabled: true,
            },
            power: Power::default(),
            workspace_names: vec!["1".into(), "2".into(), "3".into(), "4".into()],
            displays: Vec::new(),
        }
    }
}

/// The old name for [`Config::default`], kept so consumer call sites are a
/// one-word change.
pub fn default_config() -> Config {
    Config::default()
}

/// The registry database path, named for the old helper so consumers that only
/// wanted a path keep compiling.
pub fn default_db_path() -> PathBuf {
    icedtea_registry::default_db_path()
}

/// The handle every consumer should open: the daemon when it is reachable,
/// otherwise a direct store on the same file.
///
/// `$ICEDTEA_REGISTRY_DB`, when set, forces a direct store at that path and
/// never touches the bus. That is the isolation seam for tests that spawn a
/// real binary with a temp `XDG_CONFIG_HOME` — without it the child would talk
/// to whatever registry daemon happens to be running on the developer's
/// session bus and ignore the temp file entirely.
pub fn connect() -> Result<Registry, RegistryError> {
    if let Some(path) = std::env::var_os("ICEDTEA_REGISTRY_DB") {
        let store = icedtea_registry::Store::open(Path::new(&path), schema())?;
        return Ok(Registry::direct(std::sync::Arc::new(
            std::sync::Mutex::new(store),
        )));
    }
    // `Registry::connect` only builds a lazy proxy — it does not check that
    // anything owns the name. Probe with a cheap call so "no daemon" falls back
    // to a direct store instead of handing back a handle whose every method
    // fails with ServiceUnknown (which silently turned every write into a
    // no-op).
    match Registry::connect() {
        Ok(registry) if registry.seq().is_ok() => Ok(registry),
        Ok(_) => {
            tracing::warn!("no registry daemon owns the bus name; opening the store directly");
            direct_store()
        }
        Err(err) => {
            tracing::warn!(
                %err,
                "registry daemon unavailable; opening the store directly (degraded)"
            );
            direct_store()
        }
    }
}

fn direct_store() -> Result<Registry, RegistryError> {
    let store = icedtea_registry::Store::open(&icedtea_registry::default_db_path(), schema())?;
    Ok(Registry::direct(std::sync::Arc::new(
        std::sync::Mutex::new(store),
    )))
}

impl Config {
    /// Read every section from the registry. Keybindings and displays are
    /// dynamic domains; an empty keybindings set means "the defaults", exactly
    /// as the old read path treated an absent keybindings table.
    pub fn load(reg: &Registry) -> Result<Config, String> {
        let mut keybindings: HashMap<String, KeyCombo> = keybindings::load_all(reg)
            .map_err(|e| e.to_string())?
            .into_iter()
            .collect();
        if keybindings.is_empty() {
            keybindings = keybindings::default_bindings().into_iter().collect();
        }
        Ok(Config {
            keybindings,
            appearance: Appearance::load(reg).map_err(|e| e.to_string())?,
            launcher: LauncherConfig::load(reg).map_err(|e| e.to_string())?,
            behavior: Behavior::load(reg).map_err(|e| e.to_string())?,
            power: Power::load(reg).map_err(|e| e.to_string())?,
            workspace_names: workspaces::load(reg).map_err(|e| e.to_string())?,
            displays: DisplayConfig::load_all(reg).map_err(|e| e.to_string())?,
        })
    }

    /// Read, falling back to defaults when the registry is unreachable. The
    /// fresh-boot path: a caller with no live config to preserve.
    pub fn load_or_default(reg: &Registry) -> Config {
        Self::load(reg).unwrap_or_default()
    }

    /// Persist every section. The dynamic subtrees (keybindings, displays, and
    /// launcher recency) are reset first, so a removed action/display/app
    /// leaves no stale row — the registry equivalent of the old `save` wiping
    /// its keybindings table.
    pub fn save(&self, reg: &Registry) -> Result<(), String> {
        reg.reset(keybindings::ROOT).map_err(|e| e.to_string())?;
        reg.reset(displays::ROOT).map_err(|e| e.to_string())?;
        reg.reset(launcher::RECENCY_ROOT)
            .map_err(|e| e.to_string())?;
        let mut entries = self.appearance.entries();
        entries.extend(self.behavior.entries());
        entries.extend(self.power.entries());
        entries.push((
            workspaces::NAMES.to_string(),
            Value::StrList(self.workspace_names.clone()),
        ));
        entries.extend(self.launcher.entries());
        entries.extend(self.displays.iter().flat_map(|d| d.entries()));
        for (action, combo) in &self.keybindings {
            entries.push((keybindings::path_for(action), Value::Record(combo.record())));
        }
        if !entries.is_empty() {
            reg.set_many(&entries).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    /// Persist only the displays section (the compositor's scoped write).
    pub fn save_displays(&self, reg: &Registry) -> Result<(), String> {
        DisplayConfig::save_all(reg, &self.displays)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// Persist only the launcher section (the shell's scoped write).
    pub fn save_launcher(&self, reg: &Registry) -> Result<(), String> {
        self.launcher
            .save(reg)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

/// The union of every fixed-key domain's specs, in one list for the daemon to
/// compile at boot and each consumer to compile for typed access.
pub fn first_party_schema() -> Vec<KeySpec> {
    let mut specs = Vec::new();
    specs.extend(appearance::specs());
    specs.extend(behavior::specs());
    specs.extend(power::specs());
    specs.extend(workspaces::specs());
    specs.extend(launcher::specs());
    specs
}

/// Compile the union. Panics only on a build defect — a duplicate or
/// inconsistent path — which is exactly the intent: a schema drift must stop
/// the daemon from starting, not degrade silently.
pub fn schema() -> Schema {
    Schema::new(first_party_schema()).expect("first-party schema is well-formed")
}

/// Schema revision the first-party keys were introduced at. Bumped when a
/// later migration needs a fixed boundary (design §"Versioning").
pub(crate) const SINCE: u64 = 1;

/// A plain key: one tag, one default, no constraints.
pub(crate) fn spec(
    path: &'static str,
    tag: icedtea_registry::Tag,
    default: Value,
    description: &'static str,
) -> KeySpec {
    KeySpec {
        path,
        tag,
        default,
        description,
        choices: None,
        range: None,
        since: SINCE,
        nullable: false,
    }
}

/// A key restricted to an explicit set of values.
pub(crate) fn spec_choices(
    path: &'static str,
    tag: icedtea_registry::Tag,
    default: Value,
    description: &'static str,
    choices: Vec<Value>,
) -> KeySpec {
    KeySpec {
        choices: Some(choices),
        ..spec(path, tag, default, description)
    }
}

/// A key restricted to an inclusive numeric range.
pub(crate) fn spec_range(
    path: &'static str,
    tag: icedtea_registry::Tag,
    default: Value,
    description: &'static str,
    low: Value,
    high: Value,
) -> KeySpec {
    KeySpec {
        range: Some((low, high)),
        ..spec(path, tag, default, description)
    }
}

/// A key that also accepts [`Value::Null`], so an `Option`-typed setting can
/// be explicitly empty.
pub(crate) fn spec_nullable(
    path: &'static str,
    tag: icedtea_registry::Tag,
    description: &'static str,
) -> KeySpec {
    KeySpec {
        nullable: true,
        ..spec(path, tag, Value::Null, description)
    }
}

pub(crate) fn as_str(value: Value, path: &str) -> Result<String, RegistryError> {
    match value {
        Value::Str(s) => Ok(s),
        other => Err(wrong(path, "string", other)),
    }
}

pub(crate) fn as_u64(value: Value, path: &str) -> Result<u64, RegistryError> {
    match value {
        Value::Uint(n) => Ok(n),
        other => Err(wrong(path, "uint", other)),
    }
}

pub(crate) fn as_bool(value: Value, path: &str) -> Result<bool, RegistryError> {
    match value {
        Value::Bool(b) => Ok(b),
        other => Err(wrong(path, "bool", other)),
    }
}

pub(crate) fn as_str_list(value: Value, path: &str) -> Result<Vec<String>, RegistryError> {
    match value {
        Value::StrList(list) => Ok(list),
        other => Err(wrong(path, "str_list", other)),
    }
}

pub(crate) fn as_record_list(
    value: Value,
    path: &str,
) -> Result<Vec<BTreeMap<String, Value>>, RegistryError> {
    match value {
        Value::RecordList(rows) => Ok(rows),
        other => Err(wrong(path, "record_list", other)),
    }
}

/// Decode a nullable string key: [`Value::Null`] is `None`.
pub(crate) fn as_opt_str(value: Value, path: &str) -> Result<Option<String>, RegistryError> {
    match value {
        Value::Null => Ok(None),
        Value::Str(s) => Ok(Some(s)),
        other => Err(wrong(path, "string or null", other)),
    }
}

/// Decode a nullable uint key: [`Value::Null`] is `None`.
pub(crate) fn as_opt_u64(value: Value, path: &str) -> Result<Option<u64>, RegistryError> {
    match value {
        Value::Null => Ok(None),
        Value::Uint(n) => Ok(Some(n)),
        other => Err(wrong(path, "uint or null", other)),
    }
}

fn wrong(path: &str, expected: &str, got: Value) -> RegistryError {
    RegistryError::TypeMismatch {
        path: path.to_string(),
        expected: match expected {
            "string" | "string or null" => icedtea_registry::Tag::Str,
            "uint" | "uint or null" => icedtea_registry::Tag::Uint,
            "bool" => icedtea_registry::Tag::Bool,
            "str_list" => icedtea_registry::Tag::StrList,
            _ => got.tag(),
        },
        got: got.tag(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_party_schema_compiles_without_duplicates() {
        let schema = schema();
        assert!(!schema.is_empty());
    }

    #[test]
    fn every_default_satisfies_its_own_spec() {
        // Compilation already checks the tag, but this also exercises the
        // choices/range path for every declared key.
        let schema = schema();
        for spec in schema.specs() {
            assert!(
                schema.validate_write(spec.path, &spec.default).is_ok(),
                "default for {} is rejected by its own spec",
                spec.path
            );
        }
    }
}
