//! `GtkMenuButton` — `Kind::MenuButton`, CSS node `menubutton`.
//!
//! ```text
//! menubutton
//! ╰── button.toggle
//!     ╰── <content>
//!          ╰── [arrow]
//! ```
//!
//! The inner button takes `.image-button`, `.text-button` or `.arrow-button`
//! from its content, and the `arrow` node carries one of `.none`, `.up`,
//! `.down`, `.left`, `.right` for the direction the menu will appear in.
//!
//! Reconciliation 2 (P8-D72's close-out): GTK's own doc block for this widget
//! stops at the button and does not list the popover, so §5.2's vendored
//! fixture gained `╰── [popover.background.menu]` when the popover stopped
//! being an orphan. The block is eliding, not contradicting: `gtkmenubutton.c`
//! parents the popover to the menu button (`gtk_widget_set_parent (popover,
//! GTK_WIDGET (menu_button))`), so its CSS node *is* a child of `menubutton` —
//! which is exactly how GTK's own `GtkDropDown` block spells the same
//! relationship, and how `drop_down.txt` already carries it. Optional, because
//! a `MenuButton` with no menu has none.
//!
//! Reconciliation: GTK nests the button's content (and the arrow within it)
//! under an anonymous box the plan's doc comment calls `<content>`; the
//! fixture matcher only recognises the literal token `<child>` as a wildcard
//! (`crate::widgets::fixture_matches`), and this controller builds label,
//! image and arrow as direct children of `button` with no such wrapper node,
//! so the vendored fixture lists them there instead.

use std::rc::Rc;

use crate::css::node::{Node, PseudoStates};
use crate::css::value::image::IconRef;
use crate::layout::Rect;
use crate::view::controller::{Controller, Event, EventCx};
use crate::view::{BuildCx, EventKind, Kind, Prop, PropName, Props, View};
use crate::widgets::popover::PopoverC;
use crate::widgets::{ArrowDirection, PointerState, WidgetEnum, local_rect, shift_event};
use crate::window::popup::PopupAnchorPoint;

/// A `GtkMenuButton` labelled `label`. Children become the popover content.
#[must_use]
pub fn menu_button<Msg: Clone + 'static>(label: &str) -> View<Msg> {
    View::new(Kind::MenuButton).prop(PropName::Label, Prop::Str(Rc::from(label)))
}

/// `GtkMenuButton`'s own setters.
pub trait MenuButtonExt<Msg>: Sized {
    /// `GtkMenuButton:icon-name`, as an `IconRef`.
    fn icon(self, icon: IconRef) -> Self;
    /// `GtkMenuButton:always-show-arrow`.
    fn always_show_arrow(self, on: bool) -> Self;
    /// `GtkMenuButton:direction`.
    fn direction(self, dir: ArrowDirection) -> Self;
    /// `GtkMenuButton:has-frame`.
    fn has_frame(self, on: bool) -> Self;
    /// `GtkMenuButton:primary`.
    fn primary(self, on: bool) -> Self;
}

impl<Msg: Clone + 'static> MenuButtonExt<Msg> for View<Msg> {
    fn icon(self, icon: IconRef) -> Self {
        self.prop(PropName::Icon, Prop::Icon(icon))
    }
    fn always_show_arrow(self, on: bool) -> Self {
        self.prop(PropName::ShowArrow, Prop::Bool(on))
    }
    fn direction(self, dir: ArrowDirection) -> Self {
        self.prop(PropName::Gravity, dir.to_prop())
    }
    fn has_frame(self, on: bool) -> Self {
        self.prop(PropName::Reveal, Prop::Bool(on))
    }
    fn primary(self, on: bool) -> Self {
        self.prop(PropName::Modal, Prop::Bool(on))
    }
}

/// `Kind::MenuButton`'s controller.
pub struct MenuButtonC {
    /// Whether the popover is showing.
    pub open: bool,
    /// The `button.toggle` subnode.
    pub button: Node,
    /// The `arrow` subnode, when an arrow is shown.
    pub arrow: Option<Node>,
    /// The embedded popover.
    pub popover: PopoverC,
    pointer: PointerState,
}

impl MenuButtonC {
    /// Show or hide the menu, and paint the toggle to match.
    ///
    /// The pointer path and the property path have to agree about what "open"
    /// means down to the node's own visibility, so both go through here —
    /// `DropDownC::set_expanded`'s shape, for the same reason.
    fn set_expanded(&mut self, on: bool) {
        self.open = on;
        self.popover.open = on;
        self.popover.reveal(on);
        self.button.set_state(PseudoStates::CHECKED, on);
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for MenuButtonC {
    fn kind(&self) -> Kind {
        Kind::MenuButton
    }

    fn build(node: &Node, props: &Props, cx: &mut BuildCx<'_>) -> Self {
        let button = Node::with_classes("button", &["toggle"]);
        node.append_child(&button);
        let has_label = props.str(PropName::Label).is_some();
        let has_icon = props.get(PropName::Icon).is_some();
        if has_label {
            button.append_child(&Node::new("label"));
            button.add_class("text-button");
        }
        if has_icon {
            button.append_child(&Node::new("image"));
            button.add_class("image-button");
        }
        let direction =
            ArrowDirection::from_prop(props.get(PropName::Gravity), ArrowDirection::Down);
        let arrow =
            (props.bool(PropName::ShowArrow, true) || !(has_label || has_icon)).then(|| {
                let arrow = Node::with_classes("arrow", &[direction.css_class()]);
                button.append_child(&arrow);
                button.add_class("arrow-button");
                arrow
            });
        // A real, tree-attached popover node — `DropDownC::build`'s shape.
        // Until P8-D72's close-out this was `PopoverC::for_test`, whose root
        // is a local `Node` dropped at the end of the call: the `arrow` and
        // `contents` it built were never appended to anything, so the menu's
        // own body (routed under `contents` by `widgets::child_slot`) was
        // reconciled into a detached subtree, had no allocation in any state,
        // and could not be laid out, painted or hit. `for_test`'s own doc
        // named this the case it served; nothing in production uses it now.
        let popover_node = Node::with_classes("popover", &["background", "menu"]);
        node.append_child(&popover_node);
        let mut popover_props = Props::default();
        popover_props.set(PropName::ShowArrow, Prop::Bool(true));
        let mut popover = <PopoverC as Controller<Msg>>::build(&popover_node, &popover_props, cx);
        // A closed menu shows no menu. Without this the body would simply be
        // painted beside the button for the widget's whole life, which is the
        // defect P8-D71 measured on `DropDown` and fixed the same way.
        popover.hide_when_closed();
        let mut this = MenuButtonC {
            open: false,
            button,
            arrow,
            popover,
            pointer: PointerState::default(),
        };
        // `GtkMenuButton` has no "popover shown" property of its own; this
        // reuses `Expanded`, the name every other disclosure widget in the
        // crate already carries (`DropDownC` included), so a caller — and
        // `gallery --open`, which the interaction gate reads the open
        // coordinates from — can build the tree a click produces without one.
        this.set_expanded(props.bool(PropName::Expanded, false));
        this
    }

    fn set_prop(&mut self, _node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        match (name, value) {
            (PropName::Gravity, Prop::Enum(_)) => {
                let direction = ArrowDirection::from_prop(Some(value), ArrowDirection::Down);
                if let Some(arrow) = self.arrow.as_ref() {
                    arrow.set_classes(&[direction.css_class()]);
                }
            }
            (PropName::Expanded, Prop::Bool(on)) => self.set_expanded(*on),
            _ => {}
        }
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        if matches!(ev, Event::PopupDone) {
            // Through `set_expanded`, not by hand: the popover's own
            // `on_event` never runs for an embedded popover (`deliver` routes
            // to `Instance`s, and this node belongs to no instance), so this is
            // the only place a dismissal can put the body away.
            self.set_expanded(false);
            return Vec::new();
        }
        // Focus in/out is a widget-level signal, not a `MenuButton` one:
        // `GenericC` forwards it for every kind still on the fallback, so a
        // kind that takes its own controller has to keep forwarding it or an
        // `on_focus_in`/`on_focus_out` handler silently stops firing.
        match ev {
            Event::FocusIn { .. } => {
                return cx
                    .handlers
                    .fire_unit(EventKind::FocusIn)
                    .into_iter()
                    .collect();
            }
            Event::FocusOut => {
                return cx
                    .handlers
                    .fire_unit(EventKind::FocusOut)
                    .into_iter()
                    .collect();
            }
            _ => {}
        }
        let Some(rect) = local_rect(cx.tree, cx.node, &self.button) else {
            return Vec::new();
        };
        let shifted = shift_event(ev, rect);
        if self.pointer.observe(
            &self.button,
            &shifted,
            Some(Rect::new(0.0, 0.0, rect.width, rect.height)),
        ) {
            cx.handled = true;
            if self.open {
                self.popover.close(cx);
            } else {
                self.popover.open(
                    PopupAnchorPoint::Node(self.button.clone()),
                    (rect.width.max(1.0) as u32, 200),
                    // The menu's contents are the `MenuButton`'s own
                    // children, already retained in the parent tree.
                    None,
                    cx,
                );
            }
            // `open`/`close` have already moved the popover's own state and
            // visibility; this keeps this controller's copy and the toggle's
            // `:checked` in step with them.
            self.set_expanded(self.popover.open);
        }
        Vec::new()
    }
}
