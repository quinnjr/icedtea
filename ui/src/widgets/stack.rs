//! `GtkStack` -- one visible page at a time, with an animated switch.
//!
//! Reconciliation: `Controller::build` runs *before* the reconciler attaches
//! a container's own view children onto its node (`overlay::OverlayC`'s own
//! module doc gives the same fact for the same reason -- `build_instance`
//! calls `build_controller` first and `reconcile_reserved` second), so a
//! freshly built `StackC` always finds `node.children()` empty: the task
//! text's `build` reads pages from there anyway, which would leave `pages`
//! permanently empty for every real `stack(pages)` view. This controller
//! therefore borrows `NotebookC`'s/`HeaderBarC`'s own fix for exactly this
//! timing gap -- `StackC::place`, run from [`Controller::reserved_total`]
//! (the one `&self` hook the reconciler calls once real children are
//! attached, on every reconcile of this level) -- which re-derives `pages`
//! from `node.children()` for real. That forces `pages`/`visible` into
//! `RefCell`/`Cell` rather than the plain fields the interface sketch
//! shows, the same deviation `NotebookC::tabs`'s own doc comment already
//! establishes for the identical constraint; `outgoing`/`progress`/
//! `transition`/`duration_ms`/`started` stay plain because only `&mut self`
//! methods ever write them. A `VisibleChild` requested before any page
//! exists (every fresh widget's very first frame) is queued in
//! `pending_visible` and honoured the first time `place` finds a non-empty
//! page list.
//!
//! Visibility: an inactive page is `display: none`, not merely marked with a
//! `hidden` class -- see `set_visible`, which records both. That is what
//! keeps a stack behaving like `GtkStack` for layout, paint *and* pointer
//! dispatch; the class on its own left every page laid out and hit-tested.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use crate::css::node::Node;
use crate::layout::{Align, ChildLayout, Container, GridPlacement};
use crate::view::{BuildCx, Controller, Event, EventCx, EventKind, Kind, Prop, PropName, Props};
use crate::widgets::types::{StackPageInfo, StackTransition};
use crate::widgets::{Universal, prop_i64, prop_str, prop_u16, props_of, set_container};

/// Add or remove the `hidden` marker class *and* fold the same decision into
/// the layout side table, so an inactive page is `display: none`.
///
/// The class alone is bookkeeping the fixtures read (it is what
/// [`super::notebook::NotebookC`]'s own private `set_visible` records). The
/// [`crate::widgets::set_displayed`] call beside it is what makes a
/// hidden page behave like `GtkStack`'s: taffy gives it `Display::None`, so
/// it takes no space, paints nothing and -- because `window::pointer`'s
/// `descend` skips a node with an empty border box -- receives no pointer
/// events.
///
/// Without it every page in the stack is laid out as an ordinary grid child.
/// `build` sets a `Container::Grid { columns: 1, rows: 1 }`, so pages past the
/// first land in taffy's *implicit* rows and the stack lays its pages out
/// sequentially, one below the next: later pages fall outside the window's
/// rendered height (they are allocated but never painted), and `descend`'s
/// reverse walk -- which `break`s on the first child whose border box contains
/// the point, whether or not a deeper target was found -- terminates inside
/// the wrong page, so no control inside the *active* page is ever reachable.
fn set_visible(node: &Node, visible: bool) {
    if visible {
        node.remove_class("hidden");
    } else {
        node.add_class("hidden");
    }
    crate::widgets::set_displayed(node, visible);
}

/// The single cell every page is pinned into -- `overlay::ONE_CELL`'s own
/// constant, for the same reason.
///
/// `build` gives the stack a `Container::Grid { columns: 1, rows: 1 }`, and a
/// child with no explicit placement auto-flows: for the 200 ms of a
/// transition *two* pages are displayed at once, so the outgoing one landed in
/// an implicit second row and was laid out (and hit-tested, ahead of the
/// incoming page under `window::pointer::descend`'s reverse walk) below it.
/// Pinned, no implicit row can ever appear however many pages are displayed.
const ONE_CELL: GridPlacement = GridPlacement {
    column: 0,
    row: 0,
    column_span: 1,
    row_span: 1,
};

/// Pin `node` into the stack's one grid cell, filling it.
///
/// This *replaces* whatever `ChildLayout` the page itself would have had:
/// a `GtkStack` page fills its stack, and an unpinned one auto-flows out of
/// the single cell altogether (see [`ONE_CELL`]). A caller that wants a
/// page's contents aligned puts the alignment on a child of the page.
fn pin(node: &Node) {
    crate::widgets::set_child_layout(
        node,
        ChildLayout {
            halign: Align::Fill,
            valign: Align::Fill,
            hexpand: true,
            vexpand: true,
            grid: Some(ONE_CELL),
            ..ChildLayout::default()
        },
    );
}

/// One page's identity and node.
pub struct StackPageState {
    /// Name, title, icon and attention flag -- what the switchers read.
    pub info: StackPageInfo,
    /// The page's own subtree root.
    pub node: Node,
}

fn page_state_from(child: &Node) -> StackPageState {
    let p = props_of(child);
    StackPageState {
        info: StackPageInfo {
            name: Rc::from(p.str(PropName::PageName).unwrap_or_default()),
            title: Rc::from(p.str(PropName::PageTitle).unwrap_or_default()),
            icon: None,
            needs_attention: p.bool(PropName::NeedsAttention, false),
        },
        node: child.clone(),
    }
}

/// `GtkStack`.
pub struct StackC {
    /// Index of the visible page. A `Cell`, not a plain field -- see the
    /// module doc.
    pub visible: Cell<usize>,
    /// The page still fading/sliding out, if any.
    pub outgoing: Option<usize>,
    /// `0.0`..`1.0` through the transition.
    pub progress: f32,
    /// Which transition.
    pub transition: StackTransition,
    /// Its duration.
    pub duration_ms: u32,
    /// The duration of the switch actually in flight: `duration_ms` for an
    /// animated one, `0` for a `StackTransition::None` swap, which finishes on
    /// the next tick (so `EventKind::Change` still fires) without ever
    /// displaying two pages at once.
    active_ms: u32,
    /// Every page. A `RefCell`, not a plain `Vec` -- see the module doc.
    pub pages: RefCell<Vec<StackPageState>>,
    started: Option<Duration>,
    /// This controller's own root node, so [`StackC::place`] (a `&self`
    /// method) can re-read its real children.
    node: Node,
    /// A `visible_child` name requested before any page existed yet.
    pending_visible: RefCell<Option<Rc<str>>>,
    universal: Universal,
}

impl StackC {
    /// The page list a `StackSwitcher`/`StackSidebar` renders.
    #[must_use]
    pub fn page_infos(&self) -> Rc<[StackPageInfo]> {
        self.pages.borrow().iter().map(|p| p.info.clone()).collect()
    }

    /// Re-derive `pages` from the real children the reconciler has by now
    /// attached onto `self.node`, and apply any `visible_child` that arrived
    /// before those children existed. See the module doc.
    fn place(&self) {
        let pages: Vec<StackPageState> = self.node.children().iter().map(page_state_from).collect();
        if pages.is_empty() {
            *self.pages.borrow_mut() = pages;
            return;
        }
        if let Some(index) = self
            .pending_visible
            .borrow_mut()
            .take()
            .and_then(|name| pages.iter().position(|p| p.info.name == name))
        {
            self.visible.set(index);
        }
        let visible = self.visible.get().min(pages.len() - 1);
        self.visible.set(visible);
        for (i, page) in pages.iter().enumerate() {
            pin(&page.node);
            set_visible(&page.node, i == visible || Some(i) == self.outgoing);
        }
        *self.pages.borrow_mut() = pages;
    }

    /// Test hook: the visible page's index.
    ///
    /// `Controller<Msg>: std::any::Any` (`view::controller`'s own doc
    /// comment) is what lets a `&dyn Controller<Msg>` coerce straight to
    /// `&dyn Any` at this call site -- trait-object upcasting, stable since
    /// the workspace's pinned Rust, needs no `as_any` method on the trait
    /// itself (`ExpanderC::progress_of`, `PanedC::position_of` and
    /// `NotebookC::checked_tabs` already rely on the same coercion; the
    /// trait has no `as_any` for these hooks to call, so the task text's
    /// `c.as_any()` does not exist and this takes `&dyn Controller<Msg>`
    /// directly instead).
    #[must_use]
    pub fn visible_of<Msg: 'static>(c: &dyn Controller<Msg>) -> usize {
        let any: &dyn std::any::Any = c;
        any.downcast_ref::<Self>()
            .map_or(usize::MAX, |m| m.visible.get())
    }
    /// Test hook: the outgoing page's index, while a transition runs.
    #[must_use]
    pub fn outgoing_of<Msg: 'static>(c: &dyn Controller<Msg>) -> Option<usize> {
        let any: &dyn std::any::Any = c;
        any.downcast_ref::<Self>().and_then(|m| m.outgoing)
    }
    /// Test hook: how far through the transition.
    #[must_use]
    pub fn progress_of<Msg: 'static>(c: &dyn Controller<Msg>) -> f32 {
        let any: &dyn std::any::Any = c;
        any.downcast_ref::<Self>().map_or(f32::NAN, |m| m.progress)
    }

    /// Switch to the page called `name`; unknown names change nothing.
    fn show(&mut self, name: &str, now: Duration) -> bool {
        let pages = self.pages.borrow();
        let Some(index) = pages.iter().position(|p| &*p.info.name == name) else {
            tracing::debug!(name, "stack: no page with that name");
            return false;
        };
        if index == self.visible.get() {
            return false;
        }
        // `StackTransition::None` is an instant swap, not a 200 ms one with
        // nothing drawn: arming the timer regardless left the page the user
        // just left displayed (and laid out, and hit-tested) for the whole
        // duration, which is what `GtkStack` does not do.
        let animated = self.transition != StackTransition::None && self.duration_ms > 0;
        self.outgoing = animated.then(|| self.visible.get());
        self.active_ms = if animated { self.duration_ms } else { 0 };
        self.visible.set(index);
        // An instant swap is complete the moment it is armed; only an animated
        // one starts at 0.0 and climbs (see the matching guard in `tick`).
        self.progress = if self.active_ms == 0 { 1.0 } else { 0.0 };
        self.started = Some(now);
        for (i, page) in pages.iter().enumerate() {
            set_visible(
                &page.node,
                i == self.visible.get() || Some(i) == self.outgoing,
            );
        }
        true
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for StackC {
    fn kind(&self) -> Kind {
        Kind::Stack
    }

    fn build(node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        set_container(
            node,
            Container::Grid {
                columns: 1,
                rows: 1,
                column_spacing: 0.0,
                row_spacing: 0.0,
                column_homogeneous: false,
                row_homogeneous: false,
            },
        );
        // Empty on every real `stack(pages)` view (see the module doc);
        // non-empty only when a caller (a test) attached children onto
        // `node` before this ran.
        let pages: Vec<StackPageState> = node.children().iter().map(page_state_from).collect();
        for (i, page) in pages.iter().enumerate() {
            pin(&page.node);
            set_visible(&page.node, i == 0);
        }
        let mut me = Self {
            visible: Cell::new(0),
            outgoing: None,
            progress: 1.0,
            transition: StackTransition::from_u16(
                u16::try_from(props.int(PropName::Transition, 0)).unwrap_or(0),
            ),
            duration_ms: u32::try_from(
                props
                    .int(PropName::TransitionDuration, 200)
                    .clamp(0, 10_000),
            )
            .unwrap_or(200),
            active_ms: 0,
            pages: RefCell::new(pages),
            started: None,
            node: node.clone(),
            pending_visible: RefCell::new(None),
            universal: Universal::new(node, Kind::Stack),
        };
        if let Some(name) = props.str(PropName::VisibleChild) {
            if me.pages.borrow().is_empty() {
                me.pending_visible = RefCell::new(Some(Rc::from(name)));
            } else if me.show(name, Duration::ZERO) {
                // The initial page is shown without animating.
                me.outgoing = None;
                me.started = None;
                me.progress = 1.0;
            }
        }
        me
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, cx: &mut BuildCx<'_>) {
        match name {
            PropName::VisibleChild => {
                self.show(prop_str(value), cx.clock.now());
            }
            PropName::Transition => self.transition = StackTransition::from_u16(prop_u16(value, 0)),
            PropName::TransitionDuration => {
                self.duration_ms =
                    u32::try_from(prop_i64(value, 200).clamp(0, 10_000)).unwrap_or(200);
            }
            other => {
                self.universal.apply(node, Kind::Stack, other, value);
            }
        }
    }

    fn on_event(&mut self, _ev: &Event, _cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        Vec::new()
    }

    fn tick(&mut self, now: Duration, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        let Some(started) = self.started else {
            return Vec::new();
        };
        // An instant swap (`StackTransition::None`, `active_ms == 0`) is
        // already complete: force `progress` to 1.0 so the completion branch
        // below fires on this first tick even under a non-advancing clock,
        // rather than reading 0.0 from `(now - started) * 1000 / 1` when the
        // clock has not moved since `show`.
        self.progress = if self.active_ms == 0 {
            1.0
        } else {
            let d = f32::from(u16::try_from(self.active_ms).unwrap_or(u16::MAX)).max(1.0);
            (now.saturating_sub(started).as_secs_f32() * 1000.0 / d).clamp(0.0, 1.0)
        };
        let visible = self.visible.get();
        {
            let pages = self.pages.borrow();
            crate::widgets::set_transition_progress(
                &pages[visible].node,
                self.transition,
                self.progress,
            );
        }
        if self.progress >= 1.0 {
            let pages = self.pages.borrow();
            if let Some(old) = self.outgoing.take() {
                set_visible(&pages[old].node, false);
            }
            self.started = None;
            let name = Rc::clone(&pages[visible].info.name);
            drop(pages);
            return cx
                .handlers
                .fire_text(EventKind::Change, &name)
                .into_iter()
                .collect();
        }
        Vec::new()
    }

    fn next_deadline(&self, now: Duration) -> Option<Duration> {
        self.started.map(|s| {
            let end = s + Duration::from_millis(u64::from(self.active_ms));
            if now >= end {
                Duration::ZERO
            } else {
                end - now
            }
        })
    }

    fn reserved_total(&self, view_count: usize) -> usize {
        self.place();
        view_count
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::view::{Kind, Prop, PropName, Props};
    use crate::widgets::stack::StackC;
    use crate::widgets::types::StackTransition;
    use crate::widgets::{BuiltWidget, Headless, build_widget, matches_fixture};

    fn props() -> Props {
        let mut p = Props::default();
        p.set(PropName::VisibleChild, Prop::Str("one".into()));
        p.set(
            PropName::Transition,
            Prop::Enum(StackTransition::Crossfade.to_u16()),
        );
        p.set(PropName::TransitionDuration, Prop::Int(200));
        p
    }

    /// Build a `Kind::Stack` with `names` already attached as its children,
    /// each recorded with a `PageName` -- `build_widget` alone cannot do
    /// this (it builds a bare, childless node -- see this module's doc
    /// comment), so this reconciliation calls `build_controller` directly
    /// over a node this helper populates first, the same order a real
    /// reconciler's `Insert` op plus its later `reserved_total`-driven
    /// `place()` produces, collapsed into one step since nothing here needs
    /// a second reconcile pass to observe it.
    fn build_with_pages<const N: usize>(names: [&str; N]) -> BuiltWidget<()> {
        let node = crate::css::node::Node::with_classes(
            Kind::Stack.css_name(),
            Kind::Stack.base_classes(),
        );
        for name in names {
            let child = crate::css::node::Node::new(Kind::StackPage.css_name());
            let mut child_props = Props::default();
            child_props.set(PropName::PageName, Prop::Str(name.into()));
            crate::widgets::record_props(&child, &child_props);
            node.append_child(&child);
        }
        let mut hx = Headless::new();
        let controller = {
            let mut cx = hx.cx();
            crate::widgets::build_controller::<()>(Kind::Stack, &node, &props(), &mut cx)
        };
        BuiltWidget { node, controller }
    }

    #[test]
    fn a_stack_is_a_single_node_named_stack() {
        // Mutation check: wrapping the pages in a `box` fails the fixture,
        // which GTK gives as one node (gtk/gtkstack.c).
        let built = build_widget::<()>(Kind::Stack, &props());
        matches_fixture(&built.node, "stack\n").expect("stack fixture");
    }

    #[test]
    fn switching_pages_runs_the_transition_on_the_clock_and_then_stops() {
        // Interaction test. Mutation check: leaving `outgoing` set after the
        // transition keeps the old page painted forever; a next_deadline
        // that never returns None spins the pump.
        let built = build_with_pages(["one", "two"]);
        let mut c = built.controller;
        let mut hx = Headless::new();
        c.set_prop(
            &built.node,
            PropName::VisibleChild,
            &Prop::Str("two".into()),
            &mut hx.cx(),
        );
        assert!(c.next_deadline(Duration::ZERO).is_some());
        assert_eq!(StackC::outgoing_of(&*c), Some(0));
        let mut cx = hx.event_cx(&built.node);
        c.tick(Duration::from_millis(200), &mut cx);
        assert_eq!(StackC::progress_of(&*c), 1.0);
        assert_eq!(StackC::outgoing_of(&*c), None);
        assert_eq!(c.next_deadline(Duration::from_millis(200)), None);
    }

    /// An inactive page is `display: none`, so taffy gives it no box at all.
    ///
    /// That is the whole reason a control inside the active page is
    /// reachable: `build` puts every page in a `Grid { columns: 1, rows: 1 }`,
    /// so without this the pages past the first land in taffy's *implicit*
    /// rows and lay out sequentially below one another -- painted off the
    /// bottom of the window, and hit-tested ahead of the active page by
    /// `window::pointer::descend`'s reverse walk.
    ///
    /// Mutation check: drop the `set_displayed` call from `set_visible` and
    /// page "two" lays out at its full size right below page "one" (its
    /// `border_box.height` is non-zero and its `y` is page one's height),
    /// which is exactly the defect this asserts against. Restore.
    #[test]
    fn an_inactive_page_takes_no_space() {
        use crate::layout::{FixedMeasure, LayoutTree};
        use taffy::style::AvailableSpace;

        let built = build_with_pages(["one", "two"]);
        // `reserved_total` is the hook the reconciler calls once the real
        // children are attached; it runs `place()`, which is what records
        // each page's visibility.
        assert_eq!(built.controller.reserved_total(2), 2);
        assert_eq!(
            StackC::visible_of(&*built.controller),
            0,
            "\"one\" is shown"
        );

        let mut tree = LayoutTree::new();
        tree.sync(&built.node).expect("sync");
        crate::widgets::flush_layout(&mut tree);
        tree.compute(
            &built.node,
            taffy::Size {
                width: AvailableSpace::Definite(200.0),
                height: AvailableSpace::Definite(200.0),
            },
            &mut FixedMeasure(taffy::Size {
                width: 40.0,
                height: 20.0,
            }),
        )
        .expect("compute");

        let pages = built.node.children();
        assert!(
            tree.is_displayed(&pages[0]),
            "the visible page is displayed"
        );
        assert!(
            !tree.is_displayed(&pages[1]),
            "the inactive page is display:none"
        );
        let hidden = tree.allocation(&pages[1]).expect("an allocation");
        assert_eq!(
            (hidden.border_box.width, hidden.border_box.height),
            (0.0, 0.0),
            "an inactive page occupies no area, so no pointer can hit it"
        );
    }

    /// A `StackTransition::None` switch is instant: nothing is left displayed
    /// behind, so nothing stale can be laid out or hit-tested.
    ///
    /// Mutation check: arm the transition unconditionally again (drop
    /// `show`'s `animated` guard) and `outgoing_of` is `Some(0)` here, with
    /// page "one" still displayed for the whole 200 ms. Restore.
    #[test]
    fn a_none_transition_swaps_without_leaving_the_old_page_displayed() {
        use crate::layout::{FixedMeasure, LayoutTree};
        use taffy::style::AvailableSpace;

        let built = build_with_pages(["one", "two"]);
        let mut c = built.controller;
        assert_eq!(c.reserved_total(2), 2);
        let mut hx = Headless::new();
        c.set_prop(
            &built.node,
            PropName::Transition,
            &Prop::Enum(StackTransition::None.to_u16()),
            &mut hx.cx(),
        );
        c.set_prop(
            &built.node,
            PropName::VisibleChild,
            &Prop::Str("two".into()),
            &mut hx.cx(),
        );

        assert_eq!(StackC::visible_of(&*c), 1);
        assert_eq!(
            StackC::outgoing_of(&*c),
            None,
            "no page is left fading out of a transition that does not animate"
        );

        let mut tree = LayoutTree::new();
        tree.sync(&built.node).expect("sync");
        crate::widgets::flush_layout(&mut tree);
        tree.compute(
            &built.node,
            taffy::Size {
                width: AvailableSpace::Definite(200.0),
                height: AvailableSpace::Definite(200.0),
            },
            &mut FixedMeasure(taffy::Size {
                width: 40.0,
                height: 20.0,
            }),
        )
        .expect("compute");
        let pages = built.node.children();
        assert!(!tree.is_displayed(&pages[0]), "the page left is hidden now");
        assert!(tree.is_displayed(&pages[1]));
    }

    /// Every page is pinned into the stack's single grid cell, so even while
    /// two are displayed at once (mid-crossfade) neither auto-flows into an
    /// implicit second row below the other.
    ///
    /// Mutation check: drop `place`'s/`build`'s `pin` call and page "two"
    /// lands a row below page "one" -- exactly the geometry that made
    /// `window::pointer::descend`'s reverse walk terminate inside the stale
    /// page. Restore.
    #[test]
    fn both_pages_of_a_running_transition_share_the_one_cell() {
        use crate::layout::{FixedMeasure, LayoutTree};
        use taffy::style::AvailableSpace;

        let built = build_with_pages(["one", "two"]);
        let mut c = built.controller;
        assert_eq!(c.reserved_total(2), 2);
        let mut hx = Headless::new();
        c.set_prop(
            &built.node,
            PropName::VisibleChild,
            &Prop::Str("two".into()),
            &mut hx.cx(),
        );
        assert_eq!(
            StackC::outgoing_of(&*c),
            Some(0),
            "a crossfade really is animating"
        );

        let mut tree = LayoutTree::new();
        tree.sync(&built.node).expect("sync");
        crate::widgets::flush_layout(&mut tree);
        tree.compute(
            &built.node,
            taffy::Size {
                width: AvailableSpace::Definite(200.0),
                height: AvailableSpace::Definite(200.0),
            },
            &mut FixedMeasure(taffy::Size {
                width: 40.0,
                height: 20.0,
            }),
        )
        .expect("compute");
        let pages = built.node.children();
        let one = tree.allocation(&pages[0]).expect("one").border_box;
        let two = tree.allocation(&pages[1]).expect("two").border_box;
        assert!(one.height > 0.0 && two.height > 0.0, "both are displayed");
        assert_eq!(
            (one.x, one.y),
            (two.x, two.y),
            "both pages occupy the same single cell"
        );
    }

    #[test]
    fn an_unknown_page_name_leaves_the_visible_page_alone() {
        // Untrusted input: the name comes from the application model.
        // Mutation check: `unwrap_or(0)` on the lookup silently jumps to the
        // first page whenever a model has a typo -- pages are ordered so
        // "one" (`props()`'s initial `visible_child`) is not at index 0,
        // which such a mutant would jump to.
        let built = build_with_pages(["decoy", "one"]);
        let mut c = built.controller;
        let mut hx = Headless::new();
        let before = StackC::visible_of(&*c);
        for name in ["", "nope", "\u{0}", &"x".repeat(10_000)] {
            c.set_prop(
                &built.node,
                PropName::VisibleChild,
                &Prop::Str(name.into()),
                &mut hx.cx(),
            );
        }
        assert_eq!(StackC::visible_of(&*c), before);
    }
}
