//! `GtkScale` — `Kind::Scale`, CSS node `scale`.
//!
//! ```text
//! scale[.fine-tune][.marks-before][.marks-after]
//! ├── [value][.top][.right][.bottom][.left]
//! ├── marks.top
//! │   ├── mark
//! │   ┊    ├── [label]
//! │   ┊    ╰── indicator
//! ┊   ┊
//! │   ╰── mark
//! ├── marks.bottom
//! │   ├── mark
//! │   ┊    ├── indicator
//! │   ┊    ╰── [label]
//! ┊   ┊
//! │   ╰── mark
//! ╰── trough
//!     ├── [fill]
//!     ├── [highlight]
//!     ╰── slider
//! ```
//!
//! Within a `mark`, the `label` comes first when the mark is above or left of
//! the scale and second otherwise — GTK's own rule, and the reason the fixture
//! spells the two `marks` groups out separately.

use std::rc::Rc;

use crate::css::node::Node;
use crate::layout::Rect;
use crate::view::controller::{Controller, Event, EventCx};
use crate::view::{BuildCx, EventKind, Handler, Kind, Prop, PropName, Props, View};
use crate::widgets::{
    Adjustment, Mark, Orientation, PointerState, Position, WidgetEnum, shift_event,
};
use crate::window::keyboard::Mods;

/// A `GtkScale` over `lower..=upper`.
#[must_use]
pub fn scale<Msg: Clone + 'static>(lower: f64, upper: f64) -> View<Msg> {
    View::new(Kind::Scale)
        .prop(PropName::Lower, Prop::Float(lower))
        .prop(PropName::Upper, Prop::Float(upper))
}

/// `GtkScale`'s own setters.
pub trait ScaleExt<Msg>: Sized {
    /// `GtkRange:adjustment`'s value.
    fn value(self, v: f64) -> Self;
    /// `GtkOrientable:orientation`.
    fn orientation(self, o: Orientation) -> Self;
    /// `GtkScale:digits`.
    fn digits(self, n: i32) -> Self;
    /// `GtkScale:draw-value`.
    fn draw_value(self, on: bool) -> Self;
    /// `GtkScale:value-pos`.
    fn value_pos(self, pos: Position) -> Self;
    /// `gtk_scale_add_mark`. Repeated calls accumulate.
    fn mark(self, value: f64, pos: Position, label: Option<&str>) -> Self;
    /// `GtkRange:show-fill-level`.
    fn show_fill_level(self, on: bool) -> Self;
    /// `GtkRange:fill-level`.
    fn fill_level(self, v: f64) -> Self;
    /// `GtkRange:inverted`.
    fn inverted(self, on: bool) -> Self;
    /// `GtkRange::value-changed`.
    fn on_value_changed(self, f: impl Fn(f64) -> Msg + 'static) -> Self;
}

impl<Msg: Clone + 'static> ScaleExt<Msg> for View<Msg> {
    fn value(self, v: f64) -> Self {
        self.prop(PropName::Value, Prop::Float(v))
    }
    fn orientation(self, o: Orientation) -> Self {
        self.prop(PropName::Orientation, o.to_prop())
    }
    fn digits(self, n: i32) -> Self {
        self.prop(PropName::Digits, Prop::Int(i64::from(n)))
    }
    fn draw_value(self, on: bool) -> Self {
        self.prop(PropName::ShowText, Prop::Bool(on))
    }
    fn value_pos(self, pos: Position) -> Self {
        self.prop(PropName::Position, pos.to_prop())
    }
    fn mark(self, value: f64, pos: Position, label: Option<&str>) -> Self {
        // Marks ride in one `Classes` prop as `value=label` (label optional);
        // `Prop` has no list-of-structs variant and a scale has a handful.
        let slot = match pos {
            Position::Top | Position::Left => PropName::MarksTop,
            Position::Bottom | Position::Right => PropName::MarksBottom,
        };
        let mut encoded: Vec<Rc<str>> = match self.props.get(slot) {
            Some(Prop::Classes(list)) => list.to_vec(),
            _ => Vec::new(),
        };
        encoded.push(Rc::from(format!("{value}={}", label.unwrap_or(""))));
        self.prop(slot, Prop::Classes(Rc::from(encoded)))
    }
    fn show_fill_level(self, on: bool) -> Self {
        self.prop(PropName::FillLevel, Prop::Bool(on))
    }
    fn fill_level(self, v: f64) -> Self {
        self.prop(PropName::Ratio, Prop::Float(v))
    }
    fn inverted(self, on: bool) -> Self {
        self.prop(PropName::Inverted, Prop::Bool(on))
    }
    fn on_value_changed(self, f: impl Fn(f64) -> Msg + 'static) -> Self {
        self.on(EventKind::ValueChanged, Handler::Float(Rc::new(f)))
    }
}

/// Decode one `MarksTop`/`MarksBottom` prop into marks at `position`.
fn decode_marks(prop: Option<&Prop>, position: Position) -> Vec<Mark> {
    let Some(Prop::Classes(list)) = prop else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|encoded| {
            let (value, label) = encoded.split_once('=')?;
            let value = value.parse::<f64>().ok().filter(|v| v.is_finite())?;
            Some(Mark {
                value,
                position,
                label: (!label.is_empty()).then(|| Rc::from(label)),
            })
        })
        .collect()
}

/// `Kind::Scale`'s controller.
pub struct ScaleC {
    /// Current value.
    pub value: f64,
    /// The sanitized adjustment.
    pub adj: Adjustment,
    /// Marks, in the order the props declared them.
    pub marks: Vec<Mark>,
    /// The value the drag started from.
    pub drag: Option<f64>,
    /// `GtkRange`'s fine-tuning mode.
    pub fine_tune: bool,
    /// The `trough` subnode.
    pub trough: Node,
    /// The `slider` subnode.
    pub slider: Node,
    /// The `highlight` subnode (the scale always has an origin).
    pub highlight: Node,
    /// The `fill` subnode, only with `show-fill-level`.
    pub fill: Option<Node>,
    /// The `value` subnode, only with `draw-value`.
    pub value_node: Option<Node>,
    /// One node per mark, in `marks` order.
    pub mark_nodes: Vec<Node>,
    orientation: Orientation,
    inverted: bool,
    pointer: PointerState,
}

impl ScaleC {
    /// Where the slider sits inside a trough of `rect`.
    fn slider_centre(&self, rect: Rect) -> f32 {
        let fraction = if self.inverted {
            1.0 - self.adj.fraction()
        } else {
            self.adj.fraction()
        };
        match self.orientation {
            Orientation::Horizontal => rect.x + rect.width * fraction as f32,
            Orientation::Vertical => rect.y + rect.height * fraction as f32,
        }
    }

    /// The value a pointer at `local` (trough space) selects.
    fn value_for(&self, local: (f32, f32), rect: Rect) -> f64 {
        let (pos, span) = match self.orientation {
            Orientation::Horizontal => (local.0, rect.width),
            Orientation::Vertical => (local.1, rect.height),
        };
        let mut fraction = if span > 0.0 {
            f64::from(pos / span)
        } else {
            0.0
        };
        if self.inverted {
            fraction = 1.0 - fraction;
        }
        if self.fine_tune {
            let base = self.adj.fraction();
            fraction = base + (fraction - base) * 0.1;
        }
        self.adj.value_at_fraction(fraction)
    }

    fn apply(&self, node: &Node) {
        node.remove_class("fine-tune");
        if self.fine_tune {
            node.add_class("fine-tune");
        }
        node.remove_class("marks-before");
        node.remove_class("marks-after");
        if self
            .marks
            .iter()
            .any(|m| matches!(m.position, Position::Top | Position::Left))
        {
            node.add_class("marks-before");
        }
        if self
            .marks
            .iter()
            .any(|m| matches!(m.position, Position::Bottom | Position::Right))
        {
            node.add_class("marks-after");
        }
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for ScaleC {
    fn kind(&self) -> Kind {
        Kind::Scale
    }

    fn build(node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        let value_node = props.bool(PropName::ShowText, false).then(|| {
            let position = Position::from_prop(props.get(PropName::Position), Position::Top);
            let value = Node::with_classes("value", &[position.css_class()]);
            node.append_child(&value);
            value
        });

        let mut marks = decode_marks(props.get(PropName::MarksTop), Position::Top);
        marks.extend(decode_marks(
            props.get(PropName::MarksBottom),
            Position::Bottom,
        ));
        let mut mark_nodes = Vec::new();
        for group in [Position::Top, Position::Bottom] {
            let in_group: Vec<&Mark> = marks.iter().filter(|m| m.position == group).collect();
            if in_group.is_empty() {
                continue;
            }
            let marks_node = Node::with_classes("marks", &[group.css_class()]);
            node.append_child(&marks_node);
            for mark in in_group {
                let mark_node = Node::new("mark");
                marks_node.append_child(&mark_node);
                // Label first when the mark is above or left, indicator first
                // otherwise — GTK's own ordering rule.
                let label_first = matches!(group, Position::Top | Position::Left);
                let indicator = Node::new("indicator");
                if label_first {
                    if mark.label.is_some() {
                        mark_node.append_child(&Node::new("label"));
                    }
                    mark_node.append_child(&indicator);
                } else {
                    mark_node.append_child(&indicator);
                    if mark.label.is_some() {
                        mark_node.append_child(&Node::new("label"));
                    }
                }
                mark_nodes.push(mark_node);
            }
        }

        let trough = Node::new("trough");
        node.append_child(&trough);
        let fill = props.bool(PropName::FillLevel, false).then(|| {
            let fill = Node::new("fill");
            trough.append_child(&fill);
            fill
        });
        let highlight = Node::new("highlight");
        trough.append_child(&highlight);
        let slider = Node::new("slider");
        trough.append_child(&slider);

        let adj = Adjustment {
            value: props.float(PropName::Value, 0.0),
            lower: props.float(PropName::Lower, 0.0),
            upper: props.float(PropName::Upper, 1.0),
            step_increment: props.float(PropName::StepIncrement, 1.0),
            page_increment: props.float(PropName::PageIncrement, 10.0),
            page_size: 0.0,
        }
        .sanitized();

        let this = ScaleC {
            value: adj.value,
            adj,
            marks,
            drag: None,
            fine_tune: false,
            trough,
            slider,
            highlight,
            fill,
            value_node,
            mark_nodes,
            orientation: Orientation::from_prop(
                props.get(PropName::Orientation),
                Orientation::Horizontal,
            ),
            inverted: props.bool(PropName::Inverted, false),
            pointer: PointerState::default(),
        };
        this.apply(node);
        this
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        match (name, value) {
            (PropName::Value, Prop::Float(v)) => {
                self.adj.set_value(*v);
                self.value = self.adj.value;
            }
            (PropName::Lower, Prop::Float(v)) => self.adj.lower = *v,
            (PropName::Upper, Prop::Float(v)) => self.adj.upper = *v,
            (PropName::Inverted, Prop::Bool(on)) => self.inverted = *on,
            _ => return,
        }
        self.adj = self.adj.sanitized();
        self.value = self.adj.value;
        self.apply(node);
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        // `trough`/`slider`/`highlight` are subnodes this controller owns
        // directly rather than `View` children (see `ProgressBarC::measure`'s
        // note on the identical gap): they never get a taffy box of their
        // own, so there is no `local_rect(cx.tree, cx.node, &self.trough)` to
        // read. The whole leaf's own content box — the only allocation this
        // node actually has — stands in for the trough's instead; `Event`'s
        // `local` already arrives relative to this node's own border box, so
        // shifting by the padding offset is what re-expresses it in content
        // space.
        let Some(alloc) = cx.tree.allocation(cx.node) else {
            return Vec::new();
        };
        let border = alloc.border_box;
        let content = alloc.content_box;
        let offset = Rect::new(
            content.x - border.x,
            content.y - border.y,
            content.width,
            content.height,
        );
        let local_ev = shift_event(ev, offset);
        let bounds = Rect::new(0.0, 0.0, offset.width, offset.height);
        self.pointer.observe(&self.slider, &local_ev, Some(bounds));

        let mut emitted = None;
        let mut commit = |this: &mut Self, local: (f32, f32), cx: &mut EventCx<'_, Msg>| {
            let value = this.value_for(local, bounds);
            if this.adj.set_value(value) {
                this.value = this.adj.value;
                emitted = cx.handlers.fire_float(EventKind::ValueChanged, this.value);
            }
            cx.handled = true;
        };
        match &local_ev {
            Event::PointerDown { local, .. } => {
                self.drag = Some(self.value);
                commit(self, *local, cx);
            }
            Event::PointerMotion { local } if self.drag.is_some() => commit(self, *local, cx),
            Event::PointerUp { .. } => {
                self.drag = None;
                self.fine_tune = false;
                self.apply(cx.node);
            }
            Event::Key(key) if key.pressed => {
                self.fine_tune = key.mods.contains(Mods::SHIFT);
                self.apply(cx.node);
            }
            _ => {}
        }
        emitted.map_or_else(Vec::new, |msg| vec![msg])
    }

    fn measure(
        &mut self,
        _available: (Option<f32>, Option<f32>),
        _cx: &mut BuildCx<'_>,
    ) -> Option<(f32, f32)> {
        // `trough` and `slider` are subnodes this controller owns directly
        // rather than `View` children (`ProgressBarC::measure`'s own note on
        // the identical gap), so they never get a taffy box of their own and
        // this leaf's own intrinsic size has to stand in for the trough's CSS
        // minimum directly, or the whole scale collapses to
        // `min-width`/`min-height: 10px`. Adwaita's own
        // `scale > trough > slider { min-height: 18px; min-width: 18px }`
        // sizes the cross axis; the main axis mirrors `ProgressBar`'s own
        // 150px trough-minimum convention, since Adwaita sets no scale
        // width/height rule of its own to borrow.
        match self.orientation {
            Orientation::Horizontal => Some((150.0, 18.0)),
            Orientation::Vertical => Some((18.0, 150.0)),
        }
    }

    fn paint(
        &mut self,
        canvas: &mut skia_rs_safe::canvas::Canvas<'_>,
        alloc: &crate::layout::Allocation,
        style: &crate::css::computed::ComputedStyle,
        _cx: &mut crate::view::controller::PaintCx<'_>,
    ) -> bool {
        // The trough's highlight runs from its origin to the slider centre.
        let content = alloc.content_box;
        let centre = self.slider_centre(content);
        let highlight = match self.orientation {
            Orientation::Horizontal => Rect::new(
                content.x,
                content.y,
                (centre - content.x).max(0.0),
                content.height,
            ),
            Orientation::Vertical => Rect::new(
                content.x,
                content.y,
                content.width,
                (centre - content.y).max(0.0),
            ),
        };
        if highlight.is_empty() {
            return false;
        }
        canvas.draw_rect(
            &highlight.to_skia(),
            &crate::paint::fill_paint(style.color()),
        );
        true
    }
}
