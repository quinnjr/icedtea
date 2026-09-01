//! `GtkFontDialogButton` and `GtkFontDialog` — `Kind::FontDialogButton` (node
//! `fontbutton`) and `Kind::FontDialog` (node `window`, class `.dialog`).
//!
//! ```text
//! fontbutton
//! ╰── button.font
//!     ╰── [content]
//! ```
//!
//! ```text
//! window.dialog
//! ╰── fontchooser
//! ```
//!
//! Like `GtkColorDialog`, `GtkFontDialog` is a `GObject` launching a
//! deprecated chooser; icedtea builds the `fontchooser` body itself, listing
//! the families the M2 `FontDatabase` can actually match.

use std::rc::Rc;

use crate::css::node::Node;
use crate::layout::Rect;
use crate::view::controller::{Controller, Event, EventCx};
use crate::view::{BuildCx, EventKind, Handler, Kind, Prop, PropName, Props, View};
use crate::widgets::{FontLevel, PointerState, WidgetEnum, local_rect, shift_event};

/// A `GtkFontDialogButton` showing `desc` (a Pango-style description).
#[must_use]
pub fn font_dialog_button<Msg: Clone + 'static>(desc: &str) -> View<Msg> {
    View::new(Kind::FontDialogButton).prop(PropName::Text, Prop::Str(Rc::from(desc)))
}

/// A `GtkFontDialog` body opened on `desc`.
#[must_use]
pub fn font_dialog<Msg: Clone + 'static>(desc: &str) -> View<Msg> {
    View::new(Kind::FontDialog).prop(PropName::Text, Prop::Str(Rc::from(desc)))
}

/// `GtkFontDialogButton`'s own setters.
pub trait FontDialogButtonExt<Msg>: Sized {
    /// `GtkFontDialogButton:use-font`.
    fn use_font(self, on: bool) -> Self;
    /// `GtkFontDialogButton:use-size`.
    fn use_size(self, on: bool) -> Self;
    /// `GtkFontDialogButton:level`.
    fn level(self, level: FontLevel) -> Self;
    /// `GtkFontDialogButton:font-desc`'s change notification.
    fn on_change(self, f: impl Fn(&str) -> Msg + 'static) -> Self;
}

impl<Msg: Clone + 'static> FontDialogButtonExt<Msg> for View<Msg> {
    fn use_font(self, on: bool) -> Self {
        self.prop(PropName::Markup, Prop::Bool(on))
    }
    fn use_size(self, on: bool) -> Self {
        self.prop(PropName::ShowText, Prop::Bool(on))
    }
    fn level(self, level: FontLevel) -> Self {
        self.prop(PropName::SelectionMode, level.to_prop())
    }
    fn on_change(self, f: impl Fn(&str) -> Msg + 'static) -> Self {
        self.on(EventKind::Change, Handler::Text(Rc::new(f)))
    }
}

/// `GtkFontDialog`'s own setters.
pub trait FontDialogExt<Msg>: Sized {
    /// `GtkFontDialog:title`.
    fn title(self, text: &str) -> Self;
    /// `GtkFontDialog:modal`.
    fn modal(self, on: bool) -> Self;
    /// `GtkFontDialog:language`.
    fn language(self, lang: &str) -> Self;
    /// The dialog's response index.
    fn on_response(self, f: impl Fn(usize) -> Msg + 'static) -> Self;
    /// The chosen font description.
    fn on_change(self, f: impl Fn(&str) -> Msg + 'static) -> Self;
}

impl<Msg: Clone + 'static> FontDialogExt<Msg> for View<Msg> {
    fn title(self, text: &str) -> Self {
        self.prop(PropName::Title, Prop::Str(Rc::from(text)))
    }
    fn modal(self, on: bool) -> Self {
        self.prop(PropName::Modal, Prop::Bool(on))
    }
    fn language(self, lang: &str) -> Self {
        self.prop(PropName::Detail, Prop::Str(Rc::from(lang)))
    }
    fn on_response(self, f: impl Fn(usize) -> Msg + 'static) -> Self {
        self.on(EventKind::Response, Handler::Index(Rc::new(f)))
    }
    fn on_change(self, f: impl Fn(&str) -> Msg + 'static) -> Self {
        self.on(EventKind::Change, Handler::Text(Rc::new(f)))
    }
}

/// `Kind::FontDialogButton`'s controller.
pub struct FontDialogButtonC {
    /// The font description shown.
    pub desc: Rc<str>,
    /// Whether the dialog is showing.
    pub dialog_open: bool,
    /// The `button.font` subnode.
    pub button: Node,
    /// The `content` subnode carrying the description text.
    pub label: Node,
    /// A detached, never-attached sink: see
    /// `crate::widgets::color_dialog::ColorDialogButtonC::sink`.
    pub sink: Node,
    pointer: PointerState,
}

impl<Msg: Clone + 'static> Controller<Msg> for FontDialogButtonC {
    fn kind(&self) -> Kind {
        Kind::FontDialogButton
    }

    fn build(node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        let button = Node::with_classes("button", &["font"]);
        node.append_child(&button);
        let label = Node::new("content");
        button.append_child(&label);
        FontDialogButtonC {
            desc: props
                .str(PropName::Text)
                .map_or_else(|| Rc::from(""), Rc::from),
            dialog_open: false,
            button,
            label,
            sink: Node::new("sink"),
            pointer: PointerState::default(),
        }
    }

    fn set_prop(&mut self, _node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        if let (PropName::Text, Prop::Str(desc)) = (name, value) {
            self.desc = Rc::clone(desc);
        }
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        let Some(rect) = local_rect(cx.tree, cx.node, &self.button) else {
            return Vec::new();
        };
        let shifted = shift_event(ev, rect);
        if self.pointer.observe(
            &self.button,
            &shifted,
            Some(Rect::new(0.0, 0.0, rect.width, rect.height)),
        ) {
            self.dialog_open = !self.dialog_open;
            cx.handled = true;
        }
        Vec::new()
    }
}

/// `Kind::FontDialog`'s controller — the chooser body icedtea builds itself.
pub struct FontDialogC {
    /// Families the font database offered.
    pub families: Vec<Rc<str>>,
    /// The selected family, if any.
    pub selected: Option<usize>,
    /// The chosen size in px.
    pub size: f32,
    /// The preview string.
    pub preview: String,
    /// The `fontchooser` subnode; P6 replaces it with a real `ListViewC`.
    pub list: Node,
    /// A detached, never-attached sink: see
    /// `crate::widgets::color_dialog::ColorDialogButtonC::sink`.
    pub sink: Node,
}

impl<Msg: Clone + 'static> Controller<Msg> for FontDialogC {
    fn kind(&self) -> Kind {
        Kind::FontDialog
    }

    fn build(node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        node.add_class("dialog");
        let list = Node::new("fontchooser");
        node.append_child(&list);
        // The description is `"<family> <size>"`, Pango's own shorthand.
        let desc = props.str(PropName::Text).unwrap_or("");
        let (family, size) = desc.rsplit_once(' ').unwrap_or((desc, "11"));
        FontDialogC {
            families: if family.is_empty() {
                Vec::new()
            } else {
                vec![Rc::from(family)]
            },
            selected: (!family.is_empty()).then_some(0),
            size: size
                .parse::<f32>()
                .ok()
                .filter(|s| s.is_finite() && *s > 0.0)
                .unwrap_or(11.0),
            preview: "The quick brown fox".to_owned(),
            list,
            sink: Node::new("sink"),
        }
    }

    fn set_prop(&mut self, _node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        if let (PropName::Text, Prop::Str(desc)) = (name, value) {
            let (family, size) = desc.rsplit_once(' ').unwrap_or((desc.as_ref(), "11"));
            self.families = if family.is_empty() {
                Vec::new()
            } else {
                vec![Rc::from(family)]
            };
            self.selected = (!family.is_empty()).then_some(0);
            self.size = size
                .parse::<f32>()
                .ok()
                .filter(|s| s.is_finite() && *s > 0.0)
                .unwrap_or(11.0);
        }
    }

    fn on_event(&mut self, _ev: &Event, _cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        Vec::new()
    }
}
