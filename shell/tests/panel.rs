//! The panel, end to end: the real `view`/`update` on a real layer surface
//! under `icedtea_harness::Compositor`, driven by a virtual pointer, with the
//! command surface behind recording mocks.
//!
//! Replaces `tests/shell_gtk.rs`, which introspected a GTK widget tree in
//! its own process. Every coordinate here comes from the panel's own
//! `$ICEDTEA_PROBE_REPORT` (M5-D9), never from a literal.

mod support;

use icedtea_ui::gallery::Theme;
use support::Panel;

/// The scaffolding's own smoke test: booting `Panel` runs the real
/// `panel::view`/`update` on a layer surface under the harness compositor and
/// makes its geometry readable through `$ICEDTEA_PROBE_REPORT`.
///
/// Reconciliation (P5 Task 12). The brief's illustrative assertions —
/// `alloc bar`'s height field is `28` and its width is `>= 100` — do not hold
/// against the committed UI, and the finding is recorded rather than papered
/// over:
///
/// * `28` is the layer surface's committed height and exclusive zone
///   ([`panel::BAR_HEIGHT`]); the `alloc` line carries the laid-out **border
///   box** of the `#bar` node, which is `32` here (a 28px button inside `#bar`'s
///   `padding: 2px 6px`). The two were conflated.
/// * The layer surface *is* anchored left+right and spans the output — the
///   synthetic `root` the app fills reports its centre at the output centre
///   below — but `#bar` itself carries no `width: 100%`, so with an empty,
///   unseeded taskbar (no workspaces, no windows) it is only as wide as its one
///   `clip` button. Its span is a property of seeded content, which Task 13's
///   parity gate drives; asserting it on a bare panel is what was wrong.
///
/// What is asserted instead is exactly what proves the scaffolding, non-
/// vacuously: the panel joined *this* compositor (not the developer's live
/// session), opened its surface, filled it (root centred on the output), and
/// published a well-formed, per-id-addressable report.
#[test]
fn the_panel_opens_a_layer_surface_and_reports_its_geometry() {
    let panel = Panel::spawn(Theme::Dark);

    let bar = panel.wait_for("alloc bar ");
    let fields: Vec<&str> = bar.split_whitespace().collect();
    assert_eq!(
        fields.len(),
        6,
        "an alloc line is `alloc <id> x y w h`: {bar}"
    );
    assert_eq!(fields[0], "alloc");
    assert_eq!(fields[1], "bar");

    // The layer surface is anchored left+right, so the root the app stretches
    // to fill it spans the whole output: its centre is the output's centre.
    // This is the real "spans the output" fact the brief meant, read off the
    // node that actually fills the surface rather than the content box inside
    // it.
    let (root_x, _) = panel.point("root");
    let (out_w, _) = panel.output();
    assert_eq!(
        root_x,
        (out_w / 2) as i32,
        "an L+R-anchored layer surface spans the output, so its root centres on it: {bar}"
    );

    // Per-id lookup resolves a real box: the clip button is laid out and its
    // allocation is addressable by id — the mechanism every later gate reads.
    let (_, _, clip_w, clip_h) = panel.allocation("clip");
    assert!(
        clip_w > 0 && clip_h > 0,
        "the clip button has a real, non-empty box"
    );
}
