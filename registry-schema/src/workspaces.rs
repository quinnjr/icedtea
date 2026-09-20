//! Workspace names — the registry home of the old `config.workspace_names`.

use icedtea_registry::{KeySpec, Registry, RegistryError, Tag, Value};

use crate::{as_str_list, spec};

pub const NAMES: &str = "/org/icedtea/workspaces/names";

pub fn specs() -> Vec<KeySpec> {
    vec![spec(
        NAMES,
        Tag::StrList,
        Value::StrList(vec!["1".into(), "2".into(), "3".into(), "4".into()]),
        "Workspace names in order.",
    )]
}

pub fn load(reg: &Registry) -> Result<Vec<String>, RegistryError> {
    let (names, _) = reg.get(NAMES)?;
    as_str_list(names, NAMES)
}

pub fn save(reg: &Registry, names: &[String]) -> Result<u64, RegistryError> {
    reg.set(NAMES, &Value::StrList(names.to_vec()))
}
