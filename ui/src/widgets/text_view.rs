//! `GtkTextView` — `Kind::TextView`, CSS node `textview`, always `.view`.
//!
//! ```text
//! textview.view
//! ├── border.top
//! ├── border.left
//! ├── text
//! │   ╰── [selection]
//! ├── border.right
//! ├── border.bottom
//! ╰── [window.popup]
//! ```
//!
//! Plain text only (spec §Out of scope). Editing keys follow `GtkText`'s own
//! table, which contract §5.3 makes the single source for the whole family.

use std::rc::Rc;
use std::time::Duration;

use crate::css::computed::ComputedStyle;
use crate::css::node::Node;
use crate::layout::{LayoutTree, Rect};
use crate::text::{Ellipsize, TextLayout, TextStyle, WrapMode};
use crate::text_input::{
    ContentHint, ContentPurpose, Preedit, Snapshot, apply_commit_string, apply_delete_surrounding,
    caret_surface_rect,
};
use crate::view::cmd::Cmd;
use crate::view::controller::{Controller, Event, EventCx};
use crate::view::{BuildCx, EventKind, Kind, Prop, PropName, Props, View};
use crate::widgets::edit::{UndoStack, clamp_to_boundary};
use crate::widgets::{PointerState, local_rect, shift_event};
use crate::window::keyboard::Mods;
use crate::window::pointer::Kinetic;

/// The floor `measure` reports for an empty buffer's width — a real caret's
/// worth of area, so the node is never zero-area and so never unhittable
/// (`window/pointer.rs::descend` skips zero-area nodes outright).
const CARET_WIDTH_PX: f32 = 8.0;

/// A `GtkTextView` over `text`.
#[must_use]
pub fn text_view<Msg: Clone + 'static>(text: &str) -> View<Msg> {
    View::new(Kind::TextView).prop(PropName::Text, Prop::Str(Rc::from(text)))
}

/// `GtkTextView`'s own setters.
pub trait TextViewExt<Msg>: Sized {
    /// `GtkTextView:editable`.
    fn editable(self, on: bool) -> Self;
    /// `GtkTextView:wrap-mode`.
    fn wrap_mode(self, mode: WrapMode) -> Self;
    /// `GtkTextView:monospace`.
    fn monospace(self, on: bool) -> Self;
    /// `GtkTextView:cursor-visible`.
    fn cursor_visible(self, on: bool) -> Self;
    /// `GtkTextView:left-margin`.
    fn left_margin(self, px: i32) -> Self;
    /// `GtkTextView:right-margin`.
    fn right_margin(self, px: i32) -> Self;
    /// `GtkTextView:top-margin`.
    fn top_margin(self, px: i32) -> Self;
    /// `GtkTextView:bottom-margin`.
    fn bottom_margin(self, px: i32) -> Self;
    /// `GtkTextView:enable-undo`.
    fn enable_undo(self, on: bool) -> Self;
    // `GtkTextBuffer::changed` is the inherent `View::on_change`, which
    // shadows any same-named trait method at every call site anyway.
}

impl<Msg: Clone + 'static> TextViewExt<Msg> for View<Msg> {
    fn editable(self, on: bool) -> Self {
        self.prop(PropName::Editable, Prop::Bool(on))
    }
    fn wrap_mode(self, mode: WrapMode) -> Self {
        self.prop(
            PropName::Wrap,
            Prop::Enum(match mode {
                WrapMode::None => 0,
                WrapMode::Word => 1,
                WrapMode::Char => 2,
                WrapMode::WordChar => 3,
            }),
        )
    }
    fn monospace(self, on: bool) -> Self {
        self.prop(PropName::Homogeneous, Prop::Bool(on))
    }
    fn cursor_visible(self, on: bool) -> Self {
        self.prop(PropName::Visibility, Prop::Bool(on))
    }
    fn left_margin(self, px: i32) -> Self {
        self.prop(PropName::Column, Prop::Int(i64::from(px)))
    }
    fn right_margin(self, px: i32) -> Self {
        self.prop(PropName::ColumnSpan, Prop::Int(i64::from(px)))
    }
    fn top_margin(self, px: i32) -> Self {
        self.prop(PropName::Row, Prop::Int(i64::from(px)))
    }
    fn bottom_margin(self, px: i32) -> Self {
        self.prop(PropName::RowSpan, Prop::Int(i64::from(px)))
    }
    fn enable_undo(self, on: bool) -> Self {
        self.prop(PropName::EnableUndo, Prop::Bool(on))
    }
}

/// `Kind::TextView`'s controller.
pub struct TextViewC {
    /// The plain-text buffer.
    pub buffer: String,
    /// The wrapped paragraph over `buffer`.
    pub layout: TextLayout,
    /// Cursor byte offset.
    pub cursor: usize,
    /// Selection anchor; `None` means no selection.
    pub anchor: Option<usize>,
    /// Scroll offset in px.
    pub scroll: (f32, f32),
    /// Kinetic scrolling state, fed from touch and finger axis events.
    pub kinetic: Kinetic,
    /// The `text` subnode.
    pub text_node: Node,
    /// The `selection` subnode, present only while a selection exists.
    pub selection_node: Option<Node>,
    /// The in-flight IME preedit: display-only text that never touches
    /// [`TextViewC::buffer`] until a `commit_string` arrives.
    pub preedit: Option<Preedit>,
    /// The undo stack, honouring `GtkTextView:enable-undo`.
    pub undo: UndoStack,
    editable: bool,
    wrap: WrapMode,
    pointer: PointerState,
}

impl TextViewC {
    fn reshape(&mut self, cx: &mut BuildCx<'_>, width: Option<f32>) {
        let style = TextStyle::from_computed(&ComputedStyle::initial(cx.env));
        self.layout = TextLayout::build(
            &self.buffer,
            &style,
            cx.fonts,
            width,
            self.wrap,
            Ellipsize::None,
        );
    }

    /// Keep the `selection` subnode in sync with `anchor`.
    fn sync_selection(&mut self) {
        let has = self.anchor.is_some_and(|a| a != self.cursor);
        match (has, self.selection_node.take()) {
            (true, Some(node)) => self.selection_node = Some(node),
            (true, None) => {
                let node = Node::new("selection");
                self.text_node.append_child(&node);
                self.selection_node = Some(node);
            }
            (false, Some(node)) => node.detach(),
            (false, None) => {}
        }
    }

    /// The `ImeEnable` + `ImeSync` pair for `FocusIn`: a wrapped view is
    /// multiline, so the IME offers a matching layout.
    fn ime_enable_cmds<Msg>(&self, tree: &LayoutTree, node: &Node) -> [Cmd<Msg>; 2] {
        [
            Cmd::ImeEnable {
                hint: ContentHint::MULTILINE.wire(),
                purpose: ContentPurpose::Normal.wire(),
            },
            self.ime_sync_cmd(tree, node),
        ]
    }

    /// The `ImeSync` for a buffer or caret move while focused.
    fn ime_sync_cmd<Msg>(&self, tree: &LayoutTree, node: &Node) -> Cmd<Msg> {
        let caret = self.layout.caret_rect(self.cursor);
        Cmd::ImeSync(Snapshot::new(
            &self.buffer,
            self.cursor,
            self.anchor,
            ContentHint::MULTILINE,
            ContentPurpose::Normal,
            caret_surface_rect(tree, node, &caret, self.scroll),
        ))
    }

    /// Push the `ImeSync` for a buffer or caret move while focused.
    fn ime_resync<Msg>(&self, cx: &mut EventCx<'_, Msg>) {
        let tree = cx.tree;
        let node = cx.node;
        cx.cmds.push(self.ime_sync_cmd(tree, node));
    }

    /// Apply an IME `commit_string`: splice over the selection, record one
    /// undo step, and consume any pending preedit. Returns whether the
    /// buffer changed.
    fn apply_ime_commit(&mut self, text: &str, now: Duration) -> bool {
        let before = self.buffer.clone();
        let (buffer, cursor) = apply_commit_string(&self.buffer, self.cursor, self.anchor, text);
        self.preedit = None;
        if buffer == before {
            return false;
        }
        self.buffer = buffer;
        self.cursor = cursor;
        self.anchor = None;
        self.undo.record(&before, &self.buffer, self.cursor, now);
        self.sync_selection();
        true
    }

    /// Apply an IME `delete_surrounding_text` (UTF-8 byte lengths around the
    /// cursor, clamped to whole characters). Returns whether the buffer
    /// changed.
    fn apply_ime_delete(&mut self, before_len: u32, after_len: u32, now: Duration) -> bool {
        let before = self.buffer.clone();
        let (buffer, cursor) =
            apply_delete_surrounding(&self.buffer, self.cursor, before_len, after_len);
        if buffer == before {
            return false;
        }
        self.buffer = buffer;
        self.cursor = cursor;
        self.anchor = None;
        self.undo.record(&before, &self.buffer, self.cursor, now);
        self.sync_selection();
        true
    }

    /// Apply one key to the buffer. Returns `true` if the buffer changed.
    fn apply_key(&mut self, key: &crate::window::keyboard::KeyEvent, now: Duration) -> bool {
        use xkbcommon::xkb::keysyms;
        if !key.pressed {
            return false;
        }
        let ctrl = key.effective_mods().contains(Mods::CTRL);
        let shift = key.effective_mods().contains(Mods::SHIFT);
        let extend = |this: &mut Self, to: usize| {
            if shift {
                this.anchor.get_or_insert(this.cursor);
            } else {
                this.anchor = None;
            }
            this.cursor = to;
        };
        match u32::from(key.keysym) {
            keysyms::KEY_z if ctrl && !shift => match self.undo.undo() {
                Some((buffer, cursor)) => {
                    self.buffer = buffer;
                    self.cursor = clamp_to_boundary(&self.buffer, cursor);
                    self.anchor = None;
                    true
                }
                None => false,
            },
            keysyms::KEY_y | keysyms::KEY_Z if ctrl => match self.undo.redo() {
                Some((buffer, cursor)) => {
                    self.buffer = buffer;
                    self.cursor = clamp_to_boundary(&self.buffer, cursor);
                    self.anchor = None;
                    true
                }
                None => false,
            },
            keysyms::KEY_Left => {
                let to = if ctrl {
                    self.layout.prev_word(self.cursor)
                } else {
                    self.layout.prev_grapheme(self.cursor)
                };
                extend(self, to);
                false
            }
            keysyms::KEY_Right => {
                let to = if ctrl {
                    self.layout.next_word(self.cursor)
                } else {
                    self.layout.next_grapheme(self.cursor)
                };
                extend(self, to);
                false
            }
            keysyms::KEY_Home => {
                extend(self, 0);
                false
            }
            keysyms::KEY_End => {
                let end = self.buffer.len();
                extend(self, end);
                false
            }
            keysyms::KEY_a if ctrl => {
                self.anchor = Some(0);
                self.cursor = self.buffer.len();
                false
            }
            keysyms::KEY_BackSpace if self.editable => {
                let before = self.buffer.clone();
                let range = self.selection_range();
                if range.is_empty() {
                    let from = self.layout.prev_grapheme(self.cursor);
                    if from == self.cursor {
                        return false;
                    }
                    self.buffer.replace_range(from..self.cursor, "");
                    self.cursor = from;
                } else {
                    self.buffer.replace_range(range.clone(), "");
                    self.cursor = range.start;
                }
                self.anchor = None;
                self.undo.record(&before, &self.buffer, self.cursor, now);
                true
            }
            keysyms::KEY_Delete if self.editable => {
                let before = self.buffer.clone();
                let range = self.selection_range();
                if range.is_empty() {
                    let to = self.layout.next_grapheme(self.cursor);
                    if to == self.cursor {
                        return false;
                    }
                    self.buffer.replace_range(self.cursor..to, "");
                } else {
                    self.buffer.replace_range(range.clone(), "");
                    self.cursor = range.start;
                }
                self.anchor = None;
                self.undo.record(&before, &self.buffer, self.cursor, now);
                true
            }
            _ => {
                let Some(text) = key.utf8.as_deref().filter(|_| self.editable && !ctrl) else {
                    return false;
                };
                if text.chars().all(|c| c.is_control() && c != '\n') {
                    return false;
                }
                let before = self.buffer.clone();
                let range = self.selection_range();
                self.buffer.replace_range(range.clone(), text);
                self.cursor = range.start + text.len();
                self.anchor = None;
                self.undo.record(&before, &self.buffer, self.cursor, now);
                true
            }
        }
    }

    /// The ordered, clamped selection range; empty when there is no selection.
    fn selection_range(&self) -> std::ops::Range<usize> {
        let anchor = clamp_to_boundary(&self.buffer, self.anchor.unwrap_or(self.cursor));
        let cursor = clamp_to_boundary(&self.buffer, self.cursor);
        anchor.min(cursor)..anchor.max(cursor)
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for TextViewC {
    fn kind(&self) -> Kind {
        Kind::TextView
    }

    fn build(node: &Node, props: &Props, cx: &mut BuildCx<'_>) -> Self {
        node.add_class("view");
        for side in ["top", "left"] {
            let border = Node::with_classes("border", &[side]);
            node.append_child(&border);
        }
        let text_node = Node::new("text");
        node.append_child(&text_node);
        for side in ["right", "bottom"] {
            let border = Node::with_classes("border", &[side]);
            node.append_child(&border);
        }
        let mut this = TextViewC {
            buffer: props.str(PropName::Text).unwrap_or("").to_owned(),
            layout: TextLayout::build(
                "",
                &TextStyle::from_computed(&ComputedStyle::initial(cx.env)),
                cx.fonts,
                None,
                WrapMode::None,
                Ellipsize::None,
            ),
            cursor: 0,
            anchor: None,
            scroll: (0.0, 0.0),
            kinetic: Kinetic::default(),
            text_node,
            selection_node: None,
            preedit: None,
            undo: UndoStack::new(props.bool(PropName::EnableUndo, true)),
            editable: props.bool(PropName::Editable, true),
            wrap: match props.get(PropName::Wrap) {
                Some(Prop::Enum(1)) => WrapMode::Word,
                Some(Prop::Enum(2)) => WrapMode::Char,
                Some(Prop::Enum(3)) => WrapMode::WordChar,
                _ => WrapMode::None,
            },
            pointer: PointerState::default(),
        };
        this.reshape(cx, None);
        this
    }

    fn set_prop(&mut self, _node: &Node, name: PropName, value: &Prop, cx: &mut BuildCx<'_>) {
        match (name, value) {
            (PropName::Text, Prop::Str(text)) => {
                if self.buffer.as_str() != text.as_ref() {
                    self.buffer = text.to_string();
                    self.cursor = clamp_to_boundary(&self.buffer, self.cursor);
                    self.anchor = None;
                    self.preedit = None;
                    self.reshape(cx, None);
                    self.sync_selection();
                }
            }
            (PropName::Editable, Prop::Bool(on)) => {
                self.editable = *on;
                if *on {
                    self.text_node.remove_class("readonly");
                } else {
                    self.text_node.add_class("readonly");
                }
            }
            _ => {}
        }
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        // Left button only, exactly as `GenericC` gates it
        // (`view/controller.rs`): a right- or middle-click neither focuses a
        // GTK text view nor moves its caret.
        let left_press = matches!(
            ev,
            Event::PointerDown { button, .. } if *button == crate::window::layer::BTN_LEFT
        );
        if left_press {
            // GTK grabs focus to a clicked focusable widget itself; the
            // generic controller does this for every widget it owns
            // (`view/controller.rs`'s `GenericC`), but a dedicated
            // `Controller` — this one included — must repeat it, since
            // nothing above `Controller::on_event` grants focus on click
            // (plan reconciliation: the plan's `on_event` omitted this, and
            // without it `TextView` could never receive keyboard focus).
            cx.focus
                .set_focus(Some(cx.node), crate::window::focus::FocusCause::Pointer);
        }
        if let Some(rect) = local_rect(cx.tree, cx.node, &self.text_node) {
            let shifted = shift_event(ev, rect);
            self.pointer.observe(
                &self.text_node,
                &shifted,
                Some(Rect::new(0.0, 0.0, rect.width, rect.height)),
            );
            if let Event::PointerDown { local, .. } = shifted
                && left_press
            {
                self.cursor = clamp_to_boundary(&self.buffer, self.layout.byte_at(local));
                self.anchor = None;
                self.sync_selection();
                cx.handled = true;
            }
        }
        if let Event::Key(key) = ev {
            let changed = self.apply_key(key, cx.clock.now());
            self.sync_selection();
            if changed {
                cx.handled = true;
                self.ime_resync(cx);
                let text = self.buffer.clone();
                if let Some(msg) = cx.handlers.fire_text(EventKind::Change, &text) {
                    return vec![msg];
                }
            }
        }
        // An IME session follows the focus; composition applies to the
        // buffer above. Only a real buffer change fires `Change` (and
        // re-syncs the new surrounding text back to the IME).
        match ev {
            Event::FocusIn { .. } => {
                let tree = cx.tree;
                let node = cx.node;
                cx.cmds.extend(self.ime_enable_cmds(tree, node));
            }
            Event::FocusOut => {
                cx.cmds.push(Cmd::ImeDisable);
            }
            Event::ImePreedit {
                text,
                cursor_begin,
                cursor_end,
            } => {
                self.preedit = Some(Preedit {
                    text: text.clone(),
                    cursor_begin: *cursor_begin,
                    cursor_end: *cursor_end,
                });
            }
            Event::ImeCommit(text) => {
                if self.apply_ime_commit(text, cx.clock.now()) {
                    cx.handled = true;
                    self.ime_resync(cx);
                    let buffer = self.buffer.clone();
                    if let Some(msg) = cx.handlers.fire_text(EventKind::Change, &buffer) {
                        return vec![msg];
                    }
                }
            }
            Event::ImeDelete { before, after }
                if self.apply_ime_delete(*before, *after, cx.clock.now()) =>
            {
                cx.handled = true;
                self.ime_resync(cx);
                let buffer = self.buffer.clone();
                if let Some(msg) = cx.handlers.fire_text(EventKind::Change, &buffer) {
                    return vec![msg];
                }
            }
            _ => {}
        }
        Vec::new()
    }

    fn measure(
        &mut self,
        available: (Option<f32>, Option<f32>),
        cx: &mut BuildCx<'_>,
    ) -> Option<(f32, f32)> {
        self.reshape(cx, available.0.filter(|w| w.is_finite() && *w > 0.0));
        let (w, h) = self.layout.size();
        // Plan reconciliation: `TextLayout::size` alone measures an empty
        // buffer to zero width, and a zero-area node is one the hit-tester
        // (`window/pointer.rs::descend`) skips outright — an empty
        // `TextView` could never be clicked into focus. GTK reserves the
        // caret's own width even with nothing typed yet; `CARET_WIDTH_PX`
        // is that floor, so the node always has *some* area to hit.
        Some((w.max(CARET_WIDTH_PX), h))
    }

    fn paint(
        &mut self,
        canvas: &mut skia_rs_safe::canvas::Canvas<'_>,
        alloc: &crate::layout::Allocation,
        style: &ComputedStyle,
        cx: &mut crate::view::controller::PaintCx<'_>,
    ) -> bool {
        let content = alloc.content_box;
        for rect in self.layout.selection_rects(self.selection_range()) {
            let placed = Rect::new(
                content.x + rect.x,
                content.y + rect.y,
                rect.width,
                rect.height,
            );
            canvas.draw_rect(&placed.to_skia(), &crate::paint::fill_paint(style.color()));
        }
        self.layout.draw(
            canvas,
            (content.x - self.scroll.0, content.y - self.scroll.1),
            style.color(),
        );
        // An in-flight composition paints at the caret, ahead of it. A text
        // view never masks, so the raw preedit text is what shows.
        if let Some(preedit) = self.preedit.as_ref() {
            let caret = self.layout.caret_rect(self.cursor);
            crate::widgets::edit::paint_preedit_run(
                canvas,
                &content,
                style.color(),
                &caret,
                self.scroll,
                &preedit.text,
                cx,
            );
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use crate::view::{Kind, Prop, PropName, Props};
    use crate::widgets::{Headless, build_widget};
    use crate::window::layer::BTN_LEFT;

    fn with_text() -> Props {
        let mut p = Props::default();
        p.set(PropName::Text, Prop::Str("hello world".into()));
        p
    }

    /// Mutation: gate `on_event`'s focus grab and caret move on
    /// `Event::PointerDown { .. }` again (any button, as this module
    /// shipped) and the right-click below takes the focus and moves the
    /// caret -- neither of which a GTK text view does.
    #[test]
    fn only_a_left_click_focuses_the_view_and_moves_the_caret() {
        const BTN_RIGHT: u32 = 0x111;
        let built = build_widget::<()>(Kind::TextView, &with_text());
        let mut c = built.controller;
        let mut hx = Headless::new();
        let mut cx = hx.event_cx::<()>(&built.node);
        c.on_event(
            &crate::view::controller::Event::PointerDown {
                local: (40.0, 4.0),
                button: BTN_RIGHT,
                serial: 1,
            },
            &mut cx,
        );
        assert!(
            cx.focus.focus().is_none(),
            "a right-click must not take the focus"
        );
        c.on_event(
            &crate::view::controller::Event::PointerDown {
                local: (40.0, 4.0),
                button: BTN_LEFT,
                serial: 2,
            },
            &mut cx,
        );
        assert!(
            cx.focus.focus().is_some(),
            "a left-click focuses the view, as GTK does"
        );
    }

    #[test]
    fn focus_in_reports_multiline_and_a_commit_fires_change() {
        // mutation: enable with no multiline hint and the IME offers a
        // single-line layout for a wrapped view; mutation: skip the commit
        // apply and the composed text never lands.
        use std::rc::Rc;

        use crate::view::cmd::Cmd;
        use crate::view::controller::Event;
        use crate::view::{EventKind, Handler, Handlers, Kind, Prop, PropName, Props};
        use crate::widgets::{Headless, build_widget};
        use crate::window::focus::FocusCause;

        let mut props = Props::default();
        props.set(PropName::Text, Prop::Str("hi".into()));
        let (mut hx, built) = (
            Headless::new(),
            build_widget::<String>(Kind::TextView, &props),
        );
        let mut c = built.controller;
        let mut cx = hx.event_cx::<String>(&built.node);
        c.on_event(
            &Event::FocusIn {
                cause: FocusCause::Pointer,
            },
            &mut cx,
        );
        let enables: Vec<_> = cx
            .cmds
            .iter()
            .filter_map(|cmd| match cmd {
                Cmd::ImeEnable { hint, purpose } => Some((*hint, *purpose)),
                _ => None,
            })
            .collect();
        assert_eq!(
            enables,
            vec![(0x200, 0)],
            "a text view enables multiline/normal, saw {:?}",
            cx.cmds
        );

        let mut cx = hx.event_cx_with_handlers(&built.node, |handlers: &mut Handlers<String>| {
            handlers.set(
                EventKind::Change,
                Handler::Text(Rc::new(|text: &str| text.to_owned())),
            );
        });
        let msgs = {
            // `TextViewC` builds with the cursor at 0; move it to the end
            // through the public key path before composing.
            c.on_event(&Event::Key(Headless::key("End")), &mut cx);
            c.on_event(&Event::ImeCommit("\nthere".to_owned()), &mut cx)
        };
        assert_eq!(msgs, vec!["hi\nthere".to_owned()]);
    }
}
