//! Behavior settings — the registry home of the old `config.behavior`.

use serde::{Deserialize, Serialize};

use icedtea_registry::{KeySpec, Registry, RegistryError, Tag, Value};

use crate::{as_bool, spec};

pub const RAISE_ON_FOCUS: &str = "/org/icedtea/behavior/raise_on_focus";
pub const HIDE_BAR_ON_FULLSCREEN: &str = "/org/icedtea/behavior/hide_bar_on_fullscreen";
pub const SNAP_ENABLED: &str = "/org/icedtea/behavior/snap_enabled";

pub fn specs() -> Vec<KeySpec> {
    vec![
        spec(
            RAISE_ON_FOCUS,
            Tag::Bool,
            Value::Bool(true),
            "Raise a window when it gains focus.",
        ),
        spec(
            HIDE_BAR_ON_FULLSCREEN,
            Tag::Bool,
            Value::Bool(true),
            "Hide the bar while a window is fullscreen.",
        ),
        spec(
            SNAP_ENABLED,
            Tag::Bool,
            Value::Bool(true),
            "Enable edge and corner window snapping.",
        ),
    ]
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Behavior {
    pub raise_on_focus: bool,
    pub hide_bar_on_fullscreen: bool,
    pub snap_enabled: bool,
}

impl Behavior {
    pub fn load(reg: &Registry) -> Result<Behavior, RegistryError> {
        let (raise_on_focus, _) = reg.get(RAISE_ON_FOCUS)?;
        let (hide_bar_on_fullscreen, _) = reg.get(HIDE_BAR_ON_FULLSCREEN)?;
        let (snap_enabled, _) = reg.get(SNAP_ENABLED)?;
        Ok(Behavior {
            raise_on_focus: as_bool(raise_on_focus, RAISE_ON_FOCUS)?,
            hide_bar_on_fullscreen: as_bool(hide_bar_on_fullscreen, HIDE_BAR_ON_FULLSCREEN)?,
            snap_enabled: as_bool(snap_enabled, SNAP_ENABLED)?,
        })
    }

    pub fn entries(&self) -> Vec<(String, Value)> {
        vec![
            (RAISE_ON_FOCUS.to_string(), Value::Bool(self.raise_on_focus)),
            (
                HIDE_BAR_ON_FULLSCREEN.to_string(),
                Value::Bool(self.hide_bar_on_fullscreen),
            ),
            (SNAP_ENABLED.to_string(), Value::Bool(self.snap_enabled)),
        ]
    }

    pub fn save(&self, reg: &Registry) -> Result<u64, RegistryError> {
        reg.set_many(&self.entries())
    }
}
