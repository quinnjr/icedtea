//! `GtkStackSidebar` -- a `scrolledwindow` over a `navigation-sidebar`
//! [`crate::widgets::list_box::ListBoxC`], one row per `StackC` page.
//!
//! ```text
//! stacksidebar.sidebar
//! ╰── scrolledwindow
//!     ╰── list.navigation-sidebar
//!         ╰── row[.needs-attention]
//!         ┊
//! ```
//!
//! Reconciliation: contract deviation 12 already names the shape --
//! `StackSidebarC { selected: usize, list: ListBoxC }` -- built the same way
//! [`super::stack_switcher::StackSwitcherC`] is: `PropName::Pages`/
//! `PropName::NeedsAttention`/`PropName::Selected` are the only inputs (no
//! live children), so `build` creates the `scrolledwindow`/`list` chrome and
//! one `row` per title itself, *before* handing the `list` node to
//! [`crate::widgets::list_box::ListBoxC::build`] -- the same "attach real
//! children first" accommodation `StackC::place`'s own module doc explains
//! `ListBoxC::build` needs (an empty `list` node makes it invent a
//! placeholder row). `PropName::NeedsAttention`'s bit orientation is
//! `stack_switcher.rs`'s `needs_attention_bit` (page 0 is the mask's high
//! bit); both controllers use the same free function so a mask reads the
//! same way from either widget.
//!
//! `on_event` cannot hand its `Event` to the embedded `ListBoxC` unmodified:
//! `EventCx::tree`'s coordinates are local to *this* controller's own root
//! (`aim`'s hit chain only reaches `StackSidebar`'s `Instance`, never the
//! `scrolledwindow`/`list` chrome nested inside it), while `ListBoxC::
//! row_at` hit-tests rows against its own `node` (the `list`). Every other
//! controller with a nested subnode -- `DropDownC`'s popover list,
//! `MenuButtonC`'s popover, `EntryC`'s icons -- re-expresses the event with
//! `crate::widgets::shift_event` over `crate::widgets::local_rect`
//! first; this one does the same before delegating.

use std::rc::Rc;

use crate::css::node::Node;
use crate::view::{BuildCx, Controller, Event, EventCx, Kind, Prop, PropName, Props};
use crate::widgets::list_box::ListBoxC;
use crate::widgets::stack_switcher::needs_attention_bit;
use crate::widgets::{Universal, local_rect, prop_i64, shift_event};

/// A fresh sidebar `row`. Navigation-sidebar rows are activatable; the rows
/// present at [`StackSidebarC::build`] pick that class up from
/// [`ListBoxC::build`], so a row a later reconcile adds must carry it too or
/// an added page would render differently from a page that was there at
/// build.
fn new_row() -> Node {
    let row = Node::new("row");
    row.add_class("activatable");
    row
}

/// Fill an empty `list_node` with one `row` per `titles`, flagging
/// `.needs-attention` per [`needs_attention_bit`]. Only for the first build,
/// when the `list` has no rows yet -- every live update runs
/// [`StackSidebarC::reconcile_pages`], which reuses row nodes in place.
fn build_rows(list_node: &Node, titles: &[Rc<str>], needs_attention: i64) {
    for old in list_node.children() {
        old.detach();
    }
    let total = titles.len();
    for i in 0..total {
        let row = new_row();
        if needs_attention_bit(needs_attention, i, total) {
            row.add_class("needs-attention");
        }
        list_node.append_child(&row);
    }
}

/// `GtkStackSidebar`.
pub struct StackSidebarC {
    node: Node,
    /// The `list` chrome node `list` owns.
    list_node: Node,
    /// Index of the selected row, mirrored from `list.selection`.
    pub selected: usize,
    /// The embedded `GtkListBox` of pages.
    pub list: ListBoxC,
    titles: Vec<Rc<str>>,
    needs_attention: i64,
    universal: Universal,
}

impl StackSidebarC {
    fn titles_from(props: &Props) -> Vec<Rc<str>> {
        match props.get(PropName::Pages) {
            Some(Prop::Classes(list)) => list.to_vec(),
            _ => Vec::new(),
        }
    }

    fn apply_selected(&mut self) {
        self.list.selection.select(self.selected);
        self.list.apply_selection();
    }

    /// Reconcile the `list`'s rows against `new_titles` IN PLACE. Every title
    /// carried over from `self.titles` reuses its existing row node (matched
    /// by value, first unused occurrence); only a genuinely new title mints a
    /// row, and a removed page's row is detached. Rows are then re-ordered to
    /// match `new_titles` and their `.needs-attention` class recomputed for
    /// the new order.
    ///
    /// Selection follows the selected page's row *node identity*, not its raw
    /// index, so it stays on the same page across a reorder and is *dropped*
    /// (not silently handed to whatever page slid into the old index) when the
    /// selected page is removed. This is what keeps the embedded [`ListBoxC`]'s
    /// per-row state from being evicted on every `Pages`/`NeedsAttention`
    /// write, the way the wholesale detach-and-rebuild it replaces did.
    ///
    /// `self.needs_attention` must already hold its new value; `self.titles`
    /// is read as the *old* order and overwritten here.
    fn reconcile_pages(&mut self, new_titles: Vec<Rc<str>>) {
        let old_rows = self.list_node.children();
        // `self.selected` doubles `0` as both a real index and its "nothing
        // selected" value, so only trust it while the selection is non-empty;
        // otherwise a later reconcile would read `old_rows.get(0)` back and
        // resurrect a selection onto page 0 that the user never picked.
        let selected_row = (!self.list.selection.is_empty())
            .then(|| old_rows.get(self.selected))
            .flatten()
            .cloned();

        // Pool of reusable (old title, node), each consumable once.
        let mut pool: Vec<(Rc<str>, Node)> = self.titles.iter().cloned().zip(old_rows).collect();
        let mut used = vec![false; pool.len()];

        let mut new_rows: Vec<Node> = Vec::with_capacity(new_titles.len());
        for title in &new_titles {
            match pool
                .iter()
                .enumerate()
                .position(|(i, (t, _))| !used[i] && t == title)
            {
                Some(i) => {
                    used[i] = true;
                    new_rows.push(pool[i].1.clone());
                }
                None => new_rows.push(new_row()),
            }
        }
        // Detach the rows of pages that went away.
        for (i, (_, node)) in pool.drain(..).enumerate() {
            if !used[i] {
                node.detach();
            }
        }
        // Re-attach in the new order. `insert_child` detaches-then-inserts, so
        // a reused row that only moved is placed without being recreated.
        for (i, row) in new_rows.iter().enumerate() {
            self.list_node.insert_child(i, row);
        }
        // `.needs-attention` tracks the new ordering, on reused and new rows.
        let total = new_rows.len();
        for (i, row) in new_rows.iter().enumerate() {
            if needs_attention_bit(self.needs_attention, i, total) {
                row.add_class("needs-attention");
            } else {
                row.remove_class("needs-attention");
            }
        }

        self.titles = new_titles;
        self.list.rows = new_rows;

        // Re-find the selection by the selected page's row node.
        match selected_row.and_then(|sel| self.list.rows.iter().position(|r| r.ptr_eq(&sel))) {
            Some(index) => {
                self.selected = index;
                self.list.selection.select(index);
            }
            None => {
                self.selected = 0;
                self.list.selection.clear();
            }
        }
        self.list.selection.retain_below(self.list.rows.len());
        self.list.apply_selection();
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for StackSidebarC {
    fn kind(&self) -> Kind {
        Kind::StackSidebar
    }

    fn build(node: &Node, props: &Props, cx: &mut BuildCx<'_>) -> Self {
        let scrolled = Node::new("scrolledwindow");
        node.append_child(&scrolled);
        let list_node = Node::with_classes("list", &["navigation-sidebar"]);
        scrolled.append_child(&list_node);

        let titles = Self::titles_from(props);
        let needs_attention = props.int(PropName::NeedsAttention, 0);
        build_rows(&list_node, &titles, needs_attention);
        let list = <ListBoxC as Controller<Msg>>::build(&list_node, &Props::default(), cx);
        let selected = usize::try_from(props.int(PropName::Selected, 0).max(0)).unwrap_or(0);

        let mut me = Self {
            node: node.clone(),
            list_node,
            selected,
            list,
            titles,
            needs_attention,
            universal: Universal::new(node, Kind::StackSidebar),
        };
        me.apply_selected();
        me
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, cx: &mut BuildCx<'_>) {
        match (name, value) {
            (PropName::Pages, Prop::Classes(list)) => {
                self.reconcile_pages(list.to_vec());
            }
            (PropName::NeedsAttention, other) => {
                self.needs_attention = prop_i64(other, 0);
                // Same titles, new mask: reconcile reuses every row and just
                // recomputes each row's `.needs-attention`.
                self.reconcile_pages(self.titles.clone());
            }
            (PropName::Selected, other) => {
                self.selected = usize::try_from(prop_i64(other, 0).max(0)).unwrap_or(0);
                self.apply_selected();
            }
            _ => {
                self.universal.apply(node, Kind::StackSidebar, name, value);
            }
        }
        let _ = cx;
    }

    /// The `scrolledwindow` chrome is this controller's own node child, built
    /// before any view child could arrive, so a view child of a
    /// `StackSidebar` would start after it.
    fn child_index(&self, view_index: usize) -> usize {
        view_index + 1
    }

    /// ... and reconcile's trim step has to know it is there, or it detaches
    /// it the moment it runs -- a `StackSidebar` takes no view children (its
    /// pages arrive as `PropName::Pages`), so its single `scrolledwindow`
    /// child sits past the default trim bound of `view_count` and is evicted
    /// on the first reconcile, taking the whole `list`/rows subtree with it.
    /// This is `StackSwitcherC`'s pair (P8-D72), for the same reason and in
    /// the same shape: reserve the controller's own chrome against the trim.
    fn reserved_total(&self, view_count: usize) -> usize {
        view_count + 1
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        let Some(rect) = local_rect(cx.tree, &self.node, &self.list_node) else {
            return Vec::new();
        };
        let shifted = shift_event(ev, rect);
        let out = <ListBoxC as Controller<Msg>>::on_event(&mut self.list, &shifted, cx);
        if let Some(index) = self.list.selection.first() {
            self.selected = index;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use crate::css::node::{Node, PseudoStates};
    use crate::view::{Kind, Prop, PropName, Props};
    use crate::widgets::{Headless, build_widget, matches_fixture};

    /// The `row` children of a built sidebar, in order:
    /// `stacksidebar > scrolledwindow > list > row*`.
    fn rows(node: &Node) -> Vec<Node> {
        node.child(0)
            .and_then(|s| s.child(0))
            .map(|l| l.children())
            .unwrap_or_default()
    }

    fn pages(titles: &[&str]) -> Prop {
        Prop::Classes(titles.iter().map(|s| (*s).into()).collect())
    }

    fn is_selected(row: &Node) -> bool {
        row.states().contains(PseudoStates::SELECTED)
    }

    fn has_needs_attention(row: &Node) -> bool {
        row.classes()
            .iter()
            .any(|c| c.as_str() == "needs-attention")
    }

    #[test]
    fn the_sidebar_is_a_scrolledwindow_over_a_navigation_sidebar_list() {
        // Mutation check: putting the list straight under `stacksidebar`
        // loses the scrolling a long page list needs, and the fixture's
        // `scrolledwindow > list.navigation-sidebar` chain.
        let mut props = Props::default();
        props.set(
            PropName::Pages,
            Prop::Classes(["A", "B"].iter().map(|s| (*s).into()).collect()),
        );
        let built = build_widget::<()>(Kind::StackSidebar, &props);
        matches_fixture(
            &built.node,
            "stacksidebar.sidebar\n╰── scrolledwindow\n    ╰── list.navigation-sidebar\n        \
             ╰── row[.needs-attention]\n        ┊\n",
        )
        .expect("stack_sidebar fixture");
    }

    #[test]
    fn a_page_marked_needs_attention_carries_that_class_on_its_row() {
        // Mutation check: setting the class on the list instead of the row
        // makes the whole sidebar pulse.
        let mut props = Props::default();
        props.set(
            PropName::Pages,
            Prop::Classes(["A", "B"].iter().map(|s| (*s).into()).collect()),
        );
        props.set(PropName::NeedsAttention, Prop::Int(1));
        let built = build_widget::<()>(Kind::StackSidebar, &props);
        let row = built
            .node
            .child(0)
            .and_then(|s| s.child(0))
            .and_then(|l| l.child(1));
        assert!(
            row.expect("row")
                .classes()
                .iter()
                .any(|c| c.as_str() == "needs-attention")
        );
    }

    #[test]
    fn a_pages_update_reuses_rows_and_keeps_the_selected_page_selected_by_identity() {
        // The defect: `build_rows` detached every row and rebuilt from
        // scratch on any `Pages`/`NeedsAttention` write, evicting per-row
        // state, and `apply_selected` re-selected by raw index -- so a
        // selected page silently shifted when the list reordered.
        //
        // Mutation check: revert the reconcile to detach-all-then-rebuild
        // (fresh `Node::new("row")` for every page) and the selected page's
        // old row node is gone from the new list, so selection falls back to
        // index 0 -- landing on "C", not the still-present "B" -- and every
        // `ptr_eq` reuse assertion fails.
        let mut props = Props::default();
        props.set(PropName::Pages, pages(&["A", "B", "C"]));
        props.set(PropName::Selected, Prop::Int(1));
        let built = build_widget::<()>(Kind::StackSidebar, &props);

        let before = rows(&built.node);
        assert_eq!(before.len(), 3);
        assert!(is_selected(&before[1]), "the 2nd page starts selected");
        let (row_a, row_b, row_c) = (before[0].clone(), before[1].clone(), before[2].clone());

        // Drop "A", reorder to ["C", "B", "D"], add "D": "B" survives at a
        // new, non-zero index.
        let mut c = built.controller;
        let mut hx = Headless::new();
        c.set_prop(
            &built.node,
            PropName::Pages,
            &pages(&["C", "B", "D"]),
            &mut hx.cx(),
        );
        // ... and flag page 0 ("C") for attention (high bit of a 3-page mask).
        c.set_prop(
            &built.node,
            PropName::NeedsAttention,
            &Prop::Int(0b100),
            &mut hx.cx(),
        );

        let after = rows(&built.node);
        assert_eq!(after.len(), 3, "row content is intact: three pages");

        // (b) rows for unchanged pages were reused, not recreated.
        assert!(after[0].ptr_eq(&row_c), "C's row node was reused");
        assert!(after[1].ptr_eq(&row_b), "B's row node was reused");
        assert!(
            !after.iter().any(|r| r.ptr_eq(&row_a)),
            "A's row was removed"
        );
        assert!(row_a.parent().is_none(), "A's removed row was detached");

        // (a) the previously-selected page is still selected, by identity.
        assert!(after[1].ptr_eq(&row_b));
        assert!(
            is_selected(&after[1]),
            "B stays selected across the reorder"
        );
        assert!(!is_selected(&after[0]), "C did not inherit the selection");
        assert!(!is_selected(&after[2]), "D is not selected");

        // (c) needs-attention tracks the new order, on the reused rows.
        assert!(has_needs_attention(&after[0]), "C (page 0) is flagged");
        assert!(!has_needs_attention(&after[1]));
        assert!(!has_needs_attention(&after[2]));
    }

    #[test]
    fn removing_the_selected_page_drops_the_selection_rather_than_shifting_it() {
        // A selected page that is deleted must not silently hand its
        // selection to whatever page slid into its old index.
        let mut props = Props::default();
        props.set(PropName::Pages, pages(&["A", "B", "C"]));
        props.set(PropName::Selected, Prop::Int(1));
        let built = build_widget::<()>(Kind::StackSidebar, &props);
        let removed = rows(&built.node)[1].clone();

        let mut c = built.controller;
        let mut hx = Headless::new();
        c.set_prop(
            &built.node,
            PropName::Pages,
            &pages(&["A", "C"]),
            &mut hx.cx(),
        );

        assert!(removed.parent().is_none(), "B's row was detached");
        let after = rows(&built.node);
        assert_eq!(after.len(), 2);
        assert!(
            !after.iter().any(|r| r.ptr_eq(&removed)),
            "the removed page's row is gone"
        );
        assert!(
            !is_selected(&after[0]) && !is_selected(&after[1]),
            "the selection was dropped, not shifted onto another page"
        );

        // A SECOND reconcile that must not touch selection (a `NeedsAttention`
        // write reuses every row) must NOT resurrect the dropped selection onto
        // page 0. The `selected: usize` mirror uses `0` both as a valid index
        // and as its "nothing selected" value, so a naive reconcile reads
        // `old_rows.get(0)` back and re-selects the page the user never picked.
        c.set_prop(
            &built.node,
            PropName::NeedsAttention,
            &Prop::Int(0),
            &mut hx.cx(),
        );
        let after2 = rows(&built.node);
        assert_eq!(after2.len(), 2);
        assert!(
            !is_selected(&after2[0]) && !is_selected(&after2[1]),
            "an empty selection stays empty across a later reconcile"
        );
    }

    #[test]
    fn the_sidebar_reserves_its_scrolledwindow_against_reconcile_trim() {
        // A `StackSidebar` takes no view children, so reconcile's trim step
        // would detach its `scrolledwindow` chrome the first frame -- the
        // same defect P8-D72 fixed for `StackSwitcher` -- unless the
        // controller reserves it. Mutation check: drop the
        // `child_index`/`reserved_total` overrides and the reconciled node
        // has no children at all.
        use crate::view::builders::stack_sidebar;
        use crate::widgets::StackPageInfo;

        let pages_info: Rc<[StackPageInfo]> = ["A", "B", "C"]
            .iter()
            .map(|t| StackPageInfo {
                name: (*t).into(),
                title: (*t).into(),
                icon: None,
                needs_attention: false,
            })
            .collect();
        let view = stack_sidebar::<()>(pages_info);

        let mut hx = Headless::new();
        let root = Node::new("window");
        let mut instances: Vec<crate::view::reconcile::Instance<()>> = Vec::new();
        {
            let mut cx = hx.cx();
            crate::view::reconcile::reconcile(&root, &mut instances, vec![view], &mut cx);
        }
        let sidebar = root.child(0).expect("the sidebar instance's node");
        let scrolled = sidebar.child(0).expect("scrolledwindow survived trim");
        assert_eq!(scrolled.name().as_ref(), "scrolledwindow");
        assert_eq!(
            rows(&sidebar).len(),
            3,
            "all three page rows survived the reconcile"
        );
    }
}
