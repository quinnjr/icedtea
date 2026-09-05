//! The Displays page's per-head control panel.
//!
//! The GTK page kept a `refresh_controls` closure that both recomputed the
//! option lists *and* wrote them into six widgets. On an Elm loop the writes
//! disappear: [`repopulate`] keeps only the model half — the two option
//! vectors a dropdown index maps back through — and [`view`] renders them.

use icedtea_ui::layout::Align;
use icedtea_ui::view::View;
use icedtea_ui::view::builders::{
    DropDownExt, GridExt, SpinButtonExt, box_, drop_down, grid, label, spin_button, switch,
};
use icedtea_ui::widgets::types::Orientation;

use crate::app::{Msg, SettingsModel};
use crate::pages::displays::state::{
    DisplaysState, TRANSFORM_LABELS, TRANSFORM_VALUES, distinct_resolutions, format_refresh,
    refreshes_for,
};

/// The scale spinner's bounds and step — the GTK
/// `SpinButton::with_range(0.5, 4.0, 0.25)` with `set_digits(2)`.
pub const SCALE_MIN: f64 = 0.5;
/// See [`SCALE_MIN`].
pub const SCALE_MAX: f64 = 4.0;
/// See [`SCALE_MIN`].
pub const SCALE_STEP: f64 = 0.25;
/// See [`SCALE_MIN`].
pub const SCALE_DIGITS: u32 = 2;

/// The selected head's index, or `None` when there is no live selection.
fn selection(st: &DisplaysState) -> Option<usize> {
    st.selected
        .filter(|&i| i < st.heads.len() && i < st.edits.len())
}

/// Recompute `res_options` / `refresh_options` for the current selection.
///
/// Every `update` arm that changes the selection or an edit calls this, exactly
/// where the GTK page called `refresh_controls`.
pub fn repopulate(st: &mut DisplaysState) {
    let Some(idx) = selection(st) else {
        st.res_options.clear();
        st.refresh_options.clear();
        return;
    };
    let modes = st.heads[idx].modes.clone();
    let edit = st.edits[idx].clone();

    let res_options = distinct_resolutions(&modes);
    let (cur_w, cur_h) = edit.mode.map_or((0, 0), |m| (m.width, m.height));
    let res_sel = res_options
        .iter()
        .position(|&(w, h)| w == cur_w && h == cur_h)
        .unwrap_or(0);
    let (sel_w, sel_h) = res_options.get(res_sel).copied().unwrap_or((cur_w, cur_h));
    let refresh_options = refreshes_for(&modes, sel_w, sel_h);

    st.res_options = res_options;
    st.refresh_options = refresh_options;
}

/// `"{w}x{h}"` per distinct resolution, in `res_options` order.
#[must_use]
pub fn resolution_labels(st: &DisplaysState) -> Vec<String> {
    st.res_options
        .iter()
        .map(|(w, h)| format!("{w}\u{d7}{h}"))
        .collect()
}

/// `"{n:.2} Hz"` per refresh rate, in `refresh_options` order.
#[must_use]
pub fn refresh_labels(st: &DisplaysState) -> Vec<String> {
    st.refresh_options
        .iter()
        .map(|&r| format_refresh(r))
        .collect()
}

/// The resolution dropdown's selected index; `0` when the edit names none.
#[must_use]
pub fn resolution_index(st: &DisplaysState) -> usize {
    let Some(idx) = selection(st) else { return 0 };
    let (w, h) = st.edits[idx].mode.map_or((0, 0), |m| (m.width, m.height));
    st.res_options
        .iter()
        .position(|&(ow, oh)| ow == w && oh == h)
        .unwrap_or(0)
}

/// The refresh dropdown's selected index; `0` when the edit names none.
#[must_use]
pub fn refresh_index(st: &DisplaysState) -> usize {
    let Some(idx) = selection(st) else { return 0 };
    let r = st.edits[idx].mode.map_or(0, |m| m.refresh_mhz);
    st.refresh_options.iter().position(|&o| o == r).unwrap_or(0)
}

/// The transform dropdown's selected index, over all eight raw
/// `wl_output.transform` values (state.rs finding #8).
#[must_use]
pub fn transform_index(st: &DisplaysState) -> usize {
    let Some(idx) = selection(st) else { return 0 };
    let t = st.edits[idx].transform.unwrap_or(0);
    TRANSFORM_VALUES.iter().position(|&v| v == t).unwrap_or(0)
}

/// The scale spinner's value; `1.0` when the edit names none.
#[must_use]
pub fn scale_value(st: &DisplaysState) -> f64 {
    selection(st).map_or(1.0, |idx| st.edits[idx].scale.unwrap_or(1.0))
}

/// `"{x}, {y}"`, or an em dash when nothing is selected.
#[must_use]
pub fn position_text(st: &DisplaysState) -> String {
    match selection(st) {
        Some(idx) => {
            let (x, y) = st.edits[idx].position.unwrap_or((0, 0));
            format!("{x}, {y}")
        }
        None => "\u{2014}".to_string(),
    }
}

/// One labelled grid row: a start-aligned label in column 0, the control in 1.
fn row(text: &str, r: u16, control: View<Msg>) -> [View<Msg>; 2] {
    [label(text).halign(Align::Start).at(0, r), control.at(1, r)]
}

/// The per-head panel: enabled, resolution, refresh, scale, transform,
/// position — the same six controls, in the same order, as the GTK grid.
#[must_use]
pub fn view(m: &SettingsModel) -> View<Msg> {
    let st = &m.displays;
    let live = selection(st).is_some();
    let enabled = selection(st).is_some_and(|i| st.edits[i].enabled);

    let res_labels = resolution_labels(st);
    let res_refs: Vec<&str> = res_labels.iter().map(String::as_str).collect();
    let ref_labels = refresh_labels(st);
    let ref_refs: Vec<&str> = ref_labels.iter().map(String::as_str).collect();

    let mut children: Vec<View<Msg>> = Vec::with_capacity(12);
    children.extend(row(
        "Enabled",
        0,
        switch(enabled)
            .id("displays_enabled")
            .halign(Align::Start)
            .sensitive(live)
            .on_toggle(Msg::HeadEnabledToggled),
    ));
    children.extend(row(
        "Resolution",
        1,
        drop_down(&res_refs)
            .selected(resolution_index(st))
            .id("displays_resolution")
            .sensitive(live && !res_refs.is_empty())
            .on_selected(Msg::ResolutionSelected),
    ));
    children.extend(row(
        "Refresh",
        2,
        drop_down(&ref_refs)
            .selected(refresh_index(st))
            .id("displays_refresh")
            .sensitive(live && !ref_refs.is_empty())
            .on_selected(Msg::RefreshSelected),
    ));
    children.extend(row(
        "Scale",
        3,
        spin_button(scale_value(st), SCALE_MIN, SCALE_MAX)
            .step(SCALE_STEP)
            .digits(SCALE_DIGITS)
            .id("displays_scale")
            .sensitive(live)
            .on_value_changed(Msg::HeadScaleChanged),
    ));
    children.extend(row(
        "Transform",
        4,
        drop_down(&TRANSFORM_LABELS)
            .selected(transform_index(st))
            .id("displays_transform")
            .sensitive(live)
            .on_selected(Msg::TransformSelected),
    ));
    children.extend(row(
        "Position",
        5,
        label(&position_text(st))
            .id("displays_position")
            .halign(Align::Start),
    ));

    box_(
        Orientation::Vertical,
        [grid(children).row_spacing(8u32).column_spacing(12u32)],
    )
    .id("displays_controls")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outputs::{Head, Mode, ModeRequest};
    use crate::pages::displays::state::{DisplaysState, baseline_edit};

    fn mode(w: i32, h: i32, mhz: i32, preferred: bool) -> Mode {
        Mode {
            width: w,
            height: h,
            refresh_mhz: mhz,
            preferred,
        }
    }

    /// A head advertising 1920x1080@60/144 and 1280x720@60.
    fn multi_mode_head() -> Head {
        Head {
            name: "DP-1".to_string(),
            description: "DP-1 display".to_string(),
            enabled: true,
            modes: vec![
                mode(1920, 1080, 60_000, true),
                mode(1920, 1080, 144_000, false),
                mode(1280, 720, 60_000, false),
            ],
            current_mode: Some(mode(1920, 1080, 60_000, true)),
            x: 0,
            y: 0,
            scale: 1.0,
            transform: 0,
        }
    }

    fn state_with(head: Head) -> DisplaysState {
        let mut st = DisplaysState::new();
        st.edits = vec![baseline_edit(&head)];
        st.heads = vec![head];
        st.selected = Some(0);
        repopulate(&mut st);
        st
    }

    /// Mutation check: have `repopulate` build `refresh_options` from the
    /// head's *whole* mode list instead of `refreshes_for` at the selected
    /// resolution; 1280x720's 60 Hz reappears and the length assertion fails.
    /// Restore.
    #[test]
    fn repopulate_fills_the_option_lists_for_the_selected_head() {
        let st = state_with(multi_mode_head());
        assert_eq!(st.res_options, vec![(1920, 1080), (1280, 720)]);
        assert_eq!(st.refresh_options, vec![144_000, 60_000]);
        assert_eq!(
            resolution_labels(&st),
            vec!["1920\u{d7}1080", "1280\u{d7}720"]
        );
        assert_eq!(refresh_labels(&st), vec!["144.00 Hz", "60.00 Hz"]);
    }

    /// Mutation check: have `resolution_index` return `0` unconditionally;
    /// the 1280x720 assertion fails. Restore.
    #[test]
    fn the_dropdown_indices_follow_the_edit() {
        let mut st = state_with(multi_mode_head());
        assert_eq!(resolution_index(&st), 0);
        assert_eq!(refresh_index(&st), 1, "60 Hz is second, 144 Hz first");
        assert_eq!(transform_index(&st), 0);

        st.edits[0].mode = Some(ModeRequest {
            width: 1280,
            height: 720,
            refresh_mhz: 60_000,
        });
        st.edits[0].transform = Some(6);
        repopulate(&mut st);
        assert_eq!(resolution_index(&st), 1);
        assert_eq!(refresh_index(&st), 0, "720p offers only 60 Hz");
        assert_eq!(
            transform_index(&st),
            6,
            "Flipped 180 is TRANSFORM_VALUES[6]"
        );
    }

    /// Mutation check: return `"—"` from `position_text` unconditionally; this
    /// fails on the selected case. Restore.
    #[test]
    fn the_position_text_is_the_edits_position_or_an_em_dash() {
        let mut st = state_with(multi_mode_head());
        assert_eq!(position_text(&st), "0, 0");
        st.edits[0].position = Some((1920, -180));
        assert_eq!(position_text(&st), "1920, -180");
        st.selected = None;
        repopulate(&mut st);
        assert_eq!(position_text(&st), "\u{2014}");
    }

    /// With nothing selected every list empties and the scale falls back to 1.
    ///
    /// Mutation check: drop the `_ =>` arm of `repopulate`; the stale option
    /// lists survive and this fails. Restore.
    #[test]
    fn no_selection_clears_the_option_lists() {
        let mut st = state_with(multi_mode_head());
        st.selected = None;
        repopulate(&mut st);
        assert!(st.res_options.is_empty());
        assert!(st.refresh_options.is_empty());
        assert_eq!(scale_value(&st), 1.0);
    }

    /// A selection index past the end of `heads` is treated as no selection —
    /// never an out-of-bounds index.
    ///
    /// Mutation check: index `st.heads[idx]` directly; this panics. Restore.
    #[test]
    fn a_stale_selection_index_is_treated_as_no_selection() {
        let mut st = state_with(multi_mode_head());
        st.selected = Some(7);
        repopulate(&mut st);
        assert!(st.res_options.is_empty());
        assert_eq!(position_text(&st), "\u{2014}");
    }

    /// Mutation check: drop the `.id("displays_transform")` from `view`; the
    /// id assertion fails. Restore.
    /// Collect every `id` set anywhere in a `View` tree. `icedtea_ui` exposes
    /// no `ids_of` (reconciliation, contract §4.4's id is `PropName::Id` on
    /// `Props`, and `View`'s `children` is a plain public field) — this walks
    /// the raw, unbuilt tree `view` returns directly.
    fn view_ids(v: &icedtea_ui::view::View<crate::app::Msg>) -> Vec<String> {
        fn walk(v: &icedtea_ui::view::View<crate::app::Msg>, out: &mut Vec<String>) {
            if let Some(id) = v.props.str(icedtea_ui::view::PropName::Id) {
                out.push(id.to_string());
            }
            for child in &v.children {
                walk(child, out);
            }
        }
        let mut out = Vec::new();
        walk(v, &mut out);
        out
    }

    #[test]
    fn the_control_panel_ids_are_the_contract_ids() {
        let (m, _workers) = crate::app::tests::test_model();
        let ids = view_ids(&view(&m));
        for id in [
            "displays_enabled",
            "displays_resolution",
            "displays_refresh",
            "displays_scale",
            "displays_transform",
            "displays_position",
        ] {
            assert!(
                ids.iter().any(|got| got == id),
                "missing id {id}; got {ids:?}"
            );
        }
    }
}
