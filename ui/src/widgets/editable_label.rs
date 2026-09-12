//! `GtkEditableLabel` — `Kind::EditableLabel`, node `editablelabel`.
//!
//! ```text
//! editablelabel[.editing]
//! ╰── stack
//!     ├── label
//!     ╰── text
//! ```
//!
//! Enter starts editing from the label and commits from the text; Escape
//! reverts to the last committed value — GTK's `editing.start`/`editing.stop`.

use std::rc::Rc;

use crate::css::node::Node;
use crate::layout::Rect;
use crate::text_input::{ContentHint, ContentPurpose};
use crate::view::cmd::Cmd;
use crate::view::controller::{Controller, Event, EventCx};
use crate::view::{BuildCx, EventKind, Kind, Prop, PropName, Props, View};
use crate::widgets::PointerState;
use crate::widgets::edit::{EditOutcome, TextEditState};
use crate::window::focus::FocusCause;

/// The floor under a measured width — `EntryC`'s own `CARET_WIDTH_PX`, so an
/// empty label stays clickable instead of measuring to zero.
const CARET_WIDTH_PX: f32 = 8.0;

/// A `GtkEditableLabel` showing `text`.
#[must_use]
pub fn editable_label<Msg: Clone + 'static>(text: &str) -> View<Msg> {
    View::new(Kind::EditableLabel).prop(PropName::Text, Prop::Str(Rc::from(text)))
}

/// `GtkEditableLabel`'s own setters and signals.
pub trait EditableLabelExt<Msg>: Sized {
    /// `GtkEditableLabel:editing`.
    fn editing(self, on: bool) -> Self;
    // `GtkEditable::changed`, fired on commit, is the inherent
    // `View::on_change`, which shadows any same-named trait method at every
    // call site anyway.
}

impl<Msg: Clone + 'static> EditableLabelExt<Msg> for View<Msg> {
    fn editing(self, on: bool) -> Self {
        self.prop(PropName::Editable, Prop::Bool(on))
    }
}

/// `Kind::EditableLabel`'s controller.
pub struct EditableLabelC {
    /// The text half's editing state.
    pub edit: TextEditState,
    /// Whether the widget is in editing mode.
    pub editing: bool,
    /// The last committed value, which Escape reverts to.
    pub committed: String,
    /// The `stack` subnode.
    pub stack: Node,
    /// The `label` subnode inside the stack.
    pub label: Node,
    pointer: PointerState,
}

impl EditableLabelC {
    fn apply(&self, node: &Node) {
        if self.editing {
            node.add_class("editing");
        } else {
            node.remove_class("editing");
        }
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for EditableLabelC {
    fn kind(&self) -> Kind {
        Kind::EditableLabel
    }

    fn build(node: &Node, props: &Props, cx: &mut BuildCx<'_>) -> Self {
        let stack = Node::new("stack");
        node.append_child(&stack);
        let label = Node::new("label");
        stack.append_child(&label);
        let text = props.str(PropName::Text).unwrap_or("").to_owned();
        let edit = TextEditState::build(&stack, &text, cx);
        let this = EditableLabelC {
            edit,
            editing: props.bool(PropName::Editable, false),
            committed: text,
            stack,
            label,
            pointer: PointerState::default(),
        };
        this.apply(node);
        this
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, cx: &mut BuildCx<'_>) {
        match (name, value) {
            (PropName::Text, Prop::Str(text)) => {
                self.committed = text.to_string();
                self.edit.set_text(text, cx);
            }
            (PropName::Editable, Prop::Bool(on)) => self.editing = *on,
            _ => return,
        }
        self.apply(node);
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        use xkbcommon::xkb::keysyms;
        // Plan reconciliation: the plan's `on_event` never granted keyboard
        // focus at all, the same gap `Entry`/`TextView` had — without this
        // an `EditableLabel` could never receive `Key` events (`Enter` to
        // start editing, typed characters, `Escape` to revert) by any means,
        // since `InputEvent::Key` only routes to `FocusRing::focus()` and
        // that only ever moves from a click, `Cmd::Focus`, or a keyboard
        // binding (contract §11 E — matching the note in `entry.rs`).
        if matches!(ev, Event::PointerDown { .. }) {
            cx.focus.set_focus(Some(cx.node), FocusCause::Pointer);
        }
        // Plan reconciliation: `stack`/`label` are subnodes this controller
        // appends directly rather than `View` children (the same gap
        // `ScaleC::on_event` documents for its own `trough`/`slider`), so
        // they never get a taffy box of their own and
        // `local_rect(cx.tree, cx.node, &self.label)` is always `None` — the
        // click-to-edit hit box below is derived from the root's own
        // allocation instead.
        if let Some(alloc) = cx.tree.allocation(cx.node) {
            let border = alloc.border_box;
            let bounds = Rect::new(0.0, 0.0, border.width, border.height);
            if self.pointer.observe(&self.label, ev, Some(bounds)) && !self.editing {
                self.editing = true;
                self.apply(cx.node);
                // Editing starts here, so the IME session does too.
                let tree = cx.tree;
                let node = cx.node;
                cx.cmds.extend(self.edit.ime_enable_cmds(
                    ContentHint::NONE,
                    ContentPurpose::Normal,
                    tree,
                    node,
                ));
                cx.handled = true;
                return Vec::new();
            }
        }
        // IME follows *editing*, not just focus: composition only applies
        // while editing (a commit outside it would write a buffer the model
        // never learns). Focus loss always ends the session, editing or not.
        if matches!(ev, Event::FocusOut) {
            cx.cmds.push(Cmd::ImeDisable);
            return Vec::new();
        }
        if self.editing
            && let Some(msgs) =
                self.edit
                    .ime_event(ev, cx, ContentHint::NONE, ContentPurpose::Normal)
        {
            return msgs;
        }
        let Event::Key(key) = ev else {
            return Vec::new();
        };
        if key.pressed && u32::from(key.keysym) == keysyms::KEY_Escape && self.editing {
            self.editing = false;
            self.apply(cx.node);
            cx.handled = true;
            // Revert without an undo step: the model never saw this edit.
            // The revert also ends the IME session and drops any staged
            // composition, which was over the discarded text.
            self.edit.buffer.clone_from(&self.committed);
            self.edit.cursor = self.edit.buffer.len();
            self.edit.anchor = None;
            self.edit.apply_ime_preedit(None);
            cx.cmds.push(Cmd::ImeDisable);
            return Vec::new();
        }
        if !self.editing {
            if key.pressed
                && matches!(
                    u32::from(key.keysym),
                    keysyms::KEY_Return | keysyms::KEY_KP_Enter
                )
            {
                self.editing = true;
                self.apply(cx.node);
                let tree = cx.tree;
                let node = cx.node;
                cx.cmds.extend(self.edit.ime_enable_cmds(
                    ContentHint::NONE,
                    ContentPurpose::Normal,
                    tree,
                    node,
                ));
                cx.handled = true;
            }
            return Vec::new();
        }
        match self.edit.key(key, cx) {
            EditOutcome::Activated => {
                self.editing = false;
                self.apply(cx.node);
                self.committed.clone_from(&self.edit.buffer);
                self.edit.apply_ime_preedit(None);
                cx.handled = true;
                cx.cmds.push(Cmd::ImeDisable);
                let text = self.committed.clone();
                cx.handlers
                    .fire_text(EventKind::Change, &text)
                    .map_or_else(Vec::new, |m| vec![m])
            }
            EditOutcome::Changed => {
                // `Changed` fires no `Change` here: an editable label
                // reports `GtkEditable::changed` on commit (`Activated`
                // above), not per keystroke. The IME still needs the fresh
                // surrounding text, so it re-syncs silently.
                cx.handled = true;
                self.edit
                    .ime_resync(cx, ContentHint::NONE, ContentPurpose::Normal);
                Vec::new()
            }
            EditOutcome::Moved => {
                cx.handled = true;
                Vec::new()
            }
            EditOutcome::Ignored => Vec::new(),
        }
    }

    fn measure(
        &mut self,
        _available: (Option<f32>, Option<f32>),
        cx: &mut BuildCx<'_>,
    ) -> Option<(f32, f32)> {
        // Plan reconciliation: the plan gave no `measure` at all. `stack`,
        // `label` and `text` are appended directly (not `View` children), so
        // taffy sees no children to size against and the whole control fell
        // back to a zero-content-box default (`EntryC`'s controller hit the
        // same gap; its own `CARET_WIDTH_PX` floor is reused here).
        self.edit.reshape(cx);
        let (w, h) = self.edit.layout.size();
        Some((w.max(CARET_WIDTH_PX), h))
    }

    fn paint(
        &mut self,
        canvas: &mut skia_rs_safe::canvas::Canvas<'_>,
        alloc: &crate::layout::Allocation,
        style: &crate::css::computed::ComputedStyle,
        cx: &mut crate::view::controller::PaintCx<'_>,
    ) -> bool {
        let content = alloc.content_box;
        self.edit
            .layout
            .draw(canvas, (content.x, content.y), style.color());
        // A preedit is only ever staged while editing (see `on_event`'s
        // gate), so this is a no-op in label mode.
        self.edit.paint_preedit(canvas, &content, style.color(), cx);
        true
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use crate::view::cmd::Cmd;
    use crate::view::controller::Event;
    use crate::view::{EventKind, Handler, Handlers, Kind, Prop, PropName, Props};
    use crate::widgets::{Headless, build_widget};
    use crate::window::focus::FocusCause;

    fn label_hi() -> (Headless, crate::widgets::BuiltWidget<String>) {
        let mut props = Props::default();
        props.set(PropName::Text, Prop::Str("hi".into()));
        (
            Headless::new(),
            build_widget::<String>(Kind::EditableLabel, &props),
        )
    }

    fn has_enable(cmds: &[Cmd<String>]) -> bool {
        cmds.iter().any(|cmd| matches!(cmd, Cmd::ImeEnable { .. }))
    }

    #[test]
    fn focus_alone_starts_no_ime_session() {
        // mutation: enable on every `FocusIn` and an IME session composes
        // into a label that is not editing, whose model never learns it.
        let (mut hx, built) = label_hi();
        let mut c = built.controller;
        let mut cx = hx.event_cx::<String>(&built.node);
        c.on_event(
            &Event::FocusIn {
                cause: FocusCause::Pointer,
            },
            &mut cx,
        );
        assert!(
            cx.cmds.is_empty(),
            "no session outside editing, saw {:?}",
            cx.cmds
        );
    }

    #[test]
    fn starting_to_edit_enables_ime_and_a_commit_fires_change() {
        // mutation: skip the enable on the editing transition and the IME
        // never learns the field it is composing into.
        let (mut hx, built) = label_hi();
        let mut c = built.controller;
        let mut cx = hx.event_cx::<String>(&built.node);
        c.on_event(&Event::Key(Headless::key("Return")), &mut cx);
        assert!(
            has_enable(cx.cmds),
            "editing start enables IME, saw {:?}",
            cx.cmds
        );

        let mut cx = hx.event_cx_with_handlers(&built.node, |handlers: &mut Handlers<String>| {
            handlers.set(
                EventKind::Change,
                Handler::Text(Rc::new(|text: &str| text.to_owned())),
            );
        });
        let msgs = c.on_event(&Event::ImeCommit("!".to_owned()), &mut cx);
        assert_eq!(msgs, vec!["hi!".to_owned()]);
    }

    #[test]
    fn escape_reverts_and_disables_ime() {
        // mutation: skip the disable on revert and the session leaks after
        // editing ends.
        let (mut hx, built) = label_hi();
        let mut c = built.controller;
        let mut cx = hx.event_cx::<String>(&built.node);
        c.on_event(&Event::Key(Headless::key("Return")), &mut cx);
        cx.cmds.clear();
        c.on_event(&Event::Key(Headless::key("Escape")), &mut cx);
        assert!(
            cx.cmds.iter().any(|cmd| matches!(cmd, Cmd::ImeDisable)),
            "editing end disables IME, saw {:?}",
            cx.cmds
        );
    }
}
