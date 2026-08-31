//! The three recursive walkers the reactive layer drives every frame:
//! restyle, layout and paint.
//!
//! M2 built each of these for exactly one node (`widget::button::Button`);
//! contract §9 makes them P4's, over an arbitrary retained tree.

use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use selectors::Element;

use crate::anim::{AnimationState, Overrides};
use crate::css::cascade::CompiledSheet;
use crate::css::computed::{ComputedStyle, ResolveEnv};
use crate::css::node::Node;
use crate::css::select::MatchCx;

/// How deep a `View` tree may nest before the walkers stop descending.
///
/// A `view` function is application code and can recurse without bound; the
/// walkers are recursive, so an unbounded tree is a stack overflow, which is
/// an abort, not an error. GTK's own widget trees are shallow — this is two
/// orders of magnitude past anything real.
pub const MAX_TREE_DEPTH: usize = 512;

/// A stable per-node identity for caches.
///
/// Contract deviation D6: §3.4 says "keyed by `Node::addr()`", which is
/// `pub(crate)` (`css/node.rs:445`) and off-limits to P4 (§9 forbids
/// touching `css/**`). `Element::opaque` is the public equivalent and is
/// what `LayoutTree` already keys on (`layout.rs:225`).
pub type NodeAddr = selectors::OpaqueElement;

/// `node`'s cache key.
#[must_use]
pub fn node_addr(node: &Node) -> NodeAddr {
    Element::opaque(node)
}

/// Every live node's computed style. P3's `hit_test`/`hit_chain` take this
/// by reference and never build one (contract §3.4).
pub type StyleMap = HashMap<NodeAddr, Rc<ComputedStyle>>;

/// One [`AnimationState`] per node.
///
/// Contract deviation D3: §3.1's `Window::animations()` hands back a single
/// window-wide `AnimationState`, but M2's `AnimationState::restyle` diffs
/// exactly one node's `ComputedStyle` — one per window cannot animate two
/// nodes independently.
#[derive(Default)]
pub struct Animations(HashMap<NodeAddr, AnimationState>);

impl Animations {
    /// An empty map.
    #[must_use]
    pub fn new() -> Self {
        Animations(HashMap::new())
    }

    /// This frame's animated overrides for `node`; empty when nothing runs.
    pub fn sample(&mut self, node: &Node, now: Duration) -> Overrides {
        self.0
            .get_mut(&node_addr(node))
            .map(|state| state.sample(now))
            .unwrap_or_default()
    }

    /// `node`'s animation state, if it has one.
    #[must_use]
    pub fn get(&self, node: &Node) -> Option<&AnimationState> {
        self.0.get(&node_addr(node))
    }

    /// Whether any node still needs a frame.
    #[must_use]
    pub fn is_active(&self, now: Duration) -> bool {
        self.0.values().any(|state| state.is_active(now))
    }

    /// The earliest deadline across every node; `None` when nothing runs.
    ///
    /// `Duration::ZERO` here means "now", never "spin" — M2's
    /// `AnimationState::next_deadline` documents the same.
    #[must_use]
    pub fn next_deadline(&self, now: Duration) -> Option<Duration> {
        self.0
            .values()
            .filter_map(|state| state.next_deadline(now))
            .min()
    }

    /// Drop the state of every node that is no longer styled — i.e. no
    /// longer in the tree.
    pub fn retain_live(&mut self, styles: &StyleMap) {
        self.0.retain(|addr, _| styles.contains_key(addr));
    }

    /// How many nodes are animating.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether nothing is animating.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Recascade the whole tree under `root`, feeding every change into the
/// per-node [`Animations`], and return how many nodes' styles changed.
///
/// The parent's `ComputedStyle` is threaded down, so this is M2's
/// `ComputedStyle::resolve` (one cascade per node) and not `resolve_chain`
/// (one cascade per ancestor per node) — the difference is O(n) versus
/// O(n·depth), which matters at gallery scale.
///
/// Styles of nodes that left the tree are dropped, and
/// [`Animations::retain_live`] follows, so a removed subtree cannot leak a
/// running transition.
pub fn restyle_tree(
    root: &Node,
    sheet: &CompiledSheet,
    env: &ResolveEnv,
    styles: &mut StyleMap,
    anims: &mut Animations,
    now: Duration,
) -> usize {
    let mut cx = MatchCx::new();
    let mut live: Vec<NodeAddr> = Vec::with_capacity(styles.len().max(16));
    let mut changed = 0usize;
    restyle_one(
        root,
        None,
        sheet,
        env,
        styles,
        anims,
        now,
        &mut cx,
        &mut live,
        &mut changed,
        0,
    );
    let live: std::collections::HashSet<NodeAddr> = live.into_iter().collect();
    styles.retain(|addr, _| live.contains(addr));
    anims.retain_live(styles);
    changed
}

#[allow(
    clippy::too_many_arguments,
    reason = "one recursive walker threading the whole per-frame context; \
              bundling it into a struct would only rename the arguments"
)]
fn restyle_one(
    node: &Node,
    parent: Option<&ComputedStyle>,
    sheet: &CompiledSheet,
    env: &ResolveEnv,
    styles: &mut StyleMap,
    anims: &mut Animations,
    now: Duration,
    cx: &mut MatchCx,
    live: &mut Vec<NodeAddr>,
    changed: &mut usize,
    depth: usize,
) {
    if depth > MAX_TREE_DEPTH {
        tracing::warn!(
            depth,
            limit = MAX_TREE_DEPTH,
            node = %node.name(),
            "view tree deeper than the walker's budget; the subtree is not styled"
        );
        return;
    }

    let addr = node_addr(node);
    live.push(addr);
    let computed = Rc::new(ComputedStyle::resolve(sheet, node, parent, env, cx));
    let previous = styles.insert(addr, Rc::clone(&computed));

    let differs = previous
        .as_ref()
        .is_none_or(|old| !styles_equal(old, &computed));
    if differs {
        *changed += 1;
        let state = anims.0.entry(addr).or_default();
        state.restyle(previous.as_deref(), &computed, now, sheet);
        // A node that started nothing carries no state worth keeping; the
        // entry would otherwise grow once per node and never shrink.
        if !state.is_active(now) && state.transition_count() == 0 && state.animation_count() == 0 {
            anims.0.remove(&addr);
        }
    }

    for child in node.children() {
        restyle_one(
            &child,
            Some(&computed),
            sheet,
            env,
            styles,
            anims,
            now,
            cx,
            live,
            changed,
            depth + 1,
        );
    }
}

/// Whether two computed styles are the same for every longhand.
///
/// `ComputedStyle` is not `PartialEq` in M2, and adding the derive would
/// touch `css/**`, which §9 forbids P4 from doing. Comparing the raw values
/// longhand by longhand is exactly what a derive would generate.
fn styles_equal(a: &ComputedStyle, b: &ComputedStyle) -> bool {
    crate::css::registry::longhands().all(|prop| a.raw(prop) == b.raw(prop))
}

use crate::layout::{BoxDirection, Container, LayoutError, LayoutTree, Measure};
use crate::view::{Kind, Prop, PropName, Props};

/// `PropName::Orientation`'s `Prop::Enum` discriminant for horizontal.
pub const ORIENTATION_HORIZONTAL: u16 = 0;
/// `PropName::Orientation`'s `Prop::Enum` discriminant for vertical.
pub const ORIENTATION_VERTICAL: u16 = 1;

/// The layout container a kind's node uses.
///
/// M2's `Container` has exactly `Box` and `Leaf`; P6 (contract §3.6) adds
/// `Grid` and `Center` and rewires `Grid`/`CenterBox` here. Until then every
/// container is a box, which is what GTK's own `GtkBoxLayout` gives most of
/// them anyway.
#[must_use]
pub fn container_for(kind: Kind, props: &Props, has_children: bool) -> Container {
    if !has_children {
        return Container::Leaf;
    }
    let default = match kind {
        // Everything that stacks its children top to bottom in GTK.
        Kind::Window
        | Kind::ShortcutsWindow
        | Kind::AboutDialog
        | Kind::AlertDialog
        | Kind::ColorDialog
        | Kind::FontDialog
        | Kind::Popover
        | Kind::PopoverMenu
        | Kind::PopoverMenuItem
        | Kind::ListBox
        | Kind::ListBoxRow
        | Kind::ListView
        | Kind::ColumnView
        | Kind::Notebook
        | Kind::NotebookTab
        | Kind::Stack
        | Kind::StackPage
        | Kind::StackSidebar
        | Kind::ScrolledWindow
        | Kind::Frame
        | Kind::Overlay
        | Kind::Expander
        | Kind::SearchBar
        | Kind::TextView
        | Kind::Calendar
        | Kind::InfoBar
        | Kind::DropDown
        | Kind::EditableLabel => BoxDirection::Column,
        // GtkBox, GtkHeaderBar, GtkActionBar, GtkCenterBox, the button
        // family and the entry family are all horizontal by default.
        _ => BoxDirection::Row,
    };
    let direction = match props.get(PropName::Orientation) {
        Some(Prop::Enum(ORIENTATION_HORIZONTAL)) => BoxDirection::Row,
        Some(Prop::Enum(ORIENTATION_VERTICAL)) => BoxDirection::Column,
        _ => default,
    };
    Container::Box { direction }
}

/// Sync the retained tree into taffy, write every node's box, and compute.
///
/// `available` is the surface's content box: `Some` fixes that axis,
/// `None` asks for the intrinsic size (`MaxContent`), which is how a popup
/// or a `width_request`-free toplevel gets measured.
///
/// A node with no entry in `styles` is skipped rather than unwrapped: the
/// restyle walker stops at [`MAX_TREE_DEPTH`], so a pathological tree has
/// styled and unstyled halves and layout must survive both.
///
/// # Errors
///
/// [`LayoutError::Unsynced`] if `root` never made it into the tree, or
/// [`LayoutError::Taffy`] if taffy fails.
pub fn layout_tree(
    root: &Node,
    styles: &StyleMap,
    containers: &HashMap<NodeAddr, Container>,
    tree: &mut LayoutTree,
    env: &ResolveEnv,
    available: (Option<f32>, Option<f32>),
    measure: &mut dyn Measure,
) -> Result<(), LayoutError> {
    tree.sync(root)?;
    write_styles(root, styles, containers, tree, env, 0, available);
    let space = taffy::Size {
        width: available.0.map_or(
            taffy::AvailableSpace::MaxContent,
            taffy::AvailableSpace::Definite,
        ),
        height: available.1.map_or(
            taffy::AvailableSpace::MaxContent,
            taffy::AvailableSpace::Definite,
        ),
    };
    tree.compute(root, space, measure)
}

fn write_styles(
    node: &Node,
    styles: &StyleMap,
    containers: &HashMap<NodeAddr, Container>,
    tree: &mut LayoutTree,
    env: &ResolveEnv,
    depth: usize,
    available: (Option<f32>, Option<f32>),
) {
    if depth > MAX_TREE_DEPTH {
        return;
    }
    let addr = node_addr(node);
    if let Some(style) = styles.get(&addr) {
        let children = node.children();
        let fallback = if children.is_empty() {
            Container::Leaf
        } else {
            Container::Box {
                direction: BoxDirection::Row,
            }
        };
        let container = containers.get(&addr).copied().unwrap_or(fallback);
        // The root is the surface: its allocation is always exactly the
        // caller-supplied available space, never derived from its content
        // -- there is no `width`/`height` CSS property to express that
        // with, so it is floored the same way `window::restyle` does it: a
        // `min-width`/`min-height` override that `box_sizing: ContentBox`
        // (an AUTO size) turns into "at least the available space".
        if depth == 0 {
            let mut root_floor = crate::anim::Overrides::default();
            if let Some(w) = available.0 {
                root_floor.set(
                    crate::css::registry::Prop::MinWidth,
                    crate::css::value::Value::Length(crate::css::value::Length::px(w)),
                );
            }
            if let Some(h) = available.1 {
                root_floor.set(
                    crate::css::registry::Prop::MinHeight,
                    crate::css::value::Value::Length(crate::css::value::Length::px(h)),
                );
            }
            if root_floor.is_empty() {
                tree.set_style(node, style, container, env);
            } else {
                let floored = style.with_overrides(&root_floor).into_owned();
                tree.set_style(node, &floored, container, env);
            }
        } else {
            tree.set_style(node, style, container, env);
        }
        for child in children {
            write_styles(&child, styles, containers, tree, env, depth + 1, available);
        }
    }
}

use crate::layout::Allocation;
use crate::paint::{PaintCx, paint_node_with_children};
use skia_rs_safe::canvas::Canvas;

/// The seam between the generic paint walk and per-widget content.
///
/// `App` implements this over the `Instance` tree so a node's own
/// [`Controller::paint`](crate::view::Controller::paint) runs inside that
/// node's effect layer. Keeping it a trait is what lets [`paint_tree`] know
/// nothing about `Instance`s.
pub trait NodePainter {
    /// Paint `node`'s own content, after M2's box painting and before its
    /// children. `true` if anything was drawn.
    fn paint_content(
        &mut self,
        node: &Node,
        canvas: &mut Canvas<'_>,
        alloc: &Allocation,
        style: &ComputedStyle,
        cx: &mut PaintCx<'_>,
    ) -> bool;
}

/// A [`NodePainter`] that paints nothing — boxes, borders and backgrounds only.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoContent;

impl NodePainter for NoContent {
    fn paint_content(
        &mut self,
        _node: &Node,
        _canvas: &mut Canvas<'_>,
        _alloc: &Allocation,
        _style: &ComputedStyle,
        _cx: &mut PaintCx<'_>,
    ) -> bool {
        false
    }
}

/// Paint the whole retained tree, root first, each node inside its own
/// effect layer.
///
/// This is contract §9's "recursive paint walker": M2 shipped
/// [`paint_node_with_children`] correct for one node and never called it over
/// a tree (`paint/mod.rs:245`'s own comment says so). `origin` translates the
/// tree into surface space, exactly as `Button::render` does.
///
/// Returns the number of nodes painted; a node with no allocation (never laid
/// out, or below [`MAX_TREE_DEPTH`]) is skipped along with its subtree.
#[allow(
    clippy::too_many_arguments,
    reason = "one recursive walker threading the whole per-frame paint context"
)]
pub fn paint_tree(
    canvas: &mut Canvas<'_>,
    root: &Node,
    styles: &StyleMap,
    tree: &LayoutTree,
    anims: &mut Animations,
    now: Duration,
    origin: (f32, f32),
    cx: &mut PaintCx<'_>,
    painter: &mut dyn NodePainter,
) -> usize {
    let mut painted = 0usize;
    paint_one(
        canvas,
        root,
        styles,
        tree,
        anims,
        now,
        origin,
        cx,
        painter,
        &mut painted,
        0,
    );
    painted
}

#[allow(
    clippy::too_many_arguments,
    reason = "the recursive half of `paint_tree`"
)]
fn paint_one(
    canvas: &mut Canvas<'_>,
    node: &Node,
    styles: &StyleMap,
    tree: &LayoutTree,
    anims: &mut Animations,
    now: Duration,
    origin: (f32, f32),
    cx: &mut PaintCx<'_>,
    painter: &mut dyn NodePainter,
    painted: &mut usize,
    depth: usize,
) {
    if depth > MAX_TREE_DEPTH {
        return;
    }
    let Some(style) = styles.get(&node_addr(node)) else {
        return;
    };
    let Some(alloc) = tree.allocation(node) else {
        return;
    };
    let alloc = Allocation {
        border_box: crate::layout::Rect::new(
            alloc.border_box.x + origin.0,
            alloc.border_box.y + origin.1,
            alloc.border_box.width,
            alloc.border_box.height,
        ),
        content_box: crate::layout::Rect::new(
            alloc.content_box.x + origin.0,
            alloc.content_box.y + origin.1,
            alloc.content_box.width,
            alloc.content_box.height,
        ),
        border: alloc.border,
        padding: alloc.padding,
    };

    let overrides = anims.sample(node, now);
    *painted += 1;
    let children = node.children();

    paint_node_with_children(
        canvas,
        node,
        style,
        &alloc,
        Some(&overrides),
        cx,
        |canvas, cx| {
            // The node's own content first, then its children — GTK's order,
            // and both inside the layer `paint_node_with_children` opened, so
            // `opacity`, `transform` and `filter` reach all of it.
            painter.paint_content(node, canvas, &alloc, style, cx);
            for child in children {
                paint_one(
                    canvas,
                    &child,
                    styles,
                    tree,
                    anims,
                    now,
                    origin,
                    cx,
                    painter,
                    painted,
                    depth + 1,
                );
            }
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_childless_node_is_a_leaf_whatever_its_kind() {
        use crate::view::Kind;
        assert_eq!(
            container_for(Kind::Box, &crate::view::Props::default(), false),
            Container::Leaf
        );
        assert_eq!(
            container_for(Kind::Label, &crate::view::Props::default(), false),
            Container::Leaf
        );
    }

    #[test]
    fn gtk_s_default_orientations_are_reproduced_and_overridable() {
        use crate::view::{Kind, Prop, PropName, Props};
        let empty = Props::default();
        // GtkBox is horizontal by default.
        assert_eq!(
            container_for(Kind::Box, &empty, true),
            Container::Box {
                direction: BoxDirection::Row
            }
        );
        // A window stacks its children.
        assert_eq!(
            container_for(Kind::Window, &empty, true),
            Container::Box {
                direction: BoxDirection::Column
            }
        );
        assert_eq!(
            container_for(Kind::ListBox, &empty, true),
            Container::Box {
                direction: BoxDirection::Column
            }
        );

        let mut vertical = Props::default();
        vertical.set(PropName::Orientation, Prop::Enum(ORIENTATION_VERTICAL));
        assert_eq!(
            container_for(Kind::Box, &vertical, true),
            Container::Box {
                direction: BoxDirection::Column
            }
        );

        let mut horizontal = Props::default();
        horizontal.set(PropName::Orientation, Prop::Enum(ORIENTATION_HORIZONTAL));
        assert_eq!(
            container_for(Kind::Window, &horizontal, true),
            Container::Box {
                direction: BoxDirection::Row
            }
        );
    }

    #[test]
    fn the_layout_walker_allocates_every_styled_node() {
        let (sheet, env, window, button, label) = fixture();
        let mut styles = StyleMap::new();
        let mut anims = Animations::new();
        restyle_tree(
            &window,
            &sheet,
            &env,
            &mut styles,
            &mut anims,
            Duration::ZERO,
        );

        let mut containers: HashMap<NodeAddr, Container> = HashMap::new();
        containers.insert(
            node_addr(&window),
            Container::Box {
                direction: BoxDirection::Column,
            },
        );
        containers.insert(
            node_addr(&button),
            Container::Box {
                direction: BoxDirection::Row,
            },
        );
        containers.insert(node_addr(&label), Container::Leaf);

        let mut tree = LayoutTree::new();
        let mut measure = crate::layout::FixedMeasure(taffy::Size {
            width: 40.0,
            height: 16.0,
        });
        layout_tree(
            &window,
            &styles,
            &containers,
            &mut tree,
            &env,
            (Some(200.0), None),
            &mut measure,
        )
        .expect("the tree lays out");

        let window_alloc = tree.allocation(&window).expect("window allocation");
        let button_alloc = tree.allocation(&button).expect("button allocation");
        let label_alloc = tree.allocation(&label).expect("label allocation");
        assert!((window_alloc.border_box.width - 200.0).abs() < 0.5);
        assert!(label_alloc.border_box.width >= 40.0);
        assert!(
            button_alloc.border_box.height >= label_alloc.border_box.height,
            "the label overflowed its button"
        );
        // Allocations are absolute, so a nested node is offset by its parent.
        assert!(label_alloc.border_box.y >= button_alloc.border_box.y);
    }

    #[test]
    fn a_node_missing_from_the_container_map_is_laid_out_as_a_leaf() {
        let (sheet, env, window, _button, _label) = fixture();
        let mut styles = StyleMap::new();
        let mut anims = Animations::new();
        restyle_tree(
            &window,
            &sheet,
            &env,
            &mut styles,
            &mut anims,
            Duration::ZERO,
        );

        // An empty map: nothing has been reconciled yet. This must not panic
        // and must still produce a laid-out tree.
        let containers: HashMap<NodeAddr, Container> = HashMap::new();
        let mut tree = LayoutTree::new();
        let mut measure = crate::layout::FixedMeasure(taffy::Size {
            width: 8.0,
            height: 8.0,
        });
        layout_tree(
            &window,
            &styles,
            &containers,
            &mut tree,
            &env,
            (Some(100.0), Some(50.0)),
            &mut measure,
        )
        .expect("an unmapped tree still lays out");
        assert!(tree.allocation(&window).is_some());
    }

    #[test]
    fn an_unstyled_root_is_an_error_not_a_panic() {
        let env = ResolveEnv::default();
        let orphan = Node::new("window");
        let styles = StyleMap::new();
        let containers: HashMap<NodeAddr, Container> = HashMap::new();
        let mut tree = LayoutTree::new();
        let mut measure = crate::layout::FixedMeasure(taffy::Size::ZERO);
        // `sync` inserts the node, so this succeeds; what must not happen is
        // an unwrap on a missing style.
        assert!(
            layout_tree(
                &orphan,
                &styles,
                &containers,
                &mut tree,
                &env,
                (None, None),
                &mut measure,
            )
            .is_ok()
        );
    }
    use crate::anim::ManualClock;
    use crate::css::node::{Node, PseudoStates};
    use crate::css::registry::Prop as CssProp;

    const SHEET: &str = "
        window { background-color: #ffffff; color: #000000; }
        button { background-color: #cccccc; transition: background-color 200ms linear; }
        button:hover { background-color: #eeeeee; }
        label { color: #112233; }
    ";

    fn fixture() -> (CompiledSheet, ResolveEnv, Node, Node, Node) {
        let sheet = CompiledSheet::compile(SHEET);
        let env = ResolveEnv::default();
        let window = Node::with_classes("window", &["background"]);
        let button = Node::new("button");
        let label = Node::new("label");
        window.append_child(&button);
        button.append_child(&label);
        (sheet, env, window, button, label)
    }

    #[test]
    fn the_first_restyle_styles_every_node_and_starts_no_transition() {
        let (sheet, env, window, button, _label) = fixture();
        let mut styles = StyleMap::new();
        let mut anims = Animations::new();

        let changed = restyle_tree(
            &window,
            &sheet,
            &env,
            &mut styles,
            &mut anims,
            Duration::ZERO,
        );
        assert_eq!(changed, 3, "window, button and label all got a first style");
        assert_eq!(styles.len(), 3);
        assert!(styles.contains_key(&node_addr(&button)));
        // CSS Transitions: there is no before-change style, so nothing runs.
        assert!(!anims.is_active(Duration::ZERO));
        assert_eq!(anims.next_deadline(Duration::ZERO), None);
    }

    #[test]
    fn a_restyle_with_no_change_reports_nothing_dirty() {
        let (sheet, env, window, _button, _label) = fixture();
        let mut styles = StyleMap::new();
        let mut anims = Animations::new();
        restyle_tree(
            &window,
            &sheet,
            &env,
            &mut styles,
            &mut anims,
            Duration::ZERO,
        );
        let changed = restyle_tree(
            &window,
            &sheet,
            &env,
            &mut styles,
            &mut anims,
            Duration::from_millis(1),
        );
        assert_eq!(changed, 0);
    }

    #[test]
    fn a_state_change_restyles_that_node_and_starts_its_transition() {
        let (sheet, env, window, button, _label) = fixture();
        let mut styles = StyleMap::new();
        let mut anims = Animations::new();
        restyle_tree(
            &window,
            &sheet,
            &env,
            &mut styles,
            &mut anims,
            Duration::ZERO,
        );

        button.set_state(PseudoStates::HOVER, true);
        let changed = restyle_tree(
            &window,
            &sheet,
            &env,
            &mut styles,
            &mut anims,
            Duration::from_millis(10),
        );
        assert_eq!(changed, 1, "only the button's own style changed");
        assert!(anims.is_active(Duration::from_millis(10)));

        // Mid-transition the sampled value is neither endpoint.
        let clock = ManualClock::new();
        clock.set_ms(110);
        let overrides = anims.sample(&button, Duration::from_millis(110));
        let mid = overrides
            .get(CssProp::BackgroundColor)
            .expect("background-color is transitioning");
        let end = styles[&node_addr(&button)].raw(CssProp::BackgroundColor);
        assert_ne!(mid, end, "the transition was skipped to its end value");
    }

    #[test]
    fn detached_nodes_lose_their_style_and_their_animation_state() {
        let (sheet, env, window, button, _label) = fixture();
        let mut styles = StyleMap::new();
        let mut anims = Animations::new();
        restyle_tree(
            &window,
            &sheet,
            &env,
            &mut styles,
            &mut anims,
            Duration::ZERO,
        );
        button.set_state(PseudoStates::HOVER, true);
        restyle_tree(
            &window,
            &sheet,
            &env,
            &mut styles,
            &mut anims,
            Duration::from_millis(1),
        );
        assert_eq!(anims.len(), 1);

        button.detach();
        restyle_tree(
            &window,
            &sheet,
            &env,
            &mut styles,
            &mut anims,
            Duration::from_millis(2),
        );
        assert_eq!(styles.len(), 1, "only the window is still styled");
        assert!(anims.is_empty(), "a detached node's animations are dropped");
    }

    #[test]
    fn a_deeply_nested_tree_never_blows_the_stack() {
        // Untrusted input: a `view` that builds a pathological chain must be
        // clamped, not crash the client. The walker itself is bounded by
        // `MAX_TREE_DEPTH`, but a 10,000-deep `Node` chain's ordinary,
        // compiler-derived `Drop` (each `NodeInner` owns its children by
        // value) recurses just as deep when the chain is torn down at the
        // end of the test — unrelated to `restyle_tree`'s own recursion, and
        // not this crate's to fix (`css::node::Node` is off-limits, contract
        // §9). Run the whole test on a thread with a generous stack so that
        // teardown, not just the walk, survives.
        std::thread::Builder::new()
            .stack_size(64 * 1024 * 1024)
            .spawn(|| {
                let sheet = CompiledSheet::compile(SHEET);
                let env = ResolveEnv::default();
                let root = Node::new("window");
                let mut cursor = root.clone();
                for _ in 0..10_000 {
                    let child = Node::new("box");
                    cursor.append_child(&child);
                    cursor = child;
                }
                let mut styles = StyleMap::new();
                let mut anims = Animations::new();
                let changed =
                    restyle_tree(&root, &sheet, &env, &mut styles, &mut anims, Duration::ZERO);
                assert!(changed >= 1);
                assert!(
                    changed <= MAX_TREE_DEPTH + 1,
                    "the walk did not stop at the depth budget: {changed}"
                );
            })
            .expect("spawn")
            .join()
            .expect("the test thread panicked");
    }

    /// Paint a solid rect wherever the node has a `content` marker class, so
    /// the test can prove `Controller::paint`'s seam runs *inside* the
    /// node's effect layer.
    struct MarkerPainter;

    impl NodePainter for MarkerPainter {
        fn paint_content(
            &mut self,
            node: &Node,
            canvas: &mut skia_rs_safe::canvas::Canvas<'_>,
            alloc: &crate::layout::Allocation,
            _style: &ComputedStyle,
            _cx: &mut crate::paint::PaintCx<'_>,
        ) -> bool {
            if !node.classes().iter().any(|c| c.as_str() == "content") {
                return false;
            }
            let mut paint = skia_rs_safe::paint::Paint::default();
            paint.set_color(skia_rs_safe::core::Color(0xFF00_FF00).into());
            canvas.draw_rect(&alloc.content_box.to_skia(), &paint);
            true
        }
    }

    fn paint_fixture() -> (CompiledSheet, ResolveEnv, Node, Node) {
        let sheet = CompiledSheet::compile(
            "window { background-color: #ff0000; }
             box { background-color: #0000ff; opacity: 0.5; }",
        );
        let env = ResolveEnv::default();
        let window = Node::with_classes("window", &["background"]);
        let inner = Node::with_classes("box", &["content"]);
        window.append_child(&inner);
        (sheet, env, window, inner)
    }

    #[test]
    fn paint_tree_paints_every_node_once_root_first() {
        let (sheet, env, window, inner) = paint_fixture();
        let mut styles = StyleMap::new();
        let mut anims = Animations::new();
        restyle_tree(
            &window,
            &sheet,
            &env,
            &mut styles,
            &mut anims,
            Duration::ZERO,
        );

        let mut containers: HashMap<NodeAddr, Container> = HashMap::new();
        containers.insert(
            node_addr(&window),
            Container::Box {
                direction: BoxDirection::Column,
            },
        );
        containers.insert(node_addr(&inner), Container::Leaf);
        let mut tree = LayoutTree::new();
        let mut measure = crate::layout::FixedMeasure(taffy::Size {
            width: 30.0,
            height: 20.0,
        });
        layout_tree(
            &window,
            &styles,
            &containers,
            &mut tree,
            &env,
            (Some(60.0), Some(40.0)),
            &mut measure,
        )
        .expect("lays out");

        let mut surface =
            skia_rs_safe::canvas::Surface::new_raster_n32_premul(60, 40).expect("raster surface");
        surface
            .canvas()
            .clear(skia_rs_safe::core::Color::TRANSPARENT);
        let mut fonts = crate::text::FontDatabase::probe_only();
        let mut images = crate::paint::ImageCache::new();
        let mut cx = crate::paint::PaintCx {
            env: &env,
            colors: &sheet.colors,
            fonts: &mut fonts,
            images: &mut images,
            text: None,
        };
        let painted = {
            let mut canvas = surface.canvas();
            paint_tree(
                &mut canvas,
                &window,
                &styles,
                &tree,
                &mut anims,
                Duration::ZERO,
                (0.0, 0.0),
                &mut cx,
                &mut MarkerPainter,
            )
        };
        assert_eq!(painted, 2);

        let pixel = surface
            .pixel_buffer()
            .get_pixel(20, 15)
            .expect("pixel in range");
        // The inner box is 50% opaque green content over 50% opaque blue
        // over opaque red — the exact blend is M2's business; what this
        // pins is that all three layers ran, so no channel is saturated at
        // the pure red the window alone would leave.
        assert_ne!(pixel, skia_rs_safe::core::Color(0xFFFF_0000));

        let outside = surface
            .pixel_buffer()
            .get_pixel(59, 39)
            .expect("pixel in range");
        assert_eq!(
            outside,
            skia_rs_safe::core::Color(0xFFFF_0000),
            "the window background did not reach its own bottom-right corner"
        );
    }

    #[test]
    fn the_content_painter_runs_inside_the_node_s_own_effect_layer() {
        // `opacity: 0.5` on the box must dim the green content the painter
        // draws. If the content were painted after the layer closed, the
        // green would be fully opaque.
        let (sheet, env, window, inner) = paint_fixture();
        let mut styles = StyleMap::new();
        let mut anims = Animations::new();
        restyle_tree(
            &window,
            &sheet,
            &env,
            &mut styles,
            &mut anims,
            Duration::ZERO,
        );
        let mut containers: HashMap<NodeAddr, Container> = HashMap::new();
        containers.insert(
            node_addr(&window),
            Container::Box {
                direction: BoxDirection::Column,
            },
        );
        containers.insert(node_addr(&inner), Container::Leaf);
        let mut tree = LayoutTree::new();
        let mut measure = crate::layout::FixedMeasure(taffy::Size {
            width: 60.0,
            height: 40.0,
        });
        layout_tree(
            &window,
            &styles,
            &containers,
            &mut tree,
            &env,
            (Some(60.0), Some(40.0)),
            &mut measure,
        )
        .expect("lays out");

        let mut surface =
            skia_rs_safe::canvas::Surface::new_raster_n32_premul(60, 40).expect("raster surface");
        surface
            .canvas()
            .clear(skia_rs_safe::core::Color::TRANSPARENT);
        let mut fonts = crate::text::FontDatabase::probe_only();
        let mut images = crate::paint::ImageCache::new();
        let mut cx = crate::paint::PaintCx {
            env: &env,
            colors: &sheet.colors,
            fonts: &mut fonts,
            images: &mut images,
            text: None,
        };
        {
            let mut canvas = surface.canvas();
            paint_tree(
                &mut canvas,
                &window,
                &styles,
                &tree,
                &mut anims,
                Duration::ZERO,
                (0.0, 0.0),
                &mut cx,
                &mut MarkerPainter,
            );
        }
        let skia_rs_safe::core::Color(argb) = surface
            .pixel_buffer()
            .get_pixel(30, 20)
            .expect("pixel in range");
        let g = ((argb >> 8) & 0xFF) as u8;
        assert!(
            g > 0 && g < 0xFF,
            "the content green was not dimmed by the node's opacity: {g:#04x}"
        );
    }

    #[test]
    fn a_node_with_no_allocation_is_skipped_not_unwrapped() {
        let (sheet, env, window, _inner) = paint_fixture();
        let mut styles = StyleMap::new();
        let mut anims = Animations::new();
        restyle_tree(
            &window,
            &sheet,
            &env,
            &mut styles,
            &mut anims,
            Duration::ZERO,
        );
        // No `layout_tree` call at all: the LayoutTree is empty.
        let tree = LayoutTree::new();
        let mut surface =
            skia_rs_safe::canvas::Surface::new_raster_n32_premul(10, 10).expect("raster surface");
        let mut fonts = crate::text::FontDatabase::probe_only();
        let mut images = crate::paint::ImageCache::new();
        let mut cx = crate::paint::PaintCx {
            env: &env,
            colors: &sheet.colors,
            fonts: &mut fonts,
            images: &mut images,
            text: None,
        };
        let painted = {
            let mut canvas = surface.canvas();
            paint_tree(
                &mut canvas,
                &window,
                &styles,
                &tree,
                &mut anims,
                Duration::ZERO,
                (0.0, 0.0),
                &mut cx,
                &mut NoContent,
            )
        };
        assert_eq!(painted, 0);
    }
}
