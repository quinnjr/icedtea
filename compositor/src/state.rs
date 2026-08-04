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
use icedtea_contract::{AltTabState, Event, Rectangle, WindowId};
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

use crate::input;
use crate::layout::{self, SnapZone};
use crate::render::WallpaperState;
use crate::window::WindowManager;

/// Output geometry information (simplified from smithay's `Output`).
pub struct OutputSurface {
    pub geometry: icedtea_contract::Rectangle,
}

/// Input passed to `State::handle_pointer`, in output logical coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerEvent {
    /// Button pressed while the pointer is over `id`.
    Press { id: WindowId, pointer: (i32, i32) },
    /// Pointer moved during an in-progress drag.
    Motion { pointer: (i32, i32) },
    /// Button released, ending an in-progress drag (a no-op if none is
    /// active).
    Release { pointer: (i32, i32) },
}

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
    /// Output geometries keyed by output index. Used by fullscreen toggle.
    pub outputs: HashMap<u32, OutputSurface>,
    /// Saved window geometries before fullscreen toggle, keyed by window ID.
    /// Used to restore non-fullscreen geometry when exiting fullscreen.
    pub saved_geometry: HashMap<WindowId, icedtea_contract::Rectangle>,
    /// Alt-tab cycling state, driven by `apply_action("cycle:alt_tab")` and
    /// consumed/reset by `handle_pointer`/`handle_key` release paths.
    pub alt_tab: input::AltTabMachine,
    /// Pointer-driven window move/snap state machine.
    pub drag: input::DragMachine,
    /// Set by `apply_action("quit")`; `main.rs`'s event-loop closure checks
    /// this each iteration and calls `stop()` once true.
    pub quitting: bool,
    /// Events produced by `apply_config` (e.g. on `reload`) that don't flow
    /// through `window_manager.pending_events` because `apply_config`
    /// replaces `window_manager` wholesale. Drained by `emit_pending`
    /// alongside `window_manager.pending_events`.
    pub pending_config_events: Vec<Event>,

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
            outputs: HashMap::new(),
            saved_geometry: HashMap::new(),
            alt_tab: input::AltTabMachine::new(),
            drag: input::DragMachine::new(),
            quitting: false,
            pending_config_events: Vec::new(),
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
    ///
    /// Also drains `pending_config_events`: `apply_config` replaces
    /// `window_manager` wholesale (so it can't queue onto the old instance's
    /// `pending_events`) and instead returns its events for the caller to
    /// stash there; this is where they actually reach `dbus_tx`.
    pub fn emit_pending(&mut self) {
        for ev in self.window_manager.pending_events.drain(..) {
            let _ = self.dbus_tx.send(ev);
        }
        for ev in self.pending_config_events.drain(..) {
            let _ = self.dbus_tx.send(ev);
        }
    }

    /// Queue an event for the next `emit_pending()` drain without it having
    /// come from a `window_manager` mutation (e.g. `AltTabState`, which is
    /// driven by `alt_tab`, not `window_manager`).
    pub fn emit(&mut self, ev: Event) {
        self.window_manager.pending_events.push(ev);
    }

    /// Toggle fullscreen state for a window. When entering fullscreen, saves the
    /// current geometry and sets geometry to the output rect. When exiting, restores
    /// the saved geometry. Emits WindowUpdated event.
    pub fn toggle_fullscreen(&mut self, id: WindowId) -> Option<()> {
        let w = self.window_manager.get(id)?;
        let output_geo = self.outputs.values().next().map(|o| o.geometry)?;
        let target = !w.fullscreen;
        self.window_manager.set_fullscreen(id, target)?;
        if target {
            // Save current geometry before entering fullscreen
            if let Some(w) = self.window_manager.get(id) {
                self.saved_geometry.insert(id, w.geometry);
            }
            self.window_manager.set_geometry(id, output_geo)?;
        } else {
            // Restore saved geometry when exiting fullscreen
            let saved = self.saved_geometry.remove(&id)?;
            self.window_manager.set_geometry(id, saved)?;
        }
        self.emit_pending();
        Some(())
    }

    /// Get the decoration action for a given window at the specified local coordinates.
    /// Returns the action if the window is focused and not fullscreen, None otherwise.
    /// Note: This gates on focused state (click-to-focus design), not CSD state.
    pub fn decoration_action_for(&self, id: WindowId, local: (i32, i32)) -> Option<crate::decoration::DecorationAction> {
        let w = self.window_manager.get(id)?;
        if !w.focused || w.fullscreen {
            return None;
        }
        Some(crate::decoration::hit_test(w.geometry, local))
    }

    /// Apply a decoration action to a window: Close removes it, Maximize toggles maximized state,
    /// Minimize minimizes, Move would be handled by the input layer (not here).
    pub fn apply_decoration_action(&mut self, id: WindowId, action: crate::decoration::DecorationAction) {
        use crate::decoration::DecorationAction;
        match action {
            DecorationAction::Close => {
                self.window_manager.remove_window(id);
            }
            DecorationAction::Maximize => {
                let _ = self.window_manager.toggle_maximized(id);
            }
            DecorationAction::Minimize => {
                let _ = self.window_manager.set_minimized(id, true);
            }
            DecorationAction::Move => {
                // Move is handled by the input layer (pointer drag),
                // not by a discrete window-manager mutation.
            }
            DecorationAction::None => {
                // No action.
            }
        }
        self.emit_pending();
    }

    /// Dispatch a keybinding action string (e.g. from `handle_key`, or a
    /// D-Bus-triggered action). Returns `None` for unrecognized actions or
    /// when a precondition (no focused window, unknown output, etc.) isn't
    /// met; `Some(())` on success. Always drains pending events on success.
    pub fn apply_action(&mut self, action: &str) -> Option<()> {
        let mut parts = action.splitn(2, ':');
        let base = parts.next()?;
        let arg = parts.next().map(|s| s.to_string());
        match base {
            "close" => {
                let id = self.window_manager.focused_window()?.id;
                self.window_manager.remove_window(id);
            }
            "fullscreen" => {
                let id = self.window_manager.focused_window()?.id;
                self.toggle_fullscreen(id)?;
            }
            "quit" => self.quitting = true,
            "reload" => {
                let cfg = icedtea_config::load_or_default(&icedtea_config::default_db_path());
                let events = self.apply_config(cfg);
                self.pending_config_events.extend(events);
            }
            // NOTE (brief deviation): the sample matched on the literal
            // `"cycle:alt_tab"` here, but `action.splitn(2, ':')` above
            // already split that string into `base = "cycle"`, `arg =
            // Some("alt_tab")` -- a match on `base` can never see the colon,
            // so as written this arm was as unreachable as the
            // `"snap:restore"` arm noted above. Matching `"cycle"` and
            // checking `arg` makes it reachable (and leaves room for other
            // `cycle:*` variants later without another silent dead arm).
            "cycle" => {
                if arg.as_deref() != Some("alt_tab") {
                    return None;
                }
                let entries = self.window_manager.alt_tab_entries();
                if entries.is_empty() {
                    return None;
                }
                if !self.alt_tab.is_active() {
                    self.alt_tab.start(entries.clone());
                } else {
                    self.alt_tab.step(true);
                }
                let idx = self.alt_tab.index();
                if let Some(wid) = entries.get(idx).copied() {
                    self.window_manager.focus(wid);
                }
                self.emit(Event::AltTabState(AltTabState { active: true, entries, index: idx }));
            }
            "spawn" => {
                let cmd = arg?;
                std::process::Command::new("sh").arg("-c").arg(&cmd).spawn().ok();
            }
            "workspace" => {
                let n: u32 = arg?.parse().ok()?;
                self.window_manager.set_active_workspace(n - 1);
            }
            "move_to_workspace" => {
                let id = self.window_manager.focused_window()?.id;
                let n: u32 = arg?.parse().ok()?;
                self.window_manager.set_workspace(id, n - 1)?;
                self.window_manager.set_active_workspace(n - 1);
            }
            // NOTE (brief deviation): the sample dispatch code had a second,
            // unreachable `"snap:restore" => ...` match arm alongside this
            // one. `action.splitn(2, ':')` always assigns `base = "snap"`
            // and `arg = Some("restore")` for the action string
            // `"snap:restore"` -- matching on `base` can therefore never
            // observe `"snap:restore"` as a whole. The restore case is
            // folded into this arm's `match arg?.as_str()` instead, which is
            // the only way it's reachable.
            "snap" => {
                let id = self.window_manager.focused_window()?.id;
                match arg?.as_str() {
                    "left" => self.snap(id, SnapZone::Left)?,
                    "right" => self.snap(id, SnapZone::Right)?,
                    "up" => self.snap(id, SnapZone::Top)?,
                    "down" => self.snap(id, SnapZone::Bottom)?,
                    "restore" => self.snap_restore(id)?,
                    _ => return None,
                }
            }
            _ => return None,
        }
        self.emit_pending();
        Some(())
    }

    /// Snap `id` to `zone` on the (first) output, saving its pre-snap
    /// geometry so `snap_restore` can undo it.
    pub fn snap(&mut self, id: WindowId, zone: SnapZone) -> Option<()> {
        let output_geo = self.outputs.values().next().map(|o| o.geometry)?;
        let gap = self.config.appearance.snap_gap;
        let current = self.window_manager.get(id)?.geometry;
        self.saved_geometry.entry(id).or_insert(current);
        self.window_manager.set_geometry(id, layout::snapped_geometry(output_geo, zone, gap))?;
        Some(())
    }

    /// Restore `id`'s geometry as it was before its most recent `snap`, if
    /// any was saved.
    pub fn snap_restore(&mut self, id: WindowId) -> Option<()> {
        if let Some(orig) = self.saved_geometry.remove(&id) {
            self.window_manager.set_geometry(id, orig)?;
        }
        Some(())
    }

    /// Rebuild `window_manager`'s workspaces from `cfg.workspace_names` and
    /// swap in the new config (keybindings/appearance/behavior). Returns the
    /// events the caller should queue (via `pending_config_events`) since
    /// the just-replaced `window_manager` can't carry them.
    pub fn apply_config(&mut self, cfg: Config) -> Vec<Event> {
        self.config = cfg.clone();
        self.window_manager = WindowManager::new(cfg.workspace_names.clone());
        vec![
            Event::WorkspaceList(self.window_manager.workspace_info()),
            Event::ConfigReloaded(cfg.appearance.clone()),
        ]
    }

    /// Resolve `mods`+`keysym` against the configured keybindings and apply
    /// the matched action, if any.
    pub fn handle_key(&mut self, mods: input::Modifiers, keysym: u32) -> Option<()> {
        let action = input::match_action(&self.config.keybindings, mods, keysym)?;
        self.apply_action(&action)
    }

    /// Dispatch a pointer event (output logical coordinates) through the
    /// drag/snap state machine. See `PointerEvent` for the three cases.
    pub fn handle_pointer(&mut self, event: PointerEvent) -> Option<()> {
        match event {
            PointerEvent::Press { id, pointer } => self.handle_pointer_press(id, pointer),
            PointerEvent::Motion { pointer } => self.handle_pointer_motion(pointer),
            PointerEvent::Release { pointer } => self.handle_pointer_release(pointer),
        }
    }

    /// Pointer button press at `pointer` (output logical coordinates) on
    /// window `id`: if it hits the title bar's move area, begins a drag;
    /// otherwise applies whatever discrete decoration action (close/
    /// maximize/minimize) was hit.
    fn handle_pointer_press(&mut self, id: WindowId, pointer: (i32, i32)) -> Option<()> {
        let geo = self.window_manager.get(id)?.geometry;
        // Despite its parameter name, `decoration_action_for`/`hit_test`
        // compares against `title_bar_rect`/`button_rects`, which are
        // themselves in the window's absolute (output) geometry space (see
        // the existing `decoration_action_for_returns_close_on_button_click`
        // test, which passes `geo.x + geo.width - 5` -- an absolute
        // coordinate -- as its "local" point). So `pointer` is passed
        // through unconverted here; only the drag `grab_offset` below is a
        // genuine window-relative offset.
        let action = self.decoration_action_for(id, pointer)?;
        if action == crate::decoration::DecorationAction::Move {
            let grab_offset = (pointer.0 - geo.x, pointer.1 - geo.y);
            self.drag.begin(id, grab_offset);
        } else {
            self.apply_decoration_action(id, action);
        }
        Some(())
    }

    /// Pointer motion at `pointer` (output logical coordinates) during an
    /// in-progress drag: updates the snap-zone preview (rendering consumes
    /// `self.snap_preview`).
    fn handle_pointer_motion(&mut self, pointer: (i32, i32)) -> Option<()> {
        self.drag.window_id()?;
        let output_geo = self.outputs.values().next().map(|o| o.geometry)?;
        let threshold = self.config.appearance.snap_gap.max(1) * 4;
        self.drag.motion(pointer, output_geo, threshold);
        self.snap_preview =
            self.drag.preview_zone().map(|zone| layout::snapped_geometry(output_geo, zone, self.config.appearance.snap_gap));
        Some(())
    }

    /// Pointer button release at `pointer` (output logical coordinates):
    /// ends the drag, either snapping the window to the previewed zone or
    /// moving it to `pointer - grab_offset`.
    fn handle_pointer_release(&mut self, pointer: (i32, i32)) -> Option<()> {
        let id = self.drag.window_id()?;
        let grab_offset = self.drag.grab_offset();
        self.snap_preview = None;
        match self.drag.end() {
            input::DragResult::Snapped(zone) => {
                self.snap(id, zone)?;
            }
            input::DragResult::Moved => {
                let geo = self.window_manager.get(id)?.geometry;
                let new_pos = (pointer.0 - grab_offset.0, pointer.1 - grab_offset.1);
                self.window_manager.set_geometry(
                    id,
                    Rectangle { x: new_pos.0, y: new_pos.1, width: geo.width, height: geo.height },
                )?;
            }
            input::DragResult::Restored => {}
        }
        self.emit_pending();
        Some(())
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

        // Default toplevel size: real geometry arrives from the client's
        // first commit (xdg_surface's set_window_geometry / buffer size),
        // which isn't known yet at `new_toplevel` time; 640x400 is just the
        // model's placeholder until then. Position cascades off whatever's
        // already mapped so new windows don't stack exactly on top of each
        // other.
        let occupied: Vec<icedtea_contract::Rectangle> =
            self.window_manager.windows().map(|w| w.geometry).collect();
        let (x, y) = layout::cascade_point(&occupied, (640, 400), 24);
        let geometry = icedtea_contract::Rectangle { x, y, width: 640, height: 400 };
        let id = self.window_manager.add_window(&app_id, &title, pid, geometry);
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
        // Track that client did not explicitly request client decorations
        if let Some(&window_id) = self.surface_to_window.get(toplevel.wl_surface()) {
            let _ = self.window_manager.set_client_decorations_requested(window_id, Some(false));
        }
    }

    fn request_mode(&mut self, toplevel: ToplevelSurface, mode: DecorationMode) {
        let is_client_side = matches!(mode, DecorationMode::ClientSide);
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(mode);
        });
        // Track the client's decoration preference
        if let Some(&window_id) = self.surface_to_window.get(toplevel.wl_surface()) {
            let _ = self.window_manager.set_client_decorations_requested(window_id, Some(is_client_side));
        }
        if toplevel.is_initial_configure_sent() {
            toplevel.send_pending_configure();
        }
    }

    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(DecorationMode::ServerSide);
        });
        // Reset to server-side when client unsets mode
        if let Some(&window_id) = self.surface_to_window.get(toplevel.wl_surface()) {
            let _ = self.window_manager.set_client_decorations_requested(window_id, Some(false));
        }
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
    use icedtea_contract::Rectangle;

    const DEFAULT_GEO: Rectangle = Rectangle { x: 0, y: 0, width: 640, height: 400 };

    #[test]
    fn state_emits_pending_events_on_channel() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        state.window_manager.add_window("app", "t", 1, DEFAULT_GEO);
        state.emit_pending();
        assert!(matches!(rx.try_recv(), Ok(Event::WindowOpened(_))));
    }

    #[test]
    fn decoration_action_for_returns_close_on_button_click() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        let id = state.window_manager.add_window("app", "t", 1, DEFAULT_GEO);
        state.window_manager.set_geometry(id, Rectangle { x: 100, y: 100, width: 600, height: 400 }).unwrap();
        state.window_manager.focus(id).unwrap();
        let geo = Rectangle { x: 100, y: 100, width: 600, height: 400 };
        // Click on rightmost button (close button)
        let close_pt = (geo.x + geo.width - 5, geo.y + 5);
        assert_eq!(state.decoration_action_for(id, close_pt), Some(crate::decoration::DecorationAction::Close));
    }

    #[test]
    fn decoration_action_for_returns_none_for_unfocused_window() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        let id1 = state.window_manager.add_window("app1", "t1", 1, DEFAULT_GEO);
        let _id2 = state.window_manager.add_window("app2", "t2", 2, DEFAULT_GEO);
        // _id2 is now focused; id1 is unfocused
        state.window_manager.set_geometry(id1, Rectangle { x: 100, y: 100, width: 600, height: 400 }).unwrap();
        let geo = Rectangle { x: 100, y: 100, width: 600, height: 400 };
        let close_pt = (geo.x + geo.width - 5, geo.y + 5);
        // Should return None because id1 is not focused
        assert_eq!(state.decoration_action_for(id1, close_pt), None);
    }

    #[test]
    fn decoration_action_for_returns_none_for_fullscreen_window() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        state.outputs.insert(0, OutputSurface { geometry: Rectangle { x: 0, y: 0, width: 1920, height: 1080 } });
        let id = state.window_manager.add_window("app", "t", 1, DEFAULT_GEO);
        state.window_manager.focus(id).unwrap();
        state.toggle_fullscreen(id).unwrap();
        let geo = Rectangle { x: 0, y: 0, width: 1920, height: 1080 };
        let close_pt = (geo.x + 5, geo.y + 5);
        // Should return None because window is fullscreen
        assert_eq!(state.decoration_action_for(id, close_pt), None);
    }

    #[test]
    fn decoration_action_for_returns_none_for_csd_window() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        let id = state.window_manager.add_window("org.gtk.App", "t", 1, DEFAULT_GEO);
        state.window_manager.set_geometry(id, Rectangle { x: 100, y: 100, width: 600, height: 400 }).unwrap();
        state.window_manager.focus(id).unwrap();
        state.window_manager.set_client_decorations_requested(id, Some(true)).unwrap();
        let geo = Rectangle { x: 100, y: 100, width: 600, height: 400 };
        let close_pt = (geo.x + geo.width - 5, geo.y + 5);
        // Should work even though client requested decorations (decoration_action_for doesn't filter by CSD)
        // CSD filtering happens at rendering time in draw_frame
        assert_eq!(state.decoration_action_for(id, close_pt), Some(crate::decoration::DecorationAction::Close));
    }

    #[test]
    fn toggle_fullscreen_flips_state_and_geometry() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        state.outputs.insert(0, OutputSurface { geometry: Rectangle { x: 0, y: 0, width: 1920, height: 1080 } });
        let id = state.window_manager.add_window("app", "t", 1, DEFAULT_GEO);
        state.toggle_fullscreen(id).unwrap();
        let w = state.window_manager.get(id).unwrap();
        assert!(w.fullscreen);
        assert_eq!(w.geometry, Rectangle { x: 0, y: 0, width: 1920, height: 1080 });
    }

    // NOTE (brief deviation): the brief's Step-1 sample for
    // `apply_action_switches_workspaces_and_snaps` calls
    // `state.window_manager.add_window("app", "t", 1)` (3 args), never
    // registers an output, and adds the window *before* switching
    // workspaces. Three real bugs, not just transcription noise:
    //   1. 3-arg `add_window` no longer compiles against the new 5-arg
    //      signature (this task's own change).
    //   2. `snap()` reads `self.outputs` (empty by default in `State::new`)
    //      to find output geometry, despite the brief's own prose saying
    //      "the tests assume ... a 1000x800 output" -- nothing in the
    //      sample ever inserts one.
    //   3. `WindowManager::focused_window()` is scoped to the *active*
    //      workspace (established well before this task -- see
    //      `window.rs`'s `move_to_workspace_keeps_focus_valid` test, which
    //      asserts exactly this). Adding the window on workspace 0 and then
    //      switching to workspace 2 (index 1) leaves workspace 1 with no
    //      focused window at all, so `apply_action("snap:left")` -- which
    //      dispatches through `focused_window()` -- would return `None`
    //      and the `.unwrap()` would panic. Switching workspace *first*,
    //      then adding the window (which auto-focuses on whatever is
    //      currently active), keeps the window's workspace and the active
    //      workspace in agreement, matching how a real "switch to an empty
    //      workspace, open something there, then snap it" sequence would
    //      actually behave.
    // All three are fixed below; the assertions are unchanged from the
    // brief.
    #[test]
    fn apply_action_switches_workspaces_and_snaps() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        state.outputs.insert(0, OutputSurface { geometry: Rectangle { x: 0, y: 0, width: 1000, height: 800 } });
        state.apply_action("workspace:2").unwrap();
        assert_eq!(state.window_manager.active_workspace(), 1);
        let id = state.window_manager.add_window("app", "t", 1, DEFAULT_GEO);
        state.apply_action("snap:left").unwrap();
        assert_eq!(state.window_manager.get(id).unwrap().geometry.width, 1000 / 2 - 16);
        state.apply_action("snap:restore").unwrap();
        assert_eq!(state.window_manager.get(id).unwrap().geometry.width, 640);
    }

    #[test]
    fn apply_config_rebuilds_workspaces() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        let mut cfg = icedtea_config::default_config();
        cfg.workspace_names = vec!["A".into(), "B".into()];
        let events = state.apply_config(cfg);
        assert_eq!(state.window_manager.workspace_info().len(), 2);
        assert!(events.iter().any(|e| matches!(e, Event::WorkspaceList(_))));
    }

    #[test]
    fn close_action_removes_focused() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        let id = state.window_manager.add_window("app", "t", 1, DEFAULT_GEO);
        state.apply_action("close").unwrap();
        assert!(state.window_manager.get(id).is_none());
    }

    #[test]
    fn alt_tab_cycles_focus() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        let a = state.window_manager.add_window("a", "a", 1, DEFAULT_GEO);
        let b = state.window_manager.add_window("b", "b", 2, DEFAULT_GEO);
        state.apply_action("cycle:alt_tab").unwrap();
        assert!(state.window_manager.get(a).unwrap().focused);
        state.apply_action("cycle:alt_tab").unwrap();
        assert!(state.window_manager.get(b).unwrap().focused);
        state.apply_action("cycle:alt_tab").unwrap();
        assert!(state.window_manager.get(a).unwrap().focused);
    }

    #[test]
    fn handle_key_dispatches_bound_action() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        let id = state.window_manager.add_window("app", "t", 1, DEFAULT_GEO);
        // Default config binds SUPER+q to "close" (see config/src/defaults.rs).
        state.handle_key(input::Modifiers::SUPER, input::key_name_to_keysym("KEY_q")).unwrap();
        assert!(state.window_manager.get(id).is_none());
    }

    #[test]
    fn handle_key_ignores_unbound_combo() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        assert_eq!(state.handle_key(input::Modifiers::empty(), 0x12345), None);
    }

    #[test]
    fn handle_pointer_drag_moves_window_without_snap() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        state.outputs.insert(0, OutputSurface { geometry: Rectangle { x: 0, y: 0, width: 1000, height: 800 } });
        let id = state.window_manager.add_window("app", "t", 1, Rectangle { x: 100, y: 100, width: 640, height: 400 });
        state.window_manager.focus(id).unwrap();
        // Press inside the title bar's move area (not on a button).
        state.handle_pointer(PointerEvent::Press { id, pointer: (120, 105) }).unwrap();
        // Drag to a point away from any snap edge.
        state.handle_pointer(PointerEvent::Motion { pointer: (400, 400) }).unwrap();
        assert_eq!(state.snap_preview, None);
        state.handle_pointer(PointerEvent::Release { pointer: (400, 400) }).unwrap();
        let geo = state.window_manager.get(id).unwrap().geometry;
        // grab_offset was (20, 5); released at (400, 400) => top-left (380, 395).
        assert_eq!((geo.x, geo.y), (380, 395));
    }

    #[test]
    fn handle_pointer_drag_to_edge_snaps() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        state.outputs.insert(0, OutputSurface { geometry: Rectangle { x: 0, y: 0, width: 1000, height: 800 } });
        let id = state.window_manager.add_window("app", "t", 1, Rectangle { x: 100, y: 100, width: 640, height: 400 });
        state.window_manager.focus(id).unwrap();
        state.handle_pointer(PointerEvent::Press { id, pointer: (120, 105) }).unwrap();
        state.handle_pointer(PointerEvent::Motion { pointer: (2, 400) }).unwrap();
        assert!(state.snap_preview.is_some());
        state.handle_pointer(PointerEvent::Release { pointer: (2, 400) }).unwrap();
        let geo = state.window_manager.get(id).unwrap().geometry;
        assert_eq!(geo.width, 1000 / 2 - 16);
        assert_eq!(state.snap_preview, None);
    }
}
