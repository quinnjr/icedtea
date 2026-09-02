//! `GtkPasswordEntry` — `Kind::PasswordEntry`, node `entry`, class `.password`.
//!
//! ```text
//! entry.password
//! ╰── text
//!     ├── image.caps-lock-indicator
//!     ┊
//! ```
//!
//! The caps-lock indicator follows `Mods::CAPS` on every key event, which is
//! where the modifier mask reaches the toolkit. The peek icon flips
//! `TextEditState::visibility`, the `misc.toggle-visibility` action.

use std::rc::Rc;

use crate::css::node::{Node, PseudoStates};
use crate::layout::Rect;
use crate::view::controller::{Controller, Event, EventCx};
use crate::view::{BuildCx, EventKind, Handler, Kind, Prop, PropName, Props, View};
use crate::widgets::edit::{EditOutcome, TextEditState};
use crate::widgets::{PointerState, content_rect_local, shift_event};
use crate::window::focus::FocusCause;
use crate::window::keyboard::Mods;

/// A `GtkPasswordEntry` holding `text`.
#[must_use]
pub fn password_entry<Msg: Clone + 'static>(text: &str) -> View<Msg> {
    View::new(Kind::PasswordEntry).prop(PropName::Text, Prop::Str(Rc::from(text)))
}

/// `GtkPasswordEntry`'s own setters and signals.
pub trait PasswordEntryExt<Msg>: Sized {
    /// `GtkPasswordEntry:placeholder-text`.
    fn placeholder(self, text: &str) -> Self;
    /// `GtkPasswordEntry:show-peek-icon`.
    fn show_peek_icon(self, on: bool) -> Self;
    /// `GtkPasswordEntry:activates-default`.
    fn activates_default(self, on: bool) -> Self;
    /// `GtkEditable::changed`.
    fn on_change(self, f: impl Fn(&str) -> Msg + 'static) -> Self;
    /// `GtkPasswordEntry::activate`.
    fn on_activate(self, msg: Msg) -> Self;
}

impl<Msg: Clone + 'static> PasswordEntryExt<Msg> for View<Msg> {
    fn placeholder(self, text: &str) -> Self {
        self.prop(PropName::Placeholder, Prop::Str(Rc::from(text)))
    }
    fn show_peek_icon(self, on: bool) -> Self {
        self.prop(PropName::ShowArrow, Prop::Bool(on))
    }
    fn activates_default(self, on: bool) -> Self {
        self.prop(PropName::Modal, Prop::Bool(on))
    }
    fn on_change(self, f: impl Fn(&str) -> Msg + 'static) -> Self {
        self.on(EventKind::Change, Handler::Text(Rc::new(f)))
    }
    fn on_activate(self, msg: Msg) -> Self {
        self.on(EventKind::Activate, Handler::Unit(msg))
    }
}

/// Plan reconciliation: see [`PasswordEntryC::on_event`]'s own note — the
/// space this controller reserves, at the right of its content box, for the
/// peek icon's hit region.
///
/// Public for the same reason [`crate::widgets::spin_button::STEPPER_SIZE`]
/// is: `image.peek` carries no allocation of its own, so a test that has to
/// click the peek icon derives its point from the root's border box and this
/// width instead of from a magic number.
pub const PEEK_WIDTH_PX: f32 = 24.0;

/// Plan reconciliation: see [`crate::widgets::entry`]'s own `CARET_WIDTH_PX`
/// — an empty buffer measures to zero width, and a zero-area node is one
/// `window/pointer.rs::descend` skips outright, so an empty `PasswordEntry`
/// could never be clicked into focus without this floor.
const CARET_WIDTH_PX: f32 = 8.0;

/// `Kind::PasswordEntry`'s controller.
pub struct PasswordEntryC {
    /// The shared editing state; `visibility` starts `false`.
    pub edit: TextEditState,
    /// Whether the peek icon is currently revealing the text.
    pub peek: bool,
    /// Whether Caps Lock is on.
    pub caps_lock: bool,
    /// The `image.caps-lock-indicator` subnode.
    pub caps_node: Option<Node>,
    /// The peek `image` subnode, when `show-peek-icon`.
    pub peek_node: Option<Node>,
    pointer: PointerState,
}

impl<Msg: Clone + 'static> Controller<Msg> for PasswordEntryC {
    fn kind(&self) -> Kind {
        Kind::PasswordEntry
    }

    fn build(node: &Node, props: &Props, cx: &mut BuildCx<'_>) -> Self {
        node.add_class("password");
        let mut edit = TextEditState::build(node, props.str(PropName::Text).unwrap_or(""), cx);
        edit.visibility = false;
        edit.reshape(cx);
        let caps_node = Node::with_classes("image", &["caps-lock-indicator"]);
        edit.text_node.append_child(&caps_node);
        let peek_node = props.bool(PropName::ShowArrow, false).then(|| {
            let peek = Node::with_classes("image", &["peek"]);
            node.append_child(&peek);
            peek
        });
        PasswordEntryC {
            edit,
            peek: false,
            caps_lock: false,
            caps_node: Some(caps_node),
            peek_node,
            pointer: PointerState::default(),
        }
    }

    fn set_prop(&mut self, _node: &Node, name: PropName, value: &Prop, cx: &mut BuildCx<'_>) {
        if let (PropName::Text, Prop::Str(text)) = (name, value) {
            self.edit.set_text(text, cx);
        }
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        // Plan reconciliation: the plan hit-tested the peek icon with
        // `local_rect(cx.tree, cx.node, &peek)`, the same helper `Entry`'s
        // icon-press uses — but `peek`/`caps_node` are subnodes this
        // controller appends directly rather than `View` children (the same
        // shape `ProgressBarC::measure`'s own note documents for its
        // `trough`/`progress`), so they never get a taffy node or an
        // allocation of their own; `local_rect` is `None` for both,
        // always, and the icon could never be clicked. `on_event`'s own
        // `local` is already relative to this controller's root box (P4's
        // `window/pointer.rs::descend` stops there for the same reason), so
        // the peek band — and, below, the text's own region — is computed
        // straight from that through `content_rect_local`, the reserved-width
        // counterpart to `measure`'s own reservation below. The same is true
        // of `edit.text_node`, which `TextEditState::build` appends itself:
        // `local_rect` is `None` for it too, so the caret placement below
        // cannot use it either.
        let content = content_rect_local(cx.tree, cx.node);
        if let Some(peek) = self.peek_node.clone()
            && let Some(content) = content
        {
            let band = Rect::new(
                content.x + content.width - PEEK_WIDTH_PX,
                content.y,
                PEEK_WIDTH_PX,
                content.height,
            );
            let shifted = shift_event(ev, band);
            if self.pointer.observe(
                &peek,
                &shifted,
                Some(Rect::new(0.0, 0.0, band.width, band.height)),
            ) {
                self.peek = !self.peek;
                self.edit.visibility = self.peek;
                peek.set_state(PseudoStates::CHECKED, self.peek);
                cx.handled = true;
                return Vec::new();
            }
        }
        if matches!(ev, Event::PointerDown { .. }) {
            // Plan reconciliation: the plan's `on_event` never granted focus
            // on click, the same gap `Entry`, `TextView` and `SearchEntry`
            // (this task, see its own note) had — without this a
            // `PasswordEntry` could never receive keyboard focus by any
            // means.
            cx.focus.set_focus(Some(cx.node), FocusCause::Pointer);
        }
        if let Some(content) = content {
            // The text occupies the content box less whatever the peek band
            // reserves at its right (see `measure` below).
            let reserve = if self.peek_node.is_some() {
                PEEK_WIDTH_PX
            } else {
                0.0
            };
            let rect = Rect::new(
                content.x,
                content.y,
                (content.width - reserve).max(0.0),
                content.height,
            );
            let shifted = shift_event(ev, rect);
            self.pointer.observe(
                &self.edit.text_node,
                &shifted,
                Some(Rect::new(0.0, 0.0, rect.width, rect.height)),
            );
            if let Event::PointerDown { local, .. } = shifted {
                // Plan reconciliation: neither controller in the plan places
                // the caret on click; `Entry` and `SearchEntry` both do (their
                // own notes), and a `PasswordEntry` behaving differently from
                // its siblings for the same gesture is the asymmetry review
                // caught. `buffer_offset_at`, not `layout.byte_at`, because
                // the layout here is over the masked bullets, whose byte
                // offsets are not the buffer's.
                self.edit.cursor = self.edit.buffer_offset_at(local);
                self.edit.anchor = None;
                cx.handled = true;
            }
        }
        let Event::Key(key) = ev else {
            return Vec::new();
        };
        // The caps-lock indicator tracks Mods::CAPS on every key event.
        let caps = key.mods.contains(Mods::CAPS);
        if caps != self.caps_lock {
            self.caps_lock = caps;
            if let Some(node) = self.caps_node.as_ref() {
                node.set_state(PseudoStates::CHECKED, caps);
            }
        }
        match self.edit.key(key, cx) {
            EditOutcome::Ignored => Vec::new(),
            EditOutcome::Moved => {
                cx.handled = true;
                Vec::new()
            }
            EditOutcome::Changed => {
                cx.handled = true;
                let text = self.edit.buffer.clone();
                cx.handlers
                    .fire_text(EventKind::Change, &text)
                    .map_or_else(Vec::new, |m| vec![m])
            }
            EditOutcome::Activated => {
                cx.handled = true;
                cx.handlers
                    .fire_unit(EventKind::Activate)
                    .map_or_else(Vec::new, |m| vec![m])
            }
        }
    }

    fn measure(
        &mut self,
        _available: (Option<f32>, Option<f32>),
        cx: &mut BuildCx<'_>,
    ) -> Option<(f32, f32)> {
        self.edit.reshape(cx);
        let (w, h) = self.edit.layout.size();
        let reserve = if self.peek_node.is_some() {
            PEEK_WIDTH_PX
        } else {
            0.0
        };
        Some((w.max(CARET_WIDTH_PX) + reserve, h))
    }

    fn paint(
        &mut self,
        canvas: &mut skia_rs_safe::canvas::Canvas<'_>,
        alloc: &crate::layout::Allocation,
        style: &crate::css::computed::ComputedStyle,
        _cx: &mut crate::view::controller::PaintCx<'_>,
    ) -> bool {
        let content = alloc.content_box;
        // Selection behind the text, caret in front -- the same three steps,
        // in the same order, as `EntryC::paint`. A password entry hides
        // *which* characters were typed, not where the caret is: GTK draws
        // both over the bullets, and without them a focused password entry is
        // indistinguishable from an unfocused one.
        for rect in self.edit.layout.selection_rects(self.edit.selection()) {
            let placed = Rect::new(
                content.x + rect.x,
                content.y + rect.y,
                rect.width,
                rect.height,
            );
            canvas.draw_rect(&placed.to_skia(), &crate::paint::fill_paint(style.color()));
        }
        self.edit
            .layout
            .draw(canvas, (content.x, content.y), style.color());
        let caret = self.edit.layout.caret_rect(self.edit.cursor);
        let placed = Rect::new(content.x + caret.x, content.y + caret.y, 1.0, caret.height);
        canvas.draw_rect(&placed.to_skia(), &crate::paint::fill_paint(style.color()));
        true
    }
}
