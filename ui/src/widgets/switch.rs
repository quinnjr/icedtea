//! `GtkSwitch` — `Kind::Switch`, CSS node `switch`.
//!
//! ```text
//! switch
//! ├── image
//! ├── image
//! ╰── slider
//! ```
//!
//! Four nodes, no style classes on any of them. The switch supports pan and
//! drag: dragging the slider past the midpoint toggles, and `slide` animates
//! the slider between the two ends on the animation clock.

use std::rc::Rc;
use std::time::Duration;

use crate::css::node::{Node, PseudoStates};
use crate::layout::Rect;
use crate::view::controller::{Controller, Event, EventCx};
use crate::view::{BuildCx, EventKind, Handler, Kind, Prop, PropName, Props, View};
use crate::widgets::PointerState;

/// How long the slider takes to cross, matching Adwaita's own switch timing.
const SLIDE: Duration = Duration::from_millis(200);

/// A `GtkSwitch` in `active`.
#[must_use]
pub fn switch<Msg: Clone + 'static>(active: bool) -> View<Msg> {
    View::new(Kind::Switch).prop(PropName::Active, Prop::Bool(active))
}

/// `GtkSwitch`'s own setters and signals.
pub trait SwitchExt<Msg>: Sized {
    /// `GtkSwitch:state` — the backend state, which may lag `active`.
    fn state(self, on: bool) -> Self;
    /// `GtkSwitch::state-set`.
    fn on_toggle(self, f: impl Fn(bool) -> Msg + 'static) -> Self;
}

impl<Msg: Clone + 'static> SwitchExt<Msg> for View<Msg> {
    fn state(self, on: bool) -> Self {
        self.prop(PropName::Checked, Prop::Bool(on))
    }
    fn on_toggle(self, f: impl Fn(bool) -> Msg + 'static) -> Self {
        self.on(EventKind::Toggle, Handler::Bool(Rc::new(f)))
    }
}

/// `Kind::Switch`'s controller.
pub struct SwitchC {
    /// `GtkSwitch:active`.
    pub active: bool,
    /// Grab offset within the slider while dragging.
    pub drag: Option<f32>,
    /// Slider position, `0.0` (off) to `1.0` (on).
    pub slide: f32,
    /// The `slider` subnode.
    pub slider: Node,
    /// The first `image` subnode (on).
    pub on_image: Node,
    /// The second `image` subnode (off).
    pub off_image: Node,
    target: f32,
    last_tick: Option<Duration>,
    pointer: PointerState,
}

impl SwitchC {
    fn apply(&mut self, node: &Node) {
        node.set_state(PseudoStates::CHECKED, self.active);
        self.target = if self.active { 1.0 } else { 0.0 };
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for SwitchC {
    fn kind(&self) -> Kind {
        Kind::Switch
    }

    fn build(node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        let on_image = Node::new("image");
        node.append_child(&on_image);
        let off_image = Node::new("image");
        node.append_child(&off_image);
        let slider = Node::new("slider");
        node.append_child(&slider);
        let active = props.bool(PropName::Active, false);
        let mut this = SwitchC {
            active,
            drag: None,
            slide: if active { 1.0 } else { 0.0 },
            slider,
            on_image,
            off_image,
            target: if active { 1.0 } else { 0.0 },
            last_tick: None,
            pointer: PointerState::default(),
        };
        this.apply(node);
        this
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        if matches!(name, PropName::Active | PropName::Checked)
            && let Prop::Bool(on) = value
        {
            self.active = *on;
            self.apply(node);
        }
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        let bounds = cx
            .tree
            .allocation(cx.node)
            .map(|a| Rect::new(0.0, 0.0, a.border_box.width, a.border_box.height));
        // A drag past the midpoint toggles; a plain click toggles too.
        if let (Event::PointerMotion { local }, Some(rect), Some(_)) = (ev, bounds, self.drag) {
            if rect.width > 0.0 {
                self.slide = (local.0 / rect.width).clamp(0.0, 1.0);
            }
            return Vec::new();
        }
        if matches!(ev, Event::PointerDown { .. }) {
            self.drag = Some(0.0);
        }
        let clicked = self.pointer.observe(cx.node, ev, bounds);
        if matches!(ev, Event::PointerUp { .. }) {
            let dragged = self.drag.take().is_some();
            let next = if dragged && (self.slide - if self.active { 1.0 } else { 0.0 }).abs() > 0.25
            {
                self.slide >= 0.5
            } else if clicked {
                !self.active
            } else {
                self.active
            };
            if next != self.active {
                self.active = next;
                self.apply(cx.node);
                cx.handled = true;
                return cx
                    .handlers
                    .fire_bool(EventKind::Toggle, next)
                    .map_or_else(Vec::new, |m| vec![m]);
            }
            self.apply(cx.node);
        }
        if matches!(ev, Event::Activate) {
            self.active = !self.active;
            self.apply(cx.node);
            cx.handled = true;
            let next = self.active;
            return cx
                .handlers
                .fire_bool(EventKind::Toggle, next)
                .map_or_else(Vec::new, |m| vec![m]);
        }
        Vec::new()
    }

    fn tick(&mut self, now: Duration, _cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        let elapsed = self
            .last_tick
            .map_or(Duration::ZERO, |last| now.saturating_sub(last));
        self.last_tick = Some(now);
        let step = elapsed.as_secs_f32() / SLIDE.as_secs_f32();
        if (self.target - self.slide).abs() <= step {
            self.slide = self.target;
        } else if self.target > self.slide {
            self.slide += step;
        } else {
            self.slide -= step;
        }
        Vec::new()
    }

    fn next_deadline(&self, now: Duration) -> Option<Duration> {
        ((self.target - self.slide).abs() > f32::EPSILON).then(|| now + Duration::from_millis(16))
    }

    fn measure(
        &mut self,
        _available: (Option<f32>, Option<f32>),
        _cx: &mut BuildCx<'_>,
    ) -> Option<(f32, f32)> {
        // `image`/`slider` are subnodes this controller owns directly rather
        // than `View` children (like `ProgressBarC`'s `trough`/`progress`
        // above), so they never get a taffy box of their own for Adwaita's
        // `switch > slider { min-width: 24px; min-height: 24px }` to reach.
        // GTK's own `GtkSwitch` requests room for the slider at both ends of
        // its travel; report that directly.
        Some((48.0, 24.0))
    }

    fn paint(
        &mut self,
        canvas: &mut skia_rs_safe::canvas::Canvas<'_>,
        alloc: &crate::layout::Allocation,
        style: &crate::css::computed::ComputedStyle,
        _cx: &mut crate::view::controller::PaintCx<'_>,
    ) -> bool {
        let content = alloc.content_box;
        let knob = content.height.min(content.width / 2.0);
        if knob <= 0.0 {
            return false;
        }
        let x = content.x + (content.width - knob) * self.slide.clamp(0.0, 1.0);
        let rect = Rect::new(x, content.y, knob, content.height);
        canvas.draw_rect(&rect.to_skia(), &crate::paint::fill_paint(style.color()));
        true
    }
}
