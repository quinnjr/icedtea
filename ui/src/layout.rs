//! GTK's measure→allocate box model over a `css::node::Node` tree,
//! expressed as a `taffy` tree.
//!
//! # `min-width`/`min-height` are content-box minimums
//!
//! GTK's `min-width`/`min-height` floor the widget's *content*, exactly as
//! `box-sizing: content-box` says they should; CSS's own `min-width` on a
//! `border-box` element does not. taffy resolves `min_size` in the box the
//! node's `box_sizing` names and adds padding+border when that box is the
//! content box (`taffy-0.14.0/src/compute/flexbox.rs:244`), so this module
//! sets `BoxSizing::ContentBox` and passes GTK's minimums straight through.
//!
//! Passing them through as border-box minimums makes every widget that is
//! floored by its minimum too small by exactly its padding plus border:
//! Adwaita's "Click me" came out 27 px high against GTK 4.22's 34, and an
//! empty button 20x27 against 36x34.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use selectors::Element as _;
use selectors::OpaqueElement;
use taffy::prelude::{Style, TaffyTree};

use crate::css::computed::{ComputedStyle, ResolveEnv};
use crate::css::node::Node;
use crate::css::value::Keyword;

/// An axis-aligned box in absolute, tree-origin coordinates.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Rect {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Width; never negative for a rect this module produces.
    pub width: f32,
    /// Height; never negative for a rect this module produces.
    pub height: f32,
}

impl Rect {
    /// A rect from its origin and size.
    #[must_use]
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// The empty rect at the origin.
    #[must_use]
    pub const fn zero() -> Self {
        Self::new(0.0, 0.0, 0.0, 0.0)
    }

    /// Right edge.
    #[must_use]
    pub fn right(&self) -> f32 {
        self.x + self.width
    }

    /// Bottom edge.
    #[must_use]
    pub fn bottom(&self) -> f32 {
        self.y + self.height
    }

    /// Whether either dimension is zero or smaller (or not a number).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        !(self.width > 0.0 && self.height > 0.0)
    }

    /// Shrink by `sides` (top, right, bottom, left), clamped at zero size.
    #[must_use]
    pub fn inset(&self, sides: [f32; 4]) -> Self {
        let [top, right, bottom, left] = sides;
        Self {
            x: self.x + left,
            y: self.y + top,
            width: (self.width - left - right).max(0.0),
            height: (self.height - top - bottom).max(0.0),
        }
    }

    /// Grow by `sides` (top, right, bottom, left).
    #[must_use]
    pub fn outset(&self, sides: [f32; 4]) -> Self {
        let [top, right, bottom, left] = sides;
        Self {
            x: self.x - left,
            y: self.y - top,
            width: (self.width + left + right).max(0.0),
            height: (self.height + top + bottom).max(0.0),
        }
    }

    /// The same box as a `skia-rs` edge-form rect.
    #[must_use]
    pub fn to_skia(&self) -> skia_rs_safe::core::Rect {
        skia_rs_safe::core::Rect::from_xywh(self.x, self.y, self.width, self.height)
    }
}

/// One node's laid-out geometry, in absolute tree-origin coordinates.
///
/// Replaces M1's `{width, height, label_x, label_y}`: a generic node has no
/// "label", and per-side border and padding are first-class in M2.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Allocation {
    /// The border box: what `background-clip: border-box` fills.
    pub border_box: Rect,
    /// The content box: where children and text start.
    pub content_box: Rect,
    /// Used border widths, top/right/bottom/left.
    pub border: [f32; 4],
    /// Used padding, top/right/bottom/left.
    pub padding: [f32; 4],
}

impl Allocation {
    /// The padding box: the border box minus the used border widths.
    #[must_use]
    pub fn padding_box(&self) -> Rect {
        self.border_box.inset(self.border)
    }

    /// The box a `background-origin`/`background-clip` keyword names.
    ///
    /// Any keyword without a box in M2 (`text`, and anything a future
    /// registry row adds) resolves to the border box, which is CSS's
    /// initial `background-clip` — never an empty box.
    #[must_use]
    pub fn box_for(&self, k: Keyword) -> Rect {
        match k {
            Keyword::PaddingBox => self.padding_box(),
            Keyword::ContentBox => self.content_box,
            _ => self.border_box,
        }
    }
}

/// The main axis of a box container.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BoxDirection {
    /// Children are laid out left to right.
    Row,
    /// Children are laid out top to bottom.
    Column,
}

/// How a node lays its children out.
///
/// M2 has exactly two: GTK's box, and a leaf whose size comes from a
/// [`Measure`]. Grid and centre layouts are M3.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Container {
    /// A flex container on `direction`, centring its children on both axes
    /// -- GTK's box default, and what M1's button relied on.
    Box {
        /// The main axis.
        direction: BoxDirection,
    },
    /// A childless node sized by its [`Measure`].
    Leaf,
}

impl Default for Container {
    fn default() -> Self {
        Self::Box {
            direction: BoxDirection::Column,
        }
    }
}

/// Why a layout call could not produce an answer.
#[derive(Debug)]
pub enum LayoutError {
    /// `taffy` rejected a tree operation.
    Taffy(taffy::TaffyError),
    /// [`LayoutTree::compute`] was called for a node this tree has never
    /// seen -- call [`LayoutTree::sync`] first.
    Unsynced,
}

impl std::fmt::Display for LayoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Taffy(err) => write!(f, "taffy: {err}"),
            Self::Unsynced => write!(f, "node is not in the layout tree"),
        }
    }
}

impl std::error::Error for LayoutError {}

impl From<taffy::TaffyError> for LayoutError {
    fn from(err: taffy::TaffyError) -> Self {
        Self::Taffy(err)
    }
}

/// What each taffy node carries so the measure closure can reach the CSS.
struct NodeCtx {
    node: Node,
    style: Rc<ComputedStyle>,
}

/// A `taffy` tree that mirrors one `css::node::Node` subtree.
///
/// The tree is kept across restyles rather than rebuilt: a restyle happens
/// on every `:hover`/`:active` transition, and taffy's own dirty tracking
/// (`set_style` self-marks dirty, `taffy-0.14.0/src/tree/taffy_tree.rs:826`)
/// only recomputes what changed.
pub struct LayoutTree {
    tree: TaffyTree<NodeCtx>,
    ids: HashMap<OpaqueElement, taffy::NodeId>,
    root: Option<taffy::NodeId>,
    synced_generation: Option<u64>,
    env: ResolveEnv,
}

impl Default for LayoutTree {
    fn default() -> Self {
        Self::new()
    }
}

impl LayoutTree {
    /// An empty tree.
    #[must_use]
    pub fn new() -> Self {
        Self {
            tree: TaffyTree::new(),
            ids: HashMap::new(),
            root: None,
            synced_generation: None,
            env: ResolveEnv::default(),
        }
    }

    /// How many taffy nodes the tree currently holds.
    #[must_use]
    pub fn node_count(&self) -> usize {
        self.tree.total_node_count()
    }

    /// Whether [`sync`](Self::sync) would be a no-op for `root` right now.
    #[must_use]
    pub fn is_synced(&self, root: &Node) -> bool {
        self.synced_generation == Some(root.generation())
    }

    /// Mirror `root`'s subtree into taffy, creating, reparenting and
    /// removing nodes as needed.
    ///
    /// Idempotent, and a no-op while the tree generation has not moved.
    /// Styles set by [`set_style`](Self::set_style) survive a sync for every
    /// node that survives it.
    ///
    /// # Errors
    ///
    /// [`LayoutError::Taffy`] if `taffy` rejects a node insertion or a
    /// child-list update.
    pub fn sync(&mut self, root: &Node) -> Result<(), LayoutError> {
        if self.is_synced(root) {
            return Ok(());
        }

        let mut live: HashSet<OpaqueElement> = HashSet::new();
        let root_id = self.sync_node(root, &mut live)?;
        self.root = Some(root_id);

        let stale: Vec<(OpaqueElement, taffy::NodeId)> = self
            .ids
            .iter()
            .filter(|(key, _)| !live.contains(*key))
            .map(|(key, id)| (*key, *id))
            .collect();
        for (key, id) in stale {
            self.ids.remove(&key);
            self.tree.remove(id)?;
        }

        self.synced_generation = Some(root.generation());
        Ok(())
    }

    /// Ensure `node` and its whole subtree exist, and that `node`'s taffy
    /// children match its CSS children in order.
    fn sync_node(
        &mut self,
        node: &Node,
        live: &mut HashSet<OpaqueElement>,
    ) -> Result<taffy::NodeId, LayoutError> {
        let key = node.opaque();
        live.insert(key);

        let id = match self.ids.get(&key) {
            Some(id) => *id,
            None => {
                let ctx = NodeCtx {
                    node: node.clone(),
                    style: ComputedStyle::initial(&self.env),
                };
                let id = self.tree.new_leaf_with_context(Style::default(), ctx)?;
                self.ids.insert(key, id);
                id
            }
        };

        let mut children = Vec::with_capacity(node.child_count());
        for child in node.children() {
            children.push(self.sync_node(&child, live)?);
        }
        if self.tree.children(id)? != children {
            self.tree.set_children(id, &children)?;
        }
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::{Allocation, Rect};
    use crate::css::value::Keyword;

    /// A 100x50 border box with a 2px border and 4/6/4/6 padding.
    fn alloc() -> Allocation {
        Allocation {
            border_box: Rect::new(10.0, 20.0, 100.0, 50.0),
            content_box: Rect::new(18.0, 26.0, 84.0, 38.0),
            border: [2.0, 2.0, 2.0, 2.0],
            padding: [4.0, 6.0, 4.0, 6.0],
        }
    }

    #[test]
    fn inset_subtracts_trbl_and_never_goes_negative() {
        // Mutation check: swapping any two of the TRBL indices, or dropping
        // the `.max(0.0)` clamp, changes one of these three numbers.
        let r = Rect::new(0.0, 0.0, 20.0, 10.0).inset([1.0, 2.0, 3.0, 4.0]);
        assert_eq!(r, Rect::new(4.0, 1.0, 14.0, 6.0));
        let collapsed = Rect::new(0.0, 0.0, 4.0, 4.0).inset([9.0, 9.0, 9.0, 9.0]);
        assert_eq!(collapsed.width, 0.0);
        assert_eq!(collapsed.height, 0.0);
    }

    #[test]
    fn box_for_walks_border_padding_content_inwards() {
        // Mutation check: returning `border_box` for `PaddingBox` (the easy
        // mistake, since GTK's background-origin default is padding-box)
        // moves the second assertion by the 2px border.
        let a = alloc();
        assert_eq!(
            a.box_for(Keyword::BorderBox),
            Rect::new(10.0, 20.0, 100.0, 50.0)
        );
        assert_eq!(
            a.box_for(Keyword::PaddingBox),
            Rect::new(12.0, 22.0, 96.0, 46.0)
        );
        assert_eq!(
            a.box_for(Keyword::ContentBox),
            Rect::new(18.0, 26.0, 84.0, 38.0)
        );
        assert_eq!(a.padding_box(), a.box_for(Keyword::PaddingBox));
    }

    #[test]
    fn box_for_falls_back_to_the_border_box_for_any_other_keyword() {
        // `background-clip: text` is parsed by the registry but has no box
        // in M2; it must not panic and must not silently clip to nothing.
        assert_eq!(alloc().box_for(Keyword::TextBox), alloc().border_box);
    }

    #[test]
    fn rect_geometry_never_panics_on_hostile_numbers() {
        for &v in &[
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            -1.0e30,
            1.0e30,
            0.0,
            -0.0,
        ] {
            let r = Rect::new(v, v, v, v);
            let _ = r.right();
            let _ = r.bottom();
            let _ = r.is_empty();
            let _ = r.inset([v, v, v, v]);
            let _ = r.outset([v, v, v, v]);
            let _ = r.to_skia();
        }
    }

    use super::LayoutTree;
    use crate::css::node::Node;

    /// `window > box > (label, label)`.
    fn small_tree() -> (Node, Node, Node, Node) {
        let window = Node::new("window");
        let container = Node::new("box");
        let a = Node::new("label");
        let b = Node::new("label");
        window.append_child(&container);
        container.append_child(&a);
        container.append_child(&b);
        (window, container, a, b)
    }

    #[test]
    fn sync_creates_one_taffy_node_per_css_node() {
        // Mutation check: recursing only into the first child, or forgetting
        // the root itself, drops the count below 4.
        let (window, _c, _a, _b) = small_tree();
        let mut tree = LayoutTree::new();
        tree.sync(&window).expect("sync");
        assert_eq!(tree.node_count(), 4);
    }

    #[test]
    fn sync_is_a_no_op_while_the_generation_is_unchanged() {
        // Mutation check: dropping the generation memo makes `is_synced`
        // false immediately after a sync, and re-syncs on every frame.
        let (window, _c, _a, _b) = small_tree();
        let mut tree = LayoutTree::new();
        tree.sync(&window).expect("sync");
        assert!(tree.is_synced(&window));
        tree.sync(&window).expect("second sync");
        assert_eq!(tree.node_count(), 4);
    }

    #[test]
    fn sync_adds_and_removes_nodes_when_the_tree_changes() {
        // Mutation check: never calling `taffy.remove` leaves the count at 5
        // after the removal, so stale nodes would keep taking part in layout.
        let (window, container, a, _b) = small_tree();
        let mut tree = LayoutTree::new();
        tree.sync(&window).expect("sync");

        let c = Node::new("label");
        container.append_child(&c);
        assert!(!tree.is_synced(&window), "a mutation must dirty the memo");
        tree.sync(&window).expect("resync");
        assert_eq!(tree.node_count(), 5);

        container.remove_child(&a);
        tree.sync(&window).expect("resync after removal");
        assert_eq!(tree.node_count(), 4);
    }
}
