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
    std::thread::Builder::new()
        .name("settings-config-reloaded".to_string())
        .spawn(move || watch_config_reloaded(&signal_tx))
        .map_or_else(
            |err| tracing::warn!(%err, "no ConfigReloaded watcher thread"),
            drop,
        );

    spawn_worker(rx, tx)
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
    std::thread::Builder::new()
        .name("settings-reload".to_string())
        .spawn(move || {
            let client = ReloadClient::new();
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
fn watch_config_reloaded(tx: &InboxSender<Msg>) {
    let outcome = zbus::block_on(async {
        let conn = zbus::Connection::session().await?;
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
