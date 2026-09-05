//! The Displays page: a visual drag canvas of the connected monitors plus
//! per-monitor controls (enabled / resolution / refresh / scale / transform),
//! talking to the running compositor over `zwlr_output_management_v1` via the
//! GTK-free [`crate::outputs`] client.
//!
//! Unlike the other pages this one does **not** go through the redb `Config`
//! working-copy — the compositor persists applied layouts on its side. Edits
//! accumulate into a per-head [`crate::outputs::HeadEdit`] set; **Test** and
//! **Apply** ship that set down the protocol, and results (or a lost
//! connection) come back on the toolkit's `App::on_fd` pump.

pub mod canvas;
pub mod controls;
pub mod state;

/// The Displays page.
///
/// P1 ships the page's frame only; P4 fills it in (contract §2.6).
#[must_use]
pub fn view(m: &crate::app::SettingsModel) -> icedtea_ui::view::View<crate::app::Msg> {
    let _ = m;
    icedtea_ui::view::builders::box_(
        icedtea_ui::widgets::types::Orientation::Vertical,
        [icedtea_ui::view::builders::label("Displays")],
    )
    .id("displays_page")
    .margin(16, 16, 16, 16)
}
