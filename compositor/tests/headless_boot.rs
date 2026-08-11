//! The compositor boots and runs a bounded number of event-loop turns
//! against wlroots' headless backend — with no display, no GPU and no seat,
//! so this runs in CI.
//!
//! The smithay baseline had no equivalent: nothing in it could be driven
//! without a winit window. `a_headless_compositor_boots_runs_and_stops` is
//! the test that would have caught a boot that silently listens on a socket
//! nothing renders to: it asserts the headless backend's output actually
//! reached the model's geometry map with a real size. It does *not* assert
//! that a frame was ever rendered or a scene commit accepted --
//! `OutputHandler::frame`/`Runtime::commit_output` aren't observable from
//! outside `State` today, so proving that would need its own
//! instrumentation, which is out of scope here. `the_shutdown_source_stops_the_loop`
//! and `a_dbus_command_wakes_an_idle_loop_via_its_wake_pipe` cover the two
//! ways something outside the loop asks it to act while it's blocked: a
//! real signal, and a `crossbeam_channel` send nudged through a wake pipe.

use icedtea_compositor::state::State;

/// Set the headless-backend environment exactly once, no matter which of
/// this binary's `#[test]`s reaches it first.
///
/// libtest runs the tests in this file on separate threads by default, and
/// each needs `WLR_BACKENDS=headless` before `Backend::autocreate` reads it —
/// two concurrent `env::set_var` calls (or a set racing a read from another
/// thread's `autocreate`) is exactly the torn-environment hazard the
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

    let display = wlr::Display::new().expect("display");
    let backend = wlr::Backend::autocreate(&display.event_loop()).expect("backend");
    let runtime = wlr::Runtime::new().expect("runtime");
    runtime.init_graphics(&display, &backend).expect("graphics");

    // `state` is declared (and so, by ordinary end-of-scope drop order, is
    // dropped) after `display`/`backend`/`runtime`: `attach` below gives
    // `state.wayland` a `Runtime` clone, and `wlr` documents that a
    // `Runtime` must not outlive the `Display` it was initialized against.
    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut state = State::new(icedtea_config::default_config(), tx);
    state.wayland.attach(runtime.clone());

    let background = runtime
        .add_rect(1, 1, icedtea_compositor::render::wallpaper_color(&state.config.appearance))
        .expect("background rect");
    runtime.lower_rect_to_bottom(background);
    state.set_background(background);

    // No shutdown source here on purpose: this test asserts nothing about
    // shutdown (that's `the_shutdown_source_stops_the_loop`, below), and
    // `signal_hook::low_level::pipe::register` is a process-wide registry --
    // every registered pipe gets the byte on every delivery of the signal it
    // watches. If this test also registered one, the shutdown test's
    // `raise(SIGINT)` on a parallel libtest thread would write into this
    // test's pipe too, and if the two tests' `run_all` windows overlapped,
    // `fd_ready` would set `quitting` here and flake the assertion below.
    // Bounded (`Turns`, not `Until::Stop`) is what makes that safe to skip:
    // nothing else in this test asks the loop to stop either.
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

    let display = wlr::Display::new().expect("display");
    let backend = wlr::Backend::autocreate(&display.event_loop()).expect("backend");
    let runtime = wlr::Runtime::new().expect("runtime");

    // See `a_headless_compositor_boots_runs_and_stops` for why `state` is
    // declared after `display`/`backend`/`runtime`.
    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut state = State::new(icedtea_config::default_config(), tx);
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

/// The fix this test exists for: with nothing but `dispatch(-1)` and no fd
/// source of its own, a `crossbeam_channel` send from another thread is
/// invisible to a blocked loop. `run_all` below is called with
/// `Until::Stop`, not a bounded `Turns`, deliberately -- if the wake pipe
/// weren't registered and nudged, this test would hang rather than fail an
/// assertion, which is the correct failure mode for "the loop never woke
/// up."
#[test]
fn a_dbus_command_wakes_an_idle_loop_via_its_wake_pipe() {
    ensure_headless_env();

    let display = wlr::Display::new().expect("display");
    let backend = wlr::Backend::autocreate(&display.event_loop()).expect("backend");
    let runtime = wlr::Runtime::new().expect("runtime");

    // See `a_headless_compositor_boots_runs_and_stops` for why `state` is
    // declared after `display`/`backend`/`runtime`.
    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut state = State::new(icedtea_config::default_config(), tx);
    state.wayland.attach(runtime.clone());

    let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded();
    state.set_command_receiver(cmd_rx);
    let (wake_write, wake_id) = icedtea_compositor::backend::wake_source(&runtime).expect("wake source");
    state.set_cmd_wake_source(wake_id);

    // From another thread, exactly like the real producers
    // (`dbus::WmInterface::send`, `State::spawn_config_reload`): send onto
    // the channel, then nudge the wake pipe.
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(50));
        let _ = cmd_tx.send(icedtea_compositor::dbus::DbCommand::Quit);
        icedtea_compositor::backend::wake(&wake_write);
    });

    backend
        .run_all(&display, &mut state, &runtime, wlr::Until::Stop)
        .expect("run_all");

    assert!(state.quitting, "the Quit command must have reached State through the wake pipe");
}
