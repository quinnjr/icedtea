//! `GtkLinkButton` — `Kind::LinkButton`, CSS node `button`, class `.link`.
//!
//! ```text
//! button.link
//! ```
//!
//! Actions: `clipboard.copy` copies the uri, `menu.popup` opens the context
//! menu. Shortcut: `Shift+F10` or `Menu` opens that menu.

use std::rc::Rc;

use crate::css::node::{Node, PseudoStates};
use crate::view::cmd::Cmd;
use crate::view::controller::{Controller, Event, EventCx};
use crate::view::{BuildCx, EventKind, Handler, Kind, Prop, PropName, Props, View};
use crate::widgets::{PointerState, Universal};
use crate::window::keyboard::Mods;
use crate::window::popup::PopupKey;

/// A `GtkLinkButton` for `uri`, showing `label`.
#[must_use]
pub fn link_button<Msg: Clone + 'static>(uri: &str, label: &str) -> View<Msg> {
    View::new(Kind::LinkButton)
        .prop(PropName::Uri, Prop::Str(Rc::from(uri)))
        .prop(PropName::Label, Prop::Str(Rc::from(label)))
}

/// `GtkLinkButton`'s own setters and signals.
pub trait LinkButtonExt<Msg>: Sized {
    /// `GtkLinkButton:visited`.
    fn visited(self, on: bool) -> Self;
    /// `GtkLinkButton::activate-link`.
    fn on_activate_link(self, f: impl Fn(&str) -> Msg + 'static) -> Self;
}

impl<Msg: Clone + 'static> LinkButtonExt<Msg> for View<Msg> {
    fn visited(self, on: bool) -> Self {
        self.prop(PropName::Checked, Prop::Bool(on))
    }
    fn on_activate_link(self, f: impl Fn(&str) -> Msg + 'static) -> Self {
        self.on(EventKind::ActivateLink, Handler::Text(Rc::new(f)))
    }
}

/// `Kind::LinkButton`'s controller.
pub struct LinkButtonC {
    /// The link target.
    pub uri: Rc<str>,
    /// `GtkLinkButton:visited`.
    pub visited: bool,
    /// The `label` subnode.
    pub label: Node,
    /// The context menu's popup, while open.
    pub menu: Option<PopupKey>,
    pointer: PointerState,
    universal: Universal,
}

impl LinkButtonC {
    fn apply(&self, node: &Node) {
        node.add_class("link");
        // GTK's :visited never matches on a node (M2's `PseudoStates::VISITED`
        // exists but GTK never sets it), so `visited` is a style class here.
        if self.visited {
            node.add_class("visited");
        } else {
            node.remove_class("visited");
        }
        let _ = PseudoStates::VISITED;
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for LinkButtonC {
    fn kind(&self) -> Kind {
        Kind::LinkButton
    }

    fn build(node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        let label = Node::new("label");
        node.append_child(&label);
        let this = LinkButtonC {
            uri: props
                .str(PropName::Uri)
                .map_or_else(|| Rc::from(""), Rc::from),
            visited: props.bool(PropName::Checked, false),
            label,
            menu: None,
            pointer: PointerState::default(),
            universal: Universal::new(node, Kind::LinkButton),
        };
        this.apply(node);
        this
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        match (name, value) {
            (PropName::Uri, Prop::Str(uri)) => self.uri = Rc::clone(uri),
            // `Checked` doubles as `visited` here (`LinkButtonExt::visited`),
            // ahead of `Universal`'s own generic handling of that name so
            // this controller's meaning wins.
            (PropName::Checked, Prop::Bool(on)) => self.visited = *on,
            _ => {
                self.universal.apply(node, Kind::LinkButton, name, value);
                return;
            }
        }
        self.apply(node);
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        // Shift+F10 and Menu open the context menu; its only entry is
        // `clipboard.copy`, which the controller performs directly.
        if let Event::Key(key) = ev {
            let is_menu = u32::from(key.keysym) == xkbcommon::xkb::keysyms::KEY_Menu;
            let is_shift_f10 = u32::from(key.keysym) == xkbcommon::xkb::keysyms::KEY_F10
                && key.effective_mods().contains(Mods::SHIFT);
            if key.pressed && (is_menu || is_shift_f10) {
                cx.handled = true;
                cx.cmds.push(Cmd::Copy(self.uri.to_string()));
                return Vec::new();
            }
        }
        let bounds = cx
            .tree
            .allocation(cx.node)
            .map(|a| crate::layout::Rect::new(0.0, 0.0, a.border_box.width, a.border_box.height));
        let clicked = self.pointer.observe(cx.node, ev, bounds);
        if clicked || matches!(ev, Event::Activate) {
            self.visited = true;
            self.apply(cx.node);
            cx.handled = true;
            let uri = self.uri.to_string();
            return cx
                .handlers
                .fire_text(EventKind::ActivateLink, &uri)
                .map_or_else(Vec::new, |m| vec![m]);
        }
        Vec::new()
    }
}
