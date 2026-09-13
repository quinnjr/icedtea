//! `ext_idle_notifier_v1` idle→lock client.
//!
//! Binds the compositor's idle notifier directly (any number of clients may
//! each hold their own `ext_idle_notification_v1` timer) and requests exactly
//! one timer at `config::Power::lock_idle_timeout_ms`. On `idled` it routes
//! into the same [`LockFlow`] funnel lock-before-sleep uses (`on_idle`), so the
//! funnel's generation guard and its fresh `IsLocked` re-query keep an idle
//! timeout and a near-simultaneous suspend from spawning two lockers. On
//! `resumed` it does nothing: activity does not auto-unlock; only the locker
//! exiting (or an external `Session.Unlock`) clears the locked state.
//!
//! The wayland-client setup mirrors `clipboard/src/manager.rs`: connect, bind
//! through a registry, roundtrip to settle the globals, then dispatch. Unlike
//! the clipboard there is no command channel — the only input is the seat's
//! idle state — so the loop does nothing but block on Wayland events.
//!
//! No `unwrap`/`expect` runs on the Wayland callback thread: every fallible
//! step records a `tracing` line, and the connection ending just ends the loop.

use std::io;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use wayland_client::protocol::wl_registry;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols::ext::idle_notify::v1::client::{
    ext_idle_notification_v1 as idle_notification, ext_idle_notifier_v1 as idle_notifier,
};

use crate::flow::LockFlow;

/// The idle client's Wayland state: the bound notifier/seat and the one
/// notification object it requested.
struct IdleClient {
    flow: Arc<LockFlow>,
    timeout_ms: u32,
    notifier: Option<idle_notifier::ExtIdleNotifierV1>,
    seat: Option<WlSeat>,
    notification: Option<idle_notification::ExtIdleNotificationV1>,
}

impl IdleClient {
    /// Request the one notification once both globals are bound. Idempotent:
    /// a later global does not create a second timer.
    fn ensure_notification(&mut self, qh: &QueueHandle<Self>) {
        if self.notification.is_some() {
            return;
        }
        let (Some(notifier), Some(seat)) = (self.notifier.as_ref(), self.seat.as_ref()) else {
            return;
        };
        self.notification = Some(notifier.get_idle_notification(self.timeout_ms, seat, qh, ()));
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for IdleClient {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _data: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
                "ext_idle_notifier_v1" => {
                    state.notifier = Some(registry.bind(name, version.min(1), qh, ()));
                }
                "wl_seat" => {
                    state.seat = Some(registry.bind(name, version.min(7), qh, ()));
                }
                _ => {}
            }
            state.ensure_notification(qh);
        }
    }
}

impl Dispatch<idle_notification::ExtIdleNotificationV1, ()> for IdleClient {
    fn event(
        state: &mut Self,
        _notification: &idle_notification::ExtIdleNotificationV1,
        event: idle_notification::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            idle_notification::Event::Idled => state.flow.on_idle(),
            idle_notification::Event::Resumed => {}
            // `Event` is `#[non_exhaustive]`; a future event is a no-op.
            _ => {}
        }
    }
}

// The notifier and seat carry nothing this client acts on beyond binding.
wayland_client::delegate_noop!(IdleClient: ignore idle_notifier::ExtIdleNotifierV1);
wayland_client::delegate_noop!(IdleClient: ignore WlSeat);

/// Run the idle client on `conn` until it drops, routing `idled` into `flow`.
/// Blocks; callers run it on a thread.
pub fn run(conn: Connection, timeout_ms: u32, flow: Arc<LockFlow>) {
    let mut queue = conn.new_event_queue::<IdleClient>();
    let qh = queue.handle();
    let _registry = conn.display().get_registry(&qh, ());

    let mut client = IdleClient {
        flow,
        timeout_ms,
        notifier: None,
        seat: None,
        notification: None,
    };

    if queue.roundtrip(&mut client).is_err() {
        tracing::warn!("ext-idle-notify: registry roundtrip failed; idle-lock is off");
        return;
    }
    client.ensure_notification(&qh);

    // A compositor may not implement the idle protocol (or have a seat). The
    // globals are settled by the roundtrip above, so a missing one never
    // arrives later: log why idle-lock is inactive and end the thread rather
    // than blocking on events that can never fire.
    if client.notifier.is_none() {
        tracing::warn!("compositor did not advertise ext_idle_notifier_v1; idle-lock is inactive");
        return;
    }
    if client.seat.is_none() {
        tracing::warn!("compositor advertised no wl_seat; idle-lock is inactive");
        return;
    }
    if client.notification.is_none() {
        tracing::warn!("could not request an idle notification; idle-lock is inactive");
        return;
    }
    tracing::debug!(timeout_ms, "ext-idle-notify armed");

    loop {
        match queue.blocking_dispatch(&mut client) {
            Ok(_) => {}
            Err(err) => {
                tracing::info!(%err, "ext-idle-notify connection ended");
                break;
            }
        }
    }
}

/// How many times to try opening the ambient Wayland display before giving up.
const CONNECT_ATTEMPTS: u32 = 10;
/// Delay between Wayland connection attempts.
const CONNECT_RETRY_DELAY: Duration = Duration::from_millis(200);

/// Connect to the ambient Wayland display, retrying briefly.
///
/// `icedtea-session` and the compositor are independent session units with no
/// ordering between them, so the compositor's socket may not exist yet when
/// the daemon starts. Without a retry that transient becomes permanent:
/// idle-lock would stay off for the daemon's whole life.
fn connect_with_retry() -> io::Result<Connection> {
    retry_connect(
        Connection::connect_to_env,
        CONNECT_ATTEMPTS,
        CONNECT_RETRY_DELAY,
    )
}

/// Run `connect` up to `attempts` times, sleeping `delay` between tries; the
/// last error is returned if every attempt fails.
fn retry_connect<E>(
    mut connect: impl FnMut() -> Result<Connection, E>,
    attempts: u32,
    delay: Duration,
) -> io::Result<Connection>
where
    E: std::error::Error + Send + Sync + 'static,
{
    let mut last = None;
    for attempt in 0..attempts {
        match connect() {
            Ok(conn) => return Ok(conn),
            Err(err) => last = Some(err),
        }
        if attempt + 1 < attempts {
            std::thread::sleep(delay);
        }
    }
    Err(io::Error::other(
        last.expect("at least one connection attempt"),
    ))
}

/// Start the idle-lock client on its own thread.
///
/// `None` disables idle-lock (spec Decision 7): no connection is opened and no
/// thread is started, so the result is `Ok(None)`. `Some(ms)` connects to the
/// ambient Wayland display and requests one timer at `ms` milliseconds, clamped
/// to the protocol's `u32`.
pub fn spawn(flow: Arc<LockFlow>, timeout_ms: Option<u64>) -> io::Result<Option<JoinHandle<()>>> {
    let Some(timeout_ms) = timeout_ms else {
        tracing::info!("idle-lock disabled (lock_idle_timeout_ms unset)");
        return Ok(None);
    };
    let timeout_ms = u32::try_from(timeout_ms).unwrap_or(u32::MAX);
    let conn = connect_with_retry()?;
    let handle = std::thread::Builder::new()
        .name("icedtea-session-idle".to_string())
        .spawn(move || run(conn, timeout_ms, flow))?;
    Ok(Some(handle))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    /// A connection that never succeeds is retried exactly `attempts` times,
    /// then reported as an error — the retry is bounded.
    #[test]
    fn retry_connect_is_bounded() {
        let attempts = AtomicU32::new(0);
        let result = retry_connect(
            || -> io::Result<Connection> {
                attempts.fetch_add(1, Ordering::SeqCst);
                Err(io::Error::other("no compositor"))
            },
            3,
            Duration::from_millis(1),
        );
        assert!(result.is_err(), "a never-ready display gives up");
        assert_eq!(attempts.load(Ordering::SeqCst), 3, "bounded to 3 attempts");
    }
}
