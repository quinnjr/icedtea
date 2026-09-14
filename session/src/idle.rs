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
use std::time::{Duration, Instant};

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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionEnd {
    /// The compositor connection dropped (or the initial roundtrip failed):
    /// reconnect and re-arm.
    Disconnected,
    /// No idle protocol/seat/notification was advertised after a settled
    /// roundtrip. Retried a bounded number of times (a compositor restart can
    /// race global advertisement), then treated as terminal.
    Unsupported,
}

/// Run the idle client on `conn` until it drops, routing `idled` into `flow`.
/// Blocks; callers run it on a thread.
///
/// On a dropped connection (a compositor restart or crash) this reconnects to
/// the ambient Wayland display with capped exponential backoff and re-arms the
/// timer, rather than leaving idle-lock off for the daemon's whole life. A
/// compositor that does not advertise the idle protocol is retried a bounded
/// number of times (the globals may simply not have been advertised yet), then
/// stops.
pub fn run(conn: Connection, timeout_ms: u32, flow: Arc<LockFlow>) {
    run_reconnecting(
        Ok(conn),
        connect_with_retry,
        |conn| run_session(conn, timeout_ms, &flow),
        BackoffPolicy::PRODUCTION,
    );
}

/// Reconnect-with-backoff loop for the idle client, generic over the session
/// handle so the loop itself is testable without a Wayland connection.
///
/// `initial` is the already-open connection (`Ok`) or the initial connect's
/// failure (`Err`). Every `Disconnected` grows the backoff *before*
/// reconnecting, and the backoff is reset to the minimum only after a session
/// stayed up for at least [`BackoffPolicy::residency`] — so a compositor that
/// accepts and then immediately drops cannot hot-spin. An `Unsupported` session
/// is retried up to [`BackoffPolicy::unsupported_limit`] times consecutively
/// before the loop gives up, so a startup/advertisement race is survivable
/// while a compositor that truly lacks the protocol is eventually abandoned.
fn run_reconnecting<S>(
    initial: io::Result<S>,
    mut connect: impl FnMut() -> io::Result<S>,
    mut run_session: impl FnMut(&S) -> SessionEnd,
    policy: BackoffPolicy,
) {
    let mut backoff = policy.min;
    let mut unsupported_streak: u32 = 0;
    let mut current = initial;
    loop {
        let session = match current {
            Ok(session) => session,
            Err(err) => {
                tracing::warn!(%err, ?backoff, "ext-idle-notify connect failed; retrying");
                std::thread::sleep(backoff);
                backoff = policy.next(backoff);
                current = connect();
                continue;
            }
        };
        let started = Instant::now();
        match run_session(&session) {
            SessionEnd::Disconnected => {
                unsupported_streak = 0;
                if started.elapsed() >= policy.residency {
                    backoff = policy.min;
                }
                std::thread::sleep(backoff);
                backoff = policy.next(backoff);
            }
            SessionEnd::Unsupported => {
                unsupported_streak += 1;
                if unsupported_streak >= policy.unsupported_limit {
                    tracing::warn!(
                        attempts = unsupported_streak,
                        "ext-idle-notify: idle protocol still unsupported; giving up"
                    );
                    return;
                }
                std::thread::sleep(backoff);
                backoff = policy.next(backoff);
            }
        }
        tracing::info!("ext-idle-notify reconnecting; re-arming idle-lock");
        current = connect();
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
/// Minimum residency a session must reach before its end resets the reconnect
/// backoff: a compositor that accepts and immediately drops must not reset the
/// delay and hot-spin.
const RECONNECT_RESIDENCY: Duration = Duration::from_secs(5);
/// How many consecutive `Unsupported` sessions to retry before giving up. A
/// compositor restart can race global advertisement, so "not advertised yet"
/// must not be terminal; a compositor that never implements the protocol is
/// abandoned after this many attempts.
const UNSUPPORTED_RETRY_LIMIT: u32 = 3;

/// The reconnect loop's timing policy, a value so tests can drive the loop with
/// millisecond delays instead of the production seconds.
#[derive(Debug, Clone, Copy)]
struct BackoffPolicy {
    /// First backoff after a dropped session/failed connect.
    min: Duration,
    /// Ceiling on the backoff.
    max: Duration,
    /// Minimum session residency before the backoff resets to `min`.
    residency: Duration,
    /// Consecutive `Unsupported` sessions tolerated before giving up.
    unsupported_limit: u32,
}

impl BackoffPolicy {
    /// Production timing.
    const PRODUCTION: Self = Self {
        min: RECONNECT_MIN,
        max: RECONNECT_MAX,
        residency: RECONNECT_RESIDENCY,
        unsupported_limit: UNSUPPORTED_RETRY_LIMIT,
    };

    /// The next backoff: double the current one, capped at [`Self::max`].
    fn next(self, current: Duration) -> Duration {
        (current * 2).min(self.max)
    }
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
    // The idle thread is started even when the initial connect fails: the
    // compositor and this daemon are independent units with no ordering, so a
    // not-yet-existing socket must become a retry rather than idle-lock being
    // off for the daemon's whole life.
    let handle = std::thread::Builder::new()
        .name("icedtea-session-idle".to_string())
        .spawn(move || {
            let initial = connect();
            if let Err(err) = &initial {
                tracing::warn!(
                    %err,
                    "idle-lock: initial compositor connect failed; retrying with backoff"
                );
            }
            run_reconnecting(
                initial,
                connect,
                |conn| run_session(conn, timeout_ms, &flow),
                BackoffPolicy::PRODUCTION,
            );
        })?;
    Ok(Some(handle))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};

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
        let policy = BackoffPolicy::PRODUCTION;
        assert_eq!(policy.next(RECONNECT_MIN), Duration::from_millis(400));
        assert_eq!(policy.next(Duration::from_secs(20)), RECONNECT_MAX);
        assert_eq!(policy.next(RECONNECT_MAX), RECONNECT_MAX);
    }

    /// A compositor that accepts and then immediately drops must not hot-spin:
    /// each `Disconnected` is spaced by a growing backoff. Driven through the
    /// same `run_reconnecting` body `run`/`spawn` use, with millisecond delays;
    /// the last session is terminal so the loop returns.
    #[test]
    fn fast_disconnects_back_off_instead_of_hot_spinning() {
        let policy = BackoffPolicy {
            min: Duration::from_millis(5),
            max: Duration::from_millis(40),
            residency: Duration::from_secs(60),
            unsupported_limit: 1,
        };
        let stamps = Arc::new(Mutex::new(Vec::new()));
        let start = Instant::now();
        let mut calls = 0u32;
        let sink = Arc::clone(&stamps);
        run_reconnecting(
            Ok(()),
            || Ok(()),
            move |_| {
                sink.lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(start.elapsed());
                calls += 1;
                if calls < 4 {
                    SessionEnd::Disconnected
                } else {
                    // One consecutive Unsupported is terminal under limit 1.
                    SessionEnd::Unsupported
                }
            },
            policy,
        );
        let stamps = stamps.lock().unwrap_or_else(|e| e.into_inner());
        assert!(stamps.len() >= 4, "the loop ran its sessions: {stamps:?}");
        let gaps: Vec<Duration> = stamps.windows(2).map(|w| w[1] - w[0]).collect();
        assert!(gaps.len() >= 3, "enough gaps to see growth: {gaps:?}");
        assert!(gaps[1] >= gaps[0], "backoff must not shrink: {gaps:?}");
        assert!(gaps[2] >= gaps[1], "backoff must not shrink: {gaps:?}");
        assert!(gaps[2] > gaps[0], "backoff must grow: {gaps:?}");
    }

    /// An initial connect failure still enters the reconnect loop (this is the
    /// same body `spawn` runs on its thread): the first successful connect is
    /// followed by a session rather than the loop never starting.
    #[test]
    fn initial_connect_failure_still_runs_the_reconnect_loop() {
        let policy = BackoffPolicy {
            min: Duration::from_millis(1),
            max: Duration::from_millis(4),
            residency: Duration::from_secs(60),
            unsupported_limit: 1,
        };
        let connects = Arc::new(Mutex::new(0u32));
        let sessions = Arc::new(Mutex::new(0u32));
        let c = Arc::clone(&connects);
        let s = Arc::clone(&sessions);
        run_reconnecting(
            Err(io::Error::other("compositor socket does not exist yet")),
            move || {
                *c.lock().unwrap_or_else(|e| e.into_inner()) += 1;
                Ok(())
            },
            move |_| {
                *s.lock().unwrap_or_else(|e| e.into_inner()) += 1;
                SessionEnd::Unsupported
            },
            policy,
        );
        assert_eq!(
            *connects.lock().unwrap_or_else(|e| e.into_inner()),
            1,
            "the loop retried the connect after the initial failure"
        );
        assert_eq!(
            *sessions.lock().unwrap_or_else(|e| e.into_inner()),
            1,
            "a session ran after the retried connect"
        );
    }

    /// Missing globals are retried a bounded number of times, then abandoned —
    /// so a startup race is survivable while a compositor that truly lacks the
    /// protocol does not retry forever.
    #[test]
    fn unsupported_globals_are_retried_a_bounded_number_of_times() {
        let policy = BackoffPolicy {
            min: Duration::from_millis(1),
            max: Duration::from_millis(4),
            residency: Duration::from_secs(60),
            unsupported_limit: 3,
        };
        let calls = Arc::new(Mutex::new(0u32));
        let sink = Arc::clone(&calls);
        run_reconnecting(
            Ok(()),
            || Ok(()),
            move |_| {
                *sink.lock().unwrap_or_else(|e| e.into_inner()) += 1;
                SessionEnd::Unsupported
            },
            policy,
        );
        assert_eq!(
            *calls.lock().unwrap_or_else(|e| e.into_inner()),
            3,
            "missing globals are retried to the limit, then the loop gives up"
        );
    }

    /// A `Disconnected` session resets the unsupported-retry budget, so a
    /// compositor restart that races global advertisement does not permanently
    /// disable idle-lock: after the reconnect, a fresh run of unsupported
    /// sessions is tolerated again. The script would stop after four sessions
    /// (three consecutive Unsupported) without the reset.
    #[test]
    fn a_disconnect_resets_the_unsupported_retry_budget() {
        let policy = BackoffPolicy {
            min: Duration::from_millis(1),
            max: Duration::from_millis(4),
            residency: Duration::from_secs(60),
            unsupported_limit: 3,
        };
        let script = [
            SessionEnd::Unsupported,
            SessionEnd::Unsupported,
            SessionEnd::Disconnected,
            SessionEnd::Unsupported,
            SessionEnd::Unsupported,
            SessionEnd::Unsupported,
        ];
        let index = Arc::new(Mutex::new(0usize));
        let idx = Arc::clone(&index);
        run_reconnecting(
            Ok(()),
            || Ok(()),
            move |_| {
                let mut guard = idx.lock().unwrap_or_else(|e| e.into_inner());
                let end = script[(*guard).min(script.len() - 1)];
                *guard += 1;
                end
            },
            policy,
        );
        assert_eq!(
            *index.lock().unwrap_or_else(|e| e.into_inner()),
            script.len(),
            "the disconnect reset the budget and the loop retried past it"
        );
    }
}
