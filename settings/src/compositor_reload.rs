//! Best-effort `org.icedtea.Compositor` reload client. After a config edit is
//! written to the redb store, the settings app asks the running compositor
//! to re-read it via `ReloadConfig`. If no compositor is running (bus or
//! service absent), that's fine -- the write already landed on disk and
//! will apply on the compositor's next start.

use std::path::Path;
use std::time::Duration;

use icedtea_config::Config;
use icedtea_contract::{COMPOSITOR_BUS_NAME, COMPOSITOR_PATH};

// zbus's #[interface] exposes Rust methods in PascalCase, so the wire
// member is ReloadConfig (matching FocusWindow/CloseWindow/SetWorkspace in
// shell/src/compositor_client.rs).
const COMPOSITOR_IFACE: &str = "org.icedtea.Compositor";

/// How long [`ReloadClient::on_bus`] waits for a session bus to hand it a
/// connection.
///
/// `Connection::session()`/`Builder::build()` are blocking with no deadline
/// of their own: a bus socket that accepts and then never authenticates
/// would park the sole reload-worker thread forever, silently losing a
/// queued `Apply` on quit. The connect runs on a helper thread (mirrors
/// `ipc::portal::connect`) and this is how long its answer is waited for.
const RELOAD_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// The deadline put on the `ReloadConfig` call itself, via
/// `method_timeout`, so a wedged compositor cannot block the worker thread
/// indefinitely either. Mirrors `ipc::portal::PORTAL_CALL_TIMEOUT`.
const RELOAD_CALL_TIMEOUT: Duration = Duration::from_secs(20);

/// The outcome of a [`ReloadClient::reload`] call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReloadOutcome {
    /// The compositor accepted `ReloadConfig`.
    Reloaded,
    /// No compositor is reachable on the session bus (or it rejected the
    /// call, or the connect/call deadline elapsed). The config write already
    /// happened; this is not an error.
    CompositorAbsent,
}

/// Issues `org.icedtea.Compositor`'s `ReloadConfig` from the GTK thread.
/// `call_method` awaits a reply, so both the connect and the call are given
/// a deadline (`RELOAD_CONNECT_TIMEOUT`/`RELOAD_CALL_TIMEOUT`) rather than
/// left to block forever (mirrors `shell::compositor_client::CompositorProxy`).
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
        ReloadClient {
            conn: connect(address).ok(),
        }
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

/// A bus connection with a deadline on the connect *and* on every method call
/// made through it, via `method_timeout`.
///
/// The connect runs on its own short-lived thread because plain
/// `Builder::build`/`Connection::session()` offer no deadline for it; on
/// expiry this returns and the helper is abandoned to finish or fail on its
/// own, holding nothing the caller needs. Mirrors `ipc::portal::connect`.
fn connect(address: Option<&str>) -> Result<zbus::blocking::Connection, ()> {
    let address = address.map(str::to_owned);
    let (tx, rx) = crossbeam_channel::bounded::<Option<zbus::blocking::Connection>>(1);
    let spawned = std::thread::Builder::new()
        .name("settings-reload-connect".to_string())
        .spawn(move || {
            let built = match address {
                Some(address) => zbus::blocking::connection::Builder::address(address.as_str()),
                None => zbus::blocking::connection::Builder::session(),
            }
            .and_then(|builder| builder.method_timeout(RELOAD_CALL_TIMEOUT).build())
            .ok();
            let _ = tx.send(built);
        });
    if spawned.is_err() {
        return Err(());
    }
    rx.recv_timeout(RELOAD_CONNECT_TIMEOUT)
        .ok()
        .flatten()
        .ok_or(())
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
