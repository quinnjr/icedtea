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

use std::time::{Duration, Instant};

use icedtea_compositor::dbus::DbCommand;
use icedtea_contract::WindowId;
use icedtea_harness::Compositor;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    AtomEnum, ConfigureWindowAux, ConnectionExt as _, CreateWindowAux, PropMode, WindowClass,
};
use x11rb::wrapper::ConnectionExt as _;
use x11rb::COPY_DEPTH_FROM_PARENT;

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
