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
#![allow(unused_imports, dead_code)]

use std::time::Duration;

use icedtea_contract::Rectangle;
use icedtea_harness::{
    Compositor, PopupAnchor, PopupConstraint, PopupGravity, PopupSpec, SessionLockClient,
    TestClient, VirtualKeyboardClient, VirtualPointerClient,
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
