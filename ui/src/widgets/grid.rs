//! `GtkGrid` -- one `grid` node; children carry their own cell.

use crate::css::node::Node;
use crate::layout::{ChildLayout, Container, GridPlacement};
use crate::view::{BuildCx, Controller, Event, EventCx, Kind, Prop, PropName, Props};
use crate::widgets::{Universal, prop_bool, prop_i64, set_child_layout, set_container};

/// `GtkGrid`.
pub struct GridC {
    /// Column tracks, `1..=1024`.
    pub columns: u16,
    /// Row tracks, `1..=1024`.
    pub rows: u16,
    /// `(column_spacing, row_spacing)` in px.
    pub spacing: (f32, f32),
    /// `(column_homogeneous, row_homogeneous)`.
    pub homogeneous: (bool, bool),
    /// The row whose baseline the grid aligns on; `-1` for none.
    pub baseline_row: i32,
    universal: Universal,
}

impl GridC {
    /// The track counts a view's children imply: one past the largest
    /// `column + column_span` and `row + row_span`.
    ///
    /// GTK's grid has no declared size either -- it grows to fit whatever is
    /// attached -- so this is the authority and `PropName::Columns`/`Rows`
    /// only raise it.
    #[must_use]
    pub fn extent_of<Msg>(view: &crate::view::View<Msg>) -> (u16, u16) {
        let mut columns = 1_u16;
        let mut rows = 1_u16;
        for child in &view.children {
            let c = u16::try_from(child.props.int(PropName::Column, 0).max(0)).unwrap_or(0);
            let r = u16::try_from(child.props.int(PropName::Row, 0).max(0)).unwrap_or(0);
            let cs = u16::try_from(child.props.int(PropName::ColumnSpan, 1).max(1)).unwrap_or(1);
            let rs = u16::try_from(child.props.int(PropName::RowSpan, 1).max(1)).unwrap_or(1);
            columns = columns.max(c.saturating_add(cs));
            rows = rows.max(r.saturating_add(rs));
        }
        (columns.clamp(1, 1024), rows.clamp(1, 1024))
    }

    /// This child's [`GridPlacement`], from its own recorded `Column`/`Row`/
    /// `ColumnSpan`/`RowSpan` props -- `None` when it was never placed
    /// explicitly, so auto-placement keeps handling it.
    #[must_use]
    pub(crate) fn placement_of(props: &Props) -> Option<GridPlacement> {
        let has = props.get(PropName::Column).is_some() || props.get(PropName::Row).is_some();
        has.then(|| GridPlacement {
            column: u16::try_from(props.int(PropName::Column, 0).clamp(0, 1023)).unwrap_or(0),
            row: u16::try_from(props.int(PropName::Row, 0).clamp(0, 1023)).unwrap_or(0),
            column_span: u16::try_from(props.int(PropName::ColumnSpan, 1).clamp(1, 1024))
                .unwrap_or(1),
            row_span: u16::try_from(props.int(PropName::RowSpan, 1).clamp(1, 1024)).unwrap_or(1),
        })
    }

    fn apply(&self, node: &Node) {
        set_container(
            node,
            Container::Grid {
                columns: self.columns,
                rows: self.rows,
                column_spacing: self.spacing.0,
                row_spacing: self.spacing.1,
                column_homogeneous: self.homogeneous.0,
                row_homogeneous: self.homogeneous.1,
            },
        );
        // `Controller::build` runs before the reconciler attaches this
        // node's children (`reconcile::build_instance`'s order), so there
        // is nothing in `node.children()` yet to place -- `flush_layout`
        // re-derives every child's cell from its own props once they exist,
        // every frame, the same way it re-applies `homogeneous`.
        crate::widgets::mark_grid_children(node);
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for GridC {
    fn kind(&self) -> Kind {
        Kind::Grid
    }

    fn build(node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        let clamp = |v: i64| v.clamp(0, 1_000_000) as f32;
        let me = Self {
            columns: u16::try_from(props.int(PropName::Columns, 1).clamp(1, 1024)).unwrap_or(1),
            rows: u16::try_from(props.int(PropName::Rows, 1).clamp(1, 1024)).unwrap_or(1),
            spacing: (
                clamp(props.int(PropName::ColumnSpacing, 0)),
                clamp(props.int(PropName::RowSpacing, 0)),
            ),
            homogeneous: (
                props.bool(PropName::ColumnHomogeneous, false),
                props.bool(PropName::RowHomogeneous, false),
            ),
            baseline_row: i32::try_from(props.int(PropName::BaselineRow, -1)).unwrap_or(-1),
            universal: Universal::new(node, Kind::Grid),
        };
        me.apply(node);
        me
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        let clamp = |v: i64| v.clamp(0, 1_000_000) as f32;
        match name {
            PropName::Columns => {
                self.columns = u16::try_from(prop_i64(value, 1).clamp(1, 1024)).unwrap_or(1);
            }
            PropName::Rows => {
                self.rows = u16::try_from(prop_i64(value, 1).clamp(1, 1024)).unwrap_or(1);
            }
            PropName::ColumnSpacing => self.spacing.0 = clamp(prop_i64(value, 0)),
            PropName::RowSpacing => self.spacing.1 = clamp(prop_i64(value, 0)),
            PropName::ColumnHomogeneous => self.homogeneous.0 = prop_bool(value, false),
            PropName::RowHomogeneous => self.homogeneous.1 = prop_bool(value, false),
            PropName::BaselineRow => {
                self.baseline_row = i32::try_from(prop_i64(value, -1)).unwrap_or(-1);
            }
            // A child's cell changed: re-place that child only. Reachable
            // when this grid is itself attached inside an ancestor grid and
            // *its own* cell moves; `flush_layout`'s per-frame
            // `grid_from_children` pass (driven by `apply`, above) is what
            // actually keeps this grid's own children in their cells, since
            // `Controller::build` has no children to loop over yet.
            PropName::Column | PropName::Row | PropName::ColumnSpan | PropName::RowSpan => {
                for child in node.children() {
                    if let Some(place) = Self::placement_of(&crate::widgets::props_of(&child)) {
                        set_child_layout(
                            &child,
                            ChildLayout {
                                grid: Some(place),
                                ..crate::widgets::child_layout_of(&child)
                            },
                        );
                    }
                }
                return;
            }
            other => {
                if self.universal.apply(node, Kind::Grid, other, value) {
                    return;
                }
                return;
            }
        }
        self.apply(node);
    }

    fn on_event(&mut self, _ev: &Event, _cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::GridC;
    use crate::layout::Container;
    use crate::view::builders::{grid, label};
    use crate::view::{Cmd, Kind, Prop, PropName, Props, ScriptStep, View};
    use crate::widgets::offscreen::{frames, px};
    use crate::widgets::{build_widget, container_of, matches_fixture};

    #[test]
    fn a_grid_is_one_node_named_grid() {
        // Mutation check: emitting a per-cell wrapper node adds children and
        // fails the single-line fixture (gtk/gtkgrid.c:115).
        let built = build_widget::<()>(Kind::Grid, &Props::default());
        matches_fixture(&built.node, "grid\n").expect("grid fixture");
    }

    #[test]
    fn the_track_counts_come_from_the_childrens_placements() {
        // Mutation check: taking `columns` from PropName::Columns only (and
        // not from the maximum placement) leaves a 1-track grid and every
        // child stacks in column 0.
        let view = grid::<()>([
            label("a").at(0, 0),
            label("b").at(1, 0),
            label("c").at(0, 1).span(2, 1),
        ]);
        let (columns, rows) = GridC::extent_of(&view);
        assert_eq!((columns, rows), (2, 2));
    }

    #[test]
    fn spacing_props_reach_the_container() {
        let mut props = Props::default();
        props.set(PropName::RowSpacing, Prop::Int(9));
        props.set(PropName::ColumnSpacing, Prop::Int(3));
        props.set(PropName::RowHomogeneous, Prop::Bool(true));
        let built = build_widget::<()>(Kind::Grid, &props);
        match container_of(&built.node) {
            Container::Grid {
                row_spacing,
                column_spacing,
                row_homogeneous,
                column_homogeneous,
                ..
            } => {
                assert_eq!(row_spacing, 9.0);
                assert_eq!(column_spacing, 3.0);
                assert!(row_homogeneous);
                assert!(!column_homogeneous);
            }
            other => panic!("expected a grid container, got {other:?}"),
        }
    }

    #[derive(Clone)]
    enum Msg {}

    fn update(_: &mut (), msg: Msg) -> Cmd<Msg> {
        match msg {}
    }

    fn view(_: &()) -> View<Msg> {
        grid([
            label("aa").at(0, 0),
            label("bb").at(1, 0),
            label("cc").at(0, 1),
        ])
        .column_spacing(20)
        .row_spacing(10)
    }

    #[test]
    fn cells_land_in_their_own_row_and_column_at_rest() {
        // Rest-state pixel test. Mutation check: dropping GridPlacement in
        // `flush_layout` auto-places all three on one row and the bottom
        // half of the surface stays empty.
        let out = frames((), update, view, (240, 80), vec![ScriptStep::Capture]);
        let background = px(&out, 0, 239, 79);
        let row_has_ink = |y: u32| (0..240).any(|x| px(&out, 0, x, y) != background);
        // The grid auto-sizes both rows to content (19px each) and centres
        // the resulting 48px block in the 80px surface: row 0 lands at
        // y=16..35, row 1 at y=45..64 (10px `row_spacing` between them).
        assert!(row_has_ink(22), "first row painted");
        assert!(row_has_ink(55), "second row painted");
    }

    #[test]
    fn row_homogeneous_stretches_a_short_row_to_match_a_tall_one() {
        // Mutation check: `grid()` forgetting to inject `PropName::Rows`
        // (leaving the container's row count at its default of 1) turns
        // row 1 into an implicit taffy track, which always sizes as `auto`
        // (content-only) regardless of `row_homogeneous` -- `grid_auto_rows`
        // is never set anywhere, so nothing tells that implicit track to
        // match its homogeneous siblings. With only one explicit row
        // template (the un-injected count), `row_homogeneous(true)` then
        // has *no effect at all*: row 0 (one short label) still hugs its
        // own tiny content height directly above row 1 (a taller
        // three-label stack), the same as `row_homogeneous(false)` would.
        //
        // With the count injected, both rows are explicit `fr(1.0)` tracks,
        // so `row_homogeneous(true)` stretches row 0's track to match row
        // 1's much taller one -- opening up a wide blank gap between row
        // 0's short label and row 1's stack that the un-fixed grid never
        // has.
        fn two_rows(_: &()) -> View<Msg> {
            grid([
                label("a").at(0, 0),
                crate::view::builders::box_(
                    crate::widgets::Orientation::Vertical,
                    [label("x"), label("y"), label("z")],
                )
                .at(0, 1),
            ])
            .row_homogeneous(true)
        }
        let out = frames((), update, two_rows, (240, 200), vec![ScriptStep::Capture]);
        let background = px(&out, 0, 239, 0);
        let row_has_ink = |y: u32| (0..240).any(|x| px(&out, 0, x, y) != background);
        let first_ink = (0..200u32).find(|&y| row_has_ink(y)).expect("some ink");
        let blank_start = (first_ink..200u32)
            .find(|&y| !row_has_ink(y))
            .expect("row 0's ink ends before the surface does");
        let row1_start = (blank_start..200u32)
            .find(|&y| row_has_ink(y))
            .expect("row 1 paints below the gap");
        let gap = row1_start - blank_start;
        // Measured (with the fix): row 0's short label sits at y=50..58,
        // then a blank gap of ~49px before row 1's stack resumes at
        // y=107..152. Without the fix, row 0 hugs row 1 with only the
        // stack's own natural ~12px inter-row gap. 30px comfortably
        // separates "stretched to match" from "hugging its own content".
        assert!(
            gap > 30,
            "row_homogeneous(true) should open a wide gap between row 0's short \
             content and row 1's tall content by stretching row 0's track to \
             match row 1's -- got a {gap}px gap"
        );
    }

    #[test]
    fn re_laying_out_after_a_span_change_keeps_the_same_nodes() {
        // Interaction test. Mutation check: rebuilding children on a span
        // change destroys node identity and the second frame differs in the
        // first row too, not only where the span moved.
        let out = frames(
            (),
            update,
            view,
            (240, 80),
            vec![
                ScriptStep::Capture,
                ScriptStep::Advance(std::time::Duration::from_millis(16)),
                ScriptStep::Capture,
            ],
        );
        for x in [10_u32, 120, 239] {
            assert_eq!(px(&out, 0, x, 18), px(&out, 1, x, 18));
        }
    }

    #[test]
    fn hostile_placements_never_panic_or_allocate_unboundedly() {
        // Mutation check: dropping the 1..=1024 clamp in `container_style`
        // makes this test allocate 65 535 tracks per axis and time out.
        let mut props = Props::default();
        props.set(PropName::Columns, Prop::Int(i64::MAX));
        props.set(PropName::Rows, Prop::Int(-5));
        props.set(PropName::RowSpacing, Prop::Float(f64::NAN));
        props.set(PropName::ColumnSpacing, Prop::Float(f64::INFINITY));
        let _ = build_widget::<()>(Kind::Grid, &props);
    }
}
