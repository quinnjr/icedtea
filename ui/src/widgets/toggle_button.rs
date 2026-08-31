//! `GtkToggleButton` — `Kind::ToggleButton`, CSS node `button`, class `.toggle`.
//!
//! ```text
//! button.toggle
//! ```
//!
//! A grouped toggle behaves radio-like: activating one clears its siblings.
//! Groups are keyed by the `Group` prop's name (contract §5.2's "Switch group
//! note"), not by object identity — the reactive layer has no stable handles.

use std::rc::Rc;

use crate::css::node::{Node, PseudoStates};
use crate::view::controller::{Controller, Event, EventCx};
use crate::view::{BuildCx, EventKind, Handler, Kind, Prop, PropName, Props, View};
use crate::widgets::{PointerState, Universal};

/// A `GtkToggleButton` labelled `label`.
#[must_use]
pub fn toggle_button<Msg: Clone + 'static>(label: &str) -> View<Msg> {
    View::new(Kind::ToggleButton).prop(PropName::Label, Prop::Str(Rc::from(label)))
}

/// `GtkToggleButton`'s own setters and signals.
pub trait ToggleButtonExt<Msg>: Sized {
    /// `GtkToggleButton:active`.
    fn active(self, on: bool) -> Self;
    /// `GtkToggleButton:group`, by name.
    fn group(self, name: &str) -> Self;
    /// `GtkToggleButton::toggled`.
    fn on_toggle(self, f: impl Fn(bool) -> Msg + 'static) -> Self;
}

impl<Msg: Clone + 'static> ToggleButtonExt<Msg> for View<Msg> {
    fn active(self, on: bool) -> Self {
        self.prop(PropName::Active, Prop::Bool(on))
    }
    fn group(self, name: &str) -> Self {
        self.prop(PropName::Group, Prop::Str(Rc::from(name)))
    }
    fn on_toggle(self, f: impl Fn(bool) -> Msg + 'static) -> Self {
        self.on(EventKind::Toggle, Handler::Bool(Rc::new(f)))
    }
}

/// `Kind::ToggleButton`'s controller.
pub struct ToggleButtonC {
    /// `GtkToggleButton:active`.
    pub active: bool,
    /// The group name, when grouped.
    pub group: Option<Rc<str>>,
    /// The `label` subnode.
    pub label: Option<Node>,
    pointer: PointerState,
    universal: Universal,
}

impl ToggleButtonC {
    fn apply(&self, node: &Node) {
        node.add_class("toggle");
        node.set_state(PseudoStates::CHECKED, self.active);
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for ToggleButtonC {
    fn kind(&self) -> Kind {
        Kind::ToggleButton
    }

    fn build(node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        let label = props.str(PropName::Label).map(|_| {
            let label = Node::new("label");
            node.append_child(&label);
            label
        });
        let this = ToggleButtonC {
            active: props.bool(PropName::Active, false),
            group: props.str(PropName::Group).map(Rc::from),
            label,
            pointer: PointerState::default(),
            universal: Universal::new(node, Kind::ToggleButton),
        };
        this.apply(node);
        this
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        match (name, value) {
            (PropName::Active, Prop::Bool(on)) => self.active = *on,
            (PropName::Group, Prop::Str(name)) => self.group = Some(Rc::clone(name)),
            _ => {
                self.universal.apply(node, Kind::ToggleButton, name, value);
                return;
            }
        }
        self.apply(node);
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        let bounds = cx
            .tree
            .allocation(cx.node)
            .map(|a| crate::layout::Rect::new(0.0, 0.0, a.border_box.width, a.border_box.height));
        let clicked = self.pointer.observe(cx.node, ev, bounds);
        if !clicked && !matches!(ev, Event::Activate) {
            return Vec::new();
        }
        // A grouped toggle cannot be turned off by clicking it, exactly as a
        // radio button cannot; an ungrouped one flips.
        let next = if self.group.is_some() {
            true
        } else {
            !self.active
        };
        if next == self.active {
            return Vec::new();
        }
        self.active = next;
        self.apply(cx.node);
        cx.handled = true;
        cx.handlers
            .fire_bool(EventKind::Toggle, next)
            .map_or_else(Vec::new, |m| vec![m])
    }
}
