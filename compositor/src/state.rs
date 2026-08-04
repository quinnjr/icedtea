//! The smithay event-loop state: `State` owns the Wayland protocol state
//! (globals, seat, space) alongside our own window model (`WindowManager`)
//! and configuration, and fans out `contract::Event`s produced by model
//! mutations onto a crossbeam channel for the D-Bus side to consume.
//!
//! Deviation from the task-7 brief: `State::new` cannot literally follow
//! anvil's `AnvilState::init(display, handle, backend_data, listen_on_socket)`
//! signature, because the brief's own interface line and its Step-5 test both
//! require `State::new(config, dbus_tx) -> Self` with *no* `Display`/
//! `LoopHandle`/backend argument (the test builds a `State` with nothing but
//! a config and a channel sender, entirely outside of an event loop). So
//! `State::new` creates its own `wayland_server::Display<State>` internally
//! (registering every global against it, exactly as anvil's `init` does) and
//! holds it in a `take`-able slot; `main.rs` calls `State::take_display()`
//! once, after the event loop exists, to hand the display off to the
//! standard `ListeningSocketSource` + `Generic` dispatch wiring.

use std::collections::HashMap;
use std::time::Instant;

use icedtea_config::Config;
use icedtea_contract::{Event, WindowId};
use smithay::backend::renderer::utils::on_commit_buffer_handler;
use smithay::desktop::{PopupKind, PopupManager, Space, Window};
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::output::Output;
use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as DecorationMode;
use smithay::reexports::wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use smithay::reexports::wayland_server::protocol::wl_seat::WlSeat;
use smithay::reexports::wayland_server::protocol::{wl_buffer, wl_surface::WlSurface};
use smithay::reexports::wayland_server::{Client, Display, DisplayHandle, Resource};
use smithay::utils::Serial;
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::compositor::{
    get_parent, is_sync_subsurface, CompositorClientState, CompositorHandler, CompositorState,
};
use smithay::wayland::output::{OutputHandler, OutputManagerState};
use smithay::wayland::selection::data_device::{
    set_data_device_focus, ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
};
use smithay::wayland::selection::SelectionHandler;
use smithay::wayland::shell::wlr_layer::{Layer, LayerSurface, WlrLayerShellHandler, WlrLayerShellState};
use smithay::wayland::shell::xdg::decoration::{XdgDecorationHandler, XdgDecorationState};
use smithay::wayland::shell::xdg::{
    PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState, XdgToplevelSurfaceData,
};
use smithay::wayland::shm::{ShmHandler, ShmState};
use smithay::{
    delegate_compositor, delegate_data_device, delegate_layer_shell, delegate_output, delegate_seat,
    delegate_shm, delegate_xdg_decoration, delegate_xdg_shell,
};

use crate::render::WallpaperState;
use crate::window::WindowManager;

/// Per-client data attached by `insert_client`.
#[derive(Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}

pub struct State {
    pub window_manager: WindowManager,
    pub config: Config,
    pub dbus_tx: crossbeam_channel::Sender<Event>,

    pub display_handle: DisplayHandle,
    /// Held until `take_display` is called once the event loop exists (see
    /// module docs for why `State::new` can't take a `Display` itself).
    display: Option<Display<State>>,
    pub start_time: Instant,
    /// Set once by `main.rs` via `set_loop_signal` after the event loop is
    /// created (unavailable at `State::new` time per this module's `Display`
    /// deviation note); `stop()` is a no-op before that.
    loop_signal: Option<smithay::reexports::calloop::LoopSignal>,

    pub space: Space<Window>,
    pub popups: PopupManager,
    /// Wallpaper decode/upload pipeline state (`render::draw_frame` drives
    /// this each frame).
    pub wallpaper: WallpaperState,
    /// The drag machine's current snap-preview target, in output logical
    /// coordinates, or `None` when no drag is in a snap-preview state. Set
    /// by the drag machine (Task 9); this is the geometry hook `render.rs`
    /// consumes.
    pub snap_preview: Option<icedtea_contract::Rectangle>,
    /// Maps a mapped toplevel's underlying `wl_surface` to the `WindowId` our
    /// own model assigned it, so unmap/title-change events can be routed
    /// back into `window_manager` without re-deriving state from the
    /// wayland-protocol object.
    surface_to_window: HashMap<WlSurface, WindowId>,

    // Wayland global state.
    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub shm_state: ShmState,
    pub output_manager_state: OutputManagerState,
    pub seat_state: SeatState<State>,
    pub data_device_state: DataDeviceState,
    pub layer_shell_state: WlrLayerShellState,
    pub xdg_decoration_state: XdgDecorationState,

    pub seat: Seat<State>,
}

impl State {
    pub fn new(config: Config, dbus_tx: crossbeam_channel::Sender<Event>) -> Self {
        let display: Display<State> = Display::new().expect("failed to create wayland display");
        let dh = display.handle();

        let compositor_state = CompositorState::new::<Self>(&dh);
        let xdg_shell_state = XdgShellState::new::<Self>(&dh);
        let shm_state = ShmState::new::<Self>(&dh, vec![]);
        let output_manager_state = OutputManagerState::new_with_xdg_output::<Self>(&dh);
        let mut seat_state = SeatState::new();
        let data_device_state = DataDeviceState::new::<Self>(&dh);
        let layer_shell_state = WlrLayerShellState::new::<Self>(&dh);
        let xdg_decoration_state = XdgDecorationState::new::<Self>(&dh);

        let mut seat: Seat<Self> = seat_state.new_wl_seat(&dh, "seat0");
        seat.add_pointer();
        seat.add_keyboard(Default::default(), 200, 25).expect("failed to initialize keyboard");

        let workspace_names = config.workspace_names.clone();

        Self {
            window_manager: WindowManager::new(workspace_names),
            config,
            dbus_tx,
            display_handle: dh,
            display: Some(display),
            start_time: Instant::now(),
            loop_signal: None,
            space: Space::default(),
            popups: PopupManager::default(),
            wallpaper: WallpaperState::new(),
            snap_preview: None,
            surface_to_window: HashMap::new(),
            compositor_state,
            xdg_shell_state,
            shm_state,
            output_manager_state,
            seat_state,
            data_device_state,
            layer_shell_state,
            xdg_decoration_state,
            seat,
        }
    }

    /// Hand off the wayland `Display` for insertion into the calloop event
    /// loop. Panics if called more than once.
    pub fn take_display(&mut self) -> Display<State> {
        self.display.take().expect("display already taken")
    }

    /// Wire up the calloop stop signal once the event loop exists.
    pub fn set_loop_signal(&mut self, signal: smithay::reexports::calloop::LoopSignal) {
        self.loop_signal = Some(signal);
    }

    /// Request the event loop to stop (e.g. on `WinitEvent::CloseRequested`).
    pub fn stop(&mut self) {
        if let Some(signal) = &self.loop_signal {
            signal.stop();
        }
    }

    /// Register a new output (backends call this once they know the mode).
    ///
    /// `transform` is backend-specific, not a fixed default: winit/GL
    /// framebuffers are Y-flipped relative to the compositor's logical space,
    /// so the nested backend must pass `Transform::Flipped180` (matching
    /// `smallvil/src/winit.rs:40` and `anvil/src/winit.rs:125`), while a real
    /// DRM scanout output uses `Transform::Normal`.
    pub fn create_output(
        &mut self,
        name: &str,
        physical: smithay::output::PhysicalProperties,
        mode: smithay::output::Mode,
        transform: smithay::utils::Transform,
    ) -> Output {
        let output = Output::new(name.to_string(), physical);
        output.create_global::<Self>(&self.display_handle);
        output.change_current_state(Some(mode), Some(transform), None, Some((0, 0).into()));
        output.set_preferred(mode);
        self.space.map_output(&output, (0, 0));
        output
    }

    /// Drain `window_manager.pending_events` onto `dbus_tx`. Must be called
    /// after every mutation of `window_manager` so subscribers observe it.
    pub fn emit_pending(&mut self) {
        for ev in self.window_manager.pending_events.drain(..) {
            let _ = self.dbus_tx.send(ev);
        }
    }
}

// --- wl_compositor / wl_subcompositor / buffer / wl_shm ---

impl CompositorHandler for State {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client.get_data::<ClientState>().unwrap().compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);
        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }
            if let Some(window) =
                self.space.elements().find(|w| w.toplevel().is_some_and(|t| t.wl_surface() == &root))
            {
                window.on_commit();
            }
        }

        // Send the initial xdg_toplevel configure once a client commits after mapping.
        if let Some(window) =
            self.space.elements().find(|w| w.toplevel().is_some_and(|t| t.wl_surface() == surface)).cloned()
        {
            let initial_configure_sent = smithay::wayland::compositor::with_states(surface, |states| {
                states.data_map.get::<XdgToplevelSurfaceData>().unwrap().lock().unwrap().initial_configure_sent
            });
            if !initial_configure_sent {
                window.toplevel().unwrap().send_configure();
            }
        }

        self.popups.commit(surface);
        if let Some(PopupKind::Xdg(popup)) = self.popups.find_popup(surface)
            && !popup.is_initial_configure_sent()
        {
            let _ = popup.send_configure();
        }
    }
}

impl BufferHandler for State {
    fn buffer_destroyed(&mut self, _buffer: &wl_buffer::WlBuffer) {}
}

impl ShmHandler for State {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

delegate_compositor!(State);
delegate_shm!(State);

// --- wl_seat ---

impl SeatHandler for State {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<State> {
        &mut self.seat_state
    }

    fn cursor_image(&mut self, _seat: &Seat<Self>, _image: smithay::input::pointer::CursorImageStatus) {}

    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) {
        let dh = &self.display_handle;
        let client = focused.and_then(|s| dh.get_client(s.id()).ok());
        set_data_device_focus(dh, seat, client);
    }
}

delegate_seat!(State);

// --- wl_data_device (implied by seat handling) ---

impl SelectionHandler for State {
    type SelectionUserData = ();
}

impl DataDeviceHandler for State {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

impl ClientDndGrabHandler for State {}
impl ServerDndGrabHandler for State {}

delegate_data_device!(State);

// --- wl_output / xdg-output ---

impl OutputHandler for State {}
delegate_output!(State);

// --- xdg_wm_base ---

impl XdgShellHandler for State {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        let (app_id, title) = smithay::wayland::compositor::with_states(surface.wl_surface(), |states| {
            let data = states.data_map.get::<XdgToplevelSurfaceData>().unwrap().lock().unwrap();
            (data.app_id.clone().unwrap_or_default(), data.title.clone().unwrap_or_default())
        });
        let pid = surface
            .wl_surface()
            .client()
            .and_then(|c| c.get_credentials(&self.display_handle).ok())
            .map(|c| c.pid as u32)
            .unwrap_or(0);

        let id = self.window_manager.add_window(&app_id, &title, pid);
        self.emit_pending();
        self.surface_to_window.insert(surface.wl_surface().clone(), id);

        let window = Window::new_wayland_window(surface);
        self.space.map_element(window, (0, 0), false);
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        if let Some(id) = self.surface_to_window.remove(surface.wl_surface()) {
            self.window_manager.remove_window(id);
            self.emit_pending();
        }
        let window = self
            .space
            .elements()
            .find(|w| w.toplevel().is_some_and(|t| t.wl_surface() == surface.wl_surface()))
            .cloned();
        if let Some(window) = window {
            self.space.unmap_elem(&window);
        }
    }

    fn title_changed(&mut self, surface: ToplevelSurface) {
        if let Some(&id) = self.surface_to_window.get(surface.wl_surface()) {
            let title = smithay::wayland::compositor::with_states(surface.wl_surface(), |states| {
                states.data_map.get::<XdgToplevelSurfaceData>().unwrap().lock().unwrap().title.clone()
            })
            .unwrap_or_default();
            self.window_manager.set_title(id, title);
            self.emit_pending();
        }
    }

    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        let _ = self.popups.track_popup(PopupKind::Xdg(surface));
    }

    fn reposition_request(&mut self, surface: PopupSurface, positioner: PositionerState, token: u32) {
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
            state.positioner = positioner;
        });
        surface.send_repositioned(token);
    }

    fn grab(&mut self, _surface: PopupSurface, _seat: WlSeat, _serial: Serial) {
        // Popup pointer grabs land alongside real input handling in a later task.
    }
}

delegate_xdg_shell!(State);

// --- zwlr_layer_shell_v1 ---

impl WlrLayerShellHandler for State {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.layer_shell_state
    }

    fn new_layer_surface(
        &mut self,
        surface: LayerSurface,
        _output: Option<smithay::reexports::wayland_server::protocol::wl_output::WlOutput>,
        _layer: Layer,
        _namespace: String,
    ) {
        // Layer-surface placement (bars, docks, overlays) lands in the
        // layout task; for now we just acknowledge the global exists so
        // wlr-layer-shell clients (e.g. a status bar) can bind it.
        let _ = surface;
    }
}

delegate_layer_shell!(State);

// --- zxdg_decoration_manager_v1 ---

impl XdgDecorationHandler for State {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(DecorationMode::ServerSide);
        });
    }

    fn request_mode(&mut self, toplevel: ToplevelSurface, mode: DecorationMode) {
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(mode);
        });
        if toplevel.is_initial_configure_sent() {
            toplevel.send_pending_configure();
        }
    }

    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(DecorationMode::ServerSide);
        });
        if toplevel.is_initial_configure_sent() {
            toplevel.send_pending_configure();
        }
    }
}

delegate_xdg_decoration!(State);

#[cfg(test)]
mod tests {
    use super::*;
    use icedtea_config::default_config;

    #[test]
    fn state_emits_pending_events_on_channel() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        state.window_manager.add_window("app", "t", 1);
        state.emit_pending();
        assert!(matches!(rx.try_recv(), Ok(Event::WindowOpened(_))));
    }
}
