//! Best-effort `org.icedtea.Compositor` reload client. After a config edit is
//! written to the redb store, the settings app asks the running compositor
//! to re-read it via `ReloadConfig`. If no compositor is running (bus or
//! service absent), that's fine -- the write already landed on disk and
//! will apply on the compositor's next start.

use std::path::Path;

use icedtea_config::Config;
use icedtea_contract::{COMPOSITOR_BUS_NAME, COMPOSITOR_PATH};

// zbus's #[interface] exposes Rust methods in PascalCase, so the wire
// member is ReloadConfig (matching FocusWindow/CloseWindow/SetWorkspace in
// shell/src/compositor_client.rs).
const COMPOSITOR_IFACE: &str = "org.icedtea.Compositor";

/// The outcome of a [`ReloadClient::reload`] call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReloadOutcome {
    /// The compositor accepted `ReloadConfig`.
    Reloaded,
    /// No compositor is reachable on the session bus (or it rejected the
    /// call). The config write already happened; this is not an error.
    CompositorAbsent,
}

/// Issues `org.icedtea.Compositor`'s `ReloadConfig` from the GTK thread. Method
/// calls are no-reply and sub-millisecond, so a blocking connection here is
/// fine (mirrors `shell::compositor_client::CompositorProxy`).
pub struct ReloadClient {
    conn: Option<zbus::blocking::Connection>,
}

impl ReloadClient {
    /// Connect to the session bus if one is reachable. Never panics --
    /// absence of a bus (e.g. in a headless test) just leaves `conn: None`,
    /// so every subsequent `reload()` reports `CompositorAbsent`.
    pub fn new() -> Self {
        Self::on_bus(None)
    }

    /// [`ReloadClient::new`], against one named bus address.
    ///
    /// `None` is `$DBUS_SESSION_BUS_ADDRESS`, as before. A test passes an
    /// address instead: the environment is process-global, and a unit test
    /// that reached the *developer's* live bus would blocking-call
    /// `ReloadConfig` on whatever owns `org.icedtea.Compositor` there —
    /// forcing a real compositor to reload mid-`cargo test`. An address
    /// nothing is listening on fails here, immediately, and every later
    /// `reload()` reports `CompositorAbsent`.
    #[must_use]
    pub fn on_bus(address: Option<&str>) -> Self {
        let conn = match address {
            Some(address) => zbus::blocking::connection::Builder::address(address)
                .and_then(zbus::blocking::connection::Builder::build)
                .ok(),
            None => zbus::blocking::Connection::session().ok(),
        };
        ReloadClient { conn }
    }

    /// Ask the running compositor to re-read config. Best-effort: if the bus
    /// or the WM service is absent, report `CompositorAbsent` (the redb write
    /// already happened; the change applies on next compositor start).
    pub fn reload(&self) -> ReloadOutcome {
        let Some(conn) = &self.conn else {
            return ReloadOutcome::CompositorAbsent;
        };
        match conn.call_method(
            Some(COMPOSITOR_BUS_NAME),
            COMPOSITOR_PATH,
            Some(COMPOSITOR_IFACE),
            "ReloadConfig",
            &(),
        ) {
            Ok(_) => ReloadOutcome::Reloaded,
            Err(_) => ReloadOutcome::CompositorAbsent,
        }
    }
}

impl Default for ReloadClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Persist `cfg` to `db_path` (via [`crate::model::apply`]) then ask the
/// compositor to reload. The write is unconditional; the reload is
/// best-effort and never turns a successful write into an `Err`.
pub fn apply_and_reload(
    cfg: &Config,
    db_path: &Path,
    client: &ReloadClient,
) -> Result<ReloadOutcome, redb::Error> {
    crate::model::apply(cfg, db_path)?;
    Ok(client.reload())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `ReloadClient::new()` must never panic, bus or no bus -- the
    /// settings app has to be usable (and testable) without a running
    /// session bus.
    #[test]
    fn new_does_not_panic_without_a_bus() {
        let client = ReloadClient::new();
        // Whether or not this test environment happens to have a session
        // bus, calling reload() must not panic either.
        let _ = client.reload();
    }
}
