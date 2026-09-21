//! Displays — a **dynamic** domain: one key per output holding a `Record` of
//! that output's fields (`/org/icedtea/displays/<output>`), because the set of
//! outputs is only known at runtime and hot-unplug adds/removes keys. One key
//! per output (rather than `/<output>/<field>` leaves) keeps
//! `List(ROOT, false)` able to see every output directly. Unregistered, so it
//! rides the open fallback with typed helpers.

use serde::{Deserialize, Serialize};

use icedtea_registry::{KeySpec, Registry, RegistryError, Tag, Value};

use crate::{as_bool, as_u64};

pub const ROOT: &str = "/org/icedtea/displays";

pub fn output_root(name: &str) -> String {
    format!("{ROOT}/{name}")
}

/// Mirror of the old `contract::DisplayConfig`, with conversions to/from the
/// wire type at the compositor's `apply_display_config` / upsert seam.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DisplayConfig {
    pub name: String,
    pub enabled: bool,
    pub width: i32,
    pub height: i32,
    pub refresh_mhz: i32,
    pub x: i32,
    pub y: i32,
    pub scale: f64,
    pub transform: i32,
}

impl From<icedtea_contract::DisplayConfig> for DisplayConfig {
    fn from(d: icedtea_contract::DisplayConfig) -> Self {
        DisplayConfig {
            name: d.name,
            enabled: d.enabled,
            width: d.width,
            height: d.height,
            refresh_mhz: d.refresh_mhz,
            x: d.x,
            y: d.y,
            scale: d.scale,
            transform: d.transform,
        }
    }
}

impl From<DisplayConfig> for icedtea_contract::DisplayConfig {
    fn from(d: DisplayConfig) -> Self {
        icedtea_contract::DisplayConfig {
            name: d.name,
            enabled: d.enabled,
            width: d.width,
            height: d.height,
            refresh_mhz: d.refresh_mhz,
            x: d.x,
            y: d.y,
            scale: d.scale,
            transform: d.transform,
        }
    }
}

fn record_of(display: &DisplayConfig) -> Value {
    let mut row = std::collections::BTreeMap::new();
    row.insert("enabled".to_string(), Value::Bool(display.enabled));
    row.insert("width".to_string(), Value::Int(display.width as i64));
    row.insert("height".to_string(), Value::Int(display.height as i64));
    row.insert(
        "refresh_mhz".to_string(),
        Value::Int(display.refresh_mhz as i64),
    );
    row.insert("x".to_string(), Value::Int(display.x as i64));
    row.insert("y".to_string(), Value::Int(display.y as i64));
    row.insert("scale".to_string(), Value::Double(display.scale));
    row.insert(
        "transform".to_string(),
        Value::Uint(display.transform.max(0) as u64),
    );
    Value::Record(row)
}

impl DisplayConfig {
    pub fn entries(&self) -> Vec<(String, Value)> {
        vec![(output_root(&self.name), record_of(self))]
    }

    fn from_record(
        name: &str,
        row: std::collections::BTreeMap<String, Value>,
    ) -> Result<Self, RegistryError> {
        let path = output_root(name);
        let int = |field: &str| -> Result<i32, RegistryError> {
            match row.get(field).cloned() {
                Some(Value::Int(n)) => Ok(n as i32),
                Some(Value::Uint(n)) => Ok(n as i32),
                Some(other) => Err(RegistryError::TypeMismatch {
                    path: path.clone(),
                    expected: Tag::Int,
                    got: other.tag(),
                }),
                None => Ok(0),
            }
        };
        let enabled = row
            .get("enabled")
            .cloned()
            .map(|v| as_bool(v, &path))
            .transpose()?
            .unwrap_or(true);
        let scale = match row.get("scale").cloned() {
            Some(Value::Double(s)) => s,
            Some(other) => {
                return Err(RegistryError::TypeMismatch {
                    path: path.clone(),
                    expected: Tag::Double,
                    got: other.tag(),
                });
            }
            None => 1.0,
        };
        let transform = row
            .get("transform")
            .cloned()
            .map(|v| as_u64(v, &path))
            .transpose()?
            .unwrap_or(0) as i32;
        Ok(DisplayConfig {
            name: name.to_string(),
            enabled,
            width: int("width")?,
            height: int("height")?,
            refresh_mhz: int("refresh_mhz")?,
            x: int("x")?,
            y: int("y")?,
            scale,
            transform,
        })
    }

    pub fn load_all(reg: &Registry) -> Result<Vec<DisplayConfig>, RegistryError> {
        let mut out = Vec::new();
        for (path, _) in reg.list(ROOT, false)? {
            let Some(name) = path.strip_prefix(&format!("{ROOT}/")) else {
                continue;
            };
            let (value, _) = reg.get(&path)?;
            let Value::Record(row) = value else {
                return Err(RegistryError::TypeMismatch {
                    path: path.clone(),
                    expected: Tag::Record,
                    got: value.tag(),
                });
            };
            out.push(DisplayConfig::from_record(name, row)?);
        }
        Ok(out)
    }

    /// Persist the whole display set, replacing the subtree so a removed output
    /// leaves nothing behind.
    pub fn save_all(reg: &Registry, displays: &[DisplayConfig]) -> Result<u64, RegistryError> {
        reg.reset(ROOT)?;
        let entries: Vec<(String, Value)> = displays.iter().flat_map(|d| d.entries()).collect();
        if entries.is_empty() {
            return reg.seq();
        }
        reg.set_many(&entries)
    }
}

/// Dynamic domain — no static specs.
pub fn specs() -> Vec<KeySpec> {
    Vec::new()
}
