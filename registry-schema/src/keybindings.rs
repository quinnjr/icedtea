//! Keybindings — a **dynamic** domain: one key per action, so there is no
//! `KeySpec` (the action names are not known at build time). Typed helpers
//! instead, over the open fallback.

use serde::{Deserialize, Serialize};

use icedtea_registry::{KeySpec, Registry, RegistryError, Tag, Value};

use crate::{as_str, as_str_list};

pub const ROOT: &str = "/org/icedtea/keybindings";

/// The A3 power-key bindings that a pre-A3 config must be backfilled with.
/// Kept here (not in `icedtea-config`, which is going away) so the migration
/// and the tests share one source.
pub const POWER_KEY_BINDINGS: [(&str, &str); 3] = [
    ("spawn:icedtea-session lock", "XF86_PowerOff"),
    ("spawn:icedtea-session suspend", "XF86_Sleep"),
    ("spawn:icedtea-session hibernate", "XF86_Hibernate"),
];

/// Action names are arbitrary (`snap:right`, `spawn:icedtea-session lock`), so
/// the segment is percent-encoded — the grammar stays strict, the name stays
/// lossless.
pub fn path_for(action: &str) -> String {
    format!("{ROOT}/{}", icedtea_registry::path::encode_segment(action))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyCombo {
    pub modifiers: Vec<String>,
    pub key: String,
}

impl KeyCombo {
    pub fn record(&self) -> std::collections::BTreeMap<String, Value> {
        let mut row = std::collections::BTreeMap::new();
        row.insert(
            "modifiers".to_string(),
            Value::StrList(self.modifiers.clone()),
        );
        row.insert("key".to_string(), Value::Str(self.key.clone()));
        row
    }

    pub fn from_record(
        row: &std::collections::BTreeMap<String, Value>,
        path: &str,
    ) -> Result<KeyCombo, RegistryError> {
        let modifiers = row
            .get("modifiers")
            .cloned()
            .map(|v| as_str_list(v, path))
            .transpose()?
            .unwrap_or_default();
        let key = row
            .get("key")
            .cloned()
            .map(|v| as_str(v, path))
            .transpose()?
            .unwrap_or_default();
        Ok(KeyCombo { modifiers, key })
    }
}

/// Every stored binding, as `(action, combo)`.
pub fn load_all(reg: &Registry) -> Result<Vec<(String, KeyCombo)>, RegistryError> {
    let mut out = Vec::new();
    for (path, _) in reg.list(ROOT, false)? {
        let Some(encoded) = path.strip_prefix(&format!("{ROOT}/")) else {
            continue;
        };
        let action = icedtea_registry::path::decode_segment(encoded);
        let (value, _) = reg.get(&path)?;
        let Value::Record(row) = value else {
            return Err(RegistryError::TypeMismatch {
                path: path.clone(),
                expected: Tag::Record,
                got: value.tag(),
            });
        };
        out.push((action.to_string(), KeyCombo::from_record(&row, &path)?));
    }
    Ok(out)
}

pub fn set(reg: &Registry, action: &str, combo: &KeyCombo) -> Result<u64, RegistryError> {
    reg.set(&path_for(action), &Value::Record(combo.record()))
}

pub fn remove(reg: &Registry, action: &str) -> Result<u64, RegistryError> {
    reg.unset(&path_for(action))
}

/// The default bindings, mirroring `icedtea-config`'s `default_config`. Used by
/// the importer when the old config had no keybindings table at all.
pub fn default_bindings() -> Vec<(String, KeyCombo)> {
    let mut out = Vec::new();
    let mut add = |action: &str, mods: &[&str], key: &str| {
        out.push((
            action.to_string(),
            KeyCombo {
                modifiers: mods.iter().map(|m| (*m).to_string()).collect(),
                key: key.to_string(),
            },
        ));
    };
    add("close", &["SUPER"], "KEY_q");
    add("fullscreen", &["SUPER"], "KEY_f");
    add("reload", &["SUPER", "SHIFT"], "KEY_r");
    add("quit", &["SUPER", "SHIFT"], "KEY_q");
    add("cycle:alt_tab", &["SUPER"], "KEY_Tab");
    add("spawn:terminal", &["SUPER"], "KEY_Return");
    add("snap:left", &["SUPER"], "KEY_Left");
    add("snap:right", &["SUPER"], "KEY_Right");
    add("snap:up", &["SUPER"], "KEY_Up");
    add("snap:down", &["SUPER"], "KEY_Down");
    add("snap:restore", &["SUPER", "SHIFT"], "KEY_Left");
    for n in 1..=9u32 {
        add(&format!("workspace:{n}"), &["SUPER"], &format!("KEY_{n}"));
        add(
            &format!("move_to_workspace:{n}"),
            &["SUPER", "CTRL"],
            &format!("KEY_{n}"),
        );
    }
    for (action, key) in POWER_KEY_BINDINGS {
        add(action, &[], key);
    }
    out
}

/// Dynamic domains are unregistered, so `specs()` is empty — present for a
/// uniform domain interface.
pub fn specs() -> Vec<KeySpec> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combo_round_trips_through_a_record() {
        let combo = KeyCombo {
            modifiers: vec!["SUPER".into(), "SHIFT".into()],
            key: "KEY_q".into(),
        };
        let back = KeyCombo::from_record(&combo.record(), "/x").unwrap();
        assert_eq!(back, combo);
    }

    #[test]
    fn default_bindings_include_the_power_keys() {
        let bindings = default_bindings();
        for (action, key) in POWER_KEY_BINDINGS {
            let combo = bindings
                .iter()
                .find(|(a, _)| a == action)
                .map(|(_, c)| c)
                .unwrap_or_else(|| panic!("missing {action}"));
            assert!(combo.modifiers.is_empty());
            assert_eq!(combo.key, key);
        }
    }
}
