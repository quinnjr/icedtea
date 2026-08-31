//! `GtkCheckButton` — `Kind::CheckButton`, CSS node `checkbutton`.
//!
//! ```text
//! checkbutton[.text-button][.grouped]
//! ├── check
//! ╰── [label]
//! ```
//!
//! GTK's own prose says the indicator node "is named check when no group is
//! set, and radio if the checkbutton is grouped". In 4.22.4's rendered tree the
//! node stays `check` and the *builtin image* becomes a radio, which is what
//! `-gtk-icon-source` selects; that is what [`CheckButtonC::builtin_for`]
//! encodes, and the `.grouped` class on the root is how a stylesheet tells the
//! two apart.

use std::rc::Rc;

use crate::css::node::{Node, PseudoStates};
use crate::icons::builtin::Builtin;
use crate::view::controller::{Controller, Event, EventCx};
use crate::view::{BuildCx, EventKind, Handler, Kind, Prop, PropName, Props, View};
use crate::widgets::PointerState;

/// A `GtkCheckButton` labelled `label`.
#[must_use]
pub fn check_button<Msg: Clone + 'static>(label: &str) -> View<Msg> {
    View::new(Kind::CheckButton).prop(PropName::Label, Prop::Str(Rc::from(label)))
}

/// `GtkCheckButton`'s own setters and signals.
pub trait CheckButtonExt<Msg>: Sized {
    /// `GtkCheckButton:active`.
    fn active(self, on: bool) -> Self;
    /// `GtkCheckButton:inconsistent`.
    fn inconsistent(self, on: bool) -> Self;
    /// `GtkCheckButton:group`, by name.
    fn group(self, name: &str) -> Self;
    /// `GtkCheckButton:use-underline`.
    fn use_underline(self, on: bool) -> Self;
    /// `GtkCheckButton::toggled`.
    fn on_toggle(self, f: impl Fn(bool) -> Msg + 'static) -> Self;
}

impl<Msg: Clone + 'static> CheckButtonExt<Msg> for View<Msg> {
    fn active(self, on: bool) -> Self {
        self.prop(PropName::Active, Prop::Bool(on))
    }
    fn inconsistent(self, on: bool) -> Self {
        self.prop(PropName::Indeterminate, Prop::Bool(on))
    }
    fn group(self, name: &str) -> Self {
        self.prop(PropName::Group, Prop::Str(Rc::from(name)))
    }
    fn use_underline(self, on: bool) -> Self {
        self.prop(PropName::Editable, Prop::Bool(on))
    }
    fn on_toggle(self, f: impl Fn(bool) -> Msg + 'static) -> Self {
        self.on(EventKind::Toggle, Handler::Bool(Rc::new(f)))
    }
}

/// `Kind::CheckButton`'s controller.
pub struct CheckButtonC {
    /// `GtkCheckButton:active`.
    pub active: bool,
    /// `GtkCheckButton:inconsistent`.
    pub inconsistent: bool,
    /// The group name, when grouped.
    pub group: Option<Rc<str>>,
    /// The `check` subnode.
    pub check: Node,
    /// The `label` subnode, when the button has text.
    pub label: Option<Node>,
    pointer: PointerState,
}

impl CheckButtonC {
    /// The builtin shape for `(grouped, inconsistent)`.
    #[must_use]
    pub fn builtin_for(grouped: bool, inconsistent: bool) -> Builtin {
        match (grouped, inconsistent) {
            (true, true) => Builtin::RadioIndeterminate,
            (true, false) => Builtin::Radio,
            (false, true) => Builtin::CheckIndeterminate,
            (false, false) => Builtin::Check,
        }
    }

    /// This button's builtin shape.
    #[must_use]
    pub fn builtin(&self) -> Builtin {
        Self::builtin_for(self.group.is_some(), self.inconsistent)
    }

    fn apply(&self, node: &Node) {
        node.remove_class("text-button");
        if self.label.is_some() {
            node.add_class("text-button");
        }
        node.remove_class("grouped");
        if self.group.is_some() {
            node.add_class("grouped");
        }
        node.set_state(PseudoStates::CHECKED, self.active && !self.inconsistent);
        node.set_state(PseudoStates::INDETERMINATE, self.inconsistent);
        self.check
            .set_state(PseudoStates::CHECKED, self.active && !self.inconsistent);
        self.check
            .set_state(PseudoStates::INDETERMINATE, self.inconsistent);
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for CheckButtonC {
    fn kind(&self) -> Kind {
        Kind::CheckButton
    }

    fn build(node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        let check = Node::new("check");
        node.append_child(&check);
        let label = props.str(PropName::Label).map(|_| {
            let label = Node::new("label");
            node.append_child(&label);
            label
        });
        let this = CheckButtonC {
            active: props.bool(PropName::Active, false),
            inconsistent: props.bool(PropName::Indeterminate, false),
            group: props.str(PropName::Group).map(Rc::from),
            check,
            label,
            pointer: PointerState::default(),
        };
        this.apply(node);
        this
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        match (name, value) {
            (PropName::Active, Prop::Bool(on)) => self.active = *on,
            (PropName::Indeterminate, Prop::Bool(on)) => self.inconsistent = *on,
            (PropName::Group, Prop::Str(name)) => self.group = Some(Rc::clone(name)),
            _ => return,
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
        // Activating always leaves the inconsistent state, as GTK does; a
        // grouped check cannot be turned off by clicking it.
        self.inconsistent = false;
        let next = if self.group.is_some() {
            true
        } else {
            !self.active
        };
        self.active = next;
        self.apply(cx.node);
        cx.handled = true;
        cx.handlers
            .fire_bool(EventKind::Toggle, next)
            .map_or_else(Vec::new, |m| vec![m])
    }

    fn paint(
        &mut self,
        canvas: &mut skia_rs_safe::canvas::Canvas<'_>,
        alloc: &crate::layout::Allocation,
        style: &crate::css::computed::ComputedStyle,
        _cx: &mut crate::view::controller::PaintCx<'_>,
    ) -> bool {
        if !self.active && !self.inconsistent {
            return false;
        }
        // The indicator's own allocation is the `check` node's; the builtin is
        // drawn into it. P7 replaces Builtin::path's geometry (plan D9).
        let _ = alloc;
        self.builtin()
            .draw(canvas, alloc.content_box, style.color());
        true
    }
}
