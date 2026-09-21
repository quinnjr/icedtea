//! Power/session policy — the registry home of the old `config.power`.

use serde::{Deserialize, Serialize};

use icedtea_registry::{KeySpec, Registry, RegistryError, Tag, Value};

use crate::{as_bool, as_opt_str, as_opt_u64, spec, spec_nullable};

pub const LOCKER_COMMAND: &str = "/org/icedtea/power/locker_command";
pub const LOCK_IDLE_TIMEOUT_MS: &str = "/org/icedtea/power/lock_idle_timeout_ms";
pub const LOCK_BEFORE_SLEEP: &str = "/org/icedtea/power/lock_before_sleep";

pub fn specs() -> Vec<KeySpec> {
    vec![
        spec_nullable(
            LOCKER_COMMAND,
            Tag::Str,
            "Shell command that secures the screen, or null when no locker is configured.",
        ),
        spec_nullable(
            LOCK_IDLE_TIMEOUT_MS,
            Tag::Uint,
            "Lock after this many milliseconds of seat idle, or null to disable idle-lock.",
        ),
        spec(
            LOCK_BEFORE_SLEEP,
            Tag::Bool,
            Value::Bool(true),
            "Attempt to lock before suspend/hibernate (a no-op without a locker command).",
        ),
    ]
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Power {
    pub locker_command: Option<String>,
    pub lock_idle_timeout_ms: Option<u64>,
    pub lock_before_sleep: bool,
}

impl Default for Power {
    fn default() -> Self {
        Power {
            locker_command: None,
            lock_idle_timeout_ms: None,
            lock_before_sleep: true,
        }
    }
}

impl Power {
    pub fn load(reg: &Registry) -> Result<Power, RegistryError> {
        let (locker_command, _) = reg.get(LOCKER_COMMAND)?;
        let (lock_idle_timeout_ms, _) = reg.get(LOCK_IDLE_TIMEOUT_MS)?;
        let (lock_before_sleep, _) = reg.get(LOCK_BEFORE_SLEEP)?;
        Ok(Power {
            locker_command: as_opt_str(locker_command, LOCKER_COMMAND)?,
            lock_idle_timeout_ms: as_opt_u64(lock_idle_timeout_ms, LOCK_IDLE_TIMEOUT_MS)?,
            lock_before_sleep: as_bool(lock_before_sleep, LOCK_BEFORE_SLEEP)?,
        })
    }

    pub fn entries(&self) -> Vec<(String, Value)> {
        vec![
            (
                LOCKER_COMMAND.to_string(),
                match &self.locker_command {
                    Some(cmd) => Value::Str(cmd.clone()),
                    None => Value::Null,
                },
            ),
            (
                LOCK_IDLE_TIMEOUT_MS.to_string(),
                match self.lock_idle_timeout_ms {
                    Some(ms) => Value::Uint(ms),
                    None => Value::Null,
                },
            ),
            (
                LOCK_BEFORE_SLEEP.to_string(),
                Value::Bool(self.lock_before_sleep),
            ),
        ]
    }

    pub fn save(&self, reg: &Registry) -> Result<u64, RegistryError> {
        reg.set_many(&self.entries())
    }
}
