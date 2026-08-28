//! Pointer and touch: hit testing the retained tree, the client-side implicit
//! grab, scrolling and cursor shapes.

use crate::css::node::{Node, PseudoStates};
use crate::layout::{LayoutTree, Rect};
use crate::window::{StyleMap, node_addr};

/// A node the pointer is over, and where on it.
#[derive(Debug, Clone)]
pub struct Hit {
    pub node: Node,
    /// The point in the node's border-box space.
    pub local: (f32, f32),
}

/// How deep the hit test will descend.
///
/// A retained tree is built by a reconciler from application data; a bug at
/// either end can produce one thousands of levels deep, and an unbounded
/// recursive descent over that is a stack overflow -- an abort, not a panic a
/// test can catch. GTK's own widget hierarchies are tens of levels at most, so
/// this bound never fires in practice.
pub const MAX_HIT_DEPTH: usize = 256;

/// Whether `rect` contains `point`, half-open on the right and bottom.
///
/// Half-open is what makes two edge-to-edge siblings unambiguous: the pixel at
/// x == 60 belongs to the node starting there, not to the one ending there.
fn contains(rect: Rect, point: (f32, f32)) -> bool {
    point.0 >= rect.x && point.0 < rect.right() && point.1 >= rect.y && point.1 < rect.bottom()
}

/// Whether this node can be hit at all, ignoring geometry.
fn hittable(node: &Node, styles: &StyleMap, respect_sensitive: bool) -> bool {
    if respect_sensitive && node.states().contains(PseudoStates::DISABLED) {
        return false;
    }
    // Contract deviation 10: GTK 4 has no `visibility` property, so an
    // invisible widget reaches this layer as `opacity: 0` or as a node with no
    // allocation (checked by the caller).
    styles
        .get(&node_addr(node))
        .is_none_or(|style| style.opacity() > 0.0)
}

/// The capture->target chain at `point`, outermost first.
///
/// Controller dispatch is capture -> target -> bubble, mirroring GTK, so the
/// chain is the whole answer: `hit_test` is its last element.
///
/// Insensitive subtrees are skipped: GTK delivers no events to an insensitive
/// widget or anything inside it.
#[must_use]
pub fn hit_chain(root: &Node, tree: &LayoutTree, styles: &StyleMap, point: (f32, f32)) -> Vec<Hit> {
    let mut chain = Vec::new();
    descend(root, tree, styles, point, true, &mut chain);
    chain
}

/// The topmost-painted node containing `point` (window frame space).
///
/// Skips a subtree with no allocation, a zero-area one, a fully transparent
/// one, and -- under `respect_sensitive` -- one carrying
/// [`PseudoStates::DISABLED`].
#[must_use]
pub fn hit_test(
    root: &Node,
    tree: &LayoutTree,
    styles: &StyleMap,
    point: (f32, f32),
    respect_sensitive: bool,
) -> Option<Hit> {
    let mut chain = Vec::new();
    descend(root, tree, styles, point, respect_sensitive, &mut chain);
    chain.pop()
}

/// Push `node` onto `chain` if it contains `point`, then the deepest,
/// last-painted descendant that does.
///
/// Children are visited in reverse order because paint order is tree order:
/// the last sibling painted is the one on top, and the one the pointer hits.
/// The walk stays inside the parent's border box, as GTK's does -- a child that
/// overflows its parent is drawn but not hit.
fn descend(
    node: &Node,
    tree: &LayoutTree,
    styles: &StyleMap,
    point: (f32, f32),
    respect_sensitive: bool,
    chain: &mut Vec<Hit>,
) -> bool {
    if chain.len() >= MAX_HIT_DEPTH {
        tracing::warn!(
            depth = chain.len(),
            node = %node.name(),
            "hit test stopped at the depth guard"
        );
        return false;
    }
    let Some(alloc) = tree.allocation(node) else {
        return false;
    };
    if alloc.border_box.is_empty() || !contains(alloc.border_box, point) {
        return false;
    }
    if !hittable(node, styles, respect_sensitive) {
        return false;
    }
    chain.push(Hit {
        node: node.clone(),
        local: (point.0 - alloc.border_box.x, point.1 - alloc.border_box.y),
    });
    for child in node.children().into_iter().rev() {
        if descend(&child, tree, styles, point, respect_sensitive, chain) {
            break;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::{Hit, MAX_HIT_DEPTH, hit_chain, hit_test};
    use crate::css::cascade::CompiledSheet;
    use crate::css::computed::ResolveEnv;
    use crate::css::node::{Node, PseudoStates};
    use crate::layout::{FixedMeasure, LayoutTree};
    use crate::window::{StyleMap, restyle};

    /// `window > box > (one, two, three)`, three 60x40 buttons in a row with
    /// the second and third pulled 30px left so each overlaps its predecessor.
    ///
    /// `Container::Box` centres its children on *both* axes (`layout.rs`'s
    /// fixed `AlignItems::CENTER`/`JustifyContent::CENTER`, unconditional and
    /// out of P3's reach), so the row does not start flush with `box`'s own
    /// edge: `box` is 200x40 sitting at `y = 30` (centred in the 100px
    /// window), and the row's 120px of content (60 + 30 + 30, the two
    /// negative margins each pulling a button back 30px) sits centred in
    /// `box`'s 200px width, an outer offset of 40. `one` therefore spans
    /// x 40..100, `two` x 70..130 (overlapping `one` at 70..100), `three`
    /// x 100..160 (overlapping `two` at 100..130), all at y 30..70.
    const OVERLAP_CSS: &str = "
        window { min-width: 200px; min-height: 100px; }
        box { min-width: 200px; min-height: 40px; }
        button { min-width: 60px; min-height: 40px; }
        #two, #three { margin-left: -30px; }
    ";

    struct Fixture {
        root: Node,
        tree: LayoutTree,
        styles: StyleMap,
    }

    fn fixture(css: &str, root: &Node) -> Fixture {
        let sheet = CompiledSheet::compile(css);
        let env = ResolveEnv::default();
        let mut tree = LayoutTree::new();
        let mut styles = StyleMap::new();
        let mut measure = FixedMeasure(taffy::Size {
            width: 0.0,
            height: 0.0,
        });
        restyle(
            root,
            &sheet,
            &env,
            &mut styles,
            &mut tree,
            taffy::Size {
                width: taffy::AvailableSpace::Definite(200.0),
                height: taffy::AvailableSpace::Definite(100.0),
            },
            &mut measure,
        )
        .expect("the fixture lays out");
        Fixture {
            root: root.clone(),
            tree,
            styles,
        }
    }

    fn overlapping() -> (Fixture, Node, Node, Node) {
        let window = Node::new("window");
        let container = Node::new("box");
        window.append_child(&container);
        let mut made = Vec::new();
        for id in ["one", "two", "three"] {
            let button = Node::new("button");
            button.set_id(Some(id));
            container.append_child(&button);
            made.push(button);
        }
        let fixture = fixture(OVERLAP_CSS, &window);
        (fixture, made[0].clone(), made[1].clone(), made[2].clone())
    }

    fn id_of(hit: &Hit) -> String {
        hit.node.id().map_or_else(
            || (*hit.node.name()).to_string(),
            |id| id.as_str().to_string(),
        )
    }

    #[test]
    fn the_last_painted_sibling_wins_an_overlap() {
        let (f, one, _two, _three) = overlapping();
        // A point squarely inside `one` alone (x 40..70, before `two` starts
        // at 70).
        assert_eq!(
            id_of(&hit_test(&f.root, &f.tree, &f.styles, (50.0, 50.0), true).unwrap()),
            "one"
        );
        assert_eq!(
            id_of(&hit_test(&f.root, &f.tree, &f.styles, (85.0, 50.0), true).unwrap()),
            "two",
            "in the one/two overlap (x 70..100) the later sibling is painted on top"
        );
        assert_eq!(
            id_of(&hit_test(&f.root, &f.tree, &f.styles, (115.0, 50.0), true).unwrap()),
            "three",
            "in the two/three overlap (x 100..130) the later sibling is painted on top"
        );
        assert!(one.id().is_some(), "the fixture kept its ids");
    }

    #[test]
    fn a_point_outside_every_child_lands_on_the_container_and_then_nothing() {
        let (f, _one, _two, _three) = overlapping();
        // Above the centred 40px-tall row (`box` sits at y 30..70), still
        // inside the 100px window.
        let hit =
            hit_test(&f.root, &f.tree, &f.styles, (10.0, 10.0), true).expect("inside the window");
        assert_eq!(id_of(&hit), "window");
        assert_eq!(
            hit.local,
            (10.0, 10.0),
            "local is relative to the hit node's border box"
        );
        assert!(
            hit_test(&f.root, &f.tree, &f.styles, (500.0, 500.0), true).is_none(),
            "a point outside the root hits nothing at all"
        );
    }

    #[test]
    fn a_fully_transparent_subtree_is_not_hit() {
        // GTK delivers no events to a widget with opacity 0: it is not just
        // invisible, it is not there.
        let (f, _one, _two, _three) = {
            let window = Node::new("window");
            let container = Node::new("box");
            window.append_child(&container);
            let hidden = Node::new("button");
            hidden.set_id(Some("hidden"));
            container.append_child(&hidden);
            let css = "
                window { min-width: 200px; min-height: 100px; }
                box { min-width: 200px; min-height: 40px; }
                button { min-width: 60px; min-height: 40px; }
                #hidden { opacity: 0; }
            ";
            let f = fixture(css, &window);
            (f, hidden.clone(), hidden.clone(), hidden)
        };
        // `box` is 200x40 at y 30..70 (centred in the 100px window); its one
        // 60-wide child is centred in turn, landing at x 70..130. (100, 50)
        // is inside both.
        let hit =
            hit_test(&f.root, &f.tree, &f.styles, (100.0, 50.0), true).expect("something is hit");
        assert_ne!(id_of(&hit), "hidden", "opacity: 0 must not be hit");
        assert_eq!(id_of(&hit), "box");
    }

    #[test]
    fn a_disabled_node_is_skipped_only_when_sensitivity_is_respected() {
        let (f, one, _two, _three) = overlapping();
        one.set_state(PseudoStates::DISABLED, true);
        // (50, 50) is inside `one` alone (x 40..70), no overlap with `two`.
        // The tree changed but the geometry did not: hit testing reads the
        // node's own state, not a cached style.
        let sensitive = hit_test(&f.root, &f.tree, &f.styles, (50.0, 50.0), true).unwrap();
        assert_eq!(
            id_of(&sensitive),
            "box",
            "an insensitive widget gets no events"
        );
        let raw = hit_test(&f.root, &f.tree, &f.styles, (50.0, 50.0), false).unwrap();
        assert_eq!(id_of(&raw), "one", "a tooltip query still finds it");
    }

    #[test]
    fn hit_chain_reports_the_capture_path_outermost_first() {
        let (f, _one, _two, _three) = overlapping();
        // (85, 50) is in the one/two overlap (x 70..100): `two` wins.
        let chain = hit_chain(&f.root, &f.tree, &f.styles, (85.0, 50.0));
        let names: Vec<String> = chain.iter().map(id_of).collect();
        assert_eq!(names, vec!["window", "box", "two"]);
        assert_eq!(
            chain.last().map(id_of),
            hit_test(&f.root, &f.tree, &f.styles, (85.0, 50.0), true)
                .as_ref()
                .map(id_of),
            "the target is the last of the chain and the answer hit_test gives"
        );
        assert!(hit_chain(&f.root, &f.tree, &f.styles, (500.0, 500.0)).is_empty());
    }

    #[test]
    fn a_tree_deeper_than_the_guard_is_truncated_not_a_stack_overflow() {
        // A reconciler bug, or a list model with a cyclic-looking shape, can
        // hand the window an absurdly deep tree. Descending it recursively
        // without a bound is a stack overflow -- which is an abort, not a
        // catchable panic.
        //
        // Building and laying out a ~300-level tree is itself recursive
        // (`LayoutTree::sync`/`compute` walk it, and so does `selectors`'
        // cascade matching) deep enough to overflow libtest's default 2MiB
        // worker stack before this test's own code -- the thing under test,
        // `hit_chain`'s `MAX_HIT_DEPTH` guard -- ever runs. That default
        // stack size is a test-harness artifact, not part of what this test
        // verifies, so the body runs on an explicitly larger stack instead of
        // shrinking the tree (which would stop exercising depths past the
        // guard).
        std::thread::Builder::new()
            .stack_size(16 * 1024 * 1024)
            .spawn(|| {
                let window = Node::new("window");
                let mut cursor = window.clone();
                for _ in 0..(MAX_HIT_DEPTH + 40) {
                    let child = Node::new("box");
                    cursor.append_child(&child);
                    cursor = child;
                }
                let css = "window, box { min-width: 200px; min-height: 100px; }";
                let f = fixture(css, &window);
                let chain = hit_chain(&f.root, &f.tree, &f.styles, (10.0, 10.0));
                assert!(!chain.is_empty(), "the shallow part is still hit");
                assert!(
                    chain.len() <= MAX_HIT_DEPTH,
                    "the walk descended {} levels, past the {MAX_HIT_DEPTH} guard",
                    chain.len()
                );
                assert!(hit_test(&f.root, &f.tree, &f.styles, (10.0, 10.0), true).is_some());
            })
            .expect("spawn the deep-tree probe thread")
            .join()
            .expect("the deep-tree probe thread panicked");
    }

    #[test]
    fn a_node_with_no_allocation_is_never_hit() {
        // Contract deviation 10: "not visible" reaches this layer as "no
        // allocation", because GTK 4 has no `visibility` CSS property.
        let (f, _one, _two, _three) = overlapping();
        let orphan = Node::new("button");
        orphan.set_id(Some("orphan"));
        f.root.child(0).expect("the box").append_child(&orphan);
        // `orphan` was added after the layout pass, so the tree has no
        // allocation for it.
        let hit = hit_test(&f.root, &f.tree, &f.styles, (10.0, 20.0), true).unwrap();
        assert_ne!(id_of(&hit), "orphan");
    }
}
