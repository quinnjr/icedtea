//! `GtkStatusbar` — `Kind::Statusbar`, CSS node `statusbar`.
//!
//! ```text
//! statusbar
//! ╰── label
//! ```
//!
//! Deprecated upstream in 4.10 and kept in scope by contract ruling R1: Adwaita
//! still styles `statusbar`, and the spec names it. GTK's own widget owns a
//! message *stack* keyed by context id; the reactive model has one text prop,
//! so the stack lives in the controller and the prop replaces its top entry.
//!
//! The `label` subnode is a real CSS node (`GtkStatusbar` really does contain
//! a `GtkLabel`), but it is not a reconciler-tracked [`crate::view::Instance`]
//! — nothing but this controller ever touches it. So [`StatusbarC::paint`]
//! drives the inner [`LabelC`] itself, over the statusbar's own allocation and
//! computed style, rather than relying on the paint walker to find a
//! controller for the subnode (it never would: `ControllerPainter` looks a
//! node up by `Instance` identity, and this node isn't one).

use std::rc::Rc;

use skia_rs_safe::canvas::Canvas;

use crate::css::computed::ComputedStyle;
use crate::css::node::Node;
use crate::layout::Allocation;
use crate::text::{Ellipsize, TextStyle, WrapMode};
use crate::view::controller::{Controller, Event, EventCx, PaintCx};
use crate::view::{BuildCx, Kind, Prop, PropName, Props, View};
use crate::widgets::label::LabelC;

/// A `GtkStatusbar` with no message.
#[must_use]
pub fn statusbar<Msg: Clone + 'static>() -> View<Msg> {
    View::new(Kind::Statusbar)
}

/// `GtkStatusbar`'s message text.
pub trait StatusbarExt<Msg>: Sized {
    /// Replace the top of the message stack.
    fn text(self, text: &str) -> Self;
}

impl<Msg: Clone + 'static> StatusbarExt<Msg> for View<Msg> {
    fn text(self, text: &str) -> Self {
        self.prop(PropName::Text, Prop::Str(Rc::from(text)))
    }
}

/// `Kind::Statusbar`'s controller.
pub struct StatusbarC {
    /// `(context id, message)`, innermost last. The reactive model pushes and
    /// pops by replacing the `Text` prop, so context 0 is the only one used
    /// today; the stack is kept because `GtkStatusbar`'s pop semantics are what
    /// M5's shell will need.
    pub stack: Vec<(u32, String)>,
    /// The `label` node the text is drawn through.
    pub label: Node,
    inner: LabelC,
}

impl StatusbarC {
    fn top(&self) -> &str {
        self.stack.last().map_or("", |(_, text)| text.as_str())
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for StatusbarC {
    fn kind(&self) -> Kind {
        Kind::Statusbar
    }

    fn build(node: &Node, props: &Props, cx: &mut BuildCx<'_>) -> Self {
        // GTK's statusbar contains a label widget, whose own node is `label`.
        let label = Node::new("label");
        node.append_child(&label);
        let text = props.str(PropName::Text).unwrap_or("").to_owned();
        let mut label_props = Props::default();
        label_props.set(PropName::Label, Prop::Str(Rc::from(text.as_str())));
        let inner = <LabelC as Controller<Msg>>::build(&label, &label_props, cx);
        StatusbarC {
            stack: vec![(0, text)],
            label,
            inner,
        }
    }

    fn set_prop(&mut self, _node: &Node, name: PropName, value: &Prop, cx: &mut BuildCx<'_>) {
        if name == PropName::Text {
            let text = match value {
                Prop::Str(text) => text.to_string(),
                _ => String::new(),
            };
            match self.stack.last_mut() {
                Some(top) => top.1 = text,
                None => self.stack.push((0, text)),
            }
            let owned = self.top().to_owned();
            Controller::<Msg>::set_prop(
                &mut self.inner,
                &self.label,
                PropName::Label,
                &Prop::Str(Rc::from(owned.as_str())),
                cx,
            );
        }
    }

    fn on_event(&mut self, _ev: &Event, _cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        Vec::new()
    }

    fn measure(
        &mut self,
        available: (Option<f32>, Option<f32>),
        cx: &mut BuildCx<'_>,
    ) -> Option<(f32, f32)> {
        Controller::<Msg>::measure(&mut self.inner, available, cx)
    }

    fn paint(
        &mut self,
        canvas: &mut Canvas<'_>,
        alloc: &Allocation,
        style: &ComputedStyle,
        cx: &mut PaintCx<'_>,
    ) -> bool {
        // The paint walker only finds a controller for a reconciler-tracked
        // `Instance`'s own node (contract's `ControllerPainter`), and `label`
        // is a subnode this controller owns outright, not one — so the inner
        // `LabelC` is driven here, directly, over the statusbar's own box.
        Controller::<Msg>::paint(&mut self.inner, canvas, alloc, style, cx)
    }
}

/// Unused imports guard: the statusbar's label never wraps or ellipsizes, and
/// naming the types here keeps that decision visible rather than implicit.
const _: (WrapMode, Ellipsize) = (WrapMode::None, Ellipsize::None);
const _: fn(&crate::css::computed::ComputedStyle) -> TextStyle = TextStyle::from_computed;
