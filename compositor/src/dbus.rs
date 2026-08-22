//! The `org.icedtea.WM` D-Bus service.
//!
//! Per the binding threading-model ruling from task 7: this runs on its own
//! dedicated thread and talks to the compositor's own loop only by message
//! passing -- never touching `State` directly. Two channels cross that
//! boundary:
//!
//! - `events_rx` (compositor -> this thread): every `contract::Event`
//!   produced by a `State`/`WindowManager` mutation, forwarded here as a
//!   D-Bus signal by a dedicated emitter thread spawned from
//!   [`spawn_service`]. This is the consumer the task-7 handoff comment in
//!   `main.rs` promised for the previously-undrained `_dbus_rx` channel.
//! - `cmd_tx` (this thread -> compositor): [`DbCommand`]s produced by
//!   incoming [`WmInterface`] method calls, applied to `State` by whatever
//!   drains this channel (see `State::handle_command`).
//!
//! The command channel is a `crossbeam_channel::Sender`, drained by the
//! compositor's own event loop once per turn. It used to be paired with a
//! dedicated event-source abstraction that woke the loop on a send; with
//! that library gone the loop polls instead, which is the same latency in
//! practice because it wakes on every input and frame event anyway.
//!
//! ## Deviations from the task-12 brief
//!
//! (Standing human ruling: the plan's stated invariants/intended semantics
//! govern over its verbatim sample code; deviations are documented here.)
//!
//! - **Bus name ownership.** The brief's Step-3 sample connects to the
//!   session bus and registers the interface object but never calls
//!   `request_name`, so nothing would actually own `org.icedtea.WM` --
//!   the brief's own Step-5 manual verification
//!   (`gdbus --dest org.icedtea.WM ...`) would fail with "name has no
//!   owner" as written. Added below, after the interface is registered
//!   (so the object already exists by the time anyone can observe the name
//!   becoming owned).
//! - **`WmInterface::conn` field dropped.** The brief's sample struct
//!   carries a `conn: Connection` field that no `#[interface]` method ever
//!   reads (every method just forwards onto `cmd_tx`); an unread field is a
//!   `dead_code` warning under this workspace's `-D warnings` gate. Nothing
//!   in this task's interface needs a connection handle on `self` --
//!   `emit_signal` runs from the separate emitter thread against its own
//!   connection clone.
//! - **Signal payload typing.** The brief's sample builds one `payload`
//!   local via a `match` whose arms produce differently-shaped tuples
//!   (`(WindowInfo,)`, `(u32,)`, `(u32, WindowUpdate)`, ...) bound to a
//!   single variable -- that doesn't type-check (a `match` expression must
//!   produce one concrete type). `emit_signal` is called directly inside
//!   each arm below instead, each against its own concretely-typed body.
//! - **`quit_signal`'s use, and the return type that carries it.** Present
//!   in the brief's "Produces" signature but absent from its Step-3 sample
//!   body. Used here to turn the emitter thread's blocking `recv()` into a
//!   polling `recv_timeout`, so the thread can notice the compositor
//!   shutting down and exit its loop between messages instead of being
//!   severed mid-`emit_signal` when the process exits. Setting the flag
//!   alone doesn't achieve that, though: `main.rs` sets it as its last
//!   statement, and if nothing then waits for the thread to actually act on
//!   it, `main` (and the process with it) can return before the next
//!   `recv_timeout` tick ever wakes up -- the flag would be set on a thread
//!   that's already gone. So `spawn_service` returns
//!   `(Connection, JoinHandle<()>)`, not just the brief's bare
//!   `Connection`, and `main.rs` joins that handle immediately after
//!   setting the flag; the join is bounded by the `recv_timeout` tick
//!   (200ms) it's waiting on, not by traffic on `events_rx`.

use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use icedtea_contract::{Event, SeqEvent, Snapshot, WindowId, WM_BUS_NAME, WM_PATH};
use zbus::blocking::Connection;
use zbus::interface;

/// Commands sent from the D-Bus interface thread to the compositor's main
/// loop. Applied to `State` by `State::handle_command`.
#[derive(Debug, Clone)]
pub enum DbCommand {
    Focus(WindowId),
    Close(WindowId),
    Minimize(WindowId, bool),
    Maximize(WindowId, bool),
    Fullscreen(WindowId, bool),
    SetWorkspace(u32),
    MoveToWorkspace(WindowId, u32),
    ReloadConfig,
    Quit,
    GetState(Sender<Snapshot>),
    /// Test-only: synthesize a touch-down at `(x, y)` for touch point `id`
    /// via `wlr::Runtime::inject_touch_down`, replying with the grab serial
    /// it mints (`None` if there is no seat or no surface under the point).
    /// Not reachable from `WmInterface` -- only the test harness sends
    /// this, directly onto `cmd_tx`, since injecting synthetic touch input
    /// makes no sense as a D-Bus-exposed production operation.
    InjectTouchDown { x: f64, y: f64, id: i32, time_msec: u32, reply: Sender<Option<u32>> },
    /// Test-only: synthesize a touch-motion to `(x, y)` for touch point
    /// `id` via `wlr::Runtime::inject_touch_motion`. `reply` is purely a
    /// synchronization ack -- see `InjectTouchDown`'s doc -- so the harness
    /// call blocks until the injection has actually run on the compositor
    /// thread instead of racing ahead of it.
    InjectTouchMotion { x: f64, y: f64, id: i32, time_msec: u32, reply: Sender<()> },
    /// Test-only: synthesize a touch-up for touch point `id` via
    /// `wlr::Runtime::inject_touch_up`. See `InjectTouchDown`'s doc.
    InjectTouchUp { id: i32, time_msec: u32, reply: Sender<()> },
    /// Test-only: read the drag icon's current scene layout position via
    /// `wlr::Runtime::drag_icon_position`, replying with `None` if no drag
    /// with a visible icon is in progress. Not reachable from
    /// `WmInterface` -- only the test harness sends this, same reasoning
    /// as `InjectTouchDown`.
    DragIconPosition { reply: Sender<Option<(i32, i32)>> },
    /// Test-only: read `wlr::Runtime::is_session_locked` via `wayland`'s
    /// runtime handle. Not reachable from `WmInterface` -- only the test
    /// harness sends this, same reasoning as `DragIconPosition`.
    SessionLocked { reply: Sender<bool> },
    /// Test-only: read `wlr::Runtime::cursor_position` via `wayland`'s
    /// runtime handle. Not reachable from `WmInterface` -- only the test
    /// harness sends this, same reasoning as `SessionLocked`.
    CursorPosition { reply: Sender<(f64, f64)> },
    /// Test-only: read the `DISPLAY` name (`:N`) Xwayland advertises, via
    /// `wlr::Runtime::xwayland_display_name`. `None` when no Xwayland was
    /// created (the `Xwayland` binary is absent), so the X11 end-to-end test
    /// can skip cleanly. Available as soon as the manager reserves its display
    /// socket -- before the lazy `Xwayland` start -- which is exactly what lets
    /// the test read `DISPLAY`, connect an X11 client, and *trigger* that lazy
    /// start. Not reachable from `WmInterface` -- only the test harness sends
    /// this, same reasoning as `SessionLocked`.
    XwaylandDisplay { reply: Sender<Option<String>> },
    /// Test-only: probe every mapped override-redirect (OR) X11 pop-up the
    /// compositor is tracking in its M3 side-table, reading each one's *real*
    /// scene state — node position, whether it is parented in the band above
    /// managed toplevels, and whether it holds the seat keyboard — via the
    /// `wlr::Runtime` xwayland scene accessors. Lets the OR end-to-end test
    /// assert placement/stacking/focus without the OR surface ever entering the
    /// `Window` model (which is the whole point of the OR path). Not reachable
    /// from `WmInterface` -- only the test harness sends this, same reasoning as
    /// `SessionLocked`.
    XwaylandOverrideRedirect { reply: Sender<Vec<OverrideRedirectProbe>> },
    /// Test-only: override the primary output's recorded scale and, if Xwayland
    /// is already up, re-publish the X11 HiDPI hint (`Xft.dpi` in the root
    /// `RESOURCE_MANAGER`) for it. Lets the M4 HiDPI end-to-end test exercise a
    /// scaled output without persisting a `DisplayConfig` or driving the
    /// output-management protocol — it sets scale 2 and asserts the compositor
    /// published `Xft.dpi: 192`. `reply` is a synchronization ack (the harness
    /// call blocks until the scale has actually been recorded on the compositor
    /// thread). Not reachable from `WmInterface` -- only the test harness sends
    /// this, same reasoning as `SessionLocked`.
    SetOutputScaleForTest { scale: f64, reply: Sender<()> },
}

/// One mapped override-redirect X11 pop-up, as the test-only
/// [`DbCommand::XwaylandOverrideRedirect`] probe reports it — read straight off
/// the live scene, not the compositor's own bookkeeping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverrideRedirectProbe {
    /// The pop-up scene node's position, in layout coordinates — the proof it
    /// landed at its client-requested absolute coordinates.
    pub position: (i32, i32),
    /// Whether the node is parented in `Band::Top`, i.e. the band **above**
    /// every managed toplevel (`Band::Toplevel`) — the proof it stacks over
    /// managed windows.
    pub above_toplevel: bool,
    /// Whether the seat keyboard is currently pointed at this pop-up — the
    /// proof a focus-taking menu is navigable.
    pub keyboard_focused: bool,
}

/// Map a `contract::Event` to its D-Bus signal name, so the emitter thread
/// can dispatch. Pure and unit-tested.
pub fn event_signal_name(event: &Event) -> &'static str {
    match event {
        Event::WindowOpened(_) => "WindowOpened",
        Event::WindowClosed(_) => "WindowClosed",
        Event::WindowUpdated { .. } => "WindowUpdated",
        Event::WorkspaceSet { .. } => "WorkspaceSet",
        Event::WorkspaceList(_) => "WorkspaceList",
        Event::AltTabState(_) => "AltTabState",
        Event::ConfigReloaded(_) => "ConfigReloaded",
    }
}

/// The registered `org.icedtea.WM` interface object. Every method just
/// forwards a [`DbCommand`] onto the compositor's main loop; none of them
/// mutate compositor state directly (see this module's doc for why).
pub struct WmInterface {
    cmd_tx: crossbeam_channel::Sender<DbCommand>,
    /// Write half of the loop's D-Bus command wake pipe
    /// (`backend::wake_source`). `send` nudges it after every command so a
    /// compositor blocked in `dispatch(-1)` (idle: no damage, no input)
    /// wakes to drain `cmd_tx`'s receiver instead of waiting for whatever
    /// unrelated event happens along next.
    wake: UnixStream,
}

impl WmInterface {
    fn send(&self, cmd: DbCommand) {
        let _ = self.cmd_tx.send(cmd);
        crate::backend::wake(&self.wake);
    }
}

#[interface(name = "org.icedtea.WM")]
impl WmInterface {
    fn focus_window(&self, id: u32) {
        self.send(DbCommand::Focus(WindowId(id)));
    }
    fn close_window(&self, id: u32) {
        self.send(DbCommand::Close(WindowId(id)));
    }
    fn minimize_window(&self, id: u32, toggle: bool) {
        self.send(DbCommand::Minimize(WindowId(id), toggle));
    }
    fn maximize_window(&self, id: u32, toggle: bool) {
        self.send(DbCommand::Maximize(WindowId(id), toggle));
    }
    fn fullscreen_window(&self, id: u32, toggle: bool) {
        self.send(DbCommand::Fullscreen(WindowId(id), toggle));
    }
    fn set_workspace(&self, id: u32) {
        self.send(DbCommand::SetWorkspace(id));
    }
    fn move_window_to_workspace(&self, id: u32, workspace: u32) {
        self.send(DbCommand::MoveToWorkspace(WindowId(id), workspace));
    }
    fn get_state(&self) -> Snapshot {
        // Synchronous round-trip: ask the compositor for a snapshot. The
        // main loop answers this the moment it drains the `GetState`
        // command (see `State::handle_command`), so this blocks the
        // zbus dispatch for this connection only as long as one loop
        // iteration takes -- by design, per the brief.
        let (reply_tx, reply_rx) = crossbeam_channel::bounded(1);
        self.send(DbCommand::GetState(reply_tx));
        reply_rx.recv().unwrap_or_else(|_| Snapshot { seq: 0, windows: vec![], workspaces: vec![], active_workspace: 0 })
    }
    fn reload_config(&self) {
        self.send(DbCommand::ReloadConfig);
    }
    fn quit(&self) {
        self.send(DbCommand::Quit);
    }
}

/// Spawn the D-Bus service: connects to the session bus, registers
/// [`WmInterface`] at [`WM_PATH`], claims [`WM_BUS_NAME`], and starts a
/// dedicated emitter thread that turns every `contract::Event` received on
/// `events_rx` into a D-Bus signal. Returns the shared connection (kept
/// alive by the caller for as long as the service should stay registered)
/// and the emitter thread's `JoinHandle`, so the caller can wait for it to
/// actually observe `quit_signal` on shutdown (see this module's doc for
/// why a bare `Connection` return, as the brief's "Produces" line has it,
/// isn't enough for that).
///
/// `cmd_wake` is the write half of a `backend::wake_source` registered by
/// the caller against the same `Runtime` the loop runs on -- see
/// `WmInterface::send`'s doc for why a command needs one at all.
pub fn spawn_service(
    events_rx: Receiver<SeqEvent>,
    cmd_tx: crossbeam_channel::Sender<DbCommand>,
    quit_signal: Arc<AtomicBool>,
    cmd_wake: UnixStream,
) -> (Connection, std::thread::JoinHandle<()>) {
    let conn = Connection::session().expect("session bus available");
    let iface = WmInterface { cmd_tx, wake: cmd_wake };
    conn.object_server().at(WM_PATH, iface).expect("register org.icedtea.WM interface");
    conn.request_name(WM_BUS_NAME).unwrap_or_else(|err| {
        panic!(
            "failed to acquire the {WM_BUS_NAME} bus name -- is another icedtea-compositor \
             instance (or a stale connection holding the name) already running? ({err})"
        )
    });

    let emitter_conn = conn.clone();
    let handle = std::thread::spawn(move || {
        loop {
            if quit_signal.load(Ordering::Relaxed) {
                break;
            }
            let SeqEvent { seq, event } = match events_rx.recv_timeout(Duration::from_millis(200)) {
                Ok(event) => event,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => break,
            };
            let name = event_signal_name(&event);
            let dest: Option<&str> = None;
            // Review finding I2: `seq` is the first argument of every
            // signal, so a subscriber can drop signals already folded into
            // the `GetState()` snapshot it started from (`seq <=
            // snapshot.seq`) and detect a gap (`seq > last_seen + 1`) that
            // means it must re-sync. Recorded signatures:
            //   WindowOpened   t(ussuu(iiii)bbbb)
            //   WindowClosed   tu
            //   WindowUpdated  tu(asa(iiii)auabababab)
            //   WorkspaceSet   tub
            //   WorkspaceList  ta(us)
            //   AltTabState    t(baut)
            //   ConfigReloaded t(siii(sss)as)
            let result = match &event {
                Event::WindowOpened(info) => {
                    emitter_conn.emit_signal(dest, WM_PATH, WM_BUS_NAME, name, &(seq, info.clone()))
                }
                Event::WindowClosed(id) => emitter_conn.emit_signal(dest, WM_PATH, WM_BUS_NAME, name, &(seq, id.0)),
                Event::WindowUpdated { id, update } => {
                    emitter_conn.emit_signal(dest, WM_PATH, WM_BUS_NAME, name, &(seq, id.0, update.clone()))
                }
                Event::WorkspaceSet { id, active } => {
                    emitter_conn.emit_signal(dest, WM_PATH, WM_BUS_NAME, name, &(seq, *id, *active))
                }
                Event::WorkspaceList(ws) => {
                    emitter_conn.emit_signal(dest, WM_PATH, WM_BUS_NAME, name, &(seq, ws.clone()))
                }
                Event::AltTabState(s) => emitter_conn.emit_signal(dest, WM_PATH, WM_BUS_NAME, name, &(seq, s.clone())),
                Event::ConfigReloaded(a) => {
                    emitter_conn.emit_signal(dest, WM_PATH, WM_BUS_NAME, name, &(seq, a.clone()))
                }
            };
            if let Err(err) = result {
                tracing::warn!(signal = name, error = %err, "failed to emit D-Bus signal");
            }
        }
    });

    (conn, handle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use icedtea_contract::{AltTabState, Appearance, Rectangle, WindowInfo, WindowUpdate, WorkspaceInfo};

    #[test]
    fn event_names_match_interface() {
        assert_eq!(event_signal_name(&Event::WindowOpened(sample_info())), "WindowOpened");
        assert_eq!(event_signal_name(&Event::WindowClosed(WindowId(1))), "WindowClosed");
        assert_eq!(event_signal_name(&Event::ConfigReloaded(default_appearance())), "ConfigReloaded");
        // Fix-round addition: the brief's own Step-1 sample only exercised
        // 3 of the 7 `Event` variants; cover the remaining 4 so every
        // `event_signal_name` match arm has a passing assertion behind it.
        assert_eq!(
            event_signal_name(&Event::WindowUpdated { id: WindowId(1), update: WindowUpdate::default() }),
            "WindowUpdated"
        );
        assert_eq!(event_signal_name(&Event::WorkspaceSet { id: 0, active: true }), "WorkspaceSet");
        assert_eq!(
            event_signal_name(&Event::WorkspaceList(vec![WorkspaceInfo { id: 0, name: "1".into() }])),
            "WorkspaceList"
        );
        assert_eq!(
            event_signal_name(&Event::AltTabState(AltTabState { active: true, entries: vec![], index: 0 })),
            "AltTabState"
        );
    }

    fn sample_info() -> WindowInfo {
        WindowInfo {
            id: WindowId(1),
            app_id: "a".into(),
            title: "t".into(),
            pid: 1,
            workspace: 0,
            geometry: Rectangle { x: 0, y: 0, width: 10, height: 10 },
            maximized: false,
            minimized: false,
            fullscreen: false,
            focused: true,
        }
    }
    fn default_appearance() -> Appearance {
        Appearance {
            bar_position: "bottom".into(),
            bar_height: 42,
            corner_radius: 8,
            snap_gap: 8,
            palette: icedtea_contract::Palette {
                background: "#000000".into(),
                foreground: "#ffffff".into(),
                accent: "#0000ff".into(),
            },
            wallpaper: None,
        }
    }
}
