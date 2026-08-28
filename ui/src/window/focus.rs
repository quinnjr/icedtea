//! The focus ring: who can take focus, in what order, and whether the ring is
//! drawn.
//!
//! GTK's order is *geometric*, not tree order (ruling R2,
//! `gtk_widget_focus_sort`): a HeaderBar's end packs, a Grid's cells and an
//! Overlay's layers are all laid out in an order their tree does not describe.

use crate::css::node::{Direction, Node, PseudoStates};
use crate::layout::LayoutTree;

/// Which way focus is moving.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusDirection {
    TabForward,
    TabBackward,
    Up,
    Down,
    Left,
    Right,
}

/// The style class that marks a node as taking focus.
///
/// The retained tree's only writable per-node vocabulary is names, ids,
/// classes and pseudo-states, and "can this take focus" is none of the
/// pseudo-states GTK defines -- so P4's `PropName::Focusable` reaches this
/// layer as a class. `WindowControls`' three buttons deliberately do not carry
/// it: GTK paints and clicks them but never focuses them.
pub const FOCUSABLE_CLASS: &str = "focusable";

/// Whether a node participates in the ring at all.
///
/// Marked focusable, not disabled, and actually laid out with a non-zero
/// allocation -- an unmapped widget is not a focus candidate, and neither is
/// one the reconciler added after the last layout pass.
#[must_use]
pub fn is_focusable(node: &Node, tree: &LayoutTree) -> bool {
    if node.states().contains(PseudoStates::DISABLED) {
        return false;
    }
    if !node
        .classes()
        .iter()
        .any(|class| class.as_str() == FOCUSABLE_CLASS)
    {
        return false;
    }
    tree.allocation(node)
        .is_some_and(|alloc| !alloc.border_box.is_empty())
}

/// The centre of `node`'s border box, or `None` if it is not laid out.
fn centre(node: &Node, tree: &LayoutTree) -> Option<(f32, f32)> {
    let alloc = tree.allocation(node)?;
    if alloc.border_box.is_empty() {
        return None;
    }
    Some((
        alloc.border_box.x + alloc.border_box.width / 2.0,
        alloc.border_box.y + alloc.border_box.height / 2.0,
    ))
}

/// `parent`'s mapped, sensitive, focusable direct children, in `dir` order.
///
/// Tab order is reading order: by y-centre, then by x-centre, with x reversed
/// under RTL. The arrow directions sort along their own axis first, so
/// `Right` walks a row before descending. `TabBackward`, `Left` and `Up` are
/// the reverse of their opposites -- one comparator, four orders, so a change
/// to the tie-break cannot apply to only half of them.
#[must_use]
pub fn focus_sort(parent: &Node, tree: &LayoutTree, dir: FocusDirection) -> Vec<Node> {
    let rtl = parent.direction() == Direction::Rtl;
    let mut candidates: Vec<(Node, (f32, f32))> = parent
        .children()
        .into_iter()
        .filter(|child| !child.states().contains(PseudoStates::DISABLED))
        .filter_map(|child| centre(&child, tree).map(|c| (child, c)))
        .collect();

    let horizontal = matches!(dir, FocusDirection::Left | FocusDirection::Right);
    candidates.sort_by(|(_, a), (_, b)| {
        let (a_major, a_minor, b_major, b_minor) = if horizontal {
            (a.0, a.1, b.0, b.1)
        } else {
            (a.1, a.0, b.1, b.0)
        };
        // A row is a band, not an exact y: two widgets of different heights
        // centred on the same line differ by a fraction of a pixel, and an
        // exact comparison would order them by that fraction.
        let major = if (a_major - b_major).abs() < ROW_EPSILON {
            std::cmp::Ordering::Equal
        } else {
            a_major.total_cmp(&b_major)
        };
        major.then_with(|| {
            let minor = a_minor.total_cmp(&b_minor);
            // RTL reverses only the *horizontal* comparison: rows still run
            // top to bottom.
            if rtl && !horizontal {
                minor.reverse()
            } else {
                minor
            }
        })
    });

    if matches!(
        dir,
        FocusDirection::TabBackward | FocusDirection::Left | FocusDirection::Up
    ) {
        candidates.reverse();
    }
    candidates.into_iter().map(|(node, _)| node).collect()
}

/// How far apart two centres may be and still count as the same row/column.
///
/// Half a CSS pixel: enough to absorb a centring difference between a 34px
/// button and a 40px entry, far too little to merge two real rows.
const ROW_EPSILON: f32 = 0.5;

#[cfg(test)]
mod tests {
    use super::{FOCUSABLE_CLASS, FocusDirection, focus_sort, is_focusable};
    use crate::css::cascade::CompiledSheet;
    use crate::css::computed::ResolveEnv;
    use crate::css::node::{Direction, Node, PseudoStates};
    use crate::layout::{FixedMeasure, LayoutTree};
    use crate::window::{StyleMap, restyle};

    /// Four 60x40 buttons in one row, two of them pushed 60px down, so the
    /// arrangement reads as a 2x2 grid whose *tree* order is deliberately not
    /// its *geometric* order (ruling R2).
    const GRID_CSS: &str = "
        window { min-width: 300px; min-height: 160px; }
        box { min-width: 300px; min-height: 160px; }
        button { min-width: 60px; min-height: 40px; }
        #c, #d { margin-top: 60px; }
    ";

    struct Grid {
        root: Node,
        container: Node,
        tree: LayoutTree,
        // Held only so the map `restyle` populates outlives the fixture;
        // no test reads it back.
        #[allow(dead_code)]
        styles: StyleMap,
    }

    /// Tree order: b, d, a, c. Geometry: b and a on the top row (b left of a),
    /// d and c on the bottom row (d left of c).
    fn grid() -> Grid {
        let root = Node::new("window");
        let container = Node::new("box");
        root.append_child(&container);
        for id in ["b", "d", "a", "c"] {
            let button = Node::with_classes("button", &[FOCUSABLE_CLASS]);
            button.set_id(Some(id));
            container.append_child(&button);
        }
        let sheet = CompiledSheet::compile(GRID_CSS);
        let env = ResolveEnv::default();
        let mut tree = LayoutTree::new();
        let mut styles = StyleMap::new();
        let mut measure = FixedMeasure(taffy::Size {
            width: 0.0,
            height: 0.0,
        });
        restyle(
            &root,
            &sheet,
            &env,
            &mut styles,
            &mut tree,
            taffy::Size {
                width: taffy::AvailableSpace::Definite(300.0),
                height: taffy::AvailableSpace::Definite(160.0),
            },
            &mut measure,
        )
        .expect("the grid lays out");
        Grid {
            root,
            container,
            tree,
            styles,
        }
    }

    fn ids(nodes: &[Node]) -> Vec<String> {
        nodes
            .iter()
            .map(|n| {
                n.id()
                    .map_or_else(String::new, |id| id.as_str().to_string())
            })
            .collect()
    }

    fn centre(grid: &Grid, id: &str) -> (f32, f32) {
        let node = grid
            .container
            .children()
            .into_iter()
            .find(|n| n.id().is_some_and(|got| got.as_str() == id))
            .expect("the node exists");
        let alloc = grid.tree.allocation(&node).expect("it is laid out");
        (
            alloc.border_box.x + alloc.border_box.width / 2.0,
            alloc.border_box.y + alloc.border_box.height / 2.0,
        )
    }

    #[test]
    fn the_fixture_really_is_two_rows() {
        // Guards the tests below: if the margin stops moving anything, a
        // "geometric order" assertion would silently become a tree-order one.
        let grid = grid();
        assert!(
            centre(&grid, "b").1 < centre(&grid, "d").1,
            "b must sit above d"
        );
        assert!(
            (centre(&grid, "b").1 - centre(&grid, "a").1).abs() < 1.0,
            "b and a share a row"
        );
        assert!(
            centre(&grid, "b").0 < centre(&grid, "a").0,
            "b is left of a"
        );
        assert!(
            centre(&grid, "d").0 < centre(&grid, "c").0,
            "d is left of c"
        );
    }

    #[test]
    fn tab_order_is_geometric_and_not_tree_order() {
        // Ruling R2. Tree order is b, d, a, c; reading order is b, a, d, c.
        let grid = grid();
        assert_eq!(
            ids(&focus_sort(
                &grid.container,
                &grid.tree,
                FocusDirection::TabForward
            )),
            vec!["b", "a", "d", "c"]
        );
        assert_eq!(
            ids(&grid.container.children()),
            vec!["b", "d", "a", "c"],
            "and it really is different from tree order"
        );
    }

    #[test]
    fn shift_tab_reverses_the_ring() {
        let grid = grid();
        assert_eq!(
            ids(&focus_sort(
                &grid.container,
                &grid.tree,
                FocusDirection::TabBackward
            )),
            vec!["c", "d", "a", "b"]
        );
    }

    #[test]
    fn the_arrow_directions_sort_along_their_own_axis() {
        let grid = grid();
        // Left/Right order by x first: the two left-hand widgets before the
        // two right-hand ones.
        assert_eq!(
            ids(&focus_sort(
                &grid.container,
                &grid.tree,
                FocusDirection::Right
            )),
            vec!["b", "d", "a", "c"]
        );
        assert_eq!(
            ids(&focus_sort(
                &grid.container,
                &grid.tree,
                FocusDirection::Left
            )),
            vec!["c", "a", "d", "b"]
        );
        assert_eq!(
            ids(&focus_sort(
                &grid.container,
                &grid.tree,
                FocusDirection::Down
            )),
            vec!["b", "a", "d", "c"]
        );
        assert_eq!(
            ids(&focus_sort(&grid.container, &grid.tree, FocusDirection::Up)),
            vec!["c", "d", "a", "b"]
        );
    }

    #[test]
    fn rtl_reverses_the_horizontal_tie_break_only() {
        let grid = grid();
        grid.root.set_direction(Some(Direction::Rtl));
        assert_eq!(
            ids(&focus_sort(
                &grid.container,
                &grid.tree,
                FocusDirection::TabForward
            )),
            vec!["a", "b", "c", "d"],
            "rows still run top to bottom; within a row, right to left"
        );
        grid.root.set_direction(Some(Direction::Ltr));
        assert_eq!(
            ids(&focus_sort(
                &grid.container,
                &grid.tree,
                FocusDirection::TabForward
            )),
            vec!["b", "a", "d", "c"]
        );
    }

    #[test]
    fn only_marked_mapped_sensitive_nodes_are_candidates() {
        let grid = grid();
        let children = grid.container.children();
        let b = children[0].clone();
        assert!(is_focusable(&b, &grid.tree));

        b.set_state(PseudoStates::DISABLED, true);
        assert!(
            !is_focusable(&b, &grid.tree),
            "an insensitive widget is not in the ring"
        );
        b.set_state(PseudoStates::DISABLED, false);

        b.remove_class(FOCUSABLE_CLASS);
        assert!(
            !is_focusable(&b, &grid.tree),
            "GTK's WindowControls buttons are exactly this case: painted, clickable, never focused"
        );
        b.add_class(FOCUSABLE_CLASS);

        let unmapped = Node::with_classes("button", &[FOCUSABLE_CLASS]);
        grid.container.append_child(&unmapped);
        assert!(
            !is_focusable(&unmapped, &grid.tree),
            "a node with no allocation is not focusable"
        );
        assert_eq!(
            ids(&focus_sort(
                &grid.container,
                &grid.tree,
                FocusDirection::TabForward
            ))
            .len(),
            4,
            "and it is not in the sorted ring either"
        );
    }

    #[test]
    fn a_container_with_no_focusable_children_sorts_to_nothing() {
        let root = Node::new("window");
        let tree = LayoutTree::new();
        assert!(focus_sort(&root, &tree, FocusDirection::TabForward).is_empty());
        assert!(
            !is_focusable(&root, &tree),
            "an unsynced node is never focusable"
        );
    }
}
