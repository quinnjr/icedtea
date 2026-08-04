//! The `org.icedtea.WM` D-Bus service.
//!
//! Per the binding threading-model ruling from task 7: this runs on its own
//! dedicated thread and talks to the main calloop loop only by message
//! passing -- never touching `State` directly. Two channels cross that
//! boundary:
//!
//! - `events_rx` (compositor -> this thread): every `contract::Event`
//!   produced by a `State`/`WindowManager` mutation, forwarded here as a
//!   D-Bus signal by a dedicated emitter thread spawned from
//!   [`spawn_service`]. This is the consumer the task-7 handoff comment in
//!   `main.rs` promised for the previously-undrained `_dbus_rx` channel.
//! - `cmd_tx` (this thread -> compositor): [`DbCommand`]s produced by
//!   incoming [`WmInterface`] method calls, drained by `main.rs`'s calloop
//!   loop and applied to `State` (see `State::handle_dbus_command`).
//!
//! ## Deviations from the task-12 brief
//!
//! (Standing human ruling: the plan's stated invariants/intended semantics
//! govern over its verbatim sample code; deviations are documented here.)
//!
//! - **`cmd_tx`'s type.** The brief's "Produces" interface line types
//!   `spawn_service`'s `cmd_tx` parameter as `crossbeam_channel::Sender<DbCommand>`,
//!   but its own Step-3 prose says to "wire the command channel into the
//!   compositor's calloop loop via `calloop::channel` ... follow
//!   `anvil/src/input_handler.rs`'s use of `insert_channel`". Those two
//!   statements conflict: a bare `crossbeam_channel::Receiver` is not itself
//!   a calloop event source (unlike `calloop::channel::Channel`, whose whole
//!   reason to exist -- see `main.rs`'s wallpaper-decode-channel doc comment
//!   -- is pairing an mpsc-style channel with an eventfd-backed wakeup so
//!   calloop can dispatch it without polling). Getting a `DbCommand` onto the
//!   main loop *at all* -- the actual invariant this task cares about --
//!   therefore requires `calloop::channel::Sender` here, so that's what
//!   `cmd_tx` is typed as. `DbCommand::GetState`'s reply leg stays a plain
//!   `crossbeam_channel::Sender<Snapshot>` (matching the brief exactly)
//!   since it's a one-shot round trip that never needs to wake the calloop
//!   loop itself -- the loop, having *just* handled the `GetState` message
//!   that triggers the reply, is already awake.
//! - **`WmInterface::cmd_tx` is `Mutex`-wrapped.** zbus requires interface
//!   types to be `Send + Sync` (its `#[interface]` methods take `&self` and
//!   are dispatched from a shared, possibly-concurrent registration).
//!   `calloop::channel::Sender` wraps `std::sync::mpsc::Sender`, which is
//!   `Send` but explicitly *not* `Sync`. Wrapping it in a `std::sync::Mutex`
//!   (itself `Sync` whenever its contents are `Send`, which `Sender` is)
//!   is the ordinary, fully-safe fix -- no unsafe code, and it doesn't
//!   change this task's approved unsafe-block count.
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

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use icedtea_contract::{Event, Snapshot, WindowId, WM_BUS_NAME, WM_PATH};
use smithay::reexports::calloop;
use zbus::blocking::Connection;
use zbus::interface;

/// Commands sent from the D-Bus interface thread to the compositor's main
/// (calloop) loop. Applied to `State` by `State::handle_dbus_command`.
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
/// forwards a [`DbCommand`] onto the calloop loop; none of them mutate
/// compositor state directly (see this module's doc for why).
pub struct WmInterface {
    cmd_tx: Mutex<calloop::channel::Sender<DbCommand>>,
}

impl WmInterface {
    fn send(&self, cmd: DbCommand) {
        let _ = self.cmd_tx.lock().unwrap_or_else(|e| e.into_inner()).send(cmd);
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
        // calloop loop answers this the moment it drains the `GetState`
        // command (see `State::handle_dbus_command`), so this blocks the
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
pub fn spawn_service(
    events_rx: Receiver<Event>,
    cmd_tx: calloop::channel::Sender<DbCommand>,
    quit_signal: Arc<AtomicBool>,
) -> (Connection, std::thread::JoinHandle<()>) {
    let conn = Connection::session().expect("session bus available");
    let iface = WmInterface { cmd_tx: Mutex::new(cmd_tx) };
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
            let event = match events_rx.recv_timeout(Duration::from_millis(200)) {
                Ok(event) => event,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => break,
            };
            let name = event_signal_name(&event);
            let dest: Option<&str> = None;
            let result = match &event {
                Event::WindowOpened(info) => {
                    emitter_conn.emit_signal(dest, WM_PATH, WM_BUS_NAME, name, &(info.clone(),))
                }
                Event::WindowClosed(id) => emitter_conn.emit_signal(dest, WM_PATH, WM_BUS_NAME, name, &(id.0,)),
                Event::WindowUpdated { id, update } => {
                    emitter_conn.emit_signal(dest, WM_PATH, WM_BUS_NAME, name, &(id.0, update.clone()))
                }
                Event::WorkspaceSet { id, active } => {
                    emitter_conn.emit_signal(dest, WM_PATH, WM_BUS_NAME, name, &(*id, *active))
                }
                Event::WorkspaceList(ws) => emitter_conn.emit_signal(dest, WM_PATH, WM_BUS_NAME, name, &(ws.clone(),)),
                Event::AltTabState(s) => emitter_conn.emit_signal(dest, WM_PATH, WM_BUS_NAME, name, &(s.clone(),)),
                Event::ConfigReloaded(a) => emitter_conn.emit_signal(dest, WM_PATH, WM_BUS_NAME, name, &(a.clone(),)),
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
