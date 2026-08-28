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

use icedtea_harness::{
    Compositor, PopupAnchor, PopupConstraint, PopupGravity, PopupSpec, SessionLockClient,
    TestClient, VirtualKeyboardClient, VirtualPointerClient,
};

/// `BTN_LEFT`, the only button the compositor's decoration path acts on.
const BTN_LEFT: u32 = 0x110;

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
