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
use std::path::PathBuf;
use std::time::Instant;

use icedtea_config::Config;
use icedtea_contract::{AltTabState, Event, Rectangle, SeqEvent, WindowId};
use smithay::backend::renderer::utils::on_commit_buffer_handler;
use smithay::desktop::{PopupKind, PopupManager, Space, Window};
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::output::Output;
use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as DecorationMode;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::{
    ResizeEdge as XdgResizeEdge, State as XdgToplevelState,
};
use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;
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
    /// Outbound event channel. Every message carries the `seq` its mutation
    /// produced (review finding I2) so a subscriber can order signals
    /// against a `GetState()` snapshot and detect gaps.
    pub dbus_tx: crossbeam_channel::Sender<SeqEvent>,

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
    /// The `Space` element backing each model window, kept here rather than
    /// looked up by scanning `space.elements()`: an element that is unmapped
    /// (because its workspace isn't active -- review finding I1) is no
    /// longer *in* the `Space` at all, so a scan could never find it again
    /// to map it back when its workspace returns.
    elements: HashMap<WindowId, Window>,
    /// Output geometries keyed by output index. Used by fullscreen toggle.
    pub outputs: HashMap<u32, OutputSurface>,
    /// Saved window geometries before fullscreen toggle, keyed by window ID.
    /// Used to restore non-fullscreen geometry when exiting fullscreen.
    ///
    /// Kept separate from `snap_saved_geometry` (Task 11 review #2): both
    /// `toggle_fullscreen` and `snap` used to share one `saved_geometry` map
    /// keyed only by `WindowId`, so snapping a window and then
    /// fullscreening it clobbered the pre-snap geometry with the snapped
    /// one, and `snap_restore` after unfullscreening silently restored
    /// nothing (it returns `Some(())` on a missing entry). Two independent
    /// save slots per window means each feature's restore point survives
    /// the other feature running in between.
    pub fullscreen_saved_geometry: HashMap<WindowId, icedtea_contract::Rectangle>,
    /// Saved window geometry before a `snap`, keyed by window ID. Used by
    /// `snap_restore`. See `fullscreen_saved_geometry`'s doc for why this is
    /// a separate map.
    pub snap_saved_geometry: HashMap<WindowId, icedtea_contract::Rectangle>,
    /// Saved window geometry before a maximize, keyed by window ID. Its own
    /// slot for the same reason `fullscreen_saved_geometry` and
    /// `snap_saved_geometry` are separate: maximizing a snapped window must
    /// not clobber snap's restore point (review finding I5).
    pub maximized_saved_geometry: HashMap<WindowId, icedtea_contract::Rectangle>,
    /// Alt-tab cycling state, driven by `apply_action("cycle:alt_tab")` and
    /// ended by `end_alt_tab` (called from `backend.rs`'s keyboard filter
    /// when the held modifier is released -- see `end_alt_tab`'s doc for
    /// why that's the chosen end condition).
    pub alt_tab: input::AltTabMachine,
    /// Pointer-driven window move/snap state machine.
    pub drag: input::DragMachine,
    /// Pointer-driven interactive-resize state machine, started by the xdg
    /// `resize_request` handler and stepped by the same pointer
    /// motion/release path the drag machine uses.
    pub resize: input::ResizeMachine,
    /// Set by `apply_action("quit")`; `main.rs`'s event-loop closure checks
    /// this each iteration and calls `stop()` once true.
    pub quitting: bool,
    /// Last known pointer position in output logical coordinates. `backend.rs`
    /// updates this on every `PointerMotionAbsolute` event; `PointerButton`
    /// events (which carry no position of their own) read it back to build
    /// `PointerEvent::Press`/`Release`.
    pub pointer_location: (i32, i32),
    /// On-disk location `reload_config_from_disk`/`handle_command`'s
    /// `ReloadConfig` worker thread reads from. `None` (the boot-time
    /// default) means "use `icedtea_config::default_db_path()`" -- this is
    /// only ever overridden by tests, which need an isolated temp DB rather
    /// than the real XDG path.
    pub config_path: Option<PathBuf>,
    /// Set by `main.rs` via `set_config_reload_sender` once the event loop
    /// exists (unavailable at `State::new` time, same reason
    /// `loop_signal` is). `handle_command`'s `ReloadConfig` arm sends the
    /// freshly-loaded `Config` back over this from a worker thread so the
    /// redb I/O + JSON parse never blocks the render loop (task-13's
    /// threading requirement); `None` before that wiring exists is a no-op
    /// (nothing to reload into).
    config_reload_tx: Option<smithay::reexports::calloop::channel::Sender<Config>>,

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
    pub fn new(config: Config, dbus_tx: crossbeam_channel::Sender<SeqEvent>) -> Self {
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
        // Review finding I4: report unusable bindings once, here, instead of
        // from inside the per-key-press lookup.
        input::warn_about_keybindings(&config.keybindings);

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
            elements: HashMap::new(),
            outputs: HashMap::new(),
            fullscreen_saved_geometry: HashMap::new(),
            snap_saved_geometry: HashMap::new(),
            maximized_saved_geometry: HashMap::new(),
            alt_tab: input::AltTabMachine::new(),
            drag: input::DragMachine::new(),
            resize: input::ResizeMachine::new(),
            quitting: false,
            pointer_location: (0, 0),
            config_path: None,
            config_reload_tx: None,
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

    /// Wire up the config-reload result channel once the event loop exists
    /// (see `config_reload_tx`'s field doc for why this can't be done in
    /// `State::new`). `main.rs` registers the paired receiver half as a
    /// calloop source that applies whatever `Config` arrives via
    /// `apply_reloaded_config`.
    pub fn set_config_reload_sender(&mut self, tx: smithay::reexports::calloop::channel::Sender<Config>) {
        self.config_reload_tx = Some(tx);
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
        // Task 11 review (minor): `self.outputs` -- the geometry map `snap`,
        // `snap_restore`'s counterpart `toggle_fullscreen`, and
        // `handle_pointer_motion` all read -- was never populated outside
        // test bodies, so every snap/fullscreen action was a silent runtime
        // no-op. Single-output-at-(0,0) only for now (matches
        // `space.map_output(&output, (0, 0))` above); multi-output geometry
        // placement is out of this task's scope.
        let index = self.outputs.len() as u32;
        self.outputs.insert(
            index,
            OutputSurface { geometry: icedtea_contract::Rectangle { x: 0, y: 0, width: mode.size.w, height: mode.size.h } },
        );
        output
    }

    /// Drain `window_manager.pending_events` onto `dbus_tx`. Must be called
    /// after every mutation of `window_manager` so subscribers observe it.
    ///
    /// Each event goes out as a `SeqEvent` carrying the `seq` its mutation
    /// produced (review finding I2); `apply_reloaded_config` pushes
    /// `apply_config`'s events through `WindowManager::push_event` for the
    /// same reason, so there is exactly one queue and one counter.
    pub fn emit_pending(&mut self) {
        for ev in self.window_manager.pending_events.drain(..) {
            let _ = self.dbus_tx.send(ev);
        }
    }

    /// Queue an event for the next `emit_pending()` drain without it having
    /// come from a `window_manager` mutation (e.g. `AltTabState`, which is
    /// driven by `alt_tab`, not `window_manager`).
    ///
    /// Task 11 re-review #4: this used to push straight onto
    /// `pending_events`, bypassing `WindowManager::bump()` -- every
    /// `AltTabState` went out without ever advancing `seq`, so
    /// `snapshot().seq` couldn't be used to detect that an alt-tab change
    /// had happened. Routed through `note_event()` (which bumps then
    /// returns the same `pending_events` vec) so it participates in the
    /// same sequence counter as every other event.
    pub fn emit(&mut self, ev: Event) {
        self.window_manager.push_event(ev);
    }

    // --- Model <-> `Space`/xdg reconciliation (review finding C1) ---
    //
    // Before this, `new_toplevel`'s `map_element(window, (0, 0), false)` was
    // the only call that ever positioned anything in the `Space`, and no
    // `send_configure`/`send_close` existed anywhere in the tree: every
    // client rendered at (0,0), was never told its size, and never learned
    // it had been asked to close, fullscreen, or maximize. The model, the
    // D-Bus surface, and the tests were all self-consistent -- it was the
    // model -> Wayland edge that was missing, and no single task owned it.
    //
    // The whole edge is these three functions plus their call sites: every
    // geometry/state mutation ends in `sync_window_to_space`, every
    // workspace/visibility change ends in `sync_space`, and every close goes
    // through `request_close`.

    /// The `Space` element backing model window `id`, if any -- mapped or
    /// not (see the `elements` field). Model windows created by tests (and
    /// by any future non-Wayland source) have no element; every caller
    /// treats that as "nothing to push to a client".
    fn element_for(&self, id: WindowId) -> Option<Window> {
        self.elements.get(&id).cloned()
    }

    /// Push model window `id`'s geometry, visibility, and xdg state onto its
    /// client: (re)map the `Space` element at the model's position, or unmap
    /// it when the window isn't on the active workspace (review finding I1),
    /// and configure the toplevel with the model's size plus the
    /// maximized/fullscreen/activated states.
    ///
    /// Call this at the tail of *every* geometry or state mutation. A window
    /// with no backing surface is a silent no-op.
    pub fn sync_window_to_space(&mut self, id: WindowId) {
        let Some(element) = self.element_for(id) else { return };
        let Some(w) = self.window_manager.get(id) else {
            // The model window is gone but its element is still mapped
            // (e.g. a close that raced the client's destroy): drop it rather
            // than rendering a window nothing tracks.
            self.space.unmap_elem(&element);
            self.elements.remove(&id);
            return;
        };
        let (geo, fullscreen, maximized, focused) = (w.geometry, w.fullscreen, w.maximized, w.focused);
        let visible = self.window_manager.is_visible(w);

        if !visible {
            self.space.unmap_elem(&element);
        } else {
            let mapped_at = self.space.element_location(&element);
            let moved = mapped_at != Some((geo.x, geo.y).into());
            // `Space::map_element` also restacks the element to the top, so
            // it is the raise operation as well as the move one. Ledger item
            // 28 / recommendation 4: `behavior.raise_on_focus` is what
            // decides whether a focus change alone is allowed to raise --
            // with it off, a focused window is still activated and
            // configured, it just keeps its place in the stack unless its
            // geometry actually changed.
            if moved || (focused && self.config.behavior.raise_on_focus) {
                self.space.map_element(element.clone(), (geo.x, geo.y), focused);
            }
        }

        if let Some(toplevel) = element.toplevel() {
            toplevel.with_pending_state(|state| {
                state.size = Some((geo.width.max(1), geo.height.max(1)).into());
                let set = |states: &mut smithay::wayland::shell::xdg::ToplevelStateSet,
                           flag: XdgToplevelState,
                           on: bool| {
                    if on {
                        states.set(flag);
                    } else {
                        states.unset(flag);
                    }
                };
                set(&mut state.states, XdgToplevelState::Maximized, maximized);
                set(&mut state.states, XdgToplevelState::Fullscreen, fullscreen);
                set(&mut state.states, XdgToplevelState::Activated, focused && visible);
            });
            // Before the initial configure the client hasn't committed yet;
            // `CompositorHandler::commit` sends that first configure, and it
            // picks up whatever pending state was staged above.
            if toplevel.is_initial_configure_sent() {
                toplevel.send_pending_configure();
            }
        }

        // Keyboard focus is the other half of "focus reached the client":
        // `backend.rs`'s keyboard filter forwards every unconsumed key press
        // to the seat's current focus, and nothing ever set one, so no
        // client could receive a single keystroke. Re-set only on an actual
        // change, so an ordinary geometry sync doesn't churn
        // `leave`/`enter` pairs at the client.
        if focused && visible {
            let surface = element.toplevel().map(|t| t.wl_surface().clone());
            if let Some(keyboard) = self.seat.get_keyboard()
                && keyboard.current_focus() != surface
            {
                let serial = smithay::utils::SERIAL_COUNTER.next_serial();
                keyboard.set_focus(self, surface, serial);
            }
        }
    }

    /// Reconcile every model window with the `Space` at once. Used after
    /// changes that can alter many windows' visibility in one go (workspace
    /// switch, config reload).
    pub fn sync_space(&mut self) {
        let ids: Vec<WindowId> = self.window_manager.windows().map(|w| w.id).collect();
        for id in ids {
            self.sync_window_to_space(id);
        }
    }

    /// Ask window `id` to close.
    ///
    /// For a real client this is `xdg_toplevel.close`, and the model entry
    /// stays until the client actually destroys its toplevel (which lands in
    /// `toplevel_destroyed` and drives `remove_window`) -- previously the
    /// model row was dropped immediately while the client kept running and
    /// rendering forever, *and* `surface_to_window` kept a stale entry that
    /// made the eventual real `toplevel_destroyed` a silent no-op. A model
    /// window with no backing surface (tests, and any future non-Wayland
    /// source) is removed synchronously, since nothing will ever send a
    /// destroy for it.
    pub fn request_close(&mut self, id: WindowId) {
        if let Some(toplevel) = self.element_for(id).and_then(|e| e.toplevel().cloned()) {
            toplevel.send_close();
            return;
        }
        self.surface_to_window.retain(|_, wid| *wid != id);
        self.elements.remove(&id);
        self.forget_window(id);
        self.emit_pending();
    }

    /// Drop every trace of `id` from the model and its side tables, then
    /// hand focus to whatever is left on the active workspace (review
    /// finding I6's coherence requirement, applied to closes as well as
    /// moves: an action right after a close must not no-op on a dangling
    /// focus pointer).
    fn forget_window(&mut self, id: WindowId) {
        self.window_manager.remove_window(id);
        self.fullscreen_saved_geometry.remove(&id);
        self.snap_saved_geometry.remove(&id);
        self.maximized_saved_geometry.remove(&id);
        if self.window_manager.focused_window().is_none() {
            let active = self.window_manager.active_workspace();
            self.window_manager.focus_mru_in_workspace(active);
        }
    }

    /// Toggle fullscreen state for a window. When entering fullscreen, saves the
    /// current geometry and sets geometry to the output rect. When exiting, restores
    /// the saved geometry. Emits WindowUpdated event.
    pub fn toggle_fullscreen(&mut self, id: WindowId) -> Option<()> {
        let w = self.window_manager.get(id)?;
        let target = !w.fullscreen;
        self.set_fullscreen_target(id, target)
    }

    /// Set fullscreen to an explicit `target` value, saving/restoring
    /// geometry exactly like `toggle_fullscreen` (which is now a thin
    /// `target = !current` wrapper around this). Added for task 12's
    /// `DbCommand::Fullscreen(id, toggle)` D-Bus command, whose `toggle`
    /// argument (despite the name -- it's the interface method's parameter
    /// name from the brief) is an explicit target state, not a flip
    /// request; a `FullscreenWindow(id, true)` call on an
    /// already-fullscreen window must stay a no-op rather than treating the
    /// current geometry as a fresh "pre-fullscreen" save point and
    /// clobbering the real one.
    pub fn set_fullscreen_target(&mut self, id: WindowId, target: bool) -> Option<()> {
        let w = self.window_manager.get(id)?;
        if w.fullscreen == target {
            return Some(());
        }
        let output_geo = self.outputs.values().next().map(|o| o.geometry)?;
        self.window_manager.set_fullscreen(id, target)?;
        if target {
            // Save current geometry before entering fullscreen
            if let Some(w) = self.window_manager.get(id) {
                self.fullscreen_saved_geometry.insert(id, w.geometry);
            }
            self.window_manager.set_geometry(id, output_geo)?;
        } else {
            // Restore saved geometry when exiting fullscreen
            let saved = self.fullscreen_saved_geometry.remove(&id)?;
            self.window_manager.set_geometry(id, saved)?;
        }
        self.sync_window_to_space(id);
        self.emit_pending();
        Some(())
    }

    /// Toggle maximized state for `id`.
    pub fn toggle_maximized(&mut self, id: WindowId) -> Option<()> {
        let target = !self.window_manager.get(id)?.maximized;
        self.set_maximized_target(id, target)
    }

    /// Set maximized to an explicit `target`, computing and applying real
    /// geometry (the output rect inset by `snap_gap`) with its own restore
    /// slot, mirroring `set_fullscreen_target`.
    ///
    /// Review finding I5: `toggle_maximized` used to flip a flag and emit,
    /// with no geometry and no configure -- the maximize button, the
    /// `MaximizeWindow` D-Bus method, and the client's own
    /// `xdg_toplevel.set_maximized` were all visual no-ops.
    pub fn set_maximized_target(&mut self, id: WindowId, target: bool) -> Option<()> {
        if self.window_manager.get(id)?.maximized == target {
            return Some(());
        }
        let output_geo = self.outputs.values().next().map(|o| o.geometry)?;
        self.window_manager.set_maximized(id, target)?;
        if target {
            let current = self.window_manager.get(id)?.geometry;
            self.maximized_saved_geometry.insert(id, current);
            let gap = self.config.appearance.snap_gap;
            self.window_manager.set_geometry(id, layout::maximized_geometry(output_geo, gap))?;
        } else if let Some(saved) = self.maximized_saved_geometry.remove(&id) {
            self.window_manager.set_geometry(id, saved)?;
        }
        self.sync_window_to_space(id);
        self.emit_pending();
        Some(())
    }

    /// Synchronously reload the config from `self.config_path` (falling
    /// back to `icedtea_config::default_db_path()` when unset) and apply it.
    /// Returns the events `apply_config` produced (which already includes a
    /// trailing `Event::ConfigReloaded` -- see that method's doc -- so
    /// nothing is pushed again here).
    ///
    /// This is the *synchronous* load+apply path -- both the load and the
    /// apply happen on whatever thread calls it, in one blocking call. It
    /// exists for tests (task-13's Step-1 test calls it against a temp DB,
    /// where the extra thread hop of the async path would just be noise)
    /// and is *not* wired to either live reload trigger: per the module's
    /// threading requirement, the redb I/O + JSON parse must never run on
    /// the render loop, so neither `handle_command`'s `ReloadConfig` arm nor
    /// `apply_action`'s `"reload"` arm calls this directly -- they both go
    /// through `spawn_config_reload`'s worker-thread + channel path instead,
    /// which lands on `apply_reloaded_config` (the same apply+emit tail this
    /// method also uses) once the load finishes off-loop.
    ///
    /// NOTE (brief deviation): the brief's Step-3 sample pushed a *second*
    /// `Event::ConfigReloaded(self.config.appearance.clone())` after calling
    /// `apply_config`. `apply_config` (task 11) already appends its own
    /// `Event::ConfigReloaded(cfg.appearance.clone())` to the vec it
    /// returns, so following the sample literally would emit the signal
    /// twice per reload -- a genuine duplicate-event bug, not just
    /// transcription noise (the standing human ruling is to fix genuine
    /// sample bugs and document them here). Dropped; `apply_config`'s event
    /// is the one and only `ConfigReloaded` this path emits.
    pub fn reload_config_from_disk(&mut self) -> Vec<Event> {
        let path = self.config_path.clone().unwrap_or_else(icedtea_config::default_db_path);
        let cfg = icedtea_config::load_or_default(&path);
        self.apply_reloaded_config(cfg)
    }

    /// Apply a `Config` (already loaded, whether synchronously by
    /// `reload_config_from_disk` or on a worker thread by
    /// `handle_command`'s `ReloadConfig` async path): calls `apply_config`,
    /// pushes its events onto the ordinary seq-tagged event queue, and flushes them
    /// immediately (via `emit_pending`) so the D-Bus emitter thread
    /// observes `ConfigReloaded` (and any `WindowClosed`/`WorkspaceList`/
    /// terminal `AltTabState` it carries) right away rather than waiting for
    /// some unrelated later mutation to flush the queue. Returns the same
    /// events, for callers (like `reload_config_from_disk`'s test) that want
    /// to inspect what was produced.
    pub fn apply_reloaded_config(&mut self, cfg: Config) -> Vec<Event> {
        let events = self.apply_config(cfg);
        // Review finding I2: these used to be extended onto a separate
        // `pending_config_events` vec that bypassed the sequence counter
        // entirely, so the reload's events went out with no seq of their
        // own. `apply_config` has already replaced `window_manager` (whose
        // `seq` is floored at the pre-reload high-water mark and whose
        // pending queue is empty) by the time this runs, so pushing them
        // through the ordinary queue both preserves their order and gives
        // each one a real, monotonically increasing seq.
        for ev in &events {
            self.window_manager.push_event(ev.clone());
        }
        self.emit_pending();
        events
    }

    /// Spawn the off-loop worker thread shared by both async reload
    /// triggers -- `handle_command`'s `DbCommand::ReloadConfig` arm and
    /// `apply_action`'s `"reload"` arm (bound to `SUPER+SHIFT+r` by
    /// default). Loads `self.config_path` (falling back to
    /// `icedtea_config::default_db_path()`) on the worker thread and ships
    /// the result back over `config_reload_tx`; `main.rs`'s calloop source
    /// for that channel calls `apply_reloaded_config` once it arrives. A
    /// no-op if `config_reload_tx` hasn't been wired yet (only possible
    /// before `main.rs` finishes event-loop setup).
    fn spawn_config_reload(&self) {
        let Some(tx) = self.config_reload_tx.clone() else { return };
        let path = self.config_path.clone().unwrap_or_else(icedtea_config::default_db_path);
        std::thread::spawn(move || {
            let cfg = icedtea_config::load_or_default(&path);
            let _ = tx.send(cfg);
        });
    }

    /// Central dispatcher for every [`crate::dbus::DbCommand`] received from
    /// the D-Bus service thread (see `dbus.rs`'s module doc for the
    /// threading model). Mirrors `apply_action`'s shape: `?`-propagates a
    /// failed precondition as `None` (skipping the trailing
    /// `emit_pending()`, which would be a no-op anyway since nothing queued
    /// on a failed mutation), `Some(())` on success.
    ///
    /// `ReloadConfig` is the one arm that doesn't mutate anything itself:
    /// per this task's threading requirement, the redb I/O + JSON parse
    /// must never block the render loop, so this spawns a worker thread
    /// (see `reload_config_from_disk`'s doc) that loads the config off-loop
    /// and ships the result back over `config_reload_tx`; `main.rs`'s
    /// calloop source for that channel is what actually calls
    /// `apply_reloaded_config` once the load finishes. If
    /// `config_reload_tx` hasn't been wired yet (only possible before
    /// `main.rs` finishes event-loop setup), this is a no-op.
    pub fn handle_command(&mut self, cmd: crate::dbus::DbCommand) -> Option<()> {
        use crate::dbus::DbCommand;
        match cmd {
            DbCommand::Focus(id) => {
                self.window_manager.focus(id)?;
                self.sync_window_to_space(id);
            }
            DbCommand::Close(id) => self.request_close(id),
            DbCommand::Minimize(id, value) => {
                self.window_manager.set_minimized(id, value)?;
                self.sync_window_to_space(id);
            }
            DbCommand::Maximize(id, value) => self.set_maximized_target(id, value)?,
            DbCommand::Fullscreen(id, value) => self.set_fullscreen_target(id, value)?,
            DbCommand::SetWorkspace(id) => {
                self.switch_workspace(id)?;
            }
            DbCommand::MoveToWorkspace(id, workspace) => self.move_to_workspace(id, workspace)?,
            DbCommand::GetState(reply_tx) => {
                let _ = reply_tx.send(self.window_manager.snapshot());
                // No model mutation happened; nothing new to flush. Return
                // early so the unconditional `emit_pending()` below (a
                // no-op here, but let's not rely on that) stays meaningful
                // for every other arm.
                return Some(());
            }
            DbCommand::ReloadConfig => {
                self.spawn_config_reload();
                return Some(());
            }
            DbCommand::Quit => self.quitting = true,
        }
        self.emit_pending();
        Some(())
    }

    /// Switch the active workspace to `workspace`, give it a focused window
    /// if it has a focusable one, and reconcile the `Space` so only its
    /// windows are mapped (review findings I1 and I6).
    pub fn switch_workspace(&mut self, workspace: u32) -> Option<()> {
        if !self.window_manager.set_active_workspace(workspace) {
            return None;
        }
        if self.window_manager.focused_window().is_none() {
            self.window_manager.focus_mru_in_workspace(workspace);
        }
        self.sync_space();
        Some(())
    }

    /// Move `id` to `workspace`, switch to it, and focus the moved window
    /// there -- leaving the origin workspace focused on whatever it has left.
    ///
    /// Review finding I6: `set_workspace` cleared the origin's focus pointer
    /// and never set the destination's, so after `MoveToWorkspace` the
    /// active workspace had no focused window and the very next
    /// `close`/`fullscreen`/`snap` action silently no-opped.
    pub fn move_to_workspace(&mut self, id: WindowId, workspace: u32) -> Option<()> {
        let origin = self.window_manager.get(id)?.workspace;
        self.window_manager.set_workspace(id, workspace)?;
        if origin != workspace {
            self.window_manager.focus_mru_in_workspace(origin);
        }
        if !self.window_manager.set_active_workspace(workspace) {
            return None;
        }
        self.window_manager.focus(id);
        self.sync_space();
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
                // Asks the client to close (and lets its own destroy drive
                // the model removal) rather than dropping the model row out
                // from under a still-running client -- see `request_close`.
                self.request_close(id);
            }
            DecorationAction::Maximize => {
                let _ = self.toggle_maximized(id);
            }
            DecorationAction::Minimize => {
                let _ = self.window_manager.set_minimized(id, true);
                if self.window_manager.focused_window().map(|w| w.id) == Some(id) {
                    let active = self.window_manager.active_workspace();
                    self.window_manager.focus_mru_in_workspace(active);
                }
                self.sync_window_to_space(id);
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
                self.request_close(id);
            }
            "maximize" => {
                let id = self.window_manager.focused_window()?.id;
                self.toggle_maximized(id)?;
            }
            "fullscreen" => {
                let id = self.window_manager.focused_window()?.id;
                self.toggle_fullscreen(id)?;
            }
            "quit" => self.quitting = true,
            // Task 13 threading requirement (binding, carried over from the
            // task-7 threading-model ruling): the redb I/O + JSON parse must
            // never block the render loop, for the keybinding-triggered
            // reload just as much as the D-Bus `ReloadConfig` method --
            // `handle_command`'s `ReloadConfig` arm and this arm share the
            // exact same worker-thread dispatch for that reason.
            "reload" => self.spawn_config_reload(),
            // NOTE (brief deviation): the sample matched on the literal
            // `"cycle:alt_tab"` here, but `action.splitn(2, ':')` above
            // already split that string into `base = "cycle"`, `arg =
            // Some("alt_tab")` -- a match on `base` can never see the colon,
            // so as written this arm was as unreachable as the
            // `"snap:restore"` arm noted above. Matching `"cycle"` and
            // checking `arg` makes it reachable (and leaves room for other
            // `cycle:*` variants later without another silent dead arm).
            //
            // Task 11 review #1: a session's entry list is now captured
            // once by `AltTabMachine::start` and kept stable for the whole
            // session -- steps read it back via `self.alt_tab.entries()`
            // instead of recomputing `alt_tab_entries()` (and therefore a
            // possibly different length/order) on every keypress. The
            // session itself is ended by `end_alt_tab` (see its doc for the
            // chosen end condition), not here.
            "cycle" => {
                if arg.as_deref() != Some("alt_tab") {
                    return None;
                }
                if !self.alt_tab.is_active() {
                    let entries = self.window_manager.alt_tab_entries();
                    if entries.is_empty() {
                        return None;
                    }
                    self.alt_tab.start(entries);
                } else {
                    self.alt_tab.step(true);
                }
                let idx = self.alt_tab.index();
                let entries = self.alt_tab.entries().to_vec();
                if let Some(wid) = entries.get(idx).copied() {
                    self.window_manager.focus(wid);
                    self.sync_window_to_space(wid);
                }
                self.emit(Event::AltTabState(AltTabState { active: true, entries, index: idx }));
            }
            "spawn" => {
                let cmd = arg?;
                std::process::Command::new("sh").arg("-c").arg(&cmd).spawn().ok();
            }
            // Task 11 review #4: `n: u32` from an externally-parseable
            // action string (this dispatcher's own doc says it's also
            // reachable "from a D-Bus-triggered action") minus 1 is unsigned
            // subtraction -- `"workspace:0"` panicked the whole compositor
            // in debug builds (and silently wrapped to `u32::MAX` in
            // release). `checked_sub` turns that into a clean `None`
            // instead. Separately, `set_active_workspace` returns `bool`
            // ("does this workspace exist") and was being discarded, so an
            // out-of-range `"workspace:7"` against the 4-workspace default
            // used to report success (`Some(())`) having silently done
            // nothing; it's now propagated as a real failure.
            "workspace" => {
                let n: u32 = arg?.parse().ok()?;
                let idx = n.checked_sub(1)?;
                self.switch_workspace(idx)?;
            }
            "move_to_workspace" => {
                let id = self.window_manager.focused_window()?.id;
                let n: u32 = arg?.parse().ok()?;
                let idx = n.checked_sub(1)?;
                self.move_to_workspace(id, idx)?;
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

    /// End an in-progress alt-tab session, if one is active: marks the
    /// machine inactive and emits the terminal `AltTabState { active: false,
    /// .. }` so the shell's overlay (the `contract::Event` consumer) can
    /// dismiss itself. A no-op (no event emitted) if no session is active,
    /// so callers can call this unconditionally.
    ///
    /// Task 11 review #1: nothing called `AltTabMachine::end()` anywhere in
    /// the compositor, so a session never terminated -- `is_active()` stayed
    /// `true` forever after the first `SUPER+Tab` and the shell's overlay
    /// had no way to ever be told to close. The brief/spec don't name an
    /// explicit end mechanism, so the one real WMs use was picked: alt-tab
    /// ends when the held modifier (SUPER, per the default `cycle:alt_tab`
    /// binding) is released. `backend.rs`'s keyboard filter calls this on
    /// every key event where the current modifier state no longer includes
    /// SUPER, which covers both "released the modifier while still holding
    /// Tab" and "released Tab first, then the modifier."
    pub fn end_alt_tab(&mut self) {
        if !self.alt_tab.is_active() {
            return;
        }
        let entries = self.alt_tab.entries().to_vec();
        let idx = self.alt_tab.index();
        self.alt_tab.end();
        self.emit(Event::AltTabState(AltTabState { active: false, entries, index: idx }));
        self.emit_pending();
    }

    /// Snap `id` to `zone` on the (first) output, saving its pre-snap
    /// geometry so `snap_restore` can undo it.
    /// Snap `id` to `zone` on the (first) output, saving its pre-snap
    /// geometry so `snap_restore` can undo it. A no-op returning `None` when
    /// `behavior.snap_enabled` is off (review finding I3: the flag had no
    /// consumers, so a user who disabled snapping still got snapping from
    /// both the `snap:*` actions and the drag machine).
    pub fn snap(&mut self, id: WindowId, zone: SnapZone) -> Option<()> {
        if !self.config.behavior.snap_enabled {
            return None;
        }
        let output_geo = self.outputs.values().next().map(|o| o.geometry)?;
        let gap = self.config.appearance.snap_gap;
        let current = self.window_manager.get(id)?.geometry;
        self.snap_saved_geometry.entry(id).or_insert(current);
        self.window_manager.set_geometry(id, layout::snapped_geometry(output_geo, zone, gap))?;
        self.sync_window_to_space(id);
        Some(())
    }

    /// Restore `id`'s geometry as it was before its most recent `snap`, if
    /// any was saved. Deliberately *not* gated on `snap_enabled`: a window
    /// snapped before the flag was turned off must still be restorable.
    pub fn snap_restore(&mut self, id: WindowId) -> Option<()> {
        if let Some(orig) = self.snap_saved_geometry.remove(&id) {
            self.window_manager.set_geometry(id, orig)?;
            self.sync_window_to_space(id);
        }
        Some(())
    }

    /// Rebuild `window_manager`'s workspaces from `cfg.workspace_names` and
    /// swap in the new config (keybindings/appearance/behavior). Returns the
    /// events the caller should queue (via `apply_reloaded_config`) since
    /// the just-replaced `window_manager` can't carry them.
    ///
    /// Task 11 review #3: this used to replace `window_manager` with a
    /// brand-new, empty one and return only `WorkspaceList`/`ConfigReloaded`
    /// -- every live window vanished from the model with zero
    /// `WindowClosed` events (a direct violation of "every state change
    /// emits"; the shell kept showing windows the compositor no longer
    /// tracked), `surface_to_window` kept pointing at now-nonexistent ids
    /// (so a later `toplevel_destroyed` for one of those surfaces would
    /// silently no-op instead of cleaning up), and the fresh
    /// `WindowManager`'s id counter reset to 1, so windows mapped after the
    /// reload could collide with ids the shell might still remember as
    /// live. Fixed here: emit `WindowClosed` for every window that's about
    /// to disappear, unmap their still-mapped `wl_surface`s from `space` and
    /// clear `surface_to_window`, and floor the new `WindowManager`'s id
    /// counter at the old one's high-water mark so no id is ever reissued.
    /// The plan's actual invariant (workspaces rebuilt from config) is
    /// preserved unchanged; only the "silently destroy live state" part of
    /// the sample was a bug.
    ///
    /// Task 11 re-review, same reload seam: also resets `drag`, `alt_tab`,
    /// and `snap_preview` (all of which can be referencing a window id
    /// that's about to stop existing -- a reload mid-drag would otherwise
    /// leave a permanently rendered stale `snap_preview`, and a reload
    /// mid-alt-tab would leave `alt_tab.is_active()` true forever with a
    /// dead entry list), and floors the fresh `WindowManager`'s `seq`
    /// counter the same way `next_id` is floored, so `snapshot().seq`
    /// cannot go backwards across a reload.
    pub fn apply_config(&mut self, cfg: Config) -> Vec<Event> {
        let mut events: Vec<Event> =
            self.window_manager.windows().map(|w| Event::WindowClosed(w.id)).collect();

        for window in self.elements.values().cloned().collect::<Vec<_>>() {
            self.space.unmap_elem(&window);
        }
        self.elements.clear();
        self.surface_to_window.clear();
        // Stale restore points would otherwise reference ids that can never
        // come back (the floor below guarantees that).
        self.fullscreen_saved_geometry.clear();
        self.snap_saved_geometry.clear();
        self.maximized_saved_geometry.clear();
        // Drop any in-flight interaction that referenced the about-to-vanish
        // windows.
        self.drag = input::DragMachine::new();
        self.resize = input::ResizeMachine::new();
        // Task 11 re-review round 3 #1: resetting `alt_tab` (below) is a
        // state change, not an event -- a reload mid-cycle used to leave
        // whatever shell overlay is rendered from the session's last
        // `AltTabState{active: true, ..}` with no dismiss signal at all.
        // The terminal event is appended to `events` here (rather than
        // calling `end_alt_tab()`, which also calls `self.emit_pending()`
        // itself) so it drains alongside -- in the same order as -- the
        // `WindowClosed`/`WorkspaceList`/`ConfigReloaded` events this same
        // call produces, instead of jumping the queue as a side effect.
        if self.alt_tab.is_active() {
            events.push(Event::AltTabState(AltTabState {
                active: false,
                entries: self.alt_tab.entries().to_vec(),
                index: self.alt_tab.index(),
            }));
        }
        self.alt_tab = input::AltTabMachine::new();
        self.snap_preview = None;

        let next_id_floor = self.window_manager.next_id();
        let seq_floor = self.window_manager.seq();
        // Review finding I4: the reload is the other place a binding can
        // become unusable, so it's the other place to say so -- once.
        input::warn_about_keybindings(&cfg.keybindings);
        self.config = cfg.clone();
        self.window_manager = WindowManager::new(cfg.workspace_names.clone());
        self.window_manager.raise_id_floor(next_id_floor);
        self.window_manager.raise_seq_floor(seq_floor);

        events.push(Event::WorkspaceList(self.window_manager.workspace_info()));
        events.push(Event::ConfigReloaded(cfg.appearance.clone()));
        events
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
    /// window `id`: focuses the window (click-to-focus), then, if the press
    /// hits the title bar's move area, begins a drag; otherwise applies
    /// whatever discrete decoration action (close/maximize/minimize) was
    /// hit.
    ///
    /// Task 11 re-review #2: this used to skip straight to
    /// `decoration_action_for`, which gates on `w.focused` -- so pressing
    /// an unfocused window's title bar or buttons did nothing at all (no
    /// focus change, no drag, no click action) since the gate always failed
    /// on the first click. Focusing first (unconditionally; `focus` on an
    /// already-focused window is a no-op) makes the very click that should
    /// raise a window also be the click that acts on it, matching ordinary
    /// click-to-focus window manager behavior.
    fn handle_pointer_press(&mut self, id: WindowId, pointer: (i32, i32)) -> Option<()> {
        self.window_manager.focus(id)?;
        // Focus changes the toplevel's `Activated` state and the `Space`
        // element's activation, so it has to reach the client too (C1).
        self.sync_window_to_space(id);
        // Task 11 re-review round 3 #2: flush right after `focus()`
        // mutates, before any of the `?`-early-returns below (e.g.
        // `decoration_action_for` returning `None` for a fullscreen
        // window) can skip the trailing `emit_pending()` call at the end
        // of this function and leave the focus event sitting queued until
        // some unrelated later flush.
        self.emit_pending();
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
        self.emit_pending();
        Some(())
    }

    /// Pointer motion at `pointer` (output logical coordinates) during an
    /// in-progress drag: updates the snap-zone preview (rendering consumes
    /// `self.snap_preview`).
    fn handle_pointer_motion(&mut self, pointer: (i32, i32)) -> Option<()> {
        // An interactive resize (started by the client's `resize_request`)
        // takes precedence: it owns the pointer until the button is
        // released.
        if let Some(id) = self.resize.window_id() {
            let geometry = self.resize.geometry_for(pointer)?;
            self.window_manager.set_geometry(id, geometry)?;
            self.sync_window_to_space(id);
            self.emit_pending();
            return Some(());
        }
        self.drag.window_id()?;
        let output_geo = self.outputs.values().next().map(|o| o.geometry)?;
        let threshold = self.config.appearance.snap_gap.max(1) * 4;
        // Review finding I3: with snapping disabled the drag machine is
        // never told about zones at all, so no preview is staged *and*
        // `handle_pointer_release` can only ever produce a plain move.
        if !self.config.behavior.snap_enabled {
            self.snap_preview = None;
            return Some(());
        }
        self.drag.motion(pointer, output_geo, threshold);
        self.snap_preview = self
            .drag
            .preview_zone()
            .map(|zone| layout::snapped_geometry(output_geo, zone, self.config.appearance.snap_gap));
        Some(())
    }

    /// Pointer button release at `pointer` (output logical coordinates):
    /// ends the drag, either snapping the window to the previewed zone or
    /// moving it to `pointer - grab_offset`.
    fn handle_pointer_release(&mut self, pointer: (i32, i32)) -> Option<()> {
        if self.resize.window_id().is_some() {
            let final_geometry = self.resize.geometry_for(pointer);
            let id = self.resize.end()?;
            if let Some(geometry) = final_geometry {
                self.window_manager.set_geometry(id, geometry)?;
            }
            self.sync_window_to_space(id);
            self.emit_pending();
            return Some(());
        }
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
                self.sync_window_to_space(id);
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
            // Looked up through `elements` rather than `space.elements()`:
            // a window on an inactive workspace is unmapped from the
            // `Space` (review finding I1) but must still process commits.
            if let Some(window) = self.surface_to_window.get(&root).and_then(|id| self.elements.get(id)) {
                window.on_commit();
            }
        }

        // Send the initial xdg_toplevel configure once a client commits after mapping.
        if let Some(window) =
            self.surface_to_window.get(surface).and_then(|id| self.elements.get(id)).cloned()
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
        let occupied: Vec<icedtea_contract::Rectangle> = self
            .window_manager
            .windows_in_workspace(self.window_manager.active_workspace())
            .iter()
            .map(|w| w.geometry)
            .collect();
        let output_geo = self
            .outputs
            .values()
            .next()
            .map(|o| o.geometry)
            .unwrap_or(icedtea_contract::Rectangle { x: 0, y: 0, width: 1920, height: 1080 });
        // Review finding M7: cascade positions now wrap inside the output
        // (and count only this workspace's windows) so the Nth window can't
        // open off-screen with no title bar to grab.
        let (x, y) = layout::cascade_point_in(&occupied, (640, 400), 24, output_geo);
        let geometry = icedtea_contract::Rectangle { x, y, width: 640, height: 400 };
        let id = self.window_manager.add_window(&app_id, &title, pid, geometry);
        self.surface_to_window.insert(surface.wl_surface().clone(), id);

        let window = Window::new_wayland_window(surface);
        // Map at the model's geometry, not (0, 0) -- see `sync_window_to_space`.
        self.space.map_element(window.clone(), (geometry.x, geometry.y), true);
        self.elements.insert(id, window);
        self.sync_window_to_space(id);
        self.emit_pending();
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        let removed = self.surface_to_window.remove(surface.wl_surface());
        if let Some(window) = removed.and_then(|id| self.elements.remove(&id)) {
            self.space.unmap_elem(&window);
        }
        if let Some(id) = removed {
            self.forget_window(id);
            self.emit_pending();
        }
    }

    // --- Client-driven window management (ledger item 15, part of C1) ---
    //
    // These five were `XdgShellHandler`'s default no-op bodies, so a client
    // pressing its own maximize button, or dragging its own CSD title bar,
    // did nothing at all. Each now routes into exactly the same `State`
    // method the keybinding and D-Bus paths use, so there is one
    // implementation of "maximize a window" and it always ends in a
    // configure.

    fn maximize_request(&mut self, surface: ToplevelSurface) {
        if let Some(&id) = self.surface_to_window.get(surface.wl_surface()) {
            self.set_maximized_target(id, true);
        } else {
            surface.send_configure();
        }
    }

    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
        if let Some(&id) = self.surface_to_window.get(surface.wl_surface()) {
            self.set_maximized_target(id, false);
        }
    }

    fn fullscreen_request(&mut self, surface: ToplevelSurface, _output: Option<WlOutput>) {
        if let Some(&id) = self.surface_to_window.get(surface.wl_surface()) {
            self.set_fullscreen_target(id, true);
        } else {
            surface.send_configure();
        }
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        if let Some(&id) = self.surface_to_window.get(surface.wl_surface()) {
            self.set_fullscreen_target(id, false);
        }
    }

    fn minimize_request(&mut self, surface: ToplevelSurface) {
        if let Some(&id) = self.surface_to_window.get(surface.wl_surface()) {
            self.apply_decoration_action(id, crate::decoration::DecorationAction::Minimize);
        }
    }

    fn move_request(&mut self, surface: ToplevelSurface, _seat: WlSeat, _serial: Serial) {
        let Some(&id) = self.surface_to_window.get(surface.wl_surface()) else { return };
        let Some(geo) = self.window_manager.get(id).map(|w| w.geometry) else { return };
        let pointer = self.pointer_location;
        // The client already holds the button down, so the ordinary
        // motion/release path takes it from here.
        self.drag.begin(id, (pointer.0 - geo.x, pointer.1 - geo.y));
    }

    fn resize_request(&mut self, surface: ToplevelSurface, _seat: WlSeat, _serial: Serial, edges: XdgResizeEdge) {
        let Some(&id) = self.surface_to_window.get(surface.wl_surface()) else { return };
        let Some(geo) = self.window_manager.get(id).map(|w| w.geometry) else { return };
        let edges = resize_edges_from_xdg(edges);
        if edges.is_empty() {
            return;
        }
        self.resize.begin(id, edges, geo, self.pointer_location);
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

/// Translate xdg-shell's `resize_edge` enum into the compositor's own
/// [`input::ResizeEdges`] set. `None` (and any unknown future value) yields
/// an empty set, which `resize_request` treats as "nothing to resize".
fn resize_edges_from_xdg(edges: XdgResizeEdge) -> input::ResizeEdges {
    use input::ResizeEdges;
    match edges {
        XdgResizeEdge::Top => ResizeEdges { top: true, ..Default::default() },
        XdgResizeEdge::Bottom => ResizeEdges { bottom: true, ..Default::default() },
        XdgResizeEdge::Left => ResizeEdges { left: true, ..Default::default() },
        XdgResizeEdge::Right => ResizeEdges { right: true, ..Default::default() },
        XdgResizeEdge::TopLeft => ResizeEdges { top: true, left: true, ..Default::default() },
        XdgResizeEdge::TopRight => ResizeEdges { top: true, right: true, ..Default::default() },
        XdgResizeEdge::BottomLeft => ResizeEdges { bottom: true, left: true, ..Default::default() },
        XdgResizeEdge::BottomRight => ResizeEdges { bottom: true, right: true, ..Default::default() },
        _ => ResizeEdges::default(),
    }
}

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
        assert!(matches!(rx.try_recv().map(|e| e.event), Ok(Event::WindowOpened(_))));
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
    fn decoration_action_for_ignores_csd_and_still_returns_close() {
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

    // --- Review fix-round tests (task-11-review.md) ---

    /// Important #1: a session's entry list is frozen at `start()` and used
    /// for the whole session even if `window_manager` changes underneath it
    /// mid-cycle -- stepping must not panic or desync just because a window
    /// in the frozen list got removed.
    #[test]
    fn alt_tab_session_survives_churn_and_ends_cleanly() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        let a = state.window_manager.add_window("a", "a", 1, DEFAULT_GEO);
        let b = state.window_manager.add_window("b", "b", 2, DEFAULT_GEO);
        let c = state.window_manager.add_window("c", "c", 3, DEFAULT_GEO);
        // `alt_tab_entries()` is id-ordered: [a, b, c]. First cycle starts
        // the session and focuses entries[0] = a.
        state.apply_action("cycle:alt_tab").unwrap();
        assert!(state.alt_tab.is_active());
        assert!(state.window_manager.get(a).unwrap().focused);

        // Churn mid-session: a window in the frozen entry list disappears.
        state.window_manager.remove_window(b);

        // Stepping must not panic even though the stored entries still
        // reference the now-gone `b` (index 1); `WindowManager::focus`
        // quietly no-ops on a missing id instead of this call failing.
        state.apply_action("cycle:alt_tab").unwrap(); // steps to index 1 (b, gone)
        state.apply_action("cycle:alt_tab").unwrap(); // steps to index 2 (c)
        assert!(state.alt_tab.is_active());
        assert_eq!(state.alt_tab.entries().len(), 3, "entry list must stay frozen across churn");
        assert!(state.window_manager.get(c).unwrap().focused);

        // Drain events so we can inspect exactly what `end_alt_tab` sends.
        let _ = rx.try_iter().count();
        state.end_alt_tab();
        assert!(!state.alt_tab.is_active());
        let events: Vec<_> = rx.try_iter().collect();
        assert!(
            events.iter().any(|e| matches!(e.event, Event::AltTabState(AltTabState { active: false, .. }))),
            "end_alt_tab must emit a terminal AltTabState so the shell overlay can dismiss"
        );

        // A no-op call afterwards must not emit a second terminal event.
        state.end_alt_tab();
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn end_alt_tab_is_a_noop_when_no_session_is_active() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        assert!(!state.alt_tab.is_active());
        state.end_alt_tab();
        assert!(rx.try_recv().is_err(), "no session was active, nothing should be emitted");
    }

    /// Important #2: `saved_geometry` used to be a single map shared by
    /// `toggle_fullscreen` and `snap`; snapping then fullscreening a window
    /// clobbered the pre-snap restore point with the snapped geometry, and
    /// `snap_restore` after exiting fullscreen silently did nothing.
    #[test]
    fn snap_then_fullscreen_then_unfullscreen_then_snap_restore_round_trips() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        state.outputs.insert(0, OutputSurface { geometry: Rectangle { x: 0, y: 0, width: 1000, height: 800 } });
        let original = Rectangle { x: 50, y: 60, width: 300, height: 200 };
        let id = state.window_manager.add_window("app", "t", 1, original);
        state.window_manager.focus(id).unwrap();

        state.snap(id, SnapZone::Left).unwrap();
        assert_eq!(state.window_manager.get(id).unwrap().geometry.width, 1000 / 2 - 16);

        state.toggle_fullscreen(id).unwrap(); // enter fullscreen from the snapped geometry
        assert!(state.window_manager.get(id).unwrap().fullscreen);
        state.toggle_fullscreen(id).unwrap(); // exit fullscreen
        assert!(!state.window_manager.get(id).unwrap().fullscreen);
        // Exiting fullscreen must restore the *snapped* geometry, not the
        // pre-snap original -- fullscreen's own restore point is untouched
        // by snap's map.
        assert_eq!(state.window_manager.get(id).unwrap().geometry.width, 1000 / 2 - 16);

        state.snap_restore(id).unwrap();
        assert_eq!(state.window_manager.get(id).unwrap().geometry, original);
    }

    /// Important #3: `apply_config` must emit `WindowClosed` for every
    /// window it discards, clear `surface_to_window` (no direct accessor,
    /// so this asserts the externally observable state that depends on it:
    /// a `toplevel_destroyed`-style removal after reload no longer applies
    /// to any window in the new manager), and never let a post-reload
    /// `add_window` reuse an id from before the reload.
    #[test]
    fn apply_config_emits_window_closed_and_floors_id_counter() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        let a = state.window_manager.add_window("a", "a", 1, DEFAULT_GEO);
        let b = state.window_manager.add_window("b", "b", 2, DEFAULT_GEO);

        let mut cfg = icedtea_config::default_config();
        cfg.workspace_names = vec!["A".into(), "B".into()];
        let events = state.apply_config(cfg);

        let closed: Vec<WindowId> =
            events.iter().filter_map(|e| if let Event::WindowClosed(id) = e { Some(*id) } else { None }).collect();
        assert_eq!(closed.len(), 2, "one WindowClosed per pre-reload window");
        assert!(closed.contains(&a));
        assert!(closed.contains(&b));
        assert!(events.iter().any(|e| matches!(e, Event::WorkspaceList(_))));
        assert!(events.iter().any(|e| matches!(e, Event::ConfigReloaded(_))));

        // The old windows are gone from the (now-empty) manager.
        assert!(state.window_manager.get(a).is_none());
        assert!(state.window_manager.get(b).is_none());

        // A window mapped after the reload must never collide with a
        // pre-reload id the shell might still remember as live.
        let c = state.window_manager.add_window("c", "c", 3, DEFAULT_GEO);
        assert!(c.0 > a.0 && c.0 > b.0);
    }

    /// Important #4: `n - 1` on a `u32` action argument must never panic,
    /// and an out-of-range workspace index must be a reported failure, not
    /// a silent no-op success.
    #[test]
    fn workspace_zero_does_not_panic_and_fails_cleanly() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        assert_eq!(state.apply_action("workspace:0"), None);
        assert_eq!(state.window_manager.active_workspace(), 0, "a failed switch must not move the active workspace");
    }

    #[test]
    fn workspace_out_of_range_fails_instead_of_reporting_success() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        // Default config has 4 workspaces (config/src/defaults.rs); 99 is
        // well out of range.
        assert_eq!(state.apply_action("workspace:99"), None);
        assert_eq!(state.window_manager.active_workspace(), 0);
    }

    #[test]
    fn move_to_workspace_zero_does_not_panic_and_fails_cleanly() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        state.window_manager.add_window("app", "t", 1, DEFAULT_GEO);
        assert_eq!(state.apply_action("move_to_workspace:0"), None);
    }

    // --- Re-review fix-round tests (task-11-review.md round 2) ---

    /// Important #2: pressing an unfocused window must focus it, and that
    /// focus must take effect *before* the decoration action for the same
    /// click is evaluated (so the click that raises a window can also act
    /// on it in one motion, matching ordinary click-to-focus behavior).
    #[test]
    fn press_on_unfocused_window_focuses_then_hits_its_decorations() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        let a_geo = Rectangle { x: 0, y: 0, width: 640, height: 400 };
        let a = state.window_manager.add_window("a", "a", 1, a_geo);
        let b = state.window_manager.add_window("b", "b", 2, Rectangle { x: 700, y: 0, width: 640, height: 400 });
        // `b` was added last, so it's focused; `a` is not.
        assert!(!state.window_manager.get(a).unwrap().focused);
        assert!(state.window_manager.get(b).unwrap().focused);

        // Press on `a`'s title bar (not a button): before the fix,
        // `decoration_action_for` would gate on `a.focused` (false) and
        // this whole call would return `None`, doing nothing.
        state.handle_pointer(PointerEvent::Press { id: a, pointer: (20, 5) }).unwrap();
        assert!(state.window_manager.get(a).unwrap().focused);
        assert!(!state.window_manager.get(b).unwrap().focused);

        // Now that `a` is focused, a press on its close button must close
        // it -- proving the *same* click sequence both focuses and acts.
        let close_pt = (a_geo.x + a_geo.width - 5, a_geo.y + 5);
        state.handle_pointer(PointerEvent::Press { id: a, pointer: close_pt }).unwrap();
        assert!(state.window_manager.get(a).is_none());
    }

    /// Important #3 (adjacent staleness bugs on the reload seam): a reload
    /// mid-drag/mid-alt-tab must not leave `drag`/`alt_tab`/`snap_preview`
    /// referencing windows that `apply_config` is about to discard.
    #[test]
    fn apply_config_resets_drag_alt_tab_and_snap_preview() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        state.outputs.insert(0, OutputSurface { geometry: Rectangle { x: 0, y: 0, width: 1000, height: 800 } });
        let a = state.window_manager.add_window("a", "a", 1, DEFAULT_GEO);
        let b = state.window_manager.add_window("b", "b", 2, DEFAULT_GEO);

        state.drag.begin(a, (5, 5));
        state.alt_tab.start(vec![a, b]);
        state.snap_preview = Some(Rectangle { x: 0, y: 0, width: 500, height: 800 });
        assert!(state.drag.window_id().is_some());
        assert!(state.alt_tab.is_active());
        assert!(state.snap_preview.is_some());

        let _ = state.apply_config(icedtea_config::default_config());

        assert!(state.drag.window_id().is_none(), "drag must not survive a reload mid-drag");
        assert!(!state.alt_tab.is_active(), "alt-tab session must not survive a reload mid-cycle");
        assert!(state.snap_preview.is_none(), "a stale snap preview must not render forever after reload");
    }

    /// Important #4: `snapshot().seq` must never go backwards across a
    /// reload, and `State::emit` (used for `AltTabState`) must advance the
    /// same counter as every other event.
    #[test]
    fn apply_config_does_not_regress_seq() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        state.window_manager.add_window("a", "a", 1, DEFAULT_GEO);
        state.window_manager.add_window("b", "b", 2, DEFAULT_GEO);
        let seq_before = state.window_manager.snapshot().seq;
        assert!(seq_before > 0);

        let _ = state.apply_config(icedtea_config::default_config());

        assert!(
            state.window_manager.snapshot().seq >= seq_before,
            "a fresh WindowManager's seq must be floored at the pre-reload high-water mark"
        );
    }

    #[test]
    fn emit_advances_seq() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        let seq_before = state.window_manager.snapshot().seq;
        state.emit(Event::AltTabState(AltTabState { active: true, entries: vec![], index: 0 }));
        assert!(
            state.window_manager.snapshot().seq > seq_before,
            "State::emit must bump seq like every other pending-event producer"
        );
    }

    // --- Re-review fix-round-3 tests (task-11-review.md round 3) ---

    /// Important #1: a reload mid-alt-tab-cycle must emit the terminal
    /// `AltTabState{active: false, ..}` so a shell overlay rendered from
    /// the session's last `active: true` event has a dismiss signal,
    /// instead of `apply_config` just silently resetting the machine.
    #[test]
    fn apply_config_mid_cycle_emits_alt_tab_inactive() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        state.window_manager.add_window("a", "a", 1, DEFAULT_GEO);
        state.window_manager.add_window("b", "b", 2, DEFAULT_GEO);
        state.apply_action("cycle:alt_tab").unwrap();
        assert!(state.alt_tab.is_active());

        let events = state.apply_config(icedtea_config::default_config());

        assert!(
            events.iter().any(|e| matches!(e, Event::AltTabState(AltTabState { active: false, .. }))),
            "reload mid-cycle must emit the terminal AltTabState"
        );
        assert!(!state.alt_tab.is_active());
    }

    #[test]
    fn apply_config_when_alt_tab_inactive_emits_no_extra_alt_tab_state() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        assert!(!state.alt_tab.is_active());
        let events = state.apply_config(icedtea_config::default_config());
        assert!(!events.iter().any(|e| matches!(e, Event::AltTabState(_))));
    }

    /// Minor #2: `handle_pointer_press`'s `focus()` mutation must flush
    /// immediately, not get deferred by an early `?`-return further down
    /// the same function (e.g. `decoration_action_for` returning `None`
    /// for a fullscreen window).
    #[test]
    fn press_on_unfocused_fullscreen_window_flushes_focus_immediately() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        state.outputs.insert(0, OutputSurface { geometry: Rectangle { x: 0, y: 0, width: 1000, height: 800 } });
        let a = state.window_manager.add_window("a", "a", 1, DEFAULT_GEO);
        let _b = state.window_manager.add_window("b", "b", 2, DEFAULT_GEO);
        state.window_manager.set_fullscreen(a, true).unwrap();
        let _ = rx.try_iter().count(); // drain setup events
        assert!(!state.window_manager.get(a).unwrap().focused, "b was added last and is focused, not a");

        // `decoration_action_for` returns `None` for a fullscreen window,
        // so this call itself returns `None` -- but the focus mutation
        // must already be visible on the channel by the time it does.
        assert_eq!(state.handle_pointer(PointerEvent::Press { id: a, pointer: (20, 5) }), None);
        assert!(state.window_manager.get(a).unwrap().focused);
        let events: Vec<_> = rx.try_iter().collect();
        assert!(
            events
                .iter()
                .any(|e| matches!(&e.event, Event::WindowUpdated { id, update } if *id == a && update.focused == Some(true))),
            "focus event must be flushed immediately, not deferred until an unrelated later flush"
        );
    }

    // --- Task 13: config hot reload over `ReloadConfig` ---

    #[test]
    fn reload_config_applies_new_workspaces_and_emits() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut state = State::new(icedtea_config::default_config(), tx);
        // Persist a config with different workspace names, then reload from disk.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cfg.redb");
        let db = icedtea_config::open(&path).unwrap();
        let mut cfg = icedtea_config::default_config();
        cfg.workspace_names = vec!["A".into(), "B".into(), "C".into()];
        cfg.save(&db).unwrap();
        drop(db);

        state.config_path = Some(path.clone());
        let events = state.reload_config_from_disk();
        assert_eq!(state.window_manager.workspace_info().len(), 3);
        assert!(events.iter().any(|e| matches!(e, Event::ConfigReloaded(_))));
        let emitted = rx.try_iter().collect::<Vec<_>>();
        assert!(emitted.iter().any(|e| matches!(e.event, Event::ConfigReloaded(_))));
    }

    /// `handle_command`'s `ReloadConfig` arm must go through the async
    /// worker path -- it only spawns a thread and returns, never blocking
    /// on the load itself -- and must be a harmless no-op when
    /// `config_reload_tx` hasn't been wired (as in every other test in this
    /// module, which never call `set_config_reload_sender`).
    #[test]
    fn handle_command_reload_without_sender_wired_is_a_harmless_noop() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        assert_eq!(state.handle_command(crate::dbus::DbCommand::ReloadConfig), Some(()));
    }

    /// The async worker path actually delivers a reloaded config back to
    /// the channel `set_config_reload_sender` was given, and
    /// `apply_reloaded_config` applies + emits it exactly like the sync
    /// path.
    #[test]
    fn handle_command_reload_spawns_worker_that_delivers_config() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut state = State::new(icedtea_config::default_config(), tx);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cfg.redb");
        let db = icedtea_config::open(&path).unwrap();
        let mut cfg = icedtea_config::default_config();
        cfg.workspace_names = vec!["X".into(), "Y".into()];
        cfg.save(&db).unwrap();
        drop(db);
        state.config_path = Some(path);

        let (reload_tx, reload_rx) = smithay::reexports::calloop::channel::channel::<Config>();
        state.set_config_reload_sender(reload_tx);

        assert_eq!(state.handle_command(crate::dbus::DbCommand::ReloadConfig), Some(()));

        // The worker thread runs off-loop; block briefly for its result
        // (mirrors how `main.rs`'s calloop source would be woken).
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let mut received = None;
        while std::time::Instant::now() < deadline {
            match reload_rx.try_recv() {
                Ok(cfg) => {
                    received = Some(cfg);
                    break;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => std::thread::sleep(std::time::Duration::from_millis(5)),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
            }
        }
        let cfg = received.expect("worker thread must deliver a Config over the reload channel");
        assert_eq!(cfg.workspace_names, vec!["X".to_string(), "Y".to_string()]);

        state.apply_reloaded_config(cfg);
        assert_eq!(state.window_manager.workspace_info().len(), 2);
        let emitted = rx.try_iter().collect::<Vec<_>>();
        assert!(emitted.iter().any(|e| matches!(e.event, Event::ConfigReloaded(_))));
    }

    /// The keybinding-triggered `"reload"` action (`SUPER+SHIFT+r` by
    /// default) must go through the exact same async worker path as the
    /// D-Bus `ReloadConfig` command -- per the task-13 threading
    /// requirement, neither may block the render loop with redb I/O.
    #[test]
    fn apply_action_reload_spawns_worker_that_delivers_config() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(icedtea_config::default_config(), tx);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cfg.redb");
        let db = icedtea_config::open(&path).unwrap();
        let mut cfg = icedtea_config::default_config();
        cfg.workspace_names = vec!["P".into(), "Q".into()];
        cfg.save(&db).unwrap();
        drop(db);
        state.config_path = Some(path);

        let (reload_tx, reload_rx) = smithay::reexports::calloop::channel::channel::<Config>();
        state.set_config_reload_sender(reload_tx);

        assert_eq!(state.apply_action("reload"), Some(()));

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let mut received = None;
        while std::time::Instant::now() < deadline {
            match reload_rx.try_recv() {
                Ok(cfg) => {
                    received = Some(cfg);
                    break;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => std::thread::sleep(std::time::Duration::from_millis(5)),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
            }
        }
        let cfg = received.expect("worker thread must deliver a Config over the reload channel");
        assert_eq!(cfg.workspace_names, vec!["P".to_string(), "Q".to_string()]);
    }

    #[test]
    fn apply_action_reload_without_sender_wired_is_a_harmless_noop() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        assert_eq!(state.apply_action("reload"), Some(()));
    }

    // --- Final-review fix-round tests ---

    fn state_with_output(width: i32, height: i32) -> (State, crossbeam_channel::Receiver<SeqEvent>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        state.outputs.insert(0, OutputSurface { geometry: Rectangle { x: 0, y: 0, width, height } });
        (state, rx)
    }

    /// C1/I5: maximize computes and applies real geometry against its own
    /// restore slot, and toggling back returns exactly the pre-maximize
    /// geometry -- it used to flip a flag and nothing else.
    #[test]
    fn maximize_applies_output_geometry_and_restores_it() {
        let (mut state, _rx) = state_with_output(1000, 800);
        let original = Rectangle { x: 40, y: 50, width: 300, height: 200 };
        let id = state.window_manager.add_window("app", "t", 1, original);

        state.toggle_maximized(id).unwrap();
        let w = state.window_manager.get(id).unwrap();
        assert!(w.maximized);
        assert_eq!(w.geometry, layout::maximized_geometry(Rectangle { x: 0, y: 0, width: 1000, height: 800 }, 8));

        state.toggle_maximized(id).unwrap();
        let w = state.window_manager.get(id).unwrap();
        assert!(!w.maximized);
        assert_eq!(w.geometry, original);
    }

    /// I5: maximize's restore slot is independent of snap's, exactly like
    /// fullscreen's -- snapping between maximize and unmaximize must not
    /// clobber either restore point.
    #[test]
    fn maximize_and_snap_keep_independent_restore_points() {
        let (mut state, _rx) = state_with_output(1000, 800);
        let original = Rectangle { x: 40, y: 50, width: 300, height: 200 };
        let id = state.window_manager.add_window("app", "t", 1, original);
        state.window_manager.focus(id).unwrap();

        state.snap(id, SnapZone::Left).unwrap();
        let snapped = state.window_manager.get(id).unwrap().geometry;
        state.set_maximized_target(id, true).unwrap();
        state.set_maximized_target(id, false).unwrap();
        assert_eq!(state.window_manager.get(id).unwrap().geometry, snapped, "unmaximize returns to the snapped geometry");
        state.snap_restore(id).unwrap();
        assert_eq!(state.window_manager.get(id).unwrap().geometry, original);
    }

    /// I5: `MaximizeWindow` over D-Bus is not a visual no-op any more, and
    /// an explicit target that already holds stays a no-op (it must not
    /// re-save the current geometry as a fresh restore point).
    #[test]
    fn maximize_command_is_idempotent_on_its_target() {
        let (mut state, _rx) = state_with_output(1000, 800);
        let original = Rectangle { x: 40, y: 50, width: 300, height: 200 };
        let id = state.window_manager.add_window("app", "t", 1, original);
        state.handle_command(crate::dbus::DbCommand::Maximize(id, true)).unwrap();
        let maximized = state.window_manager.get(id).unwrap().geometry;
        state.handle_command(crate::dbus::DbCommand::Maximize(id, true)).unwrap();
        assert_eq!(state.window_manager.get(id).unwrap().geometry, maximized);
        state.handle_command(crate::dbus::DbCommand::Maximize(id, false)).unwrap();
        assert_eq!(state.window_manager.get(id).unwrap().geometry, original, "the restore point survived the no-op");
    }

    /// I3: `behavior.snap_enabled` is actually consumed -- with snapping
    /// off, neither the `snap:*` actions nor a drag to an edge snap
    /// anything, and the drag still performs a plain move.
    #[test]
    fn snap_disabled_blocks_actions_and_drag_snapping() {
        let (mut state, _rx) = state_with_output(1000, 800);
        state.config.behavior.snap_enabled = false;
        let geo = Rectangle { x: 100, y: 100, width: 640, height: 400 };
        let id = state.window_manager.add_window("app", "t", 1, geo);
        state.window_manager.focus(id).unwrap();

        assert_eq!(state.apply_action("snap:left"), None, "the snap action must not fire when snapping is off");
        assert_eq!(state.window_manager.get(id).unwrap().geometry, geo);

        // A drag to the left edge shows no preview and ends as a plain move.
        state.handle_pointer(PointerEvent::Press { id, pointer: (120, 105) }).unwrap();
        state.handle_pointer(PointerEvent::Motion { pointer: (2, 400) }).unwrap();
        assert_eq!(state.snap_preview, None, "no snap preview may be drawn when snapping is off");
        state.handle_pointer(PointerEvent::Release { pointer: (2, 400) }).unwrap();
        let moved = state.window_manager.get(id).unwrap().geometry;
        assert_eq!((moved.x, moved.y), (2 - 20, 400 - 5), "the drag still moves the window");
        assert_eq!((moved.width, moved.height), (geo.width, geo.height), "…without resizing it");
    }

    /// I3 (the other direction): the default config leaves snapping on, so
    /// nothing about the existing behavior changes.
    #[test]
    fn snap_enabled_by_default_still_snaps() {
        let (mut state, _rx) = state_with_output(1000, 800);
        assert!(state.config.behavior.snap_enabled);
        let id = state.window_manager.add_window("app", "t", 1, DEFAULT_GEO);
        state.window_manager.focus(id).unwrap();
        assert_eq!(state.apply_action("snap:left"), Some(()));
        assert_eq!(state.window_manager.get(id).unwrap().geometry.width, 1000 / 2 - 16);
    }

    /// I6: after `MoveToWorkspace` the destination has a focused window, so
    /// the *next* action isn't a silent no-op -- and the origin workspace is
    /// left focused on whatever it still has.
    #[test]
    fn move_to_workspace_focuses_the_moved_window_and_reseats_the_origin() {
        let (mut state, _rx) = state_with_output(1000, 800);
        let a = state.window_manager.add_window("a", "a", 1, DEFAULT_GEO);
        let b = state.window_manager.add_window("b", "b", 2, DEFAULT_GEO);
        // `b` is focused (added last) and is the one that moves.
        state.handle_command(crate::dbus::DbCommand::MoveToWorkspace(b, 1)).unwrap();

        assert_eq!(state.window_manager.active_workspace(), 1);
        assert_eq!(state.window_manager.focused_window().map(|w| w.id), Some(b), "the moved window is focused there");
        // The origin kept a coherent focus rather than a dangling pointer.
        state.switch_workspace(0).unwrap();
        assert_eq!(state.window_manager.focused_window().map(|w| w.id), Some(a));
    }

    /// I6, via the keybinding path: the action that used to leave the
    /// destination unfocused is followed by a `fullscreen` that must act on
    /// the moved window.
    #[test]
    fn action_after_move_to_workspace_is_not_a_noop() {
        let (mut state, _rx) = state_with_output(1000, 800);
        let id = state.window_manager.add_window("app", "t", 1, DEFAULT_GEO);
        state.apply_action("move_to_workspace:2").unwrap();
        assert_eq!(state.apply_action("fullscreen"), Some(()), "the next action must find a focused window");
        assert!(state.window_manager.get(id).unwrap().fullscreen);
    }

    /// I1/I6: switching to a workspace that has windows focuses its MRU
    /// head, so actions work there immediately; switching to an empty one
    /// leaves focus cleanly absent rather than pointing elsewhere.
    #[test]
    fn switch_workspace_focuses_that_workspaces_mru_head() {
        let (mut state, _rx) = state_with_output(1000, 800);
        let a = state.window_manager.add_window("a", "a", 1, DEFAULT_GEO);
        state.switch_workspace(1).unwrap();
        assert!(state.window_manager.focused_window().is_none(), "workspace 2 is empty");
        assert_eq!(state.apply_action("close"), None, "…so an action there is a clean no-op");
        state.switch_workspace(0).unwrap();
        assert_eq!(state.window_manager.focused_window().map(|w| w.id), Some(a));
    }

    /// I2: every event reaching the D-Bus channel carries the seq its
    /// mutation produced, those seqs strictly increase, and the last one
    /// matches `snapshot().seq` -- which is what lets a subscriber order
    /// signals against a `GetState()` snapshot.
    #[test]
    fn emitted_events_carry_monotonic_seq_matching_the_snapshot() {
        let (mut state, rx) = state_with_output(1000, 800);
        let id = state.window_manager.add_window("app", "t", 1, DEFAULT_GEO);
        state.emit_pending();
        state.handle_command(crate::dbus::DbCommand::Fullscreen(id, true)).unwrap();
        state.handle_command(crate::dbus::DbCommand::SetWorkspace(1)).unwrap();

        let events: Vec<SeqEvent> = rx.try_iter().collect();
        assert!(events.len() >= 3, "expected several events, got {}", events.len());
        let seqs: Vec<u64> = events.iter().map(|e| e.seq).collect();
        assert!(seqs.windows(2).all(|w| w[1] > w[0]), "seqs must strictly increase: {seqs:?}");
        assert_eq!(*seqs.last().unwrap(), state.window_manager.snapshot().seq);
        assert!(seqs[0] > 0, "seq 0 means 'nothing has happened yet' and must never be emitted");
    }

    /// I2: the reload path used to bypass the counter entirely (its events
    /// went out through a separate queue with no seq of their own).
    #[test]
    fn reloaded_config_events_carry_seq_and_never_regress() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        state.window_manager.add_window("a", "a", 1, DEFAULT_GEO);
        state.emit_pending();
        let before = state.window_manager.snapshot().seq;
        let drained: Vec<SeqEvent> = rx.try_iter().collect();
        assert!(drained.iter().all(|e| e.seq <= before));

        state.apply_reloaded_config(icedtea_config::default_config());
        let events: Vec<SeqEvent> = rx.try_iter().collect();
        assert!(!events.is_empty());
        assert!(events.iter().all(|e| e.seq > before), "reload events must advance past the pre-reload high-water mark");
        let seqs: Vec<u64> = events.iter().map(|e| e.seq).collect();
        assert!(seqs.windows(2).all(|w| w[1] > w[0]), "seqs must strictly increase: {seqs:?}");
        assert!(events.iter().any(|e| matches!(e.event, Event::ConfigReloaded(_))));
    }

    /// C1: closing a model window with no backing client surface still
    /// removes it synchronously (there is no client to send `close` to and
    /// no destroy will ever arrive), and focus lands somewhere sensible.
    #[test]
    fn request_close_without_a_surface_removes_and_reseats_focus() {
        let (mut state, _rx) = state_with_output(1000, 800);
        let a = state.window_manager.add_window("a", "a", 1, DEFAULT_GEO);
        let b = state.window_manager.add_window("b", "b", 2, DEFAULT_GEO);
        state.request_close(b);
        assert!(state.window_manager.get(b).is_none());
        assert_eq!(state.window_manager.focused_window().map(|w| w.id), Some(a), "focus must not dangle after a close");
    }

    /// C1: closing must not leave restore points behind for an id that can
    /// never come back.
    #[test]
    fn closing_clears_saved_geometry_slots() {
        let (mut state, _rx) = state_with_output(1000, 800);
        let id = state.window_manager.add_window("app", "t", 1, DEFAULT_GEO);
        state.window_manager.focus(id).unwrap();
        state.snap(id, SnapZone::Left).unwrap();
        state.set_maximized_target(id, true).unwrap();
        state.set_fullscreen_target(id, true).unwrap();
        assert!(state.snap_saved_geometry.contains_key(&id));
        state.request_close(id);
        assert!(!state.snap_saved_geometry.contains_key(&id));
        assert!(!state.maximized_saved_geometry.contains_key(&id));
        assert!(!state.fullscreen_saved_geometry.contains_key(&id));
    }

    /// C1 (`resize_request`'s model half): an interactive resize driven by
    /// the pointer path updates the model geometry as it goes and commits
    /// the final one on release.
    #[test]
    fn interactive_resize_updates_geometry_and_ends_on_release() {
        let (mut state, _rx) = state_with_output(1000, 800);
        let geo = Rectangle { x: 100, y: 100, width: 400, height: 300 };
        let id = state.window_manager.add_window("app", "t", 1, geo);
        state.resize.begin(id, input::ResizeEdges { bottom: true, right: true, ..Default::default() }, geo, (500, 400));

        state.handle_pointer(PointerEvent::Motion { pointer: (560, 430) }).unwrap();
        assert_eq!(
            state.window_manager.get(id).unwrap().geometry,
            Rectangle { x: 100, y: 100, width: 460, height: 330 }
        );

        state.handle_pointer(PointerEvent::Release { pointer: (600, 500) }).unwrap();
        assert_eq!(
            state.window_manager.get(id).unwrap().geometry,
            Rectangle { x: 100, y: 100, width: 500, height: 400 }
        );
        assert!(state.resize.window_id().is_none(), "the resize ended on release");
        // A later motion with no resize and no drag is a clean no-op.
        assert_eq!(state.handle_pointer(PointerEvent::Motion { pointer: (700, 700) }), None);
    }

    /// M2: `State::new` accepts an arbitrary `Config`, including one with no
    /// workspace names -- that must not leave a reachable index panic.
    #[test]
    fn state_with_no_configured_workspaces_does_not_panic() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut cfg = default_config();
        cfg.workspace_names = vec![];
        let mut state = State::new(cfg, tx);
        let id = state.window_manager.add_window("app", "t", 1, DEFAULT_GEO);
        assert_eq!(state.window_manager.workspace_info().len(), 1);
        assert_eq!(state.window_manager.focused_window().map(|w| w.id), Some(id));
    }
}
