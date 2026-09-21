//! Launcher stores — pinned apps, tile groups, and per-app launch recency.
//!
//! `pinned` and `tile_groups` are fixed keys; recency is a **dynamic subtree**
//! (`/org/icedtea/launcher/recency/<app-id>/{count,last_seq}`) because the old
//! value was a map of tuples, which a one-level `Record` cannot hold (design
//! §"Per-data model", resolved open point 1).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use icedtea_registry::{KeySpec, Registry, RegistryError, Tag, Value};

use crate::{as_record_list, as_str, as_str_list, as_u64, spec};

pub const PINNED: &str = "/org/icedtea/launcher/pinned";
pub const TILE_GROUPS: &str = "/org/icedtea/launcher/tile_groups";
pub const RECENCY_ROOT: &str = "/org/icedtea/launcher/recency";

pub fn specs() -> Vec<KeySpec> {
    vec![
        spec(
            PINNED,
            Tag::StrList,
            Value::StrList(Vec::new()),
            "Pinned app ids in tile order.",
        ),
        spec(
            TILE_GROUPS,
            Tag::RecordList,
            Value::RecordList(Vec::new()),
            "Tile groups: records of {name, ids, size}.",
        ),
    ]
}

/// One named tile group. Mirrors the old `config::TileGroup` / shell type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TileGroup {
    pub name: String,
    pub ids: Vec<String>,
    #[serde(default = "default_tile_size")]
    pub size: u32,
}

pub fn default_tile_size() -> u32 {
    1
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LauncherConfig {
    pub pinned: Vec<String>,
    pub tile_groups: Vec<TileGroup>,
    pub recency: HashMap<String, (u64, u64)>,
}

/// One key per app, holding `Record{count, last_seq}`. A record at the app node
/// (rather than `/<app>/count` leaves) means `List(RECENCY_ROOT, false)` sees
/// every app directly — leaves under a non-stored intermediate node would be
/// invisible to a non-recursive listing.
pub fn recency_path(app_id: &str) -> String {
    format!("{RECENCY_ROOT}/{app_id}")
}

fn recency_record(count: u64, last_seq: u64) -> Value {
    let mut row = std::collections::BTreeMap::new();
    row.insert("count".to_string(), Value::Uint(count));
    row.insert("last_seq".to_string(), Value::Uint(last_seq));
    Value::Record(row)
}

fn tile_group_to_record(group: &TileGroup) -> std::collections::BTreeMap<String, Value> {
    let mut row = std::collections::BTreeMap::new();
    row.insert("name".to_string(), Value::Str(group.name.clone()));
    row.insert("ids".to_string(), Value::StrList(group.ids.clone()));
    row.insert("size".to_string(), Value::Uint(group.size as u64));
    row
}

fn record_to_tile_group(
    row: std::collections::BTreeMap<String, Value>,
) -> Result<TileGroup, RegistryError> {
    let name = row
        .get("name")
        .cloned()
        .map(|v| as_str(v, TILE_GROUPS))
        .transpose()?
        .unwrap_or_default();
    let ids = row
        .get("ids")
        .cloned()
        .map(|v| as_str_list(v, TILE_GROUPS))
        .transpose()?
        .unwrap_or_default();
    let size = row
        .get("size")
        .cloned()
        .map(|v| as_u64(v, TILE_GROUPS))
        .transpose()?
        .map(|n| n as u32)
        .unwrap_or_else(default_tile_size);
    Ok(TileGroup { name, ids, size })
}

impl LauncherConfig {
    pub fn load(reg: &Registry) -> Result<LauncherConfig, RegistryError> {
        let (pinned, _) = reg.get(PINNED)?;
        let (groups, _) = reg.get(TILE_GROUPS)?;
        let mut recency = HashMap::new();
        for (path, _) in reg.list(RECENCY_ROOT, false)? {
            let app_id = path
                .strip_prefix(&format!("{RECENCY_ROOT}/"))
                .unwrap_or(&path)
                .to_string();
            let (value, _) = reg.get(&path)?;
            let Value::Record(row) = value else {
                return Err(RegistryError::TypeMismatch {
                    path: path.clone(),
                    expected: Tag::Record,
                    got: value.tag(),
                });
            };
            let count = row
                .get("count")
                .cloned()
                .map(|v| as_u64(v, &path))
                .transpose()?
                .unwrap_or(0);
            let last_seq = row
                .get("last_seq")
                .cloned()
                .map(|v| as_u64(v, &path))
                .transpose()?
                .unwrap_or(0);
            recency.insert(app_id, (count, last_seq));
        }
        Ok(LauncherConfig {
            pinned: as_str_list(pinned, PINNED)?,
            tile_groups: as_record_list(groups, TILE_GROUPS)?
                .into_iter()
                .map(record_to_tile_group)
                .collect::<Result<_, _>>()?,
            recency,
        })
    }

    pub fn entries(&self) -> Vec<(String, Value)> {
        let mut entries = vec![
            (PINNED.to_string(), Value::StrList(self.pinned.clone())),
            (
                TILE_GROUPS.to_string(),
                Value::RecordList(self.tile_groups.iter().map(tile_group_to_record).collect()),
            ),
        ];
        let mut apps: Vec<_> = self.recency.iter().collect();
        apps.sort_by_key(|(id, _)| (*id).clone());
        for (app_id, (count, last_seq)) in apps {
            entries.push((recency_path(app_id), recency_record(*count, *last_seq)));
        }
        entries
    }

    /// Persist, replacing the whole launcher subtree: the fixed keys are set and
    /// the recency subtree is rewritten, so a removed app does not leave a
    /// dangling recency row.
    pub fn save(&self, reg: &Registry) -> Result<u64, RegistryError> {
        reg.reset(RECENCY_ROOT)?;
        reg.set_many(&self.entries())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_groups_round_trip_through_records() {
        let group = TileGroup {
            name: "Web".into(),
            ids: vec!["firefox".into(), "chromium".into()],
            size: 2,
        };
        let back = record_to_tile_group(tile_group_to_record(&group)).unwrap();
        assert_eq!(back, group);
    }
}
