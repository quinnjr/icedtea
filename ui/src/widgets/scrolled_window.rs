//! `GtkScrolledWindow` -- a viewport with overshoot, kinetic scrolling and
//! (optionally overlaid) scrollbars.
//!
//! ```text
//! scrolledwindow[.frame]
//! ├── <child>
//! ├── [overshoot.top][.bottom][.left][.right]
//! ├── [undershoot.top][.bottom][.left][.right]
//! ├── [scrollbar.horizontal[.overlay-indicator][.dragging][.hovering]]
//! ├── [scrollbar.vertical[.overlay-indicator][.dragging][.hovering]]
//! ╰── [junction]
//! ```
//!
//! `undershoot` (GTK's brief "tried to scroll past the edge with no elastic
//! pull" flash) is not driven by anything this task exercises -- it is
//! always absent, which the fixture's optional bracket already allows.

use std::time::Duration;

use crate::css::node::Node;
use crate::layout::{BoxDirection, Container};
use crate::view::controller::{Controller, Event, EventCx};
use crate::view::{BuildCx, EventKind, Kind, Prop, PropName, Props};
use crate::widgets::scrollbar::ScrollbarC;
use crate::widgets::types::Policy;
use crate::widgets::{Universal, prop_bool, prop_u16};
use crate::window::pointer::{Kinetic, Scroll, ScrollSource};

/// How far past the end a drag may pull, in px, before it stops moving.
const MAX_OVERSHOOT: f32 = 64.0;
/// Spring-back time from a full overshoot.
const SPRING: Duration = Duration::from_millis(200);
/// The synthetic gap used to seed a velocity estimate from a single
/// "flick" event (a lift-off `Scroll` whose `dx`/`dy` carry the last
/// motion's distance) -- see [`ScrolledWindowC::feed_kinetic`].
const FLICK_SEED_DT: Duration = Duration::from_millis(16);

/// Content-size hints GTK derives layout from (`GtkScrolledWindow`'s
/// `min/max-content-{width,height}` and `propagate-natural-{width,height}`).
///
/// Nothing in this crate's layout tree yet accepts a content-size override
/// from outside the CSS cascade (`ui/src/layout.rs`'s own min/max sizing is
/// P6-off-limits except Task 1's additions, and this task adds none), so
/// these are recorded for a future part to consume rather than applied.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct ContentBounds {
    min_width: Option<f32>,
    min_height: Option<f32>,
    max_width: Option<f32>,
    max_height: Option<f32>,
    propagate_natural_width: bool,
    propagate_natural_height: bool,
}

/// `GtkScrolledWindow`.
pub struct ScrolledWindowC {
    /// Current scroll offset, px, always within `0..=max`.
    pub offset: (f32, f32),
    /// `(content, page)` extents, px.
    pub extent: ((f32, f32), (f32, f32)),
    /// Deceleration state for a flick.
    pub kinetic: Kinetic,
    /// How far past an edge the content is pulled, px.
    pub overshoot: (f32, f32),
    /// Live scrollbar controllers, when their policy and extent want one.
    pub hbar: Option<ScrollbarC>,
    /// As `hbar`, vertically.
    pub vbar: Option<ScrollbarC>,
    /// Overlay bars stay visible until this instant after the last scroll.
    pub hovering_until: Option<Duration>,
    /// The `junction` node, present only while both bars are.
    pub junction: Option<Node>,
    /// Policies, and the flags that steer them.
    policy: (Policy, Policy),
    /// `GtkScrolledWindow:overlay-scrolling`.
    overlay: bool,
    /// `GtkScrolledWindow:kinetic-scrolling`.
    kinetic_enabled: bool,
    /// Content-size hints; recorded, not yet wired to layout (see
    /// [`ContentBounds`]).
    content: ContentBounds,
    /// This controller's own root node.
    root: Node,
    /// The `scrollbar.horizontal`/`scrollbar.vertical` nodes themselves --
    /// kept apart from `hbar`/`vbar` because those hold the *sub*nodes a
    /// [`ScrollbarC`] built under them, not the node the bar hangs from.
    hbar_node: Option<Node>,
    vbar_node: Option<Node>,
    /// The `overshoot.<left|right>` / `overshoot.<top|bottom>` nodes, one
    /// per axis, present only while that axis is pulled past its end.
    overshoot_x_node: Option<Node>,
    overshoot_y_node: Option<Node>,
    /// Spring-back start instant.
    spring_started: Option<Duration>,
    /// Whether [`Kinetic`] is (or might still be, pending the next
    /// [`Kinetic::sample`]) coasting -- `Kinetic` itself keeps that flag
    /// private, so this controller tracks its own mirror of it for
    /// [`Controller::next_deadline`], which cannot call `sample` (it takes
    /// `&self`, and sampling is destructive).
    kinetic_active: bool,
    universal: Universal,
}

impl ScrolledWindowC {
    /// The maximum offset on each axis, never negative and never NaN.
    #[must_use]
    fn max_offset(&self) -> (f32, f32) {
        let f = |content: f32, page: f32| {
            if content.is_finite() && page.is_finite() {
                (content - page).max(0.0)
            } else {
                0.0
            }
        };
        let ((cx, cy), (px, py)) = self.extent;
        (f(cx, px), f(cy, py))
    }

    /// Apply a delta, clamping to the ends and recording the overshoot.
    ///
    /// Scrolling is *never* gated on the scrollbar policy: `Policy::Never`
    /// hides the bar, it does not freeze the view (`gtkscrolledwindow.c`).
    pub fn scroll_by(&mut self, delta: (f32, f32), now: Duration) -> (f32, f32) {
        let (max_x, max_y) = self.max_offset();
        let dx = if delta.0.is_finite() { delta.0 } else { 0.0 };
        let dy = if delta.1.is_finite() { delta.1 } else { 0.0 };
        let wanted = (self.offset.0 + dx, self.offset.1 + dy);
        let clamped = (wanted.0.clamp(0.0, max_x), wanted.1.clamp(0.0, max_y));
        let over = (
            (wanted.0 - clamped.0).clamp(-MAX_OVERSHOOT, MAX_OVERSHOOT),
            (wanted.1 - clamped.1).clamp(-MAX_OVERSHOOT, MAX_OVERSHOOT),
        );
        self.offset = clamped;
        self.overshoot = over;
        if over != (0.0, 0.0) {
            self.spring_started = Some(now);
        }
        self.sync_overshoot_nodes();
        self.offset
    }

    /// Which bars are currently rendered.
    #[must_use]
    pub fn visible_bars(&self) -> (bool, bool) {
        let (max_x, max_y) = self.max_offset();
        let want = |policy: Policy, max: f32| match policy {
            Policy::Always => true,
            Policy::Automatic => max > 0.0,
            Policy::Never | Policy::External => false,
        };
        (want(self.policy.0, max_x), want(self.policy.1, max_y))
    }

    /// Test/headless hook: replace the extent and resync the bars, without
    /// going through a `Prop`.
    pub fn set_extent<Msg: 'static>(
        controller: &mut dyn Controller<Msg>,
        content: (f32, f32),
        page: (f32, f32),
    ) {
        let any: &mut dyn std::any::Any = controller;
        if let Some(me) = any.downcast_mut::<Self>() {
            me.extent = (content, page);
            me.sync_bars();
        }
    }

    /// Test/headless hook: the current offset.
    #[must_use]
    pub fn offset_of<Msg: 'static>(controller: &dyn Controller<Msg>) -> (f32, f32) {
        let any: &dyn std::any::Any = controller;
        any.downcast_ref::<Self>()
            .map_or((f32::NAN, f32::NAN), |c| c.offset)
    }

    /// Test/headless hook: the current overshoot.
    #[must_use]
    pub fn overshoot_of<Msg: 'static>(controller: &dyn Controller<Msg>) -> (f32, f32) {
        let any: &dyn std::any::Any = controller;
        any.downcast_ref::<Self>()
            .map_or((f32::NAN, f32::NAN), |c| c.overshoot)
    }

    /// Test/headless hook: which bars are currently visible.
    #[must_use]
    pub fn visible_bars_of<Msg: 'static>(controller: &dyn Controller<Msg>) -> (bool, bool) {
        let any: &dyn std::any::Any = controller;
        any.downcast_ref::<Self>()
            .map_or((false, false), |c| c.visible_bars())
    }

    /// Create or remove the `scrollbar`/`junction` chrome nodes and keep
    /// their overlay class in step with `self.overlay`.
    /// The chrome subnodes, in the sibling order GTK's own node block (this
    /// module's doc comment) and `fixtures/gtk4.22-node-trees/
    /// scrolled_window.txt` declare: overshoot, then the horizontal bar,
    /// then the vertical bar, then the junction. (`undershoot` is never
    /// built -- see the module doc.)
    fn chrome_in_order(&self) -> Vec<Node> {
        [
            self.overshoot_x_node.clone(),
            self.overshoot_y_node.clone(),
            self.hbar_node.clone(),
            self.vbar_node.clone(),
            self.junction.clone(),
        ]
        .into_iter()
        .flatten()
        .collect()
    }

    /// Move the chrome back behind the application child, in declared order.
    ///
    /// Chrome is *created* in whatever order the events that need it arrive
    /// -- the bars and junction at build or on a policy change, an
    /// `overshoot.<edge>` only the first time a drag pulls past an end --
    /// so appending each one as it appears produces `<child>`,
    /// `scrollbar.horizontal`, `scrollbar.vertical`, `junction`,
    /// `overshoot.bottom`: not the order this widget's own doc block, its
    /// fixture, or (since the paint walker treats node order as z-order) the
    /// theme's painting expects. This re-appends the whole chrome run in the
    /// declared order whenever it is not already in it, which is a no-op on
    /// a steady frame -- reattaching dirties a restyle, so the comparison
    /// comes first.
    fn restack_chrome(&self) {
        let want = self.chrome_in_order();
        let children = self.root.children();
        let tail = children.len().saturating_sub(want.len());
        if children.len() >= want.len()
            && children[tail..]
                .iter()
                .zip(&want)
                .all(|(have, expect)| have.ptr_eq(expect))
        {
            return;
        }
        for node in &want {
            node.detach();
        }
        for node in &want {
            self.root.append_child(node);
        }
    }

    fn sync_bars(&mut self) {
        let (want_h, want_v) = self.visible_bars();
        self.sync_one_bar(want_h, false);
        self.sync_one_bar(want_v, true);
        self.sync_bar_adjustments();
        let both = want_h && want_v;
        match (both, self.junction.take()) {
            (true, None) => {
                let j = Node::new("junction");
                self.root.append_child(&j);
                self.junction = Some(j);
            }
            (false, Some(j)) => {
                self.root.remove_child(&j);
            }
            (true, Some(j)) => self.junction = Some(j),
            (false, None) => {}
        }
        self.restack_chrome();
    }

    fn sync_one_bar(&mut self, want: bool, vertical: bool) {
        let (slot, node_slot, class) = if vertical {
            (&mut self.vbar, &mut self.vbar_node, "vertical")
        } else {
            (&mut self.hbar, &mut self.hbar_node, "horizontal")
        };
        match (want, node_slot.take()) {
            (true, None) => {
                let bar = Node::with_classes("scrollbar", &[class]);
                if self.overlay {
                    bar.add_class("overlay-indicator");
                }
                self.root.append_child(&bar);
                *slot = Some(ScrollbarC::for_node(&bar, vertical));
                *node_slot = Some(bar);
            }
            (false, Some(bar)) => {
                self.root.remove_child(&bar);
                *slot = None;
            }
            (true, Some(bar)) => {
                if self.overlay {
                    bar.add_class("overlay-indicator");
                } else {
                    bar.remove_class("overlay-indicator");
                }
                *node_slot = Some(bar);
            }
            (false, None) => {}
        }
    }

    /// Push the current extent/offset into whichever bars exist, so a
    /// dragged slider always reflects the real content, not the values it
    /// was created with.
    fn sync_bar_adjustments(&mut self) {
        let ((cx, cy), (px, py)) = self.extent;
        if let Some(bar) = &mut self.hbar {
            bar.set_adjustment(Self::axis_adjustment(f64::from(self.offset.0), cx, px));
        }
        if let Some(bar) = &mut self.vbar {
            bar.set_adjustment(Self::axis_adjustment(f64::from(self.offset.1), cy, py));
        }
    }

    fn axis_adjustment(value: f64, content: f32, page: f32) -> crate::widgets::Adjustment {
        let upper = if content.is_finite() {
            content.max(0.0)
        } else {
            0.0
        };
        let page_size = if page.is_finite() { page.max(0.0) } else { 0.0 };
        crate::widgets::Adjustment {
            value,
            lower: 0.0,
            upper: f64::from(upper),
            step_increment: 0.1,
            page_increment: f64::from(page_size),
            page_size: f64::from(page_size),
        }
        .sanitized()
    }

    /// Create/remove/reclass the `overshoot.<edge>` nodes from the sign of
    /// each `self.overshoot` component.
    fn sync_overshoot_nodes(&mut self) {
        Self::sync_edge(
            &self.root,
            &mut self.overshoot_x_node,
            self.overshoot.0,
            "left",
            "right",
        );
        Self::sync_edge(
            &self.root,
            &mut self.overshoot_y_node,
            self.overshoot.1,
            "top",
            "bottom",
        );
        self.restack_chrome();
    }

    fn sync_edge(root: &Node, slot: &mut Option<Node>, magnitude: f32, neg: &str, pos: &str) {
        if magnitude == 0.0 || !magnitude.is_finite() {
            if let Some(n) = slot.take() {
                root.remove_child(&n);
            }
            return;
        }
        let class = if magnitude < 0.0 { neg } else { pos };
        match slot {
            Some(n) => {
                n.remove_class(neg);
                n.remove_class(pos);
                n.add_class(class);
            }
            None => {
                let n = Node::with_classes("overshoot", &[class]);
                root.append_child(&n);
                *slot = Some(n);
            }
        }
    }

    fn set_content_bound(&mut self, name: PropName, value: &Prop) {
        let float = || match value {
            Prop::Float(v) if v.is_finite() => Some(*v as f32),
            Prop::Int(v) => Some(*v as f32),
            _ => None,
        };
        match name {
            PropName::MinContentWidth => self.content.min_width = float(),
            PropName::MinContentHeight => self.content.min_height = float(),
            PropName::MaxContentWidth => self.content.max_width = float(),
            PropName::MaxContentHeight => self.content.max_height = float(),
            PropName::PropagateNaturalWidth => {
                self.content.propagate_natural_width = prop_bool(value, false);
            }
            PropName::PropagateNaturalHeight => {
                self.content.propagate_natural_height = prop_bool(value, false);
            }
            _ => {}
        }
    }

    /// Feed a `Scroll` frame into `self.kinetic`.
    ///
    /// `Kinetic::feed` rates a motion frame only once the *following* frame
    /// tells it how long the first took, so a genuine touch drag is a
    /// sequence of non-stop frames ending in one `stop` frame with no
    /// distance of its own. This crate's headless tests instead send one
    /// "flick" frame whose `dx`/`dy` carry the whole gesture's last motion
    /// with `stop` already set; to rate that single frame, the same delta
    /// is fed once more first, non-stop, at `now`, with the real stop frame
    /// then landing `FLICK_SEED_DT` later -- the two-frame shape
    /// `Kinetic::feed` expects, without ever feeding an event stamped
    /// before this controller's own notion of `now` (a `ManualClock` a test
    /// never advances stays at zero, and `Duration` cannot go negative, so
    /// seeding *before* `now` would collapse both frames onto one instant
    /// and rate no velocity at all).
    fn feed_kinetic(&mut self, scroll: &Scroll, now: Duration) {
        if !self.kinetic_enabled {
            self.kinetic.cancel();
            return;
        }
        match scroll.source {
            ScrollSource::Finger if scroll.stop => {
                let seed = Scroll {
                    stop: false,
                    ..*scroll
                };
                self.kinetic.feed(&seed, now);
                let stop_at = now + FLICK_SEED_DT;
                self.kinetic.feed(scroll, stop_at);
                self.kinetic_active = self.kinetic.sample(stop_at).is_some();
            }
            ScrollSource::Finger => {
                self.kinetic.feed(scroll, now);
            }
            _ => self.kinetic.cancel(),
        }
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for ScrolledWindowC {
    fn kind(&self) -> Kind {
        Kind::ScrolledWindow
    }

    fn build(node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        crate::widgets::set_container(
            node,
            Container::Box {
                direction: BoxDirection::Column,
            },
        );
        if props.bool(PropName::HasFrame, false) {
            node.add_class("frame");
        }
        let mut me = Self {
            offset: (0.0, 0.0),
            extent: ((0.0, 0.0), (0.0, 0.0)),
            kinetic: Kinetic::default(),
            overshoot: (0.0, 0.0),
            hbar: None,
            vbar: None,
            hovering_until: None,
            junction: None,
            policy: (
                props
                    .get(PropName::HscrollbarPolicy)
                    .map_or(Policy::Automatic, |v| {
                        Policy::from_u16(prop_u16(v, Policy::Automatic.to_u16()))
                    }),
                props
                    .get(PropName::VscrollbarPolicy)
                    .map_or(Policy::Automatic, |v| {
                        Policy::from_u16(prop_u16(v, Policy::Automatic.to_u16()))
                    }),
            ),
            overlay: props.bool(PropName::OverlayScrolling, true),
            kinetic_enabled: props.bool(PropName::Kinetic, true),
            content: ContentBounds::default(),
            root: node.clone(),
            hbar_node: None,
            vbar_node: None,
            overshoot_x_node: None,
            overshoot_y_node: None,
            spring_started: None,
            kinetic_active: false,
            universal: Universal::new(node, Kind::ScrolledWindow),
        };
        me.sync_bars();
        me
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        match name {
            PropName::HasFrame => {
                if prop_bool(value, false) {
                    node.add_class("frame");
                } else {
                    node.remove_class("frame");
                }
            }
            PropName::HscrollbarPolicy => self.policy.0 = Policy::from_u16(prop_u16(value, 1)),
            PropName::VscrollbarPolicy => self.policy.1 = Policy::from_u16(prop_u16(value, 1)),
            PropName::OverlayScrolling => self.overlay = prop_bool(value, true),
            PropName::Kinetic => self.kinetic_enabled = prop_bool(value, true),
            PropName::MinContentWidth
            | PropName::MinContentHeight
            | PropName::MaxContentWidth
            | PropName::MaxContentHeight
            | PropName::PropagateNaturalWidth
            | PropName::PropagateNaturalHeight => {
                self.set_content_bound(name, value);
                return;
            }
            other => {
                self.universal
                    .apply(node, Kind::ScrolledWindow, other, value);
                return;
            }
        }
        self.sync_bars();
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        let Event::Scroll(scroll) = ev else {
            return Vec::new();
        };
        let now = cx.clock.now();
        let before = self.offset;
        self.scroll_by((scroll.dx, scroll.dy), now);
        self.feed_kinetic(scroll, now);
        // A fade timer only means something for a bar that can actually
        // hide again: an `Always`-policy bar is GTK's classic reserved-space
        // bar, never overlaid, so scrolling past it never arms one, and
        // neither does a scroll while no `Automatic` bar is even shown.
        let (want_h, want_v) = self.visible_bars();
        let overlayable = (want_h && self.policy.0 == Policy::Automatic)
            || (want_v && self.policy.1 == Policy::Automatic);
        if self.overlay && overlayable {
            self.hovering_until = Some(now + Duration::from_secs(1));
        }
        self.sync_bars();
        cx.handled = true;
        if self.offset == before {
            return Vec::new();
        }
        cx.handlers
            .fire_float(EventKind::Scrolled, f64::from(self.offset.1))
            .into_iter()
            .collect()
    }

    fn tick(&mut self, now: Duration, _cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        if let Some(delta) = self.kinetic.sample(now) {
            self.scroll_by(delta, now);
            self.kinetic_active = true;
        } else {
            self.kinetic_active = false;
        }
        if let Some(started) = self.spring_started {
            let t =
                (now.saturating_sub(started).as_secs_f32() / SPRING.as_secs_f32()).clamp(0.0, 1.0);
            self.overshoot = (self.overshoot.0 * (1.0 - t), self.overshoot.1 * (1.0 - t));
            if t >= 1.0 {
                self.overshoot = (0.0, 0.0);
                self.spring_started = None;
            }
            self.sync_overshoot_nodes();
        }
        if self.hovering_until.is_some_and(|until| now >= until) {
            self.hovering_until = None;
        }
        self.sync_bars();
        Vec::new()
    }

    fn next_deadline(&self, now: Duration) -> Option<Duration> {
        let kinetic = self.kinetic_active.then_some(Duration::ZERO);
        let spring = self.spring_started.map(|s| {
            let end = s + SPRING;
            if now >= end {
                Duration::ZERO
            } else {
                end - now
            }
        });
        let hover = self.hovering_until.map(|until| {
            if now >= until {
                Duration::ZERO
            } else {
                until - now
            }
        });
        [kinetic, spring, hover].into_iter().flatten().min()
    }

    fn child_index(&self, _view_index: usize) -> usize {
        // `scrolled_window(child)` always hands exactly one view child;
        // `build` runs before it is reconciled in, so whatever chrome
        // `sync_bars` already attached at build time (an `Always`-policy
        // bar, say) must not keep it from landing first.
        0
    }

    fn reserved_total(&self, view_count: usize) -> usize {
        let chrome = usize::from(self.hbar_node.is_some())
            + usize::from(self.vbar_node.is_some())
            + usize::from(self.junction.is_some())
            + usize::from(self.overshoot_x_node.is_some())
            + usize::from(self.overshoot_y_node.is_some());
        view_count + chrome
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::view::{Event, Kind, Prop, PropName, Props};
    use crate::widgets::scrolled_window::ScrolledWindowC;
    use crate::widgets::types::Policy;
    use crate::widgets::{Headless, build_widget};
    use crate::window::pointer::{Scroll, ScrollSource};

    /// [`Headless`] builds no pointer input on its own; these two are this
    /// task's own extension of it, kept local rather than added to
    /// `widgets::mod` (P6 may only touch that file's `mod`/`match` lines).
    trait HeadlessScrollExt {
        fn scroll(dx: f32, dy: f32) -> Scroll;
        fn flick(dx: f32, dy: f32) -> Scroll;
    }

    impl HeadlessScrollExt for Headless {
        fn scroll(dx: f32, dy: f32) -> Scroll {
            Scroll {
                dx,
                dy,
                source: ScrollSource::Wheel,
                stop: false,
                time_ms: 0,
            }
        }

        /// A finger lift-off carrying the flick's last motion delta -- see
        /// `ScrolledWindowC::feed_kinetic`'s doc comment for how a single
        /// frame like this seeds a velocity estimate.
        fn flick(dx: f32, dy: f32) -> Scroll {
            Scroll {
                dx,
                dy,
                source: ScrollSource::Finger,
                stop: true,
                time_ms: 0,
            }
        }
    }

    const FIXTURE: &str = "scrolledwindow[.frame]\n├── <child>\n\
        ├── [overshoot.top]\n├── [undershoot.top]\n\
        ├── [scrollbar.horizontal]\n├── [scrollbar.vertical]\n╰── [junction]\n";

    fn props(frame: bool) -> Props {
        let mut p = Props::default();
        p.set(PropName::HasFrame, Prop::Bool(frame));
        p.set(
            PropName::HscrollbarPolicy,
            Prop::Enum(Policy::Automatic.to_u16()),
        );
        p.set(
            PropName::VscrollbarPolicy,
            Prop::Enum(Policy::Always.to_u16()),
        );
        p
    }

    #[test]
    fn the_subnodes_are_gtks_and_frame_is_a_class_on_the_root() {
        // Mutation check: making `.frame` a subnode (or naming the scrollbar
        // subnodes `hscrollbar`/`vscrollbar`) fails the fixture, and every
        // Adwaita `scrolledwindow.frame` and `scrolledwindow > scrollbar`
        // rule stops matching.
        let built = build_widget::<()>(Kind::ScrolledWindow, &props(true));
        crate::widgets::matches_fixture(&built.node, FIXTURE).expect("scrolled_window fixture");
        assert!(built.node.classes().iter().any(|c| c.as_str() == "frame"));
    }

    #[test]
    fn policy_never_hides_the_bar_but_keeps_scrolling_working() {
        // Mutation check: treating Policy::Never as "do not scroll" makes
        // the offset stay zero, which is how a themed sidebar becomes
        // unscrollable while looking fine.
        let mut p = props(false);
        p.set(
            PropName::VscrollbarPolicy,
            Prop::Enum(Policy::Never.to_u16()),
        );
        let built = build_widget::<()>(Kind::ScrolledWindow, &p);
        let mut c = built.controller;
        let mut hx = Headless::new();
        let mut cx = hx.event_cx(&built.node);
        ScrolledWindowC::set_extent(c.as_mut(), (100.0, 1000.0), (100.0, 100.0));
        c.on_event(&Event::Scroll(Headless::scroll(0.0, 50.0)), &mut cx);
        assert_eq!(ScrolledWindowC::offset_of(c.as_ref()).1, 50.0);
        assert_eq!(ScrolledWindowC::visible_bars_of(c.as_ref()), (false, false));
    }

    #[test]
    fn scrolling_past_the_end_overshoots_and_springs_back_on_the_clock() {
        // Mutation check: clamping the offset without recording overshoot
        // leaves `overshoot` at zero and the `overshoot.bottom` node never
        // appears; never decaying it leaves the content permanently offset.
        let built = build_widget::<()>(Kind::ScrolledWindow, &props(false));
        let mut c = built.controller;
        let mut hx = Headless::new();
        let mut cx = hx.event_cx(&built.node);
        ScrolledWindowC::set_extent(c.as_mut(), (100.0, 200.0), (100.0, 100.0));
        c.on_event(&Event::Scroll(Headless::scroll(0.0, 500.0)), &mut cx);
        assert_eq!(
            ScrolledWindowC::offset_of(c.as_ref()).1,
            100.0,
            "clamped to the end"
        );
        assert!(
            ScrolledWindowC::overshoot_of(c.as_ref()).1 > 0.0,
            "overshoot recorded"
        );
        c.tick(Duration::from_millis(500), &mut cx);
        assert_eq!(
            ScrolledWindowC::overshoot_of(c.as_ref()).1,
            0.0,
            "sprung back"
        );
        assert_eq!(c.next_deadline(Duration::from_millis(500)), None);
    }

    #[test]
    fn a_kinetic_flick_decelerates_to_a_stop_within_a_bounded_number_of_ticks() {
        // Timing is a complexity bound, not a wall-clock pin: a flick must
        // stop, and must not stop instantly. Mutation check: a decay factor
        // of 1.0 never terminates and this test hits its 600-tick ceiling.
        let built = build_widget::<()>(Kind::ScrolledWindow, &props(false));
        let mut c = built.controller;
        let mut hx = Headless::new();
        let mut cx = hx.event_cx(&built.node);
        ScrolledWindowC::set_extent(c.as_mut(), (100.0, 10_000.0), (100.0, 100.0));
        c.on_event(&Event::Scroll(Headless::flick(0.0, 40.0)), &mut cx);
        let mut ticks = 0;
        let mut now = Duration::ZERO;
        while c.next_deadline(now).is_some() && ticks < 600 {
            now += Duration::from_millis(16);
            c.tick(now, &mut cx);
            ticks += 1;
        }
        assert!((2..600).contains(&ticks), "decelerated in {ticks} ticks");
    }

    #[test]
    fn hostile_extents_never_panic_and_never_produce_a_nan_offset() {
        // Mutation check: dividing by a zero page size to size the slider
        // yields NaN and every later comparison is false, freezing the bar.
        let built = build_widget::<()>(Kind::ScrolledWindow, &props(false));
        let mut c = built.controller;
        let mut hx = Headless::new();
        let mut cx = hx.event_cx(&built.node);
        for (content, page) in [
            ((0.0, 0.0), (0.0, 0.0)),
            ((f32::NAN, f32::INFINITY), (1.0, 1.0)),
            ((-5.0, 1e30), (1e30, -1.0)),
        ] {
            ScrolledWindowC::set_extent(c.as_mut(), content, page);
            c.on_event(&Event::Scroll(Headless::scroll(1e30, -1e30)), &mut cx);
            let (x, y) = ScrolledWindowC::offset_of(c.as_ref());
            assert!(x.is_finite() && y.is_finite(), "offset stayed finite");
        }
    }

    #[test]
    fn chrome_follows_the_application_child_in_gtks_declared_order() {
        // The order this module's doc block and
        // `fixtures/gtk4.22-node-trees/scrolled_window.txt` declare is
        // `<child>`, overshoot, undershoot, scrollbar.horizontal,
        // scrollbar.vertical, junction -- and the paint walker treats node
        // order as z-order, so it is not cosmetic.
        //
        // Mutation check: drop `restack_chrome`'s two call sites (chrome
        // appended in creation order, as this module shipped) and the
        // overshoot node lands *after* the junction, because it is only
        // created the first time a drag pulls past an end.
        let mut p = props(false);
        p.set(
            PropName::HscrollbarPolicy,
            Prop::Enum(Policy::Always.to_u16()),
        );
        p.set(
            PropName::VscrollbarPolicy,
            Prop::Enum(Policy::Always.to_u16()),
        );
        let built = build_widget::<()>(Kind::ScrolledWindow, &p);
        // What `reconcile` does with the one view child: it lands at index
        // 0, in front of the chrome `build` already attached.
        let child = crate::css::node::Node::new("viewport");
        built.node.insert_child(0, &child);

        let mut c = built.controller;
        let mut hx = Headless::new();
        let mut cx = hx.event_cx(&built.node);
        ScrolledWindowC::set_extent(c.as_mut(), (100.0, 200.0), (100.0, 100.0));
        c.on_event(&Event::Scroll(Headless::scroll(0.0, 500.0)), &mut cx);

        let order: Vec<String> = built
            .node
            .children()
            .iter()
            .map(|n| {
                let mut name = n.name().to_string();
                for class in n.classes() {
                    name.push('.');
                    name.push_str(class.as_str());
                }
                name
            })
            .collect();
        assert_eq!(
            order,
            vec![
                "viewport".to_owned(),
                "overshoot.bottom".to_owned(),
                "scrollbar.overlay-indicator.horizontal".to_owned(),
                "scrollbar.overlay-indicator.vertical".to_owned(),
                "junction".to_owned(),
            ]
        );
    }
}
