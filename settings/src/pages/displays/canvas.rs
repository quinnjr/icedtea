//! The Displays drag canvas: snapshot, scene, paint callback, pointer drags.
//!
//! The geometry is `displays_canvas`'s, called verbatim: `all_rects` gives the
//! layout-space rectangles, `compute_view` fits them into the drawing area, and
//! `View::to_canvas` maps each one. Nothing here re-derives a scale or a hit
//! test.
//!
//! The drawing area is a **fixed** `CANVAS_W` x `CANVAS_H` box (plan P4-D3).
//! The GTK page stashed the last-drawn `View` into the page state from inside
//! the draw callback; an Elm paint callback may not touch the model, so the
//! two constants are the shared source of truth instead and `update` recomputes
//! the identical `View` for the drag maths.

use crate::pages::displays::state::{self, DisplaysState};
use crate::pages::displays_canvas::{self, Rect, View};

/// The drawing area's content size, in canvas pixels — the GTK page's
/// `set_content_width(360)` / `set_content_height(240)`.
pub const CANVAS_W: f64 = 360.0;
/// See [`CANVAS_W`].
pub const CANVAS_H: f64 = 240.0;
/// The canvas label size — the GTK page's `cr.set_font_size(11.0)`.
pub const CANVAS_FONT_PX: f32 = 11.0;

/// Exactly what the paint callback reads, cheaply cloned out of the model.
///
/// The draw closure is rebuilt on every `view` and must not capture the model
/// (contract §2.8), so it captures one of these instead. Every vector is
/// parallel and indexed by *slot*, not by head index: `all_rects` already
/// collapses the head list into drawable slots.
#[derive(Clone, Debug, PartialEq)]
pub struct DisplaysSnapshot {
    /// Layout-space rectangles, one per slot.
    pub rects: Vec<Rect>,
    /// Whether each slot's head is enabled.
    pub enabled: Vec<bool>,
    /// The selected slot, if the selected head is drawable.
    pub selected: Option<usize>,
    /// Connector names, one per slot.
    pub names: Vec<String>,
    /// The second label line: `"{w}x{h}"` when enabled, `"off"` when not.
    pub details: Vec<String>,
}

impl DisplaysSnapshot {
    /// Read the paintable state out of `st`.
    #[must_use]
    pub fn of(st: &DisplaysState) -> DisplaysSnapshot {
        let (idxs, rects, enabled) = state::all_rects(st);
        let mut names = Vec::with_capacity(idxs.len());
        let mut details = Vec::with_capacity(idxs.len());
        let mut selected = None;
        for (slot, &head_idx) in idxs.iter().enumerate() {
            if st.selected == Some(head_idx) {
                selected = Some(slot);
            }
            names.push(
                st.heads
                    .get(head_idx)
                    .map_or_else(String::new, |h| h.name.clone()),
            );
            // The GTK label: the edit's own mode when it names one, else the
            // rectangle's rounded logical size. A disabled head reads `off`.
            details.push(if enabled[slot] {
                let (w, h) = st.edits.get(head_idx).and_then(|e| e.mode).map_or_else(
                    || (rects[slot].w.round() as i32, rects[slot].h.round() as i32),
                    |m| (m.width, m.height),
                );
                format!("{w}\u{d7}{h}")
            } else {
                "off".to_string()
            });
        }
        DisplaysSnapshot {
            rects,
            enabled,
            selected,
            names,
            details,
        }
    }
}

/// One monitor as the painter wants it: a canvas-space rectangle plus the four
/// flags and strings that pick its colours and labels.
#[derive(Clone, Debug, PartialEq)]
pub struct HeadTile {
    /// Canvas-space rectangle.
    pub rect: Rect,
    /// Whether the head is enabled (drives body, border and text colour).
    pub enabled: bool,
    /// Whether the head is selected (accent border).
    pub selected: bool,
    /// Connector name, the first label line.
    pub name: String,
    /// Resolution or `"off"`, the second label line.
    pub detail: String,
}

/// A whole canvas: the layout→canvas mapping the tiles were built with, kept so
/// `update` can reuse the identical `View` for the drag maths.
#[derive(Clone, Debug, PartialEq)]
pub struct CanvasScene {
    /// The mapping every tile went through.
    pub view: View,
    /// One tile per drawable head, in draw order.
    pub tiles: Vec<HeadTile>,
}

/// Fit `snap` into a `width` x `height` viewport and map every rectangle.
#[must_use]
pub fn scene(snap: &DisplaysSnapshot, width: f64, height: f64) -> CanvasScene {
    let view = displays_canvas::compute_view(&snap.rects, width, height, state::CANVAS_MARGIN);
    let tiles = snap
        .rects
        .iter()
        .enumerate()
        .map(|(slot, rect)| HeadTile {
            rect: view.to_canvas(rect),
            enabled: snap.enabled.get(slot).copied().unwrap_or(false),
            selected: snap.selected == Some(slot),
            name: snap.names.get(slot).cloned().unwrap_or_default(),
            detail: snap.details.get(slot).cloned().unwrap_or_default(),
        })
        .collect();
    CanvasScene { view, tiles }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outputs::{Head, Mode, ModeRequest};
    use crate::pages::displays::state::{DisplaysState, baseline_edit};

    /// A head with one mode, at `(x, y)`.
    fn head(name: &str, w: i32, h: i32, x: i32, y: i32, enabled: bool) -> Head {
        let mode = Mode {
            width: w,
            height: h,
            refresh_mhz: 60_000,
            preferred: true,
        };
        Head {
            name: name.to_string(),
            description: format!("{name} display"),
            enabled,
            modes: vec![mode],
            current_mode: Some(mode),
            x,
            y,
            scale: 1.0,
            transform: 0,
        }
    }

    /// A state with `heads`, baseline edits, and `selected` selected.
    fn state_of(heads: Vec<Head>, selected: Option<usize>) -> DisplaysState {
        let mut st = DisplaysState::new();
        st.edits = heads.iter().map(baseline_edit).collect();
        st.heads = heads;
        st.selected = selected;
        st
    }

    /// Mutation check: make `DisplaysSnapshot::of` read `enabled_rects`
    /// instead of `all_rects`; the disabled head disappears and this fails.
    /// Restore.
    #[test]
    fn a_snapshot_carries_every_head_including_the_disabled_one() {
        let st = state_of(
            vec![
                head("DP-1", 1920, 1080, 0, 0, true),
                head("HDMI-A-1", 1280, 720, 1920, 0, false),
            ],
            Some(1),
        );
        let snap = DisplaysSnapshot::of(&st);
        assert_eq!(snap.names, vec!["DP-1".to_string(), "HDMI-A-1".to_string()]);
        assert_eq!(snap.enabled, vec![true, false]);
        assert_eq!(snap.selected, Some(1));
        assert_eq!(
            snap.details,
            vec!["1920\u{d7}1080".to_string(), "off".to_string()],
            "a disabled head reads `off`, an enabled one its mode"
        );
        assert_eq!(snap.rects.len(), 2);
    }

    /// Mutation check: drop the `mode`-first branch of the detail text so it
    /// always reads the rect's rounded size; the 2x-scaled head then reports
    /// 960x540 and this fails. Restore.
    #[test]
    fn a_scaled_heads_detail_is_its_mode_not_its_logical_size() {
        let mut st = state_of(vec![head("DP-1", 3840, 2160, 0, 0, true)], Some(0));
        st.edits[0].scale = Some(2.0);
        st.edits[0].mode = Some(ModeRequest {
            width: 3840,
            height: 2160,
            refresh_mhz: 60_000,
        });
        let snap = DisplaysSnapshot::of(&st);
        assert_eq!(snap.details, vec!["3840\u{d7}2160".to_string()]);
        // The rect is the *logical* size: 3840/2 x 2160/2.
        assert_eq!(snap.rects[0].w, 1920.0);
        assert_eq!(snap.rects[0].h, 1080.0);
    }

    /// Mutation check: have `scene` ignore its `width`/`height` and pass
    /// `CANVAS_W`/`CANVAS_H` unconditionally; the half-size viewport then
    /// yields the same scale and this fails. Restore.
    #[test]
    fn a_scene_maps_every_tile_through_one_shared_view() {
        let st = state_of(
            vec![
                head("DP-1", 1920, 1080, 0, 0, true),
                head("HDMI-A-1", 1920, 1080, 1920, 0, true),
            ],
            Some(0),
        );
        let snap = DisplaysSnapshot::of(&st);
        let full = scene(&snap, CANVAS_W, CANVAS_H);
        let half = scene(&snap, CANVAS_W / 2.0, CANVAS_H / 2.0);
        assert!(
            half.view.scale < full.view.scale,
            "a smaller viewport must scale down further: {} vs {}",
            half.view.scale,
            full.view.scale
        );
        assert_eq!(full.tiles.len(), 2);
        // Every tile is its layout rect mapped through the scene's own view.
        for (tile, rect) in full.tiles.iter().zip(snap.rects.iter()) {
            assert_eq!(tile.rect, full.view.to_canvas(rect));
        }
        assert!(full.tiles[0].selected);
        assert!(!full.tiles[1].selected);
    }

    /// Mutation check: make `scene` mark every tile selected; this fails on
    /// the `None` case. Restore.
    #[test]
    fn a_scene_with_no_selection_marks_no_tile() {
        let st = state_of(vec![head("DP-1", 1920, 1080, 0, 0, true)], None);
        let sc = scene(&DisplaysSnapshot::of(&st), CANVAS_W, CANVAS_H);
        assert!(!sc.tiles[0].selected);
    }

    /// Mutation check: drop the empty-input guard from `scene`; `compute_view`
    /// still returns its identity view, so assert the tile list instead — make
    /// `scene` push a placeholder tile and this fails. Restore.
    #[test]
    fn an_empty_snapshot_scenes_to_no_tiles_without_panicking() {
        let sc = scene(
            &DisplaysSnapshot::of(&DisplaysState::new()),
            CANVAS_W,
            CANVAS_H,
        );
        assert!(sc.tiles.is_empty());
        assert_eq!(sc.view.scale, 1.0);
    }
}
