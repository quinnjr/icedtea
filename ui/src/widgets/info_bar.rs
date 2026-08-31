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
use crate::widgets::{MessageType, PointerState, WidgetEnum};

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

impl InfoBarC {
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

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        let Some(button) = self.close_button.as_ref() else {
            return Vec::new();
        };
        let Some(alloc) = cx.tree.allocation(button) else {
            return Vec::new();
        };
        let root = cx.tree.allocation(cx.node).map(|a| a.border_box);
        let local = root.map_or(alloc.border_box, |root| {
            Rect::new(
                alloc.border_box.x - root.x,
                alloc.border_box.y - root.y,
                alloc.border_box.width,
                alloc.border_box.height,
            )
        });
        let shifted = shift(ev, local);
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

/// Re-express a root-local pointer event in `rect`'s own space.
fn shift(ev: &Event, rect: Rect) -> Event {
    let map = |local: (f32, f32)| (local.0 - rect.x, local.1 - rect.y);
    match ev {
        Event::PointerEnter { local } => Event::PointerEnter { local: map(*local) },
        Event::PointerMotion { local } => Event::PointerMotion { local: map(*local) },
        Event::PointerDown {
            button,
            local,
            serial,
        } => Event::PointerDown {
            button: *button,
            local: map(*local),
            serial: *serial,
        },
        Event::PointerUp {
            button,
            local,
            serial,
        } => Event::PointerUp {
            button: *button,
            local: map(*local),
            serial: *serial,
        },
        other => other.clone(),
    }
}
