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

use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Serializes the real-Xwayland end-to-end tests. Each one boots a full
/// headless compositor *and* a lazily-spawned `Xwayland` process (plus, in some
/// cases, extra Wayland clients); running eight of those concurrently under the
/// default multi-threaded test runner starves the slower boots and a window can
/// miss its poll deadline. These tests are inherently heavyweight and
/// integration-shaped, so serialize them here rather than relying on the caller
/// passing `--test-threads=1`. Poisoning is ignored: a panicking test must not
/// cascade into spurious failures of the rest.
static X11_TEST_LOCK: Mutex<()> = Mutex::new(());

fn x11_test_guard() -> MutexGuard<'static, ()> {
    X11_TEST_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

use icedtea_compositor::dbus::DbCommand;
use icedtea_contract::WindowId;
use icedtea_harness::{Compositor, VirtualPointerClient};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ChangeWindowAttributesAux, ClientMessageEvent, ConfigureWindowAux,
    ConnectionExt as _, CreateWindowAux, EventMask, PropMode, WindowClass,
};
use x11rb::wrapper::ConnectionExt as _;
use x11rb::COPY_DEPTH_FROM_PARENT;

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
    std::env::var_os("PATH")
        .map(|path| {
            std::env::split_paths(&path).any(|dir| dir.join("Xwayland").is_file())
        })
        .unwrap_or(false)
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
    let window = poll_for_window(&comp, Duration::from_secs(15))
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

    // The requested size is treated by the WM as the *frame*; a decorated
    // window's client is then configured to the content rect inside the SSD.
    const FRAME_W: u16 = 400;
    const FRAME_H: u16 = 300;

    let win = conn.generate_id().expect("generate X11 window id");
    conn.create_window(
        COPY_DEPTH_FROM_PARENT,
        win,
        screen.root,
        0,
        0,
        FRAME_W,
        FRAME_H,
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
    let window = poll_for_window(&comp, Duration::from_secs(15))
        .expect("the managed X11 window never entered the compositor's window model");
    assert_eq!(window.app_id, WINDOW_CLASS, "app_id is the WM_CLASS class");
    assert_eq!(window.title, WINDOW_TITLE, "title is the X11 window name");
    let id = window.id;

    // (2) SSD parity — the client is configured to a content rect exactly one
    // title-bar shorter than the frame. This is the end-to-end proof the X11
    // window is decorated: the model reserved the strip and the configure
    // reached the X server.
    let content_h = poll_x_geometry(&conn, win, Duration::from_secs(10), |g| {
        g.width == FRAME_W && g.height == FRAME_H - TITLE_BAR_HEIGHT
    });
    assert!(
        content_h,
        "the X11 client was never configured to the SSD content rect (expected {}x{})",
        FRAME_W,
        FRAME_H - TITLE_BAR_HEIGHT
    );
    assert_eq!(window.geometry.width, FRAME_W as i32, "model geometry is the frame width");
    assert_eq!(window.geometry.height, FRAME_H as i32, "model geometry is the frame height");

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
    // It grew far past its 400px frame to fill the output.
    assert!(
        maxed.geometry.width > FRAME_W as i32 && maxed.geometry.height > FRAME_H as i32,
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

    const FRAME_W: u16 = 400;
    const FRAME_H: u16 = 300;
    let win = map_managed_x11(&conn, &screen, FRAME_W, FRAME_H);
    let window = poll_for_window(&comp, Duration::from_secs(15))
        .expect("the managed X11 window never entered the model");
    let id = window.id;
    // Baseline: decorated, so the client sits in the SSD content rect.
    assert!(
        poll_x_geometry(&conn, win, Duration::from_secs(10), |g| g.height == FRAME_H - TITLE_BAR_HEIGHT),
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
        fs.geometry.width > FRAME_W as i32 && fs.geometry.height > FRAME_H as i32,
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
    assert_eq!(restored.geometry.width, FRAME_W as i32, "frame width not restored");
    assert_eq!(restored.geometry.height, FRAME_H as i32, "frame height not restored");
    assert!(
        poll_x_geometry(&conn, win, Duration::from_secs(10), |g| {
            g.width == FRAME_W && g.height == FRAME_H - TITLE_BAR_HEIGHT
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
    let window = poll_for_window(&comp, Duration::from_secs(15))
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
    let window = poll_for_window(&comp, Duration::from_secs(15))
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
    let a = poll_for_window(&comp, Duration::from_secs(15))
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

    let window = poll_for_window(&comp, Duration::from_secs(15))
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
    let window = poll_for_window(&comp, Duration::from_secs(15))
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
    let base = poll_for_window(&comp, Duration::from_secs(15))
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
    let parent_win = poll_for_window(&comp, Duration::from_secs(15))
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
    let modelled = poll_for_window(&comp, Duration::from_secs(15))
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
    assert!(
        poll_or(&comp, Duration::from_secs(15), |ps| ps.len() == 1).is_some(),
        "the window did not enter the OR side-table when it became override-redirect"
    );

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
