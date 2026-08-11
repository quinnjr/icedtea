//! Backend bootstrap. `Backend` wraps whichever smithay backend the
//! compositor is running on: nested inside an existing Wayland/X11 session
//! (`init_nested`, following anvil/smallvil's `winit.rs`) or standalone on a
//! DRM/libinput seat (`init_drm`, following anvil's `udev.rs`).
//!
//! Deviation from the task-7 brief: see the `NOTE` in `compositor/Cargo.toml`
//! -- there is no `backend_wayland` smithay feature in 0.7.0. "Nested" here
//! is smithay's `backend_winit`, exactly as anvil's own `--backend winit`
//! (its "nested" mode) and smallvil both use it; it creates a window on
//! whatever host display is available (Wayland or X11) and renders into it
//! via GL, which satisfies the brief's actual requirement ("a nested
//! compositor a Wayland client can connect to").
//!
//! `init_drm` is intentionally scoped down from anvil's ~1700-line
//! `udev.rs`: it opens the libseat session and confirms the seat's udev
//! devices are enumerable (enough for `Backend::Drm` to exist and be
//! extended), but does **not** implement DRM/GBM device selection, mode
//! setting, libinput registration, or the page-flip/scanout render loop --
//! that's a rendering-pipeline task of its own scope beyond "boot a smithay
//! compositor with window model and event fan-out", and isn't exercised by
//! any gate in this task (the only runtime smoke test the brief specifies is
//! `--nested`). Because the default (no `--nested`) path would otherwise
//! silently listen on a wayland socket with nothing behind it, `init_drm`
//! logs a loud `tracing::error!` saying so rather than pretending to work.

use smithay::backend::input::{
    AbsolutePositionEvent, ButtonState, Event as InputEventTrait, InputEvent, KeyState, KeyboardKeyEvent,
    PointerButtonEvent,
};
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::session::libseat::LibSeatSession;
use smithay::backend::session::Session;
use smithay::backend::udev::UdevBackend;
use smithay::backend::winit::{self, WinitEvent, WinitInput};
use smithay::input::keyboard::FilterResult;
use smithay::output::{Mode as OutputMode, PhysicalProperties, Subpixel};
use smithay::reexports::calloop::LoopHandle;
use smithay::utils::{Rectangle, Transform, SERIAL_COUNTER};

use crate::input as icedtea_input;
use crate::render;
use crate::state::{PointerEvent, State};

/// A running compositor backend.
pub enum Backend {
    /// Nested inside an existing Wayland/X11 session via smithay's winit backend.
    Nested,
    /// Standalone on a DRM/libinput seat (a TTY session).
    Drm(DrmBackend),
}

/// Session handle for the DRM backend. Device/render-node enumeration and
/// the actual mode-setting/scanout loop are follow-up work (see module
/// docs) -- there is deliberately no output, no input, and no calloop
/// source registered yet, so `init_drm` logs loudly rather than pretending
/// to be a working compositor.
pub struct DrmBackend {
    pub seat_name: String,
    /// Kept alive so the seat session isn't released while `Backend::Drm` is
    /// held; not read yet because nothing renders through it (see above).
    _session: LibSeatSession,
}

impl Backend {
    /// Boot inside an existing compositor session using smithay's winit
    /// backend, wires its render/input events into the calloop loop, and
    /// sets `WAYLAND_DISPLAY` so child clients launched after this call
    /// connect to our nested socket (mirrors `smallvil/src/winit.rs:48`).
    ///
    /// `socket_name` must be set *after* `winit::init()` returns: winit needs
    /// the host's own `WAYLAND_DISPLAY` (to open the nested window against
    /// the host compositor) before we clobber the env var with our own
    /// socket for child clients.
    pub fn init_nested(state: &mut State, loop_handle: &LoopHandle<'static, State>, socket_name: &str) -> Self {
        let (mut winit_backend, winit_source) =
            winit::init().expect("failed to initialize the winit backend");

        // SAFETY: single-threaded at this point in startup (before
        // `event_loop.run`), so no other thread can observe a torn read of
        // the environment.
        unsafe {
            std::env::set_var("WAYLAND_DISPLAY", socket_name);
        }

        let mode = OutputMode { size: winit_backend.window_size(), refresh: 60_000 };
        let output = state.create_output(
            "nested-0",
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "icedtea".into(),
                model: "nested".into(),
            },
            mode,
            // winit/GL framebuffers are Y-flipped relative to the compositor's
            // logical space (smallvil/src/winit.rs:40, anvil/src/winit.rs:125).
            Transform::Flipped180,
        );

        let mut damage_tracker = OutputDamageTracker::from_output(&output);
        let render_output = output.clone();

        loop_handle
            .insert_source(winit_source, move |event, _, state: &mut State| match event {
                WinitEvent::Resized { size, .. } => {
                    render_output.change_current_state(
                        Some(OutputMode { size, refresh: 60_000 }),
                        None,
                        None,
                        None,
                    );
                    // Keep `state.outputs` (snap/fullscreen/pointer-motion's
                    // geometry source, populated by `create_output`) in sync
                    // so a resized nested window doesn't leave those acting
                    // on the stale boot-time size.
                    if let Some(o) = state.outputs.get_mut(&0) {
                        o.geometry.width = size.w;
                        o.geometry.height = size.h;
                    }
                }
                WinitEvent::Input(input_event) => {
                    process_winit_input(state, input_event);
                }
                WinitEvent::Redraw => {
                    let size = winit_backend.window_size();
                    let damage = Rectangle::from_size(size);
                    // Review finding I1: only the active workspace's
                    // non-minimized windows are drawn -- this list feeds the
                    // SSD title-bar/button elements, and `Space` itself is
                    // kept in step by `State::sync_space` (which unmaps
                    // everything else).
                    let windows: Vec<&crate::window::Window> = state.window_manager.visible_windows();
                    match winit_backend.bind() {
                        Ok((renderer, mut framebuffer)) => {
                            if let Err(err) = render::draw_frame(
                                renderer,
                                &mut framebuffer,
                                &mut damage_tracker,
                                &render_output,
                                &state.space,
                                &windows,
                                &state.config.appearance,
                                &mut state.wallpaper,
                                state.snap_preview,
                                0,
                            ) {
                                tracing::warn!("nested render_output failed: {err:?}");
                            }
                        }
                        Err(err) => tracing::warn!("nested winit bind failed: {err:?}"),
                    }
                    if let Err(err) = winit_backend.submit(Some(&[damage])) {
                        tracing::warn!("nested winit submit failed: {err:?}");
                    }

                    state.space.elements().for_each(|window| {
                        window.send_frame(&render_output, state.start_time.elapsed(), Some(std::time::Duration::ZERO), |_, _| {
                            Some(render_output.clone())
                        });
                    });
                    state.space.refresh();
                    state.popups.cleanup();
                    winit_backend.window().request_redraw();
                }
                WinitEvent::CloseRequested => {
                    state.stop();
                }
                _ => {}
            })
            .expect("failed to insert the winit event source into the event loop");

        Backend::Nested
    }

    /// Boot standalone on a DRM/libinput seat session (a TTY, no host
    /// compositor). Opens the seat session so `Backend::Drm` exists and can
    /// be extended, but does **not** yet create an output, register input,
    /// or register any calloop source -- there is no scanout pipeline behind
    /// this call. Logs an error rather than silently returning a backend
    /// that looks alive but shows nothing on screen; see module docs for the
    /// full scope note.
    pub fn init_drm() -> Self {
        let (session, _notifier) = LibSeatSession::new().expect("failed to open a libseat session");
        let seat_name = session.seat();

        // Just proving the udev device list is reachable; nothing is kept or
        // registered from it yet (see the `tracing::error!` below).
        if let Err(err) = UdevBackend::new(&seat_name) {
            tracing::error!("failed to enumerate udev devices for seat {seat_name}: {err}");
        }

        tracing::error!(
            seat = %seat_name,
            "DRM backend is not implemented yet: no output, no input, and no scanout pipeline are \
             registered. This process is listening on a wayland socket that nothing will ever render \
             to. Run with --nested for a working (winit-backed) compositor until DRM scanout lands."
        );

        Backend::Drm(DrmBackend { seat_name, _session: session })
    }
}

/// Route a winit `InputEvent` to `State::handle_key`/`State::handle_pointer`.
///
/// Task 11 review #5 (human-ruling scope extension: the brief's file list
/// excludes `backend.rs`, overridden here): previously `WinitEvent::Input`
/// was entirely discarded ("Routing input into the seat lands in a later
/// task"), which meant nothing in the plan ever actually called `handle_key`
/// or `handle_pointer` -- Task 11's dispatch/drag logic was unit-tested but
/// unreachable at runtime. This is deliberately thin: it only translates
/// winit/xkb event shapes into the types `State`'s existing dispatchers
/// already accept and calls them; no keybinding/drag/snap decision-making
/// lives here.
fn process_winit_input(state: &mut State, event: InputEvent<WinitInput>) {
    match event {
        InputEvent::Keyboard { event, .. } => {
            let serial = SERIAL_COUNTER.next_serial();
            let time = InputEventTrait::time_msec(&event);
            let keycode = event.key_code();
            let key_state = event.state();
            let Some(keyboard) = state.seat.get_keyboard() else { return };
            keyboard.input::<(), _>(state, keycode, key_state, serial, time, |data, mods, handle| {
                // Item 1's chosen end condition: alt-tab ends when the held
                // modifier (SUPER, per the default `cycle:alt_tab` binding)
                // is no longer down, regardless of which specific key this
                // event is for -- covers releasing the modifier first or
                // last relative to Tab.
                if data.alt_tab.is_active() && !mods.logo {
                    data.end_alt_tab();
                }
                if key_state == KeyState::Pressed {
                    let our_mods = to_icedtea_modifiers(mods);
                    let keysym =
                        resolve_keysym(handle.raw_latin_sym_or_raw_current_sym().map(|s| s.raw()), handle.modified_sym().raw());
                    if data.handle_key(our_mods, keysym).is_some() {
                        // Consumed by a compositor keybinding: don't forward
                        // to the focused client.
                        return FilterResult::Intercept(());
                    }
                }
                FilterResult::Forward
            });
        }
        InputEvent::PointerMotionAbsolute { event, .. } => {
            let Some(output_geo) = state.outputs.values().next().map(|o| o.geometry) else { return };
            let size: smithay::utils::Size<i32, smithay::utils::Logical> =
                (output_geo.width, output_geo.height).into();
            let pos = event.position_transformed(size);
            let pointer = (pos.x as i32 + output_geo.x, pos.y as i32 + output_geo.y);
            state.pointer_location = pointer;
            state.handle_pointer(PointerEvent::Motion { pointer });
        }
        InputEvent::PointerButton { event, .. } => {
            // BTN_LEFT only: decoration hit-testing/drag is a left-click
            // interaction (matches `WinitMouseInputEvent::button_code`'s
            // `0x110` for `WinitMouseButton::Left`).
            if event.button_code() != 0x110 {
                return;
            }
            let pointer = state.pointer_location;
            match event.state() {
                ButtonState::Pressed => {
                    // No client input-focus forwarding is wired yet (see
                    // module docs), so "which window is under the pointer"
                    // is answered from our own window model rather than
                    // smithay's `Space`: the MRU-first order
                    // `visible_windows()` returns is a reasonable
                    // topmost-first stand-in. Review finding I1: it is
                    // `visible_windows()` (active workspace, not minimized)
                    // rather than the full list, so a click can no longer
                    // focus or close a window on an inactive workspace.
                    let id = state.window_manager.window_at(pointer).map(|w| w.id);
                    if let Some(id) = id {
                        state.handle_pointer(PointerEvent::Press { id, pointer });
                    }
                }
                ButtonState::Released => {
                    state.handle_pointer(PointerEvent::Release { pointer });
                }
            }
        }
        _ => {}
    }
}

/// Translate smithay's keyboard modifier state (as tracked by the seat's
/// xkb state) to `input::Modifiers`. `logo` is the "Super"/"Windows" key,
/// matching this project's `SUPER` binding token
/// (`config/src/defaults.rs`).
fn to_icedtea_modifiers(mods: &smithay::input::keyboard::ModifiersState) -> icedtea_input::Modifiers {
    let mut out = icedtea_input::Modifiers::empty();
    if mods.logo {
        out |= icedtea_input::Modifiers::SUPER;
    }
    if mods.ctrl {
        out |= icedtea_input::Modifiers::CTRL;
    }
    if mods.alt {
        out |= icedtea_input::Modifiers::ALT;
    }
    if mods.shift {
        out |= icedtea_input::Modifiers::SHIFT;
    }
    out
}

/// Pick which keysym to feed to `handle_key`/`input::match_action`.
///
/// Task 11 re-review #1: the previous code always used
/// `KeysymHandle::modified_sym()`, which applies the keyboard's active
/// shift level. `input::key_name_to_keysym` (and every default binding in
/// `config/src/defaults.rs`) encodes the *unshifted* keysym for a key --
/// `"KEY_q"` is `0x71` (lowercase `q`). But `SUPER+SHIFT+q` (the default
/// `quit` binding) has shift held, so `modified_sym()` for that keypress is
/// `0x51` (`XK_Q`, uppercase) -- it can never equal `0x71`, so `quit` (and
/// `reload`, `SUPER+SHIFT+r`) were unreachable. The same mismatch happens
/// with Caps Lock active on plain `SUPER+q`/`SUPER+f`. Preferring
/// `raw_latin_sym_or_raw_current_sym()` -- which is deliberately
/// shift/caps-lock-agnostic (smithay's own doc: "handy to implement layout
/// agnostic bindings") -- fixes this; it's `None` only when the keycode
/// doesn't produce a keysym at all, in which case `modified_sym()` is used
/// as a fallback so a key press is never silently dropped.
///
/// Factored out as a pure function (rather than inlined at the call site)
/// specifically so it has a unit-testable seam: `KeysymHandle` itself can't
/// be constructed outside a live xkb session, so nothing in this file can
/// unit-test the real event path, but the "which of these two already-read
/// keysyms wins" decision can be.
fn resolve_keysym(raw_latin: Option<u32>, modified_fallback: u32) -> u32 {
    raw_latin.unwrap_or(modified_fallback)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_keysym_prefers_raw_latin_over_shifted_modified_sym() {
        // The actual bug: modified_sym for SUPER+SHIFT+q is XK_Q (0x51),
        // not the 0x71 every binding is keyed on. raw_latin_sym is
        // shift-agnostic and reports 0x71 for the same physical key.
        assert_eq!(resolve_keysym(Some(0x71), 0x51), 0x71);
    }

    #[test]
    fn resolve_keysym_falls_back_when_raw_latin_is_unavailable() {
        // `raw_latin_sym_or_raw_current_sym()` returns `None` only when the
        // keycode produces no keysym at all; `modified_sym()` is still used
        // rather than silently dropping the key press.
        assert_eq!(resolve_keysym(None, 0xff0d), 0xff0d);
    }
}
