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
        Rectangle {
            x: 0,
            y: 0,
            width: 640,
            height: 400,
        },
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
    assert!(matches!(
        rx.try_recv().map(|e| e.event),
        Ok(Event::WindowOpened(_))
    ));

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
    assert!(matches!(
        rx.try_recv().map(|e| e.event),
        Ok(Event::WindowClosed(_))
    ));
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
    state.outputs.insert(
        0,
        OutputSurface::new(Rectangle {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        }),
    );
    for action in [
        "close",
        "fullscreen",
        "maximize",
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
        state.window_manager.add_window(
            "a",
            "a",
            1,
            Rectangle {
                x: 0,
                y: 0,
                width: 640,
                height: 400,
            },
        );
        assert!(
            state.apply_action(action).is_some(),
            "action {action} should dispatch"
        );
    }
}

/// Final-review finding I2: a shell that reads a snapshot and then folds in
/// signals needs each signal's `seq` to know which ones the snapshot already
/// covers and whether it missed any. This drives the whole loop the shell
/// will: snapshot, subscribe, apply, re-snapshot.
#[test]
fn signals_carry_seq_that_orders_against_the_snapshot() {
    let (tx, rx) = crossbeam_channel::unbounded();
    let mut state = State::new(default_config(), tx);
    state.outputs.insert(
        0,
        OutputSurface::new(Rectangle {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        }),
    );

    let id = state.window_manager.add_window(
        "org.test.App",
        "App",
        42,
        Rectangle {
            x: 0,
            y: 0,
            width: 640,
            height: 400,
        },
    );
    state.emit_pending();

    // The shell's initial GetState().
    let snapshot = state.window_manager.snapshot();
    let already_seen: Vec<u64> = rx.try_iter().map(|e| e.seq).collect();
    assert!(
        already_seen.iter().all(|s| *s <= snapshot.seq),
        "every signal produced before the snapshot must be discardable by seq: {already_seen:?} vs {}",
        snapshot.seq
    );

    // Everything after it is strictly newer, gapless, and ends exactly where
    // the next snapshot would.
    state.handle_command(DbCommand::Maximize(id, true)).unwrap();
    state
        .handle_command(DbCommand::Fullscreen(id, true))
        .unwrap();
    state.handle_command(DbCommand::SetWorkspace(1)).unwrap();
    let after: Vec<u64> = rx.try_iter().map(|e| e.seq).collect();
    assert!(!after.is_empty());
    assert!(
        after[0] > snapshot.seq,
        "signals after the snapshot must have a higher seq"
    );
    assert!(
        after.windows(2).all(|w| w[1] == w[0] + 1),
        "no gaps within one uninterrupted stream: {after:?}"
    );
    assert_eq!(*after.last().unwrap(), state.window_manager.snapshot().seq);
}

/// Final-review finding I1: rendering and click-to-focus consume only the
/// active workspace's non-minimized windows, so switching workspaces
/// actually changes what is on screen and what a click can hit.
#[test]
fn only_the_active_workspace_is_rendered_and_clickable() {
    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut state = State::new(default_config(), tx);
    state.outputs.insert(
        0,
        OutputSurface::new(Rectangle {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        }),
    );
    let geo = Rectangle {
        x: 0,
        y: 0,
        width: 640,
        height: 400,
    };
    let a = state.window_manager.add_window("a", "a", 1, geo);
    let b = state.window_manager.add_window("b", "b", 2, geo);

    state.move_to_workspace(b, 1).unwrap();
    // Workspace 2 is active and holds only `b`.
    assert_eq!(
        state
            .window_manager
            .visible_windows()
            .iter()
            .map(|w| w.id)
            .collect::<Vec<_>>(),
        vec![b]
    );
    assert_eq!(
        state.window_manager.window_at((10, 10)).map(|w| w.id),
        Some(b)
    );

    state.switch_workspace(0).unwrap();
    assert_eq!(
        state
            .window_manager
            .visible_windows()
            .iter()
            .map(|w| w.id)
            .collect::<Vec<_>>(),
        vec![a]
    );
    assert_eq!(
        state.window_manager.window_at((10, 10)).map(|w| w.id),
        Some(a),
        "a click can't reach another workspace"
    );

    // Minimizing removes the last one from both lists.
    state.handle_command(DbCommand::Minimize(a, true)).unwrap();
    assert!(state.window_manager.visible_windows().is_empty());
    assert!(state.window_manager.window_at((10, 10)).is_none());
}
