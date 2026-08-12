//! The client-driven protocol tests: a real `wayland-client` speaking real
//! xdg-shell to a real headless compositor over a real socket.
//!
//! Everything else in this crate's test suite drives `State` directly, with
//! synthetic `ToplevelKey::for_test` ids that no wlroots object backs — which
//! means none of it can exercise the scene-graph half of the seam (a dangling
//! id makes every `Wayland::*` scene call a no-op by construction). These
//! tests are what restores that coverage: a window only appears in the model
//! here because a client actually mapped a surface, and the compositor
//! actually ran the `mapped` path against a live toplevel.

mod support;

use icedtea_contract::Event;

use support::{Compositor, TestClient};

/// The restoration test: a real toplevel maps and shows up in the model,
/// with the app_id and title the client set, at a real geometry.
#[test]
fn a_real_client_maps_and_appears_in_the_model() {
    let comp = Compositor::spawn();
    let mut client = TestClient::map_toplevel(&comp.socket, "harness.app", "first window");
    assert!(
        client.wait_until(|c| c.last_configure().is_some()),
        "no configure arrived"
    );

    let opened = comp.wait_event(|e| matches!(e, Event::WindowOpened(w) if w.app_id == "harness.app"));
    let Event::WindowOpened(info) = opened else { unreachable!() };
    assert_eq!(info.title, "first window");

    let snapshot = comp.snapshot();
    assert_eq!(snapshot.windows.len(), 1, "exactly one mapped window");
    assert_eq!(snapshot.windows[0].id, info.id);
    assert_eq!(snapshot.windows[0].app_id, "harness.app");
    assert_eq!(snapshot.windows[0].title, "first window");
    assert!(
        snapshot.windows[0].geometry.width > 0 && snapshot.windows[0].geometry.height > 0,
        "a mapped window gets a real geometry, got {:?}",
        snapshot.windows[0].geometry
    );

    client.detach();
}

/// The other direction: a model-side `Close` reaches the actual client as
/// `xdg_toplevel.close`. This is the path `Wayland::close` takes through a
/// live toplevel — unreachable with a synthetic id.
#[test]
fn closing_from_the_model_reaches_the_client() {
    let comp = Compositor::spawn();
    let mut client = TestClient::map_toplevel(&comp.socket, "harness.close", "doomed");

    let opened = comp.wait_event(|e| matches!(e, Event::WindowOpened(_)));
    let Event::WindowOpened(info) = opened else { unreachable!() };

    comp.send(icedtea_compositor::dbus::DbCommand::Close(info.id));
    assert!(
        client.wait_until(|c| c.closed()),
        "client never saw xdg_toplevel.close"
    );

    // And the model half of the same round trip: a well-behaved client
    // answers `close` by destroying its toplevel, which must take the row
    // with it. `Close` alone deliberately does not — the client is the one
    // that decides.
    client.detach();
    let closed = comp.wait_event(|e| matches!(e, Event::WindowClosed(id) if *id == info.id));
    let Event::WindowClosed(_) = closed else { unreachable!() };
    assert!(comp.snapshot().windows.is_empty());
}

/// A title set after mapping propagates to the model through
/// `ToplevelHandler::title_changed`, and shows up in a fresh snapshot.
#[test]
fn a_title_change_from_the_client_reaches_the_model() {
    let comp = Compositor::spawn();
    let mut client = TestClient::map_toplevel(&comp.socket, "harness.title", "before");
    let opened = comp.wait_event(|e| matches!(e, Event::WindowOpened(w) if w.app_id == "harness.title"));
    let Event::WindowOpened(info) = opened else { unreachable!() };
    assert_eq!(info.title, "before");

    client.set_title("after");
    let updated = comp.wait_event(|e| {
        matches!(e, Event::WindowUpdated { id, update }
            if *id == info.id && update.title.as_deref() == Some("after"))
    });
    let Event::WindowUpdated { .. } = updated else { unreachable!() };

    let snapshot = comp.snapshot();
    assert_eq!(snapshot.windows.len(), 1);
    assert_eq!(snapshot.windows[0].title, "after");

    client.detach();
}

/// A model-side `Maximize` reaches the client as a real `xdg_toplevel`
/// configure carrying the maximized state — the `Wayland::configure` path,
/// which a synthetic `ToplevelKey` cannot reach (`resolve` returns `None`
/// and every runtime call is skipped).
#[test]
fn a_model_side_maximize_reaches_the_client_as_a_configure_state() {
    /// `xdg_toplevel.state.maximized`.
    const MAXIMIZED: u32 = 1;

    let comp = Compositor::spawn();
    let mut client = TestClient::map_toplevel(&comp.socket, "harness.max", "maximize me");
    let opened = comp.wait_event(|e| matches!(e, Event::WindowOpened(w) if w.app_id == "harness.max"));
    let Event::WindowOpened(info) = opened else { unreachable!() };
    assert!(
        !client.states().contains(&MAXIMIZED),
        "a freshly mapped window is not maximized"
    );

    assert!(!comp.snapshot().windows[0].maximized);

    // Latched the way later tasks' idempotent-looking assertions must:
    // `wait_until` checks its predicate before pumping, so only a monotonic
    // counter proves a *new* configure arrived rather than the old one
    // still reading the way the test hoped.
    let configures = client.configure_count();
    comp.send(icedtea_compositor::dbus::DbCommand::Maximize(info.id, true));
    assert!(
        client.wait_until(|c| c.configure_count() > configures && c.states().contains(&MAXIMIZED)),
        "client never got a configure with the maximized state, last states: {:?}",
        client.states()
    );
    // Both sides of the seam, not just the client's half: the model has to
    // agree with what it told the client.
    assert!(
        comp.snapshot().windows[0].maximized,
        "the model must record the maximize it configured the client with"
    );

    comp.send(icedtea_compositor::dbus::DbCommand::Maximize(info.id, false));
    assert!(
        client.wait_until(|c| !c.states().contains(&MAXIMIZED)),
        "client never got a configure clearing the maximized state"
    );
    assert!(!comp.snapshot().windows[0].maximized);

    client.detach();
}

/// Two clients on the same compositor both reach the model, and destroying
/// one leaves the other — the `toplevel_destroyed` path against live objects.
#[test]
fn destroying_one_of_two_clients_leaves_the_other() {
    let comp = Compositor::spawn();
    let first = TestClient::map_toplevel(&comp.socket, "harness.one", "one");
    let opened_one = comp.wait_event(|e| matches!(e, Event::WindowOpened(w) if w.app_id == "harness.one"));
    let Event::WindowOpened(one) = opened_one else { unreachable!() };

    let second = TestClient::map_toplevel(&comp.socket, "harness.two", "two");
    let opened_two = comp.wait_event(|e| matches!(e, Event::WindowOpened(w) if w.app_id == "harness.two"));
    let Event::WindowOpened(two) = opened_two else { unreachable!() };
    assert_ne!(one.id, two.id);

    assert_eq!(comp.snapshot().windows.len(), 2);

    first.detach();
    let closed = comp.wait_event(|e| matches!(e, Event::WindowClosed(id) if *id == one.id));
    let Event::WindowClosed(_) = closed else { unreachable!() };

    let snapshot = comp.snapshot();
    assert_eq!(snapshot.windows.len(), 1, "only the destroyed window went away");
    assert_eq!(snapshot.windows[0].id, two.id);

    second.detach();
}
