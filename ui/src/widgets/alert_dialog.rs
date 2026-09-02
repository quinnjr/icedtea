//! `GtkAlertDialog`/`GtkMessageDialog` — `Kind::AlertDialog`, CSS node
//! `window`, classes `.dialog .message` (`Kind::base_classes`).
//!
//! ```text
//! window.dialog.message
//! ├── label.title
//! ├── [label]
//! ╰── box
//!     ├── button
//!     ┊
//!     ╰── button
//! ```
//!
//! One `WindowC` embedded directly on this controller's own root node, the
//! same composition [`crate::widgets::popover_menu::PopoverMenuC`] uses for
//! its embedded `PopoverC` — `label.title` (the bold heading), an optional
//! detail `label` and the button `box` are this controller's own chrome,
//! built once in [`Controller::build`] the way `PopoverMenuC::rebuild_items`
//! builds its `button.model` row. Their text is never painted, the same
//! trade `PopoverMenuC`'s own `button.model > label` subnodes already make
//! (that controller stores the raw labels and never wires a `paint`
//! override either) — GTK's own `.title`/detail labels *would* paint text
//! were this a real `GtkLabel` subtree, but a bare `Node` carries none of
//! `LabelC`'s shaping state, and a real per-label controller is out of this
//! task's scope.
//!
//! `default_button`/`cancel_button` come straight from the application
//! model (`PropName::DefaultButton`/`CancelButton`, plain integers), so an
//! out-of-range one — the model asked for a button that does not exist —
//! is simply absent rather than clamped to a real index: clamping a
//! destructive dialog's cancel index to 0 would make Enter (the *default*
//! button) fire *cancel*, which is the one thing a confirmation dialog must
//! never do.

use std::rc::Rc;

use crate::css::node::Node;
use crate::layout::Rect;
use crate::view::controller::{Controller, Event, EventCx};
use crate::view::{BuildCx, EventKind, Kind, Prop, PropName, Props, View};
use crate::widgets::window::WindowC;
use crate::widgets::{PointerState, local_rect, prop_i64, shift_event};

/// A `GtkAlertDialog`/`GtkMessageDialog` showing `message` as its bold
/// heading.
#[must_use]
pub fn alert_dialog<Msg: Clone + 'static>(message: &str) -> View<Msg> {
    View::new(Kind::AlertDialog).prop(PropName::Message, Prop::Str(Rc::from(message)))
}

/// [`alert_dialog`]'s own setters. `.on_response` is
/// [`crate::view::View::on_response`] (universal, shared with
/// `ColorDialog`/`FontDialog`/`InfoBar`) and `.modal` is
/// [`crate::widgets::window::WindowExt::modal`] (shared with every other
/// `Window` preset) — neither is repeated here.
pub trait AlertDialogExt<Msg>: Sized {
    /// A secondary line of text under the bold heading.
    fn detail(self, text: &str) -> Self;
    /// The dialog's buttons, left to right, by label.
    fn buttons(self, labels: impl IntoIterator<Item = impl Into<Rc<str>>>) -> Self;
    /// Which button index Enter activates.
    fn default_button(self, index: usize) -> Self;
    /// Which button index Escape activates.
    fn cancel_button(self, index: usize) -> Self;
}

impl<Msg: Clone + 'static> AlertDialogExt<Msg> for View<Msg> {
    fn detail(self, text: &str) -> Self {
        self.prop(PropName::Detail, Prop::Str(Rc::from(text)))
    }
    fn buttons(self, labels: impl IntoIterator<Item = impl Into<Rc<str>>>) -> Self {
        self.prop(
            PropName::Buttons,
            Prop::Classes(labels.into_iter().map(Into::into).collect()),
        )
    }
    fn default_button(self, index: usize) -> Self {
        self.prop(PropName::DefaultButton, Prop::Int(index as i64))
    }
    fn cancel_button(self, index: usize) -> Self {
        self.prop(PropName::CancelButton, Prop::Int(index as i64))
    }
}

/// An index prop (`DefaultButton`/`CancelButton`), absent when it names no
/// real button — the application model's index, not clamped (module doc).
fn button_index_raw(raw: i64, count: usize) -> Option<usize> {
    usize::try_from(raw).ok().filter(|i| *i < count)
}

/// `Kind::AlertDialog`'s controller.
pub struct AlertDialogC {
    /// The embedded `Window` chrome — see the module doc.
    pub window: WindowC,
    /// One `button` node per entry of `PropName::Buttons`, in order.
    pub buttons: Vec<Node>,
    /// Which of `buttons` Enter activates.
    pub default_button: Option<usize>,
    /// Which of `buttons` Escape activates.
    pub cancel_button: Option<usize>,
    /// One press-tracking [`PointerState`] per entry of `buttons`, parallel
    /// to it — [`crate::widgets::color_dialog::ColorDialogC`]'s own
    /// `swatch_pointers` doc gives the same reason a per-event scratch state
    /// cannot report a full press-then-release gesture.
    button_pointers: Vec<PointerState>,
}

impl AlertDialogC {
    /// Test hook: `default_button`, through a `&dyn Controller<Msg>`.
    #[must_use]
    pub fn default_of<Msg: 'static>(c: &dyn Controller<Msg>) -> Option<usize> {
        let any: &dyn std::any::Any = c;
        any.downcast_ref::<Self>().and_then(|m| m.default_button)
    }

    /// Test hook: `cancel_button`, through a `&dyn Controller<Msg>`.
    #[must_use]
    pub fn cancel_of<Msg: 'static>(c: &dyn Controller<Msg>) -> Option<usize> {
        let any: &dyn std::any::Any = c;
        any.downcast_ref::<Self>().and_then(|m| m.cancel_button)
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for AlertDialogC {
    fn kind(&self) -> Kind {
        Kind::AlertDialog
    }

    fn build(node: &Node, props: &Props, cx: &mut BuildCx<'_>) -> Self {
        let window = <WindowC as Controller<Msg>>::build(node, props, cx);
        let title = Node::with_classes("label", &["title"]);
        node.append_child(&title);
        if props.str(PropName::Detail).is_some() {
            node.append_child(&Node::new("label"));
        }
        let button_box = Node::new("box");
        node.append_child(&button_box);
        let labels: Vec<Rc<str>> = match props.get(PropName::Buttons) {
            Some(Prop::Classes(list)) => list.to_vec(),
            _ => Vec::new(),
        };
        let mut buttons = Vec::with_capacity(labels.len());
        for _ in &labels {
            let button = Node::new("button");
            button_box.append_child(&button);
            buttons.push(button);
        }
        let count = buttons.len();
        AlertDialogC {
            window,
            default_button: button_index_raw(props.int(PropName::DefaultButton, -1), count),
            cancel_button: button_index_raw(props.int(PropName::CancelButton, -1), count),
            button_pointers: buttons.iter().map(|_| PointerState::default()).collect(),
            buttons,
        }
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, cx: &mut BuildCx<'_>) {
        match name {
            PropName::DefaultButton => {
                self.default_button = button_index_raw(prop_i64(value, -1), self.buttons.len());
            }
            PropName::CancelButton => {
                self.cancel_button = button_index_raw(prop_i64(value, -1), self.buttons.len());
            }
            PropName::Message | PropName::Detail | PropName::Buttons => {
                // Rebuilding the chrome from a later prop change is out of
                // this task's scope (`AlertDialog` is built once from a
                // fixed message the application does not usually mutate in
                // place); the initial value `build` already read stands.
            }
            _ => <WindowC as Controller<Msg>>::set_prop(&mut self.window, node, name, value, cx),
        }
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        let mut msgs = <WindowC as Controller<Msg>>::on_event(&mut self.window, ev, cx);
        for (index, button) in self.buttons.iter().enumerate() {
            let Some(rect) = local_rect(cx.tree, cx.node, button) else {
                continue;
            };
            let shifted = shift_event(ev, rect);
            if self.button_pointers[index].observe(
                button,
                &shifted,
                Some(Rect::new(0.0, 0.0, rect.width, rect.height)),
            ) {
                cx.handled = true;
                if let Some(m) = cx.handlers.fire_index(EventKind::Response, index) {
                    msgs.push(m);
                }
            }
        }
        if let Event::Key(key) = ev
            && key.pressed
        {
            use xkbcommon::xkb::keysyms;
            let sym = u32::from(key.keysym);
            let target = if sym == keysyms::KEY_Escape {
                Some(self.cancel_button)
            } else if matches!(
                sym,
                s if s == keysyms::KEY_Return
                    || s == keysyms::KEY_KP_Enter
                    || s == keysyms::KEY_ISO_Enter
            ) {
                Some(self.default_button)
            } else {
                None
            };
            if let Some(index) = target {
                cx.handled = true;
                if let Some(i) = index
                    && let Some(m) = cx.handlers.fire_index(EventKind::Response, i)
                {
                    msgs.push(m);
                }
            }
        }
        msgs
    }
}

#[cfg(test)]
mod tests {
    use crate::view::{Event, EventKind, Handler, Kind, Prop, PropName, Props};
    use crate::widgets::alert_dialog::AlertDialogC;
    use crate::widgets::{Headless, build_widget, matches_fixture};

    fn props() -> Props {
        let mut p = Props::default();
        p.set(PropName::Message, Prop::Str("Discard changes?".into()));
        p.set(
            PropName::Detail,
            Prop::Str("They cannot be recovered.".into()),
        );
        p.set(
            PropName::Buttons,
            Prop::Classes(["Cancel", "Discard"].iter().map(|s| (*s).into()).collect()),
        );
        p.set(PropName::DefaultButton, Prop::Int(1));
        p.set(PropName::CancelButton, Prop::Int(0));
        p
    }

    #[test]
    fn the_dialog_is_window_dialog_message_with_a_title_label() {
        // Mutation check: dropping `.title` from the primary label makes
        // GtkMessageDialog's bold heading render as body text
        // (gtk/gtkmessagedialog.c:342).
        let built = build_widget::<()>(Kind::AlertDialog, &props());
        matches_fixture(
            &built.node,
            "window.dialog.message\n├── label.title\n├── [label]\n╰── box\n    ├── button\n    ┊\n    ╰── button\n",
        )
        .expect("alert_dialog fixture");
    }

    #[test]
    fn escape_fires_the_cancel_button_and_enter_the_default_one() {
        // Interaction test. Mutation check: firing index 0 for both makes
        // Enter cancel, which is how a destructive dialog does the opposite
        // of what the user pressed.
        let built = build_widget::<usize>(Kind::AlertDialog, &props());
        let mut c = built.controller;
        let mut hx = Headless::new();
        let mut cx = hx.event_cx_with_handlers(&built.node, |h| {
            h.set(EventKind::Response, Handler::Index(std::rc::Rc::new(|i| i)));
        });
        assert_eq!(
            c.on_event(&Event::Key(Headless::key("Escape")), &mut cx),
            vec![0]
        );
        assert_eq!(
            c.on_event(&Event::Key(Headless::key("Return")), &mut cx),
            vec![1]
        );
    }

    #[test]
    fn out_of_range_default_and_cancel_indices_are_ignored_not_panicked_on() {
        // The indices come from the application model.
        let mut p = props();
        p.set(PropName::DefaultButton, Prop::Int(i64::MAX));
        p.set(PropName::CancelButton, Prop::Int(-3));
        let built = build_widget::<usize>(Kind::AlertDialog, &p);
        // Reconciliation: the task text calls this `&built.controller`, but
        // `Box<dyn Controller<Msg>>`'s deref coercion to `&dyn
        // Controller<Msg>` does not resolve at a generic call site without
        // help -- `PopoverMenuC::mnemonic_of`'s own module doc hits the
        // identical gap and fixes it the same way, with `.as_ref()`.
        assert_eq!(AlertDialogC::default_of(built.controller.as_ref()), None);
        assert_eq!(AlertDialogC::cancel_of(built.controller.as_ref()), None);
    }
}
