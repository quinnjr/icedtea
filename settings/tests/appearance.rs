//! The Appearance and Behavior page gates.

mod support;

use std::collections::HashMap;
use std::time::Duration;

use icedtea_harness::{Compositor, ScreencopyClient};
use support::{
    EntryAllocation, client_origin, dominant_colour, latest_allocations, latest_probe_points,
    paints_something, pixel_at, seeded_config_dir, spawn_settings, wait_for_prefix,
    wait_for_window,
};

#[test]
fn the_settings_window_maps_and_reports_its_widgets() {
    let compositor = Compositor::spawn();
    let socket = compositor.socket_path().to_string_lossy().to_string();
    let config = seeded_config_dir(|_| {});
    let report = tempfile::NamedTempFile::new().expect("report file");
    let _settings = spawn_settings(&socket, config.path(), "light", "appearance", report.path());

    wait_for_prefix(
        report.path(),
        "alloc appearance_bar_position ",
        Duration::from_secs(20),
    )
    .expect("the appearance page never reported its allocations");
    let geometry = wait_for_window(&compositor);
    let (ox, oy) = client_origin(geometry);

    let allocations = latest_allocations(report.path());
    let entry = allocations
        .get("appearance_wallpaper_entry")
        .expect("the wallpaper entry has an allocation");
    let mut screencopy = ScreencopyClient::spawn(&socket);
    let frame = screencopy.capture();
    assert!(
        pixel_at(&frame, ox + entry.x as i32 + 2, oy + entry.y as i32 + 2).is_some(),
        "the entry's own pixels are inside the captured output"
    );
}

/// Ids the Appearance page must have painted something into at rest.
///
/// Deliberately not "every id in the report": `appearance` and `appearance_picker`
/// are containers whose own paint is their children's, and asserting on them
/// would pass for free. These are the leaves a user can see and touch.
const APPEARANCE_REST_IDS: &[&str] = &[
    "appearance_bar_position",
    "appearance_bar_height",
    "appearance_corner_radius",
    "appearance_background",
    "appearance_foreground",
    "appearance_accent",
    "appearance_wallpaper_entry",
    "appearance_wallpaper_browse",
    "appearance_wallpaper_clear",
    "appearance_wallpaper_status",
];

/// The same, for Behavior.
const BEHAVIOR_REST_IDS: &[&str] = &[
    "behavior_raise_on_focus",
    "behavior_hide_bar_on_fullscreen",
    "behavior_snap_enabled",
    "behavior_snap_gap",
];

/// Every id in `ids` has a non-empty allocation and paints something that is
/// not the page background, in `theme`.
///
/// "Paints something" is the honest assertion, exactly as
/// `ui/tests/gallery_gate.rs` argues: a widget whose rectangle is entirely the
/// window background has not rendered, whatever its node tree says. Colours
/// are not pinned — `themed_button_offscreen.rs` is the file that pins those.
///
/// There are no `KNOWN_BLANK` exemptions here (spec §7): an app page that
/// cannot paint one of its own controls is a defect, not a backlog entry.
fn page_paints_at_rest(theme: &str, page: &str, ids: &[&str]) {
    let compositor = Compositor::spawn();
    let socket = compositor.socket_path().to_string_lossy().to_string();
    let config = seeded_config_dir(|cfg| {
        // A wallpaper the status row can render as a label rather than a
        // missing file, so the row is never accidentally empty.
        cfg.appearance.wallpaper = None;
    });
    let report = tempfile::NamedTempFile::new().expect("report file");
    let _settings = spawn_settings(&socket, config.path(), theme, page, report.path());

    let first = format!("alloc {} ", ids[0]);
    wait_for_prefix(report.path(), &first, Duration::from_secs(20))
        .unwrap_or_else(|| panic!("the {page} page never reported `{first}`"));
    let geometry = wait_for_window(&compositor);
    let (ox, oy) = client_origin(geometry);

    let mut screencopy = ScreencopyClient::spawn(&socket);
    let frame = screencopy.capture();
    let background = dominant_colour(
        &frame,
        (
            ox,
            oy,
            geometry.width,
            geometry.height - support::TITLE_BAR_HEIGHT,
        ),
    );

    let allocations: HashMap<String, EntryAllocation> = latest_allocations(report.path());
    let points = latest_probe_points(report.path());
    let mut diagnostics: Vec<String> = Vec::new();

    for id in ids {
        let Some(alloc) = allocations.get(*id) else {
            diagnostics.push(format!("{id} reported no allocation at all"));
            continue;
        };
        let rect = (
            ox + alloc.x as i32,
            oy + alloc.y as i32,
            alloc.width as i32,
            alloc.height as i32,
        );
        if rect.2 <= 0 || rect.3 <= 0 {
            diagnostics.push(format!("{id} collapsed to {}x{}", rect.2, rect.3));
            continue;
        }
        if !paints_something(&frame, rect, background) {
            diagnostics.push(format!(
                "{id} painted nothing in the {theme} theme; its box is {rect:?}"
            ));
        }
        if let Some(point) = points.iter().find(|p| p.label == *id)
            && support::pixel_at(&frame, ox + point.x, oy + point.y).is_none()
        {
            diagnostics.push(format!(
                "{id}'s probe point ({}, {}) is outside the captured frame",
                point.x, point.y
            ));
        }
    }

    assert!(
        diagnostics.is_empty(),
        "the {page} page failed its rest-state gate in the {theme} theme:\n  {}",
        diagnostics.join("\n  ")
    );
}

#[test]
fn appearance_page_paints_every_probe_point_at_rest_in_the_light_theme() {
    page_paints_at_rest("light", "appearance", APPEARANCE_REST_IDS);
}

#[test]
fn appearance_page_paints_every_probe_point_at_rest_in_the_dark_theme() {
    page_paints_at_rest("dark", "appearance", APPEARANCE_REST_IDS);
}

#[test]
fn appearance_page_paints_every_probe_point_at_rest_in_the_high_contrast_theme() {
    page_paints_at_rest("hc", "appearance", APPEARANCE_REST_IDS);
}

#[test]
fn behavior_page_paints_every_probe_point_at_rest_in_the_light_theme() {
    page_paints_at_rest("light", "behavior", BEHAVIOR_REST_IDS);
}

#[test]
fn behavior_page_paints_every_probe_point_at_rest_in_the_dark_theme() {
    page_paints_at_rest("dark", "behavior", BEHAVIOR_REST_IDS);
}

#[test]
fn behavior_page_paints_every_probe_point_at_rest_in_the_high_contrast_theme() {
    page_paints_at_rest("hc", "behavior", BEHAVIOR_REST_IDS);
}
