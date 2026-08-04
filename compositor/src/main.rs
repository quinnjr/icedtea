pub mod backend;
pub mod decoration;
pub mod layout;
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

    // Config I/O + JSON parse happens off the main thread; the main loop
    // blocks briefly on the single result rather than doing redb/JSON work
    // itself (see the plan's threading-model constraint).
    let (config_tx, config_rx) = crossbeam_channel::bounded(1);
    let db_path = icedtea_config::default_db_path();
    std::thread::spawn(move || {
        let _ = config_tx.send(icedtea_config::load_or_default(&db_path));
    });
    let config = config_rx.recv().expect("config worker finished");

    let (dbus_tx, _dbus_rx) = crossbeam_channel::unbounded::<Event>();

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
            // Safety: the display is owned by this closure and outlives every dispatch.
            unsafe {
                display.get_mut().dispatch_clients(state).unwrap();
            }
            Ok(PostAction::Continue)
        })
        .expect("failed to insert the wayland display source into the event loop");

    tracing::info!(socket = %socket_name, "listening on wayland socket");

    let _backend = if nested {
        Backend::init_nested(&mut state, &event_loop.handle())
    } else {
        Backend::init_drm()
    };

    event_loop
        .run(None, &mut state, |state| {
            let _ = state.display_handle.flush_clients();
        })
        .expect("event loop error");
}
