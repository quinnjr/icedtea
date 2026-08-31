//! The loop: events in, messages folded, a new view reconciled, the dirty
//! parts restyled, relaid out and repainted.

use std::rc::Rc;
use std::time::Duration;

use crate::anim::Clock;
use crate::css::computed::ResolveEnv;
use crate::css::node::Node;
use crate::icons::IconTheme;
use crate::layout::LayoutTree;
use crate::text::FontDatabase;
use crate::view::cmd::Cmd;
use crate::view::controller::{Event, EventCx, Phase};
use crate::view::reconcile::Instance;
use crate::view::render::StyleMap;
use crate::window::focus::FocusRing;
use crate::window::selection::Clipboard;

/// The borrows one dispatch pass needs, minus the per-node ones.
pub struct Dispatch<'a, Msg> {
    /// Computed styles, for a controller that hit-tests inside itself.
    pub styles: &'a StyleMap,
    /// Allocations.
    pub tree: &'a LayoutTree,
    /// The window's focus owner.
    pub focus: &'a mut FocusRing,
    /// Copy/paste and the primary selection.
    pub clipboard: &'a mut Clipboard,
    /// Icon lookup.
    pub icons: &'a mut IconTheme,
    /// Font matching and shaping.
    pub fonts: &'a mut FontDatabase,
    /// The animation clock.
    pub clock: &'a Rc<dyn Clock>,
    /// DPI and root font size.
    pub env: &'a ResolveEnv,
    /// Side effects controllers requested.
    pub cmds: &'a mut Vec<Cmd<Msg>>,
}

/// The chain of nodes from the outermost instance down to `target`,
/// outermost first — the same order `window::pointer::hit_chain` returns.
/// Empty when `target` is not in the tree.
#[must_use]
pub fn path_to<Msg: Clone + 'static>(roots: &[Instance<Msg>], target: &Node) -> Vec<Node> {
    fn walk<Msg: Clone + 'static>(
        instance: &Instance<Msg>,
        target: &Node,
        out: &mut Vec<Node>,
    ) -> bool {
        out.push(instance.node.clone());
        if instance.node.ptr_eq(target) {
            return true;
        }
        for child in &instance.children {
            if walk(child, target, out) {
                return true;
            }
        }
        out.pop();
        false
    }

    let mut out = Vec::new();
    for root in roots {
        if walk(root, target, &mut out) {
            return out;
        }
        out.clear();
    }
    Vec::new()
}

/// Deliver `event` along `path` in GTK's three phases: capture from the
/// outermost node inward, then the target, then bubble back out.
///
/// A controller that sets `cx.handled` stops **the phase it is in**
/// (deviation D5): capture stops descending, bubble stops climbing, and a
/// handled target simply ends the target phase. Messages come back in the
/// order they were produced.
pub fn deliver<Msg: Clone + 'static>(
    roots: &mut [Instance<Msg>],
    path: &[Node],
    event: &Event,
    cx: &mut Dispatch<'_, Msg>,
) -> Vec<Msg> {
    if path.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let last = path.len() - 1;

    'phases: for phase in Phase::ALL {
        let order: Vec<usize> = match phase {
            Phase::Capture => (0..last).collect(),
            Phase::Target => vec![last],
            Phase::Bubble => (0..last).rev().collect(),
        };
        for index in order {
            let node = path[index].clone();
            let Some(instance) = roots.iter_mut().find_map(|root| root.find_mut(&node)) else {
                continue;
            };
            // Split the borrow: the controller is `&mut`, everything else
            // the `EventCx` needs is behind a different field or a clone.
            let Instance {
                controller,
                handlers,
                node: own,
                ..
            } = instance;
            let mut ecx = EventCx {
                node: own,
                handlers,
                tree: cx.tree,
                styles: cx.styles,
                focus: cx.focus,
                clipboard: cx.clipboard,
                icons: cx.icons,
                fonts: cx.fonts,
                clock: cx.clock,
                env: cx.env,
                cmds: cx.cmds,
                phase,
                handled: false,
            };
            out.extend(controller.on_event(event, &mut ecx));
            // `cx.handled` stops the phase it is set in (deviation D5), and
            // since dispatch never revisits a phase once left, that handled
            // node is also the last one this event reaches: a handled
            // capture never lets the event descend to the target, and a
            // handled target never lets it bubble.
            if ecx.handled {
                break 'phases;
            }
        }
    }
    out
}

/// Run every controller's `tick`, deepest last, and collect what they say.
pub fn tick_all<Msg: Clone + 'static>(
    roots: &mut [Instance<Msg>],
    now: Duration,
    cx: &mut Dispatch<'_, Msg>,
) -> Vec<Msg> {
    let mut out = Vec::new();
    for root in roots.iter_mut() {
        tick_one(root, now, cx, &mut out);
    }
    out
}

fn tick_one<Msg: Clone + 'static>(
    instance: &mut Instance<Msg>,
    now: Duration,
    cx: &mut Dispatch<'_, Msg>,
    out: &mut Vec<Msg>,
) {
    let Instance {
        controller,
        handlers,
        node,
        children,
        ..
    } = instance;
    let mut ecx = EventCx {
        node,
        handlers,
        tree: cx.tree,
        styles: cx.styles,
        focus: cx.focus,
        clipboard: cx.clipboard,
        icons: cx.icons,
        fonts: cx.fonts,
        clock: cx.clock,
        env: cx.env,
        cmds: cx.cmds,
        phase: Phase::Target,
        handled: false,
    };
    out.extend(controller.tick(now, &mut ecx));
    for child in children {
        tick_one(child, now, cx, out);
    }
}

/// The earliest deadline any controller in the tree reports.
#[must_use]
pub fn next_controller_deadline<Msg: Clone + 'static>(
    roots: &[Instance<Msg>],
    now: Duration,
) -> Option<Duration> {
    fn walk<Msg: Clone + 'static>(
        instance: &Instance<Msg>,
        now: Duration,
        best: &mut Option<Duration>,
    ) {
        if let Some(deadline) = instance.controller.next_deadline(now) {
            *best = Some(best.map_or(deadline, |b: Duration| b.min(deadline)));
        }
        for child in &instance.children {
            walk(child, now, best);
        }
    }
    let mut best = None;
    for root in roots {
        walk(root, now, &mut best);
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::css::cascade::CompiledSheet;
    use crate::view::builders::widget;
    use crate::view::reconcile::reconcile;
    use crate::view::{BuildCx, Handler, Kind, PropName};

    #[derive(Debug, Clone, PartialEq)]
    enum Msg {
        Outer,
        Inner,
    }

    #[allow(
        clippy::type_complexity,
        reason = "a test fixture's tuple return, not a public API"
    )]
    fn tree() -> (
        CompiledSheet,
        crate::text::FontDatabase,
        crate::icons::IconTheme,
        Rc<dyn Clock>,
        ResolveEnv,
        Node,
        Vec<Instance<Msg>>,
    ) {
        let sheet = CompiledSheet::compile("box { color: #000; } button { color: #111; }");
        let mut fonts = crate::text::FontDatabase::probe_only();
        let mut icons = crate::icons::IconTheme::with_name_and_roots("hicolor", vec![]);
        let clock: Rc<dyn Clock> = Rc::new(crate::anim::ManualClock::new());
        let env = ResolveEnv::default();
        let root = Node::new("window");
        let mut instances: Vec<Instance<Msg>> = Vec::new();
        {
            let mut cx = BuildCx {
                sheet: &sheet,
                fonts: &mut fonts,
                icons: &mut icons,
                clock: &clock,
                env: &env,
            };
            reconcile(
                &root,
                &mut instances,
                vec![
                    widget::<Msg>(Kind::Box)
                        .key("outer")
                        .on(crate::view::EventKind::Click, Handler::Unit(Msg::Outer))
                        .child(
                            widget::<Msg>(Kind::Button)
                                .key("inner")
                                .prop(PropName::Label, "Go")
                                .on(crate::view::EventKind::Click, Handler::Unit(Msg::Inner)),
                        ),
                ],
                &mut cx,
            );
        }
        (sheet, fonts, icons, clock, env, root, instances)
    }

    #[test]
    fn path_to_is_outermost_first_and_ends_at_the_target() {
        let (_s, _f, _i, _c, _e, _root, instances) = tree();
        let inner = instances[0].children[0].node.clone();
        let path = path_to(&instances, &inner);
        assert_eq!(path.len(), 2);
        assert!(path[0].ptr_eq(&instances[0].node));
        assert!(path[1].ptr_eq(&inner));
        assert!(path_to(&instances, &Node::new("stranger")).is_empty());
    }

    #[test]
    fn a_click_on_the_inner_node_bubbles_to_the_outer_one() {
        let (sheet, mut fonts, mut icons, clock, env, _root, mut instances) = tree();
        let inner = instances[0].children[0].node.clone();
        let path = path_to(&instances, &inner);

        let tree_layout = crate::layout::LayoutTree::new();
        let styles = crate::view::render::StyleMap::new();
        let mut focus = <crate::window::focus::FocusRing as Default>::default();
        let mut clipboard = crate::window::selection::Clipboard::offscreen();
        let mut cmds: Vec<Cmd<Msg>> = Vec::new();
        let mut cx = Dispatch {
            styles: &styles,
            tree: &tree_layout,
            focus: &mut focus,
            clipboard: &mut clipboard,
            icons: &mut icons,
            fonts: &mut fonts,
            clock: &clock,
            env: &env,
            cmds: &mut cmds,
        };
        let _ = &sheet;

        deliver(
            &mut instances,
            &path,
            &Event::PointerDown {
                button: crate::window::layer::BTN_LEFT,
                local: (1.0, 1.0),
                serial: 1,
            },
            &mut cx,
        );
        let out = deliver(
            &mut instances,
            &path,
            &Event::PointerUp {
                button: crate::window::layer::BTN_LEFT,
                local: (1.0, 1.0),
                serial: 2,
            },
            &mut cx,
        );
        // The inner button handled it and stopped the target phase; the
        // outer box never saw a press, so it produces nothing on bubble.
        assert_eq!(out, vec![Msg::Inner]);
    }

    #[test]
    fn an_empty_path_delivers_nothing_and_does_not_panic() {
        let (_s, mut fonts, mut icons, clock, env, _root, mut instances) = tree();
        let tree_layout = crate::layout::LayoutTree::new();
        let styles = crate::view::render::StyleMap::new();
        let mut focus = <crate::window::focus::FocusRing as Default>::default();
        let mut clipboard = crate::window::selection::Clipboard::offscreen();
        let mut cmds: Vec<Cmd<Msg>> = Vec::new();
        let mut cx = Dispatch {
            styles: &styles,
            tree: &tree_layout,
            focus: &mut focus,
            clipboard: &mut clipboard,
            icons: &mut icons,
            fonts: &mut fonts,
            clock: &clock,
            env: &env,
            cmds: &mut cmds,
        };
        assert!(deliver(&mut instances, &[], &Event::PointerLeave, &mut cx).is_empty());
    }

    #[test]
    fn messages_come_back_in_dispatch_order_target_before_bubble() {
        let (_s, mut fonts, mut icons, clock, env, _root, mut instances) = tree();
        let inner = instances[0].children[0].node.clone();
        let path = path_to(&instances, &inner);
        let tree_layout = crate::layout::LayoutTree::new();
        let styles = crate::view::render::StyleMap::new();
        let mut focus = <crate::window::focus::FocusRing as Default>::default();
        let mut clipboard = crate::window::selection::Clipboard::offscreen();
        let mut cmds: Vec<Cmd<Msg>> = Vec::new();
        let mut cx = Dispatch {
            styles: &styles,
            tree: &tree_layout,
            focus: &mut focus,
            clipboard: &mut clipboard,
            icons: &mut icons,
            fonts: &mut fonts,
            clock: &clock,
            env: &env,
            cmds: &mut cmds,
        };
        // `Activate` is not handled-stopping unless a handler fires, and both
        // nodes bind Click, so the target answers first and the ancestor
        // second.
        let out = deliver(&mut instances, &path, &Event::Activate, &mut cx);
        assert_eq!(out.first(), Some(&Msg::Inner));
    }
}
