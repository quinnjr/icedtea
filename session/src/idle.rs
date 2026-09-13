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
    let conn = Connection::connect_to_env().map_err(io::Error::other)?;
    let handle = std::thread::Builder::new()
        .name("icedtea-session-idle".to_string())
        .spawn(move || run(conn, timeout_ms, flow))?;
    Ok(Some(handle))
}
