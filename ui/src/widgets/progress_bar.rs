//! `GtkProgressBar` — `Kind::ProgressBar`, CSS node `progressbar`.
//!
//! ```text
//! progressbar[.osd]
//! ├── [text]
//! ╰── trough[.empty][.full]
//!     ╰── progress[.pulse]
//! ```
//!
//! A non-finite `fraction` means activity mode: the `progress` node gets
//! `.pulse` and `tick` walks it back and forth by `pulse_step` per frame.

use std::rc::Rc;
use std::time::Duration;

use crate::css::node::Node;
use crate::text::Ellipsize;
use crate::view::controller::{Controller, Event, EventCx};
use crate::view::{BuildCx, Kind, Prop, PropName, Props, View};
use crate::widgets::label::LabelC;

/// A `GtkProgressBar` at `fraction`. A non-finite fraction starts activity mode.
#[must_use]
pub fn progress_bar<Msg: Clone + 'static>(fraction: f64) -> View<Msg> {
    View::new(Kind::ProgressBar).prop(PropName::Fraction, Prop::Float(fraction))
}

/// `GtkProgressBar`'s own setters.
pub trait ProgressBarExt<Msg>: Sized {
    /// `GtkProgressBar:text`.
    fn text(self, text: &str) -> Self;
    /// `GtkProgressBar:show-text`.
    fn show_text(self, on: bool) -> Self;
    /// `GtkProgressBar:inverted`.
    fn inverted(self, on: bool) -> Self;
    /// `GtkProgressBar:pulse-step`; also what turns activity mode on.
    fn pulse_step(self, step: f64) -> Self;
    /// `GtkProgressBar:ellipsize`, for the `text` subnode.
    fn ellipsize(self, mode: Ellipsize) -> Self;
}

impl<Msg: Clone + 'static> ProgressBarExt<Msg> for View<Msg> {
    fn text(self, text: &str) -> Self {
        self.prop(PropName::Text, Prop::Str(Rc::from(text)))
    }
    fn show_text(self, on: bool) -> Self {
        self.prop(PropName::ShowText, Prop::Bool(on))
    }
    fn inverted(self, on: bool) -> Self {
        self.prop(PropName::Inverted, Prop::Bool(on))
    }
    fn pulse_step(self, step: f64) -> Self {
        self.prop(PropName::Pulse, Prop::Float(step))
    }
    fn ellipsize(self, mode: Ellipsize) -> Self {
        self.prop(
            PropName::Ellipsize,
            Prop::Enum(match mode {
                Ellipsize::None => 0,
                Ellipsize::Start => 1,
                Ellipsize::Middle => 2,
                Ellipsize::End => 3,
            }),
        )
    }
}

/// `Kind::ProgressBar`'s controller.
pub struct ProgressBarC {
    /// `0.0..=1.0`, or NaN in activity mode.
    pub fraction: f64,
    /// Activity mode.
    pub pulsing: bool,
    /// Where the pulse block sits, `0.0..=1.0`.
    pub pulse_pos: f64,
    /// The `trough` subnode.
    pub trough: Node,
    /// The `progress` subnode inside the trough.
    pub progress: Node,
    /// The `text` subnode, only when `show-text`.
    pub text: Option<Node>,
    pulse_step: f64,
    forward: bool,
    label: Option<LabelC>,
}

impl ProgressBarC {
    /// The used fraction: clamped, and 0 while pulsing.
    fn used(&self) -> f64 {
        if self.pulsing || !self.fraction.is_finite() {
            0.0
        } else {
            self.fraction.clamp(0.0, 1.0)
        }
    }

    fn apply(&mut self, node: &Node) {
        let used = self.used();
        self.trough
            .set_state(crate::css::node::PseudoStates::empty(), false);
        self.trough.remove_class("empty");
        self.trough.remove_class("full");
        if !self.pulsing && used <= 0.0 {
            self.trough.add_class("empty");
        } else if used >= 1.0 {
            self.trough.add_class("full");
        }
        if self.pulsing {
            self.progress.add_class("pulse");
        } else {
            self.progress.remove_class("pulse");
        }
        let _ = node;
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for ProgressBarC {
    fn kind(&self) -> Kind {
        Kind::ProgressBar
    }

    fn build(node: &Node, props: &Props, cx: &mut BuildCx<'_>) -> Self {
        let fraction = props.float(PropName::Fraction, 0.0);
        let text = props.bool(PropName::ShowText, false).then(|| {
            let text = Node::new("text");
            node.append_child(&text);
            text
        });
        // `GtkProgressBar` implements `GtkOrientable`; this task exposes no
        // setter for it, so the node always carries the horizontal class
        // Adwaita's `progressbar.horizontal > trough` sizing rules key off —
        // without it neither the trough nor the progress node gets a
        // min-height and the whole bar renders at zero size.
        node.add_class("horizontal");
        let trough = Node::new("trough");
        node.append_child(&trough);
        let progress = Node::new("progress");
        trough.append_child(&progress);
        let label = text.as_ref().map(|node| {
            let mut label_props = Props::default();
            label_props.set(
                PropName::Label,
                Prop::Str(Rc::from(props.str(PropName::Text).unwrap_or(""))),
            );
            <LabelC as Controller<Msg>>::build(node, &label_props, cx)
        });
        let mut this = ProgressBarC {
            pulsing: !fraction.is_finite(),
            fraction,
            pulse_pos: 0.0,
            pulse_step: props.float(PropName::Pulse, 0.1).abs().clamp(0.01, 1.0),
            forward: true,
            trough,
            progress,
            text,
            label,
        };
        this.apply(node);
        this
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        match (name, value) {
            (PropName::Fraction, Prop::Float(v)) => {
                self.fraction = *v;
                self.pulsing = !v.is_finite();
            }
            (PropName::Pulse, Prop::Float(v)) => {
                self.pulse_step = v.abs().clamp(0.01, 1.0);
            }
            _ => return,
        }
        self.apply(node);
    }

    fn tick(&mut self, _now: Duration, _cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        if self.pulsing {
            let delta = if self.forward {
                self.pulse_step
            } else {
                -self.pulse_step
            };
            self.pulse_pos += delta;
            if self.pulse_pos >= 1.0 {
                self.pulse_pos = 1.0;
                self.forward = false;
            } else if self.pulse_pos <= 0.0 {
                self.pulse_pos = 0.0;
                self.forward = true;
            }
        }
        Vec::new()
    }

    fn next_deadline(&self, now: Duration) -> Option<Duration> {
        self.pulsing.then(|| now + Duration::from_millis(50))
    }

    fn on_event(&mut self, _ev: &Event, _cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        Vec::new()
    }

    fn measure(
        &mut self,
        available: (Option<f32>, Option<f32>),
        cx: &mut BuildCx<'_>,
    ) -> Option<(f32, f32)> {
        if let Some(label) = self.label.as_mut() {
            return Controller::<Msg>::measure(label, available, cx);
        }
        // GTK sizes a `GtkProgressBar` off its `trough` child's own request
        // (Adwaita's `progressbar.horizontal > trough { min-width: 150px;
        // min-height: 2px }`), but `trough`/`progress` are subnodes this
        // controller owns directly rather than `View` children, so they
        // never get a taffy node or a box of their own for that CSS to
        // reach (see `Controller::paint` below) — leaving this leaf's own
        // intrinsic size unset would collapse it to zero. Report Adwaita's
        // own horizontal trough minimum directly instead.
        Some((150.0, 2.0))
    }

    fn paint(
        &mut self,
        canvas: &mut skia_rs_safe::canvas::Canvas<'_>,
        alloc: &crate::layout::Allocation,
        style: &crate::css::computed::ComputedStyle,
        _cx: &mut crate::view::controller::PaintCx<'_>,
    ) -> bool {
        // The `progress` node's own box is painted by M2 across the whole
        // trough; the used fraction is the clip. Painting it here (rather than
        // sizing it in layout) is what keeps P5 out of `layout.rs`, which P6
        // owns.
        let content = alloc.content_box;
        let used = if self.pulsing {
            let width = content.width * 0.25;
            crate::layout::Rect::new(
                content.x + (content.width - width) * self.pulse_pos as f32,
                content.y,
                width,
                content.height,
            )
        } else {
            crate::layout::Rect::new(
                content.x,
                content.y,
                content.width * self.used() as f32,
                content.height,
            )
        };
        if used.is_empty() {
            return false;
        }
        canvas.draw_rect(&used.to_skia(), &crate::paint::fill_paint(style.color()));
        true
    }
}
