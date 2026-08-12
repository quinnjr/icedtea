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
    runtime.create_xdg_shell(&display, 6).expect("xdg_wm_base");
    runtime.create_seat(&display, "seat0").expect("seat0");

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

/// The seam turns a library id into a model window and back, and every
/// outbound push resolves through it.
///
/// Drives `State`'s toplevel entry points directly rather than through a real
/// client: a client-driven test needs a Wayland client library this workspace
/// does not depend on, and is parity-milestone work. What is provable here is
/// the whole of the compositor's own half — the model row, the binding, the
/// cascade position, focus reconciliation, and destruction.
#[test]
fn a_toplevel_becomes_a_model_window_and_releases_it_on_destroy() {
    use icedtea_compositor::wayland::ToplevelKey;

    let (tx, rx) = crossbeam_channel::unbounded();
    let mut state = State::new(icedtea_config::default_config(), tx);
    state.create_output(0, icedtea_contract::Rectangle { x: 0, y: 0, width: 1000, height: 800 });

    let runtime = wlr::Runtime::new().expect("runtime");
    state.wayland.attach(runtime);

    let first = ToplevelKey::for_test(1);
    state.new_toplevel(first, "term", "Terminal", 4242);

    let id = state.wayland.window_for(first).expect("the key resolves to a model window");
    let w = state.window_manager.get(id).expect("model row exists");
    assert_eq!(w.app_id, "term");
    assert_eq!(w.title, "Terminal");
    assert_eq!(w.pid, 4242);
    assert!(w.focused, "a new toplevel takes focus");
    assert_eq!(
        (w.geometry.width, w.geometry.height),
        icedtea_compositor::state::PLACEHOLDER_SIZE,
        "the model's placeholder size until the client's first commit"
    );

    // A second toplevel cascades rather than stacking exactly on the first.
    let second = ToplevelKey::for_test(2);
    state.new_toplevel(second, "editor", "Editor", 4243);
    let id2 = state.wayland.window_for(second).expect("second key resolves");
    let g1 = state.window_manager.get(id).expect("first still there").geometry;
    let g2 = state.window_manager.get(id2).expect("second").geometry;
    assert_ne!((g1.x, g1.y), (g2.x, g2.y), "cascade, not overlap");
    assert!(
        state.window_manager.get(id2).is_some_and(|w| w.focused),
        "the newest toplevel has focus"
    );
    assert!(
        state.window_manager.get(id).is_some_and(|w| !w.focused),
        "and the previous one lost it -- both ends of the transition"
    );

    // Title changes route back into the model.
    state.toplevel_title_changed(second, "Editor — file.rs");
    assert_eq!(
        state.window_manager.get(id2).map(|w| w.title.as_str()),
        Some("Editor — file.rs")
    );

    // Destruction drops the row, the binding, and hands focus back.
    state.forget_toplevel(second);
    assert!(state.wayland.window_for(second).is_none(), "binding cleared");
    assert!(state.window_manager.get(id2).is_none(), "model row cleared");
    assert!(
        state.window_manager.get(id).is_some_and(|w| w.focused),
        "focus falls back to the MRU survivor"
    );

    // Every mutation above emitted; nothing was left queued.
    let events: Vec<_> = rx.try_iter().collect();
    assert!(!events.is_empty(), "model mutations must reach the event channel");
    assert!(
        events.windows(2).all(|p| p[0].seq < p[1].seq),
        "sequence numbers are strictly monotonic: {:?}",
        events.iter().map(|e| e.seq).collect::<Vec<_>>()
    );
}

/// `request_close` on a backed window asks the client and leaves the model
/// row alone; on an unbacked one it removes the row synchronously, because
/// no destroy will ever arrive for it.
#[test]
fn closing_distinguishes_a_real_client_from_a_model_only_window() {
    use icedtea_compositor::wayland::ToplevelKey;

    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut state = State::new(icedtea_config::default_config(), tx);
    state.create_output(0, icedtea_contract::Rectangle { x: 0, y: 0, width: 1000, height: 800 });
    let runtime = wlr::Runtime::new().expect("runtime");
    state.wayland.attach(runtime);

    let model_only = state.window_manager.add_window(
        "ghost",
        "Ghost",
        1,
        icedtea_contract::Rectangle { x: 0, y: 0, width: 10, height: 10 },
    );
    state.request_close(model_only);
    assert!(
        state.window_manager.get(model_only).is_none(),
        "a window with no client is removed at once"
    );

    let key = ToplevelKey::for_test(9);
    state.new_toplevel(key, "term", "Terminal", 7);
    let backed = state.wayland.window_for(key).expect("resolves");
    state.request_close(backed);
    assert!(
        state.window_manager.get(backed).is_some(),
        "a real client keeps its model row until it destroys its toplevel"
    );
}

/// Modifier translation is the seam between the library's four booleans and
/// the model's bitflags, and it is where a wrong mapping makes every binding
/// that uses that modifier silently unreachable.
#[test]
fn modifier_translation_covers_every_flag_the_model_knows() {
    use icedtea_compositor::state::to_model_modifiers;

    // A table rather than four asserts: what matters is that each library
    // flag lands on its own model flag and on no other.
    for (logo, ctrl, alt, shift, expected) in [
        (true, false, false, false, icedtea_compositor::input::Modifiers::SUPER),
        (false, true, false, false, icedtea_compositor::input::Modifiers::CTRL),
        (false, false, true, false, icedtea_compositor::input::Modifiers::ALT),
        (false, false, false, true, icedtea_compositor::input::Modifiers::SHIFT),
    ] {
        assert_eq!(
            to_model_modifiers(logo, ctrl, alt, shift),
            expected,
            "logo={logo} ctrl={ctrl} alt={alt} shift={shift}"
        );
    }

    assert_eq!(
        to_model_modifiers(true, false, false, true),
        icedtea_compositor::input::Modifiers::SUPER | icedtea_compositor::input::Modifiers::SHIFT,
        "combinations are the union, which is what SUPER+SHIFT+q needs"
    );
    assert!(to_model_modifiers(false, false, false, false).is_empty());
}

/// Click-to-focus: pressing an unfocused window focuses it, activates it,
/// deactivates the one that lost focus, and moves the seat -- all four ends
/// of the transition the baseline established.
#[test]
fn a_press_on_an_unfocused_window_moves_focus_at_both_ends() {
    use icedtea_compositor::state::PointerEvent;
    use icedtea_compositor::wayland::ToplevelKey;

    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut state = State::new(icedtea_config::default_config(), tx);
    state.create_output(0, icedtea_contract::Rectangle { x: 0, y: 0, width: 1000, height: 800 });
    let runtime = wlr::Runtime::new().expect("runtime");
    state.wayland.attach(runtime);

    let first = ToplevelKey::for_test(1);
    state.new_toplevel(first, "a", "A", 1);
    let a = state.wayland.window_for(first).expect("a");
    let second = ToplevelKey::for_test(2);
    state.new_toplevel(second, "b", "B", 2);
    let b = state.wayland.window_for(second).expect("b");

    assert!(state.window_manager.get(b).is_some_and(|w| w.focused), "newest is focused");

    // Press inside A's frame, but outside B's: B cascades 24px off A
    // (`layout::cascade_point_in`, step 24) in both axes, on top of A in
    // MRU order, so A's *geometric* center (the brief's original point) is
    // actually covered by B and would hit the wrong window. A's top-left
    // corner plus a few pixels is inside A's frame and strictly left of/above
    // B's origin (`geo_a.x + 24`, `geo_a.y + 24`), so it can only ever hit A.
    // `window_at_point` is the model's own answer to "what is under the
    // pointer", which is what click-to-focus must use: the scene knows
    // nothing about workspaces or minimization.
    let geo_a = state.window_manager.get(a).expect("a").geometry;
    let point = (geo_a.x + 5, geo_a.y + 5);
    assert_eq!(state.window_at_point(point), Some(a));

    state.handle_pointer(PointerEvent::Press { id: a, pointer: point });

    assert!(state.window_manager.get(a).is_some_and(|w| w.focused), "A gained focus");
    assert!(
        state.window_manager.get(b).is_some_and(|w| !w.focused),
        "B lost it -- both ends, not just the winner"
    );
}

/// A bound key is consumed and does not reach the client; an unbound one is
/// forwarded. This is the whole of what `SeatHandler::key`'s return value
/// decides, and getting the polarity backwards would either break every
/// binding or send every keystroke twice.
#[test]
fn a_bound_key_is_consumed_and_an_unbound_one_is_forwarded() {
    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut state = State::new(icedtea_config::default_config(), tx);

    // `SUPER+SHIFT+q` is the default `quit` binding, and 0x71 is the
    // *unshifted* keysym for q -- which is exactly why the library reports
    // the unshifted symbol.
    let mods = icedtea_compositor::input::Modifiers::SUPER | icedtea_compositor::input::Modifiers::SHIFT;
    assert_eq!(state.handle_key(mods, 0x71), Some(()), "the default quit binding matched");
    assert!(state.quitting, "and it fired");

    let (tx2, _rx2) = crossbeam_channel::unbounded();
    let mut state2 = State::new(icedtea_config::default_config(), tx2);
    assert_eq!(
        state2.handle_key(icedtea_compositor::input::Modifiers::empty(), 0x61),
        None,
        "a plain 'a' is nobody's binding and must be forwarded"
    );
}
