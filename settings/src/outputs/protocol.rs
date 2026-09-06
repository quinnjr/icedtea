//! Toolkit-free `zwlr_output_management_v1` client core.
//!
//! Owns a second `wayland-client` [`Connection`] (separate from the
//! toolkit's own window connection) and the [`EventQueue`] that drives it,
//! plus the `Dispatch` glue for every object in the output-management tree.
//! Everything here is plain data and pure protocol logic, so the whole
//! enumerate/apply cycle is exercisable from a libtest binary against
//! `icedtea-harness` with nothing but explicit roundtrips. The toolkit's poll
//! loop integration lives in [`super::pump`], layered on top of this.

use std::os::unix::net::UnixStream;
use std::path::Path;

use async_channel::Sender;
use wayland_client::backend::ObjectId;
use wayland_client::protocol::wl_output;
use wayland_client::protocol::wl_registry;
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, event_created_child};
use wayland_protocols_wlr::output_management::v1::client::{
    zwlr_output_configuration_head_v1::ZwlrOutputConfigurationHeadV1,
    zwlr_output_configuration_v1::{self, ZwlrOutputConfigurationV1},
    zwlr_output_head_v1::{self, ZwlrOutputHeadV1},
    zwlr_output_manager_v1::{self, ZwlrOutputManagerV1},
    zwlr_output_mode_v1::{self, ZwlrOutputModeV1},
};

/// The manager version this client understands. The bind is capped to this and
/// to whatever the compositor advertises, whichever is lower.
const MANAGER_VERSION: u32 = 4;

/// One display mode as reported by a head. `refresh_mhz` is in millihertz, the
/// protocol's native unit (60.000 Hz => 60000).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mode {
    pub width: i32,
    pub height: i32,
    pub refresh_mhz: i32,
    pub preferred: bool,
}

/// A snapshot of one output head — plain data, no live proxies. This is what
/// the view layer renders; it never touches the wayland objects directly.
#[derive(Clone, Debug, PartialEq)]
pub struct Head {
    pub name: String,
    pub description: String,
    pub enabled: bool,
    pub modes: Vec<Mode>,
    /// The mode currently driving the output, if any.
    pub current_mode: Option<Mode>,
    pub x: i32,
    pub y: i32,
    pub scale: f64,
    /// The raw `wl_output.transform` value (0 = normal).
    pub transform: i32,
}

/// A requested mode for an edit. If a head already advertises a mode matching
/// all three fields it is selected with `set_mode`; otherwise the edit falls
/// back to `set_custom_mode` (headless/nested backends have no fixed modes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModeRequest {
    pub width: i32,
    pub height: i32,
    pub refresh_mhz: i32,
}

/// One head's worth of changes to send in a configuration. Fields left `None`
/// keep the head's current value; `enabled == false` disables the head and
/// ignores every other field.
#[derive(Clone, Debug, PartialEq)]
pub struct HeadEdit {
    /// Connector name, e.g. `"DP-1"` — the match key against the live heads.
    pub name: String,
    pub enabled: bool,
    pub mode: Option<ModeRequest>,
    pub position: Option<(i32, i32)>,
    pub scale: Option<f64>,
    pub transform: Option<i32>,
}

/// Per-configuration `Dispatch` user-data: which kind of submission this
/// `ZwlrOutputConfigurationV1` was. Attached at `create_configuration` time so
/// the terminal event (`succeeded`/`failed`) can be matched back to the exact
/// request that produced it, rather than a process-wide "last was test" flag a
/// concurrent request would clobber.
#[derive(Clone, Copy, Debug)]
pub struct ConfigData {
    pub is_test: bool,
}

/// Messages the client pushes to the view over its async-channel sender.
#[derive(Clone, Debug, PartialEq)]
pub enum OutputsMsg {
    /// The head set changed (initial enumeration, a hotplug, or a `done`
    /// after an applied configuration). Carries the fresh snapshot.
    HeadsChanged(Vec<Head>),
    /// The compositor is not advertising `zwlr_output_manager_v1`.
    ManagerUnavailable,
    /// A submitted configuration completed successfully. `is_test` is `true`
    /// when it came from [`OutputsConnection::test_configuration`] (a preview
    /// that changed nothing) and `false` for a real apply. It is carried per
    /// reply — tagged onto the configuration object itself (see [`ConfigData`])
    /// — rather than read from a shared flag, so an overlapped Test/Apply pair
    /// can never have one reply interpreted with the other's meaning.
    ApplySucceeded { is_test: bool },
    /// A submitted configuration was rejected. `is_test` as in
    /// [`OutputsMsg::ApplySucceeded`].
    ApplyFailed { is_test: bool },
    /// A submitted configuration was cancelled (superseded by a change the
    /// compositor made meanwhile); the client should re-read and retry.
    ApplyCancelled,
    /// The second wayland connection died (compositor exited, socket error).
    /// The glib source detaches after emitting this; the page should treat
    /// output management as gone.
    Disconnected,
}

/// Why building/sending a configuration could not even be attempted.
#[derive(Debug)]
pub enum OutputsError {
    /// No manager global — nothing to configure.
    ManagerUnavailable,
    /// An edit named a head that is not currently present.
    UnknownHead(String),
    /// The underlying connection failed to flush the request.
    Protocol(wayland_client::backend::WaylandError),
}

impl std::fmt::Display for OutputsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OutputsError::ManagerUnavailable => write!(f, "output management is unavailable"),
            OutputsError::UnknownHead(name) => write!(f, "no such head: {name}"),
            OutputsError::Protocol(err) => write!(f, "protocol error: {err}"),
        }
    }
}

impl std::error::Error for OutputsError {}

/// A live mode object plus its accumulated properties.
struct ModeEntry {
    proxy: ZwlrOutputModeV1,
    id: ObjectId,
    width: i32,
    height: i32,
    refresh_mhz: i32,
    preferred: bool,
}

/// A live head object plus its accumulated properties and modes.
struct HeadEntry {
    proxy: ZwlrOutputHeadV1,
    name: String,
    description: String,
    enabled: bool,
    x: i32,
    y: i32,
    scale: f64,
    transform: i32,
    modes: Vec<ModeEntry>,
    current_mode: Option<ObjectId>,
}

impl HeadEntry {
    fn new(proxy: ZwlrOutputHeadV1) -> Self {
        HeadEntry {
            proxy,
            name: String::new(),
            description: String::new(),
            enabled: false,
            x: 0,
            y: 0,
            scale: 1.0,
            transform: 0,
            modes: Vec::new(),
            current_mode: None,
        }
    }

    /// A sensible mode to commit when a head is being enabled but the edit
    /// names no explicit mode: the head's current mode if it has one, else its
    /// preferred advertised mode, else the first mode it advertises. Returns
    /// `None` only for a head that advertises no modes at all (a headless/
    /// nested output), where the caller commits no `set_mode` and lets the
    /// compositor pick. Enabling a head with a mode chosen this way is what
    /// keeps a re-enabled, previously-disabled output from committing with no
    /// mode set and failing on real DRM.
    fn default_mode(&self) -> Option<ModeRequest> {
        let pick = |m: &ModeEntry| ModeRequest {
            width: m.width,
            height: m.height,
            refresh_mhz: m.refresh_mhz,
        };
        if let Some(id) = &self.current_mode
            && let Some(m) = self.modes.iter().find(|m| &m.id == id)
        {
            return Some(pick(m));
        }
        if let Some(m) = self.modes.iter().find(|m| m.preferred) {
            return Some(pick(m));
        }
        self.modes.first().map(pick)
    }

    fn snapshot(&self) -> Head {
        let modes: Vec<Mode> = self
            .modes
            .iter()
            .map(|m| Mode {
                width: m.width,
                height: m.height,
                refresh_mhz: m.refresh_mhz,
                preferred: m.preferred,
            })
            .collect();
        let current_mode = self.current_mode.as_ref().and_then(|id| {
            self.modes.iter().find(|m| &m.id == id).map(|m| Mode {
                width: m.width,
                height: m.height,
                refresh_mhz: m.refresh_mhz,
                preferred: m.preferred,
            })
        });
        Head {
            name: self.name.clone(),
            description: self.description.clone(),
            enabled: self.enabled,
            modes,
            current_mode,
            x: self.x,
            y: self.y,
            scale: self.scale,
            transform: self.transform,
        }
    }
}

/// Dispatch state: the manager, the heads, and the channel back to the view.
pub struct OutputsState {
    manager: Option<ZwlrOutputManagerV1>,
    /// Latest serial from the manager's `done` — required to create a
    /// configuration.
    serial: u32,
    /// Whether a `done` has ever arrived (enumeration is complete).
    saw_done: bool,
    /// Heads in arrival order (stable for the view).
    heads: Vec<HeadEntry>,
    tx: Sender<OutputsMsg>,
}

impl OutputsState {
    fn new(tx: Sender<OutputsMsg>) -> Self {
        OutputsState {
            manager: None,
            serial: 0,
            saw_done: false,
            heads: Vec::new(),
            tx,
        }
    }

    fn emit(&self, msg: OutputsMsg) {
        // Unbounded channel: `try_send` only fails if the receiver is gone,
        // which just means the page was torn down — nothing to do.
        let _ = self.tx.try_send(msg);
    }

    fn head_by_mode(&mut self, mode_id: &ObjectId) -> Option<&mut ModeEntry> {
        self.heads
            .iter_mut()
            .flat_map(|h| h.modes.iter_mut())
            .find(|m| &m.id == mode_id)
    }
}

/// The owned connection + queue + state. Drives protocol logic with explicit
/// roundtrips (tests) or from the toolkit's poll loop ([`super::pump`]).
pub struct OutputsConnection {
    conn: Connection,
    queue: EventQueue<OutputsState>,
    qh: QueueHandle<OutputsState>,
    state: OutputsState,
}

impl OutputsConnection {
    /// Connect using `WAYLAND_DISPLAY`/`WAYLAND_SOCKET` from the environment.
    pub fn connect_to_env(tx: Sender<OutputsMsg>) -> Result<Self, wayland_client::ConnectError> {
        let conn = Connection::connect_to_env()?;
        Ok(Self::from_connection(conn, tx))
    }

    /// Connect to an explicit compositor socket path (used by tests, which each
    /// talk to a private harness compositor rather than a shared
    /// `WAYLAND_DISPLAY`).
    pub fn connect_to_path(
        path: impl AsRef<Path>,
        tx: Sender<OutputsMsg>,
    ) -> std::io::Result<Self> {
        let stream = UnixStream::connect(path)?;
        let conn =
            Connection::from_socket(stream).map_err(|e| std::io::Error::other(e.to_string()))?;
        Ok(Self::from_connection(conn, tx))
    }

    /// Wrap an already-open connection: create the queue, get the registry, and
    /// perform the initial enumeration roundtrips. Emits either
    /// [`OutputsMsg::HeadsChanged`] or [`OutputsMsg::ManagerUnavailable`].
    pub fn from_connection(conn: Connection, tx: Sender<OutputsMsg>) -> Self {
        let queue = conn.new_event_queue::<OutputsState>();
        let qh = queue.handle();
        let state = OutputsState::new(tx);
        let mut this = OutputsConnection {
            conn,
            queue,
            qh,
            state,
        };

        let display = this.conn.display();
        let _registry = display.get_registry(&this.qh, ());
        // First roundtrip hears the globals and binds the manager; the second
        // lets the manager emit its heads/modes and the closing `done`.
        let _ = this.queue.roundtrip(&mut this.state);
        let _ = this.queue.roundtrip(&mut this.state);

        if this.state.manager.is_none() {
            this.state.emit(OutputsMsg::ManagerUnavailable);
        }
        this
    }

    /// Borrow the queue's readiness fd — the glib source watches this.
    pub fn queue(&self) -> &EventQueue<OutputsState> {
        &self.queue
    }

    /// True once the manager global was bound.
    pub fn manager_present(&self) -> bool {
        self.state.manager.is_some()
    }

    /// A fresh snapshot of every enumerated head, in arrival order.
    pub fn heads(&self) -> Vec<Head> {
        self.state.heads.iter().map(HeadEntry::snapshot).collect()
    }

    /// Block until the server settles the current request stream (tests).
    pub fn roundtrip(&mut self) -> Result<usize, wayland_client::DispatchError> {
        self.queue.roundtrip(&mut self.state)
    }

    /// Non-blocking drain of already-buffered events — the first half of the
    /// glib callback. Returns the number dispatched.
    pub fn dispatch_pending(&mut self) -> Result<usize, wayland_client::DispatchError> {
        self.queue.dispatch_pending(&mut self.state)
    }

    /// Read whatever is ready on the socket, then dispatch it — the second half
    /// of the glib callback, run only when `dispatch_pending` found nothing.
    /// Never blocks: `prepare_read` yields `None` if another reader raced in,
    /// in which case the events are already ours to dispatch. A `WouldBlock`
    /// from the read is benign (the fd woke us but was already drained) and is
    /// reported as "nothing dispatched" rather than a dispatch error, so it
    /// never spuriously warns.
    pub fn read_and_dispatch(&mut self) -> Result<usize, wayland_client::DispatchError> {
        if let Some(guard) = self.queue.prepare_read() {
            match guard.read() {
                Ok(_) => {}
                Err(wayland_client::backend::WaylandError::Io(e))
                    if e.kind() == std::io::ErrorKind::WouldBlock =>
                {
                    return Ok(0);
                }
                Err(e) => return Err(wayland_client::DispatchError::Backend(e)),
            }
        }
        self.queue.dispatch_pending(&mut self.state)
    }

    /// Notify the view that the connection is gone. Called by the glib source
    /// when a dispatch/read errors so the page can drop into its
    /// unavailable state instead of the source busy-looping on a dead fd.
    pub fn notify_disconnected(&self) {
        self.state.emit(OutputsMsg::Disconnected);
    }

    /// Flush queued requests out to the compositor.
    pub fn flush(&self) -> Result<(), wayland_client::backend::WaylandError> {
        self.queue.flush()
    }

    /// Build a `zwlr_output_configuration_v1` from `edits` and `apply()` it.
    /// Results arrive asynchronously as [`OutputsMsg::ApplySucceeded`] /
    /// `ApplyFailed` / `ApplyCancelled`.
    pub fn build_and_send_configuration(&mut self, edits: &[HeadEdit]) -> Result<(), OutputsError> {
        self.build_configuration(edits, true)
    }

    /// Like [`Self::build_and_send_configuration`] but `test()`s the
    /// configuration instead of applying it — no state change on success.
    pub fn test_configuration(&mut self, edits: &[HeadEdit]) -> Result<(), OutputsError> {
        self.build_configuration(edits, false)
    }

    fn build_configuration(&mut self, edits: &[HeadEdit], apply: bool) -> Result<(), OutputsError> {
        let manager = self
            .state
            .manager
            .clone()
            .ok_or(OutputsError::ManagerUnavailable)?;

        // Build the configuration from the CURRENT live head set, not from the
        // edit list. `zwlr_output_configuration_v1` requires that every head
        // the client currently knows about be configured (enabled or disabled);
        // omitting one it has already learned about — a head hotplugged after
        // the page last rebuilt its edit list, say — makes the compositor
        // answer `unconfigured_head` and kill the connection. So we iterate the
        // live heads and layer any pending edit (matched by connector name) on
        // top, falling back to the head's current state for anything the edit
        // does not override (and for heads with no edit at all). An edit naming
        // a head that is no longer present is simply skipped — there is nothing
        // live to configure it against.
        let config = manager.create_configuration(
            self.state.serial,
            &self.qh,
            ConfigData { is_test: !apply },
        );
        for head in &self.state.heads {
            let edit = edits.iter().find(|e| e.name == head.name);
            let head_proxy = head.proxy.clone();

            let enabled = edit.map(|e| e.enabled).unwrap_or(head.enabled);
            if !enabled {
                config.disable_head(&head_proxy);
                continue;
            }

            let config_head = config.enable_head(&head_proxy, &self.qh, ());

            // A head being enabled must commit a concrete mode or a real DRM
            // output rejects it. Prefer the edit's chosen mode; if it named
            // none (e.g. the enable toggle flipped a previously-disabled head
            // on without touching resolution), fall back to the head's own
            // current/preferred/first advertised mode.
            let req = edit.and_then(|e| e.mode).or_else(|| head.default_mode());
            if let Some(req) = req {
                match head.modes.iter().find(|m| {
                    m.width == req.width
                        && m.height == req.height
                        && m.refresh_mhz == req.refresh_mhz
                }) {
                    Some(existing) => config_head.set_mode(&existing.proxy),
                    None => config_head.set_custom_mode(req.width, req.height, req.refresh_mhz),
                }
            }

            // Position/scale/transform: the edit's value when it set one, else
            // the head's current value, so a head carried in only because it is
            // live keeps its existing placement rather than snapping to the
            // compositor's default.
            let (x, y) = edit.and_then(|e| e.position).unwrap_or((head.x, head.y));
            config_head.set_position(x, y);
            let scale = edit.and_then(|e| e.scale).unwrap_or(head.scale);
            config_head.set_scale(scale);
            let transform = edit.and_then(|e| e.transform).unwrap_or(head.transform);
            config_head.set_transform(transform_from_i32(transform));
        }

        if apply {
            config.apply();
        } else {
            config.test();
        }
        // A flush failure after apply()/test() would otherwise leak the
        // configuration object: no terminal event ever arrives for it, so the
        // page would hang in its "Applying…" state forever. Destroy it and
        // surface the error so the caller leaves that state.
        if let Err(err) = self.flush() {
            config.destroy();
            return Err(OutputsError::Protocol(err));
        }
        Ok(())
    }
}

/// Map a raw `wl_output.transform` value to the concrete enum a request wants,
/// falling back to `Normal` for anything out of range.
fn transform_from_i32(value: i32) -> wl_output::Transform {
    wl_output::Transform::try_from(value as u32).unwrap_or(wl_output::Transform::Normal)
}

/// Map an event's `WEnum<Transform>` to its raw numeric value.
fn transform_to_i32(value: wayland_client::WEnum<wl_output::Transform>) -> i32 {
    match value {
        wayland_client::WEnum::Value(v) => v as i32,
        wayland_client::WEnum::Unknown(u) => u as i32,
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for OutputsState {
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
            && interface == ZwlrOutputManagerV1::interface().name
            && state.manager.is_none()
        {
            let bind_version = version.min(MANAGER_VERSION);
            let manager = registry.bind::<ZwlrOutputManagerV1, _, _>(name, bind_version, qh, ());
            state.manager = Some(manager);
        }
    }
}

impl Dispatch<ZwlrOutputManagerV1, ()> for OutputsState {
    fn event(
        state: &mut Self,
        _manager: &ZwlrOutputManagerV1,
        event: zwlr_output_manager_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_output_manager_v1::Event::Head { head } => {
                state.heads.push(HeadEntry::new(head));
            }
            zwlr_output_manager_v1::Event::Done { serial } => {
                state.serial = serial;
                state.saw_done = true;
                let snapshot: Vec<Head> = state.heads.iter().map(HeadEntry::snapshot).collect();
                state.emit(OutputsMsg::HeadsChanged(snapshot));
            }
            zwlr_output_manager_v1::Event::Finished => {
                state.manager = None;
                state.heads.clear();
                state.emit(OutputsMsg::ManagerUnavailable);
            }
            _ => {}
        }
    }

    event_created_child!(OutputsState, ZwlrOutputManagerV1, [
        zwlr_output_manager_v1::EVT_HEAD_OPCODE => (ZwlrOutputHeadV1, ()),
    ]);
}

impl Dispatch<ZwlrOutputHeadV1, ()> for OutputsState {
    fn event(
        state: &mut Self,
        head: &ZwlrOutputHeadV1,
        event: zwlr_output_head_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        let head_id = head.id();
        let Some(entry) = state.heads.iter_mut().find(|h| h.proxy.id() == head_id) else {
            return;
        };
        match event {
            zwlr_output_head_v1::Event::Name { name } => entry.name = name,
            zwlr_output_head_v1::Event::Description { description } => {
                entry.description = description
            }
            zwlr_output_head_v1::Event::Enabled { enabled } => entry.enabled = enabled != 0,
            zwlr_output_head_v1::Event::Mode { mode } => {
                let id = mode.id();
                entry.modes.push(ModeEntry {
                    proxy: mode,
                    id,
                    width: 0,
                    height: 0,
                    refresh_mhz: 0,
                    preferred: false,
                });
            }
            zwlr_output_head_v1::Event::CurrentMode { mode } => {
                entry.current_mode = Some(mode.id())
            }
            zwlr_output_head_v1::Event::Position { x, y } => {
                entry.x = x;
                entry.y = y;
            }
            zwlr_output_head_v1::Event::Transform { transform } => {
                entry.transform = transform_to_i32(transform)
            }
            zwlr_output_head_v1::Event::Scale { scale } => entry.scale = scale,
            zwlr_output_head_v1::Event::Finished => {
                state.heads.retain(|h| h.proxy.id() != head_id);
            }
            _ => {}
        }
    }

    event_created_child!(OutputsState, ZwlrOutputHeadV1, [
        zwlr_output_head_v1::EVT_MODE_OPCODE => (ZwlrOutputModeV1, ()),
    ]);
}

impl Dispatch<ZwlrOutputModeV1, ()> for OutputsState {
    fn event(
        state: &mut Self,
        mode: &ZwlrOutputModeV1,
        event: zwlr_output_mode_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        let mode_id = mode.id();
        match event {
            zwlr_output_mode_v1::Event::Size { width, height } => {
                if let Some(m) = state.head_by_mode(&mode_id) {
                    m.width = width;
                    m.height = height;
                }
            }
            zwlr_output_mode_v1::Event::Refresh { refresh } => {
                if let Some(m) = state.head_by_mode(&mode_id) {
                    m.refresh_mhz = refresh;
                }
            }
            zwlr_output_mode_v1::Event::Preferred => {
                if let Some(m) = state.head_by_mode(&mode_id) {
                    m.preferred = true;
                }
            }
            zwlr_output_mode_v1::Event::Finished => {
                for h in &mut state.heads {
                    h.modes.retain(|m| m.id != mode_id);
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwlrOutputConfigurationV1, ConfigData> for OutputsState {
    fn event(
        state: &mut Self,
        config: &ZwlrOutputConfigurationV1,
        event: zwlr_output_configuration_v1::Event,
        data: &ConfigData,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_output_configuration_v1::Event::Succeeded => {
                config.destroy();
                state.emit(OutputsMsg::ApplySucceeded {
                    is_test: data.is_test,
                });
            }
            zwlr_output_configuration_v1::Event::Failed => {
                config.destroy();
                state.emit(OutputsMsg::ApplyFailed {
                    is_test: data.is_test,
                });
            }
            zwlr_output_configuration_v1::Event::Cancelled => {
                config.destroy();
                state.emit(OutputsMsg::ApplyCancelled);
            }
            _ => {}
        }
    }
}

// The configuration-head object carries no events; it exists only to receive
// the per-head setters.
wayland_client::delegate_noop!(OutputsState: ignore ZwlrOutputConfigurationHeadV1);
