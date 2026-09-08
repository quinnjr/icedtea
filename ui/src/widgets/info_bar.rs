//! `GtkInfoBar` — `Kind::InfoBar`, CSS node `infobar`.
//!
//! ```text
//! infobar[.info][.warning][.error][.question]
//! ```
//!
//! Deprecated upstream in 4.10 and kept by contract ruling R1. The close
//! button, when shown, is a `button` node carrying `.close`; GTK's block does
//! not draw it because it is a child widget, so the fixture stops at `infobar`
//! and the matcher's `<child>`-free path check permits the extra subtree only
//! because `button` is not at a path the fixture forbids — see the
//! `fixture_matches` doc: a rendered node whose path the fixture does not name
//! is rejected, so `info_bar.txt` is extended below with the button GTK's prose
//! describes ("If the info bar shows a close button, that button will have the
//! .close style class applied").

use crate::css::node::{Node, PseudoStates};
use crate::layout::Rect;
use crate::view::controller::{Controller, Event, EventCx};
use crate::view::{BuildCx, EventKind, Handler, Kind, Prop, PropName, Props, View};
use crate::widgets::{MessageType, PointerState, WidgetEnum, content_rect_local, shift_event};

/// A `GtkInfoBar`, hidden.
#[must_use]
pub fn info_bar<Msg: Clone + 'static>() -> View<Msg> {
    View::new(Kind::InfoBar)
}

/// `GtkInfoBar`'s own setters and signals.
pub trait InfoBarExt<Msg>: Sized {
    /// `GtkInfoBar:message-type`.
    fn message_type(self, kind: MessageType) -> Self;
    /// `GtkInfoBar:revealed`.
    fn revealed(self, on: bool) -> Self;
    /// `GtkInfoBar:show-close-button`.
    fn show_close_button(self, on: bool) -> Self;
    /// `GtkInfoBar::response`, carrying the action-area button index.
    fn on_response(self, f: impl Fn(usize) -> Msg + 'static) -> Self;
    /// `GtkInfoBar::close`.
    fn on_close(self, msg: Msg) -> Self;
}

impl<Msg: Clone + 'static> InfoBarExt<Msg> for View<Msg> {
    fn message_type(self, kind: MessageType) -> Self {
        self.prop(PropName::MessageType, kind.to_prop())
    }
    fn revealed(self, on: bool) -> Self {
        self.prop(PropName::Reveal, Prop::Bool(on))
    }
    fn show_close_button(self, on: bool) -> Self {
        self.prop(PropName::Buttons, Prop::Bool(on))
    }
    fn on_response(self, f: impl Fn(usize) -> Msg + 'static) -> Self {
        self.on(EventKind::Response, Handler::Index(std::rc::Rc::new(f)))
    }
    fn on_close(self, msg: Msg) -> Self {
        self.on(EventKind::Close, Handler::Unit(msg))
    }
}

/// `Kind::InfoBar`'s controller.
pub struct InfoBarC {
    /// `GtkInfoBar:revealed`.
    pub revealed: bool,
    /// `0.0..=1.0` reveal animation progress.
    pub reveal_progress: f32,
    /// The `button.close` subnode, when `show-close-button`.
    pub close_button: Option<Node>,
    kind: MessageType,
    pointer: PointerState,
}

/// The close button's side, in px.
///
/// Adwaita's `infobar .close { min-width: 16px; min-height: 16px; padding: 4px }`
/// resolved by hand: a 16px content square inside 4px of padding on each side.
/// It is a constant rather than a layout answer because the button is a subnode
/// this controller appends itself and so never gets a taffy box of its own
/// (`widgets::content_rect_local`'s note).
pub const CLOSE_BUTTON_SIZE: f32 = 24.0;

impl InfoBarC {
    /// The close button's rectangle inside `content`, in that same space.
    ///
    /// GTK packs the close button at the trailing edge of the bar, centred on
    /// the cross axis; it shrinks rather than overflowing a bar smaller than
    /// itself. `content` is the bar's own content box in its event space, i.e.
    /// what `content_rect_local` returns.
    #[must_use]
    pub fn close_rect(content: Rect) -> Rect {
        let side = CLOSE_BUTTON_SIZE
            .min(content.width.max(0.0))
            .min(content.height.max(0.0));
        Rect::new(
            content.right() - side,
            content.y + (content.height - side) / 2.0,
            side,
            side,
        )
    }

    fn apply(&self, node: &Node) {
        for candidate in MessageType::all() {
            let class = candidate.css_class();
            if !class.is_empty() {
                node.remove_class(class);
            }
        }
        let class = self.kind.css_class();
        if !class.is_empty() {
            node.add_class(class);
        }
        // An unrevealed info bar is not in the tree's paint at all: GTK unmaps
        // it. `visibility: hidden` is the retained-tree equivalent M2 honours.
        node.set_state(PseudoStates::DISABLED, !self.revealed);
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for InfoBarC {
    fn kind(&self) -> Kind {
        Kind::InfoBar
    }

    fn build(node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        let close_button = props.bool(PropName::Buttons, false).then(|| {
            let button = Node::with_classes("button", &["close"]);
            node.append_child(&button);
            button
        });
        let this = InfoBarC {
            revealed: props.bool(PropName::Reveal, false),
            reveal_progress: if props.bool(PropName::Reveal, false) {
                1.0
            } else {
                0.0
            },
            close_button,
            kind: MessageType::from_prop(props.get(PropName::MessageType), MessageType::Info),
            pointer: PointerState::default(),
        };
        this.apply(node);
        this
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        match (name, value) {
            (PropName::Reveal, Prop::Bool(on)) => {
                self.revealed = *on;
                self.reveal_progress = if *on { 1.0 } else { 0.0 };
            }
            (PropName::MessageType, Prop::Enum(_)) => {
                self.kind = MessageType::from_prop(Some(value), self.kind);
            }
            _ => return,
        }
        self.apply(node);
    }

    fn measure(
        &mut self,
        _available: (Option<f32>, Option<f32>),
        _cx: &mut BuildCx<'_>,
    ) -> Option<(f32, f32)> {
        // The close button is a subnode this controller appends itself, so it
        // never gets a taffy box of its own and none of its Adwaita sizing
        // reaches layout: a bar with no `View` children of its own would
        // otherwise be 0x0, which is not even hit-testable. The bar's own
        // intrinsic size therefore has to stand in for the button's.
        self.close_button
            .as_ref()
            .map(|_| (CLOSE_BUTTON_SIZE, CLOSE_BUTTON_SIZE))
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        let Some(button) = self.close_button.as_ref() else {
            return Vec::new();
        };
        // `local_rect(cx.tree, cx.node, button)` is `None` on every event --
        // the button is a subnode this controller appends itself, which the
        // reconciler never gives a taffy node -- so this whole body used to be
        // dead code. `ScaleC`'s fix applies here too: derive the button's
        // rectangle from the bar's own content box plus GTK's own packing rule
        // (trailing edge, cross-axis centred).
        let Some(content) = content_rect_local(cx.tree, cx.node) else {
            return Vec::new();
        };
        let local = Self::close_rect(content);
        let shifted = shift_event(ev, local);
        if self.pointer.observe(
            button,
            &shifted,
            Some(Rect::new(0.0, 0.0, local.width, local.height)),
        ) {
            cx.handled = true;
            if let Some(msg) = cx.handlers.fire_unit(EventKind::Close) {
                return vec![msg];
            }
        }
        Vec::new()
    }
}
