//! `GtkScrollbar` — `Kind::Scrollbar`, CSS node `scrollbar`.
//!
//! ```text
//! scrollbar
//! ╰── range[.fine-tune]
//!     ╰── trough
//!         ╰── slider
//! ```
//!
//! The main node takes `.horizontal`/`.vertical`; `range` takes `.fine-tune`
//! while Shift is held or a long press started the drag (`GtkRange`'s rule).
//! `.overlay-indicator`/`.dragging`/`.hovering` are added by P6's
//! `ScrolledWindow`, not here.

use std::rc::Rc;

use crate::css::node::Node;
use crate::layout::Rect;
use crate::view::controller::{Controller, Event, EventCx};
use crate::view::{BuildCx, EventKind, Handler, Kind, Prop, PropName, Props, View};
use crate::widgets::{Adjustment, Orientation, PointerState, WidgetEnum, local_rect, shift_event};
use crate::window::keyboard::Mods;

/// A `GtkScrollbar` in `orientation`.
#[must_use]
pub fn scrollbar<Msg: Clone + 'static>(orientation: Orientation) -> View<Msg> {
    View::new(Kind::Scrollbar).prop(PropName::Orientation, orientation.to_prop())
}

/// `GtkScrollbar`'s adjustment, spelled as individual props.
pub trait ScrollbarExt<Msg>: Sized {
    /// `GtkAdjustment:value`.
    fn value(self, v: f64) -> Self;
    /// `GtkAdjustment:lower`.
    fn lower(self, v: f64) -> Self;
    /// `GtkAdjustment:upper`.
    fn upper(self, v: f64) -> Self;
    /// `GtkAdjustment:page-size`.
    fn page_size(self, v: f64) -> Self;
    /// `GtkAdjustment:step-increment`.
    fn step_increment(self, v: f64) -> Self;
    /// `GtkAdjustment:page-increment`.
    fn page_increment(self, v: f64) -> Self;
    /// `GtkRange:inverted`.
    fn inverted(self, on: bool) -> Self;
    /// `GtkRange::value-changed`.
    fn on_value_changed(self, f: impl Fn(f64) -> Msg + 'static) -> Self;
}

impl<Msg: Clone + 'static> ScrollbarExt<Msg> for View<Msg> {
    fn value(self, v: f64) -> Self {
        self.prop(PropName::Value, Prop::Float(v))
    }
    fn lower(self, v: f64) -> Self {
        self.prop(PropName::Lower, Prop::Float(v))
    }
    fn upper(self, v: f64) -> Self {
        self.prop(PropName::Upper, Prop::Float(v))
    }
    fn page_size(self, v: f64) -> Self {
        self.prop(PropName::Ratio, Prop::Float(v))
    }
    fn step_increment(self, v: f64) -> Self {
        self.prop(PropName::StepIncrement, Prop::Float(v))
    }
    fn page_increment(self, v: f64) -> Self {
        self.prop(PropName::PageIncrement, Prop::Float(v))
    }
    fn inverted(self, on: bool) -> Self {
        self.prop(PropName::Inverted, Prop::Bool(on))
    }
    fn on_value_changed(self, f: impl Fn(f64) -> Msg + 'static) -> Self {
        self.on(EventKind::ValueChanged, Handler::Float(Rc::new(f)))
    }
}

/// `Kind::Scrollbar`'s controller. P6's `ScrolledWindow` embeds two of these.
pub struct ScrollbarC {
    /// Mirror of `adj.value`, kept because the model owns the authoritative one.
    pub value: f64,
    /// The sanitized adjustment.
    pub adj: Adjustment,
    /// Grab offset within the slider while dragging, in px.
    pub drag: Option<f32>,
    /// `GtkRange`'s fine-tuning mode.
    pub fine_tune: bool,
    /// The `range` subnode.
    pub range: Node,
    /// The `trough` subnode.
    pub trough: Node,
    /// The `slider` subnode.
    pub slider: Node,
    orientation: Orientation,
    inverted: bool,
    pointer: PointerState,
}

impl ScrollbarC {
    /// Replace the adjustment wholesale — P6's `ScrolledWindow` entry point.
    pub fn set_adjustment(&mut self, adj: Adjustment) {
        self.adj = adj.sanitized();
        self.value = self.adj.value;
    }

    /// Where the slider sits inside `trough`.
    ///
    /// The slider's length is the page's share of the range, floored at 20px so
    /// a huge document still leaves something to grab.
    #[must_use]
    pub fn slider_rect(&self, trough: Rect, orientation: Orientation) -> Rect {
        let span = self.adj.upper - self.adj.lower;
        let visible = if span > 0.0 {
            (self.adj.page_size / span).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let fraction = if self.inverted {
            1.0 - self.adj.fraction()
        } else {
            self.adj.fraction()
        } as f32;
        match orientation {
            Orientation::Horizontal => {
                let len = (trough.width * visible as f32).max(20.0).min(trough.width);
                Rect::new(
                    trough.x + (trough.width - len) * fraction,
                    trough.y,
                    len,
                    trough.height,
                )
            }
            Orientation::Vertical => {
                let len = (trough.height * visible as f32)
                    .max(20.0)
                    .min(trough.height);
                Rect::new(
                    trough.x,
                    trough.y + (trough.height - len) * fraction,
                    trough.width,
                    len,
                )
            }
        }
    }

    /// The value a pointer at `local` (trough space) selects.
    fn value_for(&self, local: (f32, f32), trough: Rect, grab: f32) -> f64 {
        let (pos, span) = match self.orientation {
            Orientation::Horizontal => (local.0 - grab, trough.width),
            Orientation::Vertical => (local.1 - grab, trough.height),
        };
        let slider = self.slider_rect(trough, self.orientation);
        let travel = match self.orientation {
            Orientation::Horizontal => span - slider.width,
            Orientation::Vertical => span - slider.height,
        };
        let mut fraction = if travel > 0.0 {
            f64::from(pos / travel)
        } else {
            0.0
        };
        if self.inverted {
            fraction = 1.0 - fraction;
        }
        if self.fine_tune {
            // Fine-tuning moves the value a tenth as far per pixel.
            let base = self.adj.fraction();
            fraction = base + (fraction - base) * 0.1;
        }
        self.adj.value_at_fraction(fraction)
    }

    fn apply(&self, node: &Node) {
        for candidate in Orientation::all() {
            node.remove_class(candidate.css_class());
        }
        node.add_class(self.orientation.css_class());
        if self.fine_tune {
            self.range.add_class("fine-tune");
        } else {
            self.range.remove_class("fine-tune");
        }
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for ScrollbarC {
    fn kind(&self) -> Kind {
        Kind::Scrollbar
    }

    fn build(node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        let range = Node::new("range");
        node.append_child(&range);
        let trough = Node::new("trough");
        range.append_child(&trough);
        let slider = Node::new("slider");
        trough.append_child(&slider);
        let adj = Adjustment {
            value: props.float(PropName::Value, 0.0),
            lower: props.float(PropName::Lower, 0.0),
            upper: props.float(PropName::Upper, 1.0),
            step_increment: props.float(PropName::StepIncrement, 0.1),
            page_increment: props.float(PropName::PageIncrement, 0.2),
            page_size: props.float(PropName::Ratio, 0.0),
        }
        .sanitized();
        let this = ScrollbarC {
            value: adj.value,
            adj,
            drag: None,
            fine_tune: false,
            range,
            trough,
            slider,
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
            (PropName::Ratio, Prop::Float(v)) => self.adj.page_size = *v,
            (PropName::StepIncrement, Prop::Float(v)) => self.adj.step_increment = *v,
            (PropName::PageIncrement, Prop::Float(v)) => self.adj.page_increment = *v,
            (PropName::Inverted, Prop::Bool(on)) => self.inverted = *on,
            (PropName::Orientation, Prop::Enum(_)) => {
                self.orientation = Orientation::from_prop(Some(value), self.orientation);
            }
            _ => return,
        }
        self.adj = self.adj.sanitized();
        self.value = self.adj.value;
        self.apply(node);
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        let Some(trough) = local_rect(cx.tree, cx.node, &self.trough) else {
            return Vec::new();
        };
        let local_ev = shift_event(ev, trough);
        self.pointer.observe(
            &self.slider,
            &local_ev,
            Some(Rect::new(0.0, 0.0, trough.width, trough.height)),
        );

        let mut emitted = None;
        match &local_ev {
            Event::PointerDown { local, .. } => {
                let slider = self.slider_rect(
                    Rect::new(0.0, 0.0, trough.width, trough.height),
                    self.orientation,
                );
                let grab = match self.orientation {
                    Orientation::Horizontal if local.0 >= slider.x && local.0 <= slider.right() => {
                        local.0 - slider.x
                    }
                    Orientation::Vertical if local.1 >= slider.y && local.1 <= slider.bottom() => {
                        local.1 - slider.y
                    }
                    // A click off the slider jumps it under the pointer, centred.
                    Orientation::Horizontal => slider.width / 2.0,
                    Orientation::Vertical => slider.height / 2.0,
                };
                self.drag = Some(grab);
                let value = self.value_for(*local, trough, grab);
                if self.adj.set_value(value) {
                    self.value = self.adj.value;
                    emitted = cx.handlers.fire_float(EventKind::ValueChanged, self.value);
                }
                cx.handled = true;
            }
            Event::PointerMotion { local } => {
                if let Some(grab) = self.drag {
                    let value = self.value_for(*local, trough, grab);
                    if self.adj.set_value(value) {
                        self.value = self.adj.value;
                        emitted = cx.handlers.fire_float(EventKind::ValueChanged, self.value);
                    }
                    cx.handled = true;
                }
            }
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
}
