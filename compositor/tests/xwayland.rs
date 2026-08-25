//! Xwayland (Track A, item A1) — M1 spike end-to-end proof.
//!
//! The load-bearing test for the spike: boot the real headless compositor,
//! read the `DISPLAY` its Xwayland manager advertises, connect a genuine X11
//! client to it (which is what triggers the *lazy* `Xwayland` start), map one
//! top-level window, and assert the compositor's window model gains exactly
//! one window carrying that X11 window's class and title. This exercises the
//! whole path the spike set out to prove: X11 client → Xwayland → the `wlr`
//! `xwayland` wrapper → the compositor's `XwaylandHandler` → the `WindowManager`
//! model.
//!
//! `Xwayland` is a runtime test dependency. When it is absent the compositor's
//! `create_xwayland` fails non-fatally, `DbCommand::XwaylandDisplay` comes back
//! `None`, and this test **skips visibly** rather than failing — so the
//! workspace test gate stays green on machines (and CI images) without it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Serializes the real-Xwayland end-to-end tests. Each one boots a full
/// headless compositor *and* a lazily-spawned `Xwayland` process (plus, in some
/// cases, extra Wayland clients); running many of those concurrently under the
/// default multi-threaded test runner starves the slower boots and a window can
/// miss its poll deadline. These tests are inherently heavyweight and
/// integration-shaped, so serialize them here rather than relying on the caller
/// passing `--test-threads=1`: **every** test body takes this guard as its first
/// act (after the cheap `Xwayland`-on-`PATH` skip check, which touches no
/// compositor), so at most one compositor+Xwayland is ever live at a time
/// regardless of how many test threads the harness spawns. The guard is dropped
/// last (declared first), after each test's `Compositor` has been dropped and its
/// thread joined, so there is no teardown overlap between consecutive tests.
///
/// This makes the suite reliable under the stated gate
/// `cargo test -p icedtea-compositor --test xwayland` at any `--test-threads`.
/// The remaining sensitivity is to *cross-binary* CPU contention (a full
/// `cargo test -p icedtea-compositor` runs this binary alongside the others),
/// which only slows a boot, never corrupts one; the generous [`MAP_TIMEOUT`]
/// below absorbs that. Poisoning is ignored: a panicking test must not cascade
/// into spurious failures of the rest.
static X11_TEST_LOCK: Mutex<()> = Mutex::new(());

/// The deadline the heavyweight tests give a freshly-mapped X11 window to reach
/// the compositor's model. Deliberately generous (well past the ~1s a warm boot
/// needs): a real-Xwayland boot under cross-binary CPU contention — the other
/// `icedtea-compositor` test binaries running alongside this one during a
/// package-wide `cargo test` — can take several seconds, and a too-tight
/// deadline here is exactly what turned that slowness into the flaky
/// "the managed X11 window never entered the model" failures. Since the tests
/// are serialized (see [`X11_TEST_LOCK`]) only one boot is ever in flight, so a
/// long ceiling costs nothing on the happy path — a mapped window is observed in
/// well under a second — and only ever bites a genuinely stuck one.
const MAP_TIMEOUT: Duration = Duration::from_secs(30);

fn x11_test_guard() -> MutexGuard<'static, ()> {
    X11_TEST_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

use icedtea_compositor::dbus::DbCommand;
use icedtea_contract::WindowId;
use icedtea_harness::{advertised_globals, Compositor, DataControlClient, VirtualPointerClient};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ChangeWindowAttributesAux, ClientMessageEvent, ConfigureWindowAux,
    ConnectionExt as _, CreateWindowAux, EventMask, PropMode, SelectionNotifyEvent, WindowClass,
    SELECTION_NOTIFY_EVENT,
};
use x11rb::protocol::Event;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::{COPY_DEPTH_FROM_PARENT, CURRENT_TIME, NONE};

/// `BTN_LEFT` from `linux/input-event-codes.h`.
const BTN_LEFT: u32 = 0x110;
/// `_NET_WM_MOVERESIZE_MOVE` direction from the EWMH spec.
const NET_WM_MOVERESIZE_MOVE: u32 = 8;

/// The title-bar strip height the compositor reserves for a server-side
/// decoration (`decoration::TITLE_BAR_HEIGHT`). An SSD window's client is
/// configured to a content rect this much shorter than its frame, which is the
/// non-vacuous, end-to-end proof that the X11 window really is decorated.
const TITLE_BAR_HEIGHT: u16 = 28;

/// The `WM_CLASS` "res_class" the test window sets — what the compositor maps
/// to a Wayland `app_id`.
const WINDOW_CLASS: &str = "IcedteaSpikeClass";
/// The `WM_CLASS` "res_name" (instance) — the `app_id` fallback.
const WINDOW_INSTANCE: &str = "icedtea-spike";
/// The window title (`WM_NAME`/`_NET_WM_NAME`).
const WINDOW_TITLE: &str = "Icedtea Spike Window";

/// Whether an `Xwayland` binary is on this host at all. A belt-and-braces
/// companion to the `DISPLAY == None` skip: if `Xwayland` is missing the
/// compositor never advertises a display anyway, but checking `PATH` first
/// gives a clearer skip message.
fn xwayland_on_path() -> bool {
    let present = std::env::var_os("PATH")
        .map(|path| {
            std::env::split_paths(&path).any(|dir| dir.join("Xwayland").is_file())
        })
        .unwrap_or(false);
    // Review finding #10: every test in this file returns early (reported as a
    // green PASS) when the `Xwayland` binary is absent, so on a runner without
    // it the whole suite is a silent no-op that verifies none of the feature.
    // Set `REQUIRE_XWAYLAND=1` (in CI, where Xwayland IS installed) to turn that
    // silent skip into a loud failure, so a broken provisioning step can never
    // masquerade as a passing X11 suite.
    if !present && std::env::var_os("REQUIRE_XWAYLAND").is_some() {
        panic!(
            "REQUIRE_XWAYLAND is set but the `Xwayland` binary is not on PATH; \
             the X11 end-to-end suite cannot run and must not be reported as passing"
        );
    }
    present
}

#[test]
fn one_managed_x11_window_maps_into_the_window_model() {
    // Serialize the heavyweight real-Xwayland e2e tests (see `X11_TEST_LOCK`).
    let _x11_guard = x11_test_guard();
    if !xwayland_on_path() {
        eprintln!("SKIP: Xwayland is not installed on this host; the X11 spike test cannot run");
        return;
    }

    let comp = Compositor::spawn();

    // Lazy start reserves the display socket up front, so this is available
    // before any `Xwayland` process exists. `None` means `create_xwayland`
    // failed (no `Xwayland`), in which case skip rather than fail.
    let Some(display) = comp.xwayland_display() else {
        eprintln!("SKIP: the compositor advertised no Xwayland DISPLAY (Xwayland unavailable)");
        return;
    };
    eprintln!("Xwayland DISPLAY = {display}");

    // Connecting is what execs the lazy `Xwayland`; the handshake completes
    // once it is up. The socket already exists, so `connect` proxies through
    // wlroots — retry a few times only to absorb a slow first exec.
    let (conn, screen_num) = connect_with_retry(&display);
    let screen = &conn.setup().roots[screen_num];

    let win = conn.generate_id().expect("generate X11 window id");
    conn.create_window(
        COPY_DEPTH_FROM_PARENT,
        win,
        screen.root,
        0,
        0,
        200,
        150,
        0,
        WindowClass::INPUT_OUTPUT,
        screen.root_visual,
        &CreateWindowAux::new().background_pixel(screen.white_pixel),
    )
    .expect("create X11 window");

    // WM_CLASS is "instance\0class\0" on the wire; the compositor maps the
    // class half to `app_id`.
    let wm_class = format!("{WINDOW_INSTANCE}\0{WINDOW_CLASS}\0");
    conn.change_property8(
        PropMode::REPLACE,
        win,
        AtomEnum::WM_CLASS,
        AtomEnum::STRING,
        wm_class.as_bytes(),
    )
    .expect("set WM_CLASS");

    // Title: set both WM_NAME and _NET_WM_NAME so it does not matter which the
    // xwm reads.
    conn.change_property8(
        PropMode::REPLACE,
        win,
        AtomEnum::WM_NAME,
        AtomEnum::STRING,
        WINDOW_TITLE.as_bytes(),
    )
    .expect("set WM_NAME");
    let net_wm_name = conn
        .intern_atom(false, b"_NET_WM_NAME")
        .expect("intern _NET_WM_NAME request")
        .reply()
        .expect("intern _NET_WM_NAME reply")
        .atom;
    let utf8_string = conn
        .intern_atom(false, b"UTF8_STRING")
        .expect("intern UTF8_STRING request")
        .reply()
        .expect("intern UTF8_STRING reply")
        .atom;
    conn.change_property8(
        PropMode::REPLACE,
        win,
        net_wm_name,
        utf8_string,
        WINDOW_TITLE.as_bytes(),
    )
    .expect("set _NET_WM_NAME");

    conn.map_window(win).expect("map X11 window");
    conn.flush().expect("flush X11 requests");

    // Poll the model until the managed X11 window shows up, then assert on it.
    let window = poll_for_window(&comp, MAP_TIMEOUT)
        .expect("the managed X11 window never entered the compositor's window model");

    assert_eq!(window.app_id, WINDOW_CLASS, "app_id should be the WM_CLASS class");
    assert_eq!(window.title, WINDOW_TITLE, "title should be the X11 window name");

    // Exactly one window — the spike maps one, and nothing else runs here.
    let snapshot = comp.snapshot();
    assert_eq!(
        snapshot.windows.len(),
        1,
        "exactly one window expected in the model, got {:?}",
        snapshot.windows
    );

    // Tearing the X11 window down removes the model row — the other half of the
    // lifecycle the spike proves.
    conn.destroy_window(win).expect("destroy X11 window");
    conn.flush().expect("flush destroy");
    drop(conn);

    let emptied = poll_until(&comp, Duration::from_secs(15), |snap| snap.windows.is_empty());
    assert!(emptied, "the model still held a window after the X11 client destroyed it");
}

/// M2 — managed-window parity. One managed X11 window is first-class: it wears
/// a server-side decoration (proven by the SSD content inset the client is
/// configured to), can be maximized (proven by the model geometry filling the
/// output *and* the X server seeing the maximized content), client-initiated
/// move lands in the model, focus follows it, and moving it to another
/// workspace takes it out of the active set. Destroy cleans up. Skips visibly
/// when Xwayland is absent, exactly like the M1 test.
#[test]
fn managed_x11_window_is_first_class() {
    // Serialize the heavyweight real-Xwayland e2e tests (see `X11_TEST_LOCK`).
    let _x11_guard = x11_test_guard();
    if !xwayland_on_path() {
        eprintln!("SKIP: Xwayland is not installed on this host; the X11 parity test cannot run");
        return;
    }

    let comp = Compositor::spawn();
    let Some(display) = comp.xwayland_display() else {
        eprintln!("SKIP: the compositor advertised no Xwayland DISPLAY (Xwayland unavailable)");
        return;
    };

    let (conn, screen_num) = connect_with_retry(&display);
    let screen = &conn.setup().roots[screen_num];

    // The size an X11 client asks for is its *content* (it has no notion of the
    // WM's decorations). A decorated window keeps that content size and the WM
    // wraps it in a frame one title-bar taller (review finding #2) — so the
    // client is never squished, and the model frame is REQUESTED + the strip.
    const REQUESTED_W: u16 = 400;
    const REQUESTED_H: u16 = 300;
    const FRAME_H: i32 = REQUESTED_H as i32 + TITLE_BAR_HEIGHT as i32;

    let win = conn.generate_id().expect("generate X11 window id");
    conn.create_window(
        COPY_DEPTH_FROM_PARENT,
        win,
        screen.root,
        0,
        0,
        REQUESTED_W,
        REQUESTED_H,
        0,
        WindowClass::INPUT_OUTPUT,
        screen.root_visual,
        &CreateWindowAux::new().background_pixel(screen.white_pixel),
    )
    .expect("create X11 window");
    // A non-CSD class, so `decoration::has_ssd` decorates it.
    let wm_class = format!("{WINDOW_INSTANCE}\0{WINDOW_CLASS}\0");
    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_CLASS, AtomEnum::STRING, wm_class.as_bytes())
        .expect("set WM_CLASS");
    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_NAME, AtomEnum::STRING, WINDOW_TITLE.as_bytes())
        .expect("set WM_NAME");
    conn.map_window(win).expect("map X11 window");
    conn.flush().expect("flush");

    // (1) It enters the model as one managed window with the right identity.
    let window = poll_for_window(&comp, MAP_TIMEOUT)
        .expect("the managed X11 window never entered the compositor's window model");
    assert_eq!(window.app_id, WINDOW_CLASS, "app_id is the WM_CLASS class");
    assert_eq!(window.title, WINDOW_TITLE, "title is the X11 window name");
    let id = window.id;

    // (2) SSD parity — the client keeps the exact content size it asked for
    // (never squished, review finding #2), and the model reserved the strip
    // above it. This is the end-to-end proof the X11 window is decorated: the
    // configure reached the X server at the requested content size.
    let content_ok = poll_x_geometry(&conn, win, Duration::from_secs(10), |g| {
        g.width == REQUESTED_W && g.height == REQUESTED_H
    });
    assert!(
        content_ok,
        "the X11 client was never configured to its requested content rect (expected {REQUESTED_W}x{REQUESTED_H})"
    );
    assert_eq!(window.geometry.width, REQUESTED_W as i32, "model geometry is the frame width");
    assert_eq!(
        window.geometry.height, FRAME_H,
        "model frame is the requested content plus the SSD title-bar strip"
    );

    // (3) Focus — a freshly mapped managed window is focused (add_window
    // autofocuses, and the shared path activated it and gave it the seat).
    assert!(
        poll_until(&comp, Duration::from_secs(5), |snap| {
            snap.windows.iter().any(|w| w.id == id && w.focused)
        }),
        "the managed X11 window did not become focused"
    );

    // (4) Client-initiated move (`ConfigureRequest`) — an X11 app placing
    // itself. icedtea is a floating WM and honours it; the new position lands
    // in the model frame.
    conn.configure_window(win, &ConfigureWindowAux::new().x(500).y(400))
        .expect("configure_window move");
    conn.flush().expect("flush move");
    assert!(
        poll_until(&comp, Duration::from_secs(10), |snap| {
            snap.windows.iter().any(|w| w.id == id && w.geometry.x == 500)
        }),
        "the client-initiated move never reached the model"
    );

    // (5) Maximize — the same WM state machine xdg toplevels use. The model
    // geometry fills the output usable area, and the X server sees the window
    // configured to the maximized content (proving `set_maximized` + the
    // configure both reached the client).
    comp.send(DbCommand::Maximize(id, true));
    let maxed = poll_snapshot_window(&comp, id, Duration::from_secs(10), |w| w.maximized)
        .expect("the window never maximized in the model");
    // It grew far past its requested frame to fill the output.
    assert!(
        maxed.geometry.width > REQUESTED_W as i32 && maxed.geometry.height > FRAME_H,
        "maximized geometry {:?} did not grow to fill the output",
        maxed.geometry
    );
    let max_frame = maxed.geometry;
    let configured_to_max = poll_x_geometry(&conn, win, Duration::from_secs(10), |g| {
        g.width == max_frame.width as u16 && g.height == (max_frame.height as u16 - TITLE_BAR_HEIGHT)
    });
    assert!(
        configured_to_max,
        "the X11 client was never configured to the maximized content rect"
    );

    // (6) Workspaces — move it to workspace 1 (the command follows the window
    // there), then switch the active workspace back to 0. The window now sits
    // on a workspace other than the active one, i.e. it has left the active
    // set: parity with how xdg toplevels ride the workspace model.
    comp.send(DbCommand::MoveToWorkspace(id, 1));
    comp.send(DbCommand::SetWorkspace(0));
    assert!(
        poll_until(&comp, Duration::from_secs(10), |snap| {
            snap.active_workspace == 0
                && snap.windows.iter().any(|w| w.id == id && w.workspace == 1)
        }),
        "the window never left the active workspace"
    );

    // (7) Destroy cleans the model up.
    conn.destroy_window(win).expect("destroy X11 window");
    conn.flush().expect("flush destroy");
    drop(conn);
    assert!(
        poll_until(&comp, Duration::from_secs(15), |snap| snap.windows.is_empty()),
        "the model still held a window after the X11 client destroyed it"
    );
}

/// M2 arm — FULLSCREEN write-back. Fullscreening a managed X11 window through
/// the WM makes the model geometry fill the output *with no SSD inset*, and the
/// X client is really configured to the full output (proven off the X server's
/// own geometry) with `_NET_WM_STATE_FULLSCREEN` set (proven off the window's
/// own `_NET_WM_STATE`). Un-fullscreen restores the decorated content rect and
/// clears the state. Non-vacuous: if `set_xwayland_surface_fullscreen` or the
/// configure never reached the client, neither the X geometry nor the atom
/// would change. Skips visibly when Xwayland is absent.
#[test]
fn fullscreen_x11_window_fills_the_output_without_ssd() {
    // Serialize the heavyweight real-Xwayland e2e tests (see `X11_TEST_LOCK`).
    let _x11_guard = x11_test_guard();
    if !xwayland_on_path() {
        eprintln!("SKIP: Xwayland is not installed on this host; the X11 fullscreen test cannot run");
        return;
    }
    let comp = Compositor::spawn();
    let Some(display) = comp.xwayland_display() else {
        eprintln!("SKIP: the compositor advertised no Xwayland DISPLAY (Xwayland unavailable)");
        return;
    };
    let (conn, screen_num) = connect_with_retry(&display);
    let screen = conn.setup().roots[screen_num].clone();

    // Requested = the client's content size; the decorated frame is one strip
    // taller (review finding #2).
    const REQUESTED_W: u16 = 400;
    const REQUESTED_H: u16 = 300;
    const FRAME_H: i32 = REQUESTED_H as i32 + TITLE_BAR_HEIGHT as i32;
    let win = map_managed_x11(&conn, &screen, REQUESTED_W, REQUESTED_H);
    let window = poll_for_window(&comp, MAP_TIMEOUT)
        .expect("the managed X11 window never entered the model");
    let id = window.id;
    // Baseline: decorated, so the client keeps its requested content size.
    assert!(
        poll_x_geometry(&conn, win, Duration::from_secs(10), |g| g.height == REQUESTED_H),
        "the X11 window was never decorated to begin with"
    );

    let net_wm_state = intern(&conn, b"_NET_WM_STATE");
    let fullscreen_atom = intern(&conn, b"_NET_WM_STATE_FULLSCREEN");

    // Fullscreen via the same D-Bus command `org.icedtea.WM.FullscreenWindow`
    // forwards.
    comp.send(DbCommand::Fullscreen(id, true));
    let fs = poll_snapshot_window(&comp, id, Duration::from_secs(10), |w| w.fullscreen)
        .expect("the window never fullscreened in the model");
    // The model frame grew to fill the output.
    assert!(
        fs.geometry.width > REQUESTED_W as i32 && fs.geometry.height > FRAME_H,
        "fullscreen geometry {:?} did not grow to fill the output",
        fs.geometry
    );
    let out = fs.geometry;
    // The X client is configured to the WHOLE output -- no title-bar inset,
    // because a fullscreen window drops its SSD (`content_rect(_, false)`).
    assert!(
        poll_x_geometry(&conn, win, Duration::from_secs(10), |g| {
            g.width == out.width as u16 && g.height == out.height as u16
        }),
        "the X client was never configured to the full output {}x{} (no SSD inset)",
        out.width,
        out.height
    );
    // And the fullscreen state atom reached the client's window.
    assert!(
        poll_net_wm_state(&conn, win, net_wm_state, Duration::from_secs(10), |atoms| {
            atoms.contains(&fullscreen_atom)
        }),
        "_NET_WM_STATE_FULLSCREEN was never set on the X window"
    );

    // Un-fullscreen restores the decorated frame and clears the atom.
    comp.send(DbCommand::Fullscreen(id, false));
    let restored = poll_snapshot_window(&comp, id, Duration::from_secs(10), |w| !w.fullscreen)
        .expect("the window never left fullscreen");
    assert_eq!(restored.geometry.width, REQUESTED_W as i32, "frame width not restored");
    assert_eq!(restored.geometry.height, FRAME_H, "frame height not restored");
    assert!(
        poll_x_geometry(&conn, win, Duration::from_secs(10), |g| {
            g.width == REQUESTED_W && g.height == REQUESTED_H
        }),
        "the SSD content rect was not restored after un-fullscreen"
    );
    assert!(
        poll_net_wm_state(&conn, win, net_wm_state, Duration::from_secs(10), |atoms| {
            !atoms.contains(&fullscreen_atom)
        }),
        "_NET_WM_STATE_FULLSCREEN was never cleared after un-fullscreen"
    );

    conn.destroy_window(win).expect("destroy X11 window");
    conn.flush().expect("flush destroy");
}

/// M2 arm — MINIMIZE write-back. Minimizing a managed X11 window through the WM
/// takes it out of the active/focused set in the model AND reaches the X
/// surface as `_NET_WM_STATE_HIDDEN` (the EWMH iconified marker an X11 app
/// reads). Restore clears both. Non-vacuous *and* regression-guarding: the
/// write-back seam (`Wayland::set_minimized` -> `set_xwayland_surface_minimized`)
/// was missing before this work -- a WM minimize hid the scene node but never
/// told the client -- so without that fix `_NET_WM_STATE_HIDDEN` never appears
/// and this test fails. Skips visibly when Xwayland is absent.
#[test]
fn minimize_x11_window_hides_it_and_reaches_the_surface() {
    // Serialize the heavyweight real-Xwayland e2e tests (see `X11_TEST_LOCK`).
    let _x11_guard = x11_test_guard();
    if !xwayland_on_path() {
        eprintln!("SKIP: Xwayland is not installed on this host; the X11 minimize test cannot run");
        return;
    }
    let comp = Compositor::spawn();
    let Some(display) = comp.xwayland_display() else {
        eprintln!("SKIP: the compositor advertised no Xwayland DISPLAY (Xwayland unavailable)");
        return;
    };
    let (conn, screen_num) = connect_with_retry(&display);
    let screen = conn.setup().roots[screen_num].clone();

    let win = map_managed_x11(&conn, &screen, 400, 300);
    let window = poll_for_window(&comp, MAP_TIMEOUT)
        .expect("the managed X11 window never entered the model");
    let id = window.id;
    // It starts focused (freshly mapped, autofocused).
    assert!(
        poll_snapshot_window(&comp, id, Duration::from_secs(5), |w| w.focused).is_some(),
        "the X11 window never became focused"
    );

    let net_wm_state = intern(&conn, b"_NET_WM_STATE");
    let hidden_atom = intern(&conn, b"_NET_WM_STATE_HIDDEN");

    comp.send(DbCommand::Minimize(id, true));
    // Model: minimized, and it has left the active/focused set (it was the only
    // window, so nothing succeeds it and focus clears).
    let m = poll_snapshot_window(&comp, id, Duration::from_secs(10), |w| w.minimized)
        .expect("the window never minimized in the model");
    assert!(m.minimized, "model window is not minimized");
    assert!(!m.focused, "a minimized window must not stay in the focused set");
    // Surface: the minimize reached the X client as _NET_WM_STATE_HIDDEN.
    assert!(
        poll_net_wm_state(&conn, win, net_wm_state, Duration::from_secs(10), |atoms| {
            atoms.contains(&hidden_atom)
        }),
        "_NET_WM_STATE_HIDDEN was never set -- the WM minimize never reached the X surface"
    );

    // Restore clears both.
    comp.send(DbCommand::Minimize(id, false));
    let r = poll_snapshot_window(&comp, id, Duration::from_secs(10), |w| !w.minimized)
        .expect("the window never restored in the model");
    assert!(!r.minimized, "model window is still minimized after restore");
    assert!(
        poll_net_wm_state(&conn, win, net_wm_state, Duration::from_secs(10), |atoms| {
            !atoms.contains(&hidden_atom)
        }),
        "_NET_WM_STATE_HIDDEN was never cleared after restore"
    );

    conn.destroy_window(win).expect("destroy X11 window");
    conn.flush().expect("flush destroy");
}

/// M2 arm — INTERACTIVE move via `_NET_WM_MOVERESIZE` (the `begin_client_move`
/// seam), distinct from the client-`ConfigureRequest` move the parity test
/// already covers. The X11 client asks the WM to start a title-bar-style drag;
/// a real (virtual) pointer, pressed over the window's content, then moves and
/// the window follows by the pointer delta. Non-vacuous: the press is on the
/// content (not the SSD strip, which would start the compositor's *own* drag),
/// so absent a working `xwayland_request_move` -> `begin_client_move` the
/// pointer motion moves nothing at all. Skips visibly when Xwayland is absent.
#[test]
fn interactive_move_via_net_wm_moveresize_moves_the_window() {
    // Serialize the heavyweight real-Xwayland e2e tests (see `X11_TEST_LOCK`).
    let _x11_guard = x11_test_guard();
    if !xwayland_on_path() {
        eprintln!("SKIP: Xwayland is not installed on this host; the X11 interactive-move test cannot run");
        return;
    }
    let comp = Compositor::spawn();
    let Some(display) = comp.xwayland_display() else {
        eprintln!("SKIP: the compositor advertised no Xwayland DISPLAY (Xwayland unavailable)");
        return;
    };
    let (conn, screen_num) = connect_with_retry(&display);
    let screen = conn.setup().roots[screen_num].clone();

    let win = map_managed_x11(&conn, &screen, 400, 300);
    let window = poll_for_window(&comp, MAP_TIMEOUT)
        .expect("the managed X11 window never entered the model");
    let id = window.id;

    // The virtual pointer's `motion_absolute` is normalized to the output
    // extent, which the headless backend never publishes directly. A fullscreen
    // fills the whole output *exactly* (unlike maximize, which insets by the
    // snap gap), so bounce through it to read the extent, then restore.
    comp.send(DbCommand::Fullscreen(id, true));
    let out = poll_snapshot_window(&comp, id, Duration::from_secs(10), |w| w.fullscreen)
        .expect("the window never fullscreened while reading the output extent")
        .geometry;
    comp.send(DbCommand::Fullscreen(id, false));
    let g0 = poll_snapshot_window(&comp, id, Duration::from_secs(10), |w| !w.fullscreen)
        .expect("the window never left fullscreen")
        .geometry;
    let (ow, oh) = (out.width as u32, out.height as u32);
    assert!(ow > 0 && oh > 0, "output extent must be real, got {out:?}");

    // Press inside the window's CONTENT (below the SSD title bar). A title-bar
    // press would start the compositor's own decoration drag and confound the
    // begin_client_move seam under test.
    let px = (g0.x + g0.width / 2) as f64;
    let py = (g0.y + TITLE_BAR_HEIGHT as i32 + (g0.height - TITLE_BAR_HEIGHT as i32) / 2) as f64;

    let mut vp = VirtualPointerClient::spawn(&comp.socket);
    vp.motion_absolute(px, py, ow, oh);
    vp.frame();
    vp.button(BTN_LEFT, true);
    vp.frame();
    comp.settle();

    // The client asks the WM to begin an interactive move.
    send_net_wm_moveresize(&conn, screen.root, win, px as u32, py as u32);
    conn.flush().expect("flush _NET_WM_MOVERESIZE");
    comp.settle();

    // Drag by a delta that stays clear of the screen edges (so no snap zone
    // engages) and release to drop.
    const DX: i32 = 60;
    const DY: i32 = 45;
    vp.motion_absolute(px + DX as f64, py + DY as f64, ow, oh);
    vp.frame();
    comp.settle();
    vp.button(BTN_LEFT, false);
    vp.frame();
    comp.settle();

    let moved = poll_snapshot_window(&comp, id, Duration::from_secs(10), |w| {
        w.geometry.x != g0.x || w.geometry.y != g0.y
    })
    .expect("the window never moved -- begin_client_move did not run");
    assert!(
        (moved.geometry.x - (g0.x + DX)).abs() <= 3,
        "window x moved to {} (from {}), expected ~{}",
        moved.geometry.x,
        g0.x,
        g0.x + DX
    );
    assert!(
        (moved.geometry.y - (g0.y + DY)).abs() <= 3,
        "window y moved to {} (from {}), expected ~{}",
        moved.geometry.y,
        g0.y,
        g0.y + DY
    );

    conn.destroy_window(win).expect("destroy X11 window");
    conn.flush().expect("flush destroy");
}

/// M2 arm — RESTACK on raise. Two managed X11 windows; the second maps on top
/// of the first, then focusing the first raises it above the second. Proven off
/// the *X server's own stacking order* (`query_tree`), which the xwm keeps in
/// sync via `restack_xwayland_surface` -- so this asserts the restack really
/// reached X, not just the model. Non-vacuous: the initial order is asserted
/// first (B over A), so the post-focus flip (A over B) can only come from the
/// raise path. Skips visibly when Xwayland is absent.
#[test]
fn raising_a_managed_x11_window_restacks_it_above_the_other() {
    // Serialize the heavyweight real-Xwayland e2e tests (see `X11_TEST_LOCK`).
    let _x11_guard = x11_test_guard();
    if !xwayland_on_path() {
        eprintln!("SKIP: Xwayland is not installed on this host; the X11 restack test cannot run");
        return;
    }
    let comp = Compositor::spawn();
    let Some(display) = comp.xwayland_display() else {
        eprintln!("SKIP: the compositor advertised no Xwayland DISPLAY (Xwayland unavailable)");
        return;
    };
    let (conn, screen_num) = connect_with_retry(&display);
    let screen = conn.setup().roots[screen_num].clone();

    // A maps first and is the only window.
    let win_a = map_managed_x11(&conn, &screen, 300, 200);
    let a = poll_for_window(&comp, MAP_TIMEOUT)
        .expect("window A never entered the model");
    let a_id = a.id;

    // B maps second, on top; wait until the model holds both, then take the id
    // that is not A's.
    let win_b = map_managed_x11(&conn, &screen, 300, 200);
    assert!(
        poll_until(&comp, Duration::from_secs(15), |s| s.windows.len() == 2),
        "the second managed X11 window never entered the model"
    );
    let b_id = comp
        .snapshot()
        .windows
        .into_iter()
        .find(|w| w.id != a_id)
        .expect("B must be the second model row")
        .id;

    // Initial X stacking order: B (mapped last, autofocused-and-raised) sits
    // above A.
    assert!(
        poll_stack_above(&conn, screen.root, win_b, win_a, Duration::from_secs(10)),
        "expected the freshly-mapped B to start stacked above A on the X server"
    );

    // Raise A by focusing it (raise_on_focus is on by default).
    comp.send(DbCommand::Focus(a_id));
    assert!(
        poll_snapshot_window(&comp, a_id, Duration::from_secs(10), |w| w.focused).is_some(),
        "A never became focused"
    );
    // The raise reached the X server: A is now stacked above B.
    assert!(
        poll_stack_above(&conn, screen.root, win_a, win_b, Duration::from_secs(10)),
        "focusing A never restacked its X window above B's"
    );
    // Sanity: b_id is the row we raised A over, and both are still modelled.
    assert!(comp.snapshot().windows.iter().any(|w| w.id == b_id), "B left the model unexpectedly");

    conn.destroy_window(win_a).expect("destroy A");
    conn.destroy_window(win_b).expect("destroy B");
    conn.flush().expect("flush destroy");
}

/// M2 arm — WM-INITIATED close. `CloseWindow(id)` on a managed X11 window makes
/// the WM ask the client to close through `close_xwayland_surface`; a client
/// advertising `WM_DELETE_WINDOW` receives that message (the client-observable
/// proof the request reached it), destroys itself, and the model drops the row.
/// Non-vacuous: without the close seam the `WM_DELETE_WINDOW` client message
/// never arrives. Skips visibly when Xwayland is absent.
#[test]
fn wm_initiated_close_reaches_the_x11_client() {
    // Serialize the heavyweight real-Xwayland e2e tests (see `X11_TEST_LOCK`).
    let _x11_guard = x11_test_guard();
    if !xwayland_on_path() {
        eprintln!("SKIP: Xwayland is not installed on this host; the X11 close test cannot run");
        return;
    }
    let comp = Compositor::spawn();
    let Some(display) = comp.xwayland_display() else {
        eprintln!("SKIP: the compositor advertised no Xwayland DISPLAY (Xwayland unavailable)");
        return;
    };
    let (conn, screen_num) = connect_with_retry(&display);
    let screen = conn.setup().roots[screen_num].clone();

    let win = conn.generate_id().expect("generate X11 window id");
    conn.create_window(
        COPY_DEPTH_FROM_PARENT,
        win,
        screen.root,
        0,
        0,
        300,
        200,
        0,
        WindowClass::INPUT_OUTPUT,
        screen.root_visual,
        // Select StructureNotify so the client receives the WM's messages.
        &CreateWindowAux::new()
            .background_pixel(screen.white_pixel)
            .event_mask(EventMask::STRUCTURE_NOTIFY),
    )
    .expect("create X11 window");
    let wm_class = format!("{WINDOW_INSTANCE}\0{WINDOW_CLASS}\0");
    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_CLASS, AtomEnum::STRING, wm_class.as_bytes())
        .expect("set WM_CLASS");
    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_NAME, AtomEnum::STRING, WINDOW_TITLE.as_bytes())
        .expect("set WM_NAME");

    // Advertise WM_DELETE_WINDOW so a graceful close is delivered as a client
    // message rather than an XKillClient.
    let wm_protocols = intern(&conn, b"WM_PROTOCOLS");
    let wm_delete = intern(&conn, b"WM_DELETE_WINDOW");
    conn.change_property32(PropMode::REPLACE, win, wm_protocols, AtomEnum::ATOM, &[wm_delete])
        .expect("set WM_PROTOCOLS");

    conn.map_window(win).expect("map X11 window");
    conn.flush().expect("flush");

    let window = poll_for_window(&comp, MAP_TIMEOUT)
        .expect("the managed X11 window never entered the model");
    let id = window.id;

    // WM asks it to close (the `org.icedtea.WM.CloseWindow` path).
    comp.send(DbCommand::Close(id));
    assert!(
        wait_for_wm_delete(&conn, wm_protocols, wm_delete, Duration::from_secs(10)),
        "the X client never received WM_DELETE_WINDOW -- the WM close never reached it"
    );

    // A well-behaved client tears itself down; the model then drops the row.
    conn.destroy_window(win).expect("destroy X11 window");
    conn.flush().expect("flush destroy");
    assert!(
        poll_until(&comp, Duration::from_secs(15), |s| s.windows.is_empty()),
        "the model still held the window after the client closed"
    );
}

/// M2 arm — LIVE title + class updates. Changing `WM_NAME`/`_NET_WM_NAME` and
/// then `WM_CLASS` on a mapped managed X11 window flows through
/// `xwayland_title_changed`/`xwayland_class_changed` into the model's title and
/// app_id, read fresh off a snapshot. Non-vacuous: the pre-change identity is
/// asserted first, so the post-change values can only come from the live
/// property seams. Skips visibly when Xwayland is absent.
#[test]
fn live_title_and_class_updates_reach_the_model() {
    // Serialize the heavyweight real-Xwayland e2e tests (see `X11_TEST_LOCK`).
    let _x11_guard = x11_test_guard();
    if !xwayland_on_path() {
        eprintln!("SKIP: Xwayland is not installed on this host; the X11 live-property test cannot run");
        return;
    }
    let comp = Compositor::spawn();
    let Some(display) = comp.xwayland_display() else {
        eprintln!("SKIP: the compositor advertised no Xwayland DISPLAY (Xwayland unavailable)");
        return;
    };
    let (conn, screen_num) = connect_with_retry(&display);
    let screen = conn.setup().roots[screen_num].clone();

    let win = map_managed_x11(&conn, &screen, 400, 300);
    let window = poll_for_window(&comp, MAP_TIMEOUT)
        .expect("the managed X11 window never entered the model");
    let id = window.id;
    assert_eq!(window.app_id, WINDOW_CLASS, "baseline app_id");
    assert_eq!(window.title, WINDOW_TITLE, "baseline title");

    // Live title change: set both WM_NAME and _NET_WM_NAME.
    const NEW_TITLE: &str = "Renamed Live Window";
    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_NAME, AtomEnum::STRING, NEW_TITLE.as_bytes())
        .expect("set WM_NAME");
    let net_wm_name = intern(&conn, b"_NET_WM_NAME");
    let utf8_string = intern(&conn, b"UTF8_STRING");
    conn.change_property8(PropMode::REPLACE, win, net_wm_name, utf8_string, NEW_TITLE.as_bytes())
        .expect("set _NET_WM_NAME");
    conn.flush().expect("flush title change");
    assert!(
        poll_snapshot_window(&comp, id, Duration::from_secs(10), |w| w.title == NEW_TITLE).is_some(),
        "the live WM_NAME change never reached the model title"
    );

    // Live class change.
    const NEW_CLASS: &str = "RenamedLiveClass";
    let wm_class = format!("{WINDOW_INSTANCE}\0{NEW_CLASS}\0");
    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_CLASS, AtomEnum::STRING, wm_class.as_bytes())
        .expect("set WM_CLASS");
    conn.flush().expect("flush class change");
    // app_id has no dedicated update wire field in the contract (a noted,
    // out-of-scope minor), but a fresh snapshot reads the model directly, so
    // the updated app_id is observable there.
    assert!(
        poll_snapshot_window(&comp, id, Duration::from_secs(10), |w| w.app_id == NEW_CLASS).is_some(),
        "the live WM_CLASS change never reached the model app_id"
    );

    conn.destroy_window(win).expect("destroy X11 window");
    conn.flush().expect("flush destroy");
}

/// M3 — override-redirect pop-up. An OR X11 window maps at its own client
/// coordinates as an *unmanaged* pop-up: it is placed at exactly those coords,
/// is **not** entered into the `Window` model (so it wears no SSD and is no
/// alt-tab/focus candidate), its scene node stacks in the band **above** a
/// mapped managed toplevel, and — being a focus-taking OR window (wlroots'
/// heuristic hands a plain OR window the keyboard) — it holds the seat keyboard.
/// Skips visibly when Xwayland is absent, like every other test here.
#[test]
fn override_redirect_popup_is_an_unmanaged_placed_focused_pop_up() {
    let _x11_guard = x11_test_guard();
    if !xwayland_on_path() {
        eprintln!("SKIP: Xwayland is not installed on this host; the OR pop-up test cannot run");
        return;
    }

    let comp = Compositor::spawn();
    let Some(display) = comp.xwayland_display() else {
        eprintln!("SKIP: the compositor advertised no Xwayland DISPLAY (Xwayland unavailable)");
        return;
    };
    let (conn, screen_num) = connect_with_retry(&display);
    let screen = &conn.setup().roots[screen_num];

    // A managed toplevel first, so there is a real managed window in the
    // `Band::Toplevel` band for the OR pop-up to stack above.
    let managed = map_managed_x11(&conn, screen, 400, 300);
    let base = poll_for_window(&comp, MAP_TIMEOUT)
        .expect("the managed X11 window never entered the model");
    assert_eq!(base.app_id, WINDOW_CLASS);

    // Now an override-redirect pop-up at chosen absolute coordinates.
    const OR_X: i16 = 320;
    const OR_Y: i16 = 240;
    const OR_W: u16 = 160;
    const OR_H: u16 = 90;
    let popup = map_override_redirect_x11(&conn, screen, OR_X, OR_Y, OR_W, OR_H);

    // The compositor tracks exactly one OR pop-up, placed at the client coords,
    // in the band above managed toplevels, holding the keyboard.
    let probe = poll_or(&comp, Duration::from_secs(15), |ps| ps.len() == 1)
        .expect("the OR pop-up never appeared in the compositor's OR side-table");
    let p = probe[0];
    assert_eq!(
        p.position,
        (OR_X as i32, OR_Y as i32),
        "the OR pop-up was not placed at its client-requested coordinates"
    );
    assert!(p.above_toplevel, "the OR pop-up did not stack above managed toplevels");
    assert!(
        p.keyboard_focused,
        "the focus-taking OR pop-up did not receive the seat keyboard"
    );

    // It is NOT in the `Window` model — the managed window is still the only
    // row, so the pop-up is no alt-tab/focus candidate and wears no SSD.
    let snap = comp.snapshot();
    assert_eq!(
        snap.windows.len(),
        1,
        "the OR pop-up must not enter the Window model, got {:?}",
        snap.windows
    );
    assert!(snap.windows.iter().all(|w| w.id == base.id));

    // Non-vacuous "no SSD": a managed window is configured to a content rect one
    // title-bar shorter than its frame; an OR pop-up keeps its full requested
    // size, because the compositor never insets or reconfigures it.
    let full_size = poll_x_geometry(&conn, popup, Duration::from_secs(10), |g| {
        g.width == OR_W && g.height == OR_H
    });
    assert!(full_size, "the OR pop-up was resized/decorated; it must keep its client size");

    // Dismissing the pop-up (unmap) removes it from the side-table and hands the
    // keyboard back to the managed window.
    conn.unmap_window(popup).expect("unmap OR pop-up");
    conn.flush().expect("flush unmap");
    assert!(
        poll_or(&comp, Duration::from_secs(15), |ps| ps.is_empty()).is_some(),
        "the OR pop-up was not removed from the side-table on unmap"
    );

    conn.destroy_window(popup).expect("destroy OR pop-up");
    conn.destroy_window(managed).expect("destroy managed window");
    conn.flush().expect("flush destroy");
}

/// M3 — window-type placement. A managed transient dialog (`WM_TRANSIENT_FOR` a
/// parent, `_NET_WM_WINDOW_TYPE_DIALOG`) is centered over its parent's frame,
/// rather than being cascade-placed like an ordinary top-level. Asserts against
/// the real model geometry of both windows. Skips visibly when Xwayland absent.
#[test]
fn managed_transient_dialog_is_centered_over_its_parent() {
    let _x11_guard = x11_test_guard();
    if !xwayland_on_path() {
        eprintln!("SKIP: Xwayland is not installed on this host; the dialog placement test cannot run");
        return;
    }

    let comp = Compositor::spawn();
    let Some(display) = comp.xwayland_display() else {
        eprintln!("SKIP: the compositor advertised no Xwayland DISPLAY (Xwayland unavailable)");
        return;
    };
    let (conn, screen_num) = connect_with_retry(&display);
    let screen = &conn.setup().roots[screen_num];

    // The parent, mapped and placed by the WM.
    let parent = map_managed_x11(&conn, screen, 600, 500);
    let parent_win = poll_for_window(&comp, MAP_TIMEOUT)
        .expect("the parent X11 window never entered the model");
    let parent_geo = parent_win.geometry;

    // The dialog: transient for the parent, typed as a dialog, distinct title so
    // it is identifiable in the model. Properties set before map so the xwm has
    // them by the time the map event fires.
    const DIALOG_W: u16 = 240;
    const DIALOG_H: u16 = 160;
    const DIALOG_TITLE: &str = "Icedtea Dialog";
    let dialog = conn.generate_id().expect("generate dialog id");
    conn.create_window(
        COPY_DEPTH_FROM_PARENT,
        dialog,
        screen.root,
        0,
        0,
        DIALOG_W,
        DIALOG_H,
        0,
        WindowClass::INPUT_OUTPUT,
        screen.root_visual,
        &CreateWindowAux::new().background_pixel(screen.white_pixel),
    )
    .expect("create dialog window");
    let wm_class = format!("{WINDOW_INSTANCE}\0{WINDOW_CLASS}\0");
    conn.change_property8(PropMode::REPLACE, dialog, AtomEnum::WM_CLASS, AtomEnum::STRING, wm_class.as_bytes())
        .expect("set dialog WM_CLASS");
    conn.change_property8(PropMode::REPLACE, dialog, AtomEnum::WM_NAME, AtomEnum::STRING, DIALOG_TITLE.as_bytes())
        .expect("set dialog WM_NAME");
    conn.change_property32(PropMode::REPLACE, dialog, AtomEnum::WM_TRANSIENT_FOR, AtomEnum::WINDOW, &[parent])
        .expect("set WM_TRANSIENT_FOR");
    let wt_atom = intern(&conn, b"_NET_WM_WINDOW_TYPE");
    let dialog_atom = intern(&conn, b"_NET_WM_WINDOW_TYPE_DIALOG");
    conn.change_property32(PropMode::REPLACE, dialog, wt_atom, AtomEnum::ATOM, &[dialog_atom])
        .expect("set _NET_WM_WINDOW_TYPE");
    conn.map_window(dialog).expect("map dialog");
    conn.flush().expect("flush dialog");

    // Wait until the dialog is in the model (two windows), then read its frame.
    let dialog_win = poll_named_window(&comp, DIALOG_TITLE, Duration::from_secs(15))
        .expect("the dialog never entered the model");
    let dg = dialog_win.geometry;

    // Centered over the parent frame: top-left = parent center − dialog half.
    let expect_x = parent_geo.x + (parent_geo.width - dg.width) / 2;
    let expect_y = parent_geo.y + (parent_geo.height - dg.height) / 2;
    assert_eq!(
        (dg.x, dg.y),
        (expect_x, expect_y),
        "dialog frame {:?} is not centered over parent frame {:?}",
        dg,
        parent_geo
    );

    conn.destroy_window(dialog).expect("destroy dialog");
    conn.destroy_window(parent).expect("destroy parent");
    conn.flush().expect("flush destroy");
}

/// M3 — runtime override-redirect flip. A live managed window that flips its
/// override-redirect attribute migrates out of the `Window` model into the OR
/// side-table (and back), with no leak and no double-track. Asserts against the
/// real model and OR-scene state on each transition. Skips visibly when absent.
#[test]
fn runtime_override_redirect_flip_migrates_between_managed_and_or() {
    let _x11_guard = x11_test_guard();
    if !xwayland_on_path() {
        eprintln!("SKIP: Xwayland is not installed on this host; the OR flip test cannot run");
        return;
    }

    let comp = Compositor::spawn();
    let Some(display) = comp.xwayland_display() else {
        eprintln!("SKIP: the compositor advertised no Xwayland DISPLAY (Xwayland unavailable)");
        return;
    };
    let (conn, screen_num) = connect_with_retry(&display);
    let screen = &conn.setup().roots[screen_num];

    // Start managed.
    let win = map_managed_x11(&conn, screen, 300, 200);
    let modelled = poll_for_window(&comp, MAP_TIMEOUT)
        .expect("the window never entered the model as managed");
    assert_eq!(comp.snapshot().windows.len(), 1);
    assert!(comp.xwayland_override_redirect().is_empty(), "not OR yet");
    let _ = modelled;

    // Flip to override-redirect *while mapped*, then nudge it so the xwm sees a
    // ConfigureNotify carrying the new flag and emits set_override_redirect —
    // this drives the `xwayland_override_redirect_changed` migration handler.
    conn.change_window_attributes(win, &ChangeWindowAttributesAux::new().override_redirect(1))
        .expect("set override_redirect = 1");
    conn.configure_window(win, &ConfigureWindowAux::new().x(360).y(300))
        .expect("nudge to force ConfigureNotify");
    conn.flush().expect("flush flip to OR");

    // It leaves the model and becomes a tracked OR pop-up — the managed→OR
    // runtime migration, with no model leak.
    assert!(
        poll_until(&comp, Duration::from_secs(15), |s| s.windows.is_empty()),
        "the window did not leave the Window model when it became override-redirect"
    );
    // Assert the migrated pop-up's *real scene state*, not just the side-table
    // count (review finding #14): the still-live scene node must be reparented
    // into the band above managed toplevels and repositioned at the client
    // coordinates the flip's ConfigureNotify carried (360, 300) — a bookkeeping
    // count of 1 would pass even if the reparent/reposition silently failed.
    let migrated = poll_or(&comp, Duration::from_secs(15), |ps| {
        ps.len() == 1 && ps[0].above_toplevel && ps[0].position == (360, 300)
    })
    .expect("the flipped window did not migrate into the OR band at its client coordinates");
    let _ = migrated;

    // Flip back to managed. An already-mapped window is only *adopted* into
    // management at map time (the xwm intercepts a MapRequest, which a live
    // window does not re-issue), so the realistic OR→managed migration is
    // unmap → clear the flag → remap. On the remap the surface is no longer
    // override-redirect, so it re-enters the managed model through the shared
    // path — proving the reverse migration with no OR side-table leak.
    conn.unmap_window(win).expect("unmap before adopting back");
    conn.change_window_attributes(win, &ChangeWindowAttributesAux::new().override_redirect(0))
        .expect("set override_redirect = 0");
    conn.flush().expect("flush unmap+clear");
    assert!(
        poll_or(&comp, Duration::from_secs(15), |ps| ps.is_empty()).is_some(),
        "the OR side-table still held the window after it unmapped"
    );
    conn.map_window(win).expect("remap as managed");
    conn.flush().expect("flush remap");

    // It re-enters the model and the OR side-table stays empty — no double-track.
    assert!(
        poll_until(&comp, Duration::from_secs(15), |s| s.windows.len() == 1),
        "the window did not re-enter the Window model when it became managed again"
    );
    assert!(
        comp.xwayland_override_redirect().is_empty(),
        "the OR side-table still held the window after it became managed again"
    );

    conn.destroy_window(win).expect("destroy window");
    conn.flush().expect("flush destroy");
}

/// Create and map one override-redirect X11 window of `w`x`h` at root
/// coordinates `(x, y)` — an unmanaged pop-up (menu/tooltip/combo). The
/// `override_redirect` attribute is what tells the xwm to leave it unmanaged.
fn map_override_redirect_x11(
    conn: &impl Connection,
    screen: &x11rb::protocol::xproto::Screen,
    x: i16,
    y: i16,
    w: u16,
    h: u16,
) -> u32 {
    let win = conn.generate_id().expect("generate OR window id");
    conn.create_window(
        COPY_DEPTH_FROM_PARENT,
        win,
        screen.root,
        x,
        y,
        w,
        h,
        0,
        WindowClass::INPUT_OUTPUT,
        screen.root_visual,
        &CreateWindowAux::new()
            .background_pixel(screen.white_pixel)
            .override_redirect(1),
    )
    .expect("create OR window");
    conn.map_window(win).expect("map OR window");
    conn.flush().expect("flush OR map");
    win
}

/// Poll the OR side-table probe until `pred` holds over the reported pop-ups,
/// returning the probe snapshot that satisfied it.
fn poll_or(
    comp: &Compositor,
    timeout: Duration,
    pred: impl Fn(&[icedtea_compositor::dbus::OverrideRedirectProbe]) -> bool,
) -> Option<Vec<icedtea_compositor::dbus::OverrideRedirectProbe>> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let ps = comp.xwayland_override_redirect();
        if pred(&ps) {
            return Some(ps);
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    None
}

/// Poll the snapshot until a window with the given `title` appears, returning
/// it — used to pick a specific one out when several share a class.
fn poll_named_window(
    comp: &Compositor,
    title: &str,
    timeout: Duration,
) -> Option<icedtea_contract::WindowInfo> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Some(w) = comp.snapshot().windows.into_iter().find(|w| w.title == title) {
            return Some(w);
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    None
}

/// Poll the X server's own record of `win`'s geometry (updated by the xwm when
/// the compositor configures the surface) until `pred` holds or the timeout
/// elapses. This is what makes the SSD and maximize assertions non-vacuous:
/// the geometry the client actually ended up with, not just the model's.
fn poll_x_geometry(
    conn: &impl Connection,
    win: u32,
    timeout: Duration,
    pred: impl Fn(&x11rb::protocol::xproto::GetGeometryReply) -> bool,
) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Ok(cookie) = conn.get_geometry(win)
            && let Ok(reply) = cookie.reply()
            && pred(&reply)
        {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

/// Poll the snapshot until it holds a window with `id` for which `pred` holds,
/// returning that window.
fn poll_snapshot_window(
    comp: &Compositor,
    id: WindowId,
    timeout: Duration,
    pred: impl Fn(&icedtea_contract::WindowInfo) -> bool,
) -> Option<icedtea_contract::WindowInfo> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Some(w) = comp.snapshot().windows.into_iter().find(|w| w.id == id && pred(w)) {
            return Some(w);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    None
}

/// Connect an X11 client *and* wait until the compositor has finished bringing
/// Xwayland up -- specifically past the crate's `set_xwayland_seat`, which is
/// what arms the clipboard/primary/DND bridge. `set_xwayland_seat` runs inside
/// `State::xwayland_ready`, so once the compositor reports ready
/// (`Compositor::xwayland_ready`, backed by `DbCommand::XwaylandReady`) the seat
/// is wired and the bridge armed. Without this gate an X11 client that grabs a
/// selection before the seat is wired is silently dropped by the xwm.
///
/// (This used to infer readiness from `DISPLAY` being republished into the
/// process environment on `ready`; that republish was a `set_var` from inside
/// `run_all` and is gone — DISPLAY is now published once, at boot. See review
/// finding #5.)
fn wait_for_xwayland_ready(comp: &Compositor, display: &str) -> (x11rb::rust_connection::RustConnection, usize) {
    // Connecting execs the lazy Xwayland, whose `ready` drives `set_xwayland_seat`.
    let pair = connect_with_retry(display);
    let deadline = Instant::now() + Duration::from_secs(15);
    while !comp.xwayland_ready() {
        assert!(
            Instant::now() < deadline,
            "Xwayland never signalled ready; the selection/DND bridge would not be armed"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
    pair
}

/// Connect an X11 client to `display`, retrying briefly to absorb the lazy
/// `Xwayland` exec. Panics with a clear message if it never comes up.
fn connect_with_retry(
    display: &str,
) -> (x11rb::rust_connection::RustConnection, usize) {
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut last_err = None;
    while Instant::now() < deadline {
        match x11rb::connect(Some(display)) {
            Ok(pair) => return pair,
            Err(err) => {
                last_err = Some(format!("{err:?}"));
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }
    panic!("could not connect an X11 client to {display}: {last_err:?}");
}

/// Poll the compositor snapshot until it holds a managed window, returning it.
fn poll_for_window(
    comp: &Compositor,
    timeout: Duration,
) -> Option<icedtea_contract::WindowInfo> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Some(window) = comp.snapshot().windows.into_iter().next() {
            return Some(window);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    None
}

/// Poll the snapshot until `pred` holds or the timeout elapses.
fn poll_until(
    comp: &Compositor,
    timeout: Duration,
    pred: impl Fn(&icedtea_contract::Snapshot) -> bool,
) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if pred(&comp.snapshot()) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

/// Intern an atom, panicking with a clear message on failure.
fn intern(conn: &impl Connection, name: &[u8]) -> Atom {
    conn.intern_atom(false, name)
        .expect("intern_atom request")
        .reply()
        .expect("intern_atom reply")
        .atom
}

/// Read the X window's current `_NET_WM_STATE` atom list (empty on any miss --
/// the property is absent until the xwm first writes it).
fn read_net_wm_state(conn: &impl Connection, win: u32, state_atom: Atom) -> Vec<Atom> {
    conn.get_property(false, win, state_atom, AtomEnum::ATOM, 0, 64)
        .ok()
        .and_then(|cookie| cookie.reply().ok())
        .and_then(|reply| reply.value32().map(|it| it.collect()))
        .unwrap_or_default()
}

/// Poll the X window's `_NET_WM_STATE` until `pred` holds over its atom list or
/// the timeout elapses. This is what makes the fullscreen/minimize write-back
/// assertions non-vacuous: the state atoms the client actually ended up with,
/// as the xwm reflected the WM's `set_xwayland_surface_*` call onto the window.
fn poll_net_wm_state(
    conn: &impl Connection,
    win: u32,
    state_atom: Atom,
    timeout: Duration,
    pred: impl Fn(&[Atom]) -> bool,
) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if pred(&read_net_wm_state(conn, win, state_atom)) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

/// Create, identify (WM_CLASS/WM_NAME) and map one managed (non-override-
/// redirect) top-level X11 window of `w`x`h`, returning its X id. The class is
/// a non-CSD one, so `decoration::has_ssd` decorates it -- the same setup the
/// M1/M2 tests use, factored out for the arm-specific tests below.
fn map_managed_x11(
    conn: &impl Connection,
    screen: &x11rb::protocol::xproto::Screen,
    w: u16,
    h: u16,
) -> u32 {
    let win = conn.generate_id().expect("generate X11 window id");
    conn.create_window(
        COPY_DEPTH_FROM_PARENT,
        win,
        screen.root,
        0,
        0,
        w,
        h,
        0,
        WindowClass::INPUT_OUTPUT,
        screen.root_visual,
        &CreateWindowAux::new().background_pixel(screen.white_pixel),
    )
    .expect("create X11 window");
    let wm_class = format!("{WINDOW_INSTANCE}\0{WINDOW_CLASS}\0");
    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_CLASS, AtomEnum::STRING, wm_class.as_bytes())
        .expect("set WM_CLASS");
    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_NAME, AtomEnum::STRING, WINDOW_TITLE.as_bytes())
        .expect("set WM_NAME");
    conn.map_window(win).expect("map X11 window");
    conn.flush().expect("flush");
    win
}

/// Send an EWMH `_NET_WM_MOVERESIZE` "move" client message to the root, the way
/// a real X11 app asks the WM to start an interactive drag of its title bar.
/// `x_root`/`y_root` are advisory (wlroots' xwm starts the grab from the live
/// pointer regardless), passed as the current press point for faithfulness.
fn send_net_wm_moveresize(
    conn: &impl Connection,
    root: u32,
    win: u32,
    x_root: u32,
    y_root: u32,
) {
    let atom = intern(conn, b"_NET_WM_MOVERESIZE");
    // data: [x_root, y_root, direction, button, source-indication].
    let data = [x_root, y_root, NET_WM_MOVERESIZE_MOVE, BTN_LEFT, 1];
    let event = ClientMessageEvent::new(32, win, atom, data);
    conn.send_event(
        false,
        root,
        EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
        event,
    )
    .expect("send _NET_WM_MOVERESIZE");
}

/// Poll `query_tree` on `root` until `upper`'s X window is stacked above
/// `lower`'s (later in the bottom-to-top child list) or the timeout elapses.
/// Both ids must be present in the tree for the predicate to hold, which is
/// what makes a restack assertion non-vacuous against the real X server.
fn poll_stack_above(
    conn: &impl Connection,
    root: u32,
    upper: u32,
    lower: u32,
    timeout: Duration,
) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Ok(cookie) = conn.query_tree(root)
            && let Ok(reply) = cookie.reply()
        {
            let iu = reply.children.iter().position(|&c| c == upper);
            let il = reply.children.iter().position(|&c| c == lower);
            if let (Some(iu), Some(il)) = (iu, il)
                && iu > il
            {
                return true;
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

/// Poll the X connection's event stream for a `WM_DELETE_WINDOW` client message
/// (a `WM_PROTOCOLS` message whose first data word is the delete atom) until it
/// arrives or the timeout elapses -- the client-observable proof that a
/// WM-initiated close reached the X11 client through `close_xwayland_surface`.
fn wait_for_wm_delete(
    conn: &impl Connection,
    wm_protocols: Atom,
    wm_delete: Atom,
    timeout: Duration,
) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        while let Ok(Some(event)) = conn.poll_for_event() {
            if let x11rb::protocol::Event::ClientMessage(ev) = event
                && ev.type_ == wm_protocols
                && ev.data.as_data32()[0] == wm_delete
            {
                return true;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

// ---------------------------------------------------------------------------
// M4 — clipboard / primary / DnD bridge + DISPLAY / cursor robustness.
//
// These prove, end-to-end against a real Xwayland, that wlroots' `xwm`
// automatically bridges the X11 CLIPBOARD/PRIMARY selections to the compositor's
// `wlr_seat` selections once `wlr_xwayland_set_seat` is set (which the crate does
// on `ready`). The compositor writes no selection code of its own -- the M4 job
// is to *verify* the bridge, which is exactly what these tests do: an X11 client
// and a Wayland `data-control` client exchange a text/plain payload across the
// boundary, both directions, for both selections, and the actual bytes are
// asserted to round-trip. The data-control device is the Wayland reader because
// it is focus-less (like a real clipboard manager) -- the X11 tests map no
// Wayland toplevel to take focus, so a `wl_data_device`/`zwp_primary_selection`
// reader would never be offered the selection.

/// The Wayland mime `wlr_xwm` maps the X11 `UTF8_STRING` selection target to
/// (and back). Both directions of every selection test use it, so the assertion
/// is on the mapped-mime path the bridge really takes, not a raw atom.
const SELECTION_MIME: &str = "text/plain;charset=utf-8";

/// A distinctive clipboard payload (with non-ASCII, so a botched charset
/// mapping would corrupt it) proving the CLIPBOARD bridge round-trips bytes.
const X11_CLIPBOARD_PAYLOAD: &[u8] = "icedtea⇄x11 CLIPBOARD ✂".as_bytes();
/// The clipboard payload for the Wayland→X11 direction.
const WL_CLIPBOARD_PAYLOAD: &[u8] = "wayland⇄icedtea CLIPBOARD 📋".as_bytes();
/// The PRIMARY payload set by the X11 side.
const X11_PRIMARY_PAYLOAD: &[u8] = "icedtea⇄x11 PRIMARY ⌗".as_bytes();
/// The PRIMARY payload set by the Wayland side.
const WL_PRIMARY_PAYLOAD: &[u8] = "wayland⇄icedtea PRIMARY ⎘".as_bytes();

/// M4 — CLIPBOARD, X11 → Wayland. An X11 client owns the CLIPBOARD selection and
/// serves a UTF-8 payload; a focus-less Wayland `data-control` client is offered
/// the bridged selection and reads the exact bytes back. Non-vacuous: the bytes
/// are compared for equality, and the payload carries non-ASCII, so neither a
/// missing bridge (no offer) nor a charset mismatch could pass. Skips visibly
/// when Xwayland is absent.
#[test]
fn x11_clipboard_selection_bridges_to_a_wayland_reader() {
    let _x11_guard = x11_test_guard();
    if !xwayland_on_path() {
        eprintln!("SKIP: Xwayland is not installed; the X11→Wayland clipboard bridge test cannot run");
        return;
    }
    let comp = Compositor::spawn();
    let Some(display) = comp.xwayland_display() else {
        eprintln!("SKIP: the compositor advertised no Xwayland DISPLAY (Xwayland unavailable)");
        return;
    };
    // Gate on the bridge being armed (`set_seat` done) before the owner takes
    // the selection -- an owner that grabs it earlier is dropped by the xwm.
    let (conn, _screen_num) = wait_for_xwayland_ready(&comp, &display);

    let mut manager = DataControlClient::spawn(&comp.socket);
    // The X11 owner runs its own event loop on a second connection, servicing
    // the xwm's TARGETS/data requests as a real X app would.
    let _owner = X11SelectionOwner::spawn(display.clone(), "CLIPBOARD", X11_CLIPBOARD_PAYLOAD.to_vec());

    assert!(
        manager.wait_until(|c| c.has_offer()),
        "the X11 CLIPBOARD selection never reached the Wayland seat as a data-control offer"
    );
    let got = manager.read_offer_blocking(SELECTION_MIME);
    assert_eq!(
        got, X11_CLIPBOARD_PAYLOAD,
        "the CLIPBOARD bytes did not round-trip X11 -> Wayland (got {:?})",
        String::from_utf8_lossy(&got)
    );
    drop(conn);
}

/// M4 — CLIPBOARD, Wayland → X11. A focus-less `data-control` client owns the
/// clipboard; an X11 client converts the CLIPBOARD selection and reads the exact
/// bytes. Non-vacuous: byte-equality on a non-ASCII payload, and the X reader
/// returns `None` (fails) if the bridge never made the xwm the CLIPBOARD owner.
/// Skips visibly when Xwayland is absent.
#[test]
fn wayland_clipboard_selection_bridges_to_an_x11_reader() {
    let _x11_guard = x11_test_guard();
    if !xwayland_on_path() {
        eprintln!("SKIP: Xwayland is not installed; the Wayland→X11 clipboard bridge test cannot run");
        return;
    }
    let comp = Compositor::spawn();
    let Some(display) = comp.xwayland_display() else {
        eprintln!("SKIP: the compositor advertised no Xwayland DISPLAY (Xwayland unavailable)");
        return;
    };
    let (conn, screen_num) = wait_for_xwayland_ready(&comp, &display);
    let screen = conn.setup().roots[screen_num].clone();
    // wlroots refuses to serve a Wayland-owned selection to X unless a focused
    // Xwayland surface is asking, so map one that the compositor auto-focuses.
    let focused = map_focused_managed_x11(&comp, &conn, &screen);

    let mut manager = DataControlClient::spawn(&comp.socket);
    manager.set_clipboard(SELECTION_MIME, WL_CLIPBOARD_PAYLOAD);

    // The X reader converts CLIPBOARD and reads the property; the data-control
    // owner must be pumped meanwhile so its source services the `send` the xwm
    // forwards (owner and reader are both on this thread).
    let got = x11_read_selection(&conn, &screen, "CLIPBOARD", Duration::from_secs(15), || {
        manager.pump()
    });
    assert_eq!(
        got.as_deref(),
        Some(WL_CLIPBOARD_PAYLOAD),
        "the CLIPBOARD bytes did not round-trip Wayland -> X11 (got {:?})",
        got.as_ref().map(|b| String::from_utf8_lossy(b))
    );
    conn.destroy_window(focused).expect("destroy focused window");
}

/// M4 — PRIMARY, X11 → Wayland. As the CLIPBOARD X11→Wayland test, but over the
/// PRIMARY (middle-click) selection, read through data-control v2's focus-less
/// primary offer. Proves the second selection the xwm bridges. Skips visibly
/// when Xwayland is absent.
#[test]
fn x11_primary_selection_bridges_to_a_wayland_reader() {
    let _x11_guard = x11_test_guard();
    if !xwayland_on_path() {
        eprintln!("SKIP: Xwayland is not installed; the X11→Wayland primary bridge test cannot run");
        return;
    }
    let comp = Compositor::spawn();
    let Some(display) = comp.xwayland_display() else {
        eprintln!("SKIP: the compositor advertised no Xwayland DISPLAY (Xwayland unavailable)");
        return;
    };
    let (conn, _screen_num) = wait_for_xwayland_ready(&comp, &display);

    let mut manager = DataControlClient::spawn(&comp.socket);
    let _owner = X11SelectionOwner::spawn(display.clone(), "PRIMARY", X11_PRIMARY_PAYLOAD.to_vec());

    assert!(
        manager.wait_until(|c| c.has_primary_offer()),
        "the X11 PRIMARY selection never reached the Wayland seat as a data-control primary offer"
    );
    let got = manager.read_primary_offer_blocking(SELECTION_MIME);
    assert_eq!(
        got, X11_PRIMARY_PAYLOAD,
        "the PRIMARY bytes did not round-trip X11 -> Wayland (got {:?})",
        String::from_utf8_lossy(&got)
    );
    drop(conn);
}

/// M4 — PRIMARY, Wayland → X11. A `data-control` client owns the primary
/// selection focus-lessly; an X11 client converts PRIMARY and reads the exact
/// bytes. Skips visibly when Xwayland is absent.
#[test]
fn wayland_primary_selection_bridges_to_an_x11_reader() {
    let _x11_guard = x11_test_guard();
    if !xwayland_on_path() {
        eprintln!("SKIP: Xwayland is not installed; the Wayland→X11 primary bridge test cannot run");
        return;
    }
    let comp = Compositor::spawn();
    let Some(display) = comp.xwayland_display() else {
        eprintln!("SKIP: the compositor advertised no Xwayland DISPLAY (Xwayland unavailable)");
        return;
    };
    let (conn, screen_num) = wait_for_xwayland_ready(&comp, &display);
    let screen = conn.setup().roots[screen_num].clone();
    let focused = map_focused_managed_x11(&comp, &conn, &screen);

    let mut manager = DataControlClient::spawn(&comp.socket);
    manager.set_primary(SELECTION_MIME, WL_PRIMARY_PAYLOAD);

    let got = x11_read_selection(&conn, &screen, "PRIMARY", Duration::from_secs(15), || {
        manager.pump()
    });
    assert_eq!(
        got.as_deref(),
        Some(WL_PRIMARY_PAYLOAD),
        "the PRIMARY bytes did not round-trip Wayland -> X11 (got {:?})",
        got.as_ref().map(|b| String::from_utf8_lossy(b))
    );
    conn.destroy_window(focused).expect("destroy focused window");
}

/// M4 — XDND / `wl_data_device` bridge wiring. A full cross-boundary drag gesture
/// is impractical to drive in the headless harness (it needs a live pointer grab
/// crossing an X11 surface and a Wayland drop target in lockstep), so per the
/// spec's "verify not build" this asserts the *wiring* the bridge rides on and
/// documents what is proven by construction:
///
///   * `wl_data_device_manager` is advertised, so a Wayland client has a data
///     device to receive an X11-originated drag offer (and to originate one).
///   * The `wlr_xwm` XDND bridge and the clipboard/primary bridges are the *same
///     mechanism* on the *same* `wlr_seat` -- all four come up together the
///     instant `wlr_xwayland_set_seat` runs on `ready`. The four selection tests
///     above prove that seat wiring is live end-to-end; XDND cannot be wired
///     differently, because it is not wired separately. Hence: selection bridge
///     proven end-to-end ⇒ the DnD data-device path is in place by construction.
///
/// Concretely this drives a full X11→Wayland data-device transfer (an X11
/// selection owner's bytes read back through a Wayland client), so it fails if
/// either the manager global regresses OR the shared data-device transfer path
/// stops carrying data — not a tautology. The one thing still NOT exercised is a
/// live XdndEnter/Position/Status/Drop pointer-grab drag gesture; that needs an
/// X11 drag source and a Wayland drop target moving in lockstep and is a tracked
/// follow-up. Skips visibly when Xwayland is absent (the seat/bridge only exists
/// once Xwayland is ready).
#[test]
fn xdnd_data_device_bridge_is_wired() {
    let _x11_guard = x11_test_guard();
    if !xwayland_on_path() {
        eprintln!("SKIP: Xwayland is not installed; the XDND wiring test cannot run");
        return;
    }
    let comp = Compositor::spawn();
    let Some(display) = comp.xwayland_display() else {
        eprintln!("SKIP: the compositor advertised no Xwayland DISPLAY (Xwayland unavailable)");
        return;
    };
    // Bring Xwayland up so the seat bridge (which carries XDND) is armed.
    let (conn, _screen_num) = wait_for_xwayland_ready(&comp, &display);

    let globals = advertised_globals(&comp.socket);
    assert!(
        globals.iter().any(|g| g == "wl_data_device_manager"),
        "wl_data_device_manager (the XDND<->Wayland drag target) global is missing; saw {globals:?}"
    );

    // The bridge shares the seat the selection tests exercise; prove that seat's
    // data-device transfer path is live end-to-end here too — an X11 owner's
    // bytes actually crossing into a Wayland reader — so this test stands as a
    // real construction proof (review finding #12), not a tautological "the
    // unconditional manager global exists". A full read, not just `has_offer`:
    // the same `wl_data_device`/`wlr_seat` machinery an X11-originated *drag*
    // offer would traverse is shown to carry data across the boundary intact.
    let mut manager = DataControlClient::spawn(&comp.socket);
    let _owner =
        X11SelectionOwner::spawn(display.clone(), "CLIPBOARD", X11_CLIPBOARD_PAYLOAD.to_vec());
    assert!(
        manager.wait_until(|c| c.has_offer()),
        "the X11 selection never reached the Wayland seat as a data-control offer -- \
         XDND rides this same seat"
    );
    let got = manager.read_offer_blocking(SELECTION_MIME);
    assert_eq!(
        got, X11_CLIPBOARD_PAYLOAD,
        "the shared seat's data-device transfer path did not carry the X11 payload across -- \
         XDND rides this same path"
    );
    drop(conn);
}

/// M4 — DISPLAY + cursor environment robustness. The compositor publishes a
/// valid `DISPLAY` (`:N`) and, *when it is absent*, a default `XCURSOR_THEME`
/// into its own environment, so session children spawned after the lazy start
/// inherit the right X server and pointer theme -- concretely resolving the M1
/// DISPLAY-ordering caveat. The scale-aware cursor *size* is published as the
/// `Xcursor.size` X resource rather than `XCURSOR_SIZE` (review finding #4): the
/// env var would outrank the resource in Xcursor's lookup and pin a fixed logical
/// 24, losing HiDPI sizing, so `XCURSOR_SIZE` is left unset (and a session's own
/// value respected). The compositor runs in this test's own process, so the
/// exported vars are observable here directly.
///
/// Non-vacuous, unlike the earlier version that read whatever ambient
/// `XCURSOR_*` this runner already exports: the cursor vars are *cleared* before
/// the compositor boots, so a passing assertion can only come from the
/// compositor's own export. Both branches are covered -- publish a default theme
/// (and the scale-aware `Xcursor.size` resource) when absent, and *respect* a
/// value the session already chose. Skips visibly when Xwayland is absent.
#[test]
fn xwayland_publishes_display_and_cursor_env_on_ready() {
    let _x11_guard = x11_test_guard();
    if !xwayland_on_path() {
        eprintln!("SKIP: Xwayland is not installed; the DISPLAY/cursor env test cannot run");
        return;
    }

    // Save whatever this runner exported so the process env is left as we found
    // it (these tests share one process; be a good neighbour).
    let saved_theme = std::env::var_os("XCURSOR_THEME");
    let saved_size = std::env::var_os("XCURSOR_SIZE");

    // --- Branch 1: absent -> the compositor publishes the sane defaults. ---
    // SAFETY: the X11 e2e tests are serialized by `X11_TEST_LOCK`, so no other
    // test mutates the environment concurrently; the compositor thread only
    // *writes* these (never reads back what it wrote), so a concurrent clear
    // cannot mislead it. Same env discipline as `wait_for_xwayland_ready`.
    unsafe {
        std::env::remove_var("XCURSOR_THEME");
        std::env::remove_var("XCURSOR_SIZE");
    }
    let comp = Compositor::spawn();
    let Some(display) = comp.xwayland_display() else {
        eprintln!("SKIP: the compositor advertised no Xwayland DISPLAY (Xwayland unavailable)");
        // Restore before the early return.
        restore_cursor_env(saved_theme, saved_size);
        return;
    };
    assert!(is_valid_display_name(&display), "advertised DISPLAY {display:?} is not a valid :N name");
    // The env exports fire at boot; the `Xft.dpi`/`Xcursor.size` resource fires in
    // `xwayland_ready`, which is lazy — force it by connecting a client, then poll
    // the process environment for the defaults.
    let (conn, screen_num) = connect_with_retry(&display);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let display_env = std::env::var("DISPLAY").ok();
        let theme = std::env::var("XCURSOR_THEME").ok();
        let size = std::env::var("XCURSOR_SIZE").ok();
        // Default theme is "default". `XCURSOR_SIZE` is deliberately *not* forced
        // into the env (it would override the scale-aware `Xcursor.size` resource
        // and lose HiDPI sizing), so it must stay absent when the session left it
        // unset — the scale-aware size is asserted via the X resource below.
        if display_env.as_deref() == Some(display.as_str())
            && theme.as_deref() == Some("default")
            && size.is_none()
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "DISPLAY/cursor env defaults never published after ready (DISPLAY={display_env:?} \
             XCURSOR_THEME={theme:?} XCURSOR_SIZE={size:?}); expected theme=default, size unset"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    // The scale-aware cursor size rides the root `RESOURCE_MANAGER` (`Xcursor.size`),
    // not the env: at the harness's scale-1 output it is the logical `24 * 1`.
    let root = conn.setup().roots[screen_num].root;
    let resource_manager = intern(&conn, b"RESOURCE_MANAGER");
    let matched = poll_x_resource_manager(&conn, root, resource_manager, Duration::from_secs(10), |rm| {
        rm.contains("Xcursor.size:\t24")
    });
    assert!(
        matched,
        "the compositor never published Xcursor.size=24 in RESOURCE_MANAGER for the scale-1 output; \
         RESOURCE_MANAGER was {:?}",
        read_x_resource_manager(&conn, root, resource_manager)
    );
    drop(conn);
    drop(comp);

    // --- Branch 2: already set -> the compositor respects the session's choice. ---
    const SENTINEL_THEME: &str = "IcedteaSentinelCursors";
    const SENTINEL_SIZE: &str = "99";
    // SAFETY: as Branch 1.
    unsafe {
        std::env::set_var("XCURSOR_THEME", SENTINEL_THEME);
        std::env::set_var("XCURSOR_SIZE", SENTINEL_SIZE);
    }
    let comp = Compositor::spawn();
    if let Some(display) = comp.xwayland_display() {
        // The cursor env is published at boot (before `Compositor::spawn`
        // returned), so the sentinel-respecting decision has already been made by
        // the time we read it here; `wait_for_xwayland_ready` additionally drives
        // the seat wiring so the whole ready path ran, keeping this a real
        // assertion about the respect-existing branch.
        let (conn, _screen_num) = wait_for_xwayland_ready(&comp, &display);
        assert_eq!(
            std::env::var("XCURSOR_THEME").ok().as_deref(),
            Some(SENTINEL_THEME),
            "the compositor overwrote an already-set XCURSOR_THEME"
        );
        assert_eq!(
            std::env::var("XCURSOR_SIZE").ok().as_deref(),
            Some(SENTINEL_SIZE),
            "the compositor overwrote an already-set XCURSOR_SIZE"
        );
        drop(conn);
    }
    drop(comp);

    restore_cursor_env(saved_theme, saved_size);
}

/// Restore (or clear) the `XCURSOR_*` env to the values saved before the cursor
/// test mutated them, so the shared test process is left as it was found.
fn restore_cursor_env(theme: Option<std::ffi::OsString>, size: Option<std::ffi::OsString>) {
    // SAFETY: serialized by `X11_TEST_LOCK`; see the cursor test's own note.
    unsafe {
        match theme {
            Some(v) => std::env::set_var("XCURSOR_THEME", v),
            None => std::env::remove_var("XCURSOR_THEME"),
        }
        match size {
            Some(v) => std::env::set_var("XCURSOR_SIZE", v),
            None => std::env::remove_var("XCURSOR_SIZE"),
        }
    }
}

/// M4 — DISPLAY child-spawn inheritance (Goal #3, resolving the M1 caveat
/// concretely). After Xwayland is `ready`, a child process spawned from the
/// compositor's environment inherits the right `DISPLAY=:N`. This is the
/// end-to-end proof of "children spawned after xwayland ready see the right :N":
/// the compositor exported `DISPLAY` into its own process env in the `ready`
/// handler, and this test spawns a trivial child *after* ready and reads back the
/// `DISPLAY` it actually observed. Skips visibly when Xwayland is absent.
#[test]
fn a_child_spawned_after_ready_inherits_the_display() {
    let _x11_guard = x11_test_guard();
    if !xwayland_on_path() {
        eprintln!("SKIP: Xwayland is not installed; the DISPLAY-inheritance test cannot run");
        return;
    }
    let comp = Compositor::spawn();
    let Some(display) = comp.xwayland_display() else {
        eprintln!("SKIP: the compositor advertised no Xwayland DISPLAY (Xwayland unavailable)");
        return;
    };
    // Gate on `ready` having actually republished DISPLAY into the process env.
    let (conn, _screen_num) = wait_for_xwayland_ready(&comp, &display);

    // A child spawned now inherits the compositor's environment; it should see
    // exactly the DISPLAY the compositor advertised.
    let out = std::process::Command::new("sh")
        .arg("-c")
        .arg("printf %s \"$DISPLAY\"")
        .output()
        .expect("spawn a child to read DISPLAY");
    let child_display = String::from_utf8_lossy(&out.stdout).into_owned();
    assert_eq!(
        child_display, display,
        "a child spawned after ready saw DISPLAY={child_display:?}, expected {display:?}"
    );
    drop(conn);
}

/// M4 — HiDPI DPI hint on a scaled output (Goal #2). With the primary output at
/// integer scale 2, the compositor publishes `Xft.dpi = 96 * 2 = 192` into the X
/// root window's `RESOURCE_MANAGER` property, so X11 toolkits size fonts/UI for
/// the HiDPI output. Asserted off the *real* root property an X11 client reads,
/// exactly as GTK/Qt would (via xrdb), which is the concrete, drivable half of
/// the design's HiDPI decision.
///
/// Non-vacuous: the scale is set to 2 before Xwayland comes up, the payload
/// carries the computed 192 (not a fixed 96), and the property is read straight
/// off the X server. Scene-node *buffer* upscaling of DPI-unaware X11 clients is
/// a documented wlroots-scene limitation (no `wlr_scene_tree` scale; only
/// `wlr_scene_buffer` dest-size) and is not asserted here -- the DPI hint is the
/// mechanism DPI-aware X11 toolkits actually use under the single global integer
/// scale. Skips visibly when Xwayland is absent.
#[test]
fn hidpi_dpi_hint_is_published_for_a_scaled_output() {
    let _x11_guard = x11_test_guard();
    if !xwayland_on_path() {
        eprintln!("SKIP: Xwayland is not installed; the HiDPI DPI-hint test cannot run");
        return;
    }
    let comp = Compositor::spawn();
    let Some(display) = comp.xwayland_display() else {
        eprintln!("SKIP: the compositor advertised no Xwayland DISPLAY (Xwayland unavailable)");
        return;
    };

    // Raise the primary output to integer scale 2 *before* the first X client
    // connection triggers the lazy start, so `xwayland_ready` reads scale 2 and
    // publishes Xft.dpi=192 on the spot. (Blocks until recorded.)
    comp.set_output_scale_for_test(2.0);

    // Connecting execs Xwayland and drives `ready` -> the DPI export.
    let (conn, screen_num) = wait_for_xwayland_ready(&comp, &display);
    let root = conn.setup().roots[screen_num].root;

    let resource_manager = intern(&conn, b"RESOURCE_MANAGER");
    let matched = poll_x_resource_manager(&conn, root, resource_manager, Duration::from_secs(15), |rm| {
        rm.contains("Xft.dpi:\t192")
    });
    assert!(
        matched,
        "the compositor never published Xft.dpi=192 in RESOURCE_MANAGER for a scale-2 output; \
         last saw {:?}",
        read_x_resource_manager(&conn, root, resource_manager)
    );
    drop(conn);
}

/// Read the X root window's `RESOURCE_MANAGER` (the xrdb database) as a string,
/// empty on any miss (the property is absent until the compositor first writes
/// it).
fn read_x_resource_manager(conn: &impl Connection, root: u32, resource_manager: Atom) -> String {
    conn.get_property(false, root, resource_manager, AtomEnum::ANY, 0, 4096)
        .ok()
        .and_then(|cookie| cookie.reply().ok())
        .map(|reply| String::from_utf8_lossy(&reply.value).into_owned())
        .unwrap_or_default()
}

/// Poll the X root window's `RESOURCE_MANAGER` until `pred` holds over its string
/// value or the timeout elapses. The compositor's DPI export runs on a detached
/// thread, so this polls rather than reading once.
fn poll_x_resource_manager(
    conn: &impl Connection,
    root: u32,
    resource_manager: Atom,
    timeout: Duration,
    pred: impl Fn(&str) -> bool,
) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if pred(&read_x_resource_manager(conn, root, resource_manager)) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

/// A minimal X11 selection owner running its own event loop on a dedicated
/// connection: it maps a managed top-level (which the compositor auto-focuses --
/// wlroots refuses to bridge a CLIPBOARD/PRIMARY selection to Wayland unless a
/// focused Xwayland surface owns it, exactly as a real X11 app has a window),
/// takes ownership of a named selection (`CLIPBOARD`/`PRIMARY`), and answers
/// `TARGETS` and `UTF8_STRING`/`STRING`/`TEXT` conversion requests with a fixed
/// UTF-8 payload, exactly as a real X11 app that "copied" would. The loop runs
/// continuously because the `wlr_xwm` requests `TARGETS` (to build the Wayland
/// offer) and later the data (when a Wayland reader pulls) at times the test's
/// main thread cannot predict, and it re-asserts ownership for a short while so
/// the bridge is (re-)armed the moment the mapped window actually gains focus.
struct X11SelectionOwner {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl X11SelectionOwner {
    /// Take ownership of `selection` on `display` and serve `payload` until
    /// dropped. Blocks (briefly) until ownership is confirmed, so a caller can
    /// immediately expect the bridge to observe it.
    fn spawn(display: String, selection: &'static str, payload: Vec<u8>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = stop.clone();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<()>();
        let handle = std::thread::spawn(move || {
            let (conn, screen_num) = connect_with_retry(&display);
            let screen = conn.setup().roots[screen_num].clone();
            // A managed (non-OR) top-level the compositor will auto-focus -- the
            // focused Xwayland surface wlroots requires before it will bridge
            // this client's selection.
            let win = map_managed_x11(&conn, &screen, 200, 150);

            let sel = intern(&conn, selection.as_bytes());
            let targets = intern(&conn, b"TARGETS");
            let utf8 = intern(&conn, b"UTF8_STRING");
            let text = intern(&conn, b"TEXT");
            let string_atom: Atom = AtomEnum::STRING.into();

            conn.set_selection_owner(win, sel, CURRENT_TIME).expect("set_selection_owner");
            conn.flush().expect("flush set_selection_owner");
            // Confirm we actually hold it before signalling ready.
            let owner = conn
                .get_selection_owner(sel)
                .expect("get_selection_owner request")
                .reply()
                .expect("get_selection_owner reply")
                .owner;
            assert_eq!(owner, win, "failed to take ownership of {selection}");
            let _ = ready_tx.send(());

            // Re-assert ownership a handful of times over the first few seconds:
            // the initial grab can land before the just-mapped window has gained
            // focus (wlroots then denies the bridge "no xwayland surface
            // focused"), and each fresh SetSelectionOwner re-fires XFIXES so the
            // xwm re-evaluates once focus is established.
            let mut reasserts_left = 12u32;
            let mut next_reassert = Instant::now() + Duration::from_millis(250);
            while !stop_thread.load(Ordering::Relaxed) {
                if reasserts_left > 0 && Instant::now() >= next_reassert {
                    conn.set_selection_owner(win, sel, CURRENT_TIME).expect("re-set_selection_owner");
                    conn.flush().expect("flush re-set_selection_owner");
                    reasserts_left -= 1;
                    next_reassert = Instant::now() + Duration::from_millis(250);
                }
                while let Ok(Some(event)) = conn.poll_for_event() {
                    if let Event::SelectionRequest(req) = event {
                        let mut property = req.property;
                        if req.target == targets {
                            let list = [utf8, string_atom, text];
                            conn.change_property32(
                                PropMode::REPLACE,
                                req.requestor,
                                req.property,
                                AtomEnum::ATOM,
                                &list,
                            )
                            .expect("write TARGETS");
                        } else if req.target == utf8
                            || req.target == string_atom
                            || req.target == text
                        {
                            conn.change_property8(
                                PropMode::REPLACE,
                                req.requestor,
                                req.property,
                                req.target,
                                &payload,
                            )
                            .expect("write selection payload");
                        } else {
                            // Unsupported target: refuse per ICCCM (property None).
                            property = NONE;
                        }
                        let notify = SelectionNotifyEvent {
                            response_type: SELECTION_NOTIFY_EVENT,
                            sequence: 0,
                            time: req.time,
                            requestor: req.requestor,
                            selection: req.selection,
                            target: req.target,
                            property,
                        };
                        conn.send_event(false, req.requestor, EventMask::NO_EVENT, notify)
                            .expect("send SelectionNotify");
                        conn.flush().expect("flush SelectionNotify");
                    }
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        });

        // Wait for ownership to be confirmed (or fail fast if the thread died).
        ready_rx
            .recv_timeout(Duration::from_secs(15))
            .expect("the X11 selection owner never took ownership");
        X11SelectionOwner { stop, handle: Some(handle) }
    }
}

impl Drop for X11SelectionOwner {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Convert `selection` (`CLIPBOARD`/`PRIMARY`) to `UTF8_STRING` on a fresh
/// requestor window and read the resulting property, returning the bytes once
/// the owner (here the `wlr_xwm`, proxying a Wayland source) answers. `pump` is
/// called each poll iteration so a same-thread Wayland selection owner services
/// the `send` the xwm forwards. `None` if no answer arrives before `timeout`.
fn x11_read_selection(
    conn: &impl Connection,
    screen: &x11rb::protocol::xproto::Screen,
    selection: &str,
    timeout: Duration,
    mut pump: impl FnMut(),
) -> Option<Vec<u8>> {
    let win = conn.generate_id().expect("requestor window id");
    conn.create_window(
        COPY_DEPTH_FROM_PARENT,
        win,
        screen.root,
        -100,
        -100,
        1,
        1,
        0,
        WindowClass::INPUT_OUTPUT,
        screen.root_visual,
        &CreateWindowAux::new()
            .override_redirect(1)
            .event_mask(EventMask::PROPERTY_CHANGE),
    )
    .expect("create requestor window");

    let sel = intern(conn, selection.as_bytes());
    let target = intern(conn, b"UTF8_STRING");
    let property = intern(conn, b"ICEDTEA_SELECTION_IN");

    let deadline = Instant::now() + timeout;
    // Re-issue the conversion periodically: the xwm may not yet own the X
    // selection the instant the Wayland side set it, so an early convert can
    // come back refused (property == None).
    let mut next_convert = Instant::now();
    loop {
        if Instant::now() >= deadline {
            return None;
        }
        if Instant::now() >= next_convert {
            let _ = conn.delete_property(win, property);
            conn.convert_selection(win, sel, target, property, CURRENT_TIME)
                .expect("convert_selection");
            conn.flush().expect("flush convert_selection");
            next_convert = Instant::now() + Duration::from_millis(500);
        }
        pump();
        while let Ok(Some(event)) = conn.poll_for_event() {
            if let Event::SelectionNotify(n) = event {
                if n.property == NONE {
                    // Refused; fall through to the next re-convert.
                    break;
                }
                let reply = conn
                    .get_property(true, win, property, AtomEnum::ANY, 0, u32::MAX)
                    .expect("get_property request")
                    .reply()
                    .expect("get_property reply");
                if !reply.value.is_empty() {
                    return Some(reply.value);
                }
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Map a managed X11 top-level and wait until the compositor has given it
/// keyboard focus, returning its X id. wlroots gates *both* directions of the
/// selection bridge on "a focused Xwayland surface", so a Wayland→X11 read needs
/// some focused X window in the session even though the requestor is a separate
/// unmapped window.
fn map_focused_managed_x11(
    comp: &Compositor,
    conn: &impl Connection,
    screen: &x11rb::protocol::xproto::Screen,
) -> u32 {
    let win = map_managed_x11(conn, screen, 200, 150);
    let window = poll_for_window(comp, MAP_TIMEOUT)
        .expect("the focus-holding managed X11 window never entered the model");
    assert!(
        poll_snapshot_window(comp, window.id, Duration::from_secs(10), |w| w.focused).is_some(),
        "the focus-holding managed X11 window never became focused"
    );
    win
}

/// Whether `name` is a well-formed X11 `DISPLAY` (`:N` or `:N.S`, host part
/// empty for the local Xwayland socket).
fn is_valid_display_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix(':') else { return false };
    let digits = rest.split('.').next().unwrap_or("");
    !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
}
