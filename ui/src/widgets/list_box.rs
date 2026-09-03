//! `GtkListBox` -- a `list` of `row`s with GTK's four selection modes.
//!
//! ```text
//! list[.separators][.rich-list][.navigation-sidebar][.boxed-list]
//! ╰── row[.activatable]
//! ```
//!
//! Reconciliation: the part plan's snippet for this controller assumed a
//! free `hit(row, point)` helper and an `apply_universal_prop` function
//! that this crate does not have -- P6's actual universal-prop convention,
//! established by every widget controller before this one (`ButtonC`,
//! `NotebookC`, ...), is the incremental [`Universal`] type applied as
//! `self.universal.apply(node, Kind::ListBox, name, value)`, so `set_prop`
//! uses that instead. `row_at` takes the real `&LayoutTree` from
//! [`EventCx::tree`] and hit-tests through `crate::widgets::local_rect`
//! (the same subnode-rect helper every other controller with owned
//! subnodes uses) rather than a standalone `hit` free function, because a
//! `Node` alone carries no geometry in this crate -- only a `LayoutTree`
//! does. The plan's `Headless::place_rows(&Node, height)` runs a real,
//! minimal layout pass (`LayoutTree::sync`/`compute` with a
//! [`crate::layout::FixedMeasure`]) rather than writing to some synthetic
//! side table, so it needs `&mut Headless`; the two tests that call it
//! therefore call it *before* building their `EventCx` (which borrows
//! `Headless` for as long as it lives), not after, since an `EventCx`
//! already in scope holds `Headless` mutably borrowed and a second
//! `place_rows` call on it would not compile. Finally, `build_widget`
//! (used directly by the fixture test) never attaches real children to the
//! node it builds -- unlike a real `list_box(rows)` view, whose rows are
//! always real `Kind::ListBoxRow` children in place before `Controller::
//! build` ever runs -- so `build` appends one placeholder `row` itself
//! when it finds none, the same accommodation `NotebookC::resize_pages`
//! makes (there via `PropName::Pages`) for its own bare-controller fixture
//! test; this is dead code for every real widget instance.
use crate::css::node::{Node, PseudoStates};
use crate::layout::{BoxDirection, Container, LayoutTree};
use crate::view::{BuildCx, Controller, Event, EventCx, EventKind, Kind, Prop, PropName, Props};
use crate::widgets::types::{Selection, SelectionMode};
use crate::widgets::{Universal, local_rect, prop_bool, prop_u16, set_container};

/// `GtkListBox`.
pub struct ListBoxC {
    /// The `list` node itself, for [`ListBoxC::row_at`]'s geometry lookups.
    node: Node,
    /// One `row` node per child, in order.
    pub rows: Vec<Node>,
    /// What is selected.
    pub selection: Selection,
    /// Keyboard cursor.
    pub cursor: Option<usize>,
    /// The current mode, mirrored from `selection` for readability.
    pub mode: SelectionMode,
    /// `GtkListBox:activate-on-single-click`.
    pub activate_single: bool,
    universal: Universal,
}

impl ListBoxC {
    /// Test hook: the selected model indices.
    #[must_use]
    pub fn selected_rows<Msg: 'static>(c: &dyn Controller<Msg>) -> Vec<usize> {
        let any: &dyn std::any::Any = c;
        any.downcast_ref::<Self>()
            .map_or_else(Vec::new, |me| me.selection.selected())
    }

    /// The row a point (in this controller's own root-node space) lands in.
    #[must_use]
    pub fn row_at(&self, tree: &LayoutTree, point: (f32, f32)) -> Option<usize> {
        self.rows.iter().position(|row| {
            local_rect(tree, &self.node, row).is_some_and(|r| {
                point.0 >= r.x && point.0 < r.right() && point.1 >= r.y && point.1 < r.bottom()
            })
        })
    }

    /// Push the selection onto the rows' `:selected` state.
    pub fn apply_selection(&self) {
        for (i, row) in self.rows.iter().enumerate() {
            row.set_state(PseudoStates::SELECTED, self.selection.contains(i));
        }
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for ListBoxC {
    fn kind(&self) -> Kind {
        Kind::ListBox
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
        let mut rows: Vec<Node> = node.children();
        if rows.is_empty() {
            // See the module doc: the bare-controller harness never attaches
            // real children, so the conformance fixture's one `row` line can
            // only be satisfied by a placeholder this controller creates.
            let row = Node::new("row");
            node.append_child(&row);
            rows.push(row);
        }
        for row in &rows {
            if crate::widgets::props_of(row).bool(PropName::Activatable, true) {
                row.add_class("activatable");
            }
        }
        Self {
            node: node.clone(),
            rows,
            selection: Selection::new(mode),
            cursor: None,
            mode,
            activate_single: props.bool(PropName::ActivateOnSingleClick, true),
            universal: Universal::new(node, Kind::ListBox),
        }
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        match name {
            PropName::SelectionMode => {
                self.mode = SelectionMode::from_u16(prop_u16(value, 1));
                self.selection.set_mode(self.mode);
                self.apply_selection();
            }
            PropName::ShowSeparators => {
                if prop_bool(value, false) {
                    node.add_class("separators");
                } else {
                    node.remove_class("separators");
                }
            }
            PropName::ActivateOnSingleClick => self.activate_single = prop_bool(value, true),
            other => {
                self.universal.apply(node, Kind::ListBox, other, value);
                return;
            }
        }
        // The row list is the node's children; a reconcile may have changed
        // it, and re-reading is cheaper than tracking every insertion.
        self.rows = node.children();
        self.selection.retain_below(self.rows.len());
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
                if let Some(index) = self.row_at(cx.tree, *local) {
                    self.cursor = Some(index);
                    changed = self.selection.select(index);
                    cx.handled = true;
                    if self.activate_single
                        && let Some(msg) = cx.handlers.fire_index(EventKind::Activate, index)
                    {
                        out.push(msg);
                    }
                }
            }
            Event::Key(key) if key.pressed => {
                let count = self.rows.len();
                let sym = u32::from(key.keysym);
                let ctrl = key.mods.contains(Mods::CTRL);
                match sym {
                    // Under `Multiple`, arrow keys move the keyboard cursor
                    // only -- GTK reserves actually changing the selection
                    // there for Space/click/Ctrl+A, so a plain Down never
                    // grows a multi-selection on its own.
                    keysyms::KEY_Down => {
                        self.cursor = Some(
                            self.cursor
                                .map_or(0, |c| (c + 1).min(count.saturating_sub(1))),
                        );
                        if self.mode != SelectionMode::Multiple {
                            changed = self.selection.select(self.cursor.unwrap_or(0));
                        }
                        cx.handled = true;
                    }
                    keysyms::KEY_Up => {
                        self.cursor = Some(self.cursor.map_or(0, |c| c.saturating_sub(1)));
                        if self.mode != SelectionMode::Multiple {
                            changed = self.selection.select(self.cursor.unwrap_or(0));
                        }
                        cx.handled = true;
                    }
                    keysyms::KEY_space if self.mode == SelectionMode::Multiple => {
                        if let Some(cursor) = self.cursor {
                            changed = self.selection.toggle(cursor);
                        }
                        cx.handled = true;
                    }
                    keysyms::KEY_a if ctrl && self.mode == SelectionMode::Multiple => {
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
}

#[cfg(test)]
mod tests {
    use crate::css::node::{Node, PseudoStates};
    use crate::view::{Event, EventKind, Handler, Kind, Prop, PropName, Props};
    use crate::widgets::list_box::ListBoxC;
    use crate::widgets::types::SelectionMode;
    use crate::widgets::{Headless, build_controller, build_widget, matches_fixture};

    fn props(mode: SelectionMode) -> Props {
        let mut p = Props::default();
        p.set(PropName::SelectionMode, Prop::Enum(mode.to_u16()));
        p.set(PropName::ShowSeparators, Prop::Bool(true));
        p
    }

    #[test]
    fn a_list_box_is_a_list_node_of_row_nodes() {
        // Mutation check: naming the container `listbox` (the class name,
        // not the CSS node) fails the fixture and every Adwaita `list > row`
        // rule.
        let built = build_widget::<()>(Kind::ListBox, &props(SelectionMode::Single));
        matches_fixture(
            &built.node,
            "list[.separators][.rich-list][.navigation-sidebar][.boxed-list]\n╰── row[.activatable]\n",
        )
        .expect("list_box fixture");
    }

    /// Build a `Kind::ListBox` over `row_count` real `row` children, the way
    /// a reconciled `list_box(rows)` view always would -- `build_widget`
    /// itself never attaches application children (see the module doc), so
    /// the interaction tests below build the node by hand instead.
    fn list_box_with_rows<Msg: Clone + 'static>(
        row_count: usize,
        mode: SelectionMode,
    ) -> (Node, Box<dyn crate::view::Controller<Msg>>) {
        let node = Node::with_classes(Kind::ListBox.css_name(), Kind::ListBox.base_classes());
        for _ in 0..row_count {
            node.append_child(&Node::new("row"));
        }
        let mut hx = Headless::new();
        let controller = {
            let mut cx = hx.cx();
            build_controller::<Msg>(Kind::ListBox, &node, &props(mode), &mut cx)
        };
        (node, controller)
    }

    #[test]
    fn clicking_a_row_selects_it_and_sets_selected_on_that_row_only() {
        // Interaction test. Mutation check: setting :selected without
        // clearing the previous row leaves the whole list highlighted.
        let (node, mut c) = list_box_with_rows::<usize>(2, SelectionMode::Single);
        let mut hx = Headless::new();
        hx.place_rows(&node, 30.0);
        let mut cx = hx.event_cx_with_handlers(&node, |h| {
            h.set(EventKind::Selected, Handler::Index(std::rc::Rc::new(|i| i)));
        });
        assert_eq!(
            c.on_event(
                &Event::PointerDown {
                    button: 0x110,
                    local: (5.0, 45.0),
                    serial: 1
                },
                &mut cx
            ),
            vec![1]
        );
        let selected: Vec<usize> = ListBoxC::selected_rows(c.as_ref());
        assert_eq!(selected, vec![1]);
        assert!(
            node.child(1)
                .unwrap()
                .states()
                .contains(PseudoStates::SELECTED)
        );
        assert!(
            !node
                .child(0)
                .unwrap()
                .states()
                .contains(PseudoStates::SELECTED)
        );
    }

    #[test]
    fn arrows_move_the_cursor_space_toggles_under_multiple_and_ctrl_a_selects_all() {
        // Mutation check: letting Space toggle under Single mode makes a
        // single-selection list deselect itself on Space, which GTK does not.
        let (node, mut c) = list_box_with_rows::<usize>(2, SelectionMode::Multiple);
        let mut hx = Headless::new();
        let mut cx = hx.event_cx(&node);
        c.on_event(&Event::Key(Headless::key("Down")), &mut cx);
        c.on_event(&Event::Key(Headless::key("space")), &mut cx);
        assert_eq!(ListBoxC::selected_rows(c.as_ref()), vec![0]);
        c.on_event(
            &Event::Key(Headless::key_with_mods("a", &["Control"])),
            &mut cx,
        );
        assert_eq!(
            ListBoxC::selected_rows(c.as_ref()).len(),
            node.child_count()
        );
    }
}
