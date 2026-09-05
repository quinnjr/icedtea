//! The settings window under the harness compositor.
//!
//! This is `App::run`'s first `Role::Toplevel` surface (M5 spec §10's named
//! risk): every earlier `App::run` drove the gallery's layer surface.

mod support;

use std::time::{Duration, Instant};

// Reconciliation (Task 7): the placeholder Appearance page P1 shipped was a
// single label; the real page (drop-down, two spin buttons, three colour
// swatches, a wallpaper entry and its buttons/status row) takes noticeably
// longer to reach its first composited frame under the harness's headless
// backend — 20s left the window-mapped wait and the paint-settle wait each
// racing a real first paint that was landing at ~22-23s. Widened, not
// removed: the assertions are unchanged, only the budget they get.
const SETTLE: Duration = Duration::from_secs(45);

/// The server-side title bar `compositor/src/decoration.rs` reserves above a
/// window's content area — the same constant `ui/tests/window_events.rs`
/// uses to translate a window's frame geometry into a content-area origin.
const TITLE_BAR_HEIGHT: i32 = 28;

/// Poll `comp`'s model until a window with `app_id` is mapped, returning its
/// frame geometry. The model row is only created on the wlr `mapped` signal,
/// which under this headless backend can trail the client's own layout (and
/// so its first probe report) by a visible amount.
fn wait_for_window(
    comp: &icedtea_harness::Compositor,
    app_id: &str,
    timeout: Duration,
) -> icedtea_contract::Rectangle {
    let deadline = Instant::now() + timeout;
    loop {
        let snap = comp.snapshot();
        if let Some(window) = snap.windows.iter().find(|w| w.app_id == app_id) {
            return window.geometry;
        }
        assert!(
            Instant::now() < deadline,
            "no window with app_id {app_id} ever mapped"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The window opens, paints, and both the switcher and the footer respond.
///
/// Mutation check: remove `.on_selected(Msg::PageSelected)` from `nav`; the
/// `page displays` wait fails. Restore.
#[test]
fn the_first_toplevel_app_run_paints_and_navigates() {
    let comp = icedtea_harness::Compositor::spawn();
    let mut shot = icedtea_harness::ScreencopyClient::spawn(&comp.socket);
    let background = {
        let frame = shot.capture();
        support::pixel_at(&frame, 1, 1).expect("the harness background is readable")
    };
    // Reconciled from the plan's literal ordering: `ui/tests/window_events.rs`
    // documents (on its own `VirtualPointerClient::spawn` call) that a
    // headless seat has no pointer capability until an input device shows
    // up, and a client that connects before one exists never calls
    // `wl_seat.get_pointer` for it — a click delivered afterwards is looked
    // up against that client's empty pointer list and silently dropped. The
    // pointer has to exist before `spawn_settings` connects.
    let mut pointer = icedtea_harness::VirtualPointerClient::spawn(&comp.socket);
    let app = support::spawn_settings_process(&comp);

    // 1. It painted: the first geometry block names the ids `view` builds.
    assert!(
        app.wait_line("probe root ", SETTLE).is_some(),
        "no probe block: the window never laid out\n{:?}",
        app.lines()
    );
    for id in [
        "nav",
        "pages",
        "footer",
        "status",
        "revert",
        "apply",
        "appearance",
    ] {
        assert!(
            app.point(id).is_some(),
            "#{id} is missing from the report\n{:?}",
            app.lines()
        );
    }

    // The model row (and so the window's screen position) only exists once
    // the wlr `mapped` signal has fired, which can lag the client's own
    // probe report; find out where the compositor actually put it before
    // asserting anything about what the screencopy shows or clicking it.
    // Probe points are window-local (surface) coordinates; a click (and a
    // screencopy pixel test) need the compositor's own coordinate space —
    // the window's frame origin, plus the server-side title bar strip this
    // app never opted out of (it never requests client-side decorations).
    let to_screen = |(x, y): (i32, i32)| {
        let geometry = wait_for_window(&comp, "org.icedtea.Settings", SETTLE);
        (geometry.x + x, geometry.y + TITLE_BAR_HEIGHT + y)
    };

    // Capture on a short poll: the toplevel can be mapped a frame or two
    // before the harness compositor actually composites its buffer into the
    // output the screencopy client reads.
    let paint_deadline = Instant::now() + SETTLE;
    let mut painted = false;
    while Instant::now() < paint_deadline {
        let frame = shot.capture();
        if support::paints_something_anywhere(&frame, background) {
            painted = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        painted,
        "the settings window painted nothing over the harness background"
    );

    // 2. The switcher navigates: click the fifth switcher button.
    let nav = app.point("nav").expect("#nav is laid out");
    let (_, _, nav_w, _) = app.allocation("nav").expect("#nav has an allocation");
    // Five equal-width linked buttons; the fifth's centre is at 90% of the width.
    let displays_local = (nav.0 - (nav_w as i32) / 2 + (nav_w as i32 * 9) / 10, nav.1);
    let (displays_x, displays_y) = to_screen(displays_local);
    support::click_fixed(&mut pointer, displays_x, displays_y);
    assert!(
        app.wait_line("page displays", SETTLE).is_some(),
        "the switcher did not select the Displays page\n{:?}",
        app.lines()
    );
    // Waited for, not read once: `page displays` is written by the *fold*,
    // and the page's own probe points only exist after the render that
    // follows it. (Before the fix wave this assertion passed the moment the
    // app started, because the report published every stack page's
    // widgets — the hidden ones included — from the first frame. Now that
    // it publishes only what is displayed, this really does assert that the
    // Displays page came on screen.)
    //
    // Reconciliation (P4 Task 8): P1's stub page rooted itself at
    // `displays_page`; the real page P4 assembles roots at `displays` (both
    // the unavailable-explanation branch and the canvas/controls/footer
    // branch), so this probe id follows the rename.
    assert!(
        app.wait_line("probe displays ", SETTLE).is_some(),
        "the Displays page body never entered the tree\n{:?}",
        app.lines()
    );

    // 3. The footer responds: Apply is insensitive while clean, and clicking
    //    it changes nothing — the model is not dirty.
    let apply_local = app.point("apply").expect("#apply is laid out");
    let (ax, ay) = to_screen(apply_local);
    support::click_fixed(&mut pointer, ax, ay);
    assert!(
        app.wait_line("status Applying", Duration::from_secs(2))
            .is_none(),
        "an insensitive Apply must not start a save\n{:?}",
        app.lines()
    );
}
