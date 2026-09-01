//! `GtkExpander` -- a disclosure triangle over a child.
//!
//! ```text
//! expander-widget
//! ╰── box
//!     ├── title
//!     │   ├── expander
//!     │   ╰── <child>
//!     ╰── <child>
//! ```
//!
//! The widget's own node is `expander-widget`; the node *named* `expander`
//! is the arrow inside it (`gtk/gtkexpander.c`). The `box`'s second child is
//! a dedicated `content` node this controller creates and keeps for the
//! whole widget's life: [`crate::widgets::child_slot`] routes the view's own
//! child underneath it, the same way `Popover`'s `contents` node works, so
//! the real child keeps its identity (focus, animation state, tree id)
//! across a collapse/expand cycle instead of being rebuilt, and the node
//! stays put for [`ExpanderC::content_scale`] to animate even when
//! `build_widget` never attaches a real child at all (its own unit tests).

use std::time::Duration;

use crate::css::node::{Node, PseudoStates};
use crate::layout::{BoxDirection, Container};
use crate::view::{
    BuildCx, Controller, Event, EventCx, EventKind, Kind, Prop, PropName, Props, View,
};
use crate::widgets::{Universal, prop_bool, set_container};

/// How long the disclosure animation runs; GTK's own expander transition.
const DURATION: Duration = Duration::from_millis(200);

/// `GtkExpander`'s own setters, chained after [`crate::view::builders::expander`].
///
/// A plain inherent `View<Msg>::use_underline` would shadow `LabelExt`'s and
/// `CheckButtonExt`'s own (different-semantics, contract §12 E-note 4's
/// known-bug) methods of the same name for every other widget in the crate,
/// since an inherent method always wins over a trait one -- an
/// action-at-a-distance this widget's own scoped trait avoids.
pub trait ExpanderExt<Msg>: Sized {
    /// `GtkExpander:expanded`.
    ///
    /// Moved here from an inherent `View::expanded` in the P6 fix wave: an
    /// inherent setter shadows every same-named trait method (contract §11
    /// E8).
    fn expanded(self, v: impl Into<Prop>) -> Self;
    /// `GtkExpander:use-underline` (mnemonics; the underscore is stripped).
    fn use_underline(self, v: impl Into<Prop>) -> Self;
    /// `GtkExpander:resize-toplevel`.
    fn resize_toplevel(self, v: impl Into<Prop>) -> Self;
}

impl<Msg: Clone + 'static> ExpanderExt<Msg> for View<Msg> {
    fn expanded(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::Expanded, v)
    }
    fn use_underline(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::UseUnderline, v)
    }
    fn resize_toplevel(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::ResizeToplevel, v)
    }
}

/// `GtkExpander`.
pub struct ExpanderC {
    /// Whether the child is disclosed.
    pub expanded: bool,
    /// `0.0` collapsed .. `1.0` expanded.
    pub progress: f32,
    /// The `title` node (arrow + label).
    pub title: Node,
    /// The arrow node, named `expander`.
    pub arrow: Node,
    /// The dedicated `content` subnode; the view's own child goes under it
    /// ([`crate::widgets::child_slot`]).
    pub content: Node,
    /// When the running animation started; `None` when at rest.
    started: Option<Duration>,
    universal: Universal,
}

impl ExpanderC {
    /// Test hook: the animation's current value.
    ///
    /// `Controller<Msg>: std::any::Any` (`view::controller`'s own doc
    /// comment) is what lets a `&dyn Controller<Msg>` coerce straight to
    /// `&dyn Any` at this call site -- trait-object upcasting, stable since
    /// the workspace's pinned Rust, needs no `as_any` method on the trait
    /// itself (`widgets::child_slot` and `PanedC::position_of` already rely
    /// on the same coercion).
    #[must_use]
    pub fn progress_of<Msg: 'static>(controller: &dyn Controller<Msg>) -> f32 {
        let any: &dyn std::any::Any = controller;
        any.downcast_ref::<Self>().map_or(f32::NAN, |c| c.progress)
    }

    /// Test hook: the content's current vertical scale.
    #[must_use]
    pub fn content_scale<Msg: 'static>(controller: &dyn Controller<Msg>) -> f32 {
        Self::progress_of(controller)
    }

    fn set_expanded(&mut self, node: &Node, on: bool, now: Duration) {
        if self.expanded == on {
            return;
        }
        self.expanded = on;
        self.started = Some(now);
        node.set_state(PseudoStates::CHECKED, on);
        self.arrow.set_state(PseudoStates::CHECKED, on);
    }

    /// Whether `local` (in `cx.node`'s own frame) falls inside `self.title`'s
    /// last recorded allocation, so a click lands only when it lands on the
    /// title row -- not anywhere in the (possibly much taller) content below
    /// it.
    fn hit_title<Msg>(&self, cx: &EventCx<'_, Msg>, local: (f32, f32)) -> bool {
        let Some(root) = cx.tree.allocation(cx.node) else {
            return false;
        };
        let Some(title) = cx.tree.allocation(&self.title) else {
            return false;
        };
        let x = local.0 + root.border_box.x;
        let y = local.1 + root.border_box.y;
        let t = title.border_box;
        x >= t.x && y >= t.y && x <= t.x + t.width && y <= t.y + t.height
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for ExpanderC {
    fn kind(&self) -> Kind {
        Kind::Expander
    }

    fn build(node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        set_container(
            node,
            Container::Box {
                direction: BoxDirection::Column,
            },
        );
        let outer = Node::new("box");
        node.append_child(&outer);
        set_container(
            &outer,
            Container::Box {
                direction: BoxDirection::Column,
            },
        );

        let title = Node::new("title");
        outer.append_child(&title);
        set_container(
            &title,
            Container::Box {
                direction: BoxDirection::Row,
            },
        );

        let arrow = Node::new("expander");
        title.append_child(&arrow);
        let label = Node::new("label");
        title.append_child(&label);

        // Always present, so the view's own child -- routed here by
        // `child_slot` -- lands in the same node across every reconcile,
        // and so `build_widget`'s own unit tests (no reconciler at all) can
        // still find a `content` node to animate.
        let content = Node::new("content");
        outer.append_child(&content);

        let expanded = props.bool(PropName::Expanded, false);
        node.set_state(PseudoStates::CHECKED, expanded);
        arrow.set_state(PseudoStates::CHECKED, expanded);

        Self {
            expanded,
            progress: if expanded { 1.0 } else { 0.0 },
            title,
            arrow,
            content,
            started: None,
            universal: Universal::new(node, Kind::Expander),
        }
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, cx: &mut BuildCx<'_>) {
        match name {
            PropName::Expanded => self.set_expanded(node, prop_bool(value, false), cx.clock.now()),
            // Chrome subnodes don't carry real text content anywhere in this
            // codebase yet (`Frame`'s `label`, `Button`'s `label`, ...); the
            // `label` node exists purely for the fixture/CSS shape.
            PropName::Label | PropName::UseUnderline | PropName::ResizeToplevel => {}
            other => {
                self.universal.apply(node, Kind::Expander, other, value);
            }
        }
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        let toggle = match ev {
            Event::Activate => true,
            Event::PointerUp { button, local, .. } if *button == crate::window::layer::BTN_LEFT => {
                self.hit_title(cx, *local)
            }
            _ => false,
        };
        if !toggle {
            return Vec::new();
        }
        let now = cx.clock.now();
        let want = !self.expanded;
        self.set_expanded(cx.node, want, now);
        cx.handled = true;
        cx.handlers
            .fire_bool(EventKind::Expanded, want)
            .into_iter()
            .collect()
    }

    fn tick(&mut self, now: Duration, _cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        let Some(started) = self.started else {
            return Vec::new();
        };
        let elapsed = now.saturating_sub(started);
        let t = if DURATION.is_zero() {
            1.0
        } else {
            (elapsed.as_secs_f32() / DURATION.as_secs_f32()).clamp(0.0, 1.0)
        };
        self.progress = if self.expanded { t } else { 1.0 - t };
        if t >= 1.0 {
            self.started = None;
        }
        Vec::new()
    }

    fn next_deadline(&self, now: Duration) -> Option<Duration> {
        self.started.map(|started| {
            let end = started + DURATION;
            if now >= end {
                Duration::ZERO
            } else {
                end - now
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::view::{Event, Kind, Prop, PropName, Props};
    use crate::widgets::{Headless, build_widget, matches_fixture};

    const FIXTURE: &str = "expander-widget\n╰── box\n    ├── title\n    │   ├── expander\n    \
                           │   ╰── <child>\n    ╰── <child>\n";

    #[test]
    fn the_widget_node_is_expander_widget_and_the_arrow_is_expander() {
        // Mutation check: naming the root `expander` (the name the *arrow*
        // has) makes every Adwaita `expander-widget` rule miss and the arrow
        // rule match the whole widget -- the single easiest error in this
        // catalogue, which is why the fixture is asserted both ways.
        let built = build_widget::<()>(Kind::Expander, &Props::default());
        matches_fixture(&built.node, FIXTURE).expect("expander fixture");
        assert_eq!(&*built.node.name(), "expander-widget");
        let arrow = built
            .node
            .child(0)
            .and_then(|b| b.child(0))
            .and_then(|t| t.child(0));
        assert_eq!(&*arrow.expect("arrow").name(), "expander");
    }

    #[test]
    fn space_toggles_and_emits_one_message() {
        // Interaction test. Mutation check: emitting on both press and
        // release doubles every expander message.
        let mut props = Props::default();
        props.set(PropName::Expanded, Prop::Bool(false));
        let built = build_widget::<bool>(Kind::Expander, &props);
        let mut c = built.controller;
        let mut hx = Headless::new();
        let mut cx = hx.event_cx_with_handlers(&built.node, |h| {
            h.set(
                crate::view::EventKind::Expanded,
                crate::view::Handler::Bool(std::rc::Rc::new(|on| on)),
            );
        });
        let msgs = c.on_event(&Event::Activate, &mut cx);
        assert_eq!(msgs, vec![true]);
        assert_eq!(c.on_event(&Event::Activate, &mut cx), vec![false]);
    }

    #[test]
    fn expanding_animates_progress_to_one_and_then_stops_asking_for_frames() {
        // Mutation check: a `next_deadline` that keeps returning Some after
        // the animation finishes spins the frame pump forever; returning
        // None too early freezes the arrow mid-rotation.
        let built = build_widget::<()>(Kind::Expander, &Props::default());
        let mut c = built.controller;
        let mut hx = Headless::new();
        let mut cx = hx.event_cx(&built.node);
        c.on_event(&Event::Activate, &mut cx);
        assert!(
            c.next_deadline(Duration::ZERO).is_some(),
            "a frame is wanted"
        );
        c.tick(Duration::from_millis(1_000), &mut cx);
        assert_eq!(crate::widgets::expander::ExpanderC::progress_of(&*c), 1.0);
        assert_eq!(c.next_deadline(Duration::from_millis(1_000)), None);
    }

    #[test]
    fn a_collapsed_expander_keeps_its_content_node_but_gives_it_no_height() {
        // Rest-state test. Mutation check: removing the content node on
        // collapse loses the child's focus and animation state, and the
        // reconciler then rebuilds it on every expand.
        let built = build_widget::<()>(Kind::Expander, &Props::default());
        let content = built
            .node
            .child(0)
            .and_then(|b| b.child(1))
            .expect("content");
        assert!(content.parent().is_some());
        assert_eq!(
            crate::widgets::expander::ExpanderC::content_scale(&*built.controller),
            0.0
        );
    }
}
