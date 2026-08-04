pub mod backend;
pub mod decoration;
pub mod layout;
pub mod render;
pub mod state;
pub mod window;

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

    let (dbus_tx, _dbus_rx) = crossbeam_channel::unbounded::<Event>();
    // `_dbus_rx` has no consumer yet by design: Task 12's D-Bus thread is the
    // intended consumer of this channel and becomes the drain.

    let mut event_loop: EventLoop<State> = EventLoop::try_new().expect("failed to create the event loop");
    let mut state = State::new(config, dbus_tx);
    state.set_loop_signal(event_loop.get_signal());

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
    // model); the result comes back over a `calloop::channel`, which -- like
    // Task 13's config-reload channel above -- *is* a calloop event source
    // directly, so no `Generic`/fd wrapping is needed here.
    let wallpaper_channel = render::spawn_wallpaper_decode(state.config.appearance.wallpaper.clone());
    event_loop
        .handle()
        .insert_source(wallpaper_channel, |event, _, state: &mut State| {
            if let smithay::reexports::calloop::channel::Event::Msg(image) = event {
                state.wallpaper.set_decoded(image);
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
            let _ = state.display_handle.flush_clients();
        })
        .expect("event loop error");
}
