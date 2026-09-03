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

/// Replace `list_node`'s children with one `row` per `titles`, flagging
/// `.needs-attention` per [`needs_attention_bit`].
fn build_rows(list_node: &Node, titles: &[Rc<str>], needs_attention: i64) {
    for old in list_node.children() {
        old.detach();
    }
    let total = titles.len();
    for i in 0..total {
        let row = Node::new("row");
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
                self.titles = list.to_vec();
                build_rows(&self.list_node, &self.titles, self.needs_attention);
                self.list.rows = self.list_node.children();
                self.list.selection.retain_below(self.list.rows.len());
                self.apply_selected();
            }
            (PropName::NeedsAttention, other) => {
                self.needs_attention = prop_i64(other, 0);
                build_rows(&self.list_node, &self.titles, self.needs_attention);
                self.list.rows = self.list_node.children();
                self.list.selection.retain_below(self.list.rows.len());
                self.apply_selected();
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
    use crate::view::{Kind, Prop, PropName, Props};
    use crate::widgets::{build_widget, matches_fixture};

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
}
