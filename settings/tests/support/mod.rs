//! Harness helpers shared by the settings integration tests.

/// A 480x420 toplevel window on `comp`'s socket, with the bundled Adwaita
/// sheet — the same surface `icedtea-settings` opens.
pub fn open_test_window(comp: &icedtea_harness::Compositor) -> icedtea_ui::window::Window {
    // SAFETY-free: the harness owns the socket for the life of `comp`, and
    // `Window::open` reads `$WAYLAND_DISPLAY` once, here, before any thread
    // that could race it exists.
    unsafe { std::env::set_var("WAYLAND_DISPLAY", &comp.socket) };
    icedtea_ui::window::Window::open(
        icedtea_ui::window::SurfaceSpec {
            role: icedtea_ui::window::Role::Toplevel,
            size: (480, 420),
            title: "icedtea Settings".to_string(),
            app_id: "org.icedtea.Settings".to_string(),
        },
        icedtea_ui::app::compile_theme(&icedtea_ui::app::ThemeSource::Bundled),
        icedtea_ui::text::FontDatabase::new(),
    )
    .expect("open a toplevel on the harness")
}
