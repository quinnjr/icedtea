//! `GtkSpinButton` — `Kind::SpinButton`, CSS node `spinbutton`.
//!
//! ```text
//! spinbutton.horizontal
//! ├── text
//! │    ├── undershoot.left
//! │    ╰── undershoot.right
//! ├── button.down
//! ╰── button.up
//! ```
//!
//! The vertical form orders the children `button.up`, `text`, `button.down`.
//! The steppers paint `Builtin::SpinPlus`/`SpinMinus`; a held stepper repeats
//! through [`RepeatTimer`] with `climb-rate` acceleration.

use std::rc::Rc;
use std::time::Duration;

use crate::css::node::Node;
use crate::icons::builtin::Builtin;
use crate::layout::Rect;
use crate::view::controller::{Controller, Event, EventCx};
use crate::view::{BuildCx, EventKind, Handler, Kind, Prop, PropName, Props, View};
use crate::widgets::edit::{EditOutcome, TextEditState};
use crate::widgets::{Adjustment, Orientation, PointerState, RepeatTimer, WidgetEnum, shift_event};
use crate::window::focus::FocusCause;

/// Delay before a held stepper starts repeating, and the initial interval.
const REPEAT_DELAY: Duration = Duration::from_millis(400);
const REPEAT_INTERVAL: Duration = Duration::from_millis(100);

/// A `GtkSpinButton` at `value` over `lower..=upper`.
#[must_use]
pub fn spin_button<Msg: Clone + 'static>(value: f64, lower: f64, upper: f64) -> View<Msg> {
    View::new(Kind::SpinButton)
        .prop(PropName::Value, Prop::Float(value))
        .prop(PropName::Lower, Prop::Float(lower))
        .prop(PropName::Upper, Prop::Float(upper))
}

/// `GtkSpinButton`'s own setters and signals.
pub trait SpinButtonExt<Msg>: Sized {
    /// `GtkAdjustment:step-increment`.
    fn step(self, v: f64) -> Self;
    /// `GtkAdjustment:page-increment`.
    fn page(self, v: f64) -> Self;
    /// `GtkSpinButton:digits`.
    fn digits(self, n: u32) -> Self;
    /// `GtkSpinButton:wrap`.
    fn wrap(self, on: bool) -> Self;
    /// `GtkSpinButton:numeric`.
    fn numeric(self, on: bool) -> Self;
    /// `GtkSpinButton:snap-to-ticks`.
    fn snap_to_ticks(self, on: bool) -> Self;
    /// `GtkSpinButton:climb-rate`.
    fn climb_rate(self, v: f64) -> Self;
    /// `GtkOrientable:orientation`.
    fn orientation(self, o: Orientation) -> Self;
    /// `GtkSpinButton::value-changed`.
    fn on_value_changed(self, f: impl Fn(f64) -> Msg + 'static) -> Self;
}

impl<Msg: Clone + 'static> SpinButtonExt<Msg> for View<Msg> {
    fn step(self, v: f64) -> Self {
        self.prop(PropName::StepIncrement, Prop::Float(v))
    }
    fn page(self, v: f64) -> Self {
        self.prop(PropName::PageIncrement, Prop::Float(v))
    }
    fn digits(self, n: u32) -> Self {
        self.prop(PropName::Digits, Prop::Int(i64::from(n)))
    }
    fn wrap(self, on: bool) -> Self {
        self.prop(PropName::Wrap, Prop::Bool(on))
    }
    fn numeric(self, on: bool) -> Self {
        self.prop(PropName::Editable, Prop::Bool(on))
    }
    fn snap_to_ticks(self, on: bool) -> Self {
        self.prop(PropName::Homogeneous, Prop::Bool(on))
    }
    fn climb_rate(self, v: f64) -> Self {
        self.prop(PropName::Ratio, Prop::Float(v))
    }
    fn orientation(self, o: Orientation) -> Self {
        self.prop(PropName::Orientation, o.to_prop())
    }
    fn on_value_changed(self, f: impl Fn(f64) -> Msg + 'static) -> Self {
        self.on(EventKind::ValueChanged, Handler::Float(Rc::new(f)))
    }
}

/// `Kind::SpinButton`'s controller.
pub struct SpinButtonC {
    /// The text half's editing state.
    pub edit: TextEditState,
    /// The numeric model.
    pub adj: Adjustment,
    /// `GtkSpinButton:digits`.
    pub digits: u32,
    /// `GtkSpinButton:wrap`.
    pub wrap: bool,
    /// The held-stepper repeat clock.
    pub repeat: Option<RepeatTimer>,
    /// The `button.up` subnode.
    pub up: Node,
    /// The `button.down` subnode.
    pub down: Node,
    climb: f64,
    /// `+1` while the up stepper is held, `-1` for down.
    direction: i32,
    pointer: PointerState,
    orientation: Orientation,
}

/// The pixel width (horizontal form) or height (vertical form) reserved for
/// each stepper button.
///
/// Plan reconciliation: `up`/`down` are subnodes this controller appends
/// directly rather than `View` children (the same gap `ScaleC::on_event`
/// documents for its own `trough`/`slider`), so they never get a taffy box of
/// their own — `local_rect(cx.tree, cx.node, &self.up)` the plan's `on_event`
/// relied on is always `None`, and with no `measure` at all the whole control
/// fell back to a zero-content-box default. Both `measure` and `on_event`
/// below derive the steppers' geometry from this constant against the root's
/// own allocation instead, matching `ScaleC`'s and `EntryC`'s precedent.
const STEPPER_SIZE: f32 = 20.0;

impl SpinButtonC {
    /// Format `value` at `digits`. A non-finite value renders as zero rather
    /// than `NaN`, which no spin button ever shows.
    #[must_use]
    pub fn format(value: f64, digits: u32) -> String {
        let value = if value.is_finite() { value } else { 0.0 };
        format!("{value:.*}", digits as usize)
    }

    /// Step by `steps` increments (page increments when `page`), wrapping when
    /// `wrap`. Returns the new value.
    pub fn step_by(&mut self, steps: i32, page: bool) -> f64 {
        let increment = if page {
            self.adj.page_increment
        } else {
            self.adj.step_increment
        };
        let delta = f64::from(steps) * increment;
        let raw = self.adj.value + delta;
        let span = self.adj.upper - self.adj.lower;
        let next = if self.wrap && span > 0.0 {
            if raw > self.adj.upper {
                self.adj.lower + (raw - self.adj.upper - increment).rem_euclid(span)
            } else if raw < self.adj.lower {
                self.adj.upper - (self.adj.lower - raw - increment).rem_euclid(span)
            } else {
                raw
            }
        } else {
            raw
        };
        self.adj.set_value(next);
        self.adj.value
    }

    fn sync_text(&mut self, cx: &mut BuildCx<'_>) {
        let text = Self::format(self.adj.value, self.digits);
        self.edit.set_text(&text, cx);
    }

    /// The `up`/`down` stepper hit boxes, in the root node's own border-box
    /// space — see [`STEPPER_SIZE`] for why these are computed rather than
    /// read off `local_rect`.
    fn stepper_rects(&self, border_width: f32, border_height: f32) -> (Rect, Rect) {
        match self.orientation {
            Orientation::Horizontal => (
                Rect::new(
                    border_width - STEPPER_SIZE,
                    0.0,
                    STEPPER_SIZE,
                    border_height,
                ),
                Rect::new(
                    border_width - 2.0 * STEPPER_SIZE,
                    0.0,
                    STEPPER_SIZE,
                    border_height,
                ),
            ),
            Orientation::Vertical => (
                Rect::new(0.0, 0.0, border_width, STEPPER_SIZE),
                Rect::new(
                    0.0,
                    border_height - STEPPER_SIZE,
                    border_width,
                    STEPPER_SIZE,
                ),
            ),
        }
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for SpinButtonC {
    fn kind(&self) -> Kind {
        Kind::SpinButton
    }

    fn build(node: &Node, props: &Props, cx: &mut BuildCx<'_>) -> Self {
        let orientation =
            Orientation::from_prop(props.get(PropName::Orientation), Orientation::Horizontal);
        node.add_class(orientation.css_class());
        let adj = Adjustment {
            value: props.float(PropName::Value, 0.0),
            lower: props.float(PropName::Lower, 0.0),
            upper: props.float(PropName::Upper, 100.0),
            step_increment: props.float(PropName::StepIncrement, 1.0),
            page_increment: props.float(PropName::PageIncrement, 10.0),
            page_size: 0.0,
        }
        .sanitized();
        let digits = u32::try_from(props.int(PropName::Digits, 0))
            .unwrap_or(0)
            .min(20);
        let up = Node::with_classes("button", &["up"]);
        let down = Node::with_classes("button", &["down"]);
        // Horizontal: text, button.down, button.up. Vertical: up, text, down.
        let edit = match orientation {
            Orientation::Horizontal => {
                let edit = TextEditState::build(node, &Self::format(adj.value, digits), cx);
                node.append_child(&down);
                node.append_child(&up);
                edit
            }
            Orientation::Vertical => {
                node.append_child(&up);
                let edit = TextEditState::build(node, &Self::format(adj.value, digits), cx);
                node.append_child(&down);
                edit
            }
        };
        SpinButtonC {
            edit,
            adj,
            digits,
            wrap: props.bool(PropName::Wrap, false),
            repeat: None,
            up,
            down,
            climb: props.float(PropName::Ratio, 1.0),
            direction: 0,
            pointer: PointerState::default(),
            orientation,
        }
    }

    fn set_prop(&mut self, _node: &Node, name: PropName, value: &Prop, cx: &mut BuildCx<'_>) {
        match (name, value) {
            (PropName::Value, Prop::Float(v)) => {
                self.adj.set_value(*v);
            }
            (PropName::Lower, Prop::Float(v)) => self.adj.lower = *v,
            (PropName::Upper, Prop::Float(v)) => self.adj.upper = *v,
            (PropName::StepIncrement, Prop::Float(v)) => self.adj.step_increment = *v,
            (PropName::Digits, Prop::Int(n)) => {
                self.digits = u32::try_from(*n).unwrap_or(0).min(20);
            }
            (PropName::Wrap, Prop::Bool(on)) => self.wrap = *on,
            _ => return,
        }
        self.adj = self.adj.sanitized();
        self.sync_text(cx);
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        // Plan reconciliation: the plan's `on_event` never granted keyboard
        // focus on click, the same gap `Entry`/`TextView` had — without this
        // a `SpinButton`'s text half could never receive `Key` events by any
        // means (contract §11 E — `FocusRing` only moves from a click,
        // `Cmd::Focus`, or a keyboard binding).
        if matches!(ev, Event::PointerDown { .. }) {
            cx.focus.set_focus(Some(cx.node), FocusCause::Pointer);
        }
        let stepper_rects = cx
            .tree
            .allocation(cx.node)
            .map(|alloc| self.stepper_rects(alloc.border_box.width, alloc.border_box.height));
        if let Some((up_rect, down_rect)) = stepper_rects {
            for (node, rect, direction) in [
                (self.up.clone(), up_rect, 1),
                (self.down.clone(), down_rect, -1),
            ] {
                let shifted = shift_event(ev, rect);
                let bounds = Rect::new(0.0, 0.0, rect.width, rect.height);
                if let Event::PointerDown { local, .. } = shifted
                    && local.0 >= 0.0
                    && local.1 >= 0.0
                    && local.0 <= bounds.width
                    && local.1 <= bounds.height
                {
                    self.direction = direction;
                    self.repeat = Some(RepeatTimer::armed(
                        cx.clock.now(),
                        REPEAT_DELAY,
                        REPEAT_INTERVAL,
                        self.climb,
                    ));
                    let value = self.step_by(direction, false);
                    cx.handled = true;
                    return cx
                        .handlers
                        .fire_float(EventKind::ValueChanged, value)
                        .map_or_else(Vec::new, |m| vec![m]);
                }
                self.pointer.observe(&node, &shifted, Some(bounds));
            }
        }
        if matches!(ev, Event::PointerUp { .. }) {
            self.repeat = None;
            self.direction = 0;
        }
        if let Event::Scroll(scroll) = ev {
            let steps = if scroll.dy > 0.0 { -1 } else { 1 };
            let value = self.step_by(steps, false);
            cx.handled = true;
            return cx
                .handlers
                .fire_float(EventKind::ValueChanged, value)
                .map_or_else(Vec::new, |m| vec![m]);
        }
        let Event::Key(key) = ev else {
            return Vec::new();
        };
        use xkbcommon::xkb::keysyms;
        let steps = match u32::from(key.keysym) {
            keysyms::KEY_Up if key.pressed => Some((1, false)),
            keysyms::KEY_Down if key.pressed => Some((-1, false)),
            keysyms::KEY_Page_Up if key.pressed => Some((1, true)),
            keysyms::KEY_Page_Down if key.pressed => Some((-1, true)),
            _ => None,
        };
        if let Some((steps, page)) = steps {
            let value = self.step_by(steps, page);
            cx.handled = true;
            return cx
                .handlers
                .fire_float(EventKind::ValueChanged, value)
                .map_or_else(Vec::new, |m| vec![m]);
        }
        // Typing edits the text; the model re-parses it on `Change`.
        match self.edit.key(key, cx) {
            EditOutcome::Changed | EditOutcome::Activated => {
                cx.handled = true;
                let parsed = self.edit.buffer.trim().parse::<f64>().ok();
                if let Some(value) = parsed.filter(|v| v.is_finite()) {
                    self.adj.set_value(value);
                    return cx
                        .handlers
                        .fire_float(EventKind::ValueChanged, self.adj.value)
                        .map_or_else(Vec::new, |m| vec![m]);
                }
                Vec::new()
            }
            EditOutcome::Moved => {
                cx.handled = true;
                Vec::new()
            }
            EditOutcome::Ignored => Vec::new(),
        }
    }

    fn tick(&mut self, now: Duration, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        let Some(timer) = self.repeat.as_mut() else {
            return Vec::new();
        };
        let fired = timer.fire(now);
        if fired == 0 {
            return Vec::new();
        }
        let steps = self.direction * i32::try_from(fired).unwrap_or(1);
        let value = self.step_by(steps, false);
        cx.handlers
            .fire_float(EventKind::ValueChanged, value)
            .map_or_else(Vec::new, |m| vec![m])
    }

    fn next_deadline(&self, _now: Duration) -> Option<Duration> {
        self.repeat.as_ref().map(RepeatTimer::deadline)
    }

    fn measure(
        &mut self,
        _available: (Option<f32>, Option<f32>),
        cx: &mut BuildCx<'_>,
    ) -> Option<(f32, f32)> {
        // Plan reconciliation: the plan gave no `measure` at all. `text` and
        // the two `button` subnodes are appended directly (not `View`
        // children), so taffy sees no children to size against and the whole
        // control fell back to a zero-content-box default (`EntryC`'s and
        // `ScaleC`'s controllers hit the same gap; see [`STEPPER_SIZE`]).
        self.edit.reshape(cx);
        let (text_w, text_h) = self.edit.layout.size();
        Some(match self.orientation {
            Orientation::Horizontal => (text_w + 2.0 * STEPPER_SIZE, text_h.max(STEPPER_SIZE)),
            Orientation::Vertical => (text_w.max(STEPPER_SIZE), text_h + 2.0 * STEPPER_SIZE),
        })
    }

    fn paint(
        &mut self,
        canvas: &mut skia_rs_safe::canvas::Canvas<'_>,
        alloc: &crate::layout::Allocation,
        style: &crate::css::computed::ComputedStyle,
        _cx: &mut crate::view::controller::PaintCx<'_>,
    ) -> bool {
        let content = alloc.content_box;
        self.edit
            .layout
            .draw(canvas, (content.x, content.y), style.color());
        // The steppers' builtin glyphs; P7 fills in the real geometry (D9).
        let _ = (Builtin::SpinPlus, Builtin::SpinMinus);
        true
    }
}
