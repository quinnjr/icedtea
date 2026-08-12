//! The compositor boots and runs a bounded number of event-loop turns
//! against wlroots' headless backend — with no display, no GPU and no seat,
//! so this runs in CI.
//!
//! The smithay baseline had no equivalent: nothing in it could be driven
//! without a winit window. `a_headless_compositor_boots_runs_and_stops` is
//! the test that would have caught a boot that silently listens on a socket
//! nothing renders to: it asserts the headless backend's output actually
//! reached the model's geometry map with a real size, and (review finding
//! I1's `State::frames` counter) that a real frame callback fired at least
//! once. `the_shutdown_source_stops_the_loop`
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
    //
    // Review finding I1's frame assertion is why this is `Until::Stop` with a
    // bounded backstop rather than a bounded `Until::Turns` directly: `Turns`
    // dispatches with a `0` timeout on every iteration (see `run_all`'s own
    // doc), which never actually blocks -- and `wlr_output_schedule_frame`'s
    // headless backend implementation matures on a real timer, not an idle
    // callback, so a purely non-blocking loop can spin through hundreds of
    // zero-timeout dispatches without that timer ever being live long enough
    // to fire. `Until::Stop`'s blocking `dispatch(-1)` computes the correct
    // wait from the timer wlroots actually armed and sleeps until it's due,
    // which is what lets the frame genuinely arrive. The command channel
    // (already proven to wake the loop by `a_dbus_command_wakes_an_idle_loop_via_its_wake_pipe`)
    // is reused as this test's own bounded backstop so a regression here
    // fails the test rather than hanging it.
    let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded();
    state.set_command_receiver(cmd_rx);
    let (cmd_wake_write, cmd_wake_id) = icedtea_compositor::backend::wake_source(&runtime).expect("cmd wake source");
    state.set_cmd_wake_source(cmd_wake_id);
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(100));
        let _ = cmd_tx.send(icedtea_compositor::dbus::DbCommand::Quit);
        icedtea_compositor::backend::wake(&cmd_wake_write);
    });

    backend
        .run_all(&display, &mut state, &runtime, wlr::Until::Stop)
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
    assert!(state.quitting, "the backstop Quit command must have stopped the loop");
    // Review finding I1: before `new_output` called `output.schedule_frame()`,
    // nothing owned the first frame -- it arrived only incidentally, via
    // whatever damage the background rect's own resize happened to cause.
    // `state.frames` (incremented in `OutputHandler::frame`) is the proof
    // that a frame callback actually fired at least once over the window
    // above.
    assert!(
        state.frames >= 1,
        "new_output's schedule_frame() must have produced at least one frame callback"
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

/// The config-reload counterpart to `a_dbus_command_wakes_an_idle_loop_via_its_wake_pipe`:
/// a freshly-loaded `Config` sent on `config_reload_tx` from another thread
/// is invisible to a blocked loop unless the config-reload wake pipe is
/// registered and nudged, exactly like the D-Bus command channel. This is
/// the path `lib.rs`'s `run()` wires for real (`set_config_reload_receiver`
/// and `set_config_reload_wake_source`), and until this test it had zero
/// coverage: nothing exercised a reload arriving *through the wake pipe*
/// while the loop was idle, only `apply_config`/`apply_reloaded_config`
/// called directly and synchronously.
///
/// `Until::Stop` is used here too, for the same reason as the sibling test:
/// a bounded `Until::Turns` would pass even if the wake pipe were never
/// registered, by just running out the clock without ever seeing the
/// reload. The command channel (already proven to wake the loop by the
/// sibling test) is reused as this test's own bounded backstop -- the
/// worker thread sends the reload, nudges *that* wake pipe, and only after
/// a further bounded sleep sends `Quit` on the command channel and nudges
/// its wake pipe, so the loop is guaranteed to stop rather than hang if
/// something regresses.
#[test]
fn a_config_reload_wakes_an_idle_loop_via_its_wake_pipe() {
    ensure_headless_env();

    let display = wlr::Display::new().expect("display");
    let backend = wlr::Backend::autocreate(&display.event_loop()).expect("backend");
    let runtime = wlr::Runtime::new().expect("runtime");

    // See `a_headless_compositor_boots_runs_and_stops` for why `state` is
    // declared after `display`/`backend`/`runtime`.
    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut state = State::new(icedtea_config::default_config(), tx);
    state.wayland.attach(runtime.clone());

    // The reload channel and its wake pipe -- the thing under test.
    let (reload_tx, reload_rx) = crossbeam_channel::unbounded();
    state.set_config_reload_receiver(reload_rx);
    let (reload_wake_write, reload_wake_id) =
        icedtea_compositor::backend::wake_source(&runtime).expect("reload wake source");
    state.set_config_reload_wake_source(reload_wake_id);

    // The command channel and its wake pipe, reused only as this test's
    // bounded backstop -- see the doc above.
    let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded();
    state.set_command_receiver(cmd_rx);
    let (cmd_wake_write, cmd_wake_id) = icedtea_compositor::backend::wake_source(&runtime).expect("cmd wake source");
    state.set_cmd_wake_source(cmd_wake_id);

    let starting_workspaces = state.window_manager.workspace_info().len();

    // From another thread, exactly like the real producer
    // (`State::spawn_config_reload`'s worker thread): send the freshly
    // "loaded" config, then nudge the wake pipe.
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(50));
        let mut reloaded = icedtea_config::default_config();
        reloaded.workspace_names = vec!["alpha".into(), "beta".into(), "gamma".into()];
        let _ = reload_tx.send(reloaded);
        icedtea_compositor::backend::wake(&reload_wake_write);

        // Bounded backstop: give the loop time to wake, drain, and apply
        // the reload before asking it to stop.
        std::thread::sleep(std::time::Duration::from_millis(50));
        let _ = cmd_tx.send(icedtea_compositor::dbus::DbCommand::Quit);
        icedtea_compositor::backend::wake(&cmd_wake_write);
    });

    backend
        .run_all(&display, &mut state, &runtime, wlr::Until::Stop)
        .expect("run_all");

    assert!(state.quitting, "the backstop Quit command must have stopped the loop");
    assert_eq!(
        state.window_manager.workspace_info().len(),
        3,
        "the reloaded config's workspace_names must have reached State through the wake pipe"
    );
    assert_ne!(
        state.window_manager.workspace_info().len(),
        starting_workspaces,
        "the reload must actually have changed the model, not just left it as booted"
    );
}

/// The regression test for review finding C1: the wallpaper decode wake
/// pipe's write half must survive the one-shot worker thread exiting.
///
/// Before the fix, `spawn_wallpaper_decode` was handed the wake pipe's only
/// write half *by value*; it moved into the worker thread and dropped the
/// instant that thread ended (right after its single `tx.send`). The
/// registered read end then reported a permanent `EPOLLHUP` on libwayland's
/// level-triggered loop -- `dispatch(-1)` never blocks again, `fd_ready`'s
/// wallpaper arm fires on every turn forever, and (pre-M4) `drain_wallpaper`
/// re-entered its "likely panicked" warn on every one of those turns too, on
/// an otherwise completely idle compositor.
///
/// `State::turns` is what makes this provable without an actual clock-bound
/// core-burn measurement: it counts `should_stop` calls, one per event-loop
/// turn regardless of what woke it. A wedged read end means hundreds of
/// turns fire during the idle window below (empirically, on the pre-fix
/// code, thousands); a healthy one means only the handful genuinely woken by
/// the decode's own wake and the backstop `Quit`. `TURNS_BOUND` sits
/// comfortably above that handful and nowhere near a busy-spin's count, so
/// this is not a timing-flake-prone assertion.
///
/// This was confirmed to fail against the pre-fix code (`git stash` the
/// `State::spawn_wallpaper`/`set_wallpaper_wake` fix, rerun, observe
/// `state.turns` in the thousands and the assertion fail; `git stash pop` to
/// restore it) before being checked in passing.
#[test]
fn wallpaper_decode_wake_pipe_survives_the_worker_thread_exiting() {
    ensure_headless_env();

    let display = wlr::Display::new().expect("display");
    let backend = wlr::Backend::autocreate(&display.event_loop()).expect("backend");
    let runtime = wlr::Runtime::new().expect("runtime");

    // See `a_headless_compositor_boots_runs_and_stops` for why `state` is
    // declared after `display`/`backend`/`runtime`.
    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut state = State::new(icedtea_config::default_config(), tx);
    state.wayland.attach(runtime.clone());

    // The wallpaper wake pipe, wired exactly like `run()` wires it.
    let (wallpaper_wake_write, wallpaper_wake_id) =
        icedtea_compositor::backend::wake_source(&runtime).expect("wallpaper wake source");
    state.set_wallpaper_wake_source(wallpaper_wake_id);
    state.set_wallpaper_wake(wallpaper_wake_write);

    // The command channel and its wake pipe, used only as this test's
    // bounded backstop -- same role as in the config-reload sibling test.
    let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded();
    state.set_command_receiver(cmd_rx);
    let (cmd_wake_write, cmd_wake_id) = icedtea_compositor::backend::wake_source(&runtime).expect("cmd wake source");
    state.set_cmd_wake_source(cmd_wake_id);

    // No image path: the worker resolves to `None` almost instantly. What
    // matters here is only that it sends its one result and exits -- not
    // what it sends.
    state.spawn_wallpaper(None);

    // Give the one-shot worker thread time to finish and exit (and, on the
    // buggy code, time for the loop to spin many, many times on the dead
    // read end) before asking the loop to stop.
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(300));
        let _ = cmd_tx.send(icedtea_compositor::dbus::DbCommand::Quit);
        icedtea_compositor::backend::wake(&cmd_wake_write);
    });

    backend
        .run_all(&display, &mut state, &runtime, wlr::Until::Stop)
        .expect("run_all");

    assert!(state.quitting, "the backstop Quit command must have stopped the loop");

    const TURNS_BOUND: u64 = 50;
    assert!(
        state.turns < TURNS_BOUND,
        "the loop turned {} times over a 300ms idle window after the wallpaper decode \
         landed -- a wedged wake pipe (its write half dropped when the one-shot worker \
         thread exited) leaves the read end permanently EPOLLHUP-ready and the loop never \
         blocks again",
        state.turns
    );
}

/// Review finding I2: the SSD title-bar band is painted as a scene rect
/// (interim, per the spec's own words -- see `Wayland::sync_ssd_rect`'s
/// doc), tracked one-per-window at the seam the same way toplevel bindings
/// are. This needs a real `Runtime` with `init_graphics` already run --
/// `add_rect` errors otherwise -- which is why this lives here rather than
/// in `wayland.rs`'s own unit tests (which only ever construct a bare,
/// ungraphics'd `Runtime` or no runtime at all).
#[test]
fn an_ssd_window_gets_exactly_one_title_bar_rect_and_a_csd_window_gets_none() {
    use icedtea_compositor::wayland::ToplevelKey;

    ensure_headless_env();

    let display = wlr::Display::new().expect("display");
    let backend = wlr::Backend::autocreate(&display.event_loop()).expect("backend");
    let runtime = wlr::Runtime::new().expect("runtime");
    runtime.init_graphics(&display, &backend).expect("graphics");
    runtime.create_xdg_shell(&display, 6).expect("xdg_wm_base");
    runtime.create_seat(&display, "seat0").expect("seat0");

    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut state = State::new(icedtea_config::default_config(), tx);
    state.create_output(0, icedtea_contract::Rectangle { x: 0, y: 0, width: 1000, height: 800 });
    state.wayland.attach(runtime);

    // Default app id ("term") has no CSD request and doesn't match the
    // `org.gtk*` CSD heuristic, so `decoration::has_ssd` says yes -- exactly
    // the common case a real terminal or editor hits.
    let ssd_key = ToplevelKey::for_test(1);
    state.new_toplevel(ssd_key, "term", "Terminal", 1);
    assert_eq!(
        state.wayland.ssd_rect_count(),
        1,
        "mapping an SSD window must create exactly one title-bar rect"
    );

    // `org.gtk`-prefixed app ids are CSD by the same heuristic -- no strip,
    // no rect.
    let csd_key = ToplevelKey::for_test(2);
    state.new_toplevel(csd_key, "org.gtk.MyApp", "GTK App", 2);
    assert_eq!(
        state.wayland.ssd_rect_count(),
        1,
        "a CSD window draws its own decorations and must not get a rect"
    );

    // Forgetting the SSD window drops its rect bookkeeping; the CSD window
    // never had any to begin with.
    state.forget_toplevel(ssd_key);
    assert_eq!(
        state.wayland.ssd_rect_count(),
        0,
        "forgetting a window must drop its rect bookkeeping too, not leak it"
    );

    state.forget_toplevel(csd_key);
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

    // The press point is inside A's title bar move area, so
    // `handle_pointer_press` began a drag (`DragMachine::begin`). Release it
    // so the test doesn't leave the drag machine mid-drag on drop -- a real
    // seat always pairs a press with a release, and leaving one dangling
    // here would be testing a state no real input sequence produces.
    state.handle_pointer(PointerEvent::Release { pointer: point });
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

// --- Task 11 fix round: alt-tab's end condition, and the unmap seat trap ---

/// `alt_tab_should_end` is the pure decision `SeatHandler::key` makes on
/// every event while a session is active; `wlr::KeyEvent` can't be built
/// outside the `wlr` crate (`KeyEvent::new` is `pub(crate)`), so this drives
/// the decision function directly with synthetic values rather than a real
/// event -- which is the layer `SeatHandler::key` itself is exercised at,
/// too (see the report's note on why the trait method has no direct
/// automated coverage).
#[test]
fn alt_tab_ends_on_the_watched_modifiers_own_release_event() {
    use icedtea_compositor::input::Modifiers;
    use icedtea_compositor::state::alt_tab_should_end;

    let watched = Modifiers::SUPER;

    // The bug this test exists for: wlroots emits the key event *before*
    // updating its own modifier state, so the Super_L release event's
    // `mods` still reports SUPER held. A check that only looked at `mods`
    // would say "keep going" here and the session would linger until some
    // unrelated later key. `alt_tab_should_end` must say "end" from the
    // keysym alone.
    const KEY_SUPER_L: u32 = 0xffeb;
    assert!(
        alt_tab_should_end(watched, /* mods (stale) */ Modifiers::SUPER, /* pressed */ false, KEY_SUPER_L),
        "the modifier's own release event must end the session even though \
         its reported `mods` still shows it held"
    );

    // Releasing an unrelated key (Tab) while SUPER is still down must NOT
    // end the session -- that is the middle of an ordinary alt-tab cycle,
    // not its end.
    const KEY_TAB: u32 = 0xff09;
    assert!(
        !alt_tab_should_end(watched, Modifiers::SUPER, false, KEY_TAB),
        "releasing Tab while the modifier is still held keeps cycling"
    );

    // The fallback path: some later event's `mods` genuinely no longer
    // contains the watched modifier (the library's state has caught up by
    // now), regardless of which key it's for.
    assert!(
        alt_tab_should_end(watched, Modifiers::empty(), true, 0x61),
        "any event once the modifier state has caught up must end a lingering session"
    );

    // A key that is neither a release of the watched modifier nor missing
    // it from `mods` must not end the session.
    assert!(!alt_tab_should_end(watched, Modifiers::SUPER, true, KEY_TAB), "Tab press mid-cycle keeps going");
}

/// A rebound `cycle:alt_tab` (e.g. ALT+Tab) must watch the modifier it was
/// actually bound to, not a hardcoded SUPER -- releasing Super while ALT is
/// still held (a plausible accident: many keyboards have both keys within
/// reach) must not be mistaken for ending an ALT-Tab session.
#[test]
fn alt_tab_end_condition_follows_a_rebound_modifier_not_a_hardcoded_one() {
    use icedtea_compositor::input::Modifiers;
    use icedtea_compositor::state::alt_tab_should_end;

    let watched = Modifiers::ALT;
    const KEY_SUPER_L: u32 = 0xffeb;
    const KEY_ALT_L: u32 = 0xffe9;

    assert!(
        !alt_tab_should_end(watched, Modifiers::ALT | Modifiers::SUPER, false, KEY_SUPER_L),
        "releasing an unwatched modifier (SUPER) must not end an ALT-watched session"
    );
    assert!(
        alt_tab_should_end(watched, Modifiers::ALT, false, KEY_ALT_L),
        "releasing the actually-watched modifier (ALT) ends it"
    );
}

/// The trap task 11's fix round closed: before the fix, unmapping the
/// focused window hid it but never re-derived the seat's keyboard target,
/// so a real seat kept pointing at a surface wlroots was told to stop
/// showing. This can't observe the seat's internal C state from outside the
/// `wlr` crate, but it does prove the whole path -- `ToplevelHandler::unmapped`
/// calling `sync_seat_focus`, which calls `Wayland::keyboard_focus`, which
/// calls `Runtime::focus_toplevel_keyboard`/`clear_keyboard_focus` on a
/// dangling test id with no live client and no seat -- runs to completion
/// with no panic, which is exactly what "handler bodies must be panic-free"
/// requires of it.
#[test]
fn unmapping_the_focused_window_reroutes_the_seat_without_panicking() {
    use icedtea_compositor::wayland::ToplevelKey;
    use wlr::ToplevelHandler;

    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut state = State::new(icedtea_config::default_config(), tx);
    state.create_output(0, icedtea_contract::Rectangle { x: 0, y: 0, width: 1000, height: 800 });
    let runtime = wlr::Runtime::new().expect("runtime");
    state.wayland.attach(runtime);

    let key = ToplevelKey::for_test(1);
    state.new_toplevel(key, "a", "A", 1);
    let id = state.wayland.window_for(key).expect("bound");
    assert!(state.window_manager.get(id).is_some_and(|w| w.focused), "the only window is focused");

    // `for_test(1)` and `wlr::ToplevelId::dangling_nth_for_test(1)` name the
    // same underlying id (`ToplevelKey::for_test` is a thin wrapper over
    // it), so this resolves back through `wayland.window_for` to `id`
    // exactly as a real `unmapped(toplevel.id())` call would.
    state.unmapped(wlr::ToplevelId::dangling_nth_for_test(1));

    // Ledgered, not fixed by this task (see `unmapped`'s doc): the model's
    // own bookkeeping doesn't change on an unmap.
    assert!(
        state.window_manager.get(id).is_some_and(|w| w.focused),
        "the model still reports it focused -- the model has no unmapped concept"
    );
    assert!(state.wayland.is_backed(id), "still bound -- an unmap is not a destroy");
}
