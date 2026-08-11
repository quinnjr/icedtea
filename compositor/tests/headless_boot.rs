//! The compositor boots, runs a bounded number of event-loop turns against
//! wlroots' headless backend, and shuts down cleanly — with no display, no
//! GPU and no seat, so this runs in CI.
//!
//! The smithay baseline had no equivalent: nothing in it could be driven
//! without a winit window. This is the test that would have caught a boot
//! that silently listens on a socket nothing renders to.

use icedtea_compositor::state::State;

/// Set the headless-backend environment exactly once, no matter which of
/// this binary's two `#[test]`s reaches it first.
///
/// libtest runs both tests in this file on separate threads by default, and
/// both need `WLR_BACKENDS=headless` before `Backend::autocreate` reads it —
/// two concurrent `env::set_var` calls (or a set racing a read from the
/// other thread's `autocreate`) is exactly the torn-environment hazard the
/// project's unsafe policy exception (c) requires ruling out. `Once::call_once`
/// makes that true structurally: at most one thread ever runs the block
/// below, and every other caller blocks until it has returned, so no thread
/// can observe or cause a torn read.
fn ensure_headless_env() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        // SAFETY (icedtea unsafe exception (c)): `Once::call_once` above
        // guarantees this closure runs on exactly one thread and that every
        // other thread calling `ensure_headless_env` blocks until it
        // finishes -- so nothing can observe a torn read and nothing races
        // this write.
        unsafe {
            std::env::set_var("WLR_BACKENDS", "headless");
            std::env::set_var("WLR_HEADLESS_OUTPUTS", "1");
        }
    });
}

#[test]
fn a_headless_compositor_boots_runs_and_stops() {
    ensure_headless_env();

    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut state = State::new(icedtea_config::default_config(), tx);

    let display = wlr::Display::new().expect("display");
    let backend = wlr::Backend::autocreate(&display.event_loop()).expect("backend");
    let runtime = wlr::Runtime::new().expect("runtime");
    runtime.init_graphics(&display, &backend).expect("graphics");
    state.wayland.attach(runtime.clone());

    let background = runtime
        .add_rect(1, 1, icedtea_compositor::render::wallpaper_color(&state.config.appearance))
        .expect("background rect");
    runtime.lower_rect_to_bottom(background);
    state.set_background(background);

    let source = icedtea_compositor::backend::shutdown_source(&runtime).expect("shutdown source");
    state.set_shutdown_source(source);

    // Bounded rather than `Until::Stop`: nothing in a test sends this process
    // a signal, so a blocking run would never return.
    backend
        .run_all(&display, &mut state, &runtime, wlr::Until::Turns(8))
        .expect("run_all");

    assert_eq!(
        state.outputs.len(),
        1,
        "the headless backend's output must have reached the model's geometry map"
    );
    let geo = state.outputs.get(&0).expect("output 0").geometry;
    assert!(
        geo.width > 0 && geo.height > 0,
        "an enabled output reports a real size, got {geo:?}"
    );
    assert!(
        !state.quitting,
        "nothing asked the compositor to quit, so the stop flag must be clear"
    );
}

#[test]
fn the_shutdown_source_stops_the_loop() {
    ensure_headless_env();

    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut state = State::new(icedtea_config::default_config(), tx);

    let display = wlr::Display::new().expect("display");
    let backend = wlr::Backend::autocreate(&display.event_loop()).expect("backend");
    let runtime = wlr::Runtime::new().expect("runtime");
    state.wayland.attach(runtime.clone());

    // Raise the real signal at this process: `signal_hook` writes a byte to
    // the pipe from its handler, the loop wakes on it, and `fd_ready` sets
    // the stop flag. This exercises the whole path, not a stand-in for it.
    let source = icedtea_compositor::backend::shutdown_source(&runtime).expect("shutdown source");
    state.set_shutdown_source(source);
    signal_hook::low_level::raise(signal_hook::consts::SIGINT).expect("raise");

    backend
        .run_all(&display, &mut state, &runtime, wlr::Until::Turns(16))
        .expect("run_all");

    assert!(
        state.quitting,
        "SIGINT must reach the fd source and set the stop flag"
    );
}
