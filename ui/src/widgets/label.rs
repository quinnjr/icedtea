//! `GtkLabel` — `Kind::Label`, CSS node `label`.
//!
//! ```text
//! label
//! ├── [selection]
//! ├── [link]
//! ┊
//! ╰── [link]
//! ```
//!
//! The `selection` subnode exists only while the label is selectable, and one
//! `link` subnode exists per `<a href>` the markup carried; the `label` node
//! then also gets the `.link` style class, as GTK does.

use std::ops::Range;
use std::rc::Rc;

use skia_rs_safe::canvas::Canvas;

use crate::css::computed::ComputedStyle;
use crate::css::node::{Node, PseudoStates};
use crate::layout::{Allocation, Rect};
use crate::text::{Ellipsize, MarkupSpan, TextLayout, TextStyle, WrapMode, parse_markup};
use crate::view::controller::{Controller, Event, EventCx, PaintCx};
use crate::view::{BuildCx, EventKind, Handler, Kind, Prop, PropName, Props, View};
use crate::widgets::{PointerState, Universal};

/// A `GtkLabel` showing `text`.
#[must_use]
pub fn label<Msg: Clone + 'static>(text: &str) -> View<Msg> {
    View::new(Kind::Label).prop(PropName::Label, Prop::Str(Rc::from(text)))
}

/// `GtkLabel`'s own property setters, chained after [`label`].
pub trait LabelExt<Msg>: Sized {
    /// `GtkLabel:wrap`. Turns on `WrapMode::Word` unless `wrap_mode` says else.
    fn wrap(self, on: bool) -> Self;
    /// `GtkLabel:wrap-mode`.
    fn wrap_mode(self, mode: WrapMode) -> Self;
    /// `GtkLabel:ellipsize`.
    fn ellipsize(self, mode: Ellipsize) -> Self;
    /// `GtkLabel:lines` — the maximum number of wrapped lines; -1 for no limit.
    fn lines(self, n: i32) -> Self;
    /// `GtkLabel:xalign`, 0.0..=1.0.
    fn xalign(self, a: f32) -> Self;
    /// `GtkLabel:yalign`, 0.0..=1.0.
    fn yalign(self, a: f32) -> Self;
    /// `GtkLabel:use-markup`.
    fn markup(self, on: bool) -> Self;
    /// `GtkLabel:selectable`.
    fn selectable(self, on: bool) -> Self;
    /// `GtkLabel:use-underline` (mnemonics; the underscore is stripped).
    fn use_underline(self, on: bool) -> Self;
    /// `GtkLabel:width-chars`.
    fn width_chars(self, n: i32) -> Self;
    /// `GtkLabel:max-width-chars`.
    fn max_width_chars(self, n: i32) -> Self;
    /// `GtkLabel::activate-link`.
    fn on_activate_link(self, f: impl Fn(&str) -> Msg + 'static) -> Self;
}

impl<Msg: Clone + 'static> LabelExt<Msg> for View<Msg> {
    fn wrap(self, on: bool) -> Self {
        self.prop(PropName::Wrap, Prop::Bool(on))
    }
    fn wrap_mode(self, mode: WrapMode) -> Self {
        self.prop(PropName::Wrap, Prop::Enum(wrap_to_u16(mode)))
    }
    fn ellipsize(self, mode: Ellipsize) -> Self {
        self.prop(PropName::Ellipsize, Prop::Enum(ellipsize_to_u16(mode)))
    }
    fn lines(self, n: i32) -> Self {
        self.prop(PropName::Rows, Prop::Int(i64::from(n)))
    }
    fn xalign(self, a: f32) -> Self {
        self.prop(PropName::Xalign, Prop::Float(f64::from(a)))
    }
    fn yalign(self, a: f32) -> Self {
        self.prop(PropName::Yalign, Prop::Float(f64::from(a)))
    }
    fn markup(self, on: bool) -> Self {
        self.prop(PropName::Markup, Prop::Bool(on))
    }
    fn selectable(self, on: bool) -> Self {
        self.prop(PropName::Selectable, Prop::Bool(on))
    }
    fn use_underline(self, on: bool) -> Self {
        self.prop(PropName::Editable, Prop::Bool(on))
    }
    fn width_chars(self, n: i32) -> Self {
        self.prop(PropName::WidthRequest, Prop::Int(i64::from(n)))
    }
    fn max_width_chars(self, n: i32) -> Self {
        self.prop(PropName::MaxLength, Prop::Int(i64::from(n)))
    }
    fn on_activate_link(self, f: impl Fn(&str) -> Msg + 'static) -> Self {
        self.on(EventKind::ActivateLink, Handler::Text(Rc::new(f)))
    }
}

/// `WrapMode` has no `WidgetEnum` impl (it lives in `text`), so the two
/// conversions are spelled out here and in `wrap_from_u16`.
fn wrap_to_u16(mode: WrapMode) -> u16 {
    match mode {
        WrapMode::None => 0,
        WrapMode::Word => 1,
        WrapMode::Char => 2,
        WrapMode::WordChar => 3,
    }
}

fn wrap_from_u16(raw: u16) -> WrapMode {
    match raw {
        1 => WrapMode::Word,
        2 => WrapMode::Char,
        3 => WrapMode::WordChar,
        _ => WrapMode::None,
    }
}

fn ellipsize_to_u16(mode: Ellipsize) -> u16 {
    match mode {
        Ellipsize::None => 0,
        Ellipsize::Start => 1,
        Ellipsize::Middle => 2,
        Ellipsize::End => 3,
    }
}

fn ellipsize_from_u16(raw: u16) -> Ellipsize {
    match raw {
        1 => Ellipsize::Start,
        2 => Ellipsize::Middle,
        3 => Ellipsize::End,
        _ => Ellipsize::None,
    }
}

/// `Kind::Label`'s controller.
pub struct LabelC {
    /// The wrapped, ellipsized paragraph.
    pub layout: TextLayout,
    /// Attributed runs from `parse_markup`, empty when markup is off.
    pub spans: Vec<MarkupSpan>,
    /// The selected byte range, when the label is selectable and has one.
    pub selection: Option<Range<usize>>,
    /// `(byte range, uri)` for every `<a href>` the markup carried.
    pub links: Vec<(Range<usize>, Rc<str>)>,
    /// Index into `links` under the pointer.
    pub hovered_link: Option<usize>,
    /// The `selection` subnode, present only while selectable.
    pub selection_node: Option<Node>,
    /// One `link` subnode per entry in `links`.
    pub link_nodes: Vec<Node>,
    text: Rc<str>,
    plain: String,
    wrap: WrapMode,
    ellipsize: Ellipsize,
    xalign: f32,
    markup: bool,
    selectable: bool,
    pointer: PointerState,
    /// The `GtkWidget`-universal props (`class`, `id`, `sensitive`, …).
    universal: Universal,
    /// The width the layout was last built at, so `measure` can reuse it.
    built_width: Option<f32>,
}

impl LabelC {
    /// Re-read the text props and rebuild the paragraph and the subnodes.
    fn rebuild(&mut self, node: &Node, cx: &mut BuildCx<'_>, width: Option<f32>) {
        let (plain, spans, links) = if self.markup {
            let (plain, spans) = parse_markup(&self.text);
            let links = extract_links(&self.text, &plain);
            (plain, spans, links)
        } else {
            (self.text.to_string(), Vec::new(), Vec::new())
        };
        self.plain = plain;
        self.spans = spans;
        self.links = links;

        let style = TextStyle::from_computed(&ComputedStyle::initial(cx.env));
        self.layout = TextLayout::build(
            &self.plain,
            &style,
            cx.fonts,
            width,
            self.wrap,
            self.ellipsize,
        );
        self.built_width = width;

        // Subnodes: one `selection` when selectable, one `link` per link, and
        // the `.link` class on the label itself when any link exists.
        if self.selectable && self.selection_node.is_none() {
            let selection = Node::new("selection");
            node.append_child(&selection);
            self.selection_node = Some(selection);
        } else if !self.selectable
            && let Some(selection) = self.selection_node.take()
        {
            selection.detach();
        }
        while self.link_nodes.len() > self.links.len() {
            if let Some(extra) = self.link_nodes.pop() {
                extra.detach();
            }
        }
        while self.link_nodes.len() < self.links.len() {
            let link = Node::new("link");
            node.append_child(&link);
            self.link_nodes.push(link);
        }
        node.set_state(PseudoStates::empty(), false);
        if self.links.is_empty() {
            node.remove_class("link");
        } else {
            node.add_class("link");
        }
    }
}

/// `(range in the plain text, uri)` for every `<a href="…">` in `source`.
///
/// `parse_markup` drops `<a>` with every other unknown tag; links are read
/// here instead so the plain text the two produce stays identical.
fn extract_links(source: &str, plain: &str) -> Vec<(Range<usize>, Rc<str>)> {
    let mut out = Vec::new();
    let mut plain_cursor = 0usize;
    let mut rest = source;
    while let Some(open) = rest.find("<a ") {
        // Everything before the tag lands in the plain text verbatim, modulo
        // the tags `parse_markup` already removed, so the cursor advances by
        // the plain-text length of that prefix.
        plain_cursor += parse_markup(&rest[..open]).0.len();
        let Some(gt) = rest[open..].find('>').map(|o| open + o) else {
            break;
        };
        let uri = rest[open..gt]
            .split_once("href=")
            .map(|(_, tail)| tail.trim_start().trim_start_matches(['"', '\'']))
            .and_then(|tail| tail.split(['"', '\'']).next())
            .unwrap_or("");
        let body_start = gt + 1;
        let end = rest[body_start..]
            .find("</a>")
            .map_or(rest.len(), |o| body_start + o);
        let body_len = parse_markup(&rest[body_start..end]).0.len();
        if plain_cursor + body_len <= plain.len() {
            out.push((plain_cursor..plain_cursor + body_len, Rc::from(uri)));
        }
        plain_cursor += body_len;
        rest = &rest[(end + 4).min(rest.len())..];
    }
    out
}

impl<Msg: Clone + 'static> Controller<Msg> for LabelC {
    fn kind(&self) -> Kind {
        Kind::Label
    }

    fn build(node: &Node, props: &Props, cx: &mut BuildCx<'_>) -> Self {
        let mut this = LabelC {
            layout: TextLayout::build(
                "",
                &TextStyle::from_computed(&ComputedStyle::initial(cx.env)),
                cx.fonts,
                None,
                WrapMode::None,
                Ellipsize::None,
            ),
            spans: Vec::new(),
            selection: None,
            links: Vec::new(),
            hovered_link: None,
            selection_node: None,
            link_nodes: Vec::new(),
            text: props
                .str(PropName::Label)
                .map_or_else(|| Rc::from(""), Rc::from),
            plain: String::new(),
            wrap: match props.get(PropName::Wrap) {
                Some(Prop::Enum(raw)) => wrap_from_u16(*raw),
                Some(Prop::Bool(true)) => WrapMode::Word,
                _ => WrapMode::None,
            },
            ellipsize: match props.get(PropName::Ellipsize) {
                Some(Prop::Enum(raw)) => ellipsize_from_u16(*raw),
                _ => Ellipsize::None,
            },
            xalign: props.float(PropName::Xalign, 0.5) as f32,
            markup: props.bool(PropName::Markup, false),
            selectable: props.bool(PropName::Selectable, false),
            pointer: PointerState::default(),
            universal: Universal::new(node, Kind::Label),
            built_width: None,
        };
        this.rebuild(node, cx, None);
        this
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, cx: &mut BuildCx<'_>) {
        if self.universal.apply(node, Kind::Label, name, value) {
            return;
        }
        match name {
            PropName::Label => {
                self.text = match value {
                    Prop::Str(text) => Rc::clone(text),
                    _ => Rc::from(""),
                };
            }
            PropName::Wrap => {
                self.wrap = match value {
                    Prop::Enum(raw) => wrap_from_u16(*raw),
                    Prop::Bool(true) => WrapMode::Word,
                    _ => WrapMode::None,
                };
            }
            PropName::Ellipsize => {
                self.ellipsize = match value {
                    Prop::Enum(raw) => ellipsize_from_u16(*raw),
                    _ => Ellipsize::None,
                };
            }
            PropName::Markup => self.markup = matches!(value, Prop::Bool(true)),
            PropName::Selectable => self.selectable = matches!(value, Prop::Bool(true)),
            PropName::Xalign => {
                if let Prop::Float(a) = value {
                    self.xalign = if a.is_finite() { *a as f32 } else { 0.5 };
                }
            }
            _ => return,
        }
        let width = self.built_width;
        self.rebuild(node, cx, width);
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        let alloc = cx.tree.allocation(cx.node).map(|a| a.content_box);
        let clicked = self.pointer.observe(
            cx.node,
            ev,
            alloc.map(|r| Rect::new(0.0, 0.0, r.width, r.height)),
        );
        if let Event::PointerMotion { local } = ev {
            let byte = self.layout.byte_at(*local);
            self.hovered_link = self
                .links
                .iter()
                .position(|(range, _)| range.contains(&byte));
            for (index, link) in self.link_nodes.iter().enumerate() {
                link.set_state(PseudoStates::HOVER, self.hovered_link == Some(index));
            }
        }
        if clicked && let Event::PointerUp { local, .. } = ev {
            let byte = self.layout.byte_at(*local);
            if let Some((_, uri)) = self.links.iter().find(|(range, _)| range.contains(&byte)) {
                cx.handled = true;
                if let Some(msg) = cx.handlers.fire_text(EventKind::ActivateLink, uri) {
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
        let width = available.0.filter(|w| w.is_finite() && *w > 0.0);
        if width != self.built_width {
            let style = TextStyle::from_computed(&ComputedStyle::initial(cx.env));
            self.layout = TextLayout::build(
                &self.plain,
                &style,
                cx.fonts,
                width,
                self.wrap,
                self.ellipsize,
            );
            self.built_width = width;
        }
        Some(self.layout.size())
    }

    fn paint(
        &mut self,
        canvas: &mut Canvas<'_>,
        alloc: &Allocation,
        style: &ComputedStyle,
        _cx: &mut PaintCx<'_>,
    ) -> bool {
        let content = alloc.content_box;
        let (text_w, _) = self.layout.size();
        let slack = (content.width - text_w).max(0.0);
        let x = content.x + slack * self.xalign.clamp(0.0, 1.0);
        self.layout.draw(canvas, (x, content.y), style.color());
        true
    }
}
