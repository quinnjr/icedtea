//! The reload worker: one OS thread owning the blocking `ReloadClient` for
//! outbound `ReloadConfig` calls and a second, async connection subscribed to
//! the compositor's `ConfigReloaded` signal.
//!
//! `update` never blocks and never calls D-Bus (spec D8), so every save+reload
//! arrives here as a [`ReloadRequest`] and every answer leaves as a
//! `Msg::Applied` on the inbox.

use icedtea_ui::view::InboxSender;

use crate::app::Msg;
use crate::compositor_reload::{ReloadClient, apply_and_reload};

/// What the loop thread asks the worker for.
pub enum ReloadRequest {
    /// Write `cfg` to `db_path`, then best-effort `ReloadConfig`.
    ///
    /// `Box`ed: a `Config` is a few hundred bytes and `Shutdown` carries
    /// none, so an unboxed variant would make every queued request that size.
    Apply {
        cfg: Box<icedtea_config::Config>,
        db_path: std::path::PathBuf,
    },
    /// Stop, once everything already queued has been served. `ipc::Workers::
    /// shutdown` sends this and waits: an Apply in flight is a `redb` write
    /// the app must not exit out from under.
    Shutdown,
}

const COMPOSITOR_IFACE: &str = "org.icedtea.Compositor";
/// The signal member name is **not** PascalCase — zbus exposes `#[interface]`
/// *methods* in PascalCase (`ReloadConfig`) but leaves signal names alone
/// (contract §0, and `shell/src/clip_client.rs:37,41` proves both halves).
const CONFIG_RELOADED: &str = "ConfigReloaded";

/// How long [`watch_config_reloaded`] waits for a session-bus connection
/// before giving up on the `ConfigReloaded` subscription and running without
/// it.
///
/// `zbus::Connection::session()` is async but carries no deadline of its
/// own: a bus socket that accepts and then never authenticates would park
/// this detached watcher thread forever, and the "running without it"
/// degradation log below only fires on an `Err` -- never on a hang. Mirrors
/// `ipc::portal::PORTAL_CONNECT_TIMEOUT`, sized the same way.
const WATCH_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Start the worker. It returns when every `Inbox` is dropped (the app exited)
/// or the request channel closes.
///
/// Unlike shell's worker it does **not** `process::exit` on a D-Bus failure:
/// settings is a foreground app with no `Restart=always` unit, and a dead
/// reload worker must leave the window usable.
pub fn spawn(
    rx: crossbeam_channel::Receiver<ReloadRequest>,
    tx: InboxSender<Msg>,
) -> Option<std::thread::JoinHandle<()>> {
    let signal_tx = tx.clone();
    // The signal subscription is its own thread so a long blocking
    // `ReloadConfig` cannot delay a `ConfigReloaded` and vice versa.
    spawn_watch_on_bus(signal_tx, None);

    spawn_worker(rx, tx)
}

/// Spawn the `ConfigReloaded` watcher thread, against `address` when named or
/// `$DBUS_SESSION_BUS_ADDRESS` (via `connect_with_timeout`) when `None`.
///
/// The `address` seam mirrors [`spawn_worker_on_bus`] and exists for the same
/// reason: a hermetic test can point the watcher at a private bus it owns
/// (`settings/tests/reload_watch.rs`) instead of the developer's live session
/// bus. Production always passes `None`.
pub fn spawn_watch_on_bus(
    tx: InboxSender<Msg>,
    address: Option<String>,
) -> Option<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name("settings-config-reloaded".to_string())
        .spawn(move || watch_config_reloaded(&tx, address.as_deref()))
        .map_err(|err| tracing::warn!(%err, "no ConfigReloaded watcher thread"))
        .ok()
}

/// The request-serving half of [`spawn`], without the `ConfigReloaded`
/// watcher.
///
/// Split out so a unit test can exercise the real worker without opening a
/// signal subscription on the developer's live session bus (finding 15).
pub fn spawn_worker(
    rx: crossbeam_channel::Receiver<ReloadRequest>,
    tx: InboxSender<Msg>,
) -> Option<std::thread::JoinHandle<()>> {
    spawn_worker_on_bus(rx, tx, None)
}

/// [`spawn_worker`], against one named bus address — `portal::spawn_on_bus`'s
/// shape, for the same reason.
///
/// Splitting the watcher off was not enough on its own: the worker's *own*
/// `ReloadClient` still connected to `$DBUS_SESSION_BUS_ADDRESS` and
/// blocking-called `ReloadConfig` on whatever owned `org.icedtea.Compositor`
/// there, so `cargo test -p icedtea-settings` forced the developer's running
/// compositor to reload its configuration. The address travels per client
/// instead; a unit test names one nothing is listening on, which exercises
/// the whole write path and answers `CompositorAbsent`.
pub fn spawn_worker_on_bus(
    rx: crossbeam_channel::Receiver<ReloadRequest>,
    tx: InboxSender<Msg>,
    address: Option<String>,
) -> Option<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name("settings-reload".to_string())
        .spawn(move || {
            let client = ReloadClient::on_bus(address.as_deref());
            while let Ok(request) = rx.recv() {
                let (cfg, db_path) = match request {
                    ReloadRequest::Shutdown => return,
                    ReloadRequest::Apply { cfg, db_path } => (*cfg, db_path),
                };
                let result =
                    apply_and_reload(&cfg, &db_path, &client).map_err(|err| err.to_string());
                // The snapshot travels back with the answer: `update`
                // re-baselines `saved` from *this* config, not from whatever
                // the working copy has become while the write was in flight.
                let shipped = std::sync::Arc::new(cfg);
                if tx.send(Msg::Applied { shipped, result }).is_err() {
                    // Every Inbox is gone: the app exited.
                    return;
                }
            }
        })
        // Fix wave: `.expect` here panicked the whole app at boot under
        // thread exhaustion, while §2.4 asks this worker to degrade — the
        // same shape, and the same handling, as the portal worker's own
        // spawn. Without a worker the `Sender` the caller keeps simply never
        // answers, and the window stays usable.
        .map_err(|err| tracing::warn!(%err, "no settings reload worker thread"))
        .ok()
}

/// Subscribe to `org.icedtea.Compositor`'s `ConfigReloaded` and post one
/// `Msg::ConfigReloaded` per signal.
///
/// Every failure — no session bus, no compositor, a stream error — ends the
/// watcher quietly: the app stays usable without it. Nothing here panics on a
/// malformed signal body, because the body is never deserialised.
///
/// `address` selects the bus: `None` is the session bus (production), `Some`
/// is a named address a test owns (see [`spawn_watch_on_bus`]).
fn watch_config_reloaded(tx: &InboxSender<Msg>, address: Option<&str>) {
    let outcome = zbus::block_on(async {
        let conn = match address {
            Some(addr) => zbus::connection::Builder::address(addr)?.build().await?,
            None => connect_with_timeout()?,
        };
        let rule = zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .interface(COMPOSITOR_IFACE)?
            .path(icedtea_contract::COMPOSITOR_PATH)?
            .build();
        let mut stream = zbus::MessageStream::for_match_rule(rule, &conn, None).await?;
        use futures_util::StreamExt as _;
        while let Some(Ok(msg)) = stream.next().await {
            let is_reloaded = msg
                .header()
                .member()
                .is_some_and(|member| member.as_str() == CONFIG_RELOADED);
            if is_reloaded && tx.send(Msg::ConfigReloaded).is_err() {
                break;
            }
        }
        Ok::<(), zbus::Error>(())
    });
    if let Err(err) = outcome {
        tracing::info!(%err, "no ConfigReloaded subscription; settings runs without it");
    }
}

/// [`zbus::Connection::session`], bounded by [`WATCH_CONNECT_TIMEOUT`].
///
/// The connect runs on its own short-lived thread (via the blocking API)
/// because the async connect offers no deadline of its own; on expiry this
/// returns and the helper is abandoned to finish or fail on its own, holding
/// nothing the watcher needs. Mirrors `ipc::portal::connect` /
/// `compositor_reload::connect`.
fn connect_with_timeout() -> zbus::Result<zbus::Connection> {
    let (tx, rx) = crossbeam_channel::bounded::<zbus::Result<zbus::blocking::Connection>>(1);
    let spawned = std::thread::Builder::new()
        .name("settings-config-reloaded-connect".to_string())
        .spawn(move || {
            let _ = tx.send(zbus::blocking::Connection::session());
        });
    if let Err(err) = spawned {
        return Err(zbus::Error::Failure(format!(
            "no connect-helper thread for the ConfigReloaded watcher: {err}"
        )));
    }
    rx.recv_timeout(WATCH_CONNECT_TIMEOUT)
        .unwrap_or_else(|_| {
            Err(zbus::Error::Failure(
                "the session bus did not answer".to_string(),
            ))
        })
        .map(zbus::blocking::Connection::into_inner)
}
