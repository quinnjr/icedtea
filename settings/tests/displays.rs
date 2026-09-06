//! The Displays page's gates: rest-state paint, drag-to-snap, drop-down fit.

#[path = "support/displays.rs"]
mod support;

use std::time::Duration;

use support::{REACT, SettingsDriver, TEST_THEME};

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
