//! A2 batch-1 passive-protocol tests: the six globals `d69ff5f` wired up at
//! boot (viewporter, fractional-scale, single-pixel-buffer, content-type,
//! presentation-time, xdg-output), proven with real `wayland-client`s
//! against a real headless compositor -- the same style
//! `compositor/tests/client_protocol.rs` already uses for every other
//! interop protocol in this crate.

use icedtea_harness::Compositor;

/// The six A2 batch-1 globals must all be advertised -- the daemon-free
/// baseline every other test in this file assumes.
#[test]
fn a2_batch1_globals_are_advertised() {
    let comp = Compositor::spawn();
    let globals = icedtea_harness::advertised_globals(&comp.socket);
    for iface in [
        "wp_viewporter",
        "wp_fractional_scale_manager_v1",
        "wp_single_pixel_buffer_manager_v1",
        "wp_content_type_manager_v1",
        "wp_presentation",
        "zxdg_output_manager_v1",
    ] {
        assert!(
            globals.iter().any(|g| g == iface),
            "{iface} global missing; saw {globals:?}"
        );
    }
}
