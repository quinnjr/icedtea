//! `GtkListView` -- a `listview` of recycled `row`s, plus a click-drag
//! rubberband selection.
//!
//! ```text
//! listview[.separators][.rich-list][.navigation-sidebar][.data-table]
//! ├── row[.activatable]
//! ┊
//! ╰── [rubberband]
//! ```
//!
//! Reconciliation (this task's own file list has no precedent to follow for
//! most of it -- it is the first recycling-pool controller in the crate):
//!
//! * `set_metrics`/`pool_len`/`pool_ids`/`bound_label`/`select`/`selected`/
//!   `scroll_to` are declared as `ScrolledWindowC::set_extent`-style test
//!   hooks, taking `&mut dyn Controller<Msg>` (`&dyn` for the read-only
//!   ones) and downcasting, not `&mut ListViewC` directly -- the plan's own
//!   test snippet calls them as `ListViewC::set_metrics(&mut c, ..)` where
//!   `c` is a `Box<dyn Controller<Msg>>`, which is exactly the shape every
//!   other P6 controller's own test hooks already establish
//!   (`ScrolledWindowC::set_extent`, `FlowBoxC::columns_of`); the unit tests
//!   below call them as `ListViewC::set_metrics(c.as_mut(), ..)` to match.
//! * `crate::widgets::{set_text, set_row_classes, set_row_index}` (named by
//!   the task text but declared nowhere else in the crate) are added to
//!   `widgets::mod` alongside the existing node-keyed side tables
//!   (`set_container`/`record_props`/`set_transition_progress`): a `Node`
//!   carries CSS state only, never arbitrary widget data, so a pooled row's
//!   bound text and model index live in a `RowBinding` thread-local exactly
//!   like those. `bound_label` reads the same table back.
//! * `rebind` is a no-op while `row_height` is not yet known (`<= 0.0`, its
//!   `build`-time default) rather than shrinking the pool to zero: `build`
//!   runs before any real layout pass has told this controller its metrics
//!   (the crate's props are not layout-aware -- `ScrolledWindowC`'s own
//!   `ContentBounds` doc makes the same point), so there is nothing for
//!   `visible_range` to compute yet. `build` seeds exactly one bound
//!   placeholder row when the model is non-empty, the same accommodation
//!   `ListBoxC`/`FlowBoxC::build` make for `build_widget`'s bare-controller
//!   fixture test (dead code once a real `set_metrics` call establishes real
//!   geometry, which every interaction test below does before touching the
//!   pool).
//! * `ListItem` has no `classes` field (only `id`/`text`/`subtitle`/`icon`)
//!   and its label field is named `text`, not `label` -- the task's own test
//!   snippet's `ListItem { id, label, icon, classes }` literal does not
//!   compile against the real type (contract §11 E5's re-export of P4's
//!   type). The row-level `classes` the plan's literal wanted come from
//!   `RowContent` instead, which does carry them; the tests below build
//!   items with `ListItem::new`.
//! * Selection/keyboard-nav and the rubberband drag mirror `ListBoxC`'s and
//!   `FlowBoxC`'s own `on_event`, adapted to index by *model* index (via
//!   `row_index_of`) rather than by pool slot, since a slot's identity does
//!   not track a model index across a scroll.

use std::rc::Rc;
use std::time::Duration;

use crate::css::node::{Node, PseudoStates};
use crate::layout::{BoxDirection, Container, LayoutTree, Rect};
use crate::view::controller::{Controller, Event, EventCx};
use crate::view::{BuildCx, EventKind, Kind, ListItem, Prop, PropName, Props};
use crate::widgets::types::{ItemFactory, Selection, SelectionMode};
use crate::widgets::{Universal, local_rect, prop_bool, prop_u16, set_container};
use crate::window::pointer::{Kinetic, Scroll, ScrollSource};

/// Rows kept beyond the viewport, above and below, so a one-pixel scroll
/// never has to allocate.
const OVERSCAN: usize = 2;

/// `GtkListView`.
pub struct ListViewC {
    /// The `listview` node itself, for hit-testing and for attaching/
    /// detaching pooled rows and the rubberband node.
    node: Node,
    /// The full model. Never walked to build a `Node` per item -- only the
    /// visible range (plus overscan) ever gets one.
    pub model: Rc<[ListItem]>,
    /// One `row` `Node` per pooled slot, always exactly `visible_range`'s
    /// length (once real metrics are known -- see the module doc).
    pub pool: Vec<Node>,
    /// The model index `pool[0]` currently shows.
    pub first_visible: usize,
    /// Scroll offset, px, always within `0..=model.len() as f32 *
    /// row_height - viewport`.
    pub offset: f32,
    /// A row's height, px. `<= 0.0` (its `build`-time default) means real
    /// metrics are not known yet, and `rebind` is a no-op until they are.
    pub row_height: f32,
    /// What is selected, by model index.
    pub selection: Selection,
    /// Keyboard cursor, a model index.
    pub cursor: Option<usize>,
    /// Deceleration state for a flick.
    pub kinetic: Kinetic,
    /// The band's rect (in this controller's own root-node space) and its
    /// `rubberband` node, while a drag is in progress.
    pub rubberband: Option<(Rect, Node)>,
    /// Maps a model index and item to what its row shows.
    factory: ItemFactory,
    /// The last viewport extent [`ListViewC::rebind`] was called with, so a
    /// later `Model`/`ItemFactory` prop change can rebind at the same
    /// extent without a caller re-stating it.
    viewport: f32,
    /// `GtkListView:single-click-activate`.
    single_click_activate: bool,
    /// `GtkListView:enable-rubberband`; `false` still tracks the press
    /// origin (for plain click-select) but never grows a band on motion.
    rubberband_enabled: bool,
    /// The point of the still-down `PointerDown` that started this
    /// press/drag, in root-local space; `None` between gestures.
    press_origin: Option<(f32, f32)>,
    /// The pool slot, if any, the press landed on -- the click-select
    /// fallback `PointerUp` uses when no rubberband was ever created.
    press_hit: Option<usize>,
    /// Something changed this frame that the screen has not caught up with
    /// yet -- the pool grew, the metrics moved, the offset moved -- so one
    /// more frame is owed. See [`Controller::next_deadline`]'s impl below.
    dirty: bool,
    /// Whether a flick is still coasting, mirrored out of `kinetic` (which
    /// keeps that flag private) exactly as `ScrolledWindowC::kinetic_active`
    /// does, so `next_deadline` can ask for frames while it decelerates and
    /// stop when it is done.
    kinetic_active: bool,
    universal: Universal,
}

impl ListViewC {
    /// Which model indices the viewport covers, given the current offset.
    #[must_use]
    pub fn visible_range(&self, viewport: f32) -> std::ops::Range<usize> {
        if self.row_height <= 0.0 || !self.row_height.is_finite() || self.model.is_empty() {
            return 0..0;
        }
        let first = (self.offset / self.row_height).floor().max(0.0) as usize;
        let count = (viewport / self.row_height).ceil().max(0.0) as usize + 1;
        // Reconciliation: the task text's own sample also subtracts
        // `OVERSCAN` from `start` (`first.saturating_sub(OVERSCAN)`), but
        // that contradicts its own test two paragraphs later, which scrolls
        // to exactly row 500 and then asserts pool slot 0 -- i.e. `start`
        // itself -- is bound to model index 500, not 498. Subtracting
        // `OVERSCAN` from `start` also makes an identical scroll land a
        // *shorter* pool near the top of the model than in the middle of it
        // (`start` clips at 0 while `end` does not), which the same test's
        // `pool_ids` equality would catch. Keeping `start == first` and
        // spending the whole overscan budget past the viewport's far edge
        // satisfies both.
        let end = (first + count + OVERSCAN).min(self.model.len());
        first..end.max(first)
    }

    /// Grow or shrink the pool to the visible range and rebind every row.
    ///
    /// A rebind is `set_text`/`set_row_classes`/a `:selected` flip on a node
    /// that is already in the tree -- never an insert or a remove -- so a
    /// row's identity, its running animations and its `:selected` state
    /// survive scrolling. A no-op while real metrics are not yet known (see
    /// the module doc); use [`ListViewC::set_metrics`] to establish them.
    pub fn rebind(&mut self, viewport: f32) {
        if self.row_height <= 0.0 || !self.row_height.is_finite() {
            return;
        }
        let range = self.visible_range(viewport);
        while self.pool.len() < range.len() {
            let row = Node::with_classes("row", &["activatable"]);
            self.node.append_child(&row);
            self.pool.push(row);
        }
        while self.pool.len() > range.len() {
            if let Some(row) = self.pool.pop() {
                self.node.remove_child(&row);
            }
        }
        self.first_visible = range.start;
        for (slot, index) in range.clone().enumerate() {
            let Some(item) = self.model.get(index) else {
                continue;
            };
            let content = self.factory.bind(index, item);
            let row = &self.pool[slot];
            crate::widgets::set_text(row, &content.label);
            crate::widgets::set_row_classes(row, &content.classes);
            row.set_state(PseudoStates::SELECTED, self.selection.contains(index));
            crate::widgets::set_row_index(row, index);
        }
    }

    /// The pool slot a point (in this controller's own root-node space)
    /// lands in, if any.
    fn slot_at(&self, tree: &LayoutTree, point: (f32, f32)) -> Option<usize> {
        self.pool.iter().position(|row| {
            local_rect(tree, &self.node, row).is_some_and(|r| {
                point.0 >= r.x && point.0 < r.right() && point.1 >= r.y && point.1 < r.bottom()
            })
        })
    }

    /// Select every pooled row whose allocation intersects `band`, mapped to
    /// its bound *model* index, without deselecting anything already
    /// selected.
    fn select_band(&mut self, tree: &LayoutTree, band: Rect) -> bool {
        let mut changed = false;
        for row in &self.pool {
            let hit = local_rect(tree, &self.node, row).is_some_and(|r| r.intersects(&band));
            if hit && let Some(index) = crate::widgets::row_index_of(row) {
                changed |= self.selection.toggle_on(index);
            }
        }
        changed
    }

    /// Push the selection onto the pooled rows' `:selected` state.
    pub fn apply_selection(&self) {
        for row in &self.pool {
            let index = crate::widgets::row_index_of(row).unwrap_or(usize::MAX);
            row.set_state(PseudoStates::SELECTED, self.selection.contains(index));
        }
    }

    /// Feed a `Scroll` frame into `self.kinetic` -- `ScrolledWindowC::
    /// feed_kinetic`'s own logic, verbatim.
    fn feed_kinetic(&mut self, scroll: &Scroll, now: Duration) {
        if scroll.source == ScrollSource::Finger {
            self.kinetic.feed(scroll, now);
        } else {
            self.kinetic.cancel();
        }
    }

    /// A pooled row's own height, from the sheet and the font rather than
    /// from the last layout pass: the taller of `min-height` and the row
    /// text's own line box, plus the vertical padding and border a
    /// `listview > row` declares.
    ///
    /// Not the row's *allocated* height, deliberately. The pool's size is a
    /// function of the row height and the rows share the viewport between
    /// them, so reading the height back off the allocation makes the two
    /// chase each other frame after frame (a one-row pool measures the whole
    /// viewport, which halves the pool, which doubles the height, ...). Both
    /// terms here are pure functions of the sheet and the font database, so
    /// this is still a fixed point.
    ///
    /// The line box is not optional. Adwaita gives `row` no `min-height` at
    /// all — `min_size` is `(0, 0)` and the padding is 2px a side — so
    /// before P8-D72's close-out every row was 4px tall, the gallery's ten
    /// rows totalled 40px inside a 120px viewport, [`Self::max_offset`] was
    /// 0, and a `ListView` could not be scrolled or recycled at all. GTK
    /// sizes the row from the label inside it; so does this.
    fn css_row_height(
        &self,
        styles: &crate::view::StyleMap,
        fonts: &mut crate::text::FontDatabase,
    ) -> Option<f32> {
        let row = self.pool.first()?;
        let style = styles.get(&crate::view::node_addr(row))?;
        let (_, min_h) = style.min_size((0.0, 0.0));
        let text = crate::widgets::row_text_height(row, style, fonts).unwrap_or(0.0);
        let [pad_top, _, pad_bottom, _] = style.padding(0.0);
        let [top, _, bottom, _] = style.border_widths();
        let height = min_h.max(text) + pad_top + pad_bottom + top + bottom;
        (height.is_finite() && height > 0.0).then_some(height)
    }

    /// **The viewport is the extent this view was *asked* to be, never what
    /// its own rows made it.**
    ///
    /// `GtkListView` is a `GtkScrollable`: its viewport is imposed from
    /// outside, and the rows it pools are a function of that viewport. The
    /// two numbers a widget here can be given from outside are its
    /// `height-request` (P8-D71's close-out made that universal) and the
    /// sheet's `min-height`; the allocation is the fallback for a view given
    /// neither.
    ///
    /// **Not the allocation when a request exists.** A recycling view whose
    /// viewport is its own allocation feeds itself: the pool is
    /// `ceil(viewport / row_height) + 1 + OVERSCAN` rows, those rows are its
    /// children, so the box grows to hold them, so the viewport grows, so the
    /// pool grows — until the pool is the whole model, `max_offset` is 0 and
    /// nothing can scroll. Measured on the gallery's own sample the moment
    /// rows started measuring their text: a 120 px request settled at a
    /// 231 px "viewport", ten rows of 23 px, `max_offset` 0. Both terms here
    /// are instead pure functions of the props and the sheet.
    ///
    /// Take this frame's real geometry — the viewport from what the view was
    /// asked to be, the row height from the sheet and the font — and rebind
    /// if either moved.
    ///
    /// Until P8-D71's close-out `row_height`/`viewport` were written *only*
    /// by [`ListViewC::set_metrics`], which no production path ever called:
    /// `rebind` is a documented no-op while `row_height <= 0.0`, so a real
    /// `ListView` never grew past `build`'s single placeholder row and no
    /// interaction could be driven against one. This is the missing feedback
    /// path, and `tick` (which `next_deadline` already asks for every frame)
    /// is where a controller can see a completed layout pass.
    fn adopt_metrics<Msg: Clone + 'static>(&mut self, cx: &mut EventCx<'_, Msg>) {
        let Some(alloc) = cx.tree.allocation(&self.node) else {
            return;
        };
        // See this function's own doc for why the viewport is not the
        // allocation whenever the view was given an extent of its own.
        let requested = crate::widgets::size_request_of(&self.node).1;
        let css_min = cx
            .styles
            .get(&crate::view::node_addr(&self.node))
            .map_or(0.0, |style| style.min_size((0.0, 0.0)).1);
        let viewport = if requested > 0.0 {
            requested
        } else if css_min > 0.0 {
            css_min
        } else {
            alloc.content_box.height
        };
        let Some(row_height) = self.css_row_height(cx.styles, cx.fonts) else {
            return;
        };
        if !viewport.is_finite() || viewport <= 0.0 {
            return;
        }
        if (viewport - self.viewport).abs() < f32::EPSILON
            && (row_height - self.row_height).abs() < f32::EPSILON
        {
            return;
        }
        self.viewport = viewport;
        self.row_height = row_height;
        self.offset = self.offset.clamp(0.0, self.max_offset(viewport));
        self.rebind(viewport);
        self.dirty = true;
    }

    /// How many of this node's children are the controller's own: every
    /// pooled row, plus the rubberband while a drag is running.
    fn reserved(&self) -> usize {
        self.pool.len() + usize::from(self.rubberband.is_some())
    }

    /// The maximum scroll offset, never negative.
    fn max_offset(&self, viewport: f32) -> f32 {
        if self.row_height.is_finite() && self.row_height > 0.0 && viewport.is_finite() {
            (self.model.len() as f32 * self.row_height - viewport).max(0.0)
        } else {
            0.0
        }
    }

    // -- Test/headless hooks, one-line accessors over a type-erased
    // controller (`ScrolledWindowC::set_extent`'s own pattern). --

    /// Establish real row-height/viewport metrics and rebind the pool to
    /// them. A real widget instance gets these from its first layout pass;
    /// these tests set them directly instead of standing one up.
    pub fn set_metrics<Msg: 'static>(
        controller: &mut dyn Controller<Msg>,
        row_height: f32,
        viewport: f32,
    ) {
        let any: &mut dyn std::any::Any = controller;
        if let Some(me) = any.downcast_mut::<Self>() {
            me.row_height = row_height;
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

    /// A stable per-node identity for each pooled row, in slot order -- for
    /// asserting that a scroll rebinds the same `Node`s rather than
    /// replacing them.
    #[must_use]
    pub fn pool_ids<Msg: 'static>(controller: &dyn Controller<Msg>) -> Vec<usize> {
        let any: &dyn std::any::Any = controller;
        any.downcast_ref::<Self>()
            .map(|me| me.pool.iter().map(Node::addr).collect())
            .unwrap_or_default()
    }

    /// The text bound to pool slot `slot`.
    #[must_use]
    pub fn bound_label<Msg: 'static>(controller: &dyn Controller<Msg>, slot: usize) -> String {
        let any: &dyn std::any::Any = controller;
        any.downcast_ref::<Self>()
            .and_then(|me| me.pool.get(slot))
            .map(|row| crate::widgets::text_of(row).to_string())
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
            me.offset = offset.clamp(0.0, me.max_offset(viewport));
            me.rebind(viewport);
        }
    }
}

/// The rect spanning two points, normalised so width/height are never
/// negative -- `FlowBoxC`'s own `band_of`.
fn band_of(a: (f32, f32), b: (f32, f32)) -> Rect {
    let x = a.0.min(b.0);
    let y = a.1.min(b.1);
    Rect::new(x, y, (a.0 - b.0).abs(), (a.1 - b.1).abs())
}

impl<Msg: Clone + 'static> Controller<Msg> for ListViewC {
    fn kind(&self) -> Kind {
        Kind::ListView
    }

    fn build(node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        set_container(
            node,
            Container::Box {
                direction: BoxDirection::Column,
            },
        );
        if props.bool(PropName::ShowSeparators, false) {
            node.add_class("separators");
        }
        let mode = SelectionMode::from_u16(
            u16::try_from(props.int(PropName::SelectionMode, 1)).unwrap_or(1),
        );
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
            row_height: 0.0,
            selection: Selection::new(mode),
            cursor: None,
            kinetic: Kinetic::default(),
            rubberband: None,
            factory,
            viewport: 0.0,
            single_click_activate: props.bool(PropName::SingleClickActivate, false),
            rubberband_enabled: props.bool(PropName::EnableRubberband, false),
            press_origin: None,
            press_hit: None,
            dirty: true,
            kinetic_active: false,
            universal: Universal::new(node, Kind::ListView),
        };
        // `rebind` is a no-op before real metrics exist (module doc), so the
        // fixture's one required `row` line, over `build_widget`'s bare
        // controller, can only come from a placeholder built here directly.
        // Dead code for every real widget instance, which gets real metrics
        // from its first layout pass long before an application can observe
        // the pool.
        if let Some(item) = me.model.first() {
            let row = Node::with_classes("row", &["activatable"]);
            me.node.append_child(&row);
            let content = me.factory.bind(0, item);
            crate::widgets::set_text(&row, &content.label);
            crate::widgets::set_row_classes(&row, &content.classes);
            crate::widgets::set_row_index(&row, 0);
            me.pool.push(row);
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
            PropName::ShowSeparators => {
                if prop_bool(value, false) {
                    node.add_class("separators");
                } else {
                    node.remove_class("separators");
                }
            }
            PropName::SingleClickActivate => {
                self.single_click_activate = prop_bool(value, false);
            }
            PropName::Selected => {
                // The builder's `.selected(index)` writes this prop; until
                // the review-fix wave nothing read it and only a real click
                // could move the selection.
                if let Prop::Int(index) = value
                    && let Ok(index) = usize::try_from(*index)
                    && index < self.model.len()
                {
                    self.selection.select(index);
                    self.apply_selection();
                }
            }
            PropName::EnableRubberband => self.rubberband_enabled = prop_bool(value, false),
            other => {
                self.universal.apply(node, Kind::ListView, other, value);
            }
        }
    }

    /// The pooled rows (and the rubberband, while one exists) are this
    /// controller's own node children, built before any view child; a view
    /// child of a `ListView` therefore starts after them.
    fn child_index(&self, view_index: usize) -> usize {
        view_index + self.reserved()
    }

    /// ... and reconcile's trim step has to know they are there, or it
    /// detaches every pooled row the moment it runs — a `ListView` takes no
    /// view children at all, so *every* one of its node children is past the
    /// default bound of `view_count`. Until P8-D71's close-out that is
    /// exactly what happened: the pool was evicted on the first reconcile,
    /// no row ever had an allocation, and no click could land on one.
    fn reserved_total(&self, view_count: usize) -> usize {
        view_count + self.reserved()
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
                // Recorded unconditionally, same as `FlowBoxC`: whether this
                // becomes a click-select or a rubberband is only known at
                // release.
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
                    && let Some(row) = self.pool.get(slot)
                    && let Some(index) = crate::widgets::row_index_of(row)
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
                self.offset = (self.offset + scroll.dy).clamp(0.0, self.max_offset(self.viewport));
                self.feed_kinetic(scroll, cx.clock.now());
                if self.offset != before {
                    self.rebind(self.viewport);
                    self.dirty = true;
                }
                cx.handled = true;
            }
            Event::Key(key) if key.pressed => {
                let count = self.model.len();
                let sym = u32::from(key.keysym);
                let ctrl = key.mods.contains(Mods::CTRL);
                match sym {
                    keysyms::KEY_Down => {
                        self.cursor = Some(
                            self.cursor
                                .map_or(0, |c| (c + 1).min(count.saturating_sub(1))),
                        );
                        if self.selection.mode() != SelectionMode::Multiple {
                            changed = self.selection.select(self.cursor.unwrap_or(0));
                        }
                        cx.handled = true;
                    }
                    keysyms::KEY_Up => {
                        self.cursor = Some(self.cursor.map_or(0, |c| c.saturating_sub(1)));
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

    fn tick(&mut self, now: Duration, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        // Cleared *before* the work: `adopt_metrics` and a coasting flick
        // both set it again when they actually change something, and this
        // frame is the one that pays off whatever the last one owed.
        self.dirty = false;
        self.adopt_metrics(cx);
        if let Some(delta) = self.kinetic.sample(now) {
            let viewport = self.viewport;
            self.offset = (self.offset + delta.1).clamp(0.0, self.max_offset(viewport));
            self.rebind(viewport);
            self.kinetic_active = true;
            self.dirty = true;
        } else {
            self.kinetic_active = false;
        }
        Vec::new()
    }

    fn next_deadline(&self, now: Duration) -> Option<Duration> {
        let _ = now;
        // Frames are asked for only while something is actually owed: the
        // pool has just grown or rebound (`dirty`), or a flick is still
        // coasting (`kinetic_active`, mirrored out of `Kinetic`'s private
        // flag the way `ScrolledWindowC` mirrors it).
        //
        // This used to be an unconditional `Some(Duration::ZERO)` — "the
        // always-poll default", on the grounds that no unit test drove the
        // loop off it. A unit test is not the only reader: `App::run` treats
        // a zero deadline as "render again immediately", so a `ListView`
        // anywhere in the tree pinned the loop at a full repaint per
        // iteration forever. With M3's own accepted `clip_path` cost (see
        // `interaction_gate.rs`'s `REACT`) that is on the order of ten
        // seconds a frame, and the gallery's `--widget list_view` run
        // dispatched **no** pointer event at all in 25 s of a driver
        // clicking and scrolling at it: the input it was starving was the
        // interaction gate's.
        (self.dirty || self.kinetic_active).then_some(Duration::ZERO)
    }
}

// `pub(crate)`: Task 20's `grid_view` tests reuse this module's `props`
// helper (`widgets::list_view::tests::props`) rather than duplicating a
// model/props builder that is otherwise identical -- a private `mod tests`
// is visible only inside this file, so both the module and the helper need
// crate visibility to cross into `widgets::grid_view`.
#[cfg(test)]
pub(crate) mod tests {
    use std::rc::Rc;

    use crate::view::{Kind, Prop, PropName, Props};
    use crate::widgets::list_view::ListViewC;
    use crate::widgets::types::{ItemFactory, ListItem, SelectionMode};
    use crate::widgets::{Headless, build_widget, matches_fixture};

    fn model(n: u64) -> Rc<[ListItem]> {
        (0..n)
            .map(|i| ListItem::new(i, &format!("row {i}")))
            .collect()
    }

    pub(crate) fn props(n: u64) -> Props {
        let mut p = Props::default();
        p.set(PropName::Model, Prop::Items(model(n)));
        p.set(
            PropName::ItemFactory,
            Prop::Factory(ItemFactory::label_only()),
        );
        p.set(
            PropName::SelectionMode,
            Prop::Enum(SelectionMode::Multiple.to_u16()),
        );
        p
    }

    #[test]
    fn a_list_view_is_a_listview_of_rows() {
        let built = build_widget::<()>(Kind::ListView, &props(3));
        matches_fixture(
            &built.node,
            "listview[.separators][.rich-list][.navigation-sidebar][.data-table]\n\
             ├── row[.activatable]\n┊\n╰── [rubberband]\n",
        )
        .expect("list_view fixture");
    }

    #[test]
    fn the_selected_prop_moves_the_selection() {
        // Mutation check: remove `PropName::Selected` from
        // `ListViewC::set_prop` and the builder's `.selected(..)` is inert
        // again -- the selection stays at the default index 0 and the first
        // assertion fails.
        let mut p = props(5);
        p.set(PropName::Selected, Prop::Int(3));
        let built = build_widget::<()>(Kind::ListView, &p);
        assert_eq!(
            ListViewC::selected::<()>(built.controller.as_ref()),
            vec![3],
            "the Selected prop must select its index"
        );
        // Out-of-range writes are dropped, matching the DropDown arm's clamp
        // philosophy without inventing a selection.
        let mut p = props(5);
        p.set(PropName::Selected, Prop::Int(99));
        let built = build_widget::<()>(Kind::ListView, &p);
        assert!(
            !ListViewC::selected::<()>(built.controller.as_ref()).contains(&99),
            "an out-of-range Selected index must not select anything"
        );
    }

    #[test]
    fn the_pool_is_the_viewport_plus_overscan_not_the_model() {
        // The point of the whole widget. Mutation check: building one node
        // per item makes this allocate 100 000 nodes and the assertion fails
        // by four orders of magnitude.
        let built = build_widget::<()>(Kind::ListView, &props(100_000));
        let mut c = built.controller;
        ListViewC::set_metrics(c.as_mut(), 30.0, 300.0);
        assert!(
            ListViewC::pool_len(c.as_ref()) <= 20,
            "pool is {} nodes for a 300px viewport of 30px rows",
            ListViewC::pool_len(c.as_ref())
        );
    }

    #[test]
    fn scrolling_rebinds_rows_without_losing_the_selection_or_the_node_identity() {
        // The contract's P6 gate, verbatim: "row recycling is proven not to
        // lose selection or animation state across a scroll". Mutation
        // check: rebuilding rows on scroll changes every node pointer and
        // drops the :selected state with them.
        let built = build_widget::<()>(Kind::ListView, &props(1_000));
        let mut c = built.controller;
        ListViewC::set_metrics(c.as_mut(), 30.0, 300.0);
        ListViewC::select(c.as_mut(), 500);
        let before: Vec<usize> = ListViewC::pool_ids(c.as_ref());
        ListViewC::scroll_to(c.as_mut(), 500.0 * 30.0);
        let after: Vec<usize> = ListViewC::pool_ids(c.as_ref());
        assert_eq!(before, after, "the same nodes were rebound, not replaced");
        assert!(
            ListViewC::selected(c.as_ref()).contains(&500),
            "selection survived"
        );
        assert_eq!(ListViewC::bound_label(c.as_ref(), 0), "row 500");
    }

    /// The metrics-feedback path P8-D71's close-out added: a `ListView` in a
    /// real layout learns its own viewport and row height from the frame it
    /// was just laid out in, and grows its pool, with nobody calling
    /// `set_metrics`.
    ///
    /// Mutation check: drop the `self.adopt_metrics(cx)` line from
    /// `ListViewC::tick` and the pool stays at `build`'s single placeholder
    /// row, which is what every real `ListView` did before this.
    #[test]
    fn a_laid_out_list_view_learns_its_own_metrics_and_grows_its_pool() {
        use std::collections::HashMap;
        use std::time::Duration;

        use crate::anim::{Clock, ManualClock};
        use crate::css::cascade::CompiledSheet;
        use crate::css::computed::ResolveEnv;
        use crate::css::node::Node;
        use crate::layout::{BoxDirection, Container, FixedMeasure, LayoutTree};
        use crate::view::controller::{Controller, EventCx, Phase};
        use crate::view::reconcile::BuildCx;
        use crate::view::render::{Animations, StyleMap, layout_tree, node_addr, restyle_tree};
        use crate::view::{Cmd, Handlers};
        use crate::window::focus::FocusRing;
        use crate::window::selection::Clipboard;

        // 30px rows in a 120px viewport: four fit, so the pool is well short
        // of the model's thousand and well past `build`'s one placeholder.
        let sheet = CompiledSheet::compile(
            "listview { min-height: 120px; min-width: 200px } \
             listview > row { min-height: 30px }",
        );
        let mut fonts = crate::text::FontDatabase::probe_only();
        let mut icons = crate::icons::IconTheme::with_name_and_roots("hicolor", vec![]);
        let clock: Rc<dyn Clock> = Rc::new(ManualClock::new());
        let env = ResolveEnv::default();
        // Under a parent: `layout_tree` floors its *root* at the available
        // space, and this test is about the widget's own box.
        let window = Node::new("window");
        let node = Node::new("listview");
        window.append_child(&node);
        let mut controller = {
            let mut cx = BuildCx {
                sheet: &sheet,
                fonts: &mut fonts,
                icons: &mut icons,
                clock: &clock,
                env: &env,
            };
            <ListViewC as Controller<()>>::build(&node, &props(1_000), &mut cx)
        };
        assert_eq!(controller.pool.len(), 1, "build seeds one placeholder row");

        let mut anims = Animations::new();
        let mut styles = StyleMap::new();
        restyle_tree(
            &window,
            &sheet,
            &env,
            &mut styles,
            &mut anims,
            Duration::ZERO,
        );
        let mut containers: HashMap<_, Container> = HashMap::new();
        for addr in [node_addr(&window), node_addr(&node)] {
            containers.insert(
                addr,
                Container::Box {
                    direction: BoxDirection::Column,
                },
            );
        }
        let mut tree = LayoutTree::new();
        let mut measure = FixedMeasure(taffy::Size {
            width: 0.0,
            height: 0.0,
        });
        layout_tree(
            &window,
            &styles,
            &containers,
            &mut tree,
            &env,
            (Some(400.0), Some(400.0)),
            &mut measure,
        )
        .expect("the list view lays out");

        let handlers: Handlers<()> = Handlers::default();
        let mut focus = <FocusRing as Default>::default();
        let mut clipboard = Clipboard::offscreen();
        let mut cmds: Vec<Cmd<()>> = Vec::new();
        let mut ecx = EventCx {
            node: &node,
            handlers: &handlers,
            tree: &tree,
            styles: &styles,
            focus: &mut focus,
            clipboard: &mut clipboard,
            icons: &mut icons,
            fonts: &mut fonts,
            clock: &clock,
            env: &env,
            cmds: &mut cmds,
            phase: Phase::Target,
            handled: false,
        };
        Controller::<()>::tick(&mut controller, Duration::ZERO, &mut ecx);

        assert!(
            (controller.row_height - 30.0).abs() < f32::EPSILON,
            "the row height came from the sheet; got {}",
            controller.row_height
        );
        assert!(
            (controller.viewport - 120.0).abs() < f32::EPSILON,
            "the viewport came from the allocation; got {}",
            controller.viewport
        );
        assert_eq!(
            controller.pool.len(),
            controller.visible_range(120.0).len(),
            "the pool is the visible range, not the model"
        );
        assert!(
            controller.pool.len() > 1 && controller.pool.len() < 20,
            "a 120px viewport of 30px rows pools a handful, not 1 and not \
             1000; got {}",
            controller.pool.len()
        );
    }

    #[test]
    fn a_model_that_shrinks_under_the_selection_never_panics() {
        // Untrusted input: the model is the application's.
        let built = build_widget::<()>(Kind::ListView, &props(1_000));
        let mut c = built.controller;
        let mut hx = Headless::new();
        ListViewC::set_metrics(c.as_mut(), 30.0, 300.0);
        ListViewC::select(c.as_mut(), 999);
        c.set_prop(
            &built.node,
            PropName::Model,
            &Prop::Items(model(2)),
            &mut hx.cx(),
        );
        assert!(ListViewC::selected(c.as_ref()).iter().all(|i| *i < 2));
        c.set_prop(
            &built.node,
            PropName::Model,
            &Prop::Items(model(0)),
            &mut hx.cx(),
        );
        assert!(ListViewC::selected(c.as_ref()).is_empty());
    }

    /// The §8 interaction gate's "scroll a list" interaction, driven through
    /// a real `App` instead of through the compositor.
    ///
    /// `interaction_gate.rs`'s own
    /// `scrolling_a_list_view_recycles_rows_without_losing_selection` is the
    /// contract's home for this and is `#[ignore]`d: `wlr` 0.20.28 forwards
    /// no `wl_pointer.axis` to any client, so a scroll cannot be *injected*
    /// through the harness compositor at all (contract §10 P8-D74). Nothing
    /// about the widget stops it being proven here, where the event goes
    /// straight into the loop: this is the same ten rows in the same 120 px
    /// viewport the gallery's sample builds, laid out, painted, scrolled and
    /// read back off the raster surface.
    mod pixels {
        use std::rc::Rc;
        use std::time::Duration;

        use crate::view::app::ScriptStep;
        use crate::view::builders::{self as w, ListBoxExt, StackPagesExt};
        use crate::view::{Cmd, Frames, View};
        use crate::widgets::offscreen::{frames, px};
        use crate::widgets::types::{ItemFactory, ListItem, RowContent, SelectionMode};
        use crate::window::InputEvent;
        use crate::window::pointer::{Scroll, ScrollSource};

        /// The gallery's own `LIST_ROWS`, so this test and the gallery sample
        /// stay the same fixture.
        const ROWS: [&str; 10] = [
            "Row 0", "Row 1", "Row 2", "Row 3", "Row 4", "Row 5", "Row 6", "Row 7", "Row 8",
            "Row 9",
        ];

        #[derive(Clone, Debug)]
        enum Msg {
            Selected(usize),
        }

        fn update(model: &mut usize, msg: Msg) -> Cmd<Msg> {
            match msg {
                Msg::Selected(index) => *model = index,
            }
            Cmd::None
        }

        /// The gallery's `Kind::ListView` sample, verbatim in shape: ten
        /// rows, single selection, a 200x120 request.
        fn view(model: &usize) -> View<Msg> {
            let items: Rc<[ListItem]> = ROWS
                .iter()
                .enumerate()
                .map(|(i, text)| ListItem::new(i as u64, text))
                .collect();
            let factory =
                ItemFactory::new(|_index, item: &ListItem| RowContent::from_label(&item.text));
            w::list_view(items, factory)
                .selection_mode(SelectionMode::Single)
                .selected(*model)
                .on_selected(Msg::Selected)
                .width_request(200)
                .height_request(120)
        }

        /// Inside the second pooled row, in surface coordinates: the view is
        /// 208 px of nine ~23 px rows centred in a 260 px surface, so the
        /// rows start at y=26 and the second spans 49..72. Derived, not
        /// guessed -- `a_laid_out_list_view_learns_its_own_metrics_and_grows_its_pool`
        /// pins the geometry this follows from, and the click assertion
        /// below fails loudly if it ever stops landing on a row.
        const ROW1_Y: f64 = 60.0;

        /// A left press or release at the pointer's current position.
        fn button(pressed: bool) -> ScriptStep<Msg> {
            ScriptStep::Event(InputEvent::PointerButton {
                button: crate::window::layer::BTN_LEFT,
                pressed,
                serial: 0,
                time_ms: 0,
            })
        }

        /// A wheel scroll of `dy` px at the pointer's current position.
        fn wheel(dy: f32) -> ScriptStep<Msg> {
            ScriptStep::Event(InputEvent::Scroll(Scroll {
                dx: 0.0,
                dy,
                source: ScrollSource::Wheel,
                stop: false,
                time_ms: 0,
            }))
        }

        /// The frame's background: its most common pixel value.
        ///
        /// Not `px(.., 0, 0)`. This surface is not the window's own paint at
        /// every corner, and taking a corner as the reference made a frame of
        /// plain background read as entirely ink.
        fn background(out: &Frames, frame: usize) -> (u8, u8, u8, u8) {
            let mut counts: std::collections::HashMap<(u8, u8, u8, u8), usize> =
                std::collections::HashMap::new();
            for y in 0..out.height() {
                for x in 0..out.width() {
                    *counts.entry(px(out, frame, x, y)).or_default() += 1;
                }
            }
            counts
                .into_iter()
                .max_by_key(|&(_, n)| n)
                .map(|(colour, _)| colour)
                .expect("a frame has pixels")
        }

        /// Every pixel of one frame, for comparing two of them.
        fn whole(out: &Frames, frame: usize) -> Vec<(u8, u8, u8, u8)> {
            (0..out.height())
                .flat_map(|y| (0..out.width()).map(move |x| (x, y)))
                .map(|(x, y)| px(out, frame, x, y))
                .collect()
        }

        /// Rows paint their bound text at all.
        ///
        /// Mutation check: make `ControllerPainter::paint_content`'s `None`
        /// arm return `false` again instead of calling
        /// `widgets::paint_row`; a pooled row is a bare `Node` with no
        /// instance, nothing draws its label, and the whole frame is
        /// background. Restore.
        #[test]
        fn a_pooled_row_paints_its_bound_label() {
            let out = frames(
                0usize,
                update,
                view,
                (200, 260),
                vec![
                    ScriptStep::Advance(Duration::from_millis(16)),
                    ScriptStep::Advance(Duration::from_millis(16)),
                    ScriptStep::Capture,
                ],
            );
            let background = background(&out, 0);
            let ink = whole(&out, 0)
                .into_iter()
                .filter(|p| *p != background)
                .count();
            assert!(ink > 0, "no pooled row painted anything");
        }

        /// A scroll rebinds the pool, and the selection made before the
        /// scroll is back — same rows, same `:selected` band — after
        /// scrolling home again.
        ///
        /// Four frames: at rest (row 0 selected, the model's default), after
        /// selecting row 1, at the far end of the scroll, and home again.
        /// `rest != selected` proves the band is really the selection and
        /// not some constant; `selected != scrolled` proves the pool rebound
        /// to other rows; `selected == home` proves the recycled rows came
        /// back carrying the same selection.
        ///
        /// Mutation check 1 (verified): make `ListViewC::adopt_metrics` take
        /// its viewport from `alloc.content_box.height`; the view sizes
        /// itself to all ten rows, `max_offset` is 0, the scrolled frame is
        /// identical to the selected one and the second assertion fails.
        /// Mutation check 2 (verified): drop the
        /// `row.set_state(PseudoStates::SELECTED, ..)` line from
        /// [`ListViewC::rebind`]; the rows come home but the `:selected`
        /// band does not, and the third assertion fails. Restore both.
        #[test]
        fn scrolling_recycles_the_pooled_rows_and_keeps_the_selection() {
            // 260px tall so every pooled row is on the surface; the view's
            // own viewport is its 120px request either way.
            let out = frames(
                0usize,
                update,
                view,
                (200, 260),
                vec![
                    ScriptStep::Advance(Duration::from_millis(16)),
                    ScriptStep::Advance(Duration::from_millis(16)),
                    ScriptStep::Capture,
                    // Click the *second* row. Not the first: `ListViewC`
                    // selects index 0 on its own, so clicking row 0 selects
                    // what is already selected and paints nothing. And a
                    // click, exercising the pointer path end to end
                    // (`set_prop` also honours `PropName::Selected` since
                    // the review-fix wave, but this test is about clicks).
                    //
                    // The pointer stays here for the scrolls below, which is
                    // what a real one would do -- `view::app`'s
                    // `InputEvent::Scroll` arm delivers to `hovered`.
                    ScriptStep::Event(InputEvent::PointerMotion {
                        x: 100.0,
                        y: ROW1_Y,
                        time_ms: 0,
                    }),
                    button(true),
                    button(false),
                    ScriptStep::Advance(Duration::from_millis(16)),
                    ScriptStep::Capture,
                    // Further than `max_offset`, which the controller clamps.
                    wheel(10_000.0),
                    ScriptStep::Advance(Duration::from_millis(16)),
                    ScriptStep::Capture,
                    wheel(-10_000.0),
                    ScriptStep::Advance(Duration::from_millis(16)),
                    ScriptStep::Capture,
                ],
            );
            assert_eq!(out.len(), 4, "four frames were captured");

            let rest = whole(&out, 0);
            let selected = whole(&out, 1);
            let scrolled = whole(&out, 2);
            let home = whole(&out, 3);
            assert!(
                rest != selected,
                "clicking the second row painted nothing, so the frames \
                 below prove nothing about a selection"
            );
            assert!(
                selected != scrolled,
                "scrolling to the end left every pixel exactly as it was, so \
                 nothing recycled"
            );
            assert!(
                selected == home,
                "scrolling home again must rebind the same rows, with the \
                 same selection: {} of {} pixels differ",
                selected.iter().zip(&home).filter(|(a, b)| a != b).count(),
                selected.len()
            );
        }
    }
}
