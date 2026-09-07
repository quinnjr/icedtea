//! The panel, end to end: the real `view`/`update` on a real layer surface
//! under `icedtea_harness::Compositor`, driven by a virtual pointer, with the
//! command surface behind recording mocks.
//!
//! Replaces `tests/shell_gtk.rs`, which introspected a GTK widget tree in
//! its own process. Every coordinate here comes from the panel's own
//! `$ICEDTEA_PROBE_REPORT` (M5-D9), never from a literal.

mod support;

use std::sync::Arc;

use icedtea_contract::WorkspaceInfo;
use icedtea_shell::panel::{self, Msg};
use icedtea_shell::taskbar::CompositorUpdate;
use icedtea_ui::gallery::Theme;
use support::{Panel, snapshot, win};

/// The scaffolding's own smoke test: booting `Panel` runs the real
/// `panel::view`/`update` on a layer surface under the harness compositor and
/// makes its geometry readable through `$ICEDTEA_PROBE_REPORT`.
///
/// Reconciliation (P5 Task 12). The brief's illustrative assertions —
/// `alloc bar`'s height field is `28` and its width is `>= 100` — did not
/// hold against the UI as Task 12 found it, and the finding was recorded
/// rather than papered over:
///
/// * `28` is the layer surface's committed height and exclusive zone
///   ([`panel::BAR_HEIGHT`]); the `alloc` line carries the laid-out **border
///   box** of the `#bar` node, which Task 12 measured at `32` (a 28px button
///   inside `#bar`'s then-`padding: 2px 6px`). Task 13's mandated fix
///   (`style.css`'s `#bar` rule) closed that gap, so this test's own
///   `alloc bar` line is `28` again by construction, not by assertion here —
///   `the_bar_spans_the_output_and_fits_its_surface` (below) is what
///   actually pins it.
/// * The layer surface *is* anchored left+right and spans the output — the
///   synthetic `root` the app fills reports its centre at the output centre
///   below — but on the bare, unseeded panel this test spawns, `#bar` itself
///   still has not converged on `root`'s width yet (`panel::PanelModel`'s
///   `bar_width`, via its `Msg::SurfaceWidth` round trip, needs a frame
///   beyond the very first one `Panel::spawn` waits for), so asserting a
///   span here, before that message has had a chance to land, would be
///   exactly the kind of race `the_bar_spans_the_output_and_fits_its_surface`
///   is careful to wait out instead.
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

/// Contract §3.6's re-expression of `shell_gtk.rs`, steps 1-4: the same seed
/// snapshot, the same `["One", "Two"]`, the same `("focus", 1)`, the same
/// `["Two"]` after a close, and the same `"workspace"` entry -- asserted
/// through the panel's own report and a real pointer instead of a GTK tree
/// walk and `emit_clicked`.
#[test]
fn a_panel_click_reaches_the_command_surface() {
    let mut panel = Panel::spawn(Theme::Dark);

    // 1. Seed exactly what `shell_gtk.rs` seeded -- through the inbox, which
    //    is the path a real `GetState` reply takes.
    panel.send(Msg::Compositor(Arc::new(CompositorUpdate::Snapshot(
        snapshot(
            vec![win(1, "One"), win(2, "Two")],
            vec![WorkspaceInfo {
                id: 0,
                name: String::new(),
            }],
        ),
    ))));
    panel.wait_until(
        |lines| {
            lines.iter().any(|l| l.starts_with("alloc window_1 "))
                && lines.iter().any(|l| l.starts_with("alloc window_2 "))
        },
        "both window buttons appeared",
    );
    assert_eq!(
        panel.labels_under("windows", "window_"),
        vec!["window_1".to_string(), "window_2".to_string()],
        "two window buttons, in model order"
    );

    // 2. Click the button for window 1 and assert it reached `focus_window(1)`.
    let (x, y) = panel.point("window_1");
    panel.click(x, y);
    panel.wait_for_calls(
        |calls| calls.contains(&("focus".to_string(), 1)),
        "the click reached focus_window(1)",
    );

    // 3. Close window 1 and assert its button is gone.
    //
    // Reconciliation (P5 Task 13): the brief's sample checks
    // `!lines.iter().any(...)` over the *whole* report, but
    // `support::Panel`'s report is append-only across frames (see
    // `wait_for`'s doc comment) -- window 1's `alloc` line from the frame
    // that first drew it never leaves the file. The absence has to be read
    // off the *current* frame only, the same "latest wins" rule
    // `labels_under` already applies per id.
    panel.send(Msg::Compositor(Arc::new(CompositorUpdate::Closed(1))));
    panel.wait_until(
        |lines| {
            let last_frame = lines
                .iter()
                .rposition(|l| l.starts_with("frame "))
                .unwrap_or(0);
            !lines[last_frame..]
                .iter()
                .any(|l| l.starts_with("alloc window_1 "))
        },
        "window 1's button was dropped",
    );
    assert_eq!(
        panel.labels_under("windows", "window_"),
        vec!["window_2".to_string()],
        "the closed window's button is gone and the other survives"
    );

    // 4. Click workspace 0 and assert a `workspace` entry.
    let (wx, wy) = panel.point("ws_0");
    panel.click(wx, wy);
    panel.wait_for_calls(
        |calls| {
            calls
                .iter()
                .any(|(action, id)| action == "workspace" && *id == 0)
        },
        "the workspace click reached set_workspace(0)",
    );
}

/// M5 Task 13, Part B (controller ruling on Task 12's review): `#bar` must
/// span the output like a taskbar, not collapse to a centred pill, and its
/// border box must not overflow the 28px layer-surface height
/// ([`panel::BAR_HEIGHT`]).
#[test]
fn the_bar_spans_the_output_and_fits_its_surface() {
    let panel = Panel::spawn(Theme::Dark);

    let (out_w, _) = panel.output();
    let out_w = out_w as i32;

    // The panel's own `Msg::SurfaceWidth` round trip (`on_frame` reads the
    // surface's just-configured width, sends it in, `update` folds it into
    // the model, and only *then* does `view` re-run with the corrected
    // `width_request`) takes one extra frame beyond the very first one
    // `Panel::spawn` already waited for -- so this waits for the bar's width
    // to actually converge instead of reading whatever the first frame
    // happened to publish.
    panel.wait_until(
        |lines| {
            lines
                .iter()
                .rev()
                .find(|l| l.starts_with("alloc bar "))
                .is_some_and(|l| {
                    let f: Vec<&str> = l.split_whitespace().collect();
                    let width: f32 = f[4].parse().unwrap_or(0.0);
                    (width - out_w as f32).abs() <= 2.0
                })
        },
        "the bar's width converged on the output width",
    );

    let (_, _, bar_w, bar_h) = panel.allocation("bar");

    assert!(
        (bar_w - out_w).abs() <= 2,
        "the bar should span the output ({out_w}px) but was {bar_w}px wide"
    );
    assert!(
        bar_h <= panel::BAR_HEIGHT,
        "the bar's border box ({bar_h}px) must not overflow the {}px surface",
        panel::BAR_HEIGHT
    );
}
