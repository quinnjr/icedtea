//! The Behavior page: three boolean switches over `working.behavior`.

/// The Behavior page.
///
/// P1 ships the page's frame only; P2 fills it in (contract §2.6).
#[must_use]
pub fn view(m: &crate::app::SettingsModel) -> icedtea_ui::view::View<crate::app::Msg> {
    let _ = m;
    icedtea_ui::view::builders::box_(
        icedtea_ui::widgets::types::Orientation::Vertical,
        [icedtea_ui::view::builders::label("Behavior")],
    )
    .id("behavior_page")
    .margin(16, 16, 16, 16)
}
