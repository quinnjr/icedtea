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


use icedtea_contract::Event;

use icedtea_harness::{
    Compositor, DataControlClient, TestClient, VirtualKeyboardClient, VirtualPointerClient,
};

/// A data-control client's set (no serial) reaches a focused wl_data_device
/// client as an offer — the delivery path a clipboard manager's re-paste uses.
#[test]
fn data_control_set_reaches_a_focused_wl_data_device_client() {
    let comp = Compositor::spawn();
    let mut b = TestClient::map_toplevel(&comp.socket, "reader.app", "reader");
    assert!(b.wait_until(|c| c.last_configure().is_some()));
    let mut mgr = DataControlClient::spawn(&comp.socket);
    mgr.set_clipboard("text/plain;charset=utf-8", b"via-data-control");
    assert!(
        b.wait_until(|c| c.has_selection_offer()),
        "focused wl_data_device client never received the data-control-set selection"
    );
    let got = icedtea_harness::read_selection_from_data_control(&mut b, &mut mgr, "text/plain;charset=utf-8");
    assert_eq!(got, b"via-data-control");
}

/// The M4.6 daemon's actual path: a regular app copies via `wl_data_device`
/// and a data-control client (a clipboard manager) sees the same bytes. The
/// setter needs the injected keyboard's serial.
#[test]
fn a_wl_data_device_copy_is_seen_by_a_data_control_reader() {
    let comp = Compositor::spawn();
    let mut _vk = VirtualKeyboardClient::spawn(&comp.socket);

    let mut app = TestClient::map_toplevel(&comp.socket, "app", "app");
    assert!(app.wait_until(|c| c.has_input_serial()), "app got no serial");
    app.set_selection_text("text/plain;charset=utf-8", b"copied-by-app");

    let mut manager = DataControlClient::spawn(&comp.socket);
    assert!(manager.wait_until(|c| c.has_offer()), "manager saw no offer");
    assert_eq!(
        manager.read_from_wl_data_device_owner(&mut app, "text/plain;charset=utf-8"),
        b"copied-by-app"
    );
}

/// The focus/serial gate: with no keyboard on the seat, a mapped client has no
/// input serial, so its `wl_data_device.set_selection` is rejected by wlroots
/// and the existing clipboard is untouched. This is the mechanism that stops an
/// unfocused client hijacking the selection (an unfocused client likewise has
/// no serial).
#[test]
fn set_selection_without_an_input_serial_is_rejected() {
    let comp = Compositor::spawn(); // deliberately NO virtual keyboard -> no serials

    // Baseline: a data-control client owns the clipboard.
    let mut owner = DataControlClient::spawn(&comp.socket);
    owner.set_clipboard("text/plain", b"baseline");

    // A maps and is focused, but the seat has no keyboard, so A has no serial.
    let mut a = TestClient::map_toplevel(&comp.socket, "hijack.app", "hijack");
    assert!(a.wait_until(|c| c.last_configure().is_some()));
    assert!(!a.has_input_serial(), "no keyboard on the seat means no input serial");
    a.set_selection_text("text/plain", b"hijack"); // serial 0 -> rejected

    // The clipboard is unchanged: a fresh reader still sees the baseline.
    let mut reader = DataControlClient::spawn(&comp.socket);
    assert!(reader.wait_until(|c| c.has_offer()), "no data-control offer");
    assert_eq!(reader.read_selection(&mut owner, "text/plain"), b"baseline");
}

/// A data-control client reads a selection another data-control client set —
/// the round-trip the M4.6 clipboard daemon depends on.
#[test]
fn data_control_reads_the_current_selection() {
    let comp = Compositor::spawn();
    let mut owner = DataControlClient::spawn(&comp.socket);
    owner.set_clipboard("text/plain;charset=utf-8", b"seen-by-manager");

    let mut reader = DataControlClient::spawn(&comp.socket);
    assert!(reader.wait_until(|c| c.has_offer()), "no data-control offer");
    assert_eq!(
        reader.read_selection(&mut owner, "text/plain;charset=utf-8"),
        b"seen-by-manager"
    );
}

/// The two selection-manager globals M4.1 adds must actually be advertised —
/// the daemon (M4.6) and any clipboard manager bind them by name.
#[test]
fn selection_globals_are_advertised() {
    let comp = Compositor::spawn();
    let globals = icedtea_harness::advertised_globals(&comp.socket);
    assert!(
        globals.iter().any(|g| g == "zwp_primary_selection_device_manager_v1"),
        "primary-selection manager global missing; saw {globals:?}"
    );
    assert!(
        globals.iter().any(|g| g == "zwlr_data_control_manager_v1"),
        "data-control manager global missing; saw {globals:?}"
    );
}

/// M4.2 adds the virtual-pointer manager global so DnD test harnesses (and
/// on-screen-keyboard-style input bridges) can inject pointer motion/buttons.
#[test]
fn virtual_pointer_manager_global_is_advertised() {
    let comp = Compositor::spawn();
    let globals = icedtea_harness::advertised_globals(&comp.socket);
    assert!(
        globals.iter().any(|g| g == "zwlr_virtual_pointer_manager_v1"),
        "virtual-pointer manager global missing; saw {globals:?}"
    );
}

/// A client can bind `zwlr_virtual_pointer_manager_v1` and create a virtual
/// pointer, then inject motion/button/frame requests without a protocol
/// error — the M4.2 drag-and-drop grab serial's source. Full drag coverage
/// (Task 7) builds on this.
#[test]
fn a_client_can_bind_the_virtual_pointer() {
    let comp = Compositor::spawn();
    let mut vp = VirtualPointerClient::spawn(&comp.socket);
    vp.motion_absolute(10.0, 10.0, 200, 200);
    vp.button(0x110, true);
    vp.frame();
    vp.button(0x110, false);
    vp.frame();
    vp.pump();
}

/// The harness can bind the data-device machinery and create a device from
/// the seat — the foundation the clipboard round-trip stands on.
#[test]
fn a_mapped_client_has_a_data_device() {
    let comp = Compositor::spawn();
    let client = TestClient::map_toplevel(&comp.socket, "dd.app", "dd");
    assert!(client.has_data_device(), "data device created from manager + seat");
}

/// The core interop claim: a text payload one client copies via `wl_data_device`
/// is readable, byte for byte, by another client after focus moves to it. A
/// virtual keyboard is injected first so the setter can obtain the input serial
/// wlroots requires for `set_selection`.
#[test]
fn clipboard_transfers_between_two_clients() {
    let comp = Compositor::spawn();
    // Give the seat a keyboard so focused clients receive an input serial.
    let mut _vk = VirtualKeyboardClient::spawn(&comp.socket);

    // A maps (gains focus + a keyboard-enter serial) and owns the clipboard.
    let mut a = TestClient::map_toplevel(&comp.socket, "owner.app", "owner");
    assert!(
        a.wait_until(|c| c.has_input_serial()),
        "owner never received a keyboard-enter serial"
    );
    a.set_selection_text("text/plain;charset=utf-8", b"hello-clipboard");

    // B maps (focus moves to B) and reads the current selection.
    let mut b = TestClient::map_toplevel(&comp.socket, "reader.app", "reader");
    assert!(
        b.wait_until(|c| c.has_selection_offer()),
        "B never received a data offer"
    );
    let got = icedtea_harness::read_selection(&mut b, &mut a, "text/plain;charset=utf-8");
    assert_eq!(got, b"hello-clipboard");

    a.detach();
    b.detach();
}

/// The primary (middle-click) selection round-trip: one client sets it, another
/// reads it byte for byte after focus moves. Needs the injected keyboard's
/// serial, exactly as the clipboard does.
#[test]
fn primary_selection_transfers_between_two_clients() {
    let comp = Compositor::spawn();
    let mut _vk = VirtualKeyboardClient::spawn(&comp.socket);

    let mut a = TestClient::map_toplevel(&comp.socket, "owner.app", "owner");
    assert!(a.wait_until(|c| c.has_input_serial()), "owner got no serial");
    a.set_primary_text("text/plain;charset=utf-8", b"primary-payload");

    let mut b = TestClient::map_toplevel(&comp.socket, "reader.app", "reader");
    assert!(
        b.wait_until(|c| c.has_primary_offer()),
        "B never received a primary offer"
    );
    assert_eq!(
        icedtea_harness::read_primary(&mut b, &mut a, "text/plain;charset=utf-8"),
        b"primary-payload"
    );

    a.detach();
    b.detach();
}

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

/// Task 10: a client-driven `xdg_toplevel.set_maximized` reaches
/// `ToplevelHandler::request_maximize`, which routes onto
/// `reconcile_maximized` -- both the client's configure and the model must
/// agree, the same "both sides of the seam" shape as the model-side test
/// above, but this time initiated by the client rather than the D-Bus API.
#[test]
fn a_client_maximize_request_round_trips_through_the_compositor() {
    let comp = Compositor::spawn();
    let mut client = TestClient::map_toplevel(&comp.socket, "harness.max", "maxi");
    client.request_maximize(true);
    // xdg_toplevel state 1 == maximized (xdg-shell spec numeric value).
    assert!(client.wait_until(|c| c.states().contains(&1)), "client never saw the maximized state in a configure");
    let snap = comp.snapshot();
    assert!(snap.windows.iter().any(|w| w.maximized), "model must agree");
    client.detach();
}

/// Task 10: xdg-shell requires the compositor to answer every
/// `set_maximized`/`set_fullscreen` request with a configure, even one that
/// changes nothing in the model -- the dispatch layer's guarantee, not
/// `reconcile_fullscreen`'s job. Fullscreen once, then send a redundant
/// second request and prove a configure still arrives via the monotonic
/// counter (an equality check on `states()` alone can't tell a fresh
/// configure from the stale one still reading the way the test hoped).
#[test]
fn an_unhonored_request_still_gets_a_configure() {
    let comp = Compositor::spawn();
    let mut client = TestClient::map_toplevel(&comp.socket, "harness.fs", "fs");
    client.request_fullscreen(true);
    assert!(client.wait_until(|c| c.states().contains(&2)), "fullscreen state expected"); // 2 == fullscreen
    let before = client.configure_count();
    client.request_fullscreen(true); // redundant: already fullscreen
    assert!(client.wait_until(|c| c.configure_count() > before), "no answer to the redundant request");
    client.detach();
}

/// Task 13: a client that creates a decoration object and states no
/// preference must be told **server-side** — this compositor draws the band
/// itself for anything that isn't a known CSD app.
///
/// This is also the first test in the suite where a *real* toplevel reaches
/// the whole in-tree decoration stack: answering the negotiation re-syncs
/// the window, which builds the band rect, the three button rects and the
/// cosmic-text title buffer node inside that toplevel's own scene tree
/// against a live wlroots scene. Nothing but a real client can exercise
/// those calls (a `for_test` id makes every one of them a no-op), so a
/// crash, a mis-sized buffer or a rejected node shows up here and nowhere
/// else.
#[test]
fn ssd_is_negotiated_for_a_client_that_defers() {
    let comp = Compositor::spawn();
    let mut client = TestClient::map_decorated_toplevel(&comp.socket, "harness.ssd", "decorated");
    // 2 == server_side in zxdg_toplevel_decoration_v1.
    assert!(client.wait_until(|c| c.decoration_mode() == Some(2)), "server-side expected");

    // And the band is really reserved: an SSD window's client is configured
    // at the frame height minus the title bar, never at the whole frame.
    let snap = comp.snapshot();
    let frame = snap.windows.first().expect("one mapped window").geometry;
    assert!(
        client.wait_until(|c| c.last_configure() == Some((frame.width, frame.height - 28))),
        "an SSD client must be sized to the content rect, got {:?} for frame {:?}",
        client.last_configure(),
        frame
    );

    // Review finding L1: exactly one decoration configure, and it is the
    // right one. The provisional answer made before the app-id was known is
    // staged, not sent, so the client never sees a wrong-then-right pair.
    assert_eq!(client.decoration_modes(), [2], "one configure, server-side");

    // Task 9: the decoration-mode negotiation itself is now observable over
    // D-Bus -- `set_client_decorations_requested` emits a `WindowUpdated`
    // signaling "re-fetch via GetState" once the mode settles.
    comp.wait_event(|e| matches!(e, Event::WindowUpdated { .. }));

    // Retitling drives `update_buffer` on the live title node: the model
    // must take the new title and the client must survive it.
    client.set_title("renamed");
    comp.wait_event(|e| matches!(e, Event::WindowUpdated { update, .. } if update.title.as_deref() == Some("renamed")));
    assert!(client.wait_until(|c| !c.closed()), "the client must still be alive");

    client.detach();
}

/// The other answer: a client this compositor treats as client-side
/// decorated (`decoration::is_csd`'s app-id rule) is told **client-side**,
/// and gets its whole frame rather than a content rect inset by a band it
/// will never be shown.
#[test]
fn csd_is_negotiated_for_a_client_that_draws_its_own() {
    let comp = Compositor::spawn();
    let mut client = TestClient::map_decorated_toplevel(&comp.socket, "org.gtk.Harness", "own frame");
    // 1 == client_side.
    assert!(client.wait_until(|c| c.decoration_mode() == Some(1)), "client-side expected");

    let snap = comp.snapshot();
    let frame = snap.windows.first().expect("one mapped window").geometry;
    assert!(
        client.wait_until(|c| c.last_configure() == Some((frame.width, frame.height))),
        "a CSD client owns every pixel of its frame, got {:?} for frame {:?}",
        client.last_configure(),
        frame
    );
    // L1 again, and this is the case that used to be wrong-then-right: the
    // app-id says client-side, and that is the *only* answer sent.
    assert_eq!(client.decoration_modes(), [1], "one configure, client-side");

    client.detach();
}

/// Task 14: a client that unmaps (attaches a null buffer and commits, not a
/// destroy) releases focus and alt-tab candidacy exactly the way
/// `ToplevelHandler::unmapped` now does against the model, and the row
/// survives -- this is `Wayland::keyboard_focus`/`configure` going out to a
/// *real* second client, which a synthetic `ToplevelKey` cannot exercise.
#[test]
fn an_unmapping_client_releases_focus_and_alt_tab() {
    let comp = Compositor::spawn();
    let mut first = TestClient::map_toplevel(&comp.socket, "harness.stay", "stays");
    let mut second = TestClient::map_toplevel(&comp.socket, "harness.go", "goes");
    // second has focus (latest map wins). Unmap it: attach a null buffer +
    // commit.
    second.unmap();
    assert!(
        first.wait_until(|c| c.states().contains(&4)), // 4 == activated
        "focus must return to the surviving client"
    );
    let snap = comp.snapshot();
    assert!(snap.windows.iter().any(|w| w.title == "goes"), "row survives the unmap");

    // Task 9: the unmap's `WindowUpdated` must carry `mapped: Some(false)`
    // so a D-Bus subscriber can observe the transition without polling
    // `GetState`.
    comp.wait_event(|e| matches!(e, Event::WindowUpdated { update, .. } if update.mapped == Some(false)));

    first.detach();
    second.detach();
}

/// Task 20: a real `zwlr_layer_shell_v1` panel gets configured (mandatory --
/// an unanswered layer surface hangs its client) and its exclusive zone
/// carves the usable area a maximized toplevel is placed against.
///
/// The expected maximized content height below is derived, not guessed:
/// `tests/support/mod.rs`'s headless backend reports a `1280x720` output
/// (confirmed against this same harness's `Compositor::spawn` --
/// `headless_boot.rs`'s own coverage only asserts `width > 0 && height > 0`,
/// so this comment is this suite's one place that number is pinned) --
/// `1280x720` minus the panel's `30`px top exclusive zone (usable
/// `1280x690`), minus `appearance.snap_gap`'s default `8`px inset on both
/// edges of the maximize rect (`layout::maximized_geometry`, frame
/// `1264x674`), minus the `28`px SSD title-bar band `harness.tiled` gets
/// by default (`decoration::TITLE_BAR_HEIGHT`, `content_rect`) since it
/// does not match the `org.gtk`/`gtk4` CSD carve-out: `674 - 28 = 646`.
#[test]
fn a_layer_panel_gets_configured_and_carves_the_workspace() {
    let comp = Compositor::spawn();
    let mut panel = TestClient::map_layer_panel(&comp.socket, 30);
    assert!(panel.wait_until(|c| c.layer_configure().is_some()), "panel must be configured");
    let (w, h) = panel.layer_configure().expect("size");
    assert!(w > 0 && h == 30, "panel must span the output's width at its 30px exclusive thickness, got {w}x{h}");

    let mut win = TestClient::map_toplevel(&comp.socket, "harness.tiled", "t");
    let id = comp.snapshot().windows[0].id;
    comp.send(icedtea_compositor::dbus::DbCommand::Maximize(id, true));
    assert!(
        win.wait_until(|c| c.last_configure().map(|(_, h)| h) == Some(646)),
        "maximized height must exclude the panel's zone, last configure: {:?}",
        win.last_configure()
    );

    win.detach();
}

/// Task 20 review finding J2, end-to-end (re-review Minor-2 wired
/// `LayerPanelClient::unmap` into a real test rather than leaving it
/// unused): a panel that unmaps -- without being destroyed -- must give
/// its exclusive zone back, and a maximized toplevel must track that
/// immediately with no D-Bus command needed (`layer_surface_unmapped`'s
/// own `arrange_layers()` call re-syncs it). `646` is
/// `a_layer_panel_gets_configured_and_carves_the_workspace`'s own derived
/// carved height; `676` is that same derivation with the panel's 30px
/// zone removed (`1280x720` minus `snap_gap`'s `8`px both-edges inset,
/// `1264x704` frame, minus the `28`px SSD title-bar band -> `676`).
#[test]
fn unmapping_a_layer_panel_gives_the_usable_area_back() {
    let comp = Compositor::spawn();
    let mut panel = TestClient::map_layer_panel(&comp.socket, 30);
    assert!(panel.wait_until(|c| c.layer_configure().is_some()), "panel must be configured");

    let mut win = TestClient::map_toplevel(&comp.socket, "harness.tiled", "t");
    let id = comp.snapshot().windows[0].id;
    comp.send(icedtea_compositor::dbus::DbCommand::Maximize(id, true));
    assert!(
        win.wait_until(|c| c.last_configure().map(|(_, h)| h) == Some(646)),
        "maximized height must exclude the panel's zone before it unmaps, last configure: {:?}",
        win.last_configure()
    );

    let n = win.configure_count();
    panel.unmap();
    assert!(
        win.wait_until(|c| c.configure_count() > n && c.last_configure().map(|(_, h)| h) == Some(676)),
        "maximized height must exclude nothing once the panel unmaps, last configure: {:?}",
        win.last_configure()
    );

    win.detach();
}

/// Final review I1, at the wire: an auto-hide panel that unmaps and maps
/// again must be configured again.
///
/// wlroots clears the layer surface's `initialized` flag on every unmap, so
/// the remap's initial commit owes a *mandatory* configure — but the
/// compositor recomputes the same placement it had before (same output,
/// same anchors, nothing moved), and `configure_layer`'s storm guard used
/// to match that against a `last_configured` no one had cleared and
/// suppress the send. The panel then never becomes `mapped`, which is the
/// only thing `arrange_layers`' sweep reconfigures, so nothing ever rescued
/// it: `remap()` returned `false` and the client hung forever. This is the
/// toggle-launcher / auto-hide-panel sequence, and the harness previously
/// unmapped a panel without ever remapping one.
///
/// The exclusive zone is asserted on both sides of the round trip, through
/// a maximized toplevel, so this pins that the panel really re-mapped
/// rather than merely being sent bytes: `646` and `676` are
/// `a_layer_panel_gets_configured_and_carves_the_workspace`'s and
/// `unmapping_a_layer_panel_gives_the_usable_area_back`'s own derived
/// heights, with and without the panel's 30px carve.
#[test]
fn a_remapped_layer_panel_is_configured_again_and_re_carves_the_workspace() {
    let comp = Compositor::spawn();
    let mut panel = TestClient::map_layer_panel(&comp.socket, 30);
    assert!(panel.wait_until(|c| c.layer_configure().is_some()), "panel must be configured");

    let mut win = TestClient::map_toplevel(&comp.socket, "harness.tiled", "t");
    let id = comp.snapshot().windows[0].id;
    comp.send(icedtea_compositor::dbus::DbCommand::Maximize(id, true));
    assert!(
        win.wait_until(|c| c.last_configure().map(|(_, h)| h) == Some(646)),
        "maximized height must exclude the panel's zone to begin with, last configure: {:?}",
        win.last_configure()
    );

    let n = win.configure_count();
    panel.unmap();
    assert!(
        win.wait_until(|c| c.configure_count() > n && c.last_configure().map(|(_, h)| h) == Some(676)),
        "the unmap must give the zone back first, last configure: {:?}",
        win.last_configure()
    );

    let before = panel.layer_configure_count();
    assert!(
        panel.remap(),
        "a remapped panel must be sent a fresh configure (had {before} before the remap)"
    );
    assert_eq!(
        panel.layer_configure(),
        Some((1280, 30)),
        "and at the same placement it had before -- which is exactly why the storm guard swallowed it"
    );

    let n = win.configure_count();
    assert!(
        win.wait_until(|c| c.configure_count() > n && c.last_configure().map(|(_, h)| h) == Some(646)),
        "the remapped panel must carve its zone again, last configure: {:?}",
        win.last_configure()
    );

    win.detach();
}

