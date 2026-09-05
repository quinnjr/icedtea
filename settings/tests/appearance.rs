//! The Appearance and Behavior page gates.

mod support;

use std::time::Duration;

use icedtea_harness::{Compositor, ScreencopyClient};
use support::{
    client_origin, latest_allocations, pixel_at, seeded_config_dir, spawn_settings,
    wait_for_prefix, wait_for_window,
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
