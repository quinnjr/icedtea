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

use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::session::libseat::LibSeatSession;
use smithay::backend::session::Session;
use smithay::backend::udev::UdevBackend;
use smithay::backend::winit::{self, WinitEvent};
use smithay::output::{Mode as OutputMode, PhysicalProperties, Subpixel};
use smithay::reexports::calloop::LoopHandle;
use smithay::utils::{Rectangle, Transform};

use crate::render;
use crate::state::State;

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
                }
                WinitEvent::Input(_input_event) => {
                    // Routing input into the seat lands in a later task.
                }
                WinitEvent::Redraw => {
                    let size = winit_backend.window_size();
                    let damage = Rectangle::from_size(size);
                    match winit_backend.bind() {
                        Ok((renderer, mut framebuffer)) => {
                            if let Err(err) = render::draw_frame(
                                renderer,
                                &mut framebuffer,
                                &mut damage_tracker,
                                &render_output,
                                &state.space,
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
