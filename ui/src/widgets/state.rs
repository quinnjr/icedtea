//! The node-keyed side tables every widget controller writes through.
//!
//! A controller owns its CSS [`Node`]s but not the `LayoutTree` those nodes
//! are laid out in, and a bare pooled row has no `Instance` at all — so the
//! decisions that cannot live on either (a container variant, a gap floor, a
//! grid cell, a size request, a pooled row's bound text) live here, keyed by
//! the node, and are folded into the real tree by [`flush_layout`] once per
//! frame.
//!
//! Split out of `widgets/mod.rs` verbatim; every item is re-exported from
//! [`crate::widgets`], which is the path every caller uses.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use selectors::Element as _;
use selectors::OpaqueElement;

use crate::css::node::{Node, NodeInner};
use crate::layout::{ChildLayout, Container};
use crate::view::{BuildCx, Kind, Prop, PropName, Props};

use super::{Orientation, Universal, grid, overlay, types};

/// One controller's not-yet-flushed layout decisions for one node.
///
/// A widget controller has no `&mut LayoutTree` of its own — the tree lives
/// in the `App`'s `Runtime` (or, in a headless test, is never built at all)
/// — so a container/gap/homogeneous/child-placement decision is recorded
/// *on the node* here and folded into the real tree by [`flush_layout`].
///
/// Unlike a work queue, entries are **not** removed once applied: `App`
/// reruns `write_styles` (which rebuilds a node's taffy style from its CSS
/// alone) on every single frame, so a decision made once in `Controller::build`
/// must still be there to reapply on the tenth frame, long after the
/// `Props` value that produced it stopped changing. `homogeneous` is
/// re-applied to *whatever children the node currently has* for the same
/// reason: a child added after the prop was last set must still pick it up.
#[derive(Default, Clone)]
struct Pending {
    container: Option<Container>,
    gap: Option<(f32, Orientation)>,
    homogeneous: Option<(bool, Orientation)>,
    child_layout: Option<ChildLayout>,
    /// This node is a [`Container::Grid`]; re-derive every child's cell from
    /// its own recorded props on every flush, the same way `homogeneous`
    /// re-applies to whatever children the node currently has. A `Grid`
    /// controller builds before the reconciler attaches its children (M2's
    /// `build_instance` order), so it cannot loop over `node.children()`
    /// itself -- there are none yet -- and defers the loop to here instead.
    grid_from_children: bool,
    /// This node is an [`Kind::Overlay`](crate::view::Kind::Overlay); classify
    /// every child (positional CSS class, `ChildLayout`, one-cell grid pin)
    /// from its own recorded props on every flush, for the same
    /// build-order reason as `grid_from_children`.
    overlay_from_children: bool,
    /// `GtkWidget:width-request`/`height-request`, in px: a floor on this
    /// node's own size, folded into taffy by [`flush_layout`].
    size_request: Option<(f32, f32)>,
    /// `GtkWidget:visible` for a controller-owned node, folded into taffy by
    /// [`flush_layout`] as `display: none`.
    displayed: Option<bool>,
    node: Option<Node>,
}

/// One node-keyed side table, safe against address reuse.
///
/// The key is `node.opaque()` -- the `NodeInner` allocation's address --
/// which is unique only among *live* nodes: the allocator hands that address
/// back the moment the node is torn down, so a plain
/// `HashMap<OpaqueElement, T>` lets a freshly built node silently inherit a
/// dead node's entry (its grid cell, its container variant, its pooled row
/// binding). Destroy a `Grid`/`Stack`/`ListBox` child and build another and
/// that is exactly what happens.
///
/// Every entry therefore carries a `Weak` handle on the payload it was
/// recorded against, which fixes both halves at once:
///
/// * **No aliasing.** A live `Weak` keeps the *allocation* (not the node)
///   alive, so its address cannot be handed out again while a stale entry
///   holds it; and a read still checks `upgrade()` and payload identity, so
///   a stale entry reads as absent rather than as the dead node's value.
/// * **No unbounded growth.** [`forget_subtree`] (called from `reconcile`'s
///   `Remove` op) drops a removed instance's entries eagerly, and
///   [`NodeTable::purge`] (once per [`flush_layout`]) sweeps whatever a
///   controller dropped on its own -- a scrollbar a policy change removed, a
///   pooled row a shrinking pool released.
struct NodeTable<T> {
    entries: HashMap<OpaqueElement, (Weak<NodeInner>, T)>,
}

impl<T> NodeTable<T> {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// Whether this entry's recorded payload is still `node`'s own.
    fn is_live(alive: &Weak<NodeInner>, node: &Node) -> bool {
        alive
            .upgrade()
            .is_some_and(|payload| Rc::ptr_eq(&payload, &node.opaque_payload()))
    }

    fn get(&self, node: &Node) -> Option<&T> {
        self.entries
            .get(&node.opaque())
            .filter(|(alive, _)| Self::is_live(alive, node))
            .map(|(_, value)| value)
    }

    fn insert(&mut self, node: &Node, value: T) {
        self.entries.insert(
            node.opaque(),
            (Rc::downgrade(&node.opaque_payload()), value),
        );
    }

    /// The entry for `node`, created (or *replaced*, when the entry at this
    /// address belongs to a dead node) from `T::default()`.
    fn entry_mut(&mut self, node: &Node) -> &mut T
    where
        T: Default,
    {
        let key = node.opaque();
        let stale = self
            .entries
            .get(&key)
            .is_none_or(|(alive, _)| !Self::is_live(alive, node));
        if stale {
            self.entries
                .insert(key, (Rc::downgrade(&node.opaque_payload()), T::default()));
        }
        &mut self
            .entries
            .get_mut(&key)
            .expect("just inserted when it was missing or stale")
            .1
    }

    fn remove(&mut self, node: &Node) {
        self.entries.remove(&node.opaque());
    }

    /// Drop every entry whose node is gone.
    fn purge(&mut self) {
        self.entries
            .retain(|_, (alive, _)| alive.strong_count() > 0);
    }

    /// Every value whose node is still alive.
    fn live_values(&self) -> impl Iterator<Item = &T> {
        self.entries
            .values()
            .filter(|(alive, _)| alive.strong_count() > 0)
            .map(|(_, value)| value)
    }
}

thread_local! {
    static PENDING: RefCell<NodeTable<Pending>> = RefCell::new(NodeTable::new());
    static CONTAINERS: RefCell<NodeTable<Container>> = RefCell::new(NodeTable::new());
    static NODE_PROPS: RefCell<NodeTable<Props>> = RefCell::new(NodeTable::new());
    static NODE_CHILD: RefCell<NodeTable<ChildLayout>> = RefCell::new(NodeTable::new());
    static NODE_UNIVERSAL: RefCell<NodeTable<Universal>> = RefCell::new(NodeTable::new());
}

/// The universal props [`apply_universal`] writes centrally, for *every*
/// kind, whether or not that kind's controller owns a [`Universal`].
///
/// Deliberately a subset of the fifteen [`Universal::apply`] knows. These six
/// have exactly one meaning on every `GtkWidget` — a class list, an id, a
/// focus default, a sensitivity, a size floor — so applying them centrally
/// can only ever agree with a controller that also applies them.
/// `Checked`/`Indeterminate`/`Selected` are **not** here: a widget is free to
/// give those names its own meaning (`ListView`'s `.selected(index)` is a
/// model index, not a `:selected` flag on the view's own node), so writing a
/// pseudo-state for them centrally would be wrong.
const CENTRAL_UNIVERSAL: &[PropName] = &[
    PropName::Classes,
    PropName::Focusable,
    PropName::Id,
    PropName::Sensitive,
    PropName::WidthRequest,
    PropName::HeightRequest,
];

/// Write one universal prop onto `node`, from the reconciler, before the
/// kind's own controller sees it.
///
/// [`Universal`] is opt-in per controller and only half the toolkit opted in:
/// 30 of the widget modules mention it nowhere, so their `set_prop` fell
/// through to `_ => return` for every universal name. `width-request` is the
/// visible half of that — `gallery`'s `progress_bar` sample asked for 160 px
/// and laid out `progress_bar 640 140 0 19`, a zero-width widget, and the
/// `scrollbar` sample did the same — but `Classes`, `Id`, `Sensitive` and
/// `Focusable` were dropped just as silently.
///
/// Applying centrally rather than in each of those 30 modules is what makes
/// "universal" true by construction instead of by 60 files agreeing. It is
/// idempotent against a controller that *does* own a `Universal`: both write
/// the same class/id/state/size-request, and each tracks the classes it
/// itself contributed, so a later change removes the stale class once from
/// each and adds the new one twice — the node's class list is a set.
///
/// `true` when `name` was one of [`CENTRAL_UNIVERSAL`]; the caller passes it
/// to the controller either way, because a controller may have work of its
/// own to do on the same name.
pub(crate) fn apply_universal(node: &Node, kind: Kind, name: PropName, value: &Prop) -> bool {
    if !CENTRAL_UNIVERSAL.contains(&name) {
        return false;
    }
    NODE_UNIVERSAL.with(|m| {
        m.borrow_mut()
            .entry_mut(node)
            .apply(node, kind, name, value)
    })
}

/// The depth [`forget_subtree`] walks before giving up, so a pathological
/// tree cannot blow the stack. Anything deeper is left to
/// [`NodeTable::purge`], which is not recursive at all.
const MAX_FORGET_DEPTH: usize = 256;

/// Forget every side-table entry belonging to `node` or any descendant.
///
/// `reconcile`'s `Remove` op calls this as it drops an instance: the instance
/// owns its root [`Node`] and (through its controller) that node's chrome
/// subnodes, and any of them may have recorded a container, a child layout, a
/// props subset, a stack transition or a row binding.
pub(crate) fn forget_subtree(node: &Node) {
    fn walk(node: &Node, depth: usize) {
        if depth > MAX_FORGET_DEPTH {
            return;
        }
        for child in node.children() {
            walk(&child, depth + 1);
        }
        PENDING.with(|p| p.borrow_mut().remove(node));
        CONTAINERS.with(|c| c.borrow_mut().remove(node));
        NODE_PROPS.with(|m| m.borrow_mut().remove(node));
        NODE_CHILD.with(|m| m.borrow_mut().remove(node));
        TRANSITIONS.with(|t| t.borrow_mut().remove(node));
        ROW_BINDING.with(|m| m.borrow_mut().remove(node));
        NODE_UNIVERSAL.with(|m| m.borrow_mut().remove(node));
    }
    walk(node, 0);
}

/// Drop every side-table entry whose node has been torn down.
fn purge_tables() {
    PENDING.with(|p| p.borrow_mut().purge());
    CONTAINERS.with(|c| c.borrow_mut().purge());
    NODE_PROPS.with(|m| m.borrow_mut().purge());
    NODE_CHILD.with(|m| m.borrow_mut().purge());
    TRANSITIONS.with(|t| t.borrow_mut().purge());
    ROW_BINDING.with(|m| m.borrow_mut().purge());
    NODE_UNIVERSAL.with(|m| m.borrow_mut().purge());
}

/// The prop names [`record_props`] keeps; every other prop is dropped on the
/// way in.
///
/// A full `Props` clone would retain whatever a caller last set through it
/// forever -- `Prop::Draw`'s `Rc<dyn Fn>` included -- because this table has
/// no removal path (`reconcile`'s `Remove` op drops the `Instance`, not this
/// side entry). Grid placement was the only reader before P6 Task 11, and
/// every value it needs is a `Prop::Int`, cheap to keep and inert to clone;
/// `action_bar::ActionBarC::place` is the second reader, over `Section`
/// (a `Prop::Str`, equally cheap); `overlay::classify_overlay_child` is
/// the third, over `Halign`/`Valign` (`Prop::Align`, `Copy`) and
/// `MeasureOverlay`/`ClipOverlay` (`Prop::Bool`) -- all four cheap, and all
/// four re-derive a child's placement from the child's own node rather than
/// owning that child's `Instance`.
const RECORDED_PROP_NAMES: [PropName; 16] = [
    PropName::Column,
    PropName::Row,
    PropName::ColumnSpan,
    PropName::RowSpan,
    PropName::Section,
    PropName::Halign,
    PropName::Valign,
    PropName::MeasureOverlay,
    PropName::ClipOverlay,
    // `stack::StackC::place`'s fourth reader (Task 15): a page's `Str`
    // name/title and `Bool` attention flag, all as cheap to keep as the
    // nine above and re-derived the same way, from the child's own node.
    PropName::PageName,
    PropName::PageTitle,
    PropName::NeedsAttention,
    // `column_view::ColumnViewC::place`'s reader (Task 20): a
    // `column_view_column`'s title, resizability, expand flag and
    // comparator, re-derived from the child's own node the same way every
    // reader above already does -- `ColumnViewC` has no `&mut Instance` of
    // its own column children either, only their `Node`s (via `child_slot`
    // redirecting them into its `header`).
    PropName::Title,
    PropName::Resizable,
    PropName::Expand,
    PropName::Sorter,
];

/// Record the subset of `props` a container controller can re-derive a
/// child's placement from. Called from the reconciler's `Insert`/`SetProp`
/// arms with that instance's just-applied `Props`.
pub(crate) fn record_props(node: &Node, props: &Props) {
    let mut kept = Props::default();
    for name in RECORDED_PROP_NAMES {
        if let Some(value) = props.get(name) {
            kept.set(name, value.clone());
        }
    }
    NODE_PROPS.with(|m| m.borrow_mut().insert(node, kept));
}

/// The props last applied to `node`, or an empty set.
///
/// A container controller (e.g. [`grid::GridC`]) has no `&mut Instance` of
/// its own child -- only the child's [`Node`] -- so it re-derives a child's
/// placement from here instead of owning the child's instance.
#[must_use]
pub(crate) fn props_of(node: &Node) -> Props {
    NODE_PROPS
        .with(|m| m.borrow().get(node).cloned())
        .unwrap_or_default()
}

/// The child layout last recorded for `node`.
#[must_use]
pub(crate) fn child_layout_of(node: &Node) -> ChildLayout {
    NODE_CHILD
        .with(|m| m.borrow().get(node).copied())
        .unwrap_or_default()
}

thread_local! {
    static TRANSITIONS: RefCell<NodeTable<(types::StackTransition, f32)>> =
        RefCell::new(NodeTable::new());
}

/// Record `node`'s current stack-transition state.
///
/// Reconciliation (Task 15): the task text describes this as recording "the
/// translate/opacity the paint walker applies for the named transition", but
/// no such paint-walker hook exists yet -- M2's `paint_node_with_children`
/// (this task's files do not touch `view/render.rs`/`paint.rs`) has no
/// per-node transform/opacity override to plug into. This stores the value
/// in the same kind of node-keyed thread-local table `set_container`/
/// `record_props` already use, so [`StackC::tick`] has somewhere real to
/// write it and a future paint task has somewhere real to read it from,
/// without inventing paint-walker plumbing this part's file list excludes.
pub(crate) fn set_transition_progress(
    node: &Node,
    transition: types::StackTransition,
    progress: f32,
) {
    TRANSITIONS.with(|t| t.borrow_mut().insert(node, (transition, progress)));
}

/// Record `node`'s container, for later [`flush_layout`] and for the
/// headless `container_of` test hook every container controller exposes.
pub(crate) fn set_container(node: &Node, container: Container) {
    CONTAINERS.with(|c| c.borrow_mut().insert(node, container));
    PENDING.with(|p| {
        let mut p = p.borrow_mut();
        let entry = p.entry_mut(node);
        entry.container = Some(container);
        entry.node = Some(node.clone());
    });
}

/// Mark `node` (a [`Container::Grid`]) so [`flush_layout`] re-derives every
/// child's [`crate::layout::GridPlacement`] from that child's own recorded
/// `Column`/`Row`/`ColumnSpan`/`RowSpan` props each frame.
pub(crate) fn mark_grid_children(node: &Node) {
    PENDING.with(|p| {
        let mut p = p.borrow_mut();
        let entry = p.entry_mut(node);
        entry.grid_from_children = true;
        entry.node = Some(node.clone());
    });
}

/// Mark `node` (a [`Kind::Overlay`](crate::view::Kind::Overlay)) so
/// [`flush_layout`] re-derives every child's positional class, `ChildLayout`
/// and one-cell grid pin from that child's own recorded props each frame.
pub(crate) fn mark_overlay_children(node: &Node) {
    PENDING.with(|p| {
        let mut p = p.borrow_mut();
        let entry = p.entry_mut(node);
        entry.overlay_from_children = true;
        entry.node = Some(node.clone());
    });
}

/// One pooled row's currently-bound content, for [`ListViewC`](list_view::ListViewC)
/// (and any future recycling view) to keep beside a `Node` without a
/// dedicated field on the `Node` type itself -- a `Node` carries CSS state,
/// never arbitrary widget-owned data, and text/model-index are exactly that.
#[derive(Clone)]
struct RowBinding {
    text: Rc<str>,
    index: usize,
    classes: Rc<[Rc<str>]>,
}

impl Default for RowBinding {
    fn default() -> Self {
        Self {
            text: Rc::from(""),
            index: 0,
            classes: Rc::from(&[][..]),
        }
    }
}

thread_local! {
    static ROW_BINDING: RefCell<NodeTable<RowBinding>> = RefCell::new(NodeTable::new());
}

/// Set a pooled row's displayed text.
pub(crate) fn set_text(node: &Node, text: &str) {
    ROW_BINDING.with(|m| {
        m.borrow_mut().entry_mut(node).text = Rc::from(text);
    });
}

/// A pooled row's currently-bound text, or empty if never bound.
#[must_use]
pub(crate) fn text_of(node: &Node) -> Rc<str> {
    ROW_BINDING
        .with(|m| m.borrow().get(node).map(|b| Rc::clone(&b.text)))
        .unwrap_or_else(|| Rc::from(""))
}

/// A pooled row's text, shaped in `node`'s own resolved style.
///
/// The shaping half of [`measure_row`]/[`paint_row`]. `FontDatabase::shape`
/// is itself cached on the same key, so calling this once per measure and
/// once per paint costs one map lookup after the first frame.
fn shaped_row(
    node: &Node,
    computed: &crate::css::computed::ComputedStyle,
    fonts: &mut crate::text::FontDatabase,
) -> Option<(crate::text::TextStyle, Rc<crate::text::ShapedText>)> {
    let text = text_of(node);
    if text.is_empty() {
        return None;
    }
    let style = crate::text::TextStyle::from_computed(computed);
    let face = fonts.match_face(&style.query())?;
    let shaped = fonts.shape(&style.shape_key(&text, &face));
    Some((style, shaped))
}

/// The line box a pooled row's bound text occupies, in `node`'s own resolved
/// style. `None` when the row is unbound or the sheet's family resolves to no
/// usable face.
///
/// This is a *content* height, a pure function of the sheet and the font — it
/// never reads an allocation — so a caller sizing rows by it (see
/// `ListViewC::css_row_height`) still has a fixed point rather than a
/// pool-size feedback loop.
pub(crate) fn row_text_height(
    node: &Node,
    computed: &crate::css::computed::ComputedStyle,
    fonts: &mut crate::text::FontDatabase,
) -> Option<f32> {
    let (style, shaped) = shaped_row(node, computed, fonts)?;
    let height = style.line_height_px(&shaped.metrics);
    (height.is_finite() && height > 0.0).then_some(height)
}

/// Measure a pooled row's bound text, or `None` when `node` is not a bound
/// row at all.
///
/// A pooled row is a bare [`Node`], never a reconciled `Instance`: it is
/// created by `ListViewC`/`GridViewC`/`ColumnViewC` and rebound in place, so
/// that a scroll keeps a row's identity, its `:selected` state and its
/// running animations (§4.3's recycling contract). The price is that no
/// controller owns it, so `view::app`'s `ControllerMeasure`/`ControllerPainter`
/// — which both resolve a node through the instance tree — found nothing and
/// every row measured 0 px high and drew no glyphs. Ten rows could not
/// overflow a 120 px viewport, `max_offset` was 0, and nothing scrolled or
/// recycled.
///
/// This is the second source those two consult when the instance lookup
/// misses: the row's own [`RowBinding`], measured and painted exactly the way
/// [`crate::view::controller::GenericC`] does its label.
pub(crate) fn measure_row(
    node: &Node,
    available: (Option<f32>, Option<f32>),
    cx: &mut BuildCx<'_>,
) -> Option<(f32, f32)> {
    // The emptiness check comes before the cascade, not inside `shaped_row`'s
    // own: this runs for *every* leaf taffy measures that no instance owns —
    // a scrollbar's trough, a switch's slider, every controller-built subnode
    // in the tree — and a full `resolve_chain` per leaf per layout pass is
    // not a price a row-binding lookup should make them pay.
    if text_of(node).is_empty() {
        return None;
    }
    let mut match_cx = crate::css::select::MatchCx::new();
    let computed =
        crate::css::computed::ComputedStyle::resolve_chain(cx.sheet, node, cx.env, &mut match_cx);
    let (style, shaped) = shaped_row(node, &computed, cx.fonts)?;
    let width = shaped.metrics.width;
    let height = style.line_height_px(&shaped.metrics);
    Some((
        available.0.map_or(width, |cap| width.min(cap)),
        available.1.map_or(height, |cap| height.min(cap)),
    ))
}

/// Paint a pooled row's bound text into its own content box. `true` when
/// anything was drawn. See [`measure_row`].
pub(crate) fn paint_row(
    node: &Node,
    canvas: &mut skia_rs_safe::canvas::Canvas<'_>,
    alloc: &crate::layout::Allocation,
    style: &crate::css::computed::ComputedStyle,
    cx: &mut crate::paint::PaintCx<'_>,
) -> bool {
    let Some((_, shaped)) = shaped_row(node, style, cx.fonts) else {
        return false;
    };
    crate::paint::text::paint_text(
        canvas,
        &shaped,
        alloc.content_box,
        style,
        &cx.base_length_ctx(),
    );
    true
}

/// Record which *model* index a pooled row currently displays -- never its
/// pool slot, which changes on every scroll while the row's identity does
/// not.
pub(crate) fn set_row_index(node: &Node, index: usize) {
    ROW_BINDING.with(|m| m.borrow_mut().entry_mut(node).index = index);
}

/// The model index last bound to this row, if any.
#[must_use]
pub(crate) fn row_index_of(node: &Node) -> Option<usize> {
    ROW_BINDING.with(|m| m.borrow().get(node).map(|b| b.index))
}

/// Replace a pooled row's extra classes with exactly `classes`, leaving every
/// other class (`row`'s own name, `.activatable`, `:selected`, ...) alone.
pub(crate) fn set_row_classes(node: &Node, classes: &[Rc<str>]) {
    ROW_BINDING.with(|m| {
        let mut m = m.borrow_mut();
        let entry = m.entry_mut(node);
        for old in entry.classes.iter() {
            if !classes.iter().any(|c| c == old) {
                node.remove_class(old);
            }
        }
        for new in classes {
            node.add_class(new);
        }
        entry.classes = Rc::from(classes.to_vec());
    });
}

/// This node's last recorded container, for a headless (`App`-less) test.
///
/// A real `App` derives a node's container independently, from its `Kind`
/// and `Props` (`view::render::container_for`); this is a second, cheaper
/// source of truth that lets a widget's own unit tests ask "what container
/// did my controller build?" without standing up a `LayoutTree` at all.
#[must_use]
pub(crate) fn container_of(node: &Node) -> Container {
    CONTAINERS.with(|c| c.borrow().get(node).copied().unwrap_or_default())
}

/// Record `node`'s per-child layout (alignment/expansion), keyed by the
/// *child* node itself rather than its parent -- a `Container::Center`'s
/// three children each need a different [`ChildLayout`], which the
/// one-entry-per-node `container`/`gap`/`homogeneous` fields above cannot
/// express.
pub(crate) fn set_child_layout(node: &Node, layout: ChildLayout) {
    PENDING.with(|p| {
        let mut p = p.borrow_mut();
        let entry = p.entry_mut(node);
        entry.child_layout = Some(layout);
        entry.node = Some(node.clone());
    });
    NODE_CHILD.with(|m| m.borrow_mut().insert(node, layout));
}

/// Record a gap floor for `node`'s main axis, keyed by `orientation`.
pub(crate) fn set_gap(node: &Node, spacing: f32, orientation: Orientation) {
    PENDING.with(|p| {
        let mut p = p.borrow_mut();
        let entry = p.entry_mut(node);
        entry.gap = Some((
            if spacing.is_finite() {
                spacing.max(0.0)
            } else {
                0.0
            },
            orientation,
        ));
        entry.node = Some(node.clone());
    });
}

/// Record `GtkWidget:width-request`/`height-request` for `node`, in px.
///
/// A floor, never a fixed size, exactly as GTK's own pair are: CSS
/// `min-width`/`min-height` still apply and the larger wins on each axis.
/// Recorded on the side table rather than written to the tree because a
/// controller has no `&mut LayoutTree`; [`flush_layout`] folds it in.
pub(crate) fn set_size_request(node: &Node, width: f32, height: f32) {
    PENDING.with(|p| {
        let mut p = p.borrow_mut();
        let entry = p.entry_mut(node);
        let clamp = |px: f32| if px.is_finite() { px.max(0.0) } else { 0.0 };
        entry.size_request = Some((clamp(width), clamp(height)));
        entry.node = Some(node.clone());
    });
}

/// The `width-request`/`height-request` last recorded for `node`, or `(0, 0)`.
///
/// The read side of [`set_size_request`], for the one caller that needs the
/// *requested* extent rather than the allocated one: a recycling view's
/// viewport (`ListViewC::adopt_metrics`).
#[must_use]
pub(crate) fn size_request_of(node: &Node) -> (f32, f32) {
    PENDING.with(|p| {
        p.borrow()
            .get(node)
            .and_then(|entry| entry.size_request)
            .unwrap_or((0.0, 0.0))
    })
}

/// Record whether a controller-owned `node` is shown — `GtkWidget:visible`.
///
/// Hidden means `display: none`: the node keeps its place in the CSS tree, so
/// a vendored §5.2 fixture naming it still matches and its controller keeps
/// its state, but it takes no space, receives no events and paints nothing.
/// Recorded on the side table because a controller has no `&mut LayoutTree`;
/// [`flush_layout`] folds it in.
pub(crate) fn set_displayed(node: &Node, on: bool) {
    PENDING.with(|p| {
        let mut p = p.borrow_mut();
        let entry = p.entry_mut(node);
        entry.displayed = Some(on);
        entry.node = Some(node.clone());
    });
}

/// Record that every child of `node` should expand on `orientation`'s axis
/// while `on` holds, applied to whatever children `node` has when
/// [`flush_layout`] next runs.
pub(crate) fn set_homogeneous(node: &Node, on: bool, orientation: Orientation) {
    PENDING.with(|p| {
        let mut p = p.borrow_mut();
        let entry = p.entry_mut(node);
        entry.homogeneous = Some((on, orientation));
        entry.node = Some(node.clone());
    });
}

/// Apply every recorded decision to `tree`.
///
/// Called by the real render loop (`view::render::layout_tree`) immediately
/// after `write_styles` and before `LayoutTree::compute`, and by any test
/// that builds its own `LayoutTree`. Entries whose node never made it into
/// `tree` are silent no-ops: `LayoutTree::set_container`/`set_gap_floor`/
/// `set_child_layout` already ignore an unsynced node, which covers a
/// controller that recorded a decision before its node was attached, or a
/// node `reconcile` has since removed.
pub fn flush_layout(tree: &mut crate::layout::LayoutTree) {
    // Once per frame is the natural sweep point for whatever the eager
    // `forget_subtree` path cannot see: a chrome subnode a controller dropped
    // on its own, or a pooled row a shrinking pool released.
    purge_tables();
    PENDING.with(|p| {
        for pending in p.borrow().live_values() {
            let Some(node) = &pending.node else { continue };
            if let Some(container) = pending.container {
                tree.set_container(node, container);
            }
            if let Some(child_layout) = pending.child_layout {
                tree.set_child_layout(node, child_layout);
            }
            if let Some(on) = pending.displayed {
                tree.set_displayed(node, on);
            }
            if let Some((w, h)) = pending.size_request {
                tree.set_size_floor(node, w, h);
            }
            if let Some((gap, orientation)) = pending.gap {
                // GTK's own gap property and CSS `border-spacing` both
                // apply; the larger wins, which is what GtkBox does when a
                // theme sets both.
                tree.set_gap_floor(node, gap, orientation == Orientation::Horizontal);
            }
            if let Some((on, orientation)) = pending.homogeneous {
                let horizontal = orientation == Orientation::Horizontal;
                for child in node.children() {
                    let mut cl = tree.child_layout(&child).unwrap_or_default();
                    if horizontal {
                        cl.hexpand = on;
                    } else {
                        cl.vexpand = on;
                    }
                    tree.set_child_layout(&child, cl);
                }
            }
            if pending.grid_from_children {
                for child in node.children() {
                    let Some(place) = grid::GridC::placement_of(&props_of(&child)) else {
                        continue;
                    };
                    let mut cl = tree.child_layout(&child).unwrap_or_default();
                    cl.grid = Some(place);
                    tree.set_child_layout(&child, cl);
                }
            }
            if pending.overlay_from_children {
                for (index, child) in node.children().into_iter().enumerate() {
                    let cl = overlay::classify_overlay_child(index, &child);
                    tree.set_child_layout(&child, cl);
                }
            }
        }
    });
}

#[cfg(test)]
mod side_table_tests {
    use super::{
        container_of, forget_subtree, props_of, record_props, set_container, set_text, text_of,
    };
    use crate::css::node::Node;
    use crate::layout::{BoxDirection, Container};
    use crate::view::{Prop, PropName, Props};

    const CENTER: Container = Container::Center {
        direction: BoxDirection::Row,
        shrink_center_last: false,
    };

    /// Mutation: key `NodeTable` on `node.opaque()` alone, without the `Weak`
    /// liveness tag (as these tables did before the P6 fix wave), and this
    /// fails -- the allocator hands the dead node's address straight back
    /// (verified: 512 of 512 fresh nodes land on it) and `props_of` answers
    /// `Int(7)`, `text_of` `"dead"`, for a node nobody ever recorded.
    ///
    /// The two tables read here are the ones that hold no reference of their
    /// own; `PENDING` (and `CONTAINERS`, written with it) keeps a strong
    /// `Node`, which blocks reuse but leaks instead -- that half is
    /// `forgetting_a_subtree_drops_every_entry_under_it` below.
    #[test]
    fn a_dead_node_never_lends_its_side_table_entry_to_a_fresh_one() {
        {
            let dead = Node::new("gridchild");
            let mut props = Props::default();
            props.set(PropName::Column, Prop::Int(7));
            record_props(&dead, &props);
            set_text(&dead, "dead");
            assert_eq!(props_of(&dead).get(PropName::Column), Some(&Prop::Int(7)));
        }
        for _ in 0..512 {
            let fresh = Node::new("gridchild");
            assert!(
                props_of(&fresh).get(PropName::Column).is_none(),
                "a fresh node inherited a dead node's recorded grid cell"
            );
            assert_eq!(
                &*text_of(&fresh),
                "",
                "a fresh node inherited a dead node's pooled row binding"
            );
        }
    }

    /// Mutation: delete `forget_subtree`'s body (or `reconcile`'s call to it)
    /// and this fails -- the entries outlive the subtree they describe.
    #[test]
    fn forgetting_a_subtree_drops_every_entry_under_it() {
        let root = Node::new("box");
        let child = Node::new("label");
        root.append_child(&child);
        set_container(&root, CENTER);
        let mut props = Props::default();
        props.set(PropName::Row, Prop::Int(3));
        record_props(&child, &props);

        assert_eq!(container_of(&root), CENTER);
        assert_eq!(props_of(&child).get(PropName::Row), Some(&Prop::Int(3)));

        forget_subtree(&root);

        assert_eq!(container_of(&root), Container::default());
        assert!(props_of(&child).get(PropName::Row).is_none());
    }
}
