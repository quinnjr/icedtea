//! `GtkSpinner` — `Kind::Spinner`, CSS node `spinner`.
//!
//! ```text
//! spinner
//! ```
//!
//! One node. GTK adds `:checked` while the animation runs — its own divergence
//! from what `:checked` means everywhere else, and Adwaita styles it — so the
//! controller sets `PseudoStates::CHECKED`, not a style class.
//!
//! Adwaita's own rule (`spinner:checked { animation: spin 1s linear
//! infinite; }`, with `@keyframes spin { to { transform: rotate(1turn); } }`)
//! already rotates the node once a second through M2's `@keyframes` engine —
//! the same `transform` override machinery a hover transition runs through,
//! applied to the node before [`SpinnerC::paint`] ever runs. So the arc is
//! drawn at a *fixed* reference angle: a `self.phase`-driven angle here too
//! would compound with Adwaita's own rotation (same 1s period), and at any
//! whole half-turn — 500ms, 1500ms, … — the two exactly cancel, which is
//! worse than either alone: not a faster spin, a frozen one, at one
//! unlucky-looking instant. `self.phase`/`tick`/`next_deadline` are kept to
//! this controller's documented interface (contract's `tick` names "spinner
//! rotation" as its own worked example) and still track real state, but
//! painting does not currently read `phase` — Adwaita is the only sheet this
//! crate ships, and it always supplies the rotation.

use std::time::Duration;

use skia_rs_safe::canvas::Canvas;
use skia_rs_safe::paint::{Paint, Style};

use crate::css::computed::ComputedStyle;
use crate::css::node::{Node, PseudoStates};
use crate::layout::Allocation;
use crate::view::controller::{Controller, Event, EventCx, PaintCx};
use crate::view::{BuildCx, Kind, Prop, PropName, Props, View};

/// One full rotation, matching Adwaita's own `spin` keyframe.
const PERIOD: Duration = Duration::from_millis(1000);

/// A `GtkSpinner`, stopped.
#[must_use]
pub fn spinner<Msg: Clone + 'static>() -> View<Msg> {
    View::new(Kind::Spinner)
}

/// `GtkSpinner:spinning`.
pub trait SpinnerExt<Msg>: Sized {
    /// Start or stop the animation.
    fn spinning(self, on: bool) -> Self;
}

impl<Msg: Clone + 'static> SpinnerExt<Msg> for View<Msg> {
    fn spinning(self, on: bool) -> Self {
        self.prop(PropName::Active, Prop::Bool(on))
    }
}

/// `Kind::Spinner`'s controller.
pub struct SpinnerC {
    /// Whether the animation is running.
    pub spinning: bool,
    /// Rotation in turns, `0.0..1.0`, advanced every tick while spinning —
    /// see the module doc for why `paint` does not read it back.
    pub phase: f32,
    /// Clock reading the current rotation is measured from.
    pub started: Duration,
}

impl SpinnerC {
    fn apply(&self, node: &Node) {
        node.set_state(PseudoStates::CHECKED, self.spinning);
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for SpinnerC {
    fn kind(&self) -> Kind {
        Kind::Spinner
    }

    fn build(node: &Node, props: &Props, cx: &mut BuildCx<'_>) -> Self {
        let this = SpinnerC {
            spinning: props.bool(PropName::Active, false),
            phase: 0.0,
            started: cx.clock.now(),
        };
        this.apply(node);
        this
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, cx: &mut BuildCx<'_>) {
        if name == PropName::Active {
            let next = matches!(value, Prop::Bool(true));
            if next != self.spinning {
                self.spinning = next;
                self.started = cx.clock.now();
                self.phase = 0.0;
                self.apply(node);
            }
        }
    }

    fn tick(&mut self, now: Duration, _cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        if self.spinning {
            let elapsed = now.saturating_sub(self.started).as_secs_f32();
            let period = PERIOD.as_secs_f32();
            self.phase = (elapsed / period).fract();
        }
        Vec::new()
    }

    /// One frame while spinning; nothing at all when stopped. `Duration::ZERO`
    /// is never returned — a stopped spinner must not pin the event loop.
    fn next_deadline(&self, now: Duration) -> Option<Duration> {
        self.spinning.then(|| now + Duration::from_millis(16))
    }

    fn on_event(&mut self, _ev: &Event, _cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        Vec::new()
    }

    /// GTK's spinner has no content to measure itself against — it is an icon
    /// (`-gtk-icon-source: -gtk-icontheme("process-working-symbolic")`), sized
    /// at `GTK_ICON_SIZE_NORMAL` (16px) unless CSS gives it an explicit size.
    fn measure(
        &mut self,
        _available: (Option<f32>, Option<f32>),
        _cx: &mut BuildCx<'_>,
    ) -> Option<(f32, f32)> {
        Some((16.0, 16.0))
    }

    fn paint(
        &mut self,
        canvas: &mut Canvas<'_>,
        alloc: &Allocation,
        style: &ComputedStyle,
        _cx: &mut PaintCx<'_>,
    ) -> bool {
        let content = alloc.content_box;
        let size = content.width.min(content.height);
        if size <= 0.0 {
            return false;
        }
        let stroke = (size / 8.0).max(1.0);
        let mut paint = Paint::new();
        paint.set_anti_alias(true);
        paint.set_style(Style::Stroke);
        paint.set_stroke_width(stroke);
        paint.set_color32(style.color().to_color32());
        let inset = stroke / 2.0;
        let bounds = crate::layout::Rect::new(
            content.x + (content.width - size) / 2.0 + inset,
            content.y + (content.height - size) / 2.0 + inset,
            size - stroke,
            size - stroke,
        );
        // A fixed 90° arc — GTK's spinner is a quarter-circle chasing its own
        // tail, always drawn at the same reference angle. The rotation is
        // Adwaita's `spin` keyframe (module doc), applied to the whole node
        // as a `transform` override before this closure ever runs.
        canvas.draw_arc(&bounds.to_skia(), -90.0, 90.0, false, &paint);
        true
    }
}
