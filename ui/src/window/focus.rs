//! The focus ring: who can take focus, in what order, and whether the ring is
//! drawn.
//!
//! GTK's order is *geometric*, not tree order (ruling R2,
//! `gtk_widget_focus_sort`): a HeaderBar's end packs, a Grid's cells and an
//! Overlay's layers are all laid out in an order their tree does not describe.

use crate::css::node::{Direction, Node, PseudoStates};
use crate::layout::LayoutTree;
use crate::window::keyboard::{KeyEvent, Mods};
use xkbcommon::xkb;

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

/// Why the focus moved. Drives GTK's `:focus-visible` policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusCause {
    Keyboard,
    Pointer,
    Programmatic,
}

/// How deep [`navigate`] will descend, for the same reason
/// [`super::pointer::MAX_HIT_DEPTH`] exists.
pub const MAX_FOCUS_DEPTH: usize = 256;

/// One focus owner per window; popups form a stack of these.
#[derive(Debug)]
pub struct FocusRing {
    focus: Option<Node>,
    focus_visible: bool,
    default: Option<Node>,
    /// The focus as it was when the current key went down, if one is down.
    key_focus: Option<Option<Node>>,
}

impl Default for FocusRing {
    /// `focus_visible` starts `true`, as GTK's does: a window opened by a
    /// keyboard shortcut shows its focus ring immediately.
    fn default() -> Self {
        Self {
            focus: None,
            focus_visible: true,
            default: None,
            key_focus: None,
        }
    }
}

impl FocusRing {
    #[must_use]
    pub fn focus(&self) -> Option<Node> {
        self.focus.clone()
    }

    /// Move `PseudoStates::FOCUS`, and with it M2's derived
    /// `:focus-within`/`:focus-visible`.
    pub fn set_focus(&mut self, node: Option<&Node>, cause: FocusCause) {
        match cause {
            FocusCause::Keyboard => self.focus_visible = true,
            FocusCause::Pointer => self.focus_visible = false,
            FocusCause::Programmatic => {}
        }
        if let Some(previous) = self.focus.take() {
            previous.set_state(PseudoStates::FOCUS, false);
        }
        if let Some(node) = node {
            node.set_state(PseudoStates::FOCUS, true);
            node.set_tree_focus_visible(self.focus_visible);
            self.focus = Some(node.clone());
        }
    }

    /// Whether the focus ring is drawn.
    #[must_use]
    pub fn focus_visible(&self) -> bool {
        self.focus_visible
    }

    /// Feed every key event, pressed and released.
    ///
    /// GTK's rule (`_gtk_window_update_focus_visible`), not CSS's: a press
    /// remembers the focus; the matching release clears the flag only if the
    /// focus did not move while the key was down, and sets it otherwise --
    /// so `Tab` shows the ring and typing a letter hides it. `Alt` alone
    /// forces it on, which is how a menu mnemonic reveals itself.
    pub fn note_key(&mut self, ev: &KeyEvent) {
        if matches!(ev.keysym, xkb::Keysym::Alt_L | xkb::Keysym::Alt_R) {
            self.focus_visible = true;
            self.apply_visible();
            return;
        }
        if ev.pressed {
            self.key_focus = Some(self.focus.clone());
            return;
        }
        let Some(remembered) = self.key_focus.take() else {
            return;
        };
        let moved = match (&remembered, &self.focus) {
            (None, None) => false,
            (Some(a), Some(b)) => !a.ptr_eq(b),
            _ => true,
        };
        self.focus_visible = moved;
        self.apply_visible();
    }

    pub fn set_default(&mut self, node: Option<&Node>) {
        self.default = node.cloned();
    }

    #[must_use]
    pub fn default(&self) -> Option<Node> {
        self.default.clone()
    }

    /// Push the policy onto the tree the focus owner belongs to.
    fn apply_visible(&self) {
        if let Some(focus) = &self.focus {
            focus.set_tree_focus_visible(self.focus_visible);
        }
    }
}

/// The next focus from `from` in `dir`, recursing into containers.
///
/// `None` at the end of the ring: wrapping is the caller's policy, because a
/// popup's ring pops to its parent's instead of wrapping.
#[must_use]
pub fn navigate(
    root: &Node,
    tree: &LayoutTree,
    from: Option<&Node>,
    dir: FocusDirection,
) -> Option<Node> {
    let mut ring = Vec::new();
    collect(root, tree, dir, &mut ring, 0);
    let Some(from) = from else {
        return ring.into_iter().next();
    };
    match ring.iter().position(|node| node.ptr_eq(from)) {
        // The current focus is no longer in the ring (it was removed, or
        // disabled): start over rather than trapping focus.
        None => ring.into_iter().next(),
        Some(index) => ring.into_iter().nth(index + 1),
    }
}

/// Every focus candidate under `parent`, in `dir` order, depth first.
fn collect(
    parent: &Node,
    tree: &LayoutTree,
    dir: FocusDirection,
    out: &mut Vec<Node>,
    depth: usize,
) {
    if depth >= MAX_FOCUS_DEPTH {
        tracing::warn!(depth, "focus walk stopped at the depth guard");
        return;
    }
    for child in focus_sort(parent, tree, dir) {
        if is_focusable(&child, tree) {
            out.push(child.clone());
        }
        collect(&child, tree, dir, out, depth + 1);
    }
}

/// A window-level key binding, applied before the focused node sees the event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Binding {
    /// Move the focus.
    Move(FocusDirection),
    /// Activate the focused node (`Space`).
    Activate,
    /// Activate the window's default (`Return`).
    ActivateDefault,
    /// `Escape`: a popup destroys itself client-side; a window ignores it.
    Dismiss,
}

/// The binding `ev` triggers, if any.
///
/// Contract §3.5's table exactly. `Ctrl` is allowed on the navigation keys
/// (GTK's "move focus out of this container" variants) and ignored; `Alt` and
/// `Logo` are not -- those belong to accelerators and to the compositor.
/// `Caps`/`Num` are masked out: `Tab` with Caps Lock on is still `Tab`.
#[must_use]
pub fn window_binding(ev: &KeyEvent) -> Option<Binding> {
    use FocusDirection::{Down, Left, Right, TabBackward, TabForward, Up};

    if !ev.pressed {
        return None;
    }
    let mods = ev.effective_mods().difference(Mods::CAPS | Mods::NUM);
    if mods.intersects(Mods::ALT | Mods::LOGO) {
        return None;
    }
    // `Shift+Tab` is `ISO_Left_Tab` on most layouts, but not all -- and where
    // it is, the Shift was consumed, so `effective_mods` no longer shows it.
    // Both spellings mean the same thing.
    let shifted = ev.mods.contains(Mods::SHIFT);
    let binding = match ev.keysym {
        xkb::Keysym::ISO_Left_Tab => Binding::Move(TabBackward),
        xkb::Keysym::Tab | xkb::Keysym::KP_Tab => {
            Binding::Move(if shifted { TabBackward } else { TabForward })
        }
        xkb::Keysym::Up | xkb::Keysym::KP_Up => Binding::Move(Up),
        xkb::Keysym::Down | xkb::Keysym::KP_Down => Binding::Move(Down),
        xkb::Keysym::Left | xkb::Keysym::KP_Left => Binding::Move(Left),
        xkb::Keysym::Right | xkb::Keysym::KP_Right => Binding::Move(Right),
        xkb::Keysym::space | xkb::Keysym::KP_Space => Binding::Activate,
        xkb::Keysym::Return | xkb::Keysym::ISO_Enter | xkb::Keysym::KP_Enter => {
            Binding::ActivateDefault
        }
        xkb::Keysym::Escape => Binding::Dismiss,
        _ => return None,
    };
    if mods.contains(Mods::CTRL) && !matches!(binding, Binding::Move(_)) {
        // `Ctrl+Space` and `Ctrl+Return` are widget bindings (toggling a
        // selection, inserting a newline), not window ones.
        return None;
    }
    Some(binding)
}

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

    /// Re-run the style/layout pass after mutating the fixture's tree.
    fn relayout(grid: &mut Grid) {
        let sheet = CompiledSheet::compile(GRID_CSS);
        let env = ResolveEnv::default();
        let mut measure = FixedMeasure(taffy::Size {
            width: 0.0,
            height: 0.0,
        });
        restyle(
            &grid.root,
            &sheet,
            &env,
            &mut grid.styles,
            &mut grid.tree,
            taffy::Size {
                width: taffy::AvailableSpace::Definite(300.0),
                height: taffy::AvailableSpace::Definite(160.0),
            },
            &mut measure,
        )
        .expect("the grid re-lays out");
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

    use super::{Binding, FocusCause, FocusRing, navigate, window_binding};
    use crate::window::keyboard::{KeyEvent, Mods};
    use xkbcommon::xkb;

    fn key(keysym: xkb::Keysym, mods: Mods, consumed: Mods, pressed: bool) -> KeyEvent {
        KeyEvent {
            keycode: 0,
            keysym,
            utf8: None,
            mods,
            consumed,
            pressed,
            repeat: false,
            serial: 0,
            time_ms: 0,
        }
    }

    #[test]
    fn navigate_walks_the_geometric_ring_and_stops_at_its_end() {
        let grid = grid();
        let order = ["b", "a", "d", "c"];
        let mut current: Option<Node> = None;
        for expected in order {
            let next = navigate(
                &grid.root,
                &grid.tree,
                current.as_ref(),
                FocusDirection::TabForward,
            )
            .expect("another candidate");
            assert_eq!(next.id().expect("an id").as_str(), expected);
            current = Some(next);
        }
        assert!(
            navigate(
                &grid.root,
                &grid.tree,
                current.as_ref(),
                FocusDirection::TabForward
            )
            .is_none(),
            "the end of the ring is None; wrapping is the caller's policy"
        );
        // Backwards from the last is the reverse walk.
        assert_eq!(
            navigate(
                &grid.root,
                &grid.tree,
                current.as_ref(),
                FocusDirection::TabBackward
            )
            .expect("a candidate")
            .id()
            .expect("an id")
            .as_str(),
            "d"
        );
    }

    #[test]
    fn navigate_descends_into_containers_and_skips_non_candidates() {
        let grid = grid();
        // A nested box holding one focusable button, inserted first in tree
        // order but laid out last (it has no width until it has a child).
        let nested = Node::new("box");
        let inner = Node::with_classes("button", &[FOCUSABLE_CLASS]);
        inner.set_id(Some("inner"));
        nested.append_child(&inner);
        grid.container.append_child(&nested);
        let mut grid = grid;
        relayout(&mut grid);
        let all: Vec<String> = std::iter::successors(
            navigate(&grid.root, &grid.tree, None, FocusDirection::TabForward),
            |from| {
                navigate(
                    &grid.root,
                    &grid.tree,
                    Some(from),
                    FocusDirection::TabForward,
                )
            },
        )
        .map(|n| {
            n.id()
                .map_or_else(String::new, |id| id.as_str().to_string())
        })
        .collect();
        assert!(
            all.contains(&"inner".to_string()),
            "a nested candidate is reached: {all:?}"
        );
        assert!(
            !all.iter().any(String::is_empty),
            "the containers themselves are not in the ring: {all:?}"
        );
    }

    #[test]
    fn setting_focus_moves_the_pseudo_state() {
        let grid = grid();
        let children = grid.container.children();
        let (first, second) = (children[0].clone(), children[1].clone());
        let mut ring = <FocusRing as Default>::default();
        assert!(ring.focus().is_none());

        ring.set_focus(Some(&first), FocusCause::Keyboard);
        assert!(first.states().contains(PseudoStates::FOCUS));
        assert!(
            grid.container.states().contains(PseudoStates::FOCUS_WITHIN),
            "M2 derives :focus-within up the chain"
        );

        ring.set_focus(Some(&second), FocusCause::Keyboard);
        assert!(
            !first.states().contains(PseudoStates::FOCUS),
            "the old owner lost it"
        );
        assert!(second.states().contains(PseudoStates::FOCUS));

        ring.set_focus(None, FocusCause::Programmatic);
        assert!(!second.states().contains(PseudoStates::FOCUS));
        assert!(!grid.container.states().contains(PseudoStates::FOCUS_WITHIN));
    }

    #[test]
    fn focus_visible_follows_gtks_rule_and_reaches_the_selector() {
        // Ruling R3, and contract deviation 11: the flag has to be visible to
        // the cascade or `:focus-visible` in Adwaita is a no-op.
        let grid = grid();
        let children = grid.container.children();
        let (first, second) = (children[0].clone(), children[1].clone());
        let mut ring = <FocusRing as Default>::default();
        assert!(ring.focus_visible(), "GTK defaults to visible");

        // A pointer click focuses without a ring.
        ring.set_focus(Some(&first), FocusCause::Pointer);
        assert!(!ring.focus_visible());
        assert!(first.states().contains(PseudoStates::FOCUS));
        assert!(
            !first.states().contains(PseudoStates::FOCUS_VISIBLE),
            "a click must not draw the focus ring"
        );

        // A key press that moves the focus turns it back on.
        ring.note_key(&key(xkb::Keysym::Tab, Mods::empty(), Mods::empty(), true));
        ring.set_focus(Some(&second), FocusCause::Keyboard);
        ring.note_key(&key(xkb::Keysym::Tab, Mods::empty(), Mods::empty(), false));
        assert!(ring.focus_visible());
        assert!(second.states().contains(PseudoStates::FOCUS_VISIBLE));

        // A key press that does not move it turns it off again.
        ring.note_key(&key(xkb::Keysym::a, Mods::empty(), Mods::empty(), true));
        ring.note_key(&key(xkb::Keysym::a, Mods::empty(), Mods::empty(), false));
        assert!(!ring.focus_visible(), "typing into a widget hides the ring");
        assert!(!second.states().contains(PseudoStates::FOCUS_VISIBLE));

        // Alt alone forces it on, press or release.
        ring.note_key(&key(xkb::Keysym::Alt_L, Mods::empty(), Mods::empty(), true));
        assert!(ring.focus_visible());
        assert!(second.states().contains(PseudoStates::FOCUS_VISIBLE));
    }

    #[test]
    fn the_window_default_is_separate_from_the_focus() {
        let grid = grid();
        let children = grid.container.children();
        let mut ring = <FocusRing as Default>::default();
        ring.set_default(Some(&children[2]));
        ring.set_focus(Some(&children[0]), FocusCause::Keyboard);
        assert!(ring.default().expect("a default").ptr_eq(&children[2]));
        assert!(ring.focus().expect("a focus").ptr_eq(&children[0]));
        ring.set_default(None);
        assert!(ring.default().is_none());
    }

    #[test]
    fn the_window_bindings_are_exactly_the_contracts_table() {
        use FocusDirection::{Down, Left, Right, TabBackward, TabForward, Up};
        let plain = Mods::empty();
        let cases: Vec<(xkb::Keysym, Mods, Mods, Option<Binding>)> = vec![
            (
                xkb::Keysym::Tab,
                plain,
                plain,
                Some(Binding::Move(TabForward)),
            ),
            (
                xkb::Keysym::KP_Tab,
                plain,
                plain,
                Some(Binding::Move(TabForward)),
            ),
            (
                xkb::Keysym::Tab,
                Mods::CTRL,
                plain,
                Some(Binding::Move(TabForward)),
            ),
            (
                xkb::Keysym::ISO_Left_Tab,
                Mods::SHIFT,
                Mods::SHIFT,
                Some(Binding::Move(TabBackward)),
            ),
            (
                xkb::Keysym::Tab,
                Mods::SHIFT,
                plain,
                Some(Binding::Move(TabBackward)),
            ),
            (xkb::Keysym::Up, plain, plain, Some(Binding::Move(Up))),
            (
                xkb::Keysym::KP_Up,
                Mods::CTRL,
                plain,
                Some(Binding::Move(Up)),
            ),
            (xkb::Keysym::Down, plain, plain, Some(Binding::Move(Down))),
            (xkb::Keysym::Left, plain, plain, Some(Binding::Move(Left))),
            (
                xkb::Keysym::KP_Right,
                plain,
                plain,
                Some(Binding::Move(Right)),
            ),
            (xkb::Keysym::space, plain, plain, Some(Binding::Activate)),
            (xkb::Keysym::KP_Space, plain, plain, Some(Binding::Activate)),
            (
                xkb::Keysym::Return,
                plain,
                plain,
                Some(Binding::ActivateDefault),
            ),
            (
                xkb::Keysym::ISO_Enter,
                plain,
                plain,
                Some(Binding::ActivateDefault),
            ),
            (
                xkb::Keysym::KP_Enter,
                plain,
                plain,
                Some(Binding::ActivateDefault),
            ),
            (xkb::Keysym::Escape, plain, plain, Some(Binding::Dismiss)),
            // Not bound: the letters, and anything with Alt or Logo held --
            // those belong to accelerators and to the compositor.
            (xkb::Keysym::a, plain, plain, None),
            (xkb::Keysym::Tab, Mods::ALT, plain, None),
            (xkb::Keysym::Up, Mods::LOGO, plain, None),
            (xkb::Keysym::F1, plain, plain, None),
        ];
        for (keysym, mods, consumed, expected) in cases {
            let ev = key(keysym, mods, consumed, true);
            assert_eq!(window_binding(&ev), expected, "{keysym:?} with {mods:?}");
            let released = key(keysym, mods, consumed, false);
            assert_eq!(window_binding(&released), None, "a release binds nothing");
        }
        // Caps and Num lock are ignored: Tab with Caps on is still Tab.
        let with_locks = key(xkb::Keysym::Tab, Mods::CAPS | Mods::NUM, plain, true);
        assert_eq!(window_binding(&with_locks), Some(Binding::Move(TabForward)));
    }
}
