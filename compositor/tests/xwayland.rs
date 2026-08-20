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

use icedtea_harness::Compositor;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    AtomEnum, ConnectionExt as _, CreateWindowAux, PropMode, WindowClass,
};
use x11rb::wrapper::ConnectionExt as _;
use x11rb::COPY_DEPTH_FROM_PARENT;

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
