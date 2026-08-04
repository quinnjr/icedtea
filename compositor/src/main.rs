pub mod backend;
pub mod config_combo;
pub mod dbus;
pub mod decoration;
pub mod input;
pub mod layout;
pub mod render;
pub mod state;
pub mod window;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use icedtea_contract::Event;
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{EventLoop, Interest, Mode, PostAction};
use smithay::reexports::wayland_server::Display;

use backend::Backend;
use state::{ClientState, State};

fn main() {
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::from_default_env()).init();

    let nested = std::env::args().any(|a| a == "--nested");

    // The event loop isn't running yet, so there is nothing else useful to
    // overlap this with -- a synchronous load is correct here. Task 13 owns
    // the *async reload* mechanism (a `calloop::channel` source draining a
    // worker-thread result onto the main loop); this is boot-time only.
    let db_path = icedtea_config::default_db_path();
    let config = icedtea_config::load_or_default(&db_path);

    let (dbus_tx, dbus_events_rx) = crossbeam_channel::unbounded::<Event>();
    // `dbus_events_rx` is drained by the D-Bus service's emitter thread,
    // spawned below via `dbus::spawn_service` -- the consumer this channel
    // was created for back in task 7.

    let mut event_loop: EventLoop<State> = EventLoop::try_new().expect("failed to create the event loop");
    let mut state = State::new(config, dbus_tx);
    state.set_loop_signal(event_loop.get_signal());

    // The D-Bus service runs on its own thread (binding threading-model
    // ruling from task 7) and talks back to this loop only via `cmd_channel`
    // (see `dbus.rs`'s module doc for why that's a `calloop::channel`
    // `Sender`, not the brief's literal `crossbeam_channel::Sender`).
    let (cmd_tx, cmd_channel) = smithay::reexports::calloop::channel::channel::<dbus::DbCommand>();
    let dbus_quit_signal = Arc::new(AtomicBool::new(false));
    let _dbus_conn = dbus::spawn_service(dbus_events_rx, cmd_tx, dbus_quit_signal.clone());
    event_loop
        .handle()
        .insert_source(cmd_channel, |event, _, state: &mut State| {
            if let smithay::reexports::calloop::channel::Event::Msg(cmd) = event {
                state.handle_dbus_command(cmd);
            }
        })
        .expect("failed to insert the D-Bus command channel into the event loop");

    let display: Display<State> = state.take_display();

    let socket_source =
        smithay::wayland::socket::ListeningSocketSource::new_auto().expect("failed to create wayland socket");
    let socket_name = socket_source.socket_name().to_string_lossy().into_owned();
    event_loop
        .handle()
        .insert_source(socket_source, |client_stream, _, state: &mut State| {
            if let Err(err) =
                state.display_handle.insert_client(client_stream, Arc::new(ClientState::default()))
            {
                tracing::warn!("error adding wayland client: {err}");
            }
        })
        .expect("failed to insert the wayland socket source into the event loop");

    event_loop
        .handle()
        .insert_source(Generic::new(display, Interest::READ, Mode::Level), |_, display, state: &mut State| {
            // SAFETY: `Generic::get_mut`'s obligation is that the wrapped
            // `Display` is not dropped (nor its fd closed) while this source
            // remains registered with the loop. It stays alive inside this
            // closure's captured source data for as long as the source is
            // registered, and is only ever removed by dropping the whole
            // event loop, so that obligation holds.
            unsafe {
                if let Err(err) = display.get_mut().dispatch_clients(state) {
                    tracing::warn!("error dispatching wayland client requests: {err}");
                }
            }
            Ok(PostAction::Continue)
        })
        .expect("failed to insert the wayland display source into the event loop");

    tracing::info!(socket = %socket_name, "listening on wayland socket");

    // Wallpaper decode runs on a worker thread (multi-megapixel JPEG decode
    // is CPU-heavy and must stay off the render loop, per the threading
    // model); the result comes back over a `calloop::channel`, whose
    // `Channel<T>` *is* a calloop event source directly, so no
    // `Generic`/fd wrapping is needed here (see `render.rs`'s module doc
    // for why this is `calloop::channel` and not a bare
    // `crossbeam_channel::Receiver`).
    let mut wallpaper_decoded = false;
    let wallpaper_channel = render::spawn_wallpaper_decode(state.config.appearance.wallpaper.clone());
    event_loop
        .handle()
        .insert_source(wallpaper_channel, move |event, _, state: &mut State| match event {
            smithay::reexports::calloop::channel::Event::Msg(image) => {
                wallpaper_decoded = true;
                state.wallpaper.set_decoded(image);
            }
            smithay::reexports::calloop::channel::Event::Closed => {
                // The sender side is dropped once the worker thread ends,
                // which happens both on the normal "sent a result, thread
                // exits" path and on a panic. Only the panic case -- closed
                // without ever having sent -- is worth a log; the normal
                // case would otherwise spam a misleading warning on every
                // boot.
                if !wallpaper_decoded {
                    tracing::warn!(
                        "wallpaper decode worker thread exited without producing a result \
                         (it likely panicked); wallpaper stays solid-color"
                    );
                }
            }
        })
        .expect("failed to insert the wallpaper decode channel into the event loop");

    let _backend = if nested {
        Backend::init_nested(&mut state, &event_loop.handle(), &socket_name)
    } else {
        Backend::init_drm()
    };

    event_loop
        .run(None, &mut state, |state| {
            // `apply_action("quit")` (bound to a keybinding by default) only
            // sets the flag -- it can't call `state.stop()` itself without
            // depending on the calloop signal that lives on the loop this
            // closure runs inside, so this is where the flag actually stops
            // the loop.
            if state.quitting {
                state.stop();
            }
            let _ = state.display_handle.flush_clients();
        })
        .expect("event loop error");

    // Let the D-Bus emitter thread's polling `recv_timeout` (see
    // `dbus.rs`'s module doc) notice the shutdown and exit its loop rather
    // than being severed mid-`emit_signal` when the process exits.
    dbus_quit_signal.store(true, Ordering::Relaxed);
}
