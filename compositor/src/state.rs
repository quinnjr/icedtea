//! The compositor's own state: `State` owns the window model
//! (`WindowManager`) and configuration, and fans out `contract::Event`s
//! produced by model mutations onto a crossbeam channel for the D-Bus side
//! to consume.
//!
//! This is the model-only shape of `State`, mid-port (wlr-port milestone 1,
//! task 4): every push out to a client goes through `wayland: Wayland`
//! (`wayland.rs`) rather than through smithay's `Space`/`ToplevelSurface`
//! types directly, so this file has no compositor-library dependency at
//! all. `Wayland`'s methods are no-ops until task 5 backs them with `wlr`;
//! until then every mutation here behaves exactly as it does in any test
//! that never binds a toplevel.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use icedtea_config::Config;
use icedtea_contract::{AltTabState, Event, Rectangle, SeqEvent, WindowId};

use crate::input;
use crate::layout::{self, SnapZone};
use crate::render::WallpaperState;
use crate::window::WindowManager;

/// The model's placeholder toplevel size: staged as both the new window's
/// frame geometry and the client's first configure, until the client's own
/// first commit says otherwise. One constant rather than the literal
/// `(640, 400)` repeated at each site (`new_toplevel`'s cascade placement,
/// `initial_commit`'s configure, and the test that asserts on it) so the
/// three/four call sites cannot drift apart from each other.
pub const PLACEHOLDER_SIZE: (i32, i32) = (640, 400);

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

pub struct State {
    pub window_manager: WindowManager,
    pub config: Config,
    /// Outbound event channel. Every message carries the `seq` its mutation
    /// produced (review finding I2) so a subscriber can order signals
    /// against a `GetState()` snapshot and detect gaps.
    pub dbus_tx: crossbeam_channel::Sender<SeqEvent>,
    /// The compositor's Wayland side -- the one seam between this model and
    /// a client. See `wayland.rs`'s module doc.
    pub wayland: crate::wayland::Wayland,
    pub start_time: Instant,
    /// Wallpaper decode state (`render::spawn_wallpaper_decode` produces it;
    /// there is no renderer in this crate to upload or draw it yet).
    pub wallpaper: WallpaperState,
    /// The drag machine's current snap-preview target, in output logical
    /// coordinates, or `None` when no drag is in a snap-preview state. Set
    /// by the drag machine (Task 9); this is the geometry hook `render.rs`
    /// consumes.
    pub snap_preview: Option<icedtea_contract::Rectangle>,
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
    /// ended by `end_alt_tab` (see its doc for the chosen end condition --
    /// driving this from a real keyboard filter is task 5's job).
    pub alt_tab: input::AltTabMachine,
    /// Pointer-driven window move/snap state machine.
    pub drag: input::DragMachine,
    /// Pointer-driven interactive-resize state machine, stepped by the same
    /// pointer motion/release path the drag machine uses. A client-driven
    /// resize request will start one again once task 5 wires the inbound
    /// half of that protocol back up.
    pub resize: input::ResizeMachine,
    /// Set by `apply_action("quit")`; the event loop (task 5 reintroduces
    /// one) checks this each iteration and calls `stop()` once true.
    pub quitting: bool,
    /// Last known pointer position in output logical coordinates. Task 5's
    /// input plumbing updates this on every pointer-motion event;
    /// button-only events (which carry no position of their own) read it
    /// back to build `PointerEvent::Press`/`Release`.
    pub pointer_location: (i32, i32),
    /// On-disk location `reload_config_from_disk`/`handle_command`'s
    /// `ReloadConfig` worker thread reads from. `None` (the boot-time
    /// default) means "use `icedtea_config::default_db_path()`" -- this is
    /// only ever overridden by tests, which need an isolated temp DB rather
    /// than the real XDG path.
    pub config_path: Option<PathBuf>,
    /// Set by `set_config_reload_sender` once the caller has somewhere to
    /// send reload results (in production, `lib.rs`'s `run()`, which also
    /// keeps the paired receiver -- see `config_reload_rx`).
    /// `handle_command`'s `ReloadConfig` arm sends the freshly-loaded
    /// `Config` back over this from a worker thread so the redb I/O + JSON
    /// parse never blocks the caller's loop; `None` before that wiring
    /// exists is a no-op (nothing to reload into).
    config_reload_tx: Option<crossbeam_channel::Sender<Config>>,
    /// Paired receiving half of `config_reload_tx`, drained once per turn by
    /// `drain_config_reload`. `None` until `set_config_reload_receiver`
    /// wires it -- tests that only exercise the sender side (the reload
    /// tests below) never set this.
    config_reload_rx: Option<crossbeam_channel::Receiver<Config>>,
    /// Which model output index each library output id maps to.
    ///
    /// Kept because `OutputHandler::destroyed` is given only an id, and the
    /// model's geometry map is keyed by index.
    output_ids: HashMap<wlr::OutputId, u32>,
    /// The scene rect painted behind everything, once boot has made one.
    background: Option<wlr::RectId>,
    /// The fd source SIGINT/SIGTERM write to. Compared in `fd_ready` so that
    /// a future second source cannot be mistaken for this one.
    shutdown_source: Option<wlr::SourceId>,
    /// The D-Bus service's command channel, drained once per loop turn (by
    /// `fd_ready`'s `cmd_wake_source` arm, and as a backstop by
    /// `should_stop`).
    cmd_rx: Option<crossbeam_channel::Receiver<crate::dbus::DbCommand>>,
    /// The fd source `WmInterface::send` (`dbus.rs`) nudges after every
    /// command it forwards onto `cmd_rx`. Compared in `fd_ready` the same
    /// way `shutdown_source` is; without it a command sent while the loop is
    /// blocked in `dispatch(-1)` would sit undrained until some unrelated
    /// event happened to wake the loop anyway.
    cmd_wake_source: Option<wlr::SourceId>,
    /// The fd source `spawn_config_reload`'s worker thread nudges after
    /// sending a freshly-loaded config over `config_reload_tx`. Same
    /// purpose as `cmd_wake_source`, for `drain_config_reload`.
    config_reload_wake_source: Option<wlr::SourceId>,
    /// Write half of the pipe registered as `config_reload_wake_source`.
    /// Kept here (not just handed to the one worker thread that exists when
    /// `set_config_reload_wake` runs) because `spawn_config_reload` can spin
    /// up a fresh worker on every reload trigger, and each one needs its own
    /// `try_clone`d handle -- see that method's doc.
    config_reload_wake: Option<std::os::unix::net::UnixStream>,
}

impl State {
    pub fn new(config: Config, dbus_tx: crossbeam_channel::Sender<SeqEvent>) -> Self {
        let workspace_names = config.workspace_names.clone();
        // Review finding I4: report unusable bindings once, here, instead of
        // from inside the per-key-press lookup.
        input::warn_about_keybindings(&config.keybindings);

        Self {
            window_manager: WindowManager::new(workspace_names),
            config,
            dbus_tx,
            wayland: crate::wayland::Wayland::new(),
            start_time: Instant::now(),
            wallpaper: WallpaperState::new(),
            snap_preview: None,
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
            config_reload_rx: None,
            output_ids: HashMap::new(),
            background: None,
            shutdown_source: None,
            cmd_rx: None,
            cmd_wake_source: None,
            config_reload_wake_source: None,
            config_reload_wake: None,
        }
    }

    /// Wire up the config-reload result channel. Both halves are kept: the
    /// sender goes to the worker thread `spawn_config_reload` spins up, and
    /// the loop drains the receiver itself (via `drain_config_reload`) now
    /// that there is no event-source abstraction to do it.
    pub fn set_config_reload_sender(&mut self, tx: crossbeam_channel::Sender<Config>) {
        self.config_reload_tx = Some(tx);
    }

    /// Keep the receiving half so [`Self::drain_config_reload`] has
    /// somewhere to read from. Separate from `set_config_reload_sender`
    /// because the tests wire only the sender and read the receiver
    /// themselves.
    pub fn set_config_reload_receiver(&mut self, rx: crossbeam_channel::Receiver<Config>) {
        self.config_reload_rx = Some(rx);
    }

    /// Apply any config a reload worker has finished loading.
    ///
    /// Called once per event-loop turn. Non-blocking: `try_recv` on an empty
    /// channel is the overwhelmingly common case and must cost nothing.
    pub fn drain_config_reload(&mut self) {
        let Some(rx) = self.config_reload_rx.as_ref() else { return };
        let pending: Vec<Config> = rx.try_iter().collect();
        for cfg in pending {
            let _ = self.apply_reloaded_config(cfg);
        }
    }

    /// Ask the event loop to stop.
    ///
    /// Sets the same flag `apply_action("quit")` does rather than signalling a
    /// loop handle, because the loop is now driven by `wlr` and asks the state
    /// whether to stop (`LoopHandler::should_stop`) instead of being told.
    pub fn stop(&mut self) {
        self.quitting = true;
    }

    /// Record an output's geometry. Backends call this once they know the mode.
    ///
    /// The smithay version of this also created a protocol global and mapped
    /// the output into a `Space`; both now belong to the compositor library
    /// (the global comes with the backend, the placement with the scene's
    /// output layout), so all that is left here is the geometry map that
    /// `snap`, `set_fullscreen_target`, `set_maximized_target` and
    /// `handle_pointer_motion` read.
    pub fn create_output(&mut self, index: u32, geometry: icedtea_contract::Rectangle) {
        self.outputs.insert(index, OutputSurface { geometry });
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

    // --- Model <-> Wayland reconciliation (review finding C1) ---
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
    // geometry/state mutation ends in `sync_window_to_scene`, every
    // workspace/visibility change ends in `sync_scene`, and every close goes
    // through `request_close`.

    /// Push model window `id`'s geometry, visibility, and xdg state out to its
    /// client: position its scene node at the model's position, hide it when
    /// the window isn't on the active workspace (review finding I1), and
    /// configure the toplevel with the model's size plus the
    /// maximized/fullscreen/activated states.
    ///
    /// Call this at the tail of *every* geometry or state mutation. A window
    /// with no backing client is a silent no-op.
    ///
    /// Coordinate spaces (re-review minor 3, carried over verbatim from the
    /// smithay implementation because the invariant is unchanged): the model
    /// (`window.rs`, `window_at`, `decoration::hit_test`, drag offsets) is in
    /// *frame* space; the scene node's position and the staged size are in
    /// *content* space -- for an SSD window they differ by `TITLE_BAR_HEIGHT`.
    /// Any consumer of scene coordinates (pointer hit-testing into client
    /// surfaces, above all) must convert with `decoration::content_rect`,
    /// never compare the two spaces directly.
    ///
    /// Note (re-review minor 4): the early return means the trailing
    /// `sync_seat_focus` only runs for windows that still have a client.
    /// Focus-clearing on removal paths must not rely on this tail --
    /// `forget_window` calls `sync_focus_change(None)` explicitly for exactly
    /// that reason.
    pub fn sync_window_to_scene(&mut self, id: WindowId) {
        if !self.wayland.is_backed(id) {
            return;
        }
        let Some(w) = self.window_manager.get(id) else {
            // The model window is gone but the client is still here (a close
            // that raced the client's destroy): hide it and drop the binding
            // rather than showing a window nothing tracks.
            self.wayland.set_visible(id, false);
            self.wayland.forget(id);
            return;
        };
        let (geo, fullscreen, maximized, focused) = (w.geometry, w.fullscreen, w.maximized, w.focused);
        let visible = self.window_manager.is_visible(w);
        // Re-review finding New-4: the model's geometry is the *frame*; an
        // SSD window's client owns only the band below the title bar.
        let ssd = crate::decoration::has_ssd(&w.app_id, w.client_decorations_requested, fullscreen);
        let content = crate::decoration::content_rect(geo, ssd);

        self.wayland.set_visible(id, visible);
        if visible {
            self.wayland.set_position(id, content.x, content.y);
            // Ledger item 28 / recommendation 4: `behavior.raise_on_focus`
            // decides whether a focus change alone may restack. With it off a
            // focused window is still activated and configured, it just keeps
            // its place in the stack.
            //
            // LEDGER DECISION (task 8): the pre-port smithay code also raised
            // on *any* geometry change, with no `raise_on_focus` check at
            // all -- `Space::map_element` was both the move and the raise in
            // one call, so every move restacked whether or not the mover was
            // the focused window. That does not return here. It was an
            // artifact of `map_element`'s API shape, not a documented
            // behavior the seam owes: a pure move not restacking is the
            // better floating-WM behavior, and it is what `raise_on_focus`'s
            // own name promises -- a knob that says "raise on focus," not
            // "raise on focus or move." Reintroducing moved-implies-raise
            // would make the knob lie for drag-move, which is by far the most
            // common source of geometry changes.
            if focused && self.config.behavior.raise_on_focus {
                self.wayland.raise(id);
            }
        }
        self.wayland
            .configure(id, content, focused && visible, maximized, fullscreen);

        // Keyboard focus is the other half of "focus reached the client".
        self.sync_seat_focus();
    }

    /// The active workspace's focused window id, if any. Callers capture this
    /// *before* a focus-changing mutation and hand it to `sync_focus_change`.
    fn focused_id(&self) -> Option<WindowId> {
        self.window_manager.focused_window().map(|w| w.id)
    }

    /// Point the seat's keyboard at whatever the model currently says is
    /// focused -- or at nothing, when it says nothing is.
    ///
    /// Re-review finding New-3: seat keyboard focus used to be set-only, so
    /// after the last window on a workspace was closed, minimized, or left
    /// behind by a workspace switch, the seat kept pointing at a surface the
    /// user could no longer see. Driving the target from the model -- not from
    /// whichever window a caller happens to be syncing -- makes this
    /// idempotent, which is why `sync_window_to_scene` can call it
    /// unconditionally. The library compares against the seat's current focus
    /// and does nothing when it already matches, so an ordinary geometry sync
    /// does not churn leave/enter pairs at the client.
    fn sync_seat_focus(&mut self) {
        let focused = self
            .focused_id()
            .filter(|&id| self.window_manager.is_visible_id(id))
            .filter(|&id| self.wayland.is_backed(id));
        self.wayland.keyboard_focus(focused);
    }

    /// Reconcile a focus transition all the way out to the clients.
    /// `previous` is what `focused_id()` returned *before* the model
    /// mutation; pass `None` when the previously focused window is already
    /// gone from the model (a close/destroy).
    ///
    /// Re-review findings New-1 and New-2. New-1: every focus path synced
    /// only the window that *gained* focus, so the one that lost it kept its
    /// `Activated` xdg state and its client went on rendering itself as the
    /// active window -- moving focus A -> B left two windows looking focused.
    /// New-2: when the focused toplevel was destroyed the model picked a
    /// successor (`forget_window` -> `focus_mru_in_workspace`) that was never
    /// synced, so the successor's client was never activated and never
    /// received seat keyboard focus -- the keyboard was dead until the user
    /// clicked something. Both are the same missing step: a focus change has
    /// two ends, and both have to be pushed.
    ///
    /// Wayland-side only: the model mutation that moved focus already emitted
    /// its own `WindowUpdated` events (`WindowManager::focus` emits for the
    /// window losing focus as well as the one gaining it), so nothing here
    /// emits.
    fn sync_focus_change(&mut self, previous: Option<WindowId>) {
        let current = self.focused_id();
        if let Some(prev) = previous.filter(|prev| Some(*prev) != current) {
            self.sync_window_to_scene(prev);
        }
        match current {
            Some(id) => self.sync_window_to_scene(id),
            // Nothing focused: `sync_window_to_scene` isn't reached at all,
            // so clear the seat here (New-3).
            None => self.sync_seat_focus(),
        }
    }

    /// Minimize or restore window `id`, handing focus to the workspace's MRU
    /// successor when the focused window is the one being minimized, then
    /// reconcile both ends of the transition out to the clients.
    ///
    /// Re-review Important 1: `DbCommand::Minimize` mutated the model and
    /// synced only `id`, bypassing the focus-transition path entirely --
    /// minimizing the focused window from the taskbar (the common entry
    /// point) left the model's focus on a now-invisible window, activated no
    /// successor, and `sync_seat_focus`'s visibility filter then cleared the
    /// seat: a dead keyboard with windows still on screen, New-2's symptom
    /// through a different door. The title-bar button already did this
    /// correctly; this is that arm's body, extracted so "minimize a window"
    /// has exactly one implementation, matching the maximize/fullscreen
    /// handlers' pattern.
    fn set_minimized_and_reconcile(&mut self, id: WindowId, value: bool) -> Option<()> {
        let previous = self.focused_id();
        self.window_manager.set_minimized(id, value)?;
        if value && previous == Some(id) {
            let active = self.window_manager.active_workspace();
            self.window_manager.focus_mru_in_workspace(active);
        }
        // `id` itself always needs a sync (its visibility just changed),
        // even when it wasn't the focused window; `sync_focus_change` then
        // handles the successor and the seat (New-1/2/3). On restore, if the
        // model's focus pointer never left `id`, the visibility filter now
        // passes again and this same pair re-activates it and returns it the
        // keyboard.
        self.sync_window_to_scene(id);
        self.sync_focus_change(previous);
        Some(())
    }

    /// Reconcile every model window with its client at once. Used after
    /// changes that can alter many windows' visibility in one go (workspace
    /// switch, config reload).
    ///
    /// Per-window syncing covers the focus transition's two ends on its own
    /// (every window is synced, including whichever one just lost focus), but
    /// the trailing `sync_seat_focus` is still needed for New-3's
    /// "switched to an empty workspace" case: with nothing focused *and*
    /// possibly nothing to iterate, the loop body may never run.
    pub fn sync_scene(&mut self) {
        let ids: Vec<WindowId> = self.window_manager.windows().map(|w| w.id).collect();
        for id in ids {
            self.sync_window_to_scene(id);
        }
        self.sync_seat_focus();
    }

    /// Ask window `id` to close.
    pub fn request_close(&mut self, id: WindowId) {
        if self.wayland.close(id) {
            // A real client: the model row stays until the client actually
            // destroys its toplevel, which lands in `forget_toplevel`.
            return;
        }
        self.wayland.forget(id);
        self.forget_window(id);
        self.emit_pending();
    }

    /// Drop every trace of `id` from the model and its side tables, then
    /// hand focus to whatever is left on the active workspace (review
    /// finding I6's coherence requirement, applied to closes as well as
    /// moves: an action right after a close must not no-op on a dangling
    /// focus pointer).
    fn forget_window(&mut self, id: WindowId) {
        self.wayland.forget(id);
        self.window_manager.remove_window(id);
        self.fullscreen_saved_geometry.remove(&id);
        self.snap_saved_geometry.remove(&id);
        self.maximized_saved_geometry.remove(&id);
        if self.window_manager.focused_window().is_none() {
            let active = self.window_manager.active_workspace();
            self.window_manager.focus_mru_in_workspace(active);
        }
        // Re-review finding New-2: picking a successor in the model was only
        // half the job -- it also has to reach the client (`Activated`) and
        // the seat (keyboard focus), and when the workspace has no successor
        // left the seat's focus has to be cleared rather than left pointing
        // at the destroyed surface. `previous` is `None` because `id` is
        // already out of the model by this point, so there is nothing left
        // to sync on the losing end.
        self.sync_focus_change(None);
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
        self.sync_window_to_scene(id);
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
        self.sync_window_to_scene(id);
        self.emit_pending();
        Some(())
    }

    /// Apply a client-requested maximize/unmaximize for `toplevel`, reporting
    /// whether the model actually changed -- i.e. whether
    /// `sync_window_to_scene` will have sent the client a configure of its
    /// own. `false` (unknown toplevel, state already as requested, or a
    /// failed precondition such as no known output) tells the caller to
    /// answer with a bare configure instead, so the request is never left
    /// unanswered.
    ///
    /// Unused in this commit: nothing constructs a `ToplevelKey` until task 5
    /// wires a client-driven maximize/fullscreen request onto this seam.
    /// Kept now (with the `ToplevelKey` signature already in place) so that
    /// wiring is a call site, not a rewrite.
    #[allow(dead_code)]
    pub fn reconcile_maximized(&mut self, toplevel: crate::wayland::ToplevelKey, target: bool) -> bool {
        let Some(id) = self.wayland.window_for(toplevel) else { return false };
        let changes = self.window_manager.get(id).is_some_and(|w| w.maximized != target);
        changes && self.set_maximized_target(id, target).is_some()
    }

    /// Fullscreen counterpart of [`Self::reconcile_maximized`]; same
    /// "did a configure actually go out" contract.
    #[allow(dead_code)]
    pub fn reconcile_fullscreen(&mut self, toplevel: crate::wayland::ToplevelKey, target: bool) -> bool {
        let Some(id) = self.wayland.window_for(toplevel) else { return false };
        let changes = self.window_manager.get(id).is_some_and(|w| w.fullscreen != target);
        changes && self.set_fullscreen_target(id, target).is_some()
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
    /// the result back over `config_reload_tx`; `drain_config_reload` picks
    /// it up on the next turn once it arrives. A no-op if `config_reload_tx`
    /// hasn't been wired yet (only possible before `set_config_reload_sender`
    /// has run).
    fn spawn_config_reload(&self) {
        let Some(tx) = self.config_reload_tx.clone() else { return };
        let path = self.config_path.clone().unwrap_or_else(icedtea_config::default_db_path);
        // `try_clone` dup(2)s the wake pipe's write half so the worker
        // thread owns a handle it can move across the `'static` bound
        // rather than borrowing `self`'s. `None` (no test wires this, and a
        // failed `try_clone` degrades the same way) means no wake is sent
        // and `drain_config_reload`'s periodic poll from `should_stop`
        // remains the only way the result is ever picked up -- exactly the
        // pre-wake-pipe behavior, not a regression.
        let wake = self.config_reload_wake.as_ref().and_then(|w| w.try_clone().ok());
        std::thread::spawn(move || {
            let cfg = icedtea_config::load_or_default(&path);
            let _ = tx.send(cfg);
            if let Some(wake) = wake {
                crate::backend::wake(&wake);
            }
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
    /// and ships the result back over `config_reload_tx`; `drain_config_reload`
    /// is what actually calls `apply_reloaded_config` once the load
    /// finishes. If `config_reload_tx` hasn't been wired yet, this is a
    /// no-op.
    pub fn handle_command(&mut self, cmd: crate::dbus::DbCommand) -> Option<()> {
        use crate::dbus::DbCommand;
        match cmd {
            DbCommand::Focus(id) => {
                let previous = self.focused_id();
                self.window_manager.focus(id)?;
                self.sync_focus_change(previous);
            }
            DbCommand::Close(id) => self.request_close(id),
            DbCommand::Minimize(id, value) => self.set_minimized_and_reconcile(id, value)?,
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
    /// if it has a focusable one, and reconcile the scene so only its
    /// windows are visible (review findings I1 and I6).
    pub fn switch_workspace(&mut self, workspace: u32) -> Option<()> {
        if !self.window_manager.set_active_workspace(workspace) {
            return None;
        }
        if self.window_manager.focused_window().is_none() {
            self.window_manager.focus_mru_in_workspace(workspace);
        }
        self.sync_scene();
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
        self.sync_scene();
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
                let _ = self.set_minimized_and_reconcile(id, true);
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
                    let previous = self.focused_id();
                    self.window_manager.focus(wid);
                    self.sync_focus_change(previous);
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
    /// binding) is released. The keyboard filter (task 5 reintroduces one)
    /// calls this on every key event where the current modifier state no
    /// longer includes SUPER, which covers both "released the modifier
    /// while still holding Tab" and "released Tab first, then the
    /// modifier."
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
        self.sync_window_to_scene(id);
        Some(())
    }

    /// Restore `id`'s geometry as it was before its most recent `snap`, if
    /// any was saved. Deliberately *not* gated on `snap_enabled`: a window
    /// snapped before the flag was turned off must still be restorable.
    pub fn snap_restore(&mut self, id: WindowId) -> Option<()> {
        if let Some(orig) = self.snap_saved_geometry.remove(&id) {
            let snapped = self.window_manager.get(id)?.geometry;
            let restored = layout::restored_geometry(orig, snapped);
            self.window_manager.set_geometry(id, restored)?;
            self.sync_window_to_scene(id);
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
    /// tracked), and the fresh `WindowManager`'s id counter reset to 1, so
    /// windows mapped after the reload could collide with ids the shell
    /// might still remember as live. Fixed here: emit `WindowClosed` for
    /// every window that's about to disappear, drop their client bindings
    /// (so a later destroy for one of those toplevels silently no-ops
    /// instead of resolving to a stale id), and floor the new
    /// `WindowManager`'s id counter at the old one's high-water mark so no
    /// id is ever reissued. The plan's actual invariant (workspaces rebuilt
    /// from config) is preserved unchanged; only the "silently destroy live
    /// state" part of the sample was a bug.
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

        // Every window about to be discarded loses its client binding too --
        // `wayland.rs`'s bind/forget replace the old `elements`/
        // `surface_to_window` maps as the record of which toplevel backs
        // which model window.
        let ids: Vec<WindowId> = self.window_manager.windows().map(|w| w.id).collect();
        for id in ids {
            self.wayland.forget(id);
        }
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
        let previous = self.focused_id();
        self.window_manager.focus(id)?;
        // Focus changes the client's activation state, so it has to reach
        // the client too (C1) -- both ends of the transition, not just the
        // new one (New-1).
        self.sync_focus_change(previous);
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
            self.sync_window_to_scene(id);
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
            self.sync_window_to_scene(id);
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
                self.sync_window_to_scene(id);
            }
            input::DragResult::Restored => {}
        }
        self.emit_pending();
        Some(())
    }

    /// A client mapped a new toplevel. Creates the model window at a cascade
    /// position, binds it to `toplevel`, and reconciles focus.
    pub fn new_toplevel(&mut self, toplevel: crate::wayland::ToplevelKey, app_id: &str, title: &str, pid: u32) {
        // Default toplevel size: real geometry arrives from the client's
        // first commit, which isn't known yet; `PLACEHOLDER_SIZE` is the
        // model's placeholder until then. Position cascades off whatever is
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
        // Review finding M7: cascade positions wrap inside the output (and
        // count only this workspace's windows) so the Nth window can't open
        // off-screen with no title bar to grab.
        let (x, y) = layout::cascade_point_in(&occupied, PLACEHOLDER_SIZE, 24, output_geo);
        let geometry =
            icedtea_contract::Rectangle { x, y, width: PLACEHOLDER_SIZE.0, height: PLACEHOLDER_SIZE.1 };
        // `add_window` autofocuses, so this is a focus change like any other
        // (New-1): capture the outgoing focus before it happens.
        let previous = self.focused_id();
        let id = self.window_manager.add_window(app_id, title, pid, geometry);
        self.wayland.bind(id, toplevel);
        // Explicit sync of the new window first (re-review minor 2):
        // `sync_focus_change` also reaches it today, but only because
        // `add_window` autofocuses onto the active workspace.
        self.sync_window_to_scene(id);
        self.sync_focus_change(previous);
        self.emit_pending();
    }

    /// A client destroyed its toplevel.
    ///
    /// Named `forget_toplevel` rather than `toplevel_destroyed` (task 8):
    /// `wlr::ToplevelHandler::toplevel_destroyed` now exists as a same-named
    /// trait method on `State`, and while the two don't collide (an inherent
    /// method always wins over a trait method of the same name during
    /// resolution, so `self.toplevel_destroyed(key)` would keep working), the
    /// call site inside the trait impl would read as a same-name recursive
    /// call to a reader who can't see that resolution rule. Renaming the
    /// inherent method removes the ambiguity at the source instead of relying
    /// on it being resolved correctly.
    pub fn forget_toplevel(&mut self, toplevel: crate::wayland::ToplevelKey) {
        let Some(id) = self.wayland.window_for(toplevel) else { return };
        self.wayland.forget(id);
        self.forget_window(id);
        self.emit_pending();
    }

    /// A client changed its title.
    pub fn toplevel_title_changed(&mut self, toplevel: crate::wayland::ToplevelKey, title: &str) {
        let Some(id) = self.wayland.window_for(toplevel) else { return };
        if self.window_manager.set_title(id, title.to_string()).is_some() {
            self.emit_pending();
        }
    }

    /// The topmost model window at `point` (frame space), or `None`.
    ///
    /// The model is the authority on this rather than the scene: the scene
    /// knows nothing about workspaces or minimization, and `visible_windows`
    /// is already MRU-ordered, which is a correct topmost-first order for
    /// click-to-focus (review finding I1).
    pub fn window_at_point(&self, point: (i32, i32)) -> Option<WindowId> {
        self.window_manager.window_at(point).map(|w| w.id)
    }

    pub fn set_background(&mut self, rect: wlr::RectId) {
        self.background = Some(rect);
    }

    pub fn background(&self) -> Option<wlr::RectId> {
        self.background
    }

    pub fn set_shutdown_source(&mut self, id: wlr::SourceId) {
        self.shutdown_source = Some(id);
    }

    pub fn set_command_receiver(&mut self, rx: crossbeam_channel::Receiver<crate::dbus::DbCommand>) {
        self.cmd_rx = Some(rx);
    }

    /// The `SourceId` `fd_ready` compares against to route a wake to
    /// `drain_pending_commands`. See `cmd_wake_source`'s field doc.
    pub fn set_cmd_wake_source(&mut self, id: wlr::SourceId) {
        self.cmd_wake_source = Some(id);
    }

    /// The `SourceId` `fd_ready` compares against to route a wake to
    /// `drain_config_reload`. See `config_reload_wake_source`'s field doc.
    pub fn set_config_reload_wake_source(&mut self, id: wlr::SourceId) {
        self.config_reload_wake_source = Some(id);
    }

    /// The write half `spawn_config_reload` nudges its worker threads with.
    /// See `config_reload_wake`'s field doc for why it lives on `State`
    /// rather than being handed to one worker directly.
    pub fn set_config_reload_wake(&mut self, write: std::os::unix::net::UnixStream) {
        self.config_reload_wake = Some(write);
    }

    /// Apply every D-Bus command that arrived since the last turn.
    ///
    /// Collected before applying, rather than iterated lazily: `handle_command`
    /// takes `&mut self`, and the receiver lives in `self`.
    fn drain_pending_commands(&mut self) {
        let Some(rx) = self.cmd_rx.as_ref() else { return };
        let pending: Vec<crate::dbus::DbCommand> = rx.try_iter().collect();
        for cmd in pending {
            self.handle_command(cmd);
        }
    }
}

// --- Compositor library handlers ---
//
// Every method below runs underneath an `extern "C"` frame: a panic escaping
// one aborts the process rather than failing anything. So there is no
// `unwrap`, no `expect`, no `assert!`, and no indexing in any of these
// bodies — a condition that cannot be handled is recorded in `State` and
// acted on once control is back on the loop.

impl wlr::OutputHandler for State {
    fn new_output(&mut self, output: &wlr::Output<'_>) {
        let Some(runtime) = self.wayland.runtime().cloned() else { return };
        // Renderer before enable: the DRM backend's enabling commit needs a
        // framebuffer, which wlroots can only allocate once the output has
        // its renderer/allocator. Nested backends tolerate either order,
        // which is how enable-first survived until the first real-DRM boot.
        if let Err(err) = runtime.init_output(output) {
            tracing::error!(?err, "could not give output a renderer");
            return;
        }
        if let Err(err) = output.enable_with_preferred_mode() {
            tracing::error!(?err, "could not enable output");
            return;
        }

        let (width, height) = output.size();
        // Single output at (0, 0) for the slice. `self.outputs.len()` as the
        // index is only correct because of that -- it is not a real
        // hotplug-safe id allocator: remove output 0, then add a new one,
        // and `len()` computes `0` again, colliding with whatever the model
        // (or a client-facing consumer) still remembers about the old
        // index. A monotonic counter is the actual fix and belongs to
        // multi-output geometry, which is parity work; this comment is the
        // ledger entry for why `new_output` doesn't pretend to have solved
        // it already.
        let index = self.outputs.len() as u32;
        self.create_output(index, icedtea_contract::Rectangle { x: 0, y: 0, width, height });
        self.output_ids.insert(output.id(), index);

        // The background covers the whole output. Sized here rather than at
        // creation because the mode is not known until now.
        if let Some(rect) = self.background {
            runtime.set_rect_size(rect, width, height);
            runtime.set_rect_position(rect, 0, 0);
        }

        // Existing windows (there are none at boot, but a hotplugged output
        // is the same code path) need their geometry pushed at the new size.
        self.sync_scene();
        self.emit_pending();
    }

    fn frame(&mut self, output: &wlr::Output<'_>) {
        let Some(runtime) = self.wayland.runtime() else { return };
        // A rejected commit is routine — wlroots rejects one when nothing
        // changed — so it is logged at debug and never escalated.
        if let Err(err) = runtime.commit_output(output) {
            tracing::debug!(?err, "scene commit rejected");
        }
    }

    fn destroyed(&mut self, id: wlr::OutputId) {
        // `remove` on an unknown id, not indexing: this can name an output
        // this handler was never told about (see the library's own docs), and
        // a panic here aborts.
        if let Some(index) = self.output_ids.remove(&id) {
            self.outputs.remove(&index);
        }
    }
}

impl wlr::FdHandler for State {
    fn fd_ready(&mut self, source: wlr::SourceId, fd: std::os::fd::BorrowedFd<'_>, _readiness: wlr::Readiness) {
        // Drain whatever byte(s) woke this source before acting on it, for
        // every arm below: libwayland's loop is level-triggered, so a
        // handler that leaves data behind is called again every turn
        // forever.
        let mut buf = [0u8; 32];
        if Some(source) == self.shutdown_source {
            let _ = rustix::io::read(fd, &mut buf);
            tracing::info!("shutdown signal received");
            self.quitting = true;
            return;
        }
        if Some(source) == self.cmd_wake_source {
            let _ = rustix::io::read(fd, &mut buf);
            self.drain_pending_commands();
            return;
        }
        if Some(source) == self.config_reload_wake_source {
            let _ = rustix::io::read(fd, &mut buf);
            self.drain_config_reload();
        }
    }
}

impl wlr::LoopHandler for State {
    fn should_stop(&mut self) -> bool {
        // Not called from C — this is the one handler that may panic safely —
        // but it is still not a place to. `fd_ready`'s `cmd_wake_source` and
        // `config_reload_wake_source` arms above are what actually pull the
        // loop out of a blocked `dispatch(-1)` the instant either channel
        // has something; these two calls are a backstop that runs on every
        // turn regardless of *why* it woke, so a command or reload result
        // is never left sitting past whatever else already woke the loop
        // for an unrelated reason (a frame, input, the shutdown source).
        self.drain_pending_commands();
        self.drain_config_reload();
        self.quitting
    }
}

impl wlr::ToplevelHandler for State {
    fn new_toplevel(&mut self, toplevel: &wlr::Toplevel<'_>) {
        // Nothing is created in the model yet: at this point the client has
        // sent no buffer and no size, and it may never map at all. The model
        // row is created on `mapped`, which is the first moment a window
        // genuinely exists on screen — and the moment the smithay
        // implementation's `new_toplevel` was standing in for.
        let _ = toplevel;
    }

    fn initial_commit(&mut self, toplevel: &wlr::Toplevel<'_>) {
        // xdg-shell requires a configure here. Staging the model's
        // placeholder size means the client's very first buffer is already
        // the right size, rather than being resized one frame later --
        // except the size the client must be told is the *content* size,
        // not the model's frame size: `mapped`/`new_toplevel` hasn't run
        // yet, so there is no model window and no `client_decorations_requested`
        // to read, but a window's default SSD-or-not answer only depends on
        // `app_id` (`decoration::has_ssd`'s `requested: None` case), which
        // `Toplevel::app_id` already has at this point. Skipping
        // `content_rect` here would configure a default (SSD) window's
        // client one `TITLE_BAR_HEIGHT` too tall -- the exact one-frame-late
        // resize this comment already claims not to have.
        let Some(runtime) = self.wayland.runtime() else { return };
        let app_id = toplevel.app_id().unwrap_or_default();
        let ssd = crate::decoration::has_ssd(&app_id, None, false);
        let placeholder = icedtea_contract::Rectangle {
            x: 0,
            y: 0,
            width: PLACEHOLDER_SIZE.0,
            height: PLACEHOLDER_SIZE.1,
        };
        let content = crate::decoration::content_rect(placeholder, ssd);
        runtime.set_toplevel_size(toplevel.id(), content.width, content.height);
    }

    fn mapped(&mut self, toplevel: &wlr::Toplevel<'_>) {
        let id = toplevel.id();
        let key = crate::wayland::ToplevelKey::new(id);
        if let Some(window_id) = self.wayland.window_for(key) {
            // Remapped after an unmap: the model row survived, so this is a
            // visibility change rather than a new window.
            tracing::info!(?id, ?window_id, "toplevel remapped");
            self.sync_window_to_scene(window_id);
            self.emit_pending();
            return;
        }
        let app_id = toplevel.app_id().unwrap_or_default();
        let title = toplevel.title().unwrap_or_default();
        let pid = toplevel.pid().unwrap_or(0);
        tracing::info!(?id, %app_id, %title, pid, "toplevel mapped");
        self.new_toplevel(key, &app_id, &title, pid);
    }

    fn unmapped(&mut self, id: wlr::ToplevelId) {
        // An unmap is not a destroy: the client may map again with the same
        // id. Hide it, and let the model keep the row.
        //
        // Note what this deliberately does *not* do: the model itself has no
        // "unmapped" concept, only workspace visibility and minimization --
        // `window_manager.get(window)` still returns the row, `is_visible`
        // and `focused` are whatever they already were, and `is_backed`
        // stays `true`. `set_visible(window, false)` only reaches the
        // client's scene node; nothing here clears the model's own focus
        // pointer. That is fine as long as nothing routes input to an
        // unmapped surface -- but it is a real trap for whichever task wires
        // the seat: keyboard focus must not be handed to a window this
        // handler just hid, and `sync_seat_focus`'s filters (visibility +
        // `is_backed`) do not know an unmapped-but-still-"visible"-by-model
        // window from a mapped one. Ledgered here for that task, not solved
        // by it.
        tracing::info!(?id, "toplevel unmapped");
        let key = crate::wayland::ToplevelKey::new(id);
        let Some(window) = self.wayland.window_for(key) else { return };
        self.wayland.set_visible(window, false);
    }

    fn title_changed(&mut self, toplevel: &wlr::Toplevel<'_>) {
        let id = toplevel.id();
        let key = crate::wayland::ToplevelKey::new(id);
        let title = toplevel.title().unwrap_or_default();
        tracing::info!(?id, %title, "toplevel title changed");
        self.toplevel_title_changed(key, &title);
    }

    fn toplevel_destroyed(&mut self, id: wlr::ToplevelId) {
        // Safe against an id we were never told about (the library documents
        // that this can happen): `forget_toplevel` resolves through the map
        // and returns early on a miss, and never indexes.
        tracing::info!(?id, "toplevel destroyed");
        self.forget_toplevel(crate::wayland::ToplevelKey::new(id));
    }
}
impl wlr::SeatHandler for State {}

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
    /// window it discards, drop its client bindings (no direct accessor,
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

        let (reload_tx, reload_rx) = crossbeam_channel::unbounded::<Config>();
        state.set_config_reload_sender(reload_tx);

        assert_eq!(state.handle_command(crate::dbus::DbCommand::ReloadConfig), Some(()));

        // The worker thread runs off-loop; block briefly for its result
        // (mirrors how `drain_config_reload` would pick it up on the next
        // turn).
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let mut received = None;
        while std::time::Instant::now() < deadline {
            match reload_rx.try_recv() {
                Ok(cfg) => {
                    received = Some(cfg);
                    break;
                }
                Err(crossbeam_channel::TryRecvError::Empty) => {
                    std::thread::sleep(std::time::Duration::from_millis(5))
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => break,
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

        let (reload_tx, reload_rx) = crossbeam_channel::unbounded::<Config>();
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
                Err(crossbeam_channel::TryRecvError::Empty) => {
                    std::thread::sleep(std::time::Duration::from_millis(5))
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => break,
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

    /// Re-review Important 1: `DbCommand::Minimize` goes through the same
    /// implementation as the title-bar button, so minimizing the focused
    /// window over D-Bus (the taskbar path) hands focus to the workspace's
    /// MRU successor instead of leaving the model's focus pointer on a
    /// now-invisible window and the keyboard dead.
    #[test]
    fn dbus_minimize_of_focused_window_hands_focus_to_successor() {
        let (mut state, _rx) = state_with_output(1000, 800);
        let geo = Rectangle { x: 10, y: 10, width: 300, height: 200 };
        let a = state.window_manager.add_window("app", "a", 1, geo);
        let b = state.window_manager.add_window("app", "b", 2, geo);
        state.window_manager.focus(b).unwrap();

        state.handle_command(crate::dbus::DbCommand::Minimize(b, true)).unwrap();
        assert!(state.window_manager.get(b).unwrap().minimized);
        assert_eq!(
            state.window_manager.focused_window().map(|w| w.id),
            Some(a),
            "focus handed to the MRU successor, matching the title-bar button"
        );
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

    // --- Re-review fix round: New-1 / New-2 / New-3 ---
    //
    // These cover the *model* half of the focus-sync work plus the fact that
    // every focus path now runs `sync_focus_change`/`sync_seat_focus` without
    // panicking. The Wayland half proper -- that the losing window's client
    // is staged `Activated = false`, that the successor's client is staged
    // `Activated = true`, and that `wayland.keyboard_focus(Some(id))` is
    // called -- is **not** observable here: no window in this module's
    // tests is ever bound to a toplevel (`state.wayland.is_backed` is false
    // for all of them), so `sync_window_to_scene` short-circuits before
    // touching client state and `wayland.keyboard_focus` (a no-op in this
    // commit regardless) never fires. See the fix report for the
    // manual-trace argument that stands in for those.

    /// New-2: the successor `forget_window` picks is reconciled, not just
    /// chosen -- and the model never ends up with a dangling focus pointer.
    #[test]
    fn closing_the_focused_window_reconciles_the_mru_successor() {
        let (mut state, _rx) = state_with_output(1000, 800);
        let a = state.window_manager.add_window("a", "a", 1, DEFAULT_GEO);
        let b = state.window_manager.add_window("b", "b", 2, DEFAULT_GEO);
        assert_eq!(state.focused_id(), Some(b));

        state.request_close(b);
        assert_eq!(state.focused_id(), Some(a), "the successor is focused, not left dangling");
        assert!(state.window_manager.get(a).unwrap().focused);
    }

    /// New-2/New-3: with no successor left there is nothing to activate, and
    /// the model's focus pointer ends up cleared rather than pointing at the
    /// destroyed window.
    #[test]
    fn closing_the_last_window_leaves_nothing_focused_and_clears_the_seat() {
        let (mut state, _rx) = state_with_output(1000, 800);
        let id = state.window_manager.add_window("a", "a", 1, DEFAULT_GEO);
        state.request_close(id);
        assert_eq!(state.focused_id(), None);
    }

    /// New-1: a focus change reports the window that *lost* focus as
    /// unfocused in the model, which is the state `sync_focus_change` then
    /// pushes to that window's client as `Activated = false`.
    #[test]
    fn click_to_focus_unfocuses_the_previous_window() {
        let (mut state, _rx) = state_with_output(1000, 800);
        let a = state.window_manager.add_window("a", "a", 1, DEFAULT_GEO);
        let b = state.window_manager.add_window("b", "b", 2, DEFAULT_GEO);
        assert!(state.window_manager.get(b).unwrap().focused);

        state.handle_pointer(PointerEvent::Press { id: a, pointer: (5, 5) });
        assert!(state.window_manager.get(a).unwrap().focused);
        assert!(!state.window_manager.get(b).unwrap().focused, "the old focus is dropped");
    }

    /// New-3: switching to an empty workspace leaves nothing focused, and
    /// `sync_scene`'s trailing `sync_seat_focus` runs even though the
    /// per-window loop has nothing visible to say about it.
    #[test]
    fn switching_to_an_empty_workspace_clears_focus_and_the_seat() {
        let (mut state, _rx) = state_with_output(1000, 800);
        let id = state.window_manager.add_window("a", "a", 1, DEFAULT_GEO);
        assert_eq!(state.focused_id(), Some(id));

        state.switch_workspace(1).unwrap();
        assert_eq!(state.focused_id(), None, "workspace 2 is empty");

        // And switching back re-focuses the workspace's MRU head.
        state.switch_workspace(0).unwrap();
        assert_eq!(state.focused_id(), Some(id));
    }

    /// New-3: minimizing the focused window hands focus on when there is a
    /// successor, and leaves nothing focused when there isn't.
    #[test]
    fn minimizing_the_focused_window_moves_focus_on() {
        let (mut state, _rx) = state_with_output(1000, 800);
        let a = state.window_manager.add_window("a", "a", 1, DEFAULT_GEO);
        let b = state.window_manager.add_window("b", "b", 2, DEFAULT_GEO);

        state.apply_decoration_action(b, crate::decoration::DecorationAction::Minimize);
        assert!(state.window_manager.get(b).unwrap().minimized);
        assert_eq!(state.focused_id(), Some(a));

        state.apply_decoration_action(a, crate::decoration::DecorationAction::Minimize);
        // With no successor the model leaves its focus pointer on `a` (that
        // is pre-existing `focus_mru_in_workspace` behaviour: it clears
        // nothing when it finds nothing). What must *not* happen is the
        // seat -- were there a real one -- going on delivering keys to a
        // window the user can no longer see: `sync_seat_focus` gates on
        // `is_visible_id`, so an invisible focus is the same as no focus as
        // far as the keyboard is concerned.
        assert!(!state.window_manager.is_visible_id(a));
    }

    /// New-4: the inset the sync path applies is the shared
    /// `decoration::content_rect`, keyed off the same `has_ssd` predicate the
    /// renderer uses -- so a decorated window's client is configured a title
    /// bar shorter and a title bar lower, and a CSD one is left alone.
    /// (`sync_window_to_scene` itself can't be observed without a real
    /// toplevel; this pins the geometry contract it consumes.)
    #[test]
    fn ssd_inset_applies_to_decorated_windows_only() {
        use crate::decoration::{content_rect, has_ssd, TITLE_BAR_HEIGHT};
        let (mut state, _rx) = state_with_output(1000, 800);
        let geo = Rectangle { x: 10, y: 20, width: 400, height: 300 };
        let ssd = state.window_manager.add_window("org.example.Ssd", "t", 1, geo);
        let csd = state.window_manager.add_window("org.gtk.Csd", "t", 2, geo);

        let w = state.window_manager.get(ssd).unwrap();
        assert!(has_ssd(&w.app_id, w.client_decorations_requested, w.fullscreen));
        assert_eq!(
            content_rect(w.geometry, true),
            Rectangle { x: 10, y: 20 + TITLE_BAR_HEIGHT, width: 400, height: 300 - TITLE_BAR_HEIGHT }
        );

        let w = state.window_manager.get(csd).unwrap();
        assert!(!has_ssd(&w.app_id, w.client_decorations_requested, w.fullscreen));
        assert_eq!(content_rect(w.geometry, false), geo, "CSD windows are untouched");

        // Fullscreen drops the strip, so the client gets the whole output.
        state.set_fullscreen_target(ssd, true).unwrap();
        let w = state.window_manager.get(ssd).unwrap();
        assert!(!has_ssd(&w.app_id, w.client_decorations_requested, w.fullscreen));
        assert_eq!(content_rect(w.geometry, false), w.geometry);
    }
}
