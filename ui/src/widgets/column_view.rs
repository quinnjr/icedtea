//! `GtkColumnView` -- a `header` row of per-column headers over a real
//! `listview`, plus click-to-sort and (resizable columns') drag-to-resize.
//!
//! ```text
//! columnview[.column-separators][.rich-list][.navigation-sidebar][.data-table]
//! ├── header
//! │   ╰── <column header>
//! ├── listview
//! ╰── [rubberband]
//! ```
//!
//! Reconciliation notes:
//!
//! * `Kind::ColumnViewColumn` renders no node of its own (contract deviation
//!   D15), so its `View`s never get their own `Controller` beyond the
//!   generic fallback -- what makes each one a *column* is entirely this
//!   controller reading its recorded props back off its own `Node` (the
//!   established `props_of`/`RECORDED_PROP_NAMES` mechanism `stack::StackC`
//!   already uses for `StackPage`, extended here with `Title`/`Resizable`/
//!   `Expand`/`Sorter`).
//! * `build_instance` calls `build_controller` *before* the reconciler
//!   attaches a container's own view children (`stack::StackC`'s own module
//!   doc gives the same fact) -- but `widgets::child_slot` gives this
//!   controller a further seam StackC has no counterpart for: `header`
//!   itself, not `columnview`'s root node, is what `column_view(model,
//!   columns)`'s real `column_view_column` children get reconciled *into*,
//!   via `Kind::ColumnView`'s new `child_slot` arm. So `build` seeds `header`
//!   with exactly one placeholder column (the same accommodation `ListViewC`/
//!   `FlowBoxC`/`StackC::build` all make for `build_widget`'s bare-controller
//!   fixture test and for a widget's very first frame), and this
//!   controller's own [`Controller::reserved_total`] -- the one `&self` hook
//!   the reconciler calls, on every reconcile, once real columns have
//!   actually been inserted into `header` -- re-derives `columns` for real
//!   from `header`'s first `view_count` children and lets the reconciler's
//!   own trim step (which runs right after `reserved_total` returns) evict
//!   the placeholder, now pushed past that count. `columns` is therefore a
//!   `RefCell`, not the plain `Vec` the interface sketch shows -- the same
//!   deviation `StackC::pages`'s own doc comment makes for the identical
//!   `&self` constraint.
//! * The embedded `list: ListViewC` is built directly (`<ListViewC as
//!   Controller<Msg>>::build`), not through `build_controller`, because this
//!   controller needs the concrete type to forward events and props to it by
//!   name; the interface sketch's `pub list: ListViewC` field is exactly
//!   this. Per-column cell rendering (each `ColumnViewColumn`'s own
//!   `ItemFactory` painting only its own column of every row) is out of this
//!   task's file list -- `ListViewC::rebind` binds one whole-row
//!   `ItemFactory` per model index, with no notion of splitting that into
//!   columns -- so the embedded list uses `column_view`'s own `Model` prop
//!   with `ItemFactory::label_only()` for now; a real per-column cell layout
//!   is future work this doc comment flags rather than silently guesses at.
//! * Clicking a header column fires and cycles the sort on `PointerDown`
//!   itself, not on release: GTK's own `GtkColumnViewSorter` triggers from a
//!   `GtkGestureClick` that has already fired by the time a plain click
//!   completes, and the task's own interaction test sends bare
//!   `PointerDown`s and asserts the message from each one directly.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use crate::css::node::Node;
use crate::layout::LayoutTree;
use crate::view::controller::{Controller, Event, EventCx};
use crate::view::{BuildCx, EventKind, Kind, Prop, PropName, Props};
use crate::widgets::list_view::ListViewC;
use crate::widgets::types::{ItemFactory, SortOrder, Sorter};
use crate::widgets::{Universal, local_rect, props_of};

/// One column's identity, geometry and behaviour.
pub struct ColumnState {
    /// The column's header title.
    pub title: Rc<str>,
    /// Its current width, px.
    pub width: f32,
    /// Whether a drag on its trailing edge resizes it.
    pub resizable: bool,
    /// Whether it grows to fill leftover space.
    pub expand: bool,
    /// Its comparator, if it is sortable at all.
    pub sorter: Option<Sorter>,
    /// The column's own header `Node` -- the real `ColumnViewColumn`'s node
    /// once one exists, `header`'s placeholder child until then.
    pub node: Node,
}

/// A column's default width, px, before any resize.
const DEFAULT_COLUMN_WIDTH: f32 = 120.0;

/// `GtkColumnView`.
pub struct ColumnViewC {
    /// The `columnview` node itself, for hit-testing.
    node: Node,
    /// One state per column, in order. See the module doc for why this is a
    /// `RefCell` rather than a plain `Vec`.
    pub columns: RefCell<Vec<ColumnState>>,
    /// The embedded row list.
    pub list: ListViewC,
    /// The `listview` node `list` was built over, kept so props/events can
    /// be forwarded to it by name (`ListViewC::set_prop`/`on_event` both
    /// take the target node as an explicit argument, not `self.node`).
    list_node: Node,
    /// Which column is sorted, and which way.
    pub sort: Option<(usize, SortOrder)>,
    /// `(column, pointer-down x)` while a resize drag is in progress.
    pub drag: Option<(usize, f32)>,
    /// The `header` node -- also this controller's [`crate::widgets::
    /// child_slot`] target, so real `column_view_column` children land here.
    pub header: Node,
    universal: Universal,
}

impl ColumnViewC {
    /// Re-derive `columns` from `header`'s first `view_count` children --
    /// the module doc's own timing note. A `view_count` of zero (nothing
    /// reconciled yet, or a real `column_view` with no columns at all)
    /// leaves the placeholder/previous state alone rather than emptying it,
    /// since an empty `columns` would leave no header to click at all.
    fn place(&self, view_count: usize) {
        if view_count == 0 {
            return;
        }
        let children = self.header.children();
        let real = &children[..view_count.min(children.len())];
        let columns: Vec<ColumnState> = real
            .iter()
            .map(|child| {
                let p = props_of(child);
                let sorter = match p.get(PropName::Sorter) {
                    Some(Prop::Sorter(sorter)) => Some(sorter.clone()),
                    _ => None,
                };
                ColumnState {
                    title: Rc::from(p.str(PropName::Title).unwrap_or_default()),
                    width: DEFAULT_COLUMN_WIDTH,
                    resizable: p.bool(PropName::Resizable, true),
                    expand: p.bool(PropName::Expand, false),
                    sorter,
                    node: child.clone(),
                }
            })
            .collect();
        *self.columns.borrow_mut() = columns;
    }

    /// The column a point (in this controller's own root-node space) lands
    /// in, if any, together with how far the point sits from that column's
    /// trailing edge.
    fn column_at(&self, tree: &LayoutTree, point: (f32, f32)) -> Option<(usize, f32)> {
        self.columns
            .borrow()
            .iter()
            .enumerate()
            .find_map(|(index, column)| {
                let r = local_rect(tree, &self.node, &column.node)?;
                (point.0 >= r.x && point.0 < r.right() && point.1 >= r.y && point.1 < r.bottom())
                    .then(|| (index, r.right() - point.0))
            })
    }

    /// Cycle this column's sort: unsorted -> ascending -> descending ->
    /// ascending ... (GTK never returns to unsorted by clicking).
    fn cycle_sort(&mut self, column: usize) -> (usize, SortOrder) {
        let next = match self.sort {
            Some((c, SortOrder::Ascending)) if c == column => SortOrder::Descending,
            _ => SortOrder::Ascending,
        };
        self.sort = Some((column, next));
        let columns = self.columns.borrow();
        for (i, state) in columns.iter().enumerate() {
            state
                .node
                .set_state(crate::css::node::PseudoStates::CHECKED, i == column);
            state.node.remove_class("ascending");
            state.node.remove_class("descending");
        }
        columns[column].node.add_class(match next {
            SortOrder::Ascending => "ascending",
            SortOrder::Descending => "descending",
        });
        (column, next)
    }

    /// Re-sort the embedded list's model by `column`'s comparator (falling
    /// back to [`Sorter::by_label`] when the column has none of its own --
    /// GTK still lets a click sort an otherwise-plain column by its cell
    /// text), reversing it for [`SortOrder::Descending`].
    ///
    /// Only reassigns `self.list.model`: `ListViewC` keeps its last-known
    /// viewport private (no accessor this controller can re-derive a
    /// `rebind` call from), so the pooled rows already on screen pick up the
    /// new order on the embedded list's own next scroll/`set_metrics` call
    /// rather than this one repainting them immediately -- the same
    /// "metrics not known yet" gap `ListViewC::build`'s own module doc
    /// describes for a widget's very first frame.
    fn apply_sort(&mut self, column: usize, order: SortOrder) {
        let sorter = self
            .columns
            .borrow()
            .get(column)
            .and_then(|c| c.sorter.clone())
            .unwrap_or_else(Sorter::by_label);
        let mut items: Vec<crate::view::ListItem> = self.list.model.to_vec();
        items.sort_by(|a, b| sorter.compare(a, b));
        if order == SortOrder::Descending {
            items.reverse();
        }
        self.list.model = items.into();
    }

    // -- Test/headless hooks. --

    /// The current `(column, order)` sort, if any.
    #[must_use]
    pub fn sort_of<Msg: 'static>(c: &dyn Controller<Msg>) -> Option<(usize, SortOrder)> {
        let any: &dyn std::any::Any = c;
        any.downcast_ref::<Self>().and_then(|me| me.sort)
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for ColumnViewC {
    fn kind(&self) -> Kind {
        Kind::ColumnView
    }

    fn build(node: &Node, props: &Props, cx: &mut BuildCx<'_>) -> Self {
        let header = Node::new("header");
        node.append_child(&header);
        // See the module doc: a placeholder column, sortable like any real
        // one, so `build_widget`'s bare-controller fixture and a widget's
        // very first frame both have something to render and to click.
        let placeholder = Node::with_classes("button", &[]);
        header.append_child(&placeholder);
        let columns = RefCell::new(vec![ColumnState {
            title: Rc::from(""),
            width: DEFAULT_COLUMN_WIDTH,
            resizable: true,
            expand: false,
            sorter: None,
            node: placeholder,
        }]);

        if props.bool(PropName::ShowColumnSeparators, false) {
            node.add_class("column-separators");
        }
        if props.bool(PropName::ShowRowSeparators, false) {
            node.add_class("row-separators");
        }

        let list_node =
            Node::with_classes(Kind::ListView.css_name(), Kind::ListView.base_classes());
        node.append_child(&list_node);
        let mut list_props = Props::default();
        if let Some(model) = props.get(PropName::Model) {
            list_props.set(PropName::Model, model.clone());
        }
        list_props.set(
            PropName::ItemFactory,
            Prop::Factory(ItemFactory::label_only()),
        );
        let list = <ListViewC as Controller<Msg>>::build(&list_node, &list_props, cx);

        Self {
            node: node.clone(),
            columns,
            list,
            list_node,
            sort: None,
            drag: None,
            header,
            universal: Universal::new(node, Kind::ColumnView),
        }
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, cx: &mut BuildCx<'_>) {
        match name {
            PropName::ShowColumnSeparators => {
                if crate::widgets::prop_bool(value, false) {
                    node.add_class("column-separators");
                } else {
                    node.remove_class("column-separators");
                }
            }
            PropName::ShowRowSeparators => {
                if crate::widgets::prop_bool(value, false) {
                    node.add_class("row-separators");
                } else {
                    node.remove_class("row-separators");
                }
            }
            PropName::SortColumn => {
                let column = usize::try_from(crate::widgets::prop_i64(value, -1)).ok();
                if let Some(column) = column {
                    let order = self.sort.map_or(SortOrder::Ascending, |(_, o)| o);
                    self.sort = Some((column, order));
                }
            }
            PropName::Model | PropName::ItemFactory => {
                // The embedded list owns `Model`; its own `ItemFactory`
                // stays `label_only` (module doc), so only `Model` forwards.
                if name == PropName::Model {
                    <ListViewC as Controller<Msg>>::set_prop(
                        &mut self.list,
                        &self.list_node,
                        PropName::Model,
                        value,
                        cx,
                    );
                }
            }
            other => {
                self.universal.apply(node, Kind::ColumnView, other, value);
            }
        }
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        const EDGE: f32 = 4.0;
        let mut out = Vec::new();
        match ev {
            Event::PointerDown { button, local, .. }
                if *button == crate::window::layer::BTN_LEFT =>
            {
                if let Some((column, from_edge)) = self.column_at(cx.tree, *local) {
                    let resizable = self.columns.borrow()[column].resizable;
                    if resizable && from_edge.abs() <= EDGE {
                        self.drag = Some((column, local.0));
                    } else {
                        let (column, order) = self.cycle_sort(column);
                        self.apply_sort(column, order);
                        let text = format!(
                            "{column}:{}",
                            match order {
                                SortOrder::Ascending => "ascending",
                                SortOrder::Descending => "descending",
                            }
                        );
                        if let Some(msg) = cx.handlers.fire_text(EventKind::Change, &text) {
                            out.push(msg);
                        }
                    }
                    cx.handled = true;
                    return out;
                }
            }
            Event::PointerMotion { local } => {
                if let Some((column, start_x)) = self.drag {
                    let delta = local.0 - start_x;
                    let mut columns = self.columns.borrow_mut();
                    if let Some(state) = columns.get_mut(column) {
                        state.width = (state.width + delta).max(1.0);
                    }
                    drop(columns);
                    self.drag = Some((column, local.0));
                    cx.handled = true;
                    return out;
                }
            }
            Event::PointerUp { .. } if self.drag.is_some() => {
                self.drag = None;
                cx.handled = true;
                return out;
            }
            _ => {}
        }
        <ListViewC as Controller<Msg>>::on_event(&mut self.list, ev, cx)
    }

    fn tick(&mut self, now: Duration, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        <ListViewC as Controller<Msg>>::tick(&mut self.list, now, cx)
    }

    fn next_deadline(&self, now: Duration) -> Option<Duration> {
        <ListViewC as Controller<Msg>>::next_deadline(&self.list, now)
    }

    fn reserved_total(&self, view_count: usize) -> usize {
        self.place(view_count);
        view_count
    }
}

#[cfg(test)]
mod tests {
    use crate::view::{Event, EventKind, Handler, Kind, Props};
    use crate::widgets::column_view::ColumnViewC;
    use crate::widgets::types::SortOrder;
    use crate::widgets::{Headless, build_widget, matches_fixture};

    #[test]
    fn a_column_view_contains_a_real_listview_node() {
        // Mutation check: rendering the rows directly under `columnview`
        // fails the fixture, which nests a `listview` -- and node_tree_of
        // renders through it, so this is asserted, not assumed.
        let built = build_widget::<()>(Kind::ColumnView, &Props::default());
        matches_fixture(
            &built.node,
            "columnview[.column-separators][.rich-list][.navigation-sidebar][.data-table]\n\
             ├── header\n│   ╰── <column header>\n├── listview\n╰── [rubberband]\n",
        )
        .expect("column_view fixture");
    }

    #[test]
    fn clicking_a_sortable_header_cycles_ascending_descending_and_reports_it() {
        // Interaction test. Mutation check: toggling without reporting
        // leaves the model sorted the old way while the arrow says otherwise.
        let built = build_widget::<String>(Kind::ColumnView, &Props::default());
        let mut c = built.controller;
        let mut hx = Headless::new();
        // Reconciliation: the task text calls `place_columns` *after*
        // `event_cx_with_handlers`, but every other headless layout helper
        // in this crate (`Headless::place_rows`, and every P6 controller
        // test that uses it) runs its layout pass first and only then
        // borrows `Headless` again for an `EventCx` -- `place_columns`
        // writes into the same `self.tree` an already-live `EventCx`
        // borrows, so calling it after would hold two mutable borrows of
        // `hx` at once and not compile. This calls it first instead, the
        // established order.
        hx.place_columns(&built.node, 120.0);
        let mut cx = hx.event_cx_with_handlers(&built.node, |h| {
            h.set(
                EventKind::Change,
                Handler::Text(std::rc::Rc::new(|s| s.to_string())),
            );
        });
        assert_eq!(
            c.on_event(
                &Event::PointerDown {
                    button: 0x110,
                    local: (10.0, 5.0),
                    serial: 1
                },
                &mut cx
            ),
            vec!["0:ascending".to_string()]
        );
        assert_eq!(
            c.on_event(
                &Event::PointerDown {
                    button: 0x110,
                    local: (10.0, 5.0),
                    serial: 2
                },
                &mut cx
            ),
            vec!["0:descending".to_string()]
        );
        assert_eq!(ColumnViewC::sort_of(&*c), Some((0, SortOrder::Descending)));
    }
}
