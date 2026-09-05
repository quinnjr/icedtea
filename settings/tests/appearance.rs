//! The Appearance and Behavior page gates.

mod support;

use std::collections::HashMap;
use std::time::Duration;

use icedtea_harness::{Compositor, ScreencopyClient, VirtualPointerClient};
use icedtea_ui::widgets::color_dialog::ColorDialogC;
use support::{
    EntryAllocation, click, client_origin, dominant_colour, latest_allocations,
    latest_probe_points, matches as colour_matches, paints_something, pixel_at, seeded_config_dir,
    spawn_settings, wait_for_prefix, wait_for_window, wait_pixel_change, wait_pixel_matching,
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

/// Spec §7's Appearance interaction gate: opening the picker and clicking a
/// palette entry repaints the row's swatch.
///
/// Contract deviation P2-D4 moves this out of P4's list — it is an Appearance
/// page gate and P4 may not touch that page.
///
/// Mutation check (run): replace `.on_click(msg)` on `picker`'s palette
/// buttons in `settings/src/pages/appearance.rs` with
/// `.on_click(Msg::ColorPickerClosed)` and this fails on the fold wait with
/// "the palette click never reached update". Restore.
#[test]
fn a_colour_pick_changes_the_swatch() {
    let compositor = Compositor::spawn();
    let socket = compositor.socket_path().to_string_lossy().to_string();
    let config = seeded_config_dir(|cfg| {
        // A colour no palette entry uses, so "it changed" cannot be a
        // coincidence of the starting value.
        cfg.appearance.palette.background = "#010203".to_string();
    });
    // The pointer must exist before `spawn_settings` connects: a headless
    // seat has no pointer capability until an input device shows up, and a
    // client that connects before one exists never calls `wl_seat.get_pointer`
    // for it — a click delivered afterwards is looked up against that
    // client's empty pointer list and silently dropped (`skeleton.rs`'s Task
    // 7 reconciliation, restated here because this is P2's first click
    // against a freshly spawned settings process rather than one already
    // running).
    let mut pointer = VirtualPointerClient::spawn(&socket);
    let report = tempfile::NamedTempFile::new().expect("report file");
    let _settings = spawn_settings(&socket, config.path(), "", "appearance", report.path());

    wait_for_prefix(
        report.path(),
        "alloc appearance_background ",
        Duration::from_secs(20),
    )
    .expect("the appearance page never reported its background swatch");
    let geometry = wait_for_window(&compositor);
    let (ox, oy) = client_origin(geometry);

    let mut screencopy = ScreencopyClient::spawn(&socket);

    let before_box = latest_allocations(report.path())
        .remove("appearance_background")
        .expect("the swatch has an allocation");
    let sample = (
        ox + before_box.x as i32 + before_box.width as i32 / 2,
        oy + before_box.y as i32 + before_box.height as i32 / 2,
    );
    let before = support::pixel_at(&screencopy.capture(), sample.0, sample.1)
        .expect("the swatch is on screen");
    assert!(
        colour_matches(before, (0x01, 0x02, 0x03)),
        "the swatch starts on the seeded colour, got {before:?}"
    );

    // Open the picker.
    //
    // Deviation P2-D19: the plan's literal 10s budget assumed a click's round
    // trip (compositor forwards the button, the app rebuilds and repaints, the
    // probe report gets a fresh `alloc` line) lands quickly once the window is
    // up. It does not — one frame of this window takes ~17.9s to paint in the
    // harness's debug profile, and events arriving during a paint are
    // delivered in one batch afterwards. 180s is four frames' headroom; see
    // `support::PIXEL_CHANGE_BUDGET`'s own comment for the measurement.
    click(&mut pointer, &mut screencopy, sample.0, sample.1);
    if wait_for_prefix(
        report.path(),
        "alloc appearance_swatch_0 ",
        Duration::from_secs(180),
    )
    .is_none()
    {
        panic!("the palette panel never opened");
    }
    // The panel's *last* child (`appearance_picker_close`, after the whole
    // grid) is the settled marker: reading a palette entry's box right after
    // `_swatch_0` first appears can catch a transient layout pass before the
    // grid's later rows have their final position, so a click computed from
    // it lands on the wrong swatch. Waiting for the close button closes that
    // window the same way the rest-state gates wait for their own last id.
    wait_for_prefix(
        report.path(),
        "alloc appearance_picker_close ",
        Duration::from_secs(180),
    )
    .expect("the palette panel never finished laying out");

    // Pick the third palette entry: a mid-blue no theme uses for a control.
    let index = 2usize;
    let expected = ColorDialogC::default_palette()[index];
    let expected_rgb = (
        (expected.r * 255.0).round() as u8,
        (expected.g * 255.0).round() as u8,
        (expected.b * 255.0).round() as u8,
    );
    let target = latest_allocations(report.path())
        .remove(&format!("appearance_swatch_{index}"))
        .expect("the palette entry has an allocation");
    click(
        &mut pointer,
        &mut screencopy,
        ox + target.x as i32 + target.width as i32 / 2,
        oy + target.y as i32 + target.height as i32 / 2,
    );

    // Wait for the *fold* before reading a pixel. `update` writes one
    // `msg <variant>` line per fold, so this is the app saying "the click
    // reached the palette button" — without it the pixel read below races the
    // panel still being on screen, where the sampled point is some other
    // palette entry rather than the row swatch, and a colour that differs
    // from `before` for the wrong reason. It weakens nothing: the assertions
    // are still both pixel reads.
    wait_for_prefix(
        report.path(),
        "msg BackgroundPicked",
        Duration::from_secs(180),
    )
    .expect("the palette click never reached update");
    // The row swatch moved back to where it was before the panel opened, so
    // re-read its box rather than reusing the old one. Same widened budget as
    // the picker-open wait above, for the same reason.
    wait_for_prefix(
        report.path(),
        "alloc appearance_background ",
        Duration::from_secs(180),
    )
    .expect("the swatch's allocation settled after the panel closed");
    let after_box = latest_allocations(report.path())
        .remove("appearance_background")
        .expect("the swatch still has an allocation");
    let after_sample = (
        ox + after_box.x as i32 + after_box.width as i32 / 2,
        oy + after_box.y as i32 + after_box.height as i32 / 2,
    );
    // Wait for the *settled* colour, not merely "different": the frame in
    // which the panel is still closing puts some other palette entry over
    // this point, which differs from `before` for the wrong reason.
    let after = wait_pixel_matching(
        &mut screencopy,
        after_sample.0,
        after_sample.1,
        expected_rgb,
    );

    assert!(
        !colour_matches(after, before),
        "the swatch still shows the old colour {before:?}"
    );
    assert!(
        colour_matches(after, expected_rgb),
        "the swatch shows the picked palette entry: expected {expected_rgb:?}, got {after:?}"
    );
}

/// The Behavior page's only handler shape, end to end: a switch click reaches
/// `update`, the model goes dirty, and the footer recomputes.
///
/// Mutation check (run): delete `.on_toggle(Msg::RaiseOnFocusToggled)` from
/// `settings/src/pages/behavior.rs` and this fails on the fold wait with
/// "the switch click never reached update". Restore.
#[test]
fn toggling_raise_on_focus_repaints_the_switch_and_the_footer() {
    let compositor = Compositor::spawn();
    let socket = compositor.socket_path().to_string_lossy().to_string();
    let config = seeded_config_dir(|cfg| cfg.behavior.raise_on_focus = false);
    // The pointer must exist before `spawn_settings` connects — see the
    // comment on the same call in `a_colour_pick_changes_the_swatch`.
    let mut pointer = VirtualPointerClient::spawn(&socket);
    let report = tempfile::NamedTempFile::new().expect("report file");
    let _settings = spawn_settings(&socket, config.path(), "", "behavior", report.path());

    wait_for_prefix(
        report.path(),
        "alloc behavior_raise_on_focus ",
        Duration::from_secs(45),
    )
    .expect("the behavior page never reported its first switch");
    wait_for_prefix(report.path(), "alloc apply ", Duration::from_secs(45))
        .expect("the footer never reported its Apply button");
    let geometry = wait_for_window(&compositor);
    let (ox, oy) = client_origin(geometry);

    let mut screencopy = ScreencopyClient::spawn(&socket);
    let allocations = latest_allocations(report.path());

    let switch = allocations
        .get("behavior_raise_on_focus")
        .expect("the switch");
    let switch_sample = (
        ox + switch.x as i32 + switch.width as i32 / 2,
        oy + switch.y as i32 + switch.height as i32 / 2,
    );
    let switch_before = support::pixel_at(&screencopy.capture(), switch_sample.0, switch_sample.1)
        .expect("the switch is on screen");
    click(
        &mut pointer,
        &mut screencopy,
        switch_sample.0,
        switch_sample.1,
    );

    // The fold first, then the paint it causes — see the same wait in
    // `a_colour_pick_changes_the_swatch`.
    wait_for_prefix(
        report.path(),
        "msg RaiseOnFocusToggled",
        Duration::from_secs(180),
    )
    .expect("the switch click never reached update");
    // The switch repaints: its trough goes from the off colour to the theme's
    // accent.
    let switch_after = wait_pixel_change(
        &mut screencopy,
        switch_sample.0,
        switch_sample.1,
        switch_before,
    );
    assert!(
        !colour_matches(switch_after, switch_before),
        "the switch never repainted: still {switch_before:?}"
    );

    // And the footer recomputes. Reconciliation (fix wave): the plan's
    // literal form read Apply's own pixel, but Apply's only visible change
    // here is sensitive-vs-insensitive, whose two fills are 4 bytes apart —
    // inside `SCREENCOPY_TOLERANCE` (12), which exists because the output
    // goes through a format conversion. That read can therefore never fail,
    // in either direction, so it is not a test. `footer_text` is what the
    // footer paints, and `update` publishes it verbatim on every fold, so
    // this asserts the same property against a value the tolerance cannot
    // swallow.
    wait_for_prefix(
        report.path(),
        "status Unsaved changes",
        Duration::from_secs(180),
    )
    .expect("the footer never recomputed after the edit");
}
