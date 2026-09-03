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
use crate::layout::Rect;
use crate::text::{Ellipsize, TextLayout, TextStyle, WrapMode};
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
                let text = self.buffer.clone();
                if let Some(msg) = cx.handlers.fire_text(EventKind::Change, &text) {
                    return vec![msg];
                }
            }
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
        _cx: &mut crate::view::controller::PaintCx<'_>,
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
}
