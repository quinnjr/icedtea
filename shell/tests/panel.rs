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
use icedtea_shell::clipboard::ClipUpdate;
use icedtea_shell::panel::{self, Msg};
use icedtea_shell::taskbar::CompositorUpdate;
use icedtea_ui::gallery::Theme;
use support::{Panel, clip_entry, snapshot, win};

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

/// Task 14, gate 1: a middle-click on a `window_<id>` button reaches
/// `close_window(id)` — the taskbar wiring — and does *not* also fire a
/// spurious `focus_window(id)`. The old GTK test called `render` by hand and
/// never exercised the button-2 gesture; this drives it through the live panel.
///
/// The negative half is what P5-D10 (the `PointerState::observe` primary-button
/// gate) buys: before that fix, the middle release produced both the panel's
/// own `WindowPointerUp{BTN_MIDDLE}` → `close_window` *and* a stray
/// `EventKind::Click` → `focus_window`.
#[test]
fn middle_clicking_a_window_button_closes_it() {
    // mutation: drop the `PointerUp` BTN_LEFT guard in `observe` → the middle
    // release also fires `EventKind::Click` → `focus_window(id)` recorded →
    // the `!contains(("focus",id))` assert goes RED.
    let mut panel = Panel::spawn(Theme::Dark);
    panel.send(Msg::Compositor(Arc::new(CompositorUpdate::Snapshot(
        snapshot(
            vec![win(7, "Seven")],
            vec![WorkspaceInfo {
                id: 0,
                name: String::new(),
            }],
        ),
    ))));
    let (x, y) = panel.point("window_7");
    panel.click_button(x, y, icedtea_ui::window::pointer::BTN_MIDDLE);
    panel.wait_for_calls(
        |calls| calls.contains(&("close".to_string(), 7)),
        "the middle click reached close_window(7)",
    );
    assert!(
        !panel.wm_calls().contains(&("focus".to_string(), 7)),
        "a middle click must not also focus: {:?}",
        panel.wm_calls()
    );
}

/// Task 14, gate 2: a signal arriving while the panel is running — the
/// worker→inbox seam the GTK test could never reach, since it called `render`
/// by hand — re-renders the model. A `WindowOpened` pushed through the inbox
/// adds a live `window_<id>` button, and the freshly added button works: a
/// click on it reaches the command surface, which a clear-and-rebuild would
/// have silently broken by dropping the handler.
#[test]
fn a_window_opened_signal_through_the_inbox_adds_a_button() {
    // mutation: a clear-and-rebuild of the windows container on `Opened`
    // drops the new button's click handler → the final `wait_for_calls(focus,id)`
    // times out → RED.
    let mut panel = Panel::spawn(Theme::Dark);
    panel.send(Msg::Compositor(Arc::new(CompositorUpdate::Snapshot(
        snapshot(
            vec![win(1, "One")],
            vec![WorkspaceInfo {
                id: 0,
                name: String::new(),
            }],
        ),
    ))));
    panel.wait_for("alloc window_1 ");
    assert_eq!(
        panel.labels_under("windows", "window_"),
        vec!["window_1".to_string()]
    );

    // What `compositor_client`'s `WindowOpened` arm produces, arriving the way
    // the forward thread delivers it.
    panel.send(Msg::Compositor(Arc::new(CompositorUpdate::Opened(win(
        4, "Four",
    )))));
    panel.wait_for("alloc window_4 ");
    assert_eq!(
        panel.labels_under("windows", "window_"),
        vec!["window_1".to_string(), "window_4".to_string()],
        "an inbox signal must add a button without a rebuild of the bar"
    );

    // And the new button works: a click on it reaches the command surface,
    // which is what a clear-and-rebuild would have silently broken by
    // dropping the handler.
    let (x, y) = panel.point("window_4");
    panel.click(x, y);
    panel.wait_for_calls(
        |calls| calls.contains(&("focus".to_string(), 4)),
        "the freshly added button is live",
    );
}

/// Task 15, the clipboard popover gate: contract §3.6's steps 5-8. Open the
/// popover with a real click, assert two rows, activate row 0 and see
/// `("activate", 10)` on the mock, then dismiss with an outside click and see
/// the model let go — every row coordinate read off the popover's own probe
/// points (`Window::popup_probe_points`/`popup_position`, P5-D8), never a
/// literal, because M5-D9's window probe cannot see a second surface the
/// compositor may have slid.
///
/// Reconciliation (P5 Task 15). The brief wrote the dismissal and replacement
/// checks as whole-report `!any("popup ...")` scans, which assume a report
/// that forgets. `$ICEDTEA_PROBE_REPORT` is append-only (Task 11 moved its
/// writer into `App::run`), so the popover's *current* rows are a separate,
/// truncate-written file (`panel::write_popup_report`) that `support::Panel`
/// folds into `report()`; against that current-state view the brief's scans
/// hold as written.
#[test]
fn the_clipboard_popover_opens_pastes_and_dismisses() {
    let mut panel = Panel::spawn(Theme::Dark);
    panel.send(Msg::Compositor(Arc::new(CompositorUpdate::Snapshot(
        snapshot(
            vec![win(1, "One")],
            vec![WorkspaceInfo {
                id: 0,
                name: String::new(),
            }],
        ),
    ))));
    panel.send(Msg::Clip(Arc::new(ClipUpdate::History(vec![
        clip_entry(10, "copied text"),
        clip_entry(11, "second entry"),
    ]))));

    // Wait for the seeded layout to *converge* before reading `#clip`'s point,
    // not merely for an `alloc clip` line to exist. On the first, pre-snapshot
    // frame `#clip` is the only right-hand button and sits where `window_1`
    // will land once the snapshot renders and the surface width converges on
    // the output (`Msg::SurfaceWidth`'s one-frame round trip, per
    // `the_bar_spans_the_output_and_fits_its_surface`). Reading its point off
    // that stale frame and clicking there lands on `window_1` after the reflow
    // — the click reaches `focus_window(1)`, never `ClipButtonClicked`, and the
    // popover never opens. The converged frame is the one where `window_1` is
    // laid out (the snapshot landed) *and* `#bar` spans the output (the width
    // settled); `#clip` has taken its final x by then.
    let (out_w, _) = panel.output();
    let out_w = out_w as i32;
    panel.wait_until(
        |lines| {
            let window_1 = lines.iter().any(|l| l.starts_with("alloc window_1 "));
            let bar_spans = lines
                .iter()
                .rev()
                .find(|l| l.starts_with("alloc bar "))
                .and_then(|l| l.split_whitespace().nth(4).map(str::to_owned))
                .and_then(|w| w.parse::<f32>().ok())
                .is_some_and(|w| (w - out_w as f32).abs() <= 2.0);
            window_1 && bar_spans
        },
        "the seeded layout converged before reading #clip's point",
    );

    // 5. Open the popover and assert two rows.
    let (cx, cy) = panel.point("clip");
    panel.click(cx, cy);
    panel.wait_until(
        |lines| {
            lines
                .iter()
                .any(|l| l.starts_with("popup history_open_10 "))
                && lines
                    .iter()
                    .any(|l| l.starts_with("popup history_open_11 "))
        },
        "the popover opened with two rows",
    );

    // 6. Activate row 0 and assert it pasted entry 10.
    let (rx, ry) = panel.popup_point("history_open_10");
    panel.click(rx, ry);
    panel.wait_for_clip_calls(
        |calls| calls.contains(&("activate".to_string(), 10)),
        "activating row 0 reached activate(10)",
    );

    // The paste dismissed the popover: no popup lines remain.
    panel.wait_until(
        |lines| !lines.iter().any(|l| l.starts_with("popup ")),
        "the popover closed after a paste",
    );

    // 7. Reopen, replace the history underneath it, and assert one row — the
    //    open surface tracks the model (contract §6 P5-D2).
    panel.click(cx, cy);
    panel.wait_until(
        |lines| {
            lines
                .iter()
                .any(|l| l.starts_with("popup history_open_10 "))
        },
        "the popover reopened",
    );
    panel.send(Msg::Clip(Arc::new(ClipUpdate::History(vec![clip_entry(
        12, "only",
    )]))));
    panel.wait_until(
        |lines| {
            lines
                .iter()
                .any(|l| l.starts_with("popup history_open_12 "))
                && !lines
                    .iter()
                    .any(|l| l.starts_with("popup history_open_10 "))
        },
        "the open popover followed the history update",
    );

    // 8. An outside click dismisses it, and the model lets go.
    let (ox, oy) = (panel.output().0 as i32 / 2, panel.output().1 as i32 - 20);
    panel.click(ox, oy);
    panel.wait_until(
        |lines| !lines.iter().any(|l| l.starts_with("popup ")),
        "an outside click dismissed the popover",
    );
    assert!(
        !panel.clip_calls().iter().any(|(a, _)| a == "clear"),
        "dismissing must not have pressed anything: {:?}",
        panel.clip_calls()
    );
}

/// Every widget the panel puts on screen paints something at rest.
///
/// The gallery gate's rule, applied to an app: sample each reported
/// allocation and require at least one pixel that is not the wallpaper. No
/// exemption list — an app widget that renders nothing is a bug, not a known
/// gap (spec §7).
fn panel_paints_every_probe_point_at_rest(theme: Theme) {
    let mut panel = Panel::spawn(theme);
    panel.send(Msg::Compositor(Arc::new(CompositorUpdate::Snapshot(
        snapshot(
            vec![win(1, "One"), win(2, "Two")],
            vec![
                WorkspaceInfo {
                    id: 0,
                    name: String::new(),
                },
                WorkspaceInfo {
                    id: 1,
                    name: "web".into(),
                },
            ],
        ),
    ))));
    for id in [
        "bar",
        "workspaces",
        "windows",
        "clip",
        "ws_0",
        "ws_1",
        "window_1",
        "window_2",
    ] {
        panel.wait_for(&format!("alloc {id} "));
    }

    // The seeded layout must *converge* before it is sampled, not merely
    // exist: `#bar`'s own `Msg::SurfaceWidth` round trip
    // (`the_bar_spans_the_output_and_fits_its_surface`) takes one extra frame
    // beyond the snapshot's own, and until it lands `#workspaces`/`#windows`
    // report a stale, pre-reflow box. Waiting on `#bar`'s width, the same
    // signal the other gates in this file already wait on, is what makes the
    // rest-state check "at rest" rather than "mid-reflow".
    let (out_w, _) = panel.output();
    let out_w = out_w as i32;
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
        "the seeded layout converged before the rest-state sample",
    );

    let background = panel.background();
    // `capture_settled`, not `capture`: the report's own convergence and the
    // compositor actually presenting that layout are two different events
    // (see its doc comment) — a plain `capture` here caught the *previous*
    // frame's buttons in the *previous* frame's positions live.
    let frame = panel.capture_settled();
    // The report is append-only across frames (`wait_for`'s doc comment): an
    // id that moved during convergence carries one `alloc` line per frame it
    // changed in, and only the last one describes what is on screen now — the
    // same "latest wins" rule `labels_under` already applies per id. Reading
    // every line unfiltered would flag an id's own stale, pre-convergence box
    // (or a since-superseded one another widget now occupies) as blank.
    let mut latest: std::collections::HashMap<String, (i32, i32, i32, i32)> =
        std::collections::HashMap::new();
    for line in panel.report() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() != 6 || f[0] != "alloc" {
            continue;
        }
        latest.insert(
            f[1].to_string(),
            (
                f[2].parse().unwrap_or(0),
                f[3].parse().unwrap_or(0),
                f[4].parse().unwrap_or(0),
                f[5].parse().unwrap_or(0),
            ),
        );
    }
    let mut blank: Vec<String> = Vec::new();
    for (id, rect) in latest {
        if !support::paints_something(&frame, rect, background) {
            blank.push(format!("{id} at {rect:?}"));
        }
    }
    assert!(
        blank.is_empty(),
        "{}: these widgets painted nothing at rest: {blank:?}",
        theme.name()
    );
}

#[test]
fn panel_paints_every_probe_point_at_rest_in_the_light_theme() {
    panel_paints_every_probe_point_at_rest(Theme::Light);
}

#[test]
fn panel_paints_every_probe_point_at_rest_in_the_dark_theme() {
    panel_paints_every_probe_point_at_rest(Theme::Dark);
}

#[test]
fn panel_paints_every_probe_point_at_rest_in_the_high_contrast_theme() {
    panel_paints_every_probe_point_at_rest(Theme::HighContrast);
}

/// The focused and attention states are visible, not just set.
///
/// Finding F10: `taskbar::render` added both classes and nothing styled
/// either, so the compositor's focus and attention bits were invisible on the
/// bar. `style.css`'s left-accent gradients are what fixed that, and this is
/// what keeps them fixed — a CSS engine that silently dropped
/// `background-image: linear-gradient` on a button would pass every other test
/// in this file.
#[test]
fn a_focused_window_button_looks_different_from_an_unfocused_one() {
    let mut panel = Panel::spawn(Theme::Dark);
    panel.send(Msg::Compositor(Arc::new(CompositorUpdate::Snapshot(
        snapshot(
            vec![win(1, "One"), win(2, "Two")],
            vec![WorkspaceInfo {
                id: 0,
                name: String::new(),
            }],
        ),
    ))));
    panel.wait_for("alloc window_2 ");

    // The seeded layout must converge (the same `Msg::SurfaceWidth` round
    // trip `the_bar_spans_the_output_and_fits_its_surface` and the rest-state
    // gate above both wait on) before the "before" sample is taken -- a
    // sample caught mid-reflow would differ from the "after" one just from
    // the layout still settling, with or without the `focused` class ever
    // changing anything, and would make this gate pass for the wrong reason.
    let (out_w, _) = panel.output();
    let out_w = out_w as i32;
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
        "the seeded layout converged before the before/after sample",
    );

    let sample = |panel: &mut Panel, id: &str| -> (u8, u8, u8) {
        let (x, y, _, h) = panel.allocation(id);
        // `capture_settled`, not `capture`: see its doc comment -- a plain
        // capture here caught a stale, pre-convergence frame live, which
        // made "before" and "after" differ from layout settling alone.
        let frame = panel.capture_settled();
        // Two pixels in from the left edge: the accent stripe is 3px wide.
        support::pixel(&frame, (x + 1) as u32, (y + h / 2) as u32).expect("inside the frame")
    };
    let before = sample(&mut panel, "window_1");

    panel.send(Msg::Compositor(Arc::new(CompositorUpdate::Updated {
        id: 1,
        update: icedtea_contract::WindowUpdate {
            focused: Some(true),
            ..Default::default()
        },
    })));
    // The class change does not move anything, so wait on the pixel.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut after = before;
    while std::time::Instant::now() < deadline && support::same(after, before) {
        std::thread::sleep(std::time::Duration::from_millis(50));
        after = sample(&mut panel, "window_1");
    }
    assert!(
        !support::same(after, before),
        "the `focused` class must change what the button paints: {before:?} -> {after:?}"
    );
}
