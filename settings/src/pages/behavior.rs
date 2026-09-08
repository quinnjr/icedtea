//! The Behavior page: three boolean switches and the snap gap.
//!
//! Every control is a pure function of `model.working`; there is no populate
//! pass and therefore no populate-vs-write-back guard (contract §2.2). The
//! snap-gap spin button lives here rather than on Appearance (spec §5.4) even
//! though the value it writes is `working.appearance.snap_gap` — the config
//! layout is not part of this migration.

use icedtea_ui::layout::Align;
use icedtea_ui::view::View;
use icedtea_ui::view::builders::{self as w, GridExt, SpinButtonExt};

use crate::app::{Msg, SettingsModel};
use crate::pages::appearance::SPIN_MAX_PX;
use crate::pages::labeled_row;

/// The Behavior page.
#[must_use]
pub fn view(m: &SettingsModel) -> View<Msg> {
    let b = &m.model.working.behavior;
    let children = [
        labeled_row(
            0,
            "Raise on focus",
            w::switch(b.raise_on_focus)
                .halign(Align::Start)
                .id("behavior_raise_on_focus")
                .on_toggle(Msg::RaiseOnFocusToggled),
        ),
        labeled_row(
            1,
            "Hide bar on fullscreen",
            w::switch(b.hide_bar_on_fullscreen)
                .halign(Align::Start)
                .id("behavior_hide_bar_on_fullscreen")
                .on_toggle(Msg::HideBarOnFullscreenToggled),
        ),
        labeled_row(
            2,
            "Snap enabled",
            w::switch(b.snap_enabled)
                .halign(Align::Start)
                .id("behavior_snap_enabled")
                .on_toggle(Msg::SnapEnabledToggled),
        ),
        labeled_row(
            3,
            "Snap gap",
            w::spin_button(
                f64::from(m.model.working.appearance.snap_gap),
                0.0,
                f64::from(SPIN_MAX_PX),
            )
            .halign(Align::Start)
            .step(1.0)
            .id("behavior_snap_gap")
            .on_value_changed(Msg::SnapGapChanged),
        ),
    ];

    w::grid(children.into_iter().flatten())
        .row_spacing(10)
        .column_spacing(16)
        .margin(16, 16, 16, 16)
        .id("behavior")
}

#[cfg(test)]
mod tests {
    use icedtea_ui::anim::{Clock, ManualClock};
    use icedtea_ui::css::cascade::CompiledSheet;
    use icedtea_ui::icons::IconTheme;
    use icedtea_ui::text::FontDatabase;
    use icedtea_ui::view::App;

    use super::*;

    fn ids_of(m: crate::app::SettingsModel) -> Vec<String> {
        let sheet = CompiledSheet::compile(icedtea_ui::BUNDLED_ADWAITA_LIGHT);
        let clock: std::rc::Rc<dyn Clock> = std::rc::Rc::new(ManualClock::new());
        let probe = App::new(m, crate::app::update, view)
            .probe(
                (480, 420),
                sheet,
                FontDatabase::new(),
                IconTheme::from_env(),
                clock,
            )
            .expect("the behavior page lays out");
        probe
            .root()
            .descendants()
            .filter_map(|n| n.id().map(|id| id.as_str().to_string()))
            .collect()
    }

    #[test]
    fn the_page_carries_every_id_its_gates_address() {
        let (m, _workers) = crate::app::tests::test_model();
        let ids = ids_of(m);
        for id in [
            "behavior_raise_on_focus",
            "behavior_hide_bar_on_fullscreen",
            "behavior_snap_enabled",
            "behavior_snap_gap",
        ] {
            assert!(
                ids.contains(&id.to_string()),
                "{id} is missing from {ids:?}"
            );
        }
    }

    #[test]
    fn the_page_lays_out_whatever_the_model_says() {
        // Both extremes of every control: a page that only lays out for the
        // default config is a page that will crash on a real one.
        let (mut m, _workers) = crate::app::tests::test_model();
        m.model.working.behavior.raise_on_focus = true;
        m.model.working.behavior.hide_bar_on_fullscreen = true;
        m.model.working.behavior.snap_enabled = true;
        m.model.working.appearance.snap_gap = crate::pages::appearance::SPIN_MAX_PX;
        assert!(!ids_of(m).is_empty());
    }
}
