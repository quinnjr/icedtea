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

use std::rc::Rc;

use icedtea_ui::css::value::{FontFamily, FontStyle, GenericFamily, Keyword, LineHeight, Rgba};
use icedtea_ui::layout::Rect as UiRect;
use icedtea_ui::paint::{PaintCx, fill_paint};
use icedtea_ui::text::{Ellipsize, TextLayout, TextStyle, WrapMode};
use icedtea_ui::view::View as UiView;
use icedtea_ui::view::builders::{DrawingAreaExt, drawing_area, frame};
use skia_rs_safe::canvas::Canvas;
use skia_rs_safe::paint::Style;

use crate::app::{Msg, SettingsModel};
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

/// One opaque colour from three channels in 0..=1.
const fn rgb(r: f32, g: f32, b: f32) -> Rgba {
    Rgba { r, g, b, a: 1.0 }
}

/// The canvas backdrop.
pub const CANVAS_BG: Rgba = rgb(0.12, 0.12, 0.16);
/// An enabled monitor's body.
pub const TILE_ON: Rgba = rgb(0.26, 0.28, 0.36);
/// A disabled monitor's body, dimmed.
pub const TILE_OFF: Rgba = rgb(0.17, 0.18, 0.22);
/// The selected monitor's accent border.
pub const BORDER_SELECTED: Rgba = rgb(0.54, 0.71, 0.98);
/// An enabled monitor's border.
pub const BORDER_ON: Rgba = rgb(0.45, 0.47, 0.55);
/// A disabled monitor's border.
pub const BORDER_OFF: Rgba = rgb(0.34, 0.35, 0.42);
/// An enabled monitor's label.
pub const TEXT_ON: Rgba = rgb(0.90, 0.91, 0.95);
/// A disabled monitor's label.
pub const TEXT_OFF: Rgba = rgb(0.60, 0.61, 0.66);

/// The selected monitor's border width, in canvas pixels.
const BORDER_SELECTED_PX: f32 = 2.5;
/// Every other monitor's border width.
const BORDER_PLAIN_PX: f32 = 1.0;
/// Label inset from the tile's left edge.
const LABEL_X: f64 = 6.0;
/// The name's *baseline* offset from the tile's top edge (cairo semantics).
const NAME_BASELINE: f64 = 16.0;
/// The detail line's baseline offset.
const DETAIL_BASELINE: f64 = 30.0;

/// The face the canvas labels are shaped with.
///
/// The GTK page used cairo's toy text API with `set_font_size(11.0)` and no
/// family at all, so there is no computed style to read here; this is the
/// registry's initial font at the same size.
#[must_use]
pub fn canvas_text_style() -> TextStyle {
    TextStyle {
        families: Rc::from(vec![FontFamily::Generic(GenericFamily::SansSerif)]),
        weight: 400.0,
        style: FontStyle::Normal,
        stretch: 100.0,
        size_px: CANVAS_FONT_PX,
        letter_spacing_px: 0.0,
        features: Rc::from(Vec::new()),
        variations: Rc::from(Vec::new()),
        transform: Keyword::None,
        line_height: LineHeight::Normal,
    }
}

/// Draw one string with its cairo *baseline* at `baseline_y` (plan P4-D11:
/// `TextLayout::draw`'s origin is the layout box's top-left, cairo's
/// `move_to` is a baseline, and the conversion is one font size).
fn draw_label(
    canvas: &mut Canvas<'_>,
    cx: &mut PaintCx<'_>,
    text: &str,
    x: f64,
    baseline_y: f64,
    colour: Rgba,
) {
    if text.is_empty() {
        return;
    }
    let style = canvas_text_style();
    let layout = TextLayout::build(text, &style, cx.fonts, None, WrapMode::None, Ellipsize::End);
    layout.draw(
        canvas,
        (x as f32, baseline_y as f32 - CANVAS_FONT_PX),
        colour,
    );
}

/// The `Prop::Draw` callback for `snap`.
///
/// The cairo calls of the GTK page map one for one: `set_source_rgb`,
/// `rectangle` and `fill` become `draw_rect` with [`fill_paint`];
/// `set_line_width` and `stroke` become a `Style::Stroke` paint; `set_font_size`,
/// `move_to` and `show_text` become a [`TextLayout`] through `cx.fonts`, which
/// is why M5-D6 widened the callback to carry `&mut PaintCx`.
///
/// The closure captures `snap` by value and never the model (contract §2.8).
pub fn draw(
    snap: DisplaysSnapshot,
) -> impl Fn(&mut Canvas<'_>, UiRect, &mut PaintCx<'_>) + 'static {
    move |canvas, rect, cx| {
        // Neutral background across the whole content box.
        canvas.draw_rect(&rect.to_skia().clone(), &fill_paint(CANVAS_BG));

        let sc = scene(&snap, f64::from(rect.width), f64::from(rect.height));
        for tile in &sc.tiles {
            let r = UiRect::new(
                rect.x + tile.rect.x as f32,
                rect.y + tile.rect.y as f32,
                tile.rect.w as f32,
                tile.rect.h as f32,
            );
            // Body.
            let body = if tile.enabled { TILE_ON } else { TILE_OFF };
            canvas.draw_rect(&r.to_skia(), &fill_paint(body));
            // Border.
            let (edge, width) = if tile.selected {
                (BORDER_SELECTED, BORDER_SELECTED_PX)
            } else if tile.enabled {
                (BORDER_ON, BORDER_PLAIN_PX)
            } else {
                (BORDER_OFF, BORDER_PLAIN_PX)
            };
            let mut stroke = fill_paint(edge);
            stroke.set_style(Style::Stroke);
            stroke.set_stroke_width(width);
            canvas.draw_rect(&r.to_skia(), &stroke);
            // Labels.
            let ink = if tile.enabled { TEXT_ON } else { TEXT_OFF };
            let x = f64::from(rect.x) + tile.rect.x + LABEL_X;
            draw_label(
                canvas,
                cx,
                &tile.name,
                x,
                f64::from(rect.y) + tile.rect.y + NAME_BASELINE,
                ink,
            );
            draw_label(
                canvas,
                cx,
                &tile.detail,
                x,
                f64::from(rect.y) + tile.rect.y + DETAIL_BASELINE,
                ink,
            );
        }
    }
}

/// The canvas, framed, with its three pointer handlers.
///
/// Fixed size (plan P4-D3): `hexpand`/`vexpand` are off and the content size is
/// `CANVAS_W` x `CANVAS_H`, so `Prop::Draw`'s rectangle and the coordinates the
/// pointer handlers receive are the same box, and `update` can recompute the
/// identical `View` from the two constants.
#[must_use]
pub fn view(m: &SettingsModel) -> UiView<Msg> {
    frame(
        drawing_area(draw(DisplaysSnapshot::of(&m.displays)))
            .content_width(CANVAS_W as i32)
            .content_height(CANVAS_H as i32)
            .id("displays_canvas")
            .hexpand(false)
            .vexpand(false)
            .on_pointer_down(Msg::HeadDragBegan)
            .on_pointer_motion(Msg::HeadDragged)
            .on_pointer_up(Msg::HeadDragEnded),
    )
}

/// Begin a drag at canvas point `(x, y)`.
///
/// Recomputes the view the paint used (plan P4-D3) and publishes it into
/// `st.view` so [`dragged`]'s delta maths reads the same mapping. Hit-tests
/// against **every** head, disabled ones included, so a disabled monitor can be
/// re-selected and switched back on (state.rs finding #10). A miss clears the
/// drag and leaves the selection alone (contract §2.8).
pub fn drag_began(st: &mut DisplaysState, x: f64, y: f64) {
    let (idxs, rects, _enabled) = state::all_rects(st);
    let view = displays_canvas::compute_view(&rects, CANVAS_W, CANVAS_H, state::CANVAS_MARGIN);
    st.view = view;
    match displays_canvas::hit_test(&rects, &view, x, y) {
        Some(slot) => {
            let Some(&head) = idxs.get(slot) else {
                st.drag = None;
                return;
            };
            let start = st
                .edits
                .get(head)
                .and_then(|e| e.position)
                .unwrap_or((0, 0));
            st.selected = Some(head);
            st.drag = Some(state::Drag {
                head,
                origin: (x, y),
                start,
            });
        }
        None => st.drag = None,
    }
}

/// Continue a drag at canvas point `(x, y)`.
///
/// The canvas delta from the press point becomes a layout delta through the
/// published view; the dragged head's own rectangle (which may be a disabled
/// head's placeholder) gives the width and height `snap` needs; the snap targets
/// are the *other* enabled heads plus the origin.
pub fn dragged(st: &mut DisplaysState, x: f64, y: f64) {
    let Some(drag) = st.drag else {
        return;
    };
    // A `HeadsChanged` mid-drag replaces `heads`/`edits` and clears `drag`, but
    // guard the cached index anyway: a stale drag bails rather than indexing
    // out of bounds (state.rs finding #2).
    if drag.head >= st.edits.len() {
        st.drag = None;
        return;
    }
    let (dx, dy) = st
        .view
        .canvas_delta_to_layout(x - drag.origin.0, y - drag.origin.1);
    let (aidxs, arects, _enabled) = state::all_rects(st);
    let size = aidxs
        .iter()
        .position(|&i| i == drag.head)
        .map_or((0.0, 0.0), |slot| (arects[slot].w, arects[slot].h));
    let candidate = Rect::new(
        f64::from(drag.start.0) + dx,
        f64::from(drag.start.1) + dy,
        size.0,
        size.1,
    );
    let (eidxs, erects) = state::enabled_rects(st);
    let others: Vec<Rect> = eidxs
        .iter()
        .zip(erects.iter())
        .filter(|&(&i, _)| i != drag.head)
        .map(|(_, r)| *r)
        .collect();
    let (nx, ny) = displays_canvas::snap(candidate, &others, state::SNAP_THRESHOLD);
    st.edits[drag.head].position = Some((nx, ny));
    st.dirty = true;
}

/// End a drag at canvas point `(x, y)`: one last fold, then the drag is gone.
///
/// The release is delivered even when it lands outside the canvas (M5-D5 §3's
/// grab semantics), which is what stops a drag wedging.
pub fn drag_ended(st: &mut DisplaysState, x: f64, y: f64) {
    dragged(st, x, y);
    st.drag = None;
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

    /// Mutation check: give `canvas_text_style` a `size_px` of `0.0`; the
    /// assertion on the font size fails. Restore.
    #[test]
    fn the_canvas_text_style_is_an_11px_sans_face() {
        let style = canvas_text_style();
        assert_eq!(style.size_px, CANVAS_FONT_PX);
        assert_eq!(style.weight, 400.0);
        assert_eq!(style.letter_spacing_px, 0.0);
        assert!(!style.families.is_empty(), "a face must be named");
    }

    /// The eight literal colours of the GTK draw func, unchanged.
    ///
    /// Mutation check: change any one channel; this fails. Restore.
    #[test]
    fn the_canvas_palette_is_the_gtk_palette() {
        assert_eq!((CANVAS_BG.r, CANVAS_BG.g, CANVAS_BG.b), (0.12, 0.12, 0.16));
        assert_eq!((TILE_ON.r, TILE_ON.g, TILE_ON.b), (0.26, 0.28, 0.36));
        assert_eq!((TILE_OFF.r, TILE_OFF.g, TILE_OFF.b), (0.17, 0.18, 0.22));
        assert_eq!(
            (BORDER_SELECTED.r, BORDER_SELECTED.g, BORDER_SELECTED.b),
            (0.54, 0.71, 0.98)
        );
        assert_eq!((BORDER_ON.r, BORDER_ON.g, BORDER_ON.b), (0.45, 0.47, 0.55));
        assert_eq!(
            (BORDER_OFF.r, BORDER_OFF.g, BORDER_OFF.b),
            (0.34, 0.35, 0.42)
        );
        assert_eq!((TEXT_ON.r, TEXT_ON.g, TEXT_ON.b), (0.90, 0.91, 0.95));
        assert_eq!((TEXT_OFF.r, TEXT_OFF.g, TEXT_OFF.b), (0.60, 0.61, 0.66));
        for c in [
            CANVAS_BG,
            TILE_ON,
            TILE_OFF,
            BORDER_SELECTED,
            BORDER_ON,
            BORDER_OFF,
            TEXT_ON,
            TEXT_OFF,
        ] {
            assert_eq!(c.a, 1.0, "every canvas colour is opaque");
        }
    }

    /// Mutation check: drop the `Msg::HeadDragged` registration from
    /// `view`; the kind count falls to two and this fails. Restore.
    ///
    /// Reconciliation (Task 2): `View::children`, `View::handler_kinds` and
    /// `View::id_of` are not names `icedtea-ui` exposes. `children` is a
    /// public field (not an accessor method), there is no `handler_kinds`
    /// enumerator so each kind is checked with `Handlers::has`, and `id_of`
    /// is `Props::str(PropName::Id)`. `SettingsModel::for_test()` does not
    /// exist either; `crate::app::tests::test_model()` (P1-D8's `pub(crate)`
    /// constructor) is the equivalent the crate does expose.
    #[test]
    fn the_canvas_view_registers_all_three_pointer_kinds() {
        use icedtea_ui::view::{EventKind, PropName};

        let (m, _workers) = crate::app::tests::test_model();
        let v = view(&m);
        // `frame(drawing_area(..))` — the handlers sit on the child.
        let area = v.children.first().expect("the frame wraps the canvas");
        assert!(area.handlers.has(EventKind::PointerDown));
        assert!(area.handlers.has(EventKind::PointerMotion));
        assert!(area.handlers.has(EventKind::PointerUp));
        assert_eq!(area.props.str(PropName::Id), Some("displays_canvas"));
    }

    /// The canvas-space centre of `slot`'s tile at the fixed canvas size.
    fn tile_centre(st: &DisplaysState, slot: usize) -> (f64, f64) {
        let sc = scene(&DisplaysSnapshot::of(st), CANVAS_W, CANVAS_H);
        let r = sc.tiles[slot].rect;
        (r.x + r.w / 2.0, r.y + r.h / 2.0)
    }

    /// Contract §2.8: "on a miss clear `drag` (selection is left alone)".
    ///
    /// Mutation check: make the miss branch `st.selected = None`; this fails.
    /// Restore.
    #[test]
    fn a_drag_that_hits_nothing_leaves_the_selection_alone() {
        let mut st = state_of(vec![head("DP-1", 1920, 1080, 0, 0, true)], Some(0));
        // The canvas margin corner is guaranteed to be outside every tile.
        drag_began(&mut st, 0.0, 0.0);
        assert_eq!(st.selected, Some(0), "a miss must not clear the selection");
        assert!(st.drag.is_none(), "a miss must not start a drag");
        assert!(!st.dirty, "a miss must not dirty the page");
    }

    /// Mutation check: pass an empty `others` slice to `snap`; the head lands
    /// at its raw dragged position and this fails. Restore.
    #[test]
    fn a_drag_snaps_the_dragged_head_against_its_neighbour() {
        let mut st = state_of(
            vec![
                head("DP-1", 1920, 1080, 0, 0, true),
                head("HDMI-A-1", 1920, 1080, 4000, 0, true),
            ],
            Some(1),
        );
        let (cx, cy) = tile_centre(&st, 1);
        drag_began(&mut st, cx, cy);
        assert_eq!(st.drag.map(|d| d.head), Some(1));
        assert_eq!(st.drag.map(|d| d.start), Some((4000, 0)));

        // Drag left by exactly the canvas distance that is 2080 layout px,
        // putting the head's left edge at 1920 — the neighbour's right edge.
        let view = st.view;
        let dx_canvas = -2080.0 * view.scale;
        dragged(&mut st, cx + dx_canvas, cy);
        assert_eq!(
            st.edits[1].position,
            Some((1920, 0)),
            "the head must snap flush against DP-1's right edge"
        );
        assert!(st.dirty, "a move dirties the page");
    }

    /// Contract §2.8 / M5-D5 §3: a `PointerUp` outside the node still arrives
    /// through the implicit grab, so a drag can never wedge.
    ///
    /// Mutation check: make `drag_ended` return early without clearing
    /// `st.drag`; this fails. Restore.
    #[test]
    fn a_release_outside_the_canvas_ends_the_drag() {
        let mut st = state_of(vec![head("DP-1", 1920, 1080, 0, 0, true)], Some(0));
        let (cx, cy) = tile_centre(&st, 0);
        drag_began(&mut st, cx, cy);
        assert!(st.drag.is_some());
        // Far outside the 360x240 canvas, in both axes.
        drag_ended(&mut st, -400.0, 900.0);
        assert!(st.drag.is_none(), "the release must end the drag");
    }

    /// A drag whose head vanished mid-gesture (a hotplug clears `heads` and
    /// `edits`) must bail, not index out of bounds.
    ///
    /// Mutation check: drop the `drag.head >= st.edits.len()` guard in
    /// `dragged`; this panics instead of failing. Restore.
    #[test]
    fn a_drag_whose_head_disappeared_is_dropped_not_indexed() {
        let mut st = state_of(vec![head("DP-1", 1920, 1080, 0, 0, true)], Some(0));
        let (cx, cy) = tile_centre(&st, 0);
        drag_began(&mut st, cx, cy);
        st.heads.clear();
        st.edits.clear();
        dragged(&mut st, cx + 40.0, cy);
        assert!(st.drag.is_none(), "a stale drag must be dropped");
    }

    /// `drag_began` must publish the view the paint used, so the delta maths
    /// and the hit test agree.
    ///
    /// Mutation check: have `drag_began` compute the view with a margin of
    /// `0.0`; the scales differ and this fails. Restore.
    #[test]
    fn a_drag_publishes_the_same_view_the_paint_computed() {
        let mut st = state_of(vec![head("DP-1", 1920, 1080, 0, 0, true)], Some(0));
        let painted = scene(&DisplaysSnapshot::of(&st), CANVAS_W, CANVAS_H).view;
        let (cx, cy) = tile_centre(&st, 0);
        drag_began(&mut st, cx, cy);
        assert_eq!(st.view, painted);
    }
}
