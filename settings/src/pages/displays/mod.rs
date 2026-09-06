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

use icedtea_ui::layout::Align;
use icedtea_ui::view::View;
use icedtea_ui::view::builders::{BoxExt, box_, button, label};
use icedtea_ui::widgets::types::Orientation;

use crate::app::{Msg, SettingsModel};
use crate::outputs::OutputsMsg;
use crate::pages::displays::state::DisplaysState;

/// The whole Displays page.
///
/// Either the "output management unavailable" explanation, or the canvas, the
/// per-head panel and the page's own footer — the GTK page's `unavailable`
/// label / `content` box pair, now a branch instead of two visibility flags.
#[must_use]
pub fn view(m: &SettingsModel) -> View<Msg> {
    if !m.outputs_available {
        return box_(
            Orientation::Vertical,
            [label(UNAVAILABLE_TEXT)
                .id("displays_unavailable")
                .halign(Align::Center)
                .valign(Align::Center)
                .vexpand(true)],
        )
        .id("displays");
    }
    box_(
        Orientation::Vertical,
        [
            box_(
                Orientation::Horizontal,
                [canvas::view(m), controls::view(m)],
            )
            .spacing(12u32)
            .vexpand(true)
            .id("displays_content"),
            footer(m),
        ],
    )
    .spacing(8u32)
    .margin(12, 12, 12, 12)
    .id("displays")
}

/// Shown instead of the page when there is no compositor to talk to.
pub const UNAVAILABLE_TEXT: &str =
    "output management unavailable \u{2014} is the icedtea compositor running?";
/// A hotplug discarded unapplied edits.
pub const STATUS_DROPPED: &str = "Displays changed \u{2014} pending edits discarded";
/// A preview was accepted.
pub const STATUS_TEST_OK: &str = "Test succeeded";
/// A preview was rejected.
pub const STATUS_TEST_REJECTED: &str = "Test rejected by the compositor";
/// A real apply landed.
pub const STATUS_APPLIED: &str = "Applied";
/// A real apply was rejected.
pub const STATUS_REJECTED: &str = "Configuration rejected by the compositor";
/// The compositor superseded the request.
pub const STATUS_SUPERSEDED: &str = "Configuration superseded \u{2014} re-reading";
/// A preview is in flight.
pub const STATUS_TESTING: &str = "Testing\u{2026}";
/// A real apply is in flight.
pub const STATUS_APPLYING: &str = "Applying\u{2026}";
/// Shown in place of an empty `displays_status`: nothing has happened yet,
/// but the label must still say something.
///
/// Reconciliation (Task 11): `hexpand` is a known, still-open gap for a plain
/// `Container::Box` (`ui/tests/ingress.rs`'s own note — nothing in the
/// generic layout path reads `Hexpand`/`Halign` for a `Box` child), so an
/// empty-text label here is sized to its own zero-width measurement rather
/// than grown to fill the row, and never paints. This is out of scope for P4
/// to fix (it lives in `icedtea-ui`'s layout, not `pages/displays/*`), so the
/// page instead never *shows* an empty label: idle reads as "Ready" rather
/// than blank, which is the honest status anyway and happens to have a
/// non-zero measured width like every other state `displays_status` takes.
pub const STATUS_READY: &str = "Ready";

/// Reset the pending edits back to the last-known head snapshot.
pub fn reset_edits(st: &mut DisplaysState) {
    st.edits = st.heads.iter().map(state::baseline_edit).collect();
    st.dirty = false;
    if st.selected.is_none_or(|i| i >= st.heads.len()) {
        st.selected = if st.heads.is_empty() { None } else { Some(0) };
    }
}

/// Recompute and publish the canvas view for the current edits.
///
/// The GTK page did this from inside the draw callback; an Elm paint callback
/// may not touch the model (plan P4-D3), so `update` republishes it whenever
/// the head set changes, using the same fixed canvas size the paint uses.
pub fn republish_view(st: &mut DisplaysState) {
    let (_idxs, rects, _enabled) = state::all_rects(st);
    st.view = crate::pages::displays_canvas::compute_view(
        &rects,
        canvas::CANVAS_W,
        canvas::CANVAS_H,
        state::CANVAS_MARGIN,
    );
}

/// Clear an outstanding Test/Apply latch.
fn end_request(m: &mut SettingsModel) {
    m.displays_in_flight = false;
}

/// Submit the current edit set as a preview (`is_test`) or a real apply.
///
/// Runs on the loop thread, inside `update`: this is a wayland request build
/// plus a flush, not a blocking D-Bus round trip, and its synchronous error is
/// the only thing that can keep the in-flight latch honest (plan P4-D8).
pub fn submit(m: &mut SettingsModel, is_test: bool) {
    if m.displays_in_flight {
        return;
    }
    let Some(pump) = m.outputs.clone() else {
        // No output management at all: the page is already showing its
        // unavailable state and there is nothing to latch.
        return;
    };
    let edits = m.displays.edits.clone();
    let result = if is_test {
        pump.test_configuration(&edits)
    } else {
        pump.build_and_send_configuration(&edits)
    };
    match result {
        Ok(()) => {
            m.displays_in_flight = true;
            m.displays_status = if is_test {
                STATUS_TESTING.to_string()
            } else {
                STATUS_APPLYING.to_string()
            };
        }
        Err(err) => {
            m.displays_status = if is_test {
                format!("Test failed: {err}")
            } else {
                format!("Apply failed: {err}")
            };
        }
    }
}

/// Drop the pending edits back to the last-known head snapshot.
pub fn revert(m: &mut SettingsModel) {
    reset_edits(&mut m.displays);
    controls::repopulate(&mut m.displays);
    republish_view(&mut m.displays);
    m.displays_status = String::new();
}

/// The page's own footer: status, Test, Revert, Apply.
///
/// The shared model footer never touches Displays — this page does not go
/// through the redb working copy at all, the compositor persists applied
/// layouts on its side.
///
/// Apply is held insensitive while a request is outstanding so a
/// `HeadsChanged`-driven repaint cannot re-enable it under one (state.rs
/// finding #15); Test is held for the same reason.
#[must_use]
pub fn footer(m: &SettingsModel) -> View<Msg> {
    let dirty = m.displays.dirty;
    let busy = m.displays_in_flight;
    let live = m.outputs_available;
    let status = if m.displays_status.is_empty() {
        STATUS_READY
    } else {
        m.displays_status.as_str()
    };
    box_(
        Orientation::Horizontal,
        [
            label(status)
                .id("displays_status")
                .hexpand(true)
                .halign(Align::Start),
            button("Test")
                .id("displays_test")
                .sensitive(live && !busy)
                .on_click(Msg::DisplaysTest),
            button("Revert")
                .id("displays_revert")
                .sensitive(live && dirty)
                .on_click(Msg::DisplaysRevert),
            button("Apply")
                .id("displays_apply")
                .sensitive(live && dirty && !busy)
                .on_click(Msg::DisplaysApply),
        ],
    )
    .spacing(8u32)
    .id("displays_footer")
}

/// Fold one `zwlr_output_management_v1` message into the model.
///
/// The arm-for-arm equivalent of the glib source's `match` in the GTK page.
/// Which kind of request a reply answers rides on the reply itself as
/// `is_test`, never on `displays_in_flight` — an overlapped Test/Apply pair
/// can never have one reply read with the other's meaning.
///
/// This does not decide `Cmd`: the caller (`app::update`'s `Msg::Outputs`
/// arm) still issues `Cmd::Unwatch` for `Disconnected` — retiring the dead
/// fd's watch is a loop-level concern `on_outputs` (which only ever touches
/// the model) has no way to express, and `tests/outputs_pump.rs` gates on it.
pub fn on_outputs(m: &mut SettingsModel, msg: &OutputsMsg) {
    match msg {
        OutputsMsg::HeadsChanged(heads) => {
            m.outputs_available = true;
            let st = &mut m.displays;
            let r = state::reconcile(&st.heads, &st.edits, st.selected, st.dirty, heads);
            st.heads = heads.clone();
            st.edits = r.edits;
            st.selected = r.selected;
            // Any in-flight drag was indexed against the *old* head list, which
            // the new one may reorder or shorten (state.rs finding #2).
            st.drag = None;
            if !r.compatible {
                st.dirty = false;
            }
            controls::repopulate(st);
            republish_view(st);
            // A `HeadsChanged` is NOT a terminal reply for an outstanding
            // Test/Apply: the compositor still owes a
            // Succeeded/Failed/Cancelled, and clearing the latch here would let
            // a second request overlap the first (state.rs finding #15).
            m.displays_status = if r.dropped {
                STATUS_DROPPED.to_string()
            } else {
                String::new()
            };
        }
        OutputsMsg::ApplySucceeded { is_test } => {
            end_request(m);
            if *is_test {
                // A preview succeeded: keep the edits, and Apply, live.
                m.displays_status = STATUS_TEST_OK.to_string();
            } else {
                m.displays.dirty = false;
                m.displays_status = STATUS_APPLIED.to_string();
            }
        }
        OutputsMsg::ApplyFailed { is_test } => {
            end_request(m);
            if *is_test {
                // A preview was rejected: leave the edits intact to adjust.
                m.displays_status = STATUS_TEST_REJECTED.to_string();
            } else {
                reset_edits(&mut m.displays);
                controls::repopulate(&mut m.displays);
                republish_view(&mut m.displays);
                m.displays_status = STATUS_REJECTED.to_string();
            }
        }
        OutputsMsg::ApplyCancelled => {
            end_request(m);
            m.displays_status = STATUS_SUPERSEDED.to_string();
        }
        OutputsMsg::ManagerUnavailable | OutputsMsg::Disconnected => {
            m.displays_in_flight = false;
            m.outputs_available = false;
            m.displays_status = String::new();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outputs::{Head, Mode, OutputsMsg};
    use crate::pages::displays::state::baseline_edit;

    fn head(name: &str, enabled: bool) -> Head {
        let mode = Mode {
            width: 1920,
            height: 1080,
            refresh_mhz: 60_000,
            preferred: true,
        };
        Head {
            name: name.to_string(),
            description: format!("{name} display"),
            enabled,
            modes: vec![mode],
            current_mode: Some(mode),
            x: 0,
            y: 0,
            scale: 1.0,
            transform: 0,
        }
    }

    /// A model with one enabled head and a pending, unapplied edit.
    ///
    /// Reconciliation (Task 6): `SettingsModel::for_test()` is not a name the
    /// crate exposes; `crate::app::tests::test_model()` (P1-D8's `pub(crate)`
    /// constructor, already used by Tasks 2-5's own test modules) is the
    /// equivalent it does, and it hands back the worker queues alongside the
    /// model.
    fn dirty_model() -> crate::app::SettingsModel {
        let (mut m, _workers) = crate::app::tests::test_model();
        let h = head("DP-1", true);
        m.displays.edits = vec![baseline_edit(&h)];
        m.displays.heads = vec![h];
        m.displays.selected = Some(0);
        m.displays.edits[0].position = Some((640, 480));
        m.displays.dirty = true;
        m.outputs_available = true;
        m
    }

    /// Contract §2.8's named unit test.
    ///
    /// Mutation check: have the `ApplySucceeded` arm read
    /// `m.displays_in_flight` to decide whether it was a test, instead of the
    /// reply's own `is_test`; the second assertion fails. Restore.
    #[test]
    fn an_apply_reply_clears_in_flight_by_its_own_is_test_tag() {
        let mut m = dirty_model();
        m.displays_in_flight = true;
        on_outputs(&mut m, &OutputsMsg::ApplySucceeded { is_test: true });
        assert!(!m.displays_in_flight, "a terminal reply clears the latch");
        assert_eq!(m.displays_status, STATUS_TEST_OK);
        assert!(m.displays.dirty, "a preview keeps the edits live to commit");

        m.displays_in_flight = true;
        on_outputs(&mut m, &OutputsMsg::ApplySucceeded { is_test: false });
        assert!(!m.displays_in_flight);
        assert_eq!(m.displays_status, STATUS_APPLIED);
        assert!(!m.displays.dirty, "a real apply clears the dirty flag");
    }

    /// Mutation check: make the `ApplyFailed { is_test: true }` arm reset the
    /// edits like the `false` arm; the retained-position assertion fails.
    /// Restore.
    #[test]
    fn a_rejected_test_keeps_the_edits_and_a_rejected_apply_resets_them() {
        let mut m = dirty_model();
        m.displays_in_flight = true;
        on_outputs(&mut m, &OutputsMsg::ApplyFailed { is_test: true });
        assert_eq!(m.displays.edits[0].position, Some((640, 480)));
        assert_eq!(m.displays_status, STATUS_TEST_REJECTED);

        m.displays_in_flight = true;
        on_outputs(&mut m, &OutputsMsg::ApplyFailed { is_test: false });
        assert_eq!(
            m.displays.edits[0].position,
            Some((0, 0)),
            "a rejected apply re-baselines from the live head"
        );
        assert!(!m.displays.dirty);
        assert_eq!(m.displays_status, STATUS_REJECTED);
    }

    /// A `HeadsChanged` is not a terminal reply: it must not re-enable the
    /// buttons under an outstanding request (state.rs finding #15).
    ///
    /// Mutation check: clear `displays_in_flight` in the `HeadsChanged` arm;
    /// this fails. Restore.
    #[test]
    fn a_heads_changed_does_not_clear_an_outstanding_request() {
        let mut m = dirty_model();
        m.displays_in_flight = true;
        on_outputs(&mut m, &OutputsMsg::HeadsChanged(vec![head("DP-1", true)]));
        assert!(
            m.displays_in_flight,
            "only a terminal reply clears the latch"
        );
        assert!(m.outputs_available);
    }

    /// A connector-set change discards pending edits and says so.
    ///
    /// Mutation check: pass `false` for `dirty` into `reconcile`; `dropped`
    /// comes back `false` and the status assertion fails. Restore.
    #[test]
    fn a_hotplug_that_discards_pending_edits_says_so() {
        let mut m = dirty_model();
        on_outputs(
            &mut m,
            &OutputsMsg::HeadsChanged(vec![head("DP-1", true), head("HDMI-A-1", true)]),
        );
        assert_eq!(m.displays_status, STATUS_DROPPED);
        assert_eq!(m.displays.heads.len(), 2);
        assert!(
            !m.displays.dirty,
            "a set change resets to the fresh baseline"
        );
        assert!(m.displays.drag.is_none(), "a stale drag index is dropped");
    }

    /// Mutation check: leave `outputs_available` alone in the
    /// `Disconnected` arm; this fails.
    #[test]
    fn losing_the_manager_marks_output_management_unavailable() {
        for msg in [OutputsMsg::ManagerUnavailable, OutputsMsg::Disconnected] {
            let mut m = dirty_model();
            m.displays_in_flight = true;
            on_outputs(&mut m, &msg);
            assert!(
                !m.outputs_available,
                "{msg:?} must mark the page unavailable"
            );
            assert!(!m.displays_in_flight, "and clear any outstanding request");
            assert_eq!(m.displays_status, "");
        }
    }

    /// Mutation check: drop the `ApplyCancelled` arm's `end_request`; the
    /// latch survives and this fails. Restore.
    #[test]
    fn a_cancelled_configuration_clears_the_latch_and_says_superseded() {
        let mut m = dirty_model();
        m.displays_in_flight = true;
        on_outputs(&mut m, &OutputsMsg::ApplyCancelled);
        assert!(!m.displays_in_flight);
        assert_eq!(m.displays_status, STATUS_SUPERSEDED);
    }

    /// A `HeadsChanged` republishes the view so a drag started before the
    /// next paint still maps correctly.
    ///
    /// Mutation check: drop the `republish_view` call; the view keeps its
    /// `DisplaysState::new` identity scale and this fails. Restore.
    #[test]
    fn a_heads_changed_republishes_the_canvas_view() {
        let (mut m, _workers) = crate::app::tests::test_model();
        m.outputs_available = true;
        on_outputs(&mut m, &OutputsMsg::HeadsChanged(vec![head("DP-1", true)]));
        assert!(
            m.displays.view.scale < 1.0,
            "a 1920x1080 head in a 360x240 canvas scales down, got {}",
            m.displays.view.scale
        );
    }

    /// With no pump at all (no compositor, no second connection) a submit is a
    /// status change and nothing else — never a panic, never a stuck latch.
    ///
    /// Mutation check: set `displays_in_flight = true` before the `None` check
    /// in `submit`; this fails. Restore.
    #[test]
    fn a_submit_without_a_pump_never_latches() {
        let mut m = dirty_model();
        assert!(m.outputs.is_none(), "the test model has no pump");
        submit(&mut m, true);
        assert!(!m.displays_in_flight);
        submit(&mut m, false);
        assert!(!m.displays_in_flight);
    }

    /// A second submit while one is outstanding is refused.
    ///
    /// Mutation check: drop the `displays_in_flight` guard from `submit`; the
    /// status is overwritten and this fails. Restore.
    #[test]
    fn a_submit_while_one_is_outstanding_is_refused() {
        let mut m = dirty_model();
        m.displays_in_flight = true;
        m.displays_status = STATUS_TESTING.to_string();
        submit(&mut m, false);
        assert_eq!(
            m.displays_status, STATUS_TESTING,
            "the outstanding request's status survives"
        );
    }

    /// Revert re-baselines the edits, clears dirty, and empties the status —
    /// the GTK `revert_btn` handler exactly.
    ///
    /// Reconciliation (brief step 1): the brief's version of this test uses
    /// `dirty_model()`'s single head and drags *its* position, expecting the
    /// scale to shrink. `displays_canvas::content_bounds` unions rect
    /// extents (`state.rs`/`displays_canvas.rs`), so a *single* rect's
    /// bounding box is exactly its own width/height regardless of where it
    /// sits — translating one head alone can never change the computed
    /// scale, so that assertion can never pass no matter how `revert` is
    /// implemented. This version drags one head of a *two*-head layout far
    /// from the other, which does grow the union bbox (and shrink the
    /// scale) the way the brief's docstring describes, then reverts it.
    ///
    /// Mutation check: drop the `republish_view` call from `revert`; the view
    /// keeps the dragged layout's scale and this fails. Restore.
    #[test]
    fn revert_rebaselines_the_edits_and_republishes_the_view() {
        let (mut m, _workers) = crate::app::tests::test_model();
        let h1 = head("DP-1", true);
        let mut h2 = head("HDMI-A-1", true);
        h2.x = 1920;
        m.displays.edits = vec![baseline_edit(&h1), baseline_edit(&h2)];
        m.displays.heads = vec![h1, h2];
        m.displays.selected = Some(1);
        m.displays.dirty = true;
        m.outputs_available = true;

        m.displays.edits[1].position = Some((4000, 4000));
        republish_view(&mut m.displays);
        let dragged_scale = m.displays.view.scale;

        revert(&mut m);
        assert_eq!(m.displays.edits[1].position, Some((1920, 0)));
        assert!(!m.displays.dirty);
        assert_eq!(m.displays_status, "");
        assert!(
            m.displays.view.scale > dragged_scale,
            "a smaller layout fits at a larger scale: {} vs {dragged_scale}",
            m.displays.view.scale
        );
    }

    /// Collect every `id` set anywhere in a `View` tree. `icedtea_ui` exposes
    /// no `ids_of` (reconciliation, matching `pages::displays::controls`'s
    /// own `view_ids`: contract §4.4's id is `PropName::Id` on `Props`, and
    /// `View`'s `children` is a plain public field) — this walks the raw,
    /// unbuilt tree `footer` returns directly.
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

    /// Mutation check: drop `.sensitive(...)` from the Apply button; the
    /// footer's id set is unchanged but the interaction gate in Task 11 fails.
    /// Assert the ids here so a rename is caught early.
    #[test]
    fn the_footer_ids_are_the_contract_ids() {
        let m = dirty_model();
        let ids = view_ids(&footer(&m));
        for id in [
            "displays_status",
            "displays_test",
            "displays_revert",
            "displays_apply",
        ] {
            assert!(
                ids.iter().any(|got| got == id),
                "missing id {id}; got {ids:?}"
            );
        }
    }

    /// Mutation check: return the content branch unconditionally from `view`;
    /// the unavailable id disappears and this fails. Restore.
    #[test]
    fn an_unavailable_page_shows_only_its_explanation() {
        let (mut m, _workers) = crate::app::tests::test_model();
        m.outputs_available = false;
        let ids = view_ids(&view(&m));
        assert!(ids.iter().any(|id| id == "displays_unavailable"));
        assert!(
            !ids.iter().any(|id| id == "displays_canvas"),
            "no canvas while output management is unavailable; got {ids:?}"
        );
    }

    /// Mutation check: drop `controls::view(m)` from the content branch; the
    /// `displays_enabled` id disappears and this fails. Restore.
    #[test]
    fn an_available_page_shows_canvas_controls_and_footer() {
        let mut m = dirty_model();
        m.outputs_available = true;
        let ids = view_ids(&view(&m));
        for id in [
            "displays_canvas",
            "displays_enabled",
            "displays_resolution",
            "displays_refresh",
            "displays_scale",
            "displays_transform",
            "displays_position",
            "displays_status",
            "displays_test",
            "displays_revert",
            "displays_apply",
        ] {
            assert!(
                ids.iter().any(|got| got == id),
                "missing id {id}; got {ids:?}"
            );
        }
    }
}
