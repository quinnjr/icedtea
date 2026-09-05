//! `GtkButton` — `Kind::Button`, CSS node `button`.
//!
//! ```text
//! button[.image-button][.text-button][.flat][.keyboard-activating]
//! ```
//!
//! `.image-button`/`.text-button` are set from the content GTK actually finds;
//! `.keyboard-activating` is added for the duration of a Space/Enter
//! activation. `.suggested-action`, `.destructive-action` and `.circular` are
//! application-supplied and are never added here.
//!
//! This is **not** M2's `widget::button::Button`, which stays exactly as M2
//! left it so the M1 pixel gate keeps measuring the same code (contract §8.1).

use std::rc::Rc;
use std::time::Duration;

use crate::css::node::Node;
use crate::css::value::image::IconRef;
use crate::view::controller::{Controller, Event, EventCx};
use crate::view::{BuildCx, EventKind, Handler, Kind, Prop, PropName, Props, View};
use crate::widgets::{PointerState, Universal};
use crate::window::focus::FocusCause;

/// How long `.keyboard-activating` stays on after Space or Enter.
const ACTIVATE_FLASH: Duration = Duration::from_millis(120);

/// A `GtkButton` labelled `label`.
#[must_use]
pub fn button<Msg: Clone + 'static>(label: &str) -> View<Msg> {
    View::new(Kind::Button).prop(PropName::Label, Prop::Str(Rc::from(label)))
}

/// A `GtkButton` whose content is an arbitrary child.
#[must_use]
pub fn button_from<Msg: Clone + 'static>(child: View<Msg>) -> View<Msg> {
    View::new(Kind::Button).child(child)
}

/// `GtkButton`'s own setters and signals.
pub trait ButtonExt<Msg>: Sized {
    /// `GtkButton:label`.
    fn label(self, text: &str) -> Self;
    /// `GtkButton:icon-name`, as an `IconRef`.
    fn icon(self, icon: IconRef) -> Self;
    /// `GtkButton:has-frame`; `false` adds `.flat`.
    fn has_frame(self, on: bool) -> Self;
    /// `GtkButton:use-underline`.
    fn use_underline(self, on: bool) -> Self;
    /// `GtkButton::clicked`.
    fn on_click(self, msg: Msg) -> Self;
    /// `GtkButton::activate`.
    fn on_activate(self, msg: Msg) -> Self;
}

impl<Msg: Clone + 'static> ButtonExt<Msg> for View<Msg> {
    fn label(self, text: &str) -> Self {
        self.prop(PropName::Label, Prop::Str(Rc::from(text)))
    }
    fn icon(self, icon: IconRef) -> Self {
        self.prop(PropName::Icon, Prop::Icon(icon))
    }
    fn has_frame(self, on: bool) -> Self {
        self.prop(PropName::ShowArrow, Prop::Bool(on))
    }
    fn use_underline(self, on: bool) -> Self {
        self.prop(PropName::Editable, Prop::Bool(on))
    }
    fn on_click(self, msg: Msg) -> Self {
        self.on(EventKind::Click, Handler::Unit(msg))
    }
    fn on_activate(self, msg: Msg) -> Self {
        self.on(EventKind::Activate, Handler::Unit(msg))
    }
}

/// `Kind::Button`'s controller.
pub struct ButtonC {
    /// The `label` subnode, when the button has text.
    pub label: Option<Node>,
    /// The `image` subnode, when the button has an icon.
    pub image: Option<Node>,
    /// When `.keyboard-activating` should come off.
    pub activating_until: Option<Duration>,
    pointer: PointerState,
    universal: Universal,
}

impl ButtonC {
    /// GTK sets `.image-button`/`.text-button` from the content it finds.
    ///
    /// Touches only the class that actually needs to change: `set_prop` calls
    /// this again for props `build` already consumed (the reconciler's
    /// initial pass), and an unconditional remove-then-add on an already
    /// correct class would still reorder it in the rendered tree.
    fn apply_content_classes(&self, node: &Node) {
        Self::apply_content_classes_to(node, self.label.is_some(), self.image.is_some());
    }

    fn apply_content_classes_to(node: &Node, has_label: bool, has_image: bool) {
        match (has_label, has_image) {
            (true, false) => {
                node.remove_class("image-button");
                node.add_class("text-button");
            }
            (false, true) => {
                node.remove_class("text-button");
                node.add_class("image-button");
            }
            _ => {
                node.remove_class("image-button");
                node.remove_class("text-button");
            }
        }
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for ButtonC {
    fn kind(&self) -> Kind {
        Kind::Button
    }

    fn build(node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        let image = props.get(PropName::Icon).is_some().then(|| {
            let image = Node::new("image");
            node.append_child(&image);
            image
        });
        let label = props.str(PropName::Label).map(|_| {
            let label = Node::new("label");
            node.append_child(&label);
            label
        });
        if !props.bool(PropName::ShowArrow, true) {
            node.add_class("flat");
        }
        // `.text-button`/`.image-button` before P3's focus-ring marker, so a
        // bare `Kind::Button` (no base classes of its own) renders
        // `button.text-button…`, not `button.focusable.text-button…`.
        ButtonC::apply_content_classes_to(node, label.is_some(), image.is_some());
        ButtonC {
            label,
            image,
            activating_until: None,
            pointer: PointerState::default(),
            universal: Universal::new(node, Kind::Button),
        }
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        match (name, value) {
            (PropName::Label, Prop::Str(_)) => {
                if self.label.is_none() {
                    let label = Node::new("label");
                    node.append_child(&label);
                    self.label = Some(label);
                }
            }
            (PropName::ShowArrow, Prop::Bool(on)) => {
                if *on {
                    node.remove_class("flat");
                } else {
                    node.add_class("flat");
                }
            }
            _ => {
                self.universal.apply(node, Kind::Button, name, value);
                return;
            }
        }
        self.apply_content_classes(node);
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        let bounds = cx
            .tree
            .allocation(cx.node)
            .map(|a| crate::layout::Rect::new(0.0, 0.0, a.border_box.width, a.border_box.height));
        if matches!(ev, Event::PointerDown { .. }) {
            // Reconciliation (Task 8 fix-round): a click on a `GtkButton`
            // must also grant it keyboard focus, matching `Entry`/
            // `SearchEntry`/`PasswordEntry`/`SpinButton`/`EditableLabel`'s
            // own `PointerDown` handling in this same crate — without this
            // no button (including a settings page's "Set" binding-capture
            // button) can ever become the node a subsequent key event is
            // routed to, since `deliver`'s D19 rule stops all bubbling once
            // this controller marks a target-phase event handled below.
            cx.focus.set_focus(Some(cx.node), FocusCause::Pointer);
        }
        if self.pointer.observe(cx.node, ev, bounds) {
            cx.handled = true;
            return cx
                .handlers
                .fire_unit(EventKind::Click)
                .map_or_else(Vec::new, |m| vec![m]);
        }
        if matches!(ev, Event::Activate) {
            cx.node.add_class("keyboard-activating");
            self.activating_until = Some(cx.clock.now() + ACTIVATE_FLASH);
            cx.handled = true;
            let mut out = Vec::new();
            if let Some(msg) = cx.handlers.fire_unit(EventKind::Activate) {
                out.push(msg);
            }
            if let Some(msg) = cx.handlers.fire_unit(EventKind::Click) {
                out.push(msg);
            }
            return out;
        }
        Vec::new()
    }

    fn tick(&mut self, now: Duration, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        if self.activating_until.is_some_and(|until| now >= until) {
            self.activating_until = None;
            cx.node.remove_class("keyboard-activating");
        }
        Vec::new()
    }

    fn next_deadline(&self, _now: Duration) -> Option<Duration> {
        self.activating_until
    }
}
