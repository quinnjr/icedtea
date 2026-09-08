//! The GTK-free core of the Displays page: the pending-edit state the page
//! mutates, the hotplug/property-update reconciliation, and the mode/rect
//! helpers the canvas and the control panel both read.
//!
//! Moved verbatim out of the old `pages/displays.rs` (M5 contract §2.1) with
//! its twelve tests. Nothing here knows about a toolkit: `Head`/`HeadEdit`
//! come from `crate::outputs`, `Rect`/`View` from `displays_canvas`.

use crate::outputs::{Head, HeadEdit, Mode, ModeRequest};
use crate::pages::displays_canvas::{Rect, View, head_rect};

/// Canvas padding (canvas px) kept clear around the scaled monitor layout.
pub const CANVAS_MARGIN: f64 = 16.0;
/// How close (layout px) a dragged edge must come before it snaps.
pub const SNAP_THRESHOLD: f64 = 40.0;

/// The eight transform choices the UI exposes (raw `wl_output.transform`,
/// 0-7). The upper four are the flipped (mirrored) variants; exposing them all
/// means a head that arrives already flipped (raw 4-7) shows its true transform
/// and an unrelated edit does not silently drop the flip back to Normal. (#8)
pub const TRANSFORM_LABELS: [&str; 8] = [
    "Normal",
    "90\u{b0}",
    "180\u{b0}",
    "270\u{b0}",
    "Flipped",
    "Flipped 90\u{b0}",
    "Flipped 180\u{b0}",
    "Flipped 270\u{b0}",
];
pub const TRANSFORM_VALUES: [i32; 8] = [0, 1, 2, 3, 4, 5, 6, 7];

/// Everything the page mutates as the user drags and edits. `heads` is the
/// last snapshot the compositor sent; `edits` is parallel to it (same index)
/// and holds the pending change for each head.
pub struct DisplaysState {
    pub heads: Vec<Head>,
    pub edits: Vec<HeadEdit>,
    pub selected: Option<usize>,
    pub dirty: bool,
    /// The layout→canvas mapping from the last draw, reused for drag maths.
    pub view: View,
    pub drag: Option<Drag>,
    /// Option lists backing the resolution/refresh dropdowns for the selected
    /// head, so a dropdown index maps back to a concrete value.
    pub res_options: Vec<(i32, i32)>,
    pub refresh_options: Vec<i32>,
}

impl DisplaysState {
    pub fn new() -> Self {
        DisplaysState {
            heads: Vec::new(),
            edits: Vec::new(),
            selected: None,
            dirty: false,
            view: View {
                scale: 1.0,
                off_x: CANVAS_MARGIN,
                off_y: CANVAS_MARGIN,
            },
            drag: None,
            res_options: Vec::new(),
            refresh_options: Vec::new(),
        }
    }
}

impl Default for DisplaysState {
    fn default() -> Self {
        DisplaysState::new()
    }
}

/// An in-flight canvas drag. The GTK page held this implicitly inside a
/// `GestureDrag`; on the Elm loop it is model state, so P4's
/// `HeadDragBegan`/`HeadDragged`/`HeadDragEnded` arms have somewhere to put it.
///
/// `origin` is the canvas-space press point, `start` the dragged head's layout
/// position when the press landed — the drag is always computed from the press,
/// never accumulated, so a dropped motion event cannot make the head drift.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Drag {
    /// Index into `DisplaysState::heads`/`edits`.
    pub head: usize,
    pub origin: (f64, f64),
    pub start: (i32, i32),
}

/// A fresh edit mirroring a head's current state — the baseline the controls
/// mutate away from.
pub fn baseline_edit(h: &Head) -> HeadEdit {
    HeadEdit {
        name: h.name.clone(),
        enabled: h.enabled,
        mode: h.current_mode.map(|m| ModeRequest {
            width: m.width,
            height: m.height,
            refresh_mhz: m.refresh_mhz,
        }),
        position: Some((h.x, h.y)),
        scale: Some(h.scale),
        transform: Some(h.transform),
    }
}

/// A sensible mode to adopt when a head gains a concrete mode but the edit
/// names none: the head's current mode, else its preferred advertised mode,
/// else the first mode it advertises. `None` only for a head that advertises no
/// modes at all (a headless/nested output, where the compositor picks). Used to
/// give a mode-less head a `ModeRequest` the moment it is enabled or the user
/// picks a refresh, so it becomes placeable on the canvas and those edits
/// actually take effect. (#9, #14)
pub fn default_mode_for(head: &Head) -> Option<ModeRequest> {
    head.current_mode
        .or_else(|| head.modes.iter().find(|m| m.preferred).copied())
        .or_else(|| head.modes.first().copied())
        .map(|m| ModeRequest {
            width: m.width,
            height: m.height,
            refresh_mhz: m.refresh_mhz,
        })
}

/// Whether two head snapshots describe the same set of connectors (same names,
/// order-independent). When this holds across a `HeadsChanged`, the change is a
/// property update on the same monitors (a mode list refresh, an unrelated
/// client's `done`, ...) rather than a hotplug, so the user's pending edits and
/// selection can be carried over instead of reset.
pub fn same_connector_set(a: &[Head], b: &[Head]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut an: Vec<&str> = a.iter().map(|h| h.name.as_str()).collect();
    let mut bn: Vec<&str> = b.iter().map(|h| h.name.as_str()).collect();
    an.sort_unstable();
    bn.sort_unstable();
    an == bn
}

/// The result of reconciling a fresh head snapshot against the page's cached
/// heads + pending edits.
pub struct Reconciled {
    /// Edits parallel to the new head snapshot.
    pub edits: Vec<HeadEdit>,
    /// Selection index into the new snapshot.
    pub selected: Option<usize>,
    /// Whether the connector set was unchanged (edits/selection preserved).
    pub compatible: bool,
    /// Whether pending, unapplied edits had to be discarded because the set
    /// changed — used to surface a brief status to the user.
    pub dropped: bool,
}

/// Reconcile a `HeadsChanged` snapshot against the current state.
///
/// When the connector set is unchanged the caller's pending edits are carried
/// over (matched by connector name, since `edits`/`old_heads` are index-
/// parallel but the new snapshot may reorder), and the selected connector is
/// preserved by name. When the set actually changed (a hotplug/unplug) every
/// head falls back to its baseline and selection resets, and `dropped` reports
/// whether that discarded any unapplied work (`dirty`).
pub fn reconcile(
    old_heads: &[Head],
    old_edits: &[HeadEdit],
    old_selected: Option<usize>,
    dirty: bool,
    new_heads: &[Head],
) -> Reconciled {
    let compatible = same_connector_set(old_heads, new_heads);
    let selected_name = old_selected
        .and_then(|i| old_heads.get(i))
        .map(|h| h.name.as_str());
    let selected = selected_name
        .and_then(|name| new_heads.iter().position(|h| h.name == name))
        .or(if new_heads.is_empty() { None } else { Some(0) });

    if compatible {
        // Carry a pending edit forward ONLY for heads the user actually touched
        // (whose cached edit diverges from the baseline of its cached head).
        // Untouched heads re-baseline from the fresh snapshot, so an external
        // change to another head is reflected here rather than being reverted on
        // the next Apply. (#11)
        let edits = new_heads
            .iter()
            .map(|h| {
                let touched = old_edits
                    .iter()
                    .zip(old_heads.iter())
                    .find(|(e, oh)| e.name == h.name && oh.name == h.name)
                    .filter(|(e, oh)| **e != baseline_edit(oh));
                match touched {
                    Some((e, _)) => e.clone(),
                    None => baseline_edit(h),
                }
            })
            .collect();
        Reconciled {
            edits,
            selected,
            compatible,
            dropped: false,
        }
    } else {
        let edits = new_heads.iter().map(baseline_edit).collect();
        Reconciled {
            edits,
            selected,
            compatible,
            dropped: dirty,
        }
    }
}

/// Distinct `(w, h)` resolutions a head advertises, largest area first.
pub fn distinct_resolutions(modes: &[Mode]) -> Vec<(i32, i32)> {
    let mut out: Vec<(i32, i32)> = Vec::new();
    for m in modes {
        if !out.contains(&(m.width, m.height)) {
            out.push((m.width, m.height));
        }
    }
    out.sort_by_key(|&(w, h)| std::cmp::Reverse(w as i64 * h as i64));
    out
}

/// Refresh rates (mHz) a head offers at a given resolution, highest first.
pub fn refreshes_for(modes: &[Mode], w: i32, h: i32) -> Vec<i32> {
    let mut out: Vec<i32> = modes
        .iter()
        .filter(|m| m.width == w && m.height == h)
        .map(|m| m.refresh_mhz)
        .collect();
    out.sort_unstable_by(|a, b| b.cmp(a));
    out.dedup();
    out
}

pub fn format_refresh(mhz: i32) -> String {
    format!("{:.2} Hz", mhz as f64 / 1000.0)
}

/// The enabled heads' layout rects, and the head indices they map back to.
pub fn enabled_rects(st: &DisplaysState) -> (Vec<usize>, Vec<Rect>) {
    let mut idxs = Vec::new();
    let mut rects = Vec::new();
    for (i, e) in st.edits.iter().enumerate() {
        if !e.enabled {
            continue;
        }
        let (w, h) = e
            .mode
            .map(|m| (m.width, m.height))
            .or_else(|| {
                st.heads
                    .get(i)
                    .and_then(|hd| hd.current_mode)
                    .map(|m| (m.width, m.height))
            })
            .unwrap_or((0, 0));
        if w == 0 || h == 0 {
            continue;
        }
        let scale = e.scale.unwrap_or(1.0);
        let transform = e.transform.unwrap_or(0);
        let (x, y) = e.position.unwrap_or((0, 0));
        idxs.push(i);
        rects.push(head_rect(w, h, scale, transform, x, y));
    }
    (idxs, rects)
}

/// Every head's layout rect for drawing and hit-testing, tagged with whether
/// the head is enabled. Unlike [`enabled_rects`] this includes DISABLED heads
/// (and mode-less ones), sized from the edit's mode, else the head's current or
/// first advertised mode, else a 1920×1080 fallback, so a disabled monitor
/// stays visible on the canvas and can be re-selected to switch it back on. The
/// caller greys out entries whose flag is `false`. (#10)
pub fn all_rects(st: &DisplaysState) -> (Vec<usize>, Vec<Rect>, Vec<bool>) {
    let mut idxs = Vec::new();
    let mut rects = Vec::new();
    let mut enabled = Vec::new();
    for (i, e) in st.edits.iter().enumerate() {
        let head = st.heads.get(i);
        let (mut w, mut h) = e
            .mode
            .map(|m| (m.width, m.height))
            .or_else(|| head.and_then(|hd| hd.current_mode.map(|m| (m.width, m.height))))
            .or_else(|| head.and_then(|hd| hd.modes.first().map(|m| (m.width, m.height))))
            .unwrap_or((0, 0));
        if w == 0 || h == 0 {
            // No mode information at all — still draw a placeholder so the head
            // is selectable.
            w = 1920;
            h = 1080;
        }
        let scale = e.scale.unwrap_or(1.0);
        let transform = e.transform.unwrap_or(0);
        let (x, y) = e.position.unwrap_or((0, 0));
        idxs.push(i);
        rects.push(head_rect(w, h, scale, transform, x, y));
        enabled.push(e.enabled);
    }
    (idxs, rects, enabled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pages::displays::fixtures::mode;

    /// A single-mode `Head` named `name`, enabled at the origin — the common
    /// shape this module's reconcile/`same_connector_set` tests want.
    fn head(name: &str) -> Head {
        crate::pages::displays::fixtures::head(name, 1920, 1080, 0, 0, true)
    }

    #[test]
    fn distinct_resolutions_dedups_and_orders_by_area() {
        let modes = [
            mode(1920, 1080, 60000, true),
            mode(1920, 1080, 59940, false),
            mode(3840, 2160, 30000, false),
            mode(1280, 720, 60000, false),
        ];
        let res = distinct_resolutions(&modes);
        assert_eq!(res, vec![(3840, 2160), (1920, 1080), (1280, 720)]);
    }

    #[test]
    fn refreshes_for_filters_by_resolution_highest_first() {
        let modes = [
            mode(1920, 1080, 60000, true),
            mode(1920, 1080, 59940, false),
            mode(1920, 1080, 60000, false),
            mode(3840, 2160, 30000, false),
        ];
        // Only the 1080p refreshes, deduped, highest first.
        assert_eq!(refreshes_for(&modes, 1920, 1080), vec![60000, 59940]);
        assert_eq!(refreshes_for(&modes, 3840, 2160), vec![30000]);
        assert_eq!(refreshes_for(&modes, 800, 600), Vec::<i32>::new());
    }

    #[test]
    fn baseline_edit_mirrors_head() {
        let head = Head {
            name: "DP-1".into(),
            description: "Test".into(),
            enabled: true,
            modes: vec![mode(1920, 1080, 60000, true)],
            current_mode: Some(mode(1920, 1080, 60000, true)),
            x: 10,
            y: 20,
            scale: 1.5,
            transform: 2,
        };
        let e = baseline_edit(&head);
        assert_eq!(e.name, "DP-1");
        assert!(e.enabled);
        assert_eq!(
            e.mode,
            Some(ModeRequest {
                width: 1920,
                height: 1080,
                refresh_mhz: 60000
            })
        );
        assert_eq!(e.position, Some((10, 20)));
        assert_eq!(e.scale, Some(1.5));
        assert_eq!(e.transform, Some(2));
    }

    #[test]
    fn same_connector_set_is_order_independent() {
        let a = [head("DP-1"), head("HDMI-A-1")];
        let b = [head("HDMI-A-1"), head("DP-1")];
        assert!(same_connector_set(&a, &b));
        assert!(!same_connector_set(&a, &[head("DP-1")]));
        assert!(!same_connector_set(&a, &[head("DP-1"), head("DP-2")]));
    }

    #[test]
    fn reconcile_preserves_edits_when_the_set_is_unchanged() {
        // Two heads with a pending, unapplied edit on the second.
        let old_heads = vec![head("DP-1"), head("HDMI-A-1")];
        let mut edits: Vec<HeadEdit> = old_heads.iter().map(baseline_edit).collect();
        edits[1].position = Some((500, 600)); // user dragged HDMI-A-1
        edits[1].enabled = false; // and disabled it

        // A HeadsChanged for the SAME connectors, but reordered and with a
        // refreshed mode list (a property update, not a hotplug).
        let mut hdmi = head("HDMI-A-1");
        hdmi.modes.push(mode(1280, 720, 60000, false));
        let new_heads = vec![hdmi, head("DP-1")];

        let r = reconcile(&old_heads, &edits, Some(1), true, &new_heads);
        assert!(r.compatible);
        assert!(!r.dropped);
        // Selection followed HDMI-A-1 to its new index 0.
        assert_eq!(r.selected, Some(0));
        // The pending edit for HDMI-A-1 survived, matched by name to new idx 0.
        assert_eq!(r.edits[0].name, "HDMI-A-1");
        assert_eq!(r.edits[0].position, Some((500, 600)));
        assert!(!r.edits[0].enabled);
    }

    #[test]
    fn reconcile_resets_and_reports_dropped_on_a_set_change() {
        let old_heads = vec![head("DP-1"), head("HDMI-A-1")];
        let mut edits: Vec<HeadEdit> = old_heads.iter().map(baseline_edit).collect();
        edits[0].position = Some((123, 456)); // unapplied work

        // HDMI-A-1 was unplugged: the connector set changed.
        let new_heads = vec![head("DP-1")];

        let r = reconcile(&old_heads, &edits, Some(1), true, &new_heads);
        assert!(!r.compatible);
        assert!(
            r.dropped,
            "unapplied edits were discarded, so dropped must be set"
        );
        // Everything falls back to baseline for the surviving head.
        assert_eq!(r.edits, vec![baseline_edit(&head("DP-1"))]);
        // Selection was on the now-gone head, so it resets to the first head.
        assert_eq!(r.selected, Some(0));
    }

    #[test]
    fn reconcile_rebaselines_untouched_heads_from_external_changes() {
        // Two heads; the user touched only DP-1. HDMI-A-1 is left untouched.
        let old_heads = vec![head("DP-1"), head("HDMI-A-1")];
        let mut edits: Vec<HeadEdit> = old_heads.iter().map(baseline_edit).collect();
        edits[0].position = Some((100, 200)); // user dragged DP-1 (touched)

        // A compatible HeadsChanged where an *external* actor moved and rescaled
        // HDMI-A-1 (the head the user never touched).
        let mut hdmi = head("HDMI-A-1");
        hdmi.x = 4321;
        hdmi.y = 8765;
        hdmi.scale = 2.0;
        let new_heads = vec![head("DP-1"), hdmi.clone()];

        let r = reconcile(&old_heads, &edits, Some(0), true, &new_heads);
        assert!(r.compatible);
        assert!(!r.dropped);
        // DP-1 was touched: its pending edit is preserved, NOT reverted.
        assert_eq!(r.edits[0].name, "DP-1");
        assert_eq!(r.edits[0].position, Some((100, 200)));
        // HDMI-A-1 was untouched: it re-baselines to the fresh external state
        // instead of carrying the stale cached baseline forward. (#11)
        assert_eq!(r.edits[1], baseline_edit(&hdmi));
        assert_eq!(r.edits[1].position, Some((4321, 8765)));
        assert_eq!(r.edits[1].scale, Some(2.0));
    }

    #[test]
    fn default_mode_for_prefers_current_then_preferred_then_first() {
        // current_mode wins.
        let mut h = head("DP-1");
        h.modes = vec![mode(1920, 1080, 60000, true), mode(1280, 720, 60000, false)];
        h.current_mode = Some(mode(1280, 720, 60000, false));
        assert_eq!(
            default_mode_for(&h),
            Some(ModeRequest {
                width: 1280,
                height: 720,
                refresh_mhz: 60000
            })
        );

        // No current_mode: the preferred advertised mode wins over the first.
        let mut h = head("DP-1");
        h.modes = vec![mode(1280, 720, 60000, false), mode(1920, 1080, 60000, true)];
        h.current_mode = None;
        assert_eq!(
            default_mode_for(&h),
            Some(ModeRequest {
                width: 1920,
                height: 1080,
                refresh_mhz: 60000
            })
        );

        // No current and none preferred: the first advertised mode.
        let mut h = head("DP-1");
        h.modes = vec![
            mode(1600, 900, 60000, false),
            mode(1920, 1080, 60000, false),
        ];
        h.current_mode = None;
        assert_eq!(
            default_mode_for(&h),
            Some(ModeRequest {
                width: 1600,
                height: 900,
                refresh_mhz: 60000
            })
        );

        // A truly mode-less head (headless/nested) yields None.
        let mut h = head("HEADLESS-1");
        h.modes = vec![];
        h.current_mode = None;
        assert_eq!(default_mode_for(&h), None);
    }

    #[test]
    fn all_rects_includes_disabled_heads_for_selection() {
        // A disabled head must still produce a rect so it stays visible and
        // selectable on the canvas. (#10)
        let mut st = DisplaysState::new();
        st.heads = vec![head("DP-1"), head("HDMI-A-1")];
        st.edits = st.heads.iter().map(baseline_edit).collect();
        st.edits[1].enabled = false;

        let (idxs, rects, enabled) = all_rects(&st);
        assert_eq!(idxs, vec![0, 1]);
        assert_eq!(rects.len(), 2);
        assert_eq!(enabled, vec![true, false]);
        // enabled_rects, by contrast, drops the disabled head.
        let (en_idxs, _) = enabled_rects(&st);
        assert_eq!(en_idxs, vec![0]);
    }

    #[test]
    fn all_rects_sizes_a_mode_less_head_with_a_fallback() {
        // A head with no mode info at all still gets a non-zero placeholder rect
        // so it can be hit-tested and selected.
        let mut st = DisplaysState::new();
        let mut h = head("HEADLESS-1");
        h.modes = vec![];
        h.current_mode = None;
        st.heads = vec![h];
        st.edits = st.heads.iter().map(baseline_edit).collect();

        let (idxs, rects, _enabled) = all_rects(&st);
        assert_eq!(idxs, vec![0]);
        assert!(
            rects[0].w > 0.0 && rects[0].h > 0.0,
            "fallback rect must be non-empty"
        );
    }

    #[test]
    fn transform_dropdown_exposes_all_eight_variants() {
        // Every raw wl_output.transform value 0-7 must be selectable and
        // round-trip, so a flipped head (4-7) is neither mislabeled Normal nor
        // silently un-flipped. (#8)
        assert_eq!(TRANSFORM_VALUES.len(), 8);
        assert_eq!(TRANSFORM_LABELS.len(), 8);
        for t in 0..8 {
            let sel = TRANSFORM_VALUES.iter().position(|&v| v == t);
            assert_eq!(
                sel,
                Some(t as usize),
                "transform {t} must be present at its index"
            );
        }
    }

    #[test]
    fn reconcile_set_change_without_pending_edits_is_not_dropped() {
        let old_heads = vec![head("DP-1")];
        let edits: Vec<HeadEdit> = old_heads.iter().map(baseline_edit).collect();
        // A clean hotplug (no unapplied work) must not claim edits were dropped.
        let new_heads = vec![head("DP-1"), head("DP-2")];
        let r = reconcile(&old_heads, &edits, Some(0), false, &new_heads);
        assert!(!r.compatible);
        assert!(!r.dropped);
        assert_eq!(r.edits.len(), 2);
    }
}

#[cfg(test)]
mod move_proof {
    /// The twelve module tests of the old `pages/displays.rs` now live here,
    /// and every item they exercise is reachable at this path.
    ///
    /// Mutation check: rename `state::reconcile` to `state::reconcile2`; this
    /// test stops compiling. Restore.
    #[test]
    fn the_pure_core_is_reachable_at_pages_displays_state() {
        let heads: Vec<crate::outputs::Head> = Vec::new();
        let r = super::reconcile(&heads, &[], None, false, &heads);
        assert!(r.edits.is_empty());
        assert_eq!(r.selected, None);
        assert!(r.compatible, "an empty-to-empty set change is compatible");
        assert!(!r.dropped);
        assert_eq!(super::CANVAS_MARGIN, 16.0);
        assert_eq!(super::SNAP_THRESHOLD, 40.0);
        assert_eq!(super::TRANSFORM_VALUES, [0, 1, 2, 3, 4, 5, 6, 7]);
    }
}
