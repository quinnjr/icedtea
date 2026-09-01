//! `GtkHeaderBar` -- a draggable title bar with start/end packs and window
//! controls placed by `gtk-decoration-layout`.
//!
//! ```text
//! headerbar
//! ╰── windowhandle
//!     ╰── box
//!         ├── box.start
//!         │   ├── windowcontrols.start
//!         │   ╰── [other children]
//!         ├── [Title Widget]
//!         ╰── box.end
//!             ├── [other children]
//!             ╰── windowcontrols.end
//! ```
//!
//! The chrome (`windowhandle` > `box` > the three slots) is built once, in
//! [`HeaderBarC::build`]; the view's own children -- routed here flat, as
//! direct siblings of `windowhandle` on the widget's own root node, the same
//! "chrome alongside the view's children" arrangement [`super::action_bar`]
//! uses -- are sorted into their slot by [`HeaderBarC::place`], which reads
//! each child's own [`PropName::Section`] back with [`super::props_of`]
//! rather than through an `Instance` this controller does not own.
//!
//! [`HeaderBarC::place`] runs from [`Controller::reserved_total`]: the
//! reconciler calls that once per reconcile, after every new or moved child
//! is already attached flat to the root, and before it trims anything past
//! the returned bound. Because a `.title_widget()` child can arrive *after*
//! `build()` already created the default `label` title, swapping which node
//! plays that role has to happen from a `&self` method -- `reserved_total`
//! is not `&mut self` -- so `title` is a `RefCell<Node>` rather than a plain
//! field, unlike the `pub title: Node` the design sketch shows.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use crate::css::node::Node;
use crate::layout::{Align, BoxDirection, ChildLayout, Container};
use crate::view::{BuildCx, Cmd, Controller, Event, EventCx, Kind, Prop, PropName, Props};
use crate::widgets::window_controls::WindowControlsC;
use crate::widgets::{
    Side, Universal, WidgetEnum as _, local_rect, prop_bool, prop_str, props_of, set_child_layout,
    set_container,
};

/// `gtk-titlebar-double-click`'s default action window.
const DOUBLE_CLICK: Duration = Duration::from_millis(400);

/// `GtkHeaderBar`.
pub struct HeaderBarC {
    /// `box.start`.
    pub start: Node,
    /// `box.end`.
    pub end: Node,
    /// The title widget (a `label` by default, or the app's own view).
    ///
    /// A `RefCell` rather than a plain field -- see the module doc.
    pub title: RefCell<Node>,
    /// `[start, end]` window controls, when `show-title-buttons`.
    pub controls: [Option<WindowControlsC>; 2],
    /// The effective `gtk-decoration-layout`.
    pub layout: Rc<str>,
    /// The `windowhandle` node, for drag and double-click.
    handle: Node,
    /// `box` -- the row `box.start`/title/`box.end` all live on; the title
    /// swap in [`HeaderBarC::place`] inserts into this node.
    row: Node,
    /// The `windowcontrols` node each present side built, so new packed
    /// children can be inserted before (end) or after (start) it without
    /// walking the whole child list.
    control_nodes: [Option<Node>; 2],
    /// When the last press on the handle landed.
    last_press: Option<Duration>,
    universal: Universal,
}

impl HeaderBarC {
    /// Test hook: the button names each side actually rendered.
    #[must_use]
    pub fn button_names<Msg: 'static>(c: &dyn Controller<Msg>) -> (Vec<String>, Vec<String>) {
        let any: &dyn std::any::Any = c;
        let me = any.downcast_ref::<Self>();
        let names = |slot: usize| {
            me.and_then(|m| m.controls[slot].as_ref())
                .map(WindowControlsC::button_names)
                .unwrap_or_default()
        };
        (names(0), names(1))
    }

    /// Split `gtk-decoration-layout` on its single colon.
    ///
    /// User input: a missing colon puts everything on the start side (GTK's
    /// own reading), extra colons are ignored, and each half is capped at 8
    /// entries so a pathological settings file cannot build a thousand
    /// buttons.
    #[must_use]
    pub fn split_layout(layout: &str) -> (Vec<&str>, Vec<&str>) {
        let (start, end) = layout.split_once(':').unwrap_or((layout, ""));
        fn half(s: &str) -> Vec<&str> {
            s.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .take(8)
                .collect::<Vec<_>>()
        }
        (half(start), half(end))
    }

    /// Build `windowcontrols.start` and `windowcontrols.end`, wired to
    /// `self.layout` -- each side reads its own half via
    /// [`WindowControlsC::tokens`], so the close button cannot appear twice.
    fn build_controls<Msg: Clone + 'static>(&mut self, cx: &mut BuildCx<'_>) {
        for (slot, side, container) in [
            (0usize, Side::Start, &self.start),
            (1, Side::End, &self.end),
        ] {
            let node = Node::with_classes("windowcontrols", &[side.css_class()]);
            match side {
                // `windowcontrols.start` leads its box; `.end` trails —
                // matching the fixture's `box.start`/`box.end` order.
                Side::Start => container.append_child(&node),
                Side::End => container.insert_child(0, &node),
            }
            let mut props = Props::default();
            props.set(PropName::Side, side.to_prop());
            props.set(PropName::Decoration, Prop::Str(Rc::clone(&self.layout)));
            let controller = <WindowControlsC as Controller<Msg>>::build(&node, &props, cx);
            self.controls[slot] = Some(controller);
            self.control_nodes[slot] = Some(node);
        }
    }

    /// Detach both `windowcontrols` nodes and forget their controllers.
    fn drop_controls(&mut self) {
        for slot in 0..2 {
            if let Some(node) = self.control_nodes[slot].take() {
                node.detach();
            }
            self.controls[slot] = None;
        }
    }

    fn rebuild_controls<Msg: Clone + 'static>(&mut self, cx: &mut BuildCx<'_>) {
        self.drop_controls();
        self.build_controls::<Msg>(cx);
    }

    /// Move every real child the reconciler just attached flat to `node`
    /// (this controller's own root, alongside `handle`) into its recorded
    /// [`PropName::Section`] -- `"start"` (the default), `"end"` or
    /// `"title"`.
    fn place(&self) {
        let Some(root) = self.handle.parent() else {
            return;
        };
        for child in root.children() {
            if child.ptr_eq(&self.handle) {
                continue;
            }
            let props = props_of(&child);
            match props.str(PropName::Section).unwrap_or("start") {
                "end" => {
                    let idx = self.control_nodes[1]
                        .as_ref()
                        .and_then(Node::index_in_parent)
                        .unwrap_or_else(|| self.end.child_count());
                    self.end.insert_child(idx, &child);
                }
                "title" => {
                    let mut title = self.title.borrow_mut();
                    if title.ptr_eq(&child) {
                        continue;
                    }
                    let idx = title.index_in_parent().unwrap_or(0);
                    title.detach();
                    self.row.insert_child(idx, &child);
                    *title = child;
                }
                _ => self.start.append_child(&child),
            }
        }
    }
}

/// Whether `local` (in the controller's own event space) lands on `handle`.
///
/// With no computed layout yet -- every headless unit test in this module,
/// [`crate::widgets::Headless`]'s own doc comment -- there is nothing to
/// hit-test against, so a press is presumed to be on the draggable bar
/// rather than silently never toggling.
fn on_handle(
    tree: &crate::layout::LayoutTree,
    root: &Node,
    handle: &Node,
    local: (f32, f32),
) -> bool {
    match local_rect(tree, root, handle) {
        Some(rect) => {
            local.0 >= rect.x
                && local.0 < rect.right()
                && local.1 >= rect.y
                && local.1 < rect.bottom()
        }
        None => true,
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for HeaderBarC {
    fn kind(&self) -> Kind {
        Kind::HeaderBar
    }

    fn build(node: &Node, props: &Props, cx: &mut BuildCx<'_>) -> Self {
        set_container(
            node,
            Container::Box {
                direction: BoxDirection::Column,
            },
        );
        let handle = Node::new("windowhandle");
        let row = Node::new("box");
        let start = Node::with_classes("box", &["start"]);
        let title = Node::new("label");
        let end = Node::with_classes("box", &["end"]);
        node.append_child(&handle);
        handle.append_child(&row);
        for (child, direction) in [
            (&row, BoxDirection::Row),
            (&start, BoxDirection::Row),
            (&end, BoxDirection::Row),
        ] {
            set_container(child, Container::Box { direction });
        }
        row.append_child(&start);
        row.append_child(&title);
        row.append_child(&end);
        set_child_layout(
            &title,
            ChildLayout {
                halign: Align::Center,
                hexpand: true,
                ..ChildLayout::default()
            },
        );
        // Chrome subnodes don't carry real text content anywhere in this
        // codebase yet (`Frame`'s `label`, `Expander`'s `label`, ...); the
        // `label` node exists purely for the fixture/CSS shape, so `Title`
        // and `Subtitle` are recorded on nothing further than that shape.
        let layout: Rc<str> = Rc::from(
            props
                .str(PropName::Decoration)
                .unwrap_or("icon:minimize,maximize,close"),
        );
        let mut me = Self {
            start,
            end,
            title: RefCell::new(title),
            controls: [None, None],
            layout,
            handle,
            row,
            control_nodes: [None, None],
            last_press: None,
            universal: Universal::new(node, Kind::HeaderBar),
        };
        if props.bool(PropName::ShowTitleButtons, true) {
            me.build_controls::<Msg>(cx);
        }
        me
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, cx: &mut BuildCx<'_>) {
        match name {
            // See the "Chrome subnodes" note in `build`.
            PropName::Title | PropName::Subtitle => {}
            PropName::Decoration => {
                self.layout = Rc::from(prop_str(value));
                if self.controls[0].is_some() || self.controls[1].is_some() {
                    self.rebuild_controls::<Msg>(cx);
                }
            }
            PropName::ShowTitleButtons => {
                if prop_bool(value, true) {
                    self.rebuild_controls::<Msg>(cx);
                } else {
                    self.drop_controls();
                }
            }
            other => {
                self.universal.apply(node, Kind::HeaderBar, other, value);
            }
        }
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        // Window control buttons get first refusal at every event: a click
        // that lands on `minimize`/`maximize`/`close` must dispatch its
        // `Cmd` and not also register as a press on the handle beneath it.
        for control in self.controls.iter_mut().flatten() {
            Controller::<Msg>::on_event(control, ev, cx);
            if cx.handled {
                return Vec::new();
            }
        }
        match ev {
            Event::PointerDown {
                button: 0x110,
                local,
                ..
            } if on_handle(cx.tree, cx.node, &self.handle, *local) => {
                let now = cx.clock.now();
                let double = self
                    .last_press
                    .is_some_and(|last| now.saturating_sub(last) <= DOUBLE_CLICK);
                self.last_press = Some(now);
                if double {
                    // `gtk-titlebar-double-click` defaults to toggle-maximize.
                    cx.cmds.push(Cmd::ToggleMaximized);
                    self.last_press = None;
                    cx.handled = true;
                }
            }
            // `gtk-titlebar-middle-click` defaults to `none`; the press is
            // observed and deliberately does nothing.
            Event::PointerDown { button: 0x112, .. } => {}
            _ => {}
        }
        Vec::new()
    }

    fn child_index(&self, view_index: usize) -> usize {
        // `handle` is chrome at index 0; every real child lands flat, after
        // it, until `place` sorts them into their slot.
        view_index + 1
    }

    fn reserved_total(&self, view_count: usize) -> usize {
        let _ = view_count;
        self.place();
        // `place` has just emptied the root back down to `handle` alone.
        1
    }
}

#[cfg(test)]
mod tests {
    use crate::view::{Event, Kind, Prop, PropName, Props};
    use crate::widgets::header_bar::HeaderBarC;
    use crate::widgets::{Headless, build_widget, matches_fixture};

    // Not a `\`-continued literal: that would eat every continued line's
    // leading whitespace, which is exactly the indentation this fixture's
    // depth tracking reads (`node_tree::split_line`).
    const FIXTURE: &str = concat!(
        "headerbar\n",
        "╰── windowhandle\n",
        "    ╰── box\n",
        "        ├── box.start\n",
        "        │   ├── windowcontrols.start\n",
        "        │   ╰── [other children]\n",
        "        ├── [label]\n",
        "        ╰── box.end\n",
        "            ├── [other children]\n",
        "            ╰── windowcontrols.end\n",
    );

    fn props(layout: &str, buttons: bool) -> Props {
        let mut p = Props::default();
        p.set(PropName::Decoration, Prop::Str(layout.into()));
        p.set(PropName::ShowTitleButtons, Prop::Bool(buttons));
        p.set(PropName::Title, Prop::Str("Files".into()));
        p
    }

    #[test]
    fn the_bar_is_a_windowhandle_over_a_three_slot_box() {
        // Mutation check: dropping the `windowhandle` node makes the bar
        // undraggable and Adwaita's `headerbar > windowhandle` rules miss.
        let built = build_widget::<()>(Kind::HeaderBar, &props("icon:minimize,close", true));
        matches_fixture(&built.node, FIXTURE).expect("header_bar fixture");
    }

    #[test]
    fn the_decoration_layout_splits_on_the_colon_and_feeds_both_sides() {
        // Mutation check: giving both WindowControls the whole layout string
        // renders the close button twice, once on each side.
        let built = build_widget::<()>(Kind::HeaderBar, &props("icon:minimize,close", true));
        let (start, end) = HeaderBarC::button_names(built.controller.as_ref());
        assert_eq!(start, vec!["icon"]);
        assert_eq!(end, vec!["minimize", "close"]);
    }

    #[test]
    fn show_title_buttons_false_leaves_no_controls_at_all() {
        // Mutation check: building the controls and hiding them with a class
        // still reserves their width, so the title stops being centred.
        let built = build_widget::<()>(Kind::HeaderBar, &props("icon:close", false));
        let (start, end) = HeaderBarC::button_names(built.controller.as_ref());
        assert!(start.is_empty() && end.is_empty());
    }

    #[test]
    fn a_double_click_on_the_handle_asks_the_compositor_to_toggle_maximised() {
        // Interaction test. Mutation check: emitting Cmd::ToggleMaximized on
        // a single click makes a click-to-focus maximise the window.
        let built = build_widget::<()>(Kind::HeaderBar, &props("icon:close", true));
        let mut c = built.controller;
        let mut hx = Headless::new();
        let mut cx = hx.event_cx(&built.node);
        c.on_event(
            &Event::PointerDown {
                button: 0x110,
                local: (60.0, 8.0),
                serial: 1,
            },
            &mut cx,
        );
        c.on_event(
            &Event::PointerUp {
                button: 0x110,
                local: (60.0, 8.0),
                serial: 2,
            },
            &mut cx,
        );
        assert!(cx.cmds.is_empty(), "one click does nothing");
        c.on_event(
            &Event::PointerDown {
                button: 0x110,
                local: (60.0, 8.0),
                serial: 3,
            },
            &mut cx,
        );
        c.on_event(
            &Event::PointerUp {
                button: 0x110,
                local: (60.0, 8.0),
                serial: 4,
            },
            &mut cx,
        );
        assert!(
            cx.cmds
                .iter()
                .any(|c| matches!(c, crate::view::Cmd::ToggleMaximized)),
            "the second click within the double-click window toggles"
        );
    }

    #[test]
    fn a_hostile_decoration_layout_never_panics() {
        // The layout string comes from `settings.ini`, which is user input.
        for layout in [
            "",
            ":",
            "::::",
            "menu",
            "close,close,close:",
            &"a,".repeat(10_000),
            "icon:\u{0}",
            "\u{feff}minimize:close",
        ] {
            let _ = build_widget::<()>(Kind::HeaderBar, &props(layout, true));
        }
    }
}
