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

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use selectors::Element as _;
use selectors::OpaqueElement;
use taffy::prelude::{
    AlignItems, AlignSelf, BoxSizing, Dimension, Display, FlexDirection, JustifyContent,
    JustifyItems, LengthPercentage, LengthPercentageAuto, Position, Size, Style, TaffyAuto as _,
    TaffyTree, auto, fr, length, line, percent, span,
};

use crate::css::computed::{ComputedStyle, ResolveEnv};
use crate::css::node::Node;
use crate::css::registry::Prop;
use crate::css::value::{Keyword, Value};

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

/// GTK's per-child alignment (`halign`/`valign`).
///
/// P6 (contract §3.6) makes `LayoutTree::set_style` read this through a
/// `ChildLayout`; P4 needs only the enum, because `view::Prop::Align` and
/// `View::halign`/`View::valign` carry it (contract deviation D8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Align {
    /// Fill the whole allocation — GTK's default.
    #[default]
    Fill,
    /// Pack at the start edge (left under LTR).
    Start,
    /// Pack at the end edge.
    End,
    /// Centre in the allocation.
    Center,
    /// Align on the first baseline; falls back to `Start` where there is none.
    Baseline,
}

/// How a node lays its children out.
///
/// M2 had two variants; M3 adds GTK's grid and centre-box layouts, which
/// the M2 doc comment on this enum deferred to this milestone.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Container {
    /// A flex container on `direction`. With no per-child [`ChildLayout`]
    /// it centres its children on both axes -- GTK's box default, and what
    /// M1's button relied on.
    Box {
        /// The main axis.
        direction: BoxDirection,
    },
    /// `GtkGrid`: a fixed track grid. `columns`/`rows` are template bounds
    /// (clamped to `1..=1024`); children with a [`GridPlacement`] are pinned
    /// and the rest are auto-placed.
    Grid {
        /// Column tracks.
        columns: u16,
        /// Row tracks.
        rows: u16,
        /// Gap between columns, px.
        column_spacing: f32,
        /// Gap between rows, px.
        row_spacing: f32,
        /// Every column track the same width.
        column_homogeneous: bool,
        /// Every row track the same height.
        row_homogeneous: bool,
    },
    /// `GtkCenterBox`: exactly three children, the middle one centred in the
    /// whole allocation. The *first* child is allocated per text direction
    /// (`gtk/gtkcenterbox.c:45`).
    Center {
        /// The main axis.
        direction: BoxDirection,
        /// The centre child shrinks after the outer two, not before.
        shrink_center_last: bool,
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

/// Per-child placement, as GTK's `GtkWidget` expresses it.
///
/// A node this tree has never been given a `ChildLayout` for is not
/// `Align::Fill`, it is *unaligned*, and keeps the container's own centring.
/// See [`LayoutTree::set_child_layout`].
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ChildLayout {
    /// Horizontal alignment.
    pub halign: Align,
    /// Vertical alignment.
    pub valign: Align,
    /// Absorb extra horizontal space.
    pub hexpand: bool,
    /// Absorb extra vertical space.
    pub vexpand: bool,
    /// Outer margin, top/right/bottom/left, px.
    pub margin: [f32; 4],
    /// Explicit grid cell, when the parent is a [`Container::Grid`].
    pub grid: Option<GridPlacement>,
    /// Out of flow, positioned by `halign`/`valign` alone over the whole
    /// containing block instead of sharing a track's sizing with its
    /// siblings -- `GtkOverlay`'s overlay children, which must not widen a
    /// grid row/column the way an ordinary same-cell sibling would.
    pub absolute: bool,
}

/// One child's cell in a [`Container::Grid`], zero-based.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridPlacement {
    /// Zero-based column.
    pub column: u16,
    /// Zero-based row.
    pub row: u16,
    /// Columns covered; `0` is read as `1`.
    pub column_span: u16,
    /// Rows covered; `0` is read as `1`.
    pub row_span: u16,
}

/// `v` if it is finite, else `0.0`. Every geometry number that reaches taffy
/// from a `Props` value passes through here.
fn finite(v: f32) -> f32 {
    if v.is_finite() { v } else { 0.0 }
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
    /// The container this node *is*.
    container: Container,
    /// How this node is placed inside its parent. `None` -- the M2 state --
    /// means "unaligned": the parent's own centring decides, which is what
    /// keeps every M1/M2 number bit-identical.
    child: Option<ChildLayout>,
    /// A measure that overrides the tree-wide one for this node only.
    measure: Option<Rc<RefCell<dyn Measure>>>,
    /// The `ResolveEnv` the last `set_style` used, so `set_container` and
    /// `set_child_layout` can rewrite the taffy style without one.
    env: ResolveEnv,
    /// A floor on the container's `(column, row)` gap in px, from a widget's
    /// own gap property (`GtkBox:spacing`, …). CSS `border-spacing` still
    /// applies; the larger of the two wins on each axis, matching GtkBox.
    gap_floor: (f32, f32),
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
    /// `(root.tree_id(), root.generation())` as of the last successful
    /// [`sync`](Self::sync). Keyed on both, the same ABA fix as
    /// `css::select::MatchCx`: a generation counter alone can collide across
    /// two different `Node` trees (one torn down, a new one built, its
    /// internal generation counter happening to reach the same value the
    /// old tree was last synced at), which would make `is_synced` wrongly
    /// report the new, never-mirrored tree as already synced.
    synced_generation: Option<(u64, u64)>,
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
        self.synced_generation == Some((root.tree_id(), root.generation()))
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

        self.synced_generation = Some((root.tree_id(), root.generation()));
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
                    container: Container::default(),
                    child: None,
                    measure: None,
                    env: ResolveEnv::default(),
                    gap_floor: (0.0, 0.0),
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

    /// The taffy `Style` currently attached to `node`, if it is in the tree.
    ///
    /// Exposed for tests and for callers that want to inspect what the CSS
    /// box model turned into.
    #[must_use]
    pub fn taffy_style(&self, node: &Node) -> Option<&Style> {
        let id = *self.ids.get(&node.opaque())?;
        self.tree.style(id).ok()
    }

    /// Write one node's CSS box into taffy.
    ///
    /// `container` decides the display mode; `env` supplies the DPI and root
    /// font size that percentage and relative lengths resolve against.
    /// Nodes that are not in the tree are ignored: a widget may restyle
    /// before it is attached.
    ///
    /// `taffy::TaffyTree::set_style` marks the node (and its ancestors)
    /// dirty itself, so no `mark_dirty` call follows this one.
    pub fn set_style(
        &mut self,
        node: &Node,
        style: &ComputedStyle,
        container: Container,
        env: &ResolveEnv,
    ) {
        let Some(&id) = self.ids.get(&node.opaque()) else {
            tracing::debug!(node = %node.name(), "set_style on an unsynced node");
            return;
        };
        if let Some(ctx) = self.tree.get_node_context_mut(id) {
            ctx.style = Rc::new(style.clone());
            ctx.container = container;
            ctx.env = *env;
        }
        self.write_taffy_style(id);
    }

    /// Set the container without re-running the cascade.
    ///
    /// A widget controller knows what it *is* (`GtkGrid` is a grid) long
    /// before a restyle happens, so it can say so without a
    /// [`ComputedStyle`] in hand; the container is remembered on the node
    /// and re-applied by every later [`set_child_layout`](Self::set_child_layout).
    /// [`set_style`](Self::set_style) carries a container of its own and
    /// remains authoritative: a restyle that passes a different one wins,
    /// so a controller that restyles calls this again afterwards.
    pub fn set_container(&mut self, node: &Node, container: Container) {
        let Some(&id) = self.ids.get(&node.opaque()) else {
            tracing::debug!(node = %node.name(), "set_container on an unsynced node");
            return;
        };
        if let Some(ctx) = self.tree.get_node_context_mut(id) {
            ctx.container = container;
        }
        self.write_taffy_style(id);
    }

    /// Place `node` inside its parent.
    ///
    /// Calling this at all opts the node out of the container's blanket
    /// centring: from here on its alignment is exactly what `child` says.
    pub fn set_child_layout(&mut self, node: &Node, child: ChildLayout) {
        let Some(&id) = self.ids.get(&node.opaque()) else {
            tracing::debug!(node = %node.name(), "set_child_layout on an unsynced node");
            return;
        };
        if let Some(ctx) = self.tree.get_node_context_mut(id) {
            ctx.child = Some(child);
        }
        self.write_taffy_style(id);
    }

    /// Raise the floor on `node`'s container gap, on one axis, to `px`.
    ///
    /// A widget's own gap property (`GtkBox:spacing`, `GtkGrid`'s row/column
    /// spacing) and CSS `border-spacing` both apply; the larger wins, which
    /// is what GTK does when a theme sets both. `px` is clamped to
    /// non-negative and non-finite input floors at zero, so a hostile
    /// `Props` value can never poison the taffy style with NaN.
    pub fn set_gap_floor(&mut self, node: &Node, px: f32, horizontal: bool) {
        let Some(&id) = self.ids.get(&node.opaque()) else {
            tracing::debug!(node = %node.name(), "set_gap_floor on an unsynced node");
            return;
        };
        let px = if px.is_finite() { px.max(0.0) } else { 0.0 };
        if let Some(ctx) = self.tree.get_node_context_mut(id) {
            if horizontal {
                ctx.gap_floor.0 = px;
            } else {
                ctx.gap_floor.1 = px;
            }
        }
        self.write_taffy_style(id);
    }

    /// Give one node its own [`Measure`], overriding the one passed to
    /// [`compute`](Self::compute) for that node alone.
    ///
    /// `RefCell` and not a bare `Rc` because `Measure::measure` takes
    /// `&mut self` and a controller keeps its own handle to the same
    /// measurer (contract deviation 1).
    pub fn set_measure(&mut self, node: &Node, measure: Rc<RefCell<dyn Measure>>) {
        let Some(&id) = self.ids.get(&node.opaque()) else {
            tracing::debug!(node = %node.name(), "set_measure on an unsynced node");
            return;
        };
        if let Some(ctx) = self.tree.get_node_context_mut(id) {
            ctx.measure = Some(measure);
        }
        if let Err(err) = self.tree.mark_dirty(id) {
            tracing::debug!(%err, "taffy rejected a mark_dirty after set_measure");
        }
    }

    /// This node's placement inside its parent, or `None` if it was never
    /// given one.
    #[must_use]
    pub fn child_layout(&self, node: &Node) -> Option<ChildLayout> {
        let id = *self.ids.get(&node.opaque())?;
        self.tree.get_node_context(id).and_then(|ctx| ctx.child)
    }

    /// This node's container.
    #[must_use]
    pub fn container(&self, node: &Node) -> Container {
        self.ids
            .get(&node.opaque())
            .and_then(|id| self.tree.get_node_context(*id))
            .map_or(Container::default(), |ctx| ctx.container)
    }

    /// The display, axis, alignment and track half of one node's taffy style.
    fn container_style(
        container: Container,
        direction: crate::css::node::Direction,
        out: &mut Style,
    ) {
        /// GTK's grids are small; a `Props`-driven count is not trusted.
        fn tracks<S: taffy::style::CheapCloneStr>(
            n: u16,
            homogeneous: bool,
        ) -> Vec<taffy::style::GridTemplateComponent<S>> {
            let n = usize::from(n.clamp(1, 1024));
            let track = if homogeneous { fr(1.0) } else { auto() };
            vec![track; n]
        }
        let rtl = direction == crate::css::node::Direction::Rtl;
        match container {
            Container::Box { direction: axis } => {
                out.display = Display::Flex;
                out.flex_direction = match (axis, rtl) {
                    (BoxDirection::Row, false) => FlexDirection::Row,
                    (BoxDirection::Row, true) => FlexDirection::RowReverse,
                    (BoxDirection::Column, _) => FlexDirection::Column,
                };
                // GTK's box centres a child that asked for nothing; a child
                // that called `set_child_layout` overrides this per child.
                out.align_items = Some(AlignItems::CENTER);
                out.justify_content = Some(JustifyContent::CENTER);
            }
            Container::Center {
                direction: axis,
                shrink_center_last: _,
            } => {
                out.display = Display::Flex;
                out.flex_direction = match (axis, rtl) {
                    (BoxDirection::Row, false) => FlexDirection::Row,
                    (BoxDirection::Row, true) => FlexDirection::RowReverse,
                    (BoxDirection::Column, _) => FlexDirection::Column,
                };
                out.align_items = Some(AlignItems::CENTER);
                out.justify_content = Some(JustifyContent::SPACE_BETWEEN);
            }
            Container::Grid {
                columns,
                rows,
                column_spacing,
                row_spacing,
                column_homogeneous,
                row_homogeneous,
            } => {
                out.display = Display::Grid;
                out.grid_template_columns = tracks(columns, column_homogeneous);
                out.grid_template_rows = tracks(rows, row_homogeneous);
                out.gap = Size {
                    width: length(finite(column_spacing).max(0.0)),
                    height: length(finite(row_spacing).max(0.0)),
                };
                out.align_items = Some(AlignItems::CENTER);
                out.justify_items = Some(JustifyItems::CENTER);
            }
            Container::Leaf => {
                out.display = Display::Flex;
                out.flex_direction = FlexDirection::Row;
                out.align_items = Some(AlignItems::CENTER);
                out.justify_content = Some(JustifyContent::CENTER);
            }
        }
    }

    /// The per-child half: alignment, expansion, margin and grid cell.
    ///
    /// The parent's main axis has no per-child alignment in flexbox, so a
    /// main-axis [`Align`] becomes an auto margin on the free side -- the
    /// standard idiom, and exactly what GTK's own allocation does when a
    /// child is smaller than its cell. A grid parent has no "main axis" at
    /// all -- CSS Grid's `align-self`/`justify-self` are the block/inline
    /// axes directly, independent of any row/column concept -- so a grid
    /// child (Task 14's `Overlay`, stacking every child in one cell) skips
    /// the flex cross/main split and auto-margin idiom entirely.
    fn child_style(child: ChildLayout, parent_is_row: bool, parent_is_grid: bool, out: &mut Style) {
        fn to_self(a: Align) -> Option<AlignSelf> {
            Some(match a {
                Align::Fill => AlignSelf::STRETCH,
                Align::Start => AlignSelf::START,
                Align::End => AlignSelf::END,
                Align::Center => AlignSelf::CENTER,
                Align::Baseline => AlignSelf::BASELINE,
            })
        }
        if child.absolute {
            // Out of flow: every inset stays auto, so the item keeps its
            // own (measured) size and `align_self`/`justify_self` place it
            // within the whole containing block -- setting any inset to a
            // definite `0` would instead *stretch* it edge to edge on that
            // axis regardless of alignment, which is the one CSS pitfall
            // this idiom exists to avoid. Never shares track/flex-line
            // sizing with an in-flow sibling.
            out.position = Position::Absolute;
            out.align_self = to_self(child.valign);
            out.justify_self = to_self(child.halign);
        } else if parent_is_grid {
            out.align_self = to_self(child.valign);
            out.justify_self = to_self(child.halign);
            let [mt, mr, mb, ml] = child.margin.map(finite);
            out.margin = taffy::geometry::Rect {
                top: length(mt),
                right: length(mr),
                bottom: length(mb),
                left: length(ml),
            };
        } else {
            let (cross, main) = if parent_is_row {
                (child.valign, child.halign)
            } else {
                (child.halign, child.valign)
            };
            out.align_self = to_self(cross);
            let [mt, mr, mb, ml] = child.margin.map(finite);
            let (mut lead, mut trail) = (
                length(if parent_is_row { ml } else { mt }),
                length(if parent_is_row { mr } else { mb }),
            );
            match main {
                Align::Fill | Align::Baseline | Align::Start => {}
                Align::End => lead = auto(),
                Align::Center => {
                    lead = auto();
                    trail = auto();
                }
            }
            out.margin = taffy::geometry::Rect {
                top: if parent_is_row { length(mt) } else { lead },
                right: if parent_is_row { trail } else { length(mr) },
                bottom: if parent_is_row { length(mb) } else { trail },
                left: if parent_is_row { lead } else { length(ml) },
            };
            let expand = if parent_is_row {
                child.hexpand
            } else {
                child.vexpand
            };
            if expand {
                out.flex_grow = 1.0;
                out.flex_basis = Dimension::length(0.0);
            }
        }
        if let Some(g) = child.grid {
            let col = i16::try_from(g.column.min(1023)).unwrap_or(0) + 1;
            let row = i16::try_from(g.row.min(1023)).unwrap_or(0) + 1;
            out.grid_column = taffy::geometry::Line {
                start: line(col),
                end: span(g.column_span.max(1)),
            };
            out.grid_row = taffy::geometry::Line {
                start: line(row),
                end: span(g.row_span.max(1)),
            };
        }
    }

    /// Rebuild one node's taffy style from its stored style, container and
    /// child layout. The single writer; `set_style`, `set_container` and
    /// `set_child_layout` all funnel through it.
    fn write_taffy_style(&mut self, id: taffy::NodeId) {
        let Some(ctx) = self.tree.get_node_context(id) else {
            return;
        };
        let (node, style, container, child, env, gap_floor) = (
            ctx.node.clone(),
            Rc::clone(&ctx.style),
            ctx.container,
            ctx.child,
            ctx.env,
            ctx.gap_floor,
        );
        let mut taffy_style = Self::box_model_style(&style, &env, gap_floor);
        Self::container_style(container, node.direction(), &mut taffy_style);
        if let Some(child) = child {
            let parent_container = node.parent().map(|p| self.container(&p));
            let parent_is_row = matches!(
                parent_container,
                Some(
                    Container::Box {
                        direction: BoxDirection::Row
                    } | Container::Center {
                        direction: BoxDirection::Row,
                        ..
                    }
                )
            );
            let parent_is_grid = matches!(parent_container, Some(Container::Grid { .. }));
            Self::child_style(child, parent_is_row, parent_is_grid, &mut taffy_style);
        }
        if let Err(err) = self.tree.set_style(id, taffy_style) {
            tracing::debug!(node = %node.name(), %err, "taffy rejected a style");
        }
    }

    /// The CSS box model half of one node's taffy style: sizes, margins,
    /// padding, border and the container gap. M2's `set_style` body, verbatim,
    /// plus `gap_floor` (a widget's own gap property) raising the CSS
    /// `border-spacing` gap on either axis when it is larger.
    fn box_model_style(style: &ComputedStyle, env: &ResolveEnv, gap_floor: (f32, f32)) -> Style {
        // CSS resolves *every* percentage in the box model against the
        // containing block's inline size, and taffy is the only thing here
        // that knows what that is -- so a percentage must reach taffy *as* a
        // percentage. Resolving it eagerly against a hard-coded basis of 0.0
        // and handing over an absolute length collapsed `padding: 0 5%`,
        // `margin: 10%` and `min-width: 50%` to 0px. The basis below is used
        // only for the values that are *not* percentages, where it is
        // irrelevant.
        let basis = 0.0_f32;
        let [pad_top, pad_right, pad_bottom, pad_left] = style.padding(basis);
        let [bt, br, bb, bl] = style.border_widths();
        let margin = style.margin(basis);
        let (min_w, min_h) = style.min_size((basis, basis));

        /// The declared value's own percentage, when it has one.
        fn declared_percent(style: &ComputedStyle, prop: Prop) -> Option<f32> {
            match style.raw(prop) {
                Value::Length(crate::css::value::Length::Percent(fraction))
                    if fraction.is_finite() =>
                {
                    Some(*fraction)
                }
                _ => None,
            }
        }
        let len = |prop: Prop, px: f32| -> LengthPercentage {
            declared_percent(style, prop).map_or_else(|| length(px), percent)
        };
        let len_auto = |prop: Prop, px: Option<f32>| -> LengthPercentageAuto {
            match (px, declared_percent(style, prop)) {
                (None, _) => auto(),
                (Some(_), Some(fraction)) => percent(fraction),
                (Some(px), None) => length(px),
            }
        };
        let min = |prop: Prop, px: f32| -> LengthPercentageAuto {
            declared_percent(style, prop).map_or_else(|| length(px), percent)
        };
        let (gap_x, gap_y) = Self::border_spacing_px(style, env);
        let gap_x = gap_x.max(gap_floor.0);
        let gap_y = gap_y.max(gap_floor.1);

        Style {
            // GTK's min-width/min-height floor the CONTENT box.
            box_sizing: BoxSizing::ContentBox,
            size: Size {
                width: Dimension::AUTO,
                height: Dimension::AUTO,
            },
            min_size: Size {
                width: min(Prop::MinWidth, min_w),
                height: min(Prop::MinHeight, min_h),
            },
            margin: taffy::geometry::Rect {
                top: len_auto(Prop::MarginTop, margin[0]),
                right: len_auto(Prop::MarginRight, margin[1]),
                bottom: len_auto(Prop::MarginBottom, margin[2]),
                left: len_auto(Prop::MarginLeft, margin[3]),
            },
            padding: taffy::geometry::Rect {
                top: len(Prop::PaddingTop, pad_top),
                right: len(Prop::PaddingRight, pad_right),
                bottom: len(Prop::PaddingBottom, pad_bottom),
                left: len(Prop::PaddingLeft, pad_left),
            },
            border: taffy::geometry::Rect {
                top: length(bt),
                right: length(br),
                bottom: length(bb),
                left: length(bl),
            },
            gap: Size {
                width: length(gap_x),
                height: length(gap_y),
            },
            ..Style::default()
        }
    }

    /// `border-spacing` as `(column-gap, row-gap)` in px.
    ///
    /// The computed value is a `Value::Pair` of two lengths (a single value
    /// is stored duplicated by the registry's parser). Anything else -- an
    /// unresolvable calc, a wide keyword that survived -- is 0, matching the
    /// property's initial value.
    fn border_spacing_px(style: &ComputedStyle, env: &ResolveEnv) -> (f32, f32) {
        let ctx = style.length_ctx(env, None);
        let px = |v: &Value| -> f32 {
            match v {
                Value::Length(len) => len.resolve(&ctx).unwrap_or(0.0),
                Value::Number(n) if n.is_finite() => *n,
                _ => 0.0,
            }
        };
        match style.raw(Prop::BorderSpacing) {
            Value::Pair(pair) => (px(&pair.0), px(&pair.1)),
            other => {
                let v = px(other);
                (v, v)
            }
        }
    }
}

/// Intrinsic size for a leaf node.
///
/// `known` and `available` come straight from taffy: `known` carries the
/// dimensions already fixed by the parent, `available` the space offered.
pub trait Measure {
    /// Measure `node`.
    fn measure(
        &mut self,
        node: &Node,
        style: &ComputedStyle,
        known: taffy::Size<Option<f32>>,
        available: taffy::Size<taffy::AvailableSpace>,
    ) -> taffy::Size<f32>;
}

/// A [`Measure`] that reports the same size for every leaf.
///
/// Useful for layout tests and for trees whose leaves have no text.
pub struct FixedMeasure(pub taffy::Size<f32>);

impl Measure for FixedMeasure {
    fn measure(
        &mut self,
        _node: &Node,
        _style: &ComputedStyle,
        known: taffy::Size<Option<f32>>,
        _available: taffy::Size<taffy::AvailableSpace>,
    ) -> taffy::Size<f32> {
        taffy::Size {
            width: known.width.unwrap_or(self.0.width),
            height: known.height.unwrap_or(self.0.height),
        }
    }
}

impl LayoutTree {
    /// Lay the whole tree out.
    ///
    /// # Errors
    ///
    /// [`LayoutError::Unsynced`] if `root` is not in the tree, or
    /// [`LayoutError::Taffy`] if `taffy` fails.
    pub fn compute(
        &mut self,
        root: &Node,
        available: taffy::Size<taffy::AvailableSpace>,
        measure: &mut dyn Measure,
    ) -> Result<(), LayoutError> {
        let root_id = *self.ids.get(&root.opaque()).ok_or(LayoutError::Unsynced)?;
        // `taffy`'s own dirty tracking only fires from `set_style`: a leaf's
        // measured content can change (a new label, a new `Measure` impl)
        // with the node's `Style` untouched, and taffy's per-node layout
        // cache would then hand back the previous frame's answer. `compute`
        // is a whole-tree pass, so every node is dirtied unconditionally on
        // entry; `mark_dirty` short-circuits once a node is already dirty
        // (`taffy-0.14.0/src/tree/taffy_tree.rs:865`), so this stays cheap
        // for a tree that never reuses a stale measurement.
        for &id in self.ids.values() {
            self.tree.mark_dirty(id)?;
        }
        self.tree
            .compute_layout_with_measure(root_id, available, |inputs, _id, ctx, style| {
                taffy::compute_leaf_layout(
                    inputs,
                    style,
                    |_, _| 0.0,
                    |known, avail| match ctx {
                        Some(ctx) => match &ctx.measure {
                            Some(own) => own
                                .borrow_mut()
                                .measure(&ctx.node, &ctx.style, known, avail),
                            None => measure.measure(&ctx.node, &ctx.style, known, avail),
                        },
                        None => taffy::Size::ZERO,
                    },
                )
            })?;
        Ok(())
    }

    /// `node`'s allocation in absolute, tree-origin coordinates.
    ///
    /// taffy reports each node's `location` relative to its parent, so this
    /// walks the taffy parent chain and accumulates the offsets.
    #[must_use]
    pub fn allocation(&self, node: &Node) -> Option<Allocation> {
        let id = *self.ids.get(&node.opaque())?;
        let layout = self.tree.layout(id).ok()?;

        let (mut abs_x, mut abs_y) = (layout.location.x, layout.location.y);
        let mut cursor = id;
        while let Some(parent) = self.tree.parent(cursor) {
            let parent_layout = self.tree.layout(parent).ok()?;
            abs_x += parent_layout.location.x;
            abs_y += parent_layout.location.y;
            cursor = parent;
        }

        let border = [
            layout.border.top,
            layout.border.right,
            layout.border.bottom,
            layout.border.left,
        ];
        let padding = [
            layout.padding.top,
            layout.padding.right,
            layout.padding.bottom,
            layout.padding.left,
        ];
        let border_box = Rect::new(abs_x, abs_y, layout.size.width, layout.size.height);
        let content_box = border_box.inset([
            border[0] + padding[0],
            border[1] + padding[1],
            border[2] + padding[2],
            border[3] + padding[3],
        ]);

        Some(Allocation {
            border_box,
            content_box,
            border,
            padding,
        })
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
    fn is_synced_rejects_a_tree_id_mismatch_even_when_generation_collides() {
        // Regression test for the same ABA hazard `css::select::MatchCx`
        // guards against with its own `(tree_id, ...)` key: keying
        // `is_synced` on generation alone would let a dropped tree's
        // generation-counter value be "matched" by an unrelated later tree
        // that happens to read the same generation (every fresh, untouched
        // tree starts at the same low generation, so this is not exotic),
        // wrongly treating the new tree as already mirrored into taffy and
        // silently skipping its sync.
        let (window_b, _c, _a, _b) = small_tree();
        let mut tree = LayoutTree::new();

        // Forge the exact state such a collision would leave behind: the
        // real, current generation of `window_b` is on file, but under a
        // foreign tree id (`0` is never issued -- `NEXT_TREE_ID` starts at
        // 1 and only grows) that does not belong to `window_b`'s tree.
        tree.synced_generation = Some((0, window_b.generation()));

        assert!(
            !tree.is_synced(&window_b),
            "a matching generation under the wrong tree id must not read as synced -- \
             without tree_id in the key this new tree would wrongly be treated as \
             already mirrored and never actually synced"
        );
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

    use super::{BoxDirection, Container};
    use crate::css::cascade::CompiledSheet;
    use crate::css::computed::{ComputedStyle, ResolveEnv};
    use crate::css::select::MatchCx;
    use taffy::prelude::{BoxSizing, Display, FlexDirection};

    /// Resolve `node` against `css` the way a real restyle would.
    fn style_of(css: &str, root: &Node, node: &Node) -> ComputedStyle {
        let sheet = CompiledSheet::compile(css);
        let mut cx = MatchCx::new();
        let _ = root;
        ComputedStyle::resolve_chain(&sheet, node, &ResolveEnv::default(), &mut cx)
    }

    #[test]
    fn set_style_writes_the_css_box_into_taffy_as_a_content_box() {
        // Mutation check: switching to `BoxSizing::BorderBox`, or dropping
        // any of the four padding/border sides, changes one of these
        // assertions -- and would reintroduce M1's 27px-tall button bug.
        let window = Node::new("window");
        let button = Node::new("button");
        window.append_child(&button);
        let css = "button { padding: 4px 9px; border: 1px solid #000; \
                   min-width: 16px; min-height: 24px }";
        let style = style_of(css, &window, &button);

        let mut tree = LayoutTree::new();
        tree.sync(&window).expect("sync");
        tree.set_style(
            &button,
            &style,
            Container::Box {
                direction: BoxDirection::Row,
            },
            &ResolveEnv::default(),
        );

        let taffy_style = tree.taffy_style(&button).expect("styled");
        assert_eq!(taffy_style.box_sizing, BoxSizing::ContentBox);
        assert_eq!(taffy_style.display, Display::Flex);
        assert_eq!(taffy_style.flex_direction, FlexDirection::Row);
        assert_eq!(taffy_style.padding.top, taffy::prelude::length(4.0));
        assert_eq!(taffy_style.padding.left, taffy::prelude::length(9.0));
        assert_eq!(taffy_style.border.top, taffy::prelude::length(1.0));
        assert_eq!(taffy_style.min_size.width, taffy::prelude::length(16.0));
        assert_eq!(taffy_style.min_size.height, taffy::prelude::length(24.0));
    }

    #[test]
    fn border_spacing_becomes_the_containers_gap() {
        // Mutation check: reading only the first half of the `border-spacing`
        // pair puts 3px in the row gap too, and the second assertion fails.
        let window = Node::new("window");
        let css = "window { border-spacing: 3px 7px }";
        let style = style_of(css, &window, &window);

        let mut tree = LayoutTree::new();
        tree.sync(&window).expect("sync");
        tree.set_style(
            &window,
            &style,
            Container::default(),
            &ResolveEnv::default(),
        );

        let taffy_style = tree.taffy_style(&window).expect("styled");
        assert_eq!(taffy_style.gap.width, taffy::prelude::length(3.0));
        assert_eq!(taffy_style.gap.height, taffy::prelude::length(7.0));
    }

    #[test]
    fn an_auto_margin_reaches_taffy_as_auto() {
        // Mutation check: mapping `None` to `length(0.0)` instead of `auto()`
        // silently disables centring for every `margin: auto` in a theme.
        let window = Node::new("window");
        let css = "window { margin-left: auto; margin-right: 5px }";
        let style = style_of(css, &window, &window);

        let mut tree = LayoutTree::new();
        tree.sync(&window).expect("sync");
        tree.set_style(
            &window,
            &style,
            Container::default(),
            &ResolveEnv::default(),
        );

        let taffy_style = tree.taffy_style(&window).expect("styled");
        assert_eq!(taffy_style.margin.left, taffy::prelude::auto());
        assert_eq!(taffy_style.margin.right, taffy::prelude::length(5.0));
    }

    #[test]
    fn set_style_on_an_unsynced_node_is_ignored_rather_than_panicking() {
        // A widget may restyle before it is attached; that must not abort.
        let orphan = Node::new("button");
        let style = ComputedStyle::initial(&ResolveEnv::default());
        let mut tree = LayoutTree::new();
        tree.set_style(&orphan, &style, Container::Leaf, &ResolveEnv::default());
        assert!(tree.taffy_style(&orphan).is_none());
    }

    use super::{FixedMeasure, Measure};
    use taffy::prelude::{AvailableSpace, Size};

    /// M1's Adwaita-like button CSS, as a stylesheet rather than a struct
    /// literal: `ComputedStyle`'s fields no longer exist.
    const ADWAITA_LIKE: &str = "button { padding: 4px 9px; \
        border: 1px solid #cdc7c2; border-radius: 5px; \
        min-width: 16px; min-height: 24px; font-size: 14px; \
        background-color: #dad6d2; color: #2e3436 }";

    /// Every `label` leaf measures `w` x `h`; everything else is zero.
    struct LabelSize(f32, f32);

    impl Measure for LabelSize {
        fn measure(
            &mut self,
            node: &Node,
            _style: &ComputedStyle,
            _known: Size<Option<f32>>,
            _available: Size<AvailableSpace>,
        ) -> Size<f32> {
            if &*node.name() == "label" {
                Size {
                    width: self.0,
                    height: self.1,
                }
            } else {
                Size::ZERO
            }
        }
    }

    /// `window > button > label`, laid out with the M1 button CSS.
    fn button_layout(css: &str, label: (f32, f32)) -> (LayoutTree, Node, Node) {
        let window = Node::new("window");
        let button = Node::new("button");
        let label_node = Node::new("label");
        window.append_child(&button);
        button.append_child(&label_node);

        let sheet = CompiledSheet::compile(css);
        let env = ResolveEnv::default();
        let mut cx = MatchCx::new();
        let button_style = ComputedStyle::resolve_chain(&sheet, &button, &env, &mut cx);
        let label_style = ComputedStyle::resolve_chain(&sheet, &label_node, &env, &mut cx);
        let window_style = ComputedStyle::resolve_chain(&sheet, &window, &env, &mut cx);

        let mut tree = LayoutTree::new();
        tree.sync(&window).expect("sync");
        tree.set_style(&window, &window_style, Container::default(), &env);
        tree.set_style(
            &button,
            &button_style,
            Container::Box {
                direction: BoxDirection::Row,
            },
            &env,
        );
        tree.set_style(&label_node, &label_style, Container::Leaf, &env);
        tree.compute(
            &window,
            Size {
                width: AvailableSpace::MaxContent,
                height: AvailableSpace::MaxContent,
            },
            &mut LabelSize(label.0, label.1),
        )
        .expect("compute");
        (tree, button, label_node)
    }

    #[test]
    fn allocation_is_text_plus_padding_plus_border() {
        // M1's number, preserved: content max(60, min-width 16) = 60, + 9 + 9
        // + 1 + 1 = 80; content max(18, min-height 24) = 24, + 4 + 4 + 1 + 1.
        // Mutation check: dropping BoxSizing::ContentBox gives 34 -> 27.
        let (tree, button, label) = button_layout(ADWAITA_LIKE, (60.0, 18.0));
        let a = tree.allocation(&button).expect("button allocation");
        assert_eq!(a.border_box.width, 80.0);
        assert_eq!(a.border_box.height, 34.0);
        let l = tree.allocation(&label).expect("label allocation");
        assert_eq!(
            l.border_box.x - a.border_box.x,
            10.0,
            "border 1 + padding-left 9"
        );
        assert_eq!(
            l.border_box.y - a.border_box.y,
            8.0,
            "border 1 + padding-top 4 + (24 - 18) / 2 centring"
        );
    }

    #[test]
    fn min_size_is_a_content_box_minimum_not_a_border_box_one() {
        // A2, M1's reviewer scenario, preserved: an empty Adwaita button is
        // 36x34, not 20x27.
        let (tree, button, _label) = button_layout(ADWAITA_LIKE, (0.0, 6.0));
        let a = tree.allocation(&button).expect("button allocation");
        assert_eq!(a.border_box.width, 36.0, "max(0, 16) + 9 + 9 + 1 + 1 == 36");
        assert_eq!(
            a.border_box.height, 34.0,
            "max(6, 24) + 4 + 4 + 1 + 1 == 34"
        );
    }

    #[test]
    fn an_intrinsic_size_above_the_minimum_still_wins() {
        // The clamp is `max`, not "always the minimum".
        let (tree, button, _label) = button_layout(ADWAITA_LIKE, (200.0, 40.0));
        let a = tree.allocation(&button).expect("button allocation");
        assert_eq!(a.border_box.width, 220.0);
        assert_eq!(a.border_box.height, 50.0);
    }

    #[test]
    fn a_borderless_paddingless_button_is_exactly_the_label() {
        let css = "button { padding: 0; border: 0 solid #000; \
                   min-width: 0; min-height: 0 }";
        let (tree, button, label) = button_layout(css, (42.0, 17.0));
        let a = tree.allocation(&button).expect("button allocation");
        assert_eq!(a.border_box.width, 42.0);
        assert_eq!(a.border_box.height, 17.0);
        let l = tree.allocation(&label).expect("label allocation");
        assert_eq!(l.border_box.x - a.border_box.x, 0.0);
        assert_eq!(l.border_box.y - a.border_box.y, 0.0);
    }

    #[test]
    fn the_label_is_centred_when_min_size_grows_the_button() {
        // The content box is min-height 24 tall; a 6-tall label centres at 9
        // inside it, i.e. 1 (border) + 4 (padding) + 9 == 14 from the top.
        let (tree, button, label) = button_layout(ADWAITA_LIKE, (0.0, 6.0));
        let a = tree.allocation(&button).expect("button allocation");
        let l = tree.allocation(&label).expect("label allocation");
        assert_eq!(l.border_box.y - a.border_box.y, 14.0);
    }

    #[test]
    fn one_tree_reused_gives_the_same_answer_as_a_fresh_one() {
        // A8, preserved: a second layout must not inherit anything from the
        // first. Mutation check: caching the first allocation and returning
        // it unconditionally makes `small` equal `tall`.
        let (tree_tall, tall_button, _l) = button_layout(ADWAITA_LIKE, (200.0, 40.0));
        let tall = tree_tall.allocation(&tall_button).expect("tall");

        let window = Node::new("window");
        let button = Node::new("button");
        let label_node = Node::new("label");
        window.append_child(&button);
        button.append_child(&label_node);
        let sheet = CompiledSheet::compile(ADWAITA_LIKE);
        let env = ResolveEnv::default();
        let mut cx = MatchCx::new();
        let bs = ComputedStyle::resolve_chain(&sheet, &button, &env, &mut cx);
        let ls = ComputedStyle::resolve_chain(&sheet, &label_node, &env, &mut cx);
        let ws = ComputedStyle::resolve_chain(&sheet, &window, &env, &mut cx);
        let mut tree = LayoutTree::new();
        tree.sync(&window).expect("sync");
        tree.set_style(&window, &ws, Container::default(), &env);
        tree.set_style(
            &button,
            &bs,
            Container::Box {
                direction: BoxDirection::Row,
            },
            &env,
        );
        tree.set_style(&label_node, &ls, Container::Leaf, &env);
        let space = Size {
            width: AvailableSpace::MaxContent,
            height: AvailableSpace::MaxContent,
        };
        tree.compute(&window, space, &mut LabelSize(200.0, 40.0))
            .expect("first");
        assert_eq!(tree.allocation(&button).expect("first alloc"), tall);
        tree.compute(&window, space, &mut LabelSize(0.0, 6.0))
            .expect("second");
        let small = tree.allocation(&button).expect("second alloc");
        assert_eq!(small.border_box.width, 36.0);
        assert_eq!(small.border_box.height, 34.0);
        tree.compute(&window, space, &mut LabelSize(200.0, 40.0))
            .expect("third");
        assert_eq!(
            tree.allocation(&button).expect("third alloc"),
            tall,
            "the tree did not go back to the larger layout"
        );
    }

    #[test]
    fn allocation_coordinates_are_absolute_not_parent_relative() {
        // Mutation check: returning taffy's raw `location` (parent-relative)
        // puts the label at x == 10 instead of 10 plus the button's own x.
        let css = "window { padding: 12px } button { padding: 4px 9px; \
                   border: 1px solid #000; min-width: 0; min-height: 0 }";
        let (tree, button, label) = button_layout(css, (30.0, 12.0));
        let b = tree.allocation(&button).expect("button");
        let l = tree.allocation(&label).expect("label");
        assert_eq!(
            b.border_box.x, 12.0,
            "the window's padding offsets the button"
        );
        assert_eq!(l.border_box.x, 12.0 + 10.0);
    }

    #[test]
    fn allocation_reports_used_border_and_padding_and_the_content_box() {
        // Mutation check: reading padding from the CSS rather than taffy's
        // used values silently ignores any clamping taffy applied.
        let (tree, button, _label) = button_layout(ADWAITA_LIKE, (60.0, 18.0));
        let a = tree.allocation(&button).expect("button");
        assert_eq!(a.border, [1.0, 1.0, 1.0, 1.0]);
        assert_eq!(a.padding, [4.0, 9.0, 4.0, 9.0]);
        assert_eq!(a.content_box.width, 80.0 - 2.0 - 18.0);
        assert_eq!(a.content_box.height, 34.0 - 2.0 - 8.0);
        assert_eq!(a.content_box.x, a.border_box.x + 10.0);
        assert_eq!(a.padding_box().width, 78.0);
    }

    #[test]
    fn allocation_of_an_unknown_node_is_none() {
        let (tree, _button, _label) = button_layout(ADWAITA_LIKE, (10.0, 10.0));
        assert!(tree.allocation(&Node::new("popover")).is_none());
    }

    #[test]
    fn fixed_measure_reports_its_size_for_every_leaf() {
        let window = Node::new("window");
        let leaf = Node::new("label");
        window.append_child(&leaf);
        let env = ResolveEnv::default();
        let initial = ComputedStyle::initial(&env);
        let mut tree = LayoutTree::new();
        tree.sync(&window).expect("sync");
        tree.set_style(&window, &initial, Container::default(), &env);
        tree.set_style(&leaf, &initial, Container::Leaf, &env);
        tree.compute(
            &window,
            Size {
                width: AvailableSpace::MaxContent,
                height: AvailableSpace::MaxContent,
            },
            &mut FixedMeasure(Size {
                width: 33.0,
                height: 11.0,
            }),
        )
        .expect("compute");
        let a = tree.allocation(&leaf).expect("leaf");
        assert_eq!(a.border_box.width, 33.0);
        assert_eq!(a.border_box.height, 11.0);
    }
    #[test]
    fn a_box_model_percentage_reaches_taffy_as_a_percentage() {
        // F58. Every box-model percentage was resolved eagerly against a
        // hard-coded basis of 0.0 and handed to taffy as an absolute length,
        // so `padding: 0 5%`, `margin: 10%` and `min-width: 50%` all
        // collapsed to 0px. taffy is the only thing here that knows the
        // containing block's size, so the percentage has to reach it intact.
        //
        // Mutation check: restore `length(pad_left)` and the padding
        // assertion reads `length(0.0)`.
        use taffy::prelude::{LengthPercentage, LengthPercentageAuto, percent};

        let sheet = CompiledSheet::compile(
            "button { padding: 0 5%; margin-left: 10%; min-width: 50%; \
             border: 0 solid transparent }",
        );
        let window = Node::with_classes("window", &["background"]);
        let button = Node::new("button");
        window.append_child(&button);
        let env = ResolveEnv::default();
        let style = ComputedStyle::resolve_chain(
            &sheet,
            &button,
            &env,
            &mut crate::css::select::MatchCx::new(),
        );

        let mut tree = LayoutTree::new();
        tree.sync(&window).expect("sync");
        tree.set_style(&button, &style, Container::Leaf, &env);
        let taffy = tree.taffy_style(&button).expect("styled");

        assert_eq!(
            taffy.padding.left,
            {
                let p: LengthPercentage = percent(0.05);
                p
            },
            "padding: 0 5% survives as a percentage"
        );
        assert_eq!(
            taffy.margin.left,
            {
                let p: LengthPercentageAuto = percent(0.10);
                p
            },
            "margin-left: 10% survives as a percentage"
        );
        assert_eq!(
            taffy.min_size.width,
            {
                let p: LengthPercentageAuto = percent(0.50);
                p
            },
            "min-width: 50% survives as a percentage"
        );
    }

    use super::{Align, ChildLayout, GridPlacement};
    use crate::css::node::Direction;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// A tree of `root > a, b, c` with a stylesheet-free computed style.
    fn three_children() -> (Node, Node, Node, Node) {
        let root = Node::new("box");
        let a = Node::new("widget");
        let b = Node::new("widget");
        let c = Node::new("widget");
        root.append_child(&a);
        root.append_child(&b);
        root.append_child(&c);
        (root, a, b, c)
    }

    /// Lay `root` out at a fixed 300x100 with every leaf measuring 20x10.
    ///
    /// taffy sizes a non-block root to its *content*, never to the available
    /// space (`taffy-0.14.0/src/compute/mod.rs:64`), so the 300x100 viewport
    /// is pinned through the root's own CSS minimums as well: a content-box
    /// floor on a root with neither padding nor border is exactly its size.
    /// The container the test set is read back and re-applied, because
    /// `set_style` takes one.
    fn lay_out(tree: &mut LayoutTree, root: &Node) {
        tree.sync(root).expect("sync");
        let sheet = CompiledSheet::compile("box { min-width: 300px; min-height: 100px }");
        let env = ResolveEnv::default();
        let root_style = ComputedStyle::resolve_chain(&sheet, root, &env, &mut MatchCx::new());
        let container = tree.container(root);
        tree.set_style(root, &root_style, container, &env);
        tree.compute(
            root,
            taffy::Size {
                width: taffy::AvailableSpace::Definite(300.0),
                height: taffy::AvailableSpace::Definite(100.0),
            },
            &mut FixedMeasure(taffy::Size {
                width: 20.0,
                height: 10.0,
            }),
        )
        .expect("compute");
    }

    #[test]
    fn a_node_with_no_child_layout_keeps_m2s_centre_centre() {
        // Mutation check: making `write_taffy_style` treat an absent
        // ChildLayout as `ChildLayout::default()` (halign/valign = Fill =>
        // AlignSelf::STRETCH) makes `a`'s height 100, not 10, and breaks
        // every M1 button number that depends on CENTER/CENTER.
        let (root, a, _b, _c) = three_children();
        let mut tree = LayoutTree::new();
        let style = ComputedStyle::initial(&ResolveEnv::default());
        tree.sync(&root).expect("sync");
        for n in [&root, &a] {
            tree.set_style(
                n,
                &style,
                Container::Box {
                    direction: BoxDirection::Row,
                },
                &ResolveEnv::default(),
            );
        }
        assert_eq!(tree.child_layout(&a), None, "no ChildLayout was set");
        lay_out(&mut tree, &root);
        let alloc = tree.allocation(&a).expect("allocation");
        assert_eq!(alloc.border_box.height, 10.0, "centred, not stretched");
        assert_eq!(
            alloc.border_box.y, 45.0,
            "centred on the cross axis: (100 - 10) / 2"
        );
    }

    #[test]
    fn an_explicit_fill_stretches_and_start_pins_to_the_leading_edge() {
        // Mutation check: mapping Align::Fill to AlignSelf::CENTER (the
        // "keep M2 behaviour everywhere" mistake) makes the first height 10;
        // mapping Align::Start to STRETCH makes the second 100.
        let (root, a, b, _c) = three_children();
        let mut tree = LayoutTree::new();
        let style = ComputedStyle::initial(&ResolveEnv::default());
        tree.sync(&root).expect("sync");
        for n in [&root, &a, &b] {
            tree.set_style(
                n,
                &style,
                Container::Box {
                    direction: BoxDirection::Row,
                },
                &ResolveEnv::default(),
            );
        }
        tree.set_child_layout(
            &a,
            ChildLayout {
                valign: Align::Fill,
                ..ChildLayout::default()
            },
        );
        tree.set_child_layout(
            &b,
            ChildLayout {
                valign: Align::Start,
                ..ChildLayout::default()
            },
        );
        lay_out(&mut tree, &root);
        assert_eq!(tree.allocation(&a).unwrap().border_box.height, 100.0);
        assert_eq!(tree.allocation(&b).unwrap().border_box.height, 10.0);
        assert_eq!(tree.allocation(&b).unwrap().border_box.y, 0.0);
    }

    #[test]
    fn grid_places_children_at_their_column_and_row() {
        // Mutation check: dropping the `+ 1` in the taffy line conversion
        // (taffy grid lines are 1-based) puts `c` in column 0's track and
        // both x values become equal.
        let (root, a, b, c) = three_children();
        let mut tree = LayoutTree::new();
        let style = ComputedStyle::initial(&ResolveEnv::default());
        tree.sync(&root).expect("sync");
        for n in [&root, &a, &b, &c] {
            tree.set_style(n, &style, Container::Leaf, &ResolveEnv::default());
        }
        tree.set_container(
            &root,
            Container::Grid {
                columns: 2,
                rows: 2,
                column_spacing: 4.0,
                row_spacing: 6.0,
                column_homogeneous: true,
                row_homogeneous: true,
            },
        );
        for (n, place) in [
            (
                &a,
                GridPlacement {
                    column: 0,
                    row: 0,
                    column_span: 1,
                    row_span: 1,
                },
            ),
            (
                &b,
                GridPlacement {
                    column: 0,
                    row: 1,
                    column_span: 1,
                    row_span: 1,
                },
            ),
            (
                &c,
                GridPlacement {
                    column: 1,
                    row: 0,
                    column_span: 1,
                    row_span: 2,
                },
            ),
        ] {
            tree.set_child_layout(
                n,
                ChildLayout {
                    grid: Some(place),
                    ..ChildLayout::default()
                },
            );
        }
        lay_out(&mut tree, &root);
        let (aa, ab, ac) = (
            tree.allocation(&a).unwrap(),
            tree.allocation(&b).unwrap(),
            tree.allocation(&c).unwrap(),
        );
        assert_eq!(aa.border_box.x, ab.border_box.x, "same column");
        assert!(
            ac.border_box.x > aa.border_box.x,
            "second column is to the right"
        );
        assert!(ab.border_box.y > aa.border_box.y, "second row is below");
        assert_eq!(
            ab.border_box.y - aa.border_box.y,
            (100.0 - 6.0) / 2.0 + 6.0,
            "homogeneous rows plus the 6px row-spacing"
        );
    }

    #[test]
    fn an_absolute_grid_child_keeps_its_own_size_and_ignores_track_sizing() {
        // Mutation check (Task 14's `Overlay`): setting an absolute child's
        // `inset` to a definite `0` instead of leaving it auto stretches it
        // edge to edge, hiding `Align::Start`'s natural-sized 10px result
        // behind a wrongly-stretched 100px one; forgetting `absolute`
        // entirely lets the second same-cell child fight the first for the
        // single track's size, shrinking `a` from 100 to 90.
        let (root, a, b, c) = three_children();
        root.remove_child(&c);
        let mut tree = LayoutTree::new();
        let style = ComputedStyle::initial(&ResolveEnv::default());
        tree.sync(&root).expect("sync");
        for n in [&root, &a, &b] {
            tree.set_style(n, &style, Container::Leaf, &ResolveEnv::default());
        }
        tree.set_container(
            &root,
            Container::Grid {
                columns: 1,
                rows: 1,
                column_spacing: 0.0,
                row_spacing: 0.0,
                column_homogeneous: true,
                row_homogeneous: true,
            },
        );
        let pin = Some(GridPlacement {
            column: 0,
            row: 0,
            column_span: 1,
            row_span: 1,
        });
        tree.set_child_layout(
            &a,
            ChildLayout {
                halign: Align::Fill,
                valign: Align::Fill,
                hexpand: true,
                vexpand: true,
                grid: pin,
                ..ChildLayout::default()
            },
        );
        tree.set_child_layout(
            &b,
            ChildLayout {
                halign: Align::Fill,
                valign: Align::Start,
                grid: pin,
                absolute: true,
                ..ChildLayout::default()
            },
        );
        lay_out(&mut tree, &root);
        assert_eq!(
            tree.allocation(&a).unwrap().border_box.height,
            100.0,
            "the in-flow main child fills the whole track, undiminished by its absolute sibling"
        );
        let ab = tree.allocation(&b).unwrap().border_box;
        assert_eq!(
            ab.height, 10.0,
            "kept its own measured height, not stretched"
        );
        assert_eq!(
            ab.y, 0.0,
            "start-aligned to the top of the containing block"
        );
    }

    #[test]
    fn centre_box_centres_the_middle_child_whatever_the_ends_measure() {
        // Mutation check: implementing Center as plain SPACE_BETWEEN without
        // the grow-from-zero on the outer children moves the centre child
        // off centre as soon as the two ends differ in width.
        let (root, start, centre, end) = three_children();
        let mut tree = LayoutTree::new();
        let style = ComputedStyle::initial(&ResolveEnv::default());
        tree.sync(&root).expect("sync");
        for n in [&root, &start, &centre, &end] {
            tree.set_style(n, &style, Container::Leaf, &ResolveEnv::default());
        }
        tree.set_container(
            &root,
            Container::Center {
                direction: BoxDirection::Row,
                shrink_center_last: true,
            },
        );
        tree.set_style(&start, &style, Container::Leaf, &ResolveEnv::default());
        tree.set_child_layout(&start, ChildLayout::default());
        lay_out(&mut tree, &root);
        let mid = tree.allocation(&centre).unwrap().border_box;
        assert_eq!(
            mid.x + mid.width / 2.0,
            150.0,
            "the centre child's centre is the container's centre"
        );
    }

    #[test]
    fn centre_box_puts_the_first_child_on_the_right_under_rtl() {
        // Mutation check: ignoring Direction::Rtl (using Row for both) puts
        // `start` at x == 0 under RTL, which is GTK's documented opposite
        // (gtk/gtkcenterbox.c:45).
        let (root, start, _centre, _end) = three_children();
        root.set_direction(Some(Direction::Rtl));
        let mut tree = LayoutTree::new();
        let style = ComputedStyle::initial(&ResolveEnv::default());
        tree.sync(&root).expect("sync");
        tree.set_style(&root, &style, Container::Leaf, &ResolveEnv::default());
        tree.set_container(
            &root,
            Container::Center {
                direction: BoxDirection::Row,
                shrink_center_last: false,
            },
        );
        lay_out(&mut tree, &root);
        assert!(
            tree.allocation(&start).unwrap().border_box.x > 150.0,
            "the first child is allocated on the right under RTL"
        );
    }

    #[test]
    fn a_per_node_measure_overrides_the_tree_measure_for_that_node_only() {
        // Mutation check: applying the per-node measure to every leaf (the
        // "last one wins" bug) makes `b` 40x30 too.
        let (root, a, b, _c) = three_children();
        let mut tree = LayoutTree::new();
        let style = ComputedStyle::initial(&ResolveEnv::default());
        tree.sync(&root).expect("sync");
        for n in [&root, &a, &b] {
            tree.set_style(n, &style, Container::Leaf, &ResolveEnv::default());
        }
        tree.set_measure(
            &a,
            Rc::new(RefCell::new(FixedMeasure(taffy::Size {
                width: 40.0,
                height: 30.0,
            }))),
        );
        lay_out(&mut tree, &root);
        assert_eq!(tree.allocation(&a).unwrap().border_box.width, 40.0);
        assert_eq!(tree.allocation(&b).unwrap().border_box.width, 20.0);
    }

    #[test]
    fn hostile_container_and_child_layout_values_never_panic() {
        // A Props-driven container count and spacing arrive from an
        // application model, so every one of them is clamped on the way to
        // taffy. Mutation checks: `n.clamp(1, 1024)` -> `n.min(1024)` gives
        // the first case 0 tracks; -> `n.max(1)` gives the second 65535;
        // dropping `finite(..).max(0.0)` on the spacing puts NaN, +inf and
        // -1e30 into the gap; dropping `finite` on the margins puts NaN into
        // the allocation; dropping `column_span.max(1)` leaves a zero span;
        // dropping `row.min(1023)` leaves a line index of 65536.
        let (root, a, _b, _c) = three_children();
        let mut tree = LayoutTree::new();
        let style = ComputedStyle::initial(&ResolveEnv::default());
        tree.sync(&root).expect("sync");
        tree.set_style(&root, &style, Container::Leaf, &ResolveEnv::default());
        for (columns, rows, spacing, tracks) in [
            (0_u16, 0_u16, f32::NAN, (1_usize, 1_usize)),
            (u16::MAX, u16::MAX, f32::INFINITY, (1024, 1024)),
            (1, 1, -1.0e30, (1, 1)),
        ] {
            tree.set_container(
                &root,
                Container::Grid {
                    columns,
                    rows,
                    column_spacing: spacing,
                    row_spacing: spacing,
                    column_homogeneous: true,
                    row_homogeneous: false,
                },
            );
            tree.set_child_layout(
                &a,
                ChildLayout {
                    margin: [spacing; 4],
                    grid: Some(GridPlacement {
                        column: u16::MAX,
                        row: u16::MAX,
                        column_span: 0,
                        row_span: u16::MAX,
                    }),
                    ..ChildLayout::default()
                },
            );
            let taffy = tree.taffy_style(&root).expect("styled");
            assert_eq!(
                (
                    taffy.grid_template_columns.len(),
                    taffy.grid_template_rows.len()
                ),
                tracks,
                "a hostile track count is clamped to 1..=1024"
            );
            assert_eq!(
                taffy.gap.width,
                taffy::prelude::length(0.0),
                "NaN, infinite and hugely negative spacings all become 0"
            );
            assert_eq!(taffy.gap.height, taffy::prelude::length(0.0));
            let child = tree.taffy_style(&a).expect("styled child");
            assert_eq!(
                child.grid_column.end,
                taffy::prelude::span(1),
                "a zero span is read as one"
            );
            assert_eq!(
                child.grid_row.start,
                taffy::prelude::line(1024),
                "a hostile row index is clamped to the last track taffy is given"
            );
            lay_out(&mut tree, &root);
            let alloc = tree.allocation(&a).expect("allocation");
            assert!(
                alloc.border_box.x.is_finite()
                    && alloc.border_box.y.is_finite()
                    && alloc.border_box.width.is_finite()
                    && alloc.border_box.height.is_finite(),
                "a hostile margin must not put NaN into an allocation: {:?}",
                alloc.border_box
            );
        }
    }
}
