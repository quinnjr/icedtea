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
