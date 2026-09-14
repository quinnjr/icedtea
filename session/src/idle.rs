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

/// Why one [`run_session`] pass ended.
enum SessionEnd {
    /// The compositor connection dropped (or the initial roundtrip failed):
    /// reconnect and re-arm.
    Disconnected,
    /// The compositor does not offer the idle protocol (or a seat): retrying
    /// cannot help, so the client stops.
    Unsupported,
}

/// Run the idle client on `conn` until it drops, routing `idled` into `flow`.
/// Blocks; callers run it on a thread.
///
/// On a dropped connection (a compositor restart or crash) this reconnects to
/// the ambient Wayland display with capped exponential backoff and re-arms the
/// timer, rather than leaving idle-lock off for the daemon's whole life. A
/// compositor that never advertises the idle protocol is not retried: retrying
/// cannot make the global appear.
pub fn run(conn: Connection, timeout_ms: u32, flow: Arc<LockFlow>) {
    run_with(conn, timeout_ms, flow, connect_with_retry);
}

/// [`run`] with an injectable connect seam, so the reconnect path is testable.
fn run_with(
    conn: Connection,
    timeout_ms: u32,
    flow: Arc<LockFlow>,
    connect: impl Fn() -> io::Result<Connection>,
) {
    let mut conn = conn;
    let mut backoff = RECONNECT_MIN;
    loop {
        if matches!(
            run_session(&conn, timeout_ms, &flow),
            SessionEnd::Unsupported
        ) {
            return;
        }
        conn = loop {
            match connect() {
                Ok(fresh) => break fresh,
                Err(err) => {
                    tracing::warn!(
                        %err,
                        ?backoff,
                        "ext-idle-notify reconnect failed; retrying"
                    );
                    std::thread::sleep(backoff);
                    backoff = next_backoff(backoff);
                }
            }
        };
        backoff = RECONNECT_MIN;
        tracing::info!("ext-idle-notify reconnected; re-arming idle-lock");
    }
}

/// One connect-and-dispatch session on `conn`. Returns when the connection
/// ends or the protocol is unsupported; never panics on the Wayland callback
/// thread.
fn run_session(conn: &Connection, timeout_ms: u32, flow: &Arc<LockFlow>) -> SessionEnd {
    let mut queue = conn.new_event_queue::<IdleClient>();
    let qh = queue.handle();
    let _registry = conn.display().get_registry(&qh, ());

    let mut client = IdleClient {
        flow: Arc::clone(flow),
        timeout_ms,
        notifier: None,
        seat: None,
        notification: None,
    };

    if queue.roundtrip(&mut client).is_err() {
        tracing::warn!("ext-idle-notify: registry roundtrip failed; reconnecting");
        return SessionEnd::Disconnected;
    }
    client.ensure_notification(&qh);

    // A compositor may not implement the idle protocol (or have a seat). The
    // globals are settled by the roundtrip above, so a missing one never
    // arrives later: log why idle-lock is inactive and end the thread rather
    // than blocking on events that can never fire.
    if client.notifier.is_none() {
        tracing::warn!("compositor did not advertise ext_idle_notifier_v1; idle-lock is inactive");
        return SessionEnd::Unsupported;
    }
    if client.seat.is_none() {
        tracing::warn!("compositor advertised no wl_seat; idle-lock is inactive");
        return SessionEnd::Unsupported;
    }
    if client.notification.is_none() {
        tracing::warn!("could not request an idle notification; idle-lock is inactive");
        return SessionEnd::Unsupported;
    }
    // Only now, after the connection is live and both globals are bound and
    // the notification is requested, is the idle-lock client truly armed.
    tracing::info!(timeout_ms, "idle-lock client armed");

    loop {
        match queue.blocking_dispatch(&mut client) {
            Ok(_) => {}
            Err(err) => {
                tracing::info!(%err, "ext-idle-notify connection ended");
                return SessionEnd::Disconnected;
            }
        }
    }
}

/// How many times to try opening the ambient Wayland display before giving up.
const CONNECT_ATTEMPTS: u32 = 10;
/// Delay between Wayland connection attempts.
const CONNECT_RETRY_DELAY: Duration = Duration::from_millis(200);
/// First backoff after a dropped Wayland connection.
const RECONNECT_MIN: Duration = Duration::from_millis(200);
/// Ceiling on the reconnect backoff.
const RECONNECT_MAX: Duration = Duration::from_secs(30);

/// The next reconnect backoff: double the current one, capped at
/// [`RECONNECT_MAX`].
fn next_backoff(current: Duration) -> Duration {
    (current * 2).min(RECONNECT_MAX)
}

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
/// last error is returned if every attempt fails. `attempts == 0` is an error,
/// not a panic.
fn retry_connect<T, E>(
    mut connect: impl FnMut() -> Result<T, E>,
    attempts: u32,
    delay: Duration,
) -> Result<T, io::Error>
where
    E: std::error::Error + Send + Sync + 'static,
{
    let mut last = None;
    for attempt in 0..attempts {
        match connect() {
            Ok(value) => return Ok(value),
            Err(err) => last = Some(err),
        }
        if attempt + 1 < attempts {
            std::thread::sleep(delay);
        }
    }
    match last {
        Some(err) => Err(io::Error::other(err)),
        None => Err(io::Error::other("retry_connect called with zero attempts")),
    }
}

/// Start the idle-lock client on its own thread.
///
/// `None` disables idle-lock (spec Decision 7): no connection is opened and no
/// thread is started, so the result is `Ok(None)`. `Some(ms)` connects to the
/// ambient Wayland display (with a brief retry) and requests one timer at `ms`
/// milliseconds, clamped to the protocol's `u32`; the thread reconnects with
/// backoff if that connection later drops.
pub fn spawn(flow: Arc<LockFlow>, timeout_ms: Option<u64>) -> io::Result<Option<JoinHandle<()>>> {
    spawn_with_connect(flow, timeout_ms, connect_with_retry)
}

/// [`spawn`] with an injectable connect seam.
fn spawn_with_connect(
    flow: Arc<LockFlow>,
    timeout_ms: Option<u64>,
    connect: impl Fn() -> io::Result<Connection> + Send + 'static,
) -> io::Result<Option<JoinHandle<()>>> {
    let Some(timeout_ms) = timeout_ms else {
        tracing::info!("idle-lock disabled (lock_idle_timeout_ms unset)");
        return Ok(None);
    };
    let timeout_ms = u32::try_from(timeout_ms).unwrap_or(u32::MAX);
    let conn = connect()?;
    let handle = std::thread::Builder::new()
        .name("icedtea-session-idle".to_string())
        .spawn(move || run_with(conn, timeout_ms, flow, connect))?;
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

    /// A transient connect failure is retried and succeeds on the third try.
    #[test]
    fn retry_connect_recovers_after_transient_failures() {
        let attempts = AtomicU32::new(0);
        let result = retry_connect(
            || -> io::Result<u8> {
                let n = attempts.fetch_add(1, Ordering::SeqCst);
                if n < 2 {
                    Err(io::Error::other("no compositor yet"))
                } else {
                    Ok(7)
                }
            },
            3,
            Duration::from_millis(1),
        );
        assert_eq!(result.expect("third attempt succeeds"), 7);
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            3,
            "two failures then a success"
        );
    }

    /// `attempts == 0` returns an error rather than panicking.
    #[test]
    fn retry_connect_with_zero_attempts_is_an_error() {
        let result = retry_connect(|| -> io::Result<u8> { Ok(1) }, 0, Duration::from_millis(1));
        assert!(result.is_err(), "zero attempts must not panic or succeed");
    }

    /// The reconnect backoff doubles and is capped, so a long compositor
    /// outage does not hammer the socket or balloon the delay unbounded.
    #[test]
    fn reconnect_backoff_doubles_and_caps() {
        assert_eq!(next_backoff(RECONNECT_MIN), Duration::from_millis(400));
        assert_eq!(next_backoff(Duration::from_secs(20)), RECONNECT_MAX);
        assert_eq!(next_backoff(RECONNECT_MAX), RECONNECT_MAX);
    }
}
