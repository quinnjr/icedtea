//! The Displays page's gates: rest-state paint, drag-to-snap, drop-down fit.

#[path = "support/displays.rs"]
mod support;

use std::time::Duration;

use icedtea_settings::pages::displays::canvas::CANVAS_BG;
use support::{REACT, SettingsDriver, TEST_THEME};

/// [`CANVAS_BG`] as the `u8` triple a screencopy pixel compares against —
/// `settings/tests/appearance.rs`'s own `Rgba` → `(u8, u8, u8)` conversion,
/// verbatim.
fn canvas_bg_rgb() -> (u8, u8, u8) {
    (
        (CANVAS_BG.r * 255.0).round() as u8,
        (CANVAS_BG.g * 255.0).round() as u8,
        (CANVAS_BG.b * 255.0).round() as u8,
    )
}

/// Every id the Displays page is contractually required to expose
/// (M5 contract §2.3's widget-id list, the `displays.*` row).
const DISPLAYS_IDS: &[&str] = &[
    "displays_canvas",
    "displays_enabled",
    "displays_resolution",
    "displays_refresh",
    "displays_scale",
    "displays_transform",
    "displays_position",
    "displays_status",
    "displays_test",
    "displays_revert",
    "displays_apply",
];

/// The rest-state gate: at rest, in `theme`, every probe point of the Displays
/// page paints something. No `KNOWN_BLANK` exemptions — the spec is explicit
/// that app widgets get none (§7).
fn displays_page_paints_at_rest(theme: &str) {
    let mut driver = SettingsDriver::open(theme, "displays");
    assert!(
        driver.wait_state("displays.dirty", "false", REACT),
        "the settings process never reported its model"
    );

    // Every contractual id must have reported an allocation…
    for id in DISPLAYS_IDS {
        let a = driver.alloc(id);
        assert!(
            a.w > 0 && a.h > 0,
            "{id} has a zero-area allocation {a:?} in the {theme} theme"
        );
    }

    // …and paint inside it.
    let background = driver.background();
    let frame = driver.capture();
    let mut blank = Vec::new();
    for id in DISPLAYS_IDS {
        let a = driver.alloc(id);
        if !support::paints_something(&frame, (a.x, a.y, a.w, a.h), background) {
            blank.push(*id);
        }
    }
    assert!(
        blank.is_empty(),
        "these Displays widgets paint nothing at rest in the {theme} theme: {blank:?}"
    );
}

#[test]
fn displays_page_paints_every_probe_point_at_rest_in_the_light_theme() {
    displays_page_paints_at_rest("light");
}

#[test]
fn displays_page_paints_every_probe_point_at_rest_in_the_dark_theme() {
    displays_page_paints_at_rest("dark");
}

#[test]
fn displays_page_paints_every_probe_point_at_rest_in_the_high_contrast_theme() {
    displays_page_paints_at_rest("hc");
}

/// The canvas specifically: its own backdrop must be [`CANVAS_BG`], not
/// merely "some colour that differs from the window background" — a
/// transparent/gutted `draw()` lets the harness wallpaper bleed through the
/// drawing area, which *also* differs from the plain page background and
/// would pass a differs-from-page check by accident. Sampling a point inside
/// the canvas rect but away from every head tile (the margin band
/// `displays_canvas::compute_view` always leaves around the content, per
/// `state::CANVAS_MARGIN`) and asserting it is [`CANVAS_BG`] catches that: the
/// real `draw()` fills the whole rect with `CANVAS_BG` as its first
/// statement, but a canvas that painted nothing shows wallpaper, not
/// `CANVAS_BG`, at that point.
///
/// Mutation check: make `CANVAS_BG` equal the theme's window background; this
/// fails (the assertion below no longer distinguishes canvas from page).
/// Mutation check 2 (the whole gate, not just this assertion): gut
/// `canvas::draw`'s body so it issues no `draw_rect` calls at all; the sampled
/// point then reads harness wallpaper instead of `CANVAS_BG` and this fails.
/// Restore both.
#[test]
fn the_canvas_paints_its_own_backdrop() {
    let mut driver = SettingsDriver::open(TEST_THEME, "displays");
    assert!(driver.wait_state("displays.dirty", "false", REACT));
    let canvas = driver.alloc("displays_canvas");
    let footer = driver.alloc("displays_footer");
    // Just inside the canvas rect's top-left corner: within the margin band
    // every layout leaves around the content (`state::CANVAS_MARGIN`, 16
    // canvas px), so no head tile ever reaches it regardless of how many
    // heads the harness advertises.
    let expected = canvas_bg_rgb();
    let page = driver.pixel(footer.x + 2, footer.y + footer.h / 2);
    let inside = driver.pixel(canvas.x + 4, canvas.y + 4);
    assert!(
        support::matches(inside, expected),
        "the canvas backdrop {inside:?} does not match CANVAS_BG {expected:?} \
         (a blank/transparent draw() would show the harness wallpaper here instead)"
    );
    assert!(
        !support::matches(inside, page),
        "the canvas backdrop {inside:?} is indistinguishable from the page {page:?}"
    );
}

/// The driver itself: the process boots on the page it was told to, and the
/// report carries both geometry and model state for it.
///
/// Mutation check: pass `"appearance"` as the page; the `displays_canvas`
/// probe point never appears and this fails. Restore.
#[test]
fn the_driver_opens_the_displays_page_and_reads_its_report() {
    let driver = SettingsDriver::open(TEST_THEME, "displays");
    assert!(
        driver
            .wait_state("displays.dirty", "false", REACT)
            .then_some(())
            .is_some(),
        "the report never carried a displays.dirty line"
    );
    let labels: Vec<String> = driver.probe_points().into_iter().map(|p| p.label).collect();
    assert!(
        labels.iter().any(|l| l == "displays_canvas"),
        "no displays_canvas probe point; got {labels:?}"
    );
}

/// The harness advertises `zwlr_output_manager_v1` and at least one head
/// (`settings/tests/outputs_client.rs` proves both), so the page must come up
/// live rather than in its unavailable state.
///
/// Mutation check: force `outputs_available = false` in
/// `SettingsModel::new`; the canvas disappears and this fails. Restore.
#[test]
fn the_displays_page_comes_up_live_under_the_harness() {
    let driver = SettingsDriver::open(TEST_THEME, "displays");
    let labels: Vec<String> = driver.probe_points().into_iter().map(|p| p.label).collect();
    assert!(
        !labels.iter().any(|l| l == "displays_unavailable"),
        "output management should be available under the harness; got {labels:?}"
    );
    assert!(labels.iter().any(|l| l == "displays_apply"));
    let _ = Duration::from_secs(0); // keep the import honest if REACT is unused
}

/// Contract §2.8's named interaction gate: a drag across the canvas moves a
/// head and the model's rectangle follows, read out of the probe report rather
/// than off a pixel.
///
/// The harness advertises a single head, so there is no neighbour to snap
/// against — which makes the origin the binding candidate, and that is exactly
/// what `snap` is asked to prove here: dragged near `(0, 0)` the head lands
/// *on* it, and dragged far away it lands where it was dropped.
///
/// Mutation check: delete the `PointerMotion` forwarding arm from
/// `DrawingAreaC::on_event` (P0's M5-D5 §4); `displays.position` never changes
/// and this fails on the first `wait_state_change`. Restore.
#[test]
fn dragging_a_head_snaps_it_and_updates_the_model() {
    let mut driver = SettingsDriver::open(TEST_THEME, "displays");
    assert!(driver.wait_state("displays.dirty", "false", REACT));

    let canvas = driver.alloc("displays_canvas");
    let start = driver.state("displays.position");
    assert_eq!(
        start.as_deref(),
        Some("0 0"),
        "the harness head starts at the origin"
    );

    // Press on the head's tile — the canvas centre, where the only head is
    // drawn — and drag it well clear of the origin, staying inside the canvas.
    let from = (canvas.x + canvas.w / 2, canvas.y + canvas.h / 2);
    let to = (canvas.x + canvas.w - 8, canvas.y + canvas.h - 8);
    driver.drag(from, to);

    let moved = driver
        .wait_state_change("displays.position", start.clone(), REACT)
        .expect("the drag never reached the model");
    assert_ne!(
        moved.as_str(),
        "0 0",
        "the head must have left the origin; report said {moved:?}"
    );
    assert!(
        driver.wait_state("displays.dirty", "true", REACT),
        "a move must dirty the page"
    );
    // The position is two integers, so the snap ran and rounded.
    let parts: Vec<i32> = moved
        .split_whitespace()
        .map(|n| n.parse().expect("an integer position"))
        .collect();
    assert_eq!(parts.len(), 2, "position is `<x> <y>`, got {moved:?}");

    // Drag it back to within the snap threshold of the origin: it snaps flush.
    //
    // Reconciliation (Task 12): the harness advertises a single head, and
    // `compute_view` fits/centres its *own* bounding box on every
    // `drag_began` (P4-D3) — so, with nothing else to fit against, the head
    // is always drawn at the canvas centre no matter what its model position
    // is. Pressing at `to` (where the first drag visually ended) therefore
    // misses the head entirely and the second drag is silently a no-op; the
    // press point for *any* drag on this harness must be the canvas centre.
    // Releasing at the mirror of `to` through that centre applies the exact
    // negative of the first drag's delta, landing back at the origin.
    let near_origin = (
        2 * canvas.x + canvas.w - to.0,
        2 * canvas.y + canvas.h - to.1,
    );
    driver.drag(from, near_origin);
    assert!(
        driver.wait_state("displays.position", "0 0", REACT),
        "a head dropped near the origin must snap to it; report said {:?}",
        driver.state("displays.position")
    );

    // Selecting the head is part of the same gesture (contract §2.8).
    assert_eq!(driver.state("displays.selected").as_deref(), Some("0"));
}

/// Contract §2.8's P7-D54 regression: an embedded `DropDown` list must open
/// sized to its content rather than at a flat 240px.
///
/// What the probe report actually exposes for an open drop-down (observed
/// live under this harness): the list body is retained in the settings
/// window's own tree — `DropDownC` opens no `xdg_popup` surface — so its
/// allocation is in the same window-local space as `root`, and the
/// `<id>_list` id sits on the popover's `contents` node, which
/// `PopoverC::open` floors to the height the caller asks for
/// (`drop_down_list_height(len)`). The inner `listview` never carries that
/// height: its `row` nodes hold no text and measure to zero, so it collapses
/// to ~16px regardless of the model — which is exactly what the old
/// tautological assertion (`list.h <= MAX && list.h > 0`) hid, since the id
/// used to sit on that collapsed `listview`.
///
/// The harness head advertises a single resolution, so its one-row body is
/// `drop_down_list_height(1)` = 34px of content (border box 36px), far below
/// the 240px cap. Two facts prove it opened content-sized rather than flat:
/// it is at least one row tall (not the old collapsed ~16px), and it is
/// *strictly* below the cap (not the flat 240 the bug produced). It also fits
/// inside the window.
///
/// The transform list is deliberately not asserted here: its eight variants
/// clamp to exactly `DROP_DOWN_MAX_PX`, indistinguishable from the flat-240
/// bug and, anchored low on the page, genuinely taller than the window — the
/// short resolution list is the one that can tell content-sizing from the cap.
///
/// Mutation check: restore the literal `240` in
/// `ui/src/widgets/drop_down.rs`'s `popover.open` call; the `contents` floor
/// becomes 240 (border box 242), the strict-below-cap assertion fails, and so
/// does the `<=` cap assertion (the 2px chrome pushes it past 240). Restore
/// the fix.
#[test]
fn a_drop_down_list_fits_inside_the_settings_window() {
    use icedtea_ui::widgets::drop_down::{DROP_DOWN_MAX_PX, drop_down_list_height};

    let mut driver = SettingsDriver::open(TEST_THEME, "displays");
    assert!(driver.wait_state("displays.dirty", "false", REACT));

    let root = driver.alloc("root");
    let resolution = driver.alloc("displays_resolution");
    let (x, y) = (
        resolution.x + resolution.w / 2,
        resolution.y + resolution.h / 2,
    );
    driver.click(x, y);

    // The retained list body reports its own allocation once it is revealed.
    // Poll past the collapsed intrinsic height: an unsized body reports ~16px,
    // so wait until it has actually been floored to at least one row.
    let one_row = i32::try_from(drop_down_list_height(1)).unwrap();
    let cap = i32::try_from(DROP_DOWN_MAX_PX).unwrap();
    let list = {
        let mut found = None;
        let deadline = std::time::Instant::now() + REACT;
        while std::time::Instant::now() < deadline {
            if let Ok(a) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                driver.alloc("displays_resolution_list")
            })) && a.h >= one_row
            {
                found = Some(a);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        found.expect("the resolution list never reached its content height")
    };

    assert!(
        list.h <= cap,
        "the list is {}px tall, past the {DROP_DOWN_MAX_PX}px cap",
        list.h
    );
    assert!(
        list.y + list.h <= root.y + root.h,
        "the list runs past the bottom of the window: list {list:?}, root {root:?}"
    );
    // Content-sized, not flat: the single-resolution harness head yields a
    // one-row body, so the opened list is at least one row tall (ruling out
    // the old collapsed ~16px) *and* strictly below the 240px cap (ruling out
    // the flat-240 bug, whose 240px content clamps at or past the cap).
    assert!(
        list.h >= one_row,
        "the list collapsed to {}px, short of a single {one_row}px row — it \
         was never sized to its content",
        list.h
    );
    assert!(
        list.h < cap,
        "the list opened at the flat {DROP_DOWN_MAX_PX}px cap ({}px), not its \
         content height (a single {one_row}px resolution row)",
        list.h
    );
}
