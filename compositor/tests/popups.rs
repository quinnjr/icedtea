//! xdg-popup, end to end: a real `wayland-client` opening real popups on a
//! real headless compositor over a real socket.
//!
//! Everything asserted here is asserted from the **client** side. The D-Bus
//! `Snapshot` is toplevel-only by design (contract §9, "must not touch:
//! `Snapshot`/`WindowInfo`"), so a popup's placement is read back the way a
//! real client reads it: from `xdg_popup.configure`. Model-side state
//! (`popup_constraint_box`, `popup_at_point`, `popup_chain`) is unit-tested in
//! `compositor/src/state.rs`'s own `mod tests` instead.
//!
//! Contract: `docs/superpowers/plans/2026-08-27-m3-part0-contract.md` §2.4
//! fixes these twelve names and this order.
#![allow(dead_code)]

use std::time::Duration;

use icedtea_contract::Rectangle;
use icedtea_harness::{
    Compositor, PopupAnchor, PopupConstraint, PopupGravity, PopupSpec, SessionLockClient,
    TestClient, VirtualPointerClient,
};

/// `BTN_LEFT`, the only button the compositor's decoration path acts on.
const BTN_LEFT: u32 = 0x110;

/// The parent's **content** rect -- the origin of the `wl_surface` a popup's
/// coordinates are expressed against, which for a server-decorated window is
/// one title bar below the frame's top edge.
///
/// Read from the model's own snapshot rather than assumed, because the
/// compositor chooses the placement: a test that hardcoded a position would
/// silently stop testing constraint adjustment the day the layout changed.
fn content_rect_of(comp: &Compositor, app_id: &str) -> Rectangle {
    let geometry = comp
        .snapshot()
        .windows
        .iter()
        .find(|w| w.app_id == app_id)
        .unwrap_or_else(|| panic!("{app_id} is not in the model"))
        .geometry;
    let ssd = icedtea_compositor::decoration::has_ssd(app_id, None, false);
    icedtea_compositor::decoration::content_rect(geometry, ssd)
}

/// The `wlr` crate really does expose the xdg-popup API this part is written
/// against, and `wlr_xdg_positioner_rules` really does compute the geometry
/// every placement test below predicts.
///
/// This is the part's dependency check *and* its arithmetic oracle: the
/// anchor/gravity pair used throughout `popups.rs`
/// (`BottomLeft` + `BottomRight` on a `(10, 10, 20, 20)` anchor rect) is
/// pinned here against wlroots' own C implementation, so a placement test
/// that fails later is a compositor bug rather than a mis-derived expectation.
///
/// Mutation check: change the expected geometry's `y` from `30` to `10` and
/// this test fails — the call really reaches wlroots rather than returning a
/// default.
#[test]
fn the_wlr_crate_exposes_the_xdg_popup_api_part_2_is_written_against() {
    // Deviation D1: `PopupKey::for_test` cannot exist without these.
    let a = wlr::PopupId::dangling_nth_for_test(1);
    let b = wlr::PopupId::dangling_nth_for_test(2);
    assert_eq!(a, wlr::PopupId::dangling_nth_for_test(1));
    assert_ne!(a, b, "distinct n must produce distinct dangling popup ids");

    assert!(wlr::PopupParent::Popup(a).is_popup());
    assert!(!wlr::PopupParent::Toplevel(wlr::ToplevelId::dangling_nth_for_test(1)).is_popup());

    let rules = wlr::PositionerRules {
        anchor_rect: wlr::Box2D::new(10, 10, 20, 20),
        anchor: wlr::PositionerAnchor::BottomLeft,
        gravity: wlr::PositionerGravity::BottomRight,
        constraint_adjustment: wlr::ConstraintAdjustment::NONE,
        size: (64, 48),
        parent_size: None,
        offset: (0, 0),
        reactive: false,
        parent_configure_serial: None,
    };
    // Anchor BottomLeft of (10, 10, 20, 20) is (10, 30); gravity BottomRight
    // puts the surface's top-left corner on the anchor point.
    assert_eq!(
        rules.geometry(),
        wlr::Box2D::new(10, 30, 64, 48),
        "the anchor/gravity arithmetic every placement test in this file \
         predicts"
    );
    // Nothing to adjust: a constraint that already contains the geometry
    // leaves it exactly where the rules put it.
    assert_eq!(
        rules.unconstrain_box(&wlr::Box2D::new(-100, -100, 400, 400)),
        wlr::Box2D::new(10, 30, 64, 48)
    );
}

/// A popup under a toplevel is configured at exactly the geometry its
/// positioner asks for, when nothing constrains it.
///
/// `(10, 10, 20, 20)` anchored `BottomLeft` with gravity `BottomRight` puts
/// the popup's top-left corner at `(10, 30)` in the parent's window-geometry
/// coordinates -- the arithmetic
/// `the_wlr_crate_exposes_the_xdg_popup_api_part_2_is_written_against` pins
/// against wlroots itself. The window is nowhere near an output edge, so no
/// constraint adjustment can apply and the configure must be the raw geometry.
///
/// Mutation check: make `State::configure_popup_now` return early and the
/// popup is never configured, so `open_popup` times out waiting to map.
#[test]
fn a_popup_under_a_toplevel_is_configured_at_the_positioner_geometry() {
    let comp = Compositor::spawn();
    let mut a = TestClient::map_toplevel(&comp.socket, "popup.app", "popup");
    assert!(
        a.wait_until(|c| c.last_configure().is_some()),
        "the parent never configured"
    );

    a.open_popup(
        PopupSpec::new(64, 48)
            .anchor_rect(10, 10, 20, 20)
            .anchor(PopupAnchor::BottomLeft)
            .gravity(PopupGravity::BottomRight),
    );

    assert_eq!(
        a.popup_configured(),
        Some((10, 30, 64, 48)),
        "the popup must be configured at the unconstrained positioner geometry"
    );
    assert_eq!(a.popup_depth(), 1);
    assert!(!a.popup_done(), "nothing dismissed this popup");

    a.destroy_popup();
    assert_eq!(a.popup_depth(), 0);
    a.detach();
}

/// A popup that would hang off the bottom of the output is flipped to the
/// other side of its anchor when the client allowed `FLIP_Y`.
///
/// The anchor rect sits 100px above the bottom of the usable area and the
/// popup is 200 tall, so the unadjusted placement overflows by 101. Flipping
/// swaps anchor `Bottom*` for `Top*` and gravity `Bottom*` for `Top*`, which
/// puts the popup's *bottom* on the anchor point: `y = anchor_y - height`.
///
/// Mutation check: pass `PopupConstraint::empty()` instead and the popup is
/// configured at `ay + 1` -- proving the assertion below really measures the
/// adjustment rather than the raw geometry.
#[test]
fn a_popup_that_would_leave_the_output_is_flipped_by_the_constraint_adjustment() {
    let comp = Compositor::spawn();
    let mut a = TestClient::map_toplevel(&comp.socket, "popup.app", "popup");
    assert!(a.wait_until(|c| c.last_configure().is_some()));
    let content = content_rect_of(&comp, "popup.app");
    let (_ow, oh) = comp.output_size();

    const H: i32 = 200;
    // 100px of room left below the anchor point: the popup overflows by 101.
    let ay = oh - content.y - 100;
    assert!(
        ay > H,
        "this output ({oh}px tall, content at y = {}) is too short to \
         distinguish a flip from a clamp",
        content.y
    );

    a.open_popup(
        PopupSpec::new(64, H)
            .anchor_rect(0, ay, 1, 1)
            .anchor(PopupAnchor::BottomLeft)
            .gravity(PopupGravity::BottomRight)
            .constraint(PopupConstraint::FlipY),
    );

    assert_eq!(
        a.popup_configured(),
        Some((0, ay - H, 64, H)),
        "a flipped popup puts its bottom edge on the anchor point"
    );
    a.detach();
}

/// A popup that cannot flip is slid back inside the usable area instead.
///
/// Same geometry as the flip test, with only `SLIDE_Y` allowed: the popup
/// keeps its size and its side of the anchor, and moves up just far enough to
/// fit. The assertions are the property `SLIDE_Y` promises -- "no longer
/// constrained, same size" -- rather than a hardcoded offset, so the test
/// measures the behaviour rather than one implementation's arithmetic.
///
/// Mutation check: pass `PopupConstraint::empty()` and the "fits" assertion
/// fails by exactly the 101px overflow.
#[test]
fn a_popup_that_cannot_flip_is_slid_back_inside_the_usable_area() {
    let comp = Compositor::spawn();
    let mut a = TestClient::map_toplevel(&comp.socket, "popup.app", "popup");
    assert!(a.wait_until(|c| c.last_configure().is_some()));
    let content = content_rect_of(&comp, "popup.app");
    let (_ow, oh) = comp.output_size();

    const H: i32 = 200;
    let ay = oh - content.y - 100;
    assert!(ay > H, "output too short for this test");

    a.open_popup(
        PopupSpec::new(64, H)
            .anchor_rect(0, ay, 1, 1)
            .anchor(PopupAnchor::BottomLeft)
            .gravity(PopupGravity::BottomRight)
            .constraint(PopupConstraint::SlideY),
    );

    let (px, py, pw, ph) = a.popup_configured().expect("the popup never configured");
    assert_eq!((pw, ph), (64, H), "a slid popup keeps its size");
    assert_eq!(px, 0, "nothing constrains x");
    assert!(
        py < ay + 1,
        "the popup must have moved up from the unadjusted {} to fit",
        ay + 1
    );
    // The constraint box's bottom edge, in the parent surface's coordinates.
    assert!(
        py + ph <= oh - content.y,
        "the slid popup must end up inside the usable area: y = {py}, \
         h = {ph}, bottom = {}",
        oh - content.y
    );
    a.detach();
}

/// A popup opened by a layer-shell panel is clamped to the same working area
/// windows get: the panel's own output `usable` rect, which the panel's
/// exclusive zone has already carved.
///
/// The panel is top-anchored with a 28px exclusive zone at the output origin,
/// so its surface origin is `(0, 0)` and the constraint box in its own
/// coordinates starts at `y = 28`. A popup anchored at `(0, 0, 1, 1)` would
/// land at `y = 1`, inside the panel's own strip; with `SLIDE_Y` it must be
/// pushed down to the usable area and stay entirely on the output.
///
/// Mutation check: make `State::popup_constraint_box` read
/// `outputs[..].geometry` instead of `.usable` and the `>= 28` assertion
/// fails -- the popup is allowed to sit under the panel.
#[test]
fn a_popup_under_a_layer_panel_is_constrained_to_the_same_output() {
    let comp = Compositor::spawn();
    let (_ow, oh) = comp.output_size();
    let mut panel = TestClient::map_layer_panel(&comp.socket, 28);
    assert!(
        panel.wait_until(|c| c.layer_configure().is_some()),
        "the panel never configured"
    );

    panel.open_popup(
        PopupSpec::new(64, 48)
            .anchor_rect(0, 0, 1, 1)
            .anchor(PopupAnchor::BottomLeft)
            .gravity(PopupGravity::BottomRight)
            .constraint(PopupConstraint::SlideY),
    );

    let (px, py, pw, ph) = panel
        .popup_configured()
        .expect("the panel's popup was never configured");
    assert_eq!((pw, ph), (64, 48), "the popup was slid, not resized");
    assert!(
        py >= 28,
        "the popup must be pushed below the panel's own exclusive zone, got \
         y = {py} (panel-surface coordinates, usable starts at 28)"
    );
    assert!(
        py + ph <= oh,
        "the popup must stay on the output: y = {py}, h = {ph}, output \
         height = {oh}"
    );
    assert_eq!(px, 0, "nothing constrains x, so it stays at the anchor");
}

/// Every level of a nested chain is configured against **its own parent**, not
/// against the chain's root.
///
/// A submenu's positioner is written in its parent popup's coordinates; if the
/// compositor answered in root coordinates instead, the second level here
/// would come back offset by the first level's `(10, 30)`.
///
/// Mutation check: resolve `PopupHost::Popup` to the chain root in
/// `new_popup` (drop the parent-scoped `new_popup` listener's answer and
/// re-parent everything to the root) and the inner assertion reports
/// `(15, 45, 32, 24)`.
#[test]
fn a_nested_popup_chain_configures_every_level_relative_to_its_parent() {
    let comp = Compositor::spawn();
    let mut a = TestClient::map_toplevel(&comp.socket, "popup.app", "popup");
    assert!(a.wait_until(|c| c.last_configure().is_some()));

    a.open_popup(
        PopupSpec::new(64, 48)
            .anchor_rect(10, 10, 20, 20)
            .anchor(PopupAnchor::BottomLeft)
            .gravity(PopupGravity::BottomRight),
    );
    a.open_popup_from_popup(
        PopupSpec::new(32, 24)
            .anchor_rect(5, 5, 10, 10)
            .anchor(PopupAnchor::BottomLeft)
            .gravity(PopupGravity::BottomRight),
    );

    assert_eq!(a.popup_depth(), 2);
    assert_eq!(a.popup_configured_at(0), Some((10, 30, 64, 48)));
    assert_eq!(
        a.popup_configured_at(1),
        Some((5, 15, 32, 24)),
        "the submenu is placed in its parent popup's coordinates"
    );
    assert!(!a.popup_done(), "nothing dismissed this chain");

    // Unwinds in the order xdg-shell requires, without a protocol error.
    a.destroy_popup();
    assert_eq!(a.popup_depth(), 1);
    a.destroy_popup();
    assert_eq!(a.popup_depth(), 0);
    a.detach();
}

/// `xdg_popup.reposition` re-runs placement and echoes the client's token
/// back through `xdg_popup.repositioned`.
///
/// The token is opaque and the compositor forges nothing: wlroots emits
/// `repositioned` off the configure the compositor's own re-placement
/// triggers, so an echoed token *is* the proof that placement re-ran.
///
/// Mutation check: make `State::popup_reposition` return before
/// `configure_popup_now` and neither the new geometry nor the token arrives.
#[test]
fn a_reposition_request_reconfigures_and_echoes_the_token() {
    let comp = Compositor::spawn();
    let mut a = TestClient::map_toplevel(&comp.socket, "popup.app", "popup");
    assert!(a.wait_until(|c| c.last_configure().is_some()));

    a.open_popup(
        PopupSpec::new(64, 48)
            .anchor_rect(10, 10, 20, 20)
            .anchor(PopupAnchor::BottomLeft)
            .gravity(PopupGravity::BottomRight),
    );
    assert_eq!(a.popup_configured(), Some((10, 30, 64, 48)));
    assert_eq!(a.popup_repositioned(), None, "nothing has repositioned yet");

    const TOKEN: u32 = 0x1234_5678;
    a.reposition_popup(
        PopupSpec::new(64, 48)
            .anchor_rect(50, 50, 20, 20)
            .anchor(PopupAnchor::BottomLeft)
            .gravity(PopupGravity::BottomRight),
        TOKEN,
    );

    assert!(
        a.wait_until(|c| c.popup_repositioned() == Some(TOKEN)),
        "the reposition token was never echoed back"
    );
    assert_eq!(
        a.popup_configured(),
        Some((50, 70, 64, 48)),
        "the popup must be reconfigured at the new positioner's geometry"
    );

    a.detach();
}

/// Move the pointer to `point`, press and release `BTN_LEFT`.
fn click_at(comp: &Compositor, vp: &mut VirtualPointerClient, point: (i32, i32)) {
    let (ow, oh) = comp.output_size();
    vp.motion_absolute(point.0 as f64, point.1 as f64, ow as u32, oh as u32);
    vp.frame();
    vp.button(BTN_LEFT, true);
    vp.frame();
    vp.button(BTN_LEFT, false);
    vp.frame();
}

/// Press and release inside `client`'s surface at `point`, and return the
/// serial the press minted -- the serial an `xdg_popup.grab` must cite.
///
/// Waits for the client's own `enter` and `button` rather than sleeping: the
/// serial does not exist until the press has actually been delivered.
fn mint_pointer_serial(
    comp: &Compositor,
    vp: &mut VirtualPointerClient,
    client: &mut TestClient,
    point: (i32, i32),
) -> u32 {
    let (ow, oh) = comp.output_size();
    let enters = client.pointer_enters();
    vp.motion_absolute(point.0 as f64, point.1 as f64, ow as u32, oh as u32);
    vp.frame();
    assert!(
        client.wait_until(|c| c.pointer_enters() > enters),
        "the client never got wl_pointer.enter at {point:?}"
    );
    let buttons = client.pointer_buttons().len();
    vp.button(BTN_LEFT, true);
    vp.frame();
    assert!(
        client.wait_until(|c| c.pointer_buttons()[buttons..].contains(&(BTN_LEFT, true))),
        "the client never got the press that mints the grab serial"
    );
    let serial = client
        .last_pointer_serial()
        .expect("the press just delivered a serial");
    let buttons = client.pointer_buttons().len();
    vp.button(BTN_LEFT, false);
    vp.frame();
    assert!(
        client.wait_until(|c| c.pointer_buttons()[buttons..].contains(&(BTN_LEFT, false))),
        "the client never got the release"
    );
    serial
}

/// The `app_id` of whatever the model currently says is focused.
fn focused_app_id(comp: &Compositor) -> Option<String> {
    comp.snapshot()
        .windows
        .iter()
        .find(|w| w.focused)
        .map(|w| w.app_id.clone())
}

/// Poll the model's focus for up to `window`, returning whether it ever became
/// `app_id`. A generous bound, not a wall-clock pin: focus changes here are
/// driven by deferred handler events, so the only honest assertion is
/// "eventually".
fn wait_for_focus(comp: &Compositor, app_id: &str, window: Duration) -> bool {
    let deadline = std::time::Instant::now() + window;
    loop {
        if focused_app_id(comp).as_deref() == Some(app_id) {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Poll the model until it holds exactly `count` windows, or `window`
/// elapses. Returns whether it ever did.
fn wait_for_window_count(comp: &Compositor, count: usize, window: Duration) -> bool {
    let deadline = std::time::Instant::now() + window;
    loop {
        if comp.snapshot().windows.len() == count {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// One click outside a grabbing chain dismisses the **whole** chain, not just
/// its topmost level.
///
/// wlroots owns this: the compositor installs no grab of its own (contract
/// §1.6), so what is under test is that the compositor still does the two
/// things the popup grab needs from it -- clearing pointer focus on an
/// out-of-client enter, and notifying the button -- with its own implicit
/// pointer grab in the way.
///
/// Mutation check: skip `notify_button` under an explicit grab and
/// `popup_done` never arrives. (This is the same mutation
/// `client_protocol.rs`'s grab regression test records; re-run it here after
/// the popup actually maps, since a mapped popup takes a different pointer
/// focus path than the never-mapped one that test used to open.)
#[test]
fn a_grabbing_popup_chain_is_dismissed_whole_by_a_click_outside_it() {
    let comp = Compositor::spawn();
    let mut vp = VirtualPointerClient::spawn(&comp.socket);
    let mut a = TestClient::map_toplevel(&comp.socket, "popup.app", "popup");
    assert!(a.wait_until(|c| c.last_configure().is_some()));

    let content = content_rect_of(&comp, "popup.app");
    let inside = (
        content.x + content.width / 2,
        content.y + content.height / 2,
    );
    let serial = mint_pointer_serial(&comp, &mut vp, &mut a, inside);

    a.open_popup(
        PopupSpec::new(64, 48)
            .anchor_rect(10, 10, 20, 20)
            .anchor(PopupAnchor::BottomLeft)
            .gravity(PopupGravity::BottomRight)
            .grab(serial),
    );
    a.open_popup_from_popup(
        PopupSpec::new(32, 24)
            .anchor_rect(5, 5, 10, 10)
            .anchor(PopupAnchor::BottomLeft)
            .gravity(PopupGravity::BottomRight),
    );
    assert_eq!(a.popup_depth(), 2, "both levels mapped");
    assert!(!a.popup_done(), "nothing has dismissed the chain yet");

    // Bare desktop: outside every surface this client owns.
    let (ow, oh) = comp.output_size();
    let outside = (ow - 2, oh - 2);
    let frame = comp
        .snapshot()
        .windows
        .iter()
        .find(|w| w.app_id == "popup.app")
        .expect("the parent must be in the model")
        .geometry;
    assert!(
        !frame.contains(outside.0, outside.1),
        "{outside:?} is inside the parent {frame:?}, so the click would not \
         be outside the popup's client"
    );
    click_at(&comp, &mut vp, outside);

    assert!(
        a.wait_until(|c| c.popup_done()),
        "the chain was never dismissed by a click outside it"
    );
    // *Which* levels were told, recorded rather than assumed. wlroots owns
    // this path, and what it actually does (observed on the wire) is send
    // `popup_done` on the **grabbing root only** -- twice, once from ending
    // the grab and once from destroying the popup -- while the nested level's
    // role is destroyed with its parent, silently. The chain does come down
    // whole; the client simply never hears about depth 1. So this test cannot
    // speak to teardown *order*, and does not pretend to:
    // `destroying_a_parent_destroys_its_popup_chain_without_a_double_free` is
    // where deepest-first is pinned, on the compositor's own
    // `Runtime::dismiss_popup` path.
    assert!(
        a.popup_done_depths().iter().all(|depth| *depth == 0),
        "a grabbing chain's popup_done is addressed to its root, not to the \
         nested level; got depths {:?}",
        a.popup_done_depths()
    );

    // The client still owns both proxies; destroying them in reverse order
    // after a popup_done must not be a protocol error.
    a.destroy_popup();
    a.destroy_popup();
    a.detach();
}

/// When a grabbing chain ends, keyboard focus goes back to the chain's
/// **parent**, not to whatever the dismissing click landed on.
///
/// Spec §2: "on destroy, focus returns to the parent, not the pointer
/// position." The click that dismisses a menu here lands on a *second*
/// window, so the compositor's own click-to-focus moves model focus to `b`
/// first; `restore_focus_after_popups` is what puts it back on `a`.
///
/// Mutation check: delete the `restore_focus_after_popups()` call from
/// `State::popup_destroyed` and the final assertion reports `b.app`.
#[test]
fn keyboard_focus_returns_to_the_parent_when_a_grabbing_chain_ends() {
    let comp = Compositor::spawn();
    let mut vp = VirtualPointerClient::spawn(&comp.socket);
    let mut a = TestClient::map_toplevel(&comp.socket, "a.app", "a");
    assert!(a.wait_until(|c| c.last_configure().is_some()));
    let mut b = TestClient::map_toplevel(&comp.socket, "b.app", "b");
    assert!(b.wait_until(|c| c.last_configure().is_some()));

    let a_frame = comp
        .snapshot()
        .windows
        .iter()
        .find(|w| w.app_id == "a.app")
        .expect("a must be in the model")
        .geometry;
    let b_content = content_rect_of(&comp, "b.app");
    // A point on `b` that `a`'s frame does not cover. The compositor cascades
    // new windows by a fixed step, so this strip exists; assert it rather than
    // assume it, and say why if it ever stops existing.
    let on_b = (
        a_frame.x + a_frame.width + 4,
        b_content.y + b_content.height / 2,
    );
    assert!(
        b_content.contains(on_b.0, on_b.1) && !a_frame.contains(on_b.0, on_b.1),
        "{on_b:?} must be inside b's content {b_content:?} and outside a's \
         frame {a_frame:?} -- the window cascade no longer leaves a strip of \
         b uncovered, so this test needs a different outside point"
    );

    // Click into `a` to focus it and mint the grab serial. `b` cascades on
    // top of `a` and is large enough to cover `a`'s content center, so the
    // press has to land in the strip of `a` that `b`'s frame does not
    // cover -- the same reasoning `on_b` above uses in the other direction.
    let b_frame = comp
        .snapshot()
        .windows
        .iter()
        .find(|w| w.app_id == "b.app")
        .expect("b must be in the model")
        .geometry;
    let a_content = content_rect_of(&comp, "a.app");
    let on_a = (a_content.x + 4, a_content.y + a_content.height / 2);
    assert!(
        a_content.contains(on_a.0, on_a.1) && !b_frame.contains(on_a.0, on_a.1),
        "{on_a:?} must be inside a's content {a_content:?} and outside b's \
         frame {b_frame:?} -- the window cascade no longer leaves a strip of \
         a uncovered, so this test needs a different point to click into a"
    );
    let serial = mint_pointer_serial(&comp, &mut vp, &mut a, on_a);
    assert!(
        wait_for_focus(&comp, "a.app", Duration::from_secs(5)),
        "clicking into a must focus it"
    );

    a.open_popup(
        PopupSpec::new(64, 48)
            .anchor_rect(10, 10, 20, 20)
            .anchor(PopupAnchor::BottomLeft)
            .gravity(PopupGravity::BottomRight)
            .grab(serial),
    );

    click_at(&comp, &mut vp, on_b);
    assert!(
        a.wait_until(|c| c.popup_done()),
        "the click on b never dismissed the chain"
    );
    // `popup_done` only *asks* the client to tear down; wlroots' own grab
    // does not forcibly free the `wlr_xdg_popup` (contract §1.6 -- it is not
    // this compositor's grab to force), so the destroy signal that drives
    // `restore_focus_after_popups` does not fire until the client actually
    // sends `xdg_popup.destroy`, exactly as a real client is required to do
    // the moment it receives `popup_done`. `a_grabbing_popup_chain_is_
    // dismissed_whole_by_a_click_outside_it` above takes the same two-step
    // shape (`wait_until(popup_done)` then `destroy_popup`); this test
    // reconciles the same way so the focus check below is observing real
    // destroy-driven restore rather than a race with an object that is
    // still alive server-side.
    a.destroy_popup();

    assert!(
        wait_for_focus(&comp, "a.app", Duration::from_secs(5)),
        "focus must return to the popup's parent, not follow the pointer to \
         b -- currently focused: {:?}",
        focused_app_id(&comp)
    );

    a.detach();
    b.detach();
}

/// A non-grabbing popup -- a tooltip, a non-modal popover -- never moves
/// keyboard focus, at either end of its life.
///
/// Focus rule 2, and contract deviation D3's reason for existing: rule 3's
/// restore must not fire for a chain that never took focus, or opening a
/// tooltip on an unfocused window would steal the keyboard when the tooltip
/// closed.
///
/// Mutation check: drop the `grabbing &&` guard in `State::record_popup` and
/// the closing assertion reports `a.app`.
#[test]
fn a_non_grabbing_popup_never_moves_keyboard_focus() {
    let comp = Compositor::spawn();
    let mut a = TestClient::map_toplevel(&comp.socket, "a.app", "a");
    assert!(a.wait_until(|c| c.last_configure().is_some()));
    let mut b = TestClient::map_toplevel(&comp.socket, "b.app", "b");
    assert!(b.wait_until(|c| c.last_configure().is_some()));
    assert!(
        wait_for_focus(&comp, "b.app", Duration::from_secs(5)),
        "the most recently mapped window holds focus"
    );

    // `a` -- which is *not* focused -- opens a popup with no grab.
    a.open_popup(
        PopupSpec::new(64, 48)
            .anchor_rect(10, 10, 20, 20)
            .anchor(PopupAnchor::BottomLeft)
            .gravity(PopupGravity::BottomRight),
    );
    assert!(a.popup_configured().is_some(), "the popup never configured");
    assert!(
        !wait_for_focus(&comp, "a.app", Duration::from_millis(300)),
        "opening a non-grabbing popup must not move keyboard focus"
    );
    assert_eq!(focused_app_id(&comp).as_deref(), Some("b.app"));

    a.destroy_popup();
    assert!(
        !wait_for_focus(&comp, "a.app", Duration::from_millis(300)),
        "closing a non-grabbing popup must not move keyboard focus either"
    );
    assert_eq!(focused_app_id(&comp).as_deref(), Some("b.app"));

    a.detach();
    b.detach();
}

/// A **reactive** popup is re-placed when its parent moves.
///
/// The popup is anchored so that it overflows the bottom of the usable area
/// and has to slide: how far it slides is a pure function of where the parent
/// surface sits, so dragging the parent upward must change the configured `y`.
/// A non-reactive popup would keep the placement it was given, which is why
/// `reconstrain_popups` filters on the flag.
///
/// Mutation check: delete the `reconstrain_popups(PopupRoot::Window(id))` call
/// from `State::sync_window_to_scene` and the `wait_until` below times out --
/// the popup keeps its original `y` for the whole drag.
///
/// **Reconciliation (Task 10, P2).** Two departures from the plan's text,
/// both about *when* the move becomes observable rather than about what is
/// asserted. First, the assertion is made after the button comes back up:
/// the compositor commits an interactive move's geometry on release, not on
/// every intermediate motion (`State::handle_pointer_release`'s
/// `DragResult::Moved` arm is the only `set_geometry` call on this path), so
/// the reconfigure cannot be observed mid-drag. Second, `DRAG` is 20 rather
/// than 120, which is all the slide needs: the configured `y` moves by
/// exactly the drag distance (492 -> 512 here), and a shorter drag keeps the
/// pointer inside the parent for the whole gesture.
///
/// An earlier round of this task read the mid-drag silence as a `wlr` bug and
/// shipped the test `#[ignore]`d; it is not one. With the release in place the
/// second `wlr_xdg_popup_unconstrain_from_box` re-sends a genuinely different
/// configure, and the named mutation below has been run and does kill the
/// test with its own message.
#[test]
fn a_reactive_popup_is_reconfigured_when_its_parent_moves() {
    let comp = Compositor::spawn();
    let mut vp = VirtualPointerClient::spawn(&comp.socket);
    let mut a = TestClient::map_toplevel(&comp.socket, "popup.app", "popup");
    assert!(a.wait_until(|c| c.last_configure().is_some()));

    let content = content_rect_of(&comp, "popup.app");
    let (ow, oh) = comp.output_size();

    const H: i32 = 200;
    const DRAG: i32 = 20;
    let ay = oh - content.y - 100;
    assert!(
        ay > H && content.y > DRAG,
        "this output ({ow}x{oh}, content at y = {}) leaves no room to drag \
         the parent upward and still overflow",
        content.y
    );

    a.open_popup(
        PopupSpec::new(64, H)
            .anchor_rect(0, ay, 1, 1)
            .anchor(PopupAnchor::BottomLeft)
            .gravity(PopupGravity::BottomRight)
            .constraint(PopupConstraint::SlideY)
            .reactive(true),
    );
    let (_, before, _, _) = a
        .popup_configured()
        .expect("the reactive popup never configured");

    // Press and hold inside the parent, then drive an interactive move: the
    // button must still be down when `xdg_toplevel.move` arrives.
    let start = (
        content.x + content.width / 2,
        content.y + content.height / 2,
    );
    let enters = a.pointer_enters();
    vp.motion_absolute(start.0 as f64, start.1 as f64, ow as u32, oh as u32);
    vp.frame();
    assert!(a.wait_until(|c| c.pointer_enters() > enters));
    let buttons = a.pointer_buttons().len();
    vp.button(BTN_LEFT, true);
    vp.frame();
    assert!(a.wait_until(|c| c.pointer_buttons()[buttons..].contains(&(BTN_LEFT, true))));
    let serial = a.last_pointer_serial().expect("the press minted a serial");

    a.request_move(serial);
    comp.settle();
    vp.motion_absolute(
        start.0 as f64,
        (start.1 - DRAG) as f64,
        ow as u32,
        oh as u32,
    );
    vp.frame();

    // The compositor commits an interactive move's geometry on release, not
    // on every intermediate motion event (`State::handle_pointer_release`'s
    // `DragResult::Moved` arm is the only `set_geometry` call on this path) --
    // so the reconfigure this test is after cannot be observed until the
    // button comes back up.
    vp.button(BTN_LEFT, false);
    vp.frame();
    comp.settle();

    assert!(
        a.wait_until(|c| c.popup_configured().map(|g| g.1) != Some(before)),
        "a reactive popup must be re-placed when its parent moves; it kept \
         y = {before}"
    );

    let moved = content_rect_of(&comp, "popup.app");
    assert!(
        moved.y < content.y,
        "the drag must actually have moved the parent up: {} -> {}",
        content.y,
        moved.y
    );
    let (_, after, _, ph) = a.popup_configured().expect("still configured");
    assert!(
        after + ph <= oh - moved.y,
        "the re-placed popup must still fit inside the usable area: \
         y = {after}, h = {ph}, bottom = {}",
        oh - moved.y
    );

    a.destroy_popup();
    a.detach();
}

/// A popup on a window the lock screen is covering receives no input at all
/// while the session is locked.
///
/// The isolation is the library's -- its hit test is rooted at the lock band
/// while locked, so a popup on a hidden window is unreachable by construction
/// (contract §1.5) -- and the compositor adds no second gate. This test is the
/// executable proof of that claim, with its own positive controls on both
/// sides of the lock so it cannot pass by the popup simply never having taken
/// input.
///
/// Mutation check: the guard lives in `wlr`'s `Runtime::leaf_surface_at`.
/// Deleting its lock branch makes the middle assertion fail. The two controls
/// are what make that mutation the *only* way this test can pass.
#[test]
fn a_popup_on_a_hidden_window_receives_no_input_while_the_session_is_locked() {
    let comp = Compositor::spawn();
    let mut vp = VirtualPointerClient::spawn(&comp.socket);
    let mut a = TestClient::map_toplevel(&comp.socket, "popup.app", "popup");
    assert!(a.wait_until(|c| c.last_configure().is_some()));

    let content = content_rect_of(&comp, "popup.app");
    a.open_popup(
        PopupSpec::new(64, 48)
            .anchor_rect(10, 10, 20, 20)
            .anchor(PopupAnchor::BottomLeft)
            .gravity(PopupGravity::BottomRight),
    );
    let (px, py, pw, ph) = a.popup_configured().expect("the popup never configured");
    // The popup's centre, in output coordinates.
    let on_popup = (content.x + px + pw / 2, content.y + py + ph / 2);

    // Control 1: unlocked, the popup takes clicks.
    let buttons = a.pointer_buttons().len();
    click_at(&comp, &mut vp, on_popup);
    assert!(
        a.wait_until(|c| c.pointer_buttons().len() > buttons),
        "the popup never took a click even before the lock -- the rest of \
         this test would be vacuous"
    );

    let mut locker = SessionLockClient::spawn(&comp.socket);
    locker.lock();
    assert!(locker.wait_locked(), "session never reported locked");
    assert!(
        comp.session_locked(),
        "compositor is_session_locked() is false"
    );

    // The load-bearing assertion: nothing reaches the popup while locked.
    let buttons = a.pointer_buttons().len();
    click_at(&comp, &mut vp, on_popup);
    assert!(
        !a.wait_until_timeout(Duration::from_millis(300), |c| c.pointer_buttons().len()
            > buttons),
        "a popup behind the lock screen received a click"
    );

    // Control 2: unlocking restores it, so the silence above was the lock and
    // not the popup having quietly gone away.
    locker.unlock();
    assert!(!comp.session_locked(), "still locked after unlock");
    let buttons = a.pointer_buttons().len();
    click_at(&comp, &mut vp, on_popup);
    assert!(
        a.wait_until(|c| c.pointer_buttons().len() > buttons),
        "the popup did not take clicks again after unlocking"
    );

    a.destroy_popup();
    a.detach();
}

/// A popup chain comes down whole, deepest-first, whichever end tears it down
/// -- the **compositor** hiding the parent, or the parent's client dying.
///
/// Half 1 (the compositor end). Minimizing a window takes it off screen, and
/// `State::dismiss_popups_of_hidden_roots` closes the menus hanging off it:
/// the popup's scene subtree would otherwise merely go invisible while the
/// client still believed its menu was open. This is the only path in this
/// compositor that reaches `wlr::Runtime::dismiss_popup`, whose deepest-first
/// rule this half is what pins -- `xdg_popup.destroy` on a popup with live
/// children is an xdg-shell protocol error, so a shallow-first teardown is a
/// real bug, not a style preference. The `wlr` crate's own
/// `dismissal_is_deepest_first` (`runtime.rs`) is `#[ignore]`d and names
/// *this* test as its replacement, so these assertions are the whole of that
/// claim's backing.
///
/// **The dismissed count, not the event order, is what catches a shallow-first
/// dismissal**, and that is a fact about wlroots rather than a choice: it
/// frees a popup's children before the popup itself, so the two
/// `xdg_popup.popup_done` events reach the wire deepest-first however
/// `dismiss_popup` iterated (verified on the wire with `WAYLAND_DEBUG=1`
/// against a `.rev()`-less build). What a shallow-first caller loses is
/// *rows*: destroying the outer popup sweeps the inner one out of the
/// library's registry, so the next iteration misses and the call reports 1
/// instead of 2. Both are asserted below -- the order because a client
/// observing anything else would be seeing a protocol violation, the count
/// because it is the half that can actually fail.
///
/// Mutation checks for half 1, both run. `wlr`: drop the `.rev()` from
/// `Runtime::dismiss_popup` and `comp.popups_dismissed()` reports 1, not 2
/// (the depths are unchanged, per the paragraph above). `icedtea`: delete the
/// `dismiss_popups_of_hidden_roots()` call from `handle_command`'s `Minimize`
/// arm and no `popup_done` arrives at all.
///
/// Half 2 (the client end). A client that dies with a live popup chain takes
/// the whole chain down with it, and the compositor survives.
///
/// The client is dropped without destroying anything -- `wayland-client` 0.31
/// proxies send no destroy on drop, so this is a bare socket close, which is
/// the shape a crashed application has. wlroots then frees the parent's scene
/// tree, and every popup subtree hanging off it, recursively; a compositor
/// that also destroyed a popup's own tree would double-free here (contract
/// §1.5's rule, which `forget_toplevel`'s own doc in the `wlr` crate
/// explains).
///
/// Mutation checks, one per repo. `wlr`: reintroduce a
/// `wlr_scene_node_destroy` on the popup's tree in `forget_popup` and the
/// compositor aborts, so every assertion below times out. `icedtea`: delete
/// the `forget_popups_of_root` call from `State::forget_toplevel` and
/// `a_dying_root_prunes_every_popup_under_it` (state.rs's own unit test)
/// fails -- this e2e cannot see the model's popup map, which is exactly why
/// that unit test exists alongside it.
#[test]
fn destroying_a_parent_destroys_its_popup_chain_without_a_double_free() {
    let comp = Compositor::spawn();

    // --- Half 1: the compositor hides the parent -------------------------
    let mut hidden = TestClient::map_toplevel(&comp.socket, "hidden.app", "hidden");
    assert!(hidden.wait_until(|c| c.last_configure().is_some()));
    hidden.open_popup(
        PopupSpec::new(64, 48)
            .anchor_rect(10, 10, 20, 20)
            .anchor(PopupAnchor::BottomLeft)
            .gravity(PopupGravity::BottomRight),
    );
    hidden.open_popup_from_popup(
        PopupSpec::new(32, 24)
            .anchor_rect(5, 5, 10, 10)
            .anchor(PopupAnchor::BottomLeft)
            .gravity(PopupGravity::BottomRight),
    );
    assert_eq!(hidden.popup_depth(), 2, "both levels mapped");
    assert!(
        hidden.popup_done_depths().is_empty(),
        "nothing has dismissed this chain yet"
    );

    let hidden_id = comp
        .snapshot()
        .windows
        .iter()
        .find(|w| w.app_id == "hidden.app")
        .expect("the parent must be in the model")
        .id;
    comp.send(icedtea_compositor::dbus::DbCommand::Minimize(
        hidden_id, true,
    ));

    assert!(
        hidden.wait_until(|c| c.popup_done_depths().len() == 2),
        "minimizing the parent did not dismiss its whole popup chain (got \
         popup_done for depths {:?})",
        hidden.popup_done_depths()
    );
    assert_eq!(
        hidden.popup_done_depths(),
        [1, 0],
        "the chain must reach the client deepest-first -- a shallow-first \
         destroy is an xdg-shell protocol error"
    );
    assert_eq!(
        comp.popups_dismissed(),
        2,
        "the compositor must have closed both levels; 1 means dismissal ran \
         shallow-first and lost the row it had already swept"
    );
    // The client still owns both proxies; destroying them topmost-first after
    // a popup_done must not be a protocol error.
    hidden.destroy_popup();
    hidden.destroy_popup();
    hidden.detach();
    assert!(wait_for_window_count(&comp, 0, Duration::from_secs(5)));

    // --- Half 2: the parent's client dies --------------------------------
    let mut a = TestClient::map_toplevel(&comp.socket, "popup.app", "popup");
    assert!(a.wait_until(|c| c.last_configure().is_some()));

    a.open_popup(
        PopupSpec::new(64, 48)
            .anchor_rect(10, 10, 20, 20)
            .anchor(PopupAnchor::BottomLeft)
            .gravity(PopupGravity::BottomRight),
    );
    a.open_popup_from_popup(
        PopupSpec::new(32, 24)
            .anchor_rect(5, 5, 10, 10)
            .anchor(PopupAnchor::BottomLeft)
            .gravity(PopupGravity::BottomRight),
    );
    assert_eq!(a.popup_depth(), 2);
    assert!(wait_for_window_count(&comp, 1, Duration::from_secs(5)));

    // No `detach`, no `destroy_popup`: the socket just closes.
    drop(a);

    assert!(
        wait_for_window_count(&comp, 0, Duration::from_secs(5)),
        "the compositor never dropped the dead client's window -- it is \
         wedged or gone"
    );

    // Still alive and still serving: a fresh client maps, opens its own popup,
    // and is configured.
    let mut b = TestClient::map_toplevel(&comp.socket, "after.app", "after");
    assert!(
        b.wait_until(|c| c.last_configure().is_some()),
        "the compositor no longer configures toplevels after the teardown"
    );
    b.open_popup(
        PopupSpec::new(64, 48)
            .anchor_rect(10, 10, 20, 20)
            .anchor(PopupAnchor::BottomLeft)
            .gravity(PopupGravity::BottomRight),
    );
    assert_eq!(b.popup_configured(), Some((10, 30, 64, 48)));
    b.detach();
}
