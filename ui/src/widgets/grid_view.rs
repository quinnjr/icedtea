//! `GtkGridView` -- a `gridview` of recycled `child` cells laid out in a
//! wrapping grid, plus a click-drag rubberband selection.
//!
//! ```text
//! gridview
//! ├── child[.activatable]
//! ┊
//! ╰── [rubberband]
//! ```
//!
//! Reconciliation: this is `list_view::ListViewC`'s own pooling controller
//! turned two-dimensional, not a literal wrapper around it -- `ListViewC`'s
//! fields are private and its methods take `&(mut) self` typed as
//! `ListViewC`, so there is no seam to embed it through for a different cell
//! shape. Every piece below mirrors `ListViewC`'s own naming and logic
//! one-for-one (`OVERSCAN`, `visible_range`, `rebind`, `slot_at`,
//! `select_band`, `apply_selection`, `feed_kinetic`, `max_offset`, and the
//! `set_metrics`/`pool_len`/`pool_ids`/`select`/`selected`/`scroll_to`
//! type-erased test hooks), adapted from one row height to a `(width,
//! height)` cell and from a row index to a column count derived the same way
//! `FlowBoxC::columns_for` derives one -- clamped to `[min_columns,
//! max_columns]`, floor-dividing the viewport width by the cell width, never
//! zero. The cell node is `child`, not `row`: GTK's `GtkGridView` recycles
//! `GtkListItemWidget`s under a bare `child` CSS name, unlike
//! `GtkListView`'s `row`.

use std::rc::Rc;
use std::time::Duration;

use crate::css::node::{Node, PseudoStates};
use crate::layout::{Container, LayoutTree, Rect};
use crate::view::controller::{Controller, Event, EventCx};
use crate::view::{BuildCx, EventKind, Kind, ListItem, Prop, PropName, Props};
use crate::widgets::types::{ItemFactory, Selection, SelectionMode};
use crate::widgets::{Universal, local_rect, prop_bool, prop_u16, set_container};
use crate::window::pointer::{Kinetic, Scroll, ScrollSource};

/// Cells kept beyond the viewport, above and below (in whole rows), so a
/// one-pixel scroll never has to allocate. `list_view::OVERSCAN`'s own
/// value, over rows instead of individual rows-of-one.
const OVERSCAN_ROWS: usize = 2;

/// `GtkGridView`.
pub struct GridViewC {
    /// The `gridview` node itself, for hit-testing and for attaching/
    /// detaching pooled cells and the rubberband node.
    node: Node,
    /// The full model. Only the visible range (plus overscan) ever gets a
    /// `Node`.
    pub model: Rc<[ListItem]>,
    /// One `child` `Node` per pooled slot, always a whole multiple of
    /// `columns` (once real metrics are known).
    pub pool: Vec<Node>,
    /// The model index `pool[0]` currently shows.
    pub first_visible: usize,
    /// Scroll offset, px.
    pub offset: f32,
    /// `(width, height)` of one cell, px. `<= 0.0` height (its `build`-time
    /// default) means real metrics are not known yet.
    pub cell: (f32, f32),
    /// How many cells fit per row, derived from the last-known viewport
    /// width, `min_columns` and `max_columns`.
    pub columns: usize,
    /// What is selected, by model index.
    pub selection: Selection,
    /// Keyboard cursor, a model index.
    pub cursor: Option<usize>,
    /// Deceleration state for a flick.
    pub kinetic: Kinetic,
    /// The band's rect (in this controller's own root-node space) and its
    /// `rubberband` node, while a drag is in progress.
    pub rubberband: Option<(Rect, Node)>,
    /// Maps a model index and item to what its cell shows.
    factory: ItemFactory,
    /// The last viewport extent [`GridViewC::rebind`] was called with.
    viewport: (f32, f32),
    /// `GtkGridView:min-columns`, never zero.
    min_columns: u32,
    /// `GtkGridView:max-columns`, never below `min_columns`.
    max_columns: u32,
    /// `GtkGridView:single-click-activate`.
    single_click_activate: bool,
    /// `GtkGridView:enable-rubberband`.
    rubberband_enabled: bool,
    /// The point of the still-down `PointerDown` that started this
    /// press/drag, in root-local space; `None` between gestures.
    press_origin: Option<(f32, f32)>,
    /// The pool slot, if any, the press landed on.
    press_hit: Option<usize>,
    /// Something changed this frame the screen has not caught up with -- the
    /// pool grew, the metrics moved, the offset moved -- so one more frame is
    /// owed. See [`Controller::next_deadline`]'s impl below, and
    /// `list_view::ListViewC`'s own pair of flags.
    dirty: bool,
    /// Whether a flick is still coasting, mirrored out of `kinetic` (which
    /// keeps that flag private).
    kinetic_active: bool,
    universal: Universal,
}

impl GridViewC {
    /// How many cells fit per row at `width`, clamped to `[min_columns,
    /// max_columns]` and never zero -- `FlowBoxC::columns_for`'s own rule,
    /// over a cell width instead of a child measure.
    #[must_use]
    fn columns_for(&self, width: f32) -> usize {
        let fit = if self.cell.0 > 0.0 && width.is_finite() && self.cell.0.is_finite() {
            (width / self.cell.0).floor().max(1.0) as usize
        } else {
            self.min_columns as usize
        };
        fit.clamp(
            (self.min_columns.max(1)) as usize,
            (self.max_columns.max(self.min_columns).max(1)) as usize,
        )
    }

    /// Which model indices the viewport covers, given the current offset --
    /// `list_view::ListViewC::visible_range`'s own logic, over rows of
    /// `columns` cells rather than individual rows.
    #[must_use]
    pub fn visible_range(&self, viewport: (f32, f32)) -> std::ops::Range<usize> {
        if self.cell.1 <= 0.0 || !self.cell.1.is_finite() || self.model.is_empty() {
            return 0..0;
        }
        let columns = self.columns_for(viewport.0).max(1);
        let first_row = (self.offset / self.cell.1).floor().max(0.0) as usize;
        let rows = (viewport.1 / self.cell.1).ceil().max(0.0) as usize + 1;
        let start = (first_row * columns).min(self.model.len());
        let end_row = first_row + rows + OVERSCAN_ROWS;
        let end = (end_row * columns).min(self.model.len());
        start..end.max(start)
    }

    /// Grow or shrink the pool to the visible range and rebind every cell --
    /// `list_view::ListViewC::rebind`'s own logic and the same recycling
    /// guarantee: a rebind is `set_text`/`set_row_classes`/a `:selected` flip
    /// on a node already in the tree, never an insert or a remove.
    pub fn rebind(&mut self, viewport: (f32, f32)) {
        if self.cell.1 <= 0.0 || !self.cell.1.is_finite() {
            return;
        }
        // The pool is about to grow, shrink or rebind: one more frame is owed.
        self.dirty = true;
        self.columns = self.columns_for(viewport.0).max(1);
        self.rewrite_grid();
        let range = self.visible_range(viewport);
        while self.pool.len() < range.len() {
            let cell = Node::with_classes("child", &["activatable"]);
            self.node.append_child(&cell);
            self.pool.push(cell);
        }
        while self.pool.len() > range.len() {
            if let Some(cell) = self.pool.pop() {
                self.node.remove_child(&cell);
            }
        }
        self.first_visible = range.start;
        for (slot, index) in range.clone().enumerate() {
            let Some(item) = self.model.get(index) else {
                continue;
            };
            let content = self.factory.bind(index, item);
            let cell = &self.pool[slot];
            crate::widgets::set_text(cell, &content.label);
            crate::widgets::set_row_classes(cell, &content.classes);
            cell.set_state(PseudoStates::SELECTED, self.selection.contains(index));
            crate::widgets::set_row_index(cell, index);
        }
    }

    /// Re-derive the node's `Container::Grid` from `columns` -- `FlowBoxC::
    /// rewrite_grid`'s own trade: not resize-aware, since props are not
    /// layout-aware in this crate, but it does track the last-known
    /// viewport width `rebind` was called with.
    fn rewrite_grid(&self) {
        set_container(
            &self.node,
            Container::Grid {
                columns: u16::try_from(self.columns.clamp(1, 1024)).unwrap_or(1),
                rows: 1,
                column_spacing: 0.0,
                row_spacing: 0.0,
                column_homogeneous: true,
                row_homogeneous: true,
            },
        );
    }

    /// The pool slot a point (in this controller's own root-node space)
    /// lands in, if any.
    fn slot_at(&self, tree: &LayoutTree, point: (f32, f32)) -> Option<usize> {
        self.pool.iter().position(|cell| {
            local_rect(tree, &self.node, cell).is_some_and(|r| {
                point.0 >= r.x && point.0 < r.right() && point.1 >= r.y && point.1 < r.bottom()
            })
        })
    }

    /// Select every pooled cell whose allocation intersects `band`, mapped
    /// to its bound *model* index, without deselecting anything already
    /// selected.
    fn select_band(&mut self, tree: &LayoutTree, band: Rect) -> bool {
        let mut changed = false;
        for cell in &self.pool {
            let hit = local_rect(tree, &self.node, cell).is_some_and(|r| r.intersects(&band));
            if hit && let Some(index) = crate::widgets::row_index_of(cell) {
                changed |= self.selection.toggle_on(index);
            }
        }
        changed
    }

    /// Push the selection onto the pooled cells' `:selected` state.
    pub fn apply_selection(&self) {
        for cell in &self.pool {
            let index = crate::widgets::row_index_of(cell).unwrap_or(usize::MAX);
            cell.set_state(PseudoStates::SELECTED, self.selection.contains(index));
        }
    }

    /// Feed a `Scroll` frame into `self.kinetic` --
    /// `list_view::ListViewC::feed_kinetic`'s own logic, verbatim.
    fn feed_kinetic(&mut self, scroll: &Scroll, now: Duration) {
        match scroll.source {
            ScrollSource::Finger if scroll.stop => {
                let seed = Scroll {
                    stop: false,
                    ..*scroll
                };
                self.kinetic.feed(&seed, now);
                self.kinetic.feed(scroll, now + Duration::from_millis(16));
            }
            ScrollSource::Finger => self.kinetic.feed(scroll, now),
            _ => self.kinetic.cancel(),
        }
    }

    /// The maximum scroll offset, never negative.
    fn max_offset(&self, viewport: f32) -> f32 {
        if self.cell.1.is_finite() && self.cell.1 > 0.0 && self.columns > 0 && viewport.is_finite()
        {
            let rows = self.model.len().div_ceil(self.columns) as f32;
            (rows * self.cell.1 - viewport).max(0.0)
        } else {
            0.0
        }
    }

    // -- Test/headless hooks, one-line accessors over a type-erased
    // controller (`list_view::ListViewC`'s own pattern). --

    /// Establish real cell-size/viewport metrics and rebind the pool to them.
    pub fn set_metrics<Msg: 'static>(
        controller: &mut dyn Controller<Msg>,
        cell: (f32, f32),
        viewport: (f32, f32),
    ) {
        let any: &mut dyn std::any::Any = controller;
        if let Some(me) = any.downcast_mut::<Self>() {
            me.cell = cell;
            me.viewport = viewport;
            me.rebind(viewport);
        }
    }

    /// The pool's current size.
    #[must_use]
    pub fn pool_len<Msg: 'static>(controller: &dyn Controller<Msg>) -> usize {
        let any: &dyn std::any::Any = controller;
        any.downcast_ref::<Self>().map_or(0, |me| me.pool.len())
    }

    /// A stable per-node identity for each pooled cell, in slot order.
    #[must_use]
    pub fn pool_ids<Msg: 'static>(controller: &dyn Controller<Msg>) -> Vec<usize> {
        let any: &dyn std::any::Any = controller;
        any.downcast_ref::<Self>()
            .map(|me| me.pool.iter().map(Node::addr).collect())
            .unwrap_or_default()
    }

    /// Select a model index directly, bypassing pointer/keyboard input.
    pub fn select<Msg: 'static>(controller: &mut dyn Controller<Msg>, index: usize) {
        let any: &mut dyn std::any::Any = controller;
        if let Some(me) = any.downcast_mut::<Self>() {
            me.selection.select(index);
            me.apply_selection();
        }
    }

    /// The selected model indices.
    #[must_use]
    pub fn selected<Msg: 'static>(controller: &dyn Controller<Msg>) -> Vec<usize> {
        let any: &dyn std::any::Any = controller;
        any.downcast_ref::<Self>()
            .map_or_else(Vec::new, |me| me.selection.selected())
    }

    /// Jump the scroll offset directly and rebind at the last-known
    /// viewport.
    pub fn scroll_to<Msg: 'static>(controller: &mut dyn Controller<Msg>, offset: f32) {
        let any: &mut dyn std::any::Any = controller;
        if let Some(me) = any.downcast_mut::<Self>() {
            let viewport = me.viewport;
            me.offset = offset.clamp(0.0, me.max_offset(viewport.1));
            me.rebind(viewport);
        }
    }
}

/// The rect spanning two points, normalised so width/height are never
/// negative -- `ListViewC`'s/`FlowBoxC`'s own `band_of`.
fn band_of(a: (f32, f32), b: (f32, f32)) -> Rect {
    let x = a.0.min(b.0);
    let y = a.1.min(b.1);
    Rect::new(x, y, (a.0 - b.0).abs(), (a.1 - b.1).abs())
}

impl<Msg: Clone + 'static> Controller<Msg> for GridViewC {
    fn kind(&self) -> Kind {
        Kind::GridView
    }

    fn build(node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        let mode = SelectionMode::from_u16(
            u16::try_from(props.int(PropName::SelectionMode, 1)).unwrap_or(1),
        );
        let min_columns = u32::try_from(props.int(PropName::MinColumns, 1).max(0))
            .unwrap_or(1)
            .max(1);
        let max_columns = u32::try_from(props.int(PropName::MaxColumns, 7).max(0))
            .unwrap_or(7)
            .max(min_columns);
        let model: Rc<[ListItem]> = match props.get(PropName::Model) {
            Some(Prop::Items(items)) => Rc::clone(items),
            _ => Rc::from(&[][..]),
        };
        let factory = match props.get(PropName::ItemFactory) {
            Some(Prop::Factory(factory)) => factory.clone(),
            _ => ItemFactory::label_only(),
        };
        let mut me = Self {
            node: node.clone(),
            model: Rc::clone(&model),
            pool: Vec::new(),
            first_visible: 0,
            offset: 0.0,
            cell: (0.0, 0.0),
            columns: min_columns.max(1) as usize,
            selection: Selection::new(mode),
            cursor: None,
            kinetic: Kinetic::default(),
            rubberband: None,
            factory,
            viewport: (0.0, 0.0),
            min_columns,
            max_columns,
            single_click_activate: props.bool(PropName::SingleClickActivate, false),
            rubberband_enabled: props.bool(PropName::EnableRubberband, false),
            press_origin: None,
            press_hit: None,
            dirty: true,
            kinetic_active: false,
            universal: Universal::new(node, Kind::GridView),
        };
        me.rewrite_grid();
        // `rebind` is a no-op before real metrics exist (see `ListViewC`'s
        // own module doc for why), so the fixture's one required `child`
        // line, over `build_widget`'s bare controller, can only come from a
        // placeholder built here directly. Dead code for every real widget
        // instance, which gets real metrics from its first layout pass long
        // before an application can observe the pool.
        if let Some(item) = me.model.first() {
            let cell = Node::with_classes("child", &["activatable"]);
            me.node.append_child(&cell);
            let content = me.factory.bind(0, item);
            crate::widgets::set_text(&cell, &content.label);
            crate::widgets::set_row_classes(&cell, &content.classes);
            crate::widgets::set_row_index(&cell, 0);
            me.pool.push(cell);
        }
        me
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        match name {
            PropName::Model => {
                self.model = match value {
                    Prop::Items(items) => Rc::clone(items),
                    _ => Rc::from(&[][..]),
                };
                self.selection.retain_below(self.model.len());
                self.rebind(self.viewport);
            }
            PropName::ItemFactory => {
                self.factory = match value {
                    Prop::Factory(factory) => factory.clone(),
                    _ => ItemFactory::label_only(),
                };
                self.rebind(self.viewport);
            }
            PropName::SelectionMode => {
                self.selection
                    .set_mode(SelectionMode::from_u16(prop_u16(value, 1)));
                self.apply_selection();
            }
            PropName::MinColumns => {
                self.min_columns = u32::try_from(crate::widgets::prop_i64(value, 1).max(0))
                    .unwrap_or(1)
                    .max(1);
                self.max_columns = self.max_columns.max(self.min_columns);
                self.rebind(self.viewport);
            }
            PropName::MaxColumns => {
                self.max_columns = u32::try_from(crate::widgets::prop_i64(value, 7).max(0))
                    .unwrap_or(7)
                    .max(self.min_columns);
                self.rebind(self.viewport);
            }
            PropName::SingleClickActivate => {
                self.single_click_activate = prop_bool(value, false);
            }
            PropName::EnableRubberband => self.rubberband_enabled = prop_bool(value, false),
            other => {
                self.universal.apply(node, Kind::GridView, other, value);
            }
        }
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        use crate::window::keyboard::Mods;
        use xkbcommon::xkb::keysyms;

        let mut out = Vec::new();
        let mut changed = false;
        match ev {
            Event::PointerDown { button, local, .. }
                if *button == crate::window::layer::BTN_LEFT =>
            {
                self.press_origin = Some(*local);
                self.press_hit = self.slot_at(cx.tree, *local);
                cx.handled = true;
            }
            Event::PointerMotion { local } if self.rubberband_enabled => {
                if let Some(origin) = self.press_origin {
                    let band = band_of(origin, *local);
                    match &mut self.rubberband {
                        Some((rect, _)) => *rect = band,
                        None => {
                            let rb = Node::new("rubberband");
                            self.node.append_child(&rb);
                            self.rubberband = Some((band, rb));
                        }
                    }
                    cx.handled = true;
                }
            }
            Event::PointerUp { button, .. } if *button == crate::window::layer::BTN_LEFT => {
                if let Some((band, rb)) = self.rubberband.take() {
                    changed = self.select_band(cx.tree, band);
                    rb.detach();
                } else if let Some(slot) = self.press_hit
                    && let Some(cell) = self.pool.get(slot)
                    && let Some(index) = crate::widgets::row_index_of(cell)
                {
                    self.cursor = Some(index);
                    changed = self.selection.select(index);
                    if self.single_click_activate
                        && let Some(msg) = cx.handlers.fire_index(EventKind::Activate, index)
                    {
                        out.push(msg);
                    }
                }
                self.press_origin = None;
                self.press_hit = None;
                cx.handled = true;
            }
            Event::Scroll(scroll) => {
                let before = self.offset;
                self.offset =
                    (self.offset + scroll.dy).clamp(0.0, self.max_offset(self.viewport.1));
                self.feed_kinetic(scroll, cx.clock.now());
                if self.offset != before {
                    self.rebind(self.viewport);
                }
                cx.handled = true;
            }
            Event::Key(key) if key.pressed => {
                let count = self.model.len();
                let sym = u32::from(key.keysym);
                let ctrl = key.mods.contains(Mods::CTRL);
                let columns = self.columns.max(1);
                match sym {
                    keysyms::KEY_Right => {
                        self.cursor = Some(
                            self.cursor
                                .map_or(0, |c| (c + 1).min(count.saturating_sub(1))),
                        );
                        if self.selection.mode() != SelectionMode::Multiple {
                            changed = self.selection.select(self.cursor.unwrap_or(0));
                        }
                        cx.handled = true;
                    }
                    keysyms::KEY_Left => {
                        self.cursor = Some(self.cursor.map_or(0, |c| c.saturating_sub(1)));
                        if self.selection.mode() != SelectionMode::Multiple {
                            changed = self.selection.select(self.cursor.unwrap_or(0));
                        }
                        cx.handled = true;
                    }
                    keysyms::KEY_Down => {
                        self.cursor = Some(
                            self.cursor
                                .map_or(0, |c| (c + columns).min(count.saturating_sub(1))),
                        );
                        if self.selection.mode() != SelectionMode::Multiple {
                            changed = self.selection.select(self.cursor.unwrap_or(0));
                        }
                        cx.handled = true;
                    }
                    keysyms::KEY_Up => {
                        self.cursor = Some(self.cursor.map_or(0, |c| c.saturating_sub(columns)));
                        if self.selection.mode() != SelectionMode::Multiple {
                            changed = self.selection.select(self.cursor.unwrap_or(0));
                        }
                        cx.handled = true;
                    }
                    keysyms::KEY_space if self.selection.mode() == SelectionMode::Multiple => {
                        if let Some(cursor) = self.cursor {
                            changed = self.selection.toggle(cursor);
                        }
                        cx.handled = true;
                    }
                    keysyms::KEY_a if ctrl && self.selection.mode() == SelectionMode::Multiple => {
                        changed = self.selection.select_all(count);
                        cx.handled = true;
                    }
                    keysyms::KEY_Return | keysyms::KEY_KP_Enter => {
                        if let Some(cursor) = self.cursor
                            && let Some(msg) = cx.handlers.fire_index(EventKind::Activate, cursor)
                        {
                            out.push(msg);
                            cx.handled = true;
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        if changed {
            self.apply_selection();
            if let Some(index) = self.selection.first()
                && let Some(msg) = cx.handlers.fire_index(EventKind::Selected, index)
            {
                out.insert(0, msg);
            }
        }
        out
    }

    fn tick(&mut self, now: Duration, _cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        // Cleared *before* the work, as `ListViewC::tick` does: this frame is
        // the one that pays off whatever the last owed, and a coasting flick
        // (through `rebind`) sets it again when it really changes something.
        self.dirty = false;
        if let Some(delta) = self.kinetic.sample(now) {
            let viewport = self.viewport;
            self.offset = (self.offset + delta.1).clamp(0.0, self.max_offset(viewport.1));
            self.rebind(viewport);
            self.kinetic_active = true;
        } else {
            self.kinetic_active = false;
        }
        Vec::new()
    }

    fn next_deadline(&self, now: Duration) -> Option<Duration> {
        let _ = now;
        // Frames are asked for only while something is actually owed --
        // `ListViewC::next_deadline`'s own rule, and for its own reason: an
        // unconditional `Some(Duration::ZERO)` is read by `App::run` as
        // "render again immediately", so one `GridView` anywhere in the tree
        // pinned the loop at a full repaint per iteration forever and starved
        // the input it was supposed to be showing.
        (self.dirty || self.kinetic_active).then_some(Duration::ZERO)
    }
}

#[cfg(test)]
mod tests {
    use crate::view::Kind;
    use crate::widgets::grid_view::GridViewC;
    use crate::widgets::{build_widget, matches_fixture};

    #[test]
    fn a_grid_view_is_a_gridview_of_child_nodes() {
        // Mutation check: reusing `row` as the cell node (copied from
        // ListView) fails the fixture: GtkGridView's cells are `child`.
        let built = build_widget::<()>(Kind::GridView, &crate::widgets::list_view::tests::props(4));
        matches_fixture(
            &built.node,
            "gridview\n├── child[.activatable]\n┊\n╰── [rubberband]\n",
        )
        .expect("grid_view fixture");
    }

    #[test]
    fn the_pool_covers_whole_rows_of_cells_and_survives_a_scroll() {
        // Mutation check: computing the visible range in cells rather than
        // in rows-of-cells makes the pool `columns` times too small and the
        // last row of every viewport blank.
        let built = build_widget::<()>(
            Kind::GridView,
            &crate::widgets::list_view::tests::props(10_000),
        );
        let mut c = built.controller;
        // Reconciliation: `list_view::ListViewC`'s own module doc explains
        // why these hooks take `&(mut) dyn Controller<Msg>` and are called
        // through `.as_mut()`/`.as_ref()` here rather than `&mut c`/`&c`
        // directly -- `Box<dyn Controller<Msg>>` does not itself implement
        // `Controller<Msg>`, so a bare reference to the box does not deref-
        // coerce to `&dyn Controller<Msg>` at this call position.
        GridViewC::set_metrics(c.as_mut(), (100.0, 80.0), (400.0, 320.0));
        let pool = GridViewC::pool_len(c.as_ref());
        assert_eq!(pool % 4, 0, "whole rows of 4 columns, got {pool}");
        let ids = GridViewC::pool_ids(c.as_ref());
        GridViewC::scroll_to(c.as_mut(), 8_000.0);
        assert_eq!(
            ids,
            GridViewC::pool_ids(c.as_ref()),
            "cells were rebound, not rebuilt"
        );
    }

    /// A settled grid view owes no frame, so it cannot pin `App::run`'s wait
    /// at zero and starve the input the loop is there to deliver.
    ///
    /// Mutation check: return `Some(Duration::ZERO)` unconditionally again
    /// (what this controller did) and the last assertion fails. Restore.
    #[test]
    fn a_settled_grid_view_asks_for_no_more_frames() {
        use std::time::Duration;

        use crate::widgets::Headless;

        let built = build_widget::<()>(Kind::GridView, &crate::widgets::list_view::tests::props(8));
        let mut c = built.controller;
        GridViewC::set_metrics(c.as_mut(), (100.0, 80.0), (400.0, 320.0));
        assert_eq!(
            c.next_deadline(Duration::ZERO),
            Some(Duration::ZERO),
            "a pool that has just been rebound owes one frame"
        );

        let mut hx = Headless::new();
        let mut cx = hx.event_cx(&built.node);
        c.tick(Duration::from_millis(16), &mut cx);
        assert_eq!(
            c.next_deadline(Duration::from_millis(16)),
            None,
            "and nothing after it, with no flick coasting"
        );
    }
}
