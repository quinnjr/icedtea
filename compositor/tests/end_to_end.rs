//! End-to-end model-level coverage (Task 14): drives the real
//! `WindowManager`, `State::apply_action`, and `State::handle_command`
//! exactly as the D-Bus service and input layer would, without a real
//! Wayland display, event loop, or session bus.

use icedtea_compositor::dbus::DbCommand;
use icedtea_compositor::state::{OutputSurface, State};
use icedtea_config::default_config;
use icedtea_contract::{Event, Rectangle};

#[test]
fn window_lifecycle_and_dbus_commands() {
    let (tx, rx) = crossbeam_channel::unbounded();
    let mut state = State::new(default_config(), tx);

    let id = state.window_manager.add_window(
        "org.test.App",
        "App",
        42,
        Rectangle { x: 0, y: 0, width: 640, height: 400 },
    );
    // NOTE (genuine brief-sample bug, fixed here per the plan's standing
    // ruling): `WindowManager::add_window` buffers its event on
    // `window_manager.pending_events`, not on `dbus_tx` directly -- only
    // `State::emit_pending()` (called by every `State`-level mutator, e.g.
    // `handle_command`) drains that buffer onto the channel. The brief's
    // sample called `add_window` directly on `window_manager` (bypassing
    // any `State` method) and then read `rx` with no drain in between, so
    // it would never observe the event. Draining explicitly here restores
    // the sample's evident intent without changing what's under test.
    state.emit_pending();
    assert!(matches!(rx.try_recv(), Ok(Event::WindowOpened(_))));

    // DBus commands mutate the model.
    state.handle_command(DbCommand::Focus(id)).unwrap();
    assert!(state.window_manager.get(id).unwrap().focused);

    state.handle_command(DbCommand::SetWorkspace(1)).unwrap();
    assert_eq!(state.window_manager.active_workspace(), 1);

    // NOTE (genuine brief-sample bug, fixed here per the plan's standing
    // ruling): `Focus` and `SetWorkspace` above also emit their own events
    // (a `WindowUpdated`/`WorkspaceSet`) that `handle_command`'s trailing
    // `emit_pending()` already flushed onto `rx`. The brief's sample never
    // read those, so they'd sit ahead of `WindowClosed` in the channel and
    // the literal `rx.try_recv()` below would see one of them instead.
    // Draining the not-under-test events first restores the sample's
    // evident intent: assert specifically on the `Close` command's event.
    while rx.try_recv().is_ok() {}

    state.handle_command(DbCommand::Close(id)).unwrap();
    assert!(state.window_manager.get(id).is_none());
    assert!(matches!(rx.try_recv(), Ok(Event::WindowClosed(_))));
}

#[test]
fn action_dispatch_cover_all_default_actions() {
    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut state = State::new(default_config(), tx);
    // NOTE (genuine brief-sample bug, fixed here per the plan's standing
    // ruling): `fullscreen` and `snap:*` both read `state.outputs` for
    // target geometry and are a silent no-op (`None`) when it's empty --
    // the brief's sample never populated it. Task 11's own review already
    // flagged this same gap in a sibling test (see
    // `apply_action_switches_workspaces_and_snaps`'s doc comment); a single
    // output at (0, 0) mirrors that fix.
    state.outputs.insert(0, OutputSurface { geometry: Rectangle { x: 0, y: 0, width: 1920, height: 1080 } });
    for action in [
        "close",
        "fullscreen",
        "workspace:2",
        "move_to_workspace:1",
        "snap:left",
        "snap:restore",
        "cycle:alt_tab",
        "reload",
    ] {
        // NOTE (genuine brief-sample bug, fixed here per the plan's
        // standing ruling): the brief's sample added exactly one window
        // before the loop and put "close" first in the action list --
        // every action after "close" needs a focused window (`fullscreen`,
        // `move_to_workspace`, `snap:*`, `cycle:alt_tab` all call
        // `focused_window()?` or read `alt_tab_entries()`), so the
        // originally-added window being gone made every action after the
        // first a guaranteed `None`. Re-adding a window every iteration
        // (`add_window` auto-focuses, per `window.rs`) keeps each action
        // dispatchable regardless of what a prior iteration did to it.
        state.window_manager.add_window("a", "a", 1, Rectangle { x: 0, y: 0, width: 640, height: 400 });
        assert!(state.apply_action(action).is_some(), "action {action} should dispatch");
    }
}
