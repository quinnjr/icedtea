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

use std::time::Duration;

pub use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::Shape as CursorShape;

/// The client-side mirror of the compositor's implicit pointer grab.
///
/// Once a button goes down on a node, motion and the release go to that node
/// regardless of what is under the pointer, until the *last* button is
/// released. Without it, dragging a scale's slider off the widget stops moving
/// it, and a press-drag-off-release never fires the click it should not fire.
#[derive(Debug, Default)]
pub struct ImplicitGrab {
    target: Option<Node>,
    /// Which of the first 31 buttons are down, as a bitmask over
    /// `button - BTN_LEFT`.
    ///
    /// A mask, not a count: the compositor can send a release for a button
    /// pressed before this surface had focus, and a count would go negative
    /// (or, unsigned, end a grab that is still live).
    held: u32,
    /// Buttons at or past [`HIGH_BUTTON`], tracked by code.
    ///
    /// The mask has 32 bits and evdev's button codes do not stop at 32: a
    /// tablet or a gaming mouse reports codes far above `BTN_LEFT + 31`, and
    /// folding all of them onto one saturated bit means releasing any one of
    /// them ends a drag another is still holding.
    held_high: std::collections::BTreeSet<u32>,
}

/// The first button code too large for [`ImplicitGrab`]'s bitmask.
const HIGH_BUTTON: u32 = 0x110 + 31;

impl ImplicitGrab {
    /// Record a press. `true` if this press began the grab.
    pub fn press(&mut self, button: u32, target: &Node) -> bool {
        let began = !self.is_held();
        if button >= HIGH_BUTTON {
            self.held_high.insert(button);
        } else {
            self.held |= button_bit(button);
        }
        if began {
            self.target = Some(target.clone());
        }
        began
    }

    /// Record a release. `true` if this release ended the grab.
    pub fn release(&mut self, button: u32) -> bool {
        let was_held = if button >= HIGH_BUTTON {
            self.held_high.remove(&button)
        } else {
            let bit = button_bit(button);
            let held = self.held & bit != 0;
            self.held &= !bit;
            held
        };
        if !was_held {
            return false;
        }
        if !self.is_held() {
            self.target = None;
            return true;
        }
        false
    }

    /// The node every motion and release currently goes to.
    #[must_use]
    pub fn target(&self) -> Option<Node> {
        self.target.clone()
    }

    #[must_use]
    pub fn is_held(&self) -> bool {
        self.held != 0 || !self.held_high.is_empty()
    }

    /// Drop the grab outright: the pointer left, or the seat lost it.
    pub fn clear(&mut self) {
        self.target = None;
        self.held = 0;
        self.held_high.clear();
    }
}

/// One bit per pointer button, for the codes the mask can hold.
///
/// `BTN_LEFT` is 0x110 and the codes run upward; anything from
/// [`HIGH_BUTTON`] on is the caller's job (`held_high`), so the shift here is
/// always in range.
fn button_bit(button: u32) -> u32 {
    1u32 << button.saturating_sub(0x110).min(30)
}

/// One `wl_pointer.axis` frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scroll {
    pub dx: f32,
    pub dy: f32,
    pub source: ScrollSource,
    /// `wl_pointer.axis_stop` -- the finger left the touchpad, which is what
    /// starts a kinetic coast.
    pub stop: bool,
    pub time_ms: u32,
}

/// `wl_pointer.axis_source`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollSource {
    Wheel,
    Finger,
    Continuous,
    WheelTilt,
}

/// The fraction of a coast's velocity left after one second.
///
/// Chosen to match GTK's `GtkKineticScrolling` feel: fast at first, visibly
/// stopped within about a second.
const DECAY_PER_SECOND: f32 = 0.05;

/// Below this many pixels per second a coast is over.
pub const MIN_VELOCITY: f32 = 20.0;

/// The shortest interval a motion delta is ever rated over, in seconds.
///
/// `wl_pointer.axis_stop` is the one frame with no *following* frame to time
/// the previous motion against, and compositors routinely send it in the same
/// millisecond as the last motion. Rating that motion over a zero interval
/// would either discard it or blow up to infinity; rating it over one 60 Hz
/// frame is the honest lower bound on how long it took.
const MIN_RATE_DT: f32 = 0.016;

/// Kinetic deceleration for finger scrolls.
#[derive(Debug, Default)]
pub struct Kinetic {
    velocity: (f32, f32),
    /// A motion delta received but not yet rated: `wl_pointer.axis` reports
    /// distance already travelled, not a rate, so it takes the *next* event's
    /// arrival time to know how long that distance took. Applied and cleared
    /// on the following `feed` call, whether that call is more motion or the
    /// stop.
    pending: Option<(f32, f32, Duration)>,
    last: Option<Duration>,
    /// The delta `sample` last handed out, so a duplicate call at the same
    /// timestamp (two callers racing the same frame, or a caller re-reading
    /// after a spurious wakeup) hands back the same answer instead of a
    /// fabricated zero that would make the *next* real sample look like an
    /// increase.
    last_delta: (f32, f32),
    coasting: bool,
}

impl Kinetic {
    /// Feed one scroll frame.
    ///
    /// While the finger is down this tracks velocity; `axis_stop` starts the
    /// coast. Every other source cancels: a wheel click is a discrete step, and
    /// coasting one would scroll a list on every notch.
    pub fn feed(&mut self, scroll: &Scroll, now: Duration) {
        if scroll.source != ScrollSource::Finger {
            self.cancel();
            return;
        }
        // Rate the previous event now that we know how long it took to reach
        // this one.
        if let Some((pdx, pdy, pending_at)) = self.pending.take() {
            let mut dt = now.saturating_sub(pending_at).as_secs_f32();
            // On lift-off there is no *later* frame to time this one against,
            // so a stop frame that shares its predecessor's timestamp gets the
            // frame-time floor rather than being thrown away: the last motion
            // before the finger left is the one that defines the flick.
            if scroll.stop {
                dt = dt.max(MIN_RATE_DT);
            }
            // A zero or backwards step tells us nothing about velocity;
            // keeping the previous estimate is the only finite answer.
            if dt > f32::EPSILON {
                self.blend((pdx / dt, pdy / dt));
            }
        }
        if scroll.stop {
            // A stop frame normally carries no distance of its own, but one
            // that does (a synthesised single-frame flick) is the same last
            // motion, rated at the same floor.
            if scroll.dx != 0.0 || scroll.dy != 0.0 {
                self.blend((scroll.dx / MIN_RATE_DT, scroll.dy / MIN_RATE_DT));
            }
            self.coasting = self.speed() >= MIN_VELOCITY;
            self.last = Some(now);
            return;
        }
        self.coasting = false;
        self.pending = Some((scroll.dx, scroll.dy, now));
    }

    /// The delta to apply this frame; `None` once stopped.
    pub fn sample(&mut self, now: Duration) -> Option<(f32, f32)> {
        if !self.coasting {
            return None;
        }
        let last = self.last?;
        let dt = now.saturating_sub(last).as_secs_f32();
        if dt <= 0.0 {
            return Some(self.last_delta);
        }
        self.last = Some(now);
        let delta = (self.velocity.0 * dt, self.velocity.1 * dt);
        let decay = DECAY_PER_SECOND.powf(dt);
        self.velocity = (self.velocity.0 * decay, self.velocity.1 * decay);
        if !delta.0.is_finite() || !delta.1.is_finite() {
            self.cancel();
            return None;
        }
        self.last_delta = delta;
        if self.speed() < MIN_VELOCITY {
            self.coasting = false;
        }
        Some(delta)
    }

    /// Stop dead: a new touch, a new scroll, a focus change.
    pub fn cancel(&mut self) {
        self.velocity = (0.0, 0.0);
        self.pending = None;
        self.last = None;
        self.last_delta = (0.0, 0.0);
        self.coasting = false;
    }

    /// Fold one instantaneous velocity estimate into the running one.
    ///
    /// Half old, half new: one jittery frame must not define the flick, and
    /// the last frames before lift-off must dominate. A non-finite estimate
    /// (a clock that went backwards, a zero interval) is dropped rather than
    /// poisoning every scroll offset downstream.
    fn blend(&mut self, instant: (f32, f32)) {
        if instant.0.is_finite() && instant.1.is_finite() {
            self.velocity = (
                self.velocity.0.mul_add(0.5, instant.0 * 0.5),
                self.velocity.1.mul_add(0.5, instant.1 * 0.5),
            );
        }
    }

    fn speed(&self) -> f32 {
        self.velocity.0.hypot(self.velocity.1)
    }
}

/// A CSS/GTK cursor name as a `wp_cursor_shape_v1` shape.
///
/// Contract deviation 9: this takes a *name*, not a `&ComputedStyle`. GTK 4 has
/// no `cursor` CSS property -- M2's registry holds the 114 properties GTK
/// actually parses and `ui/tests/gtk4_property_reference.rs` pins that count --
/// so a widget declares its cursor by name, exactly as
/// `gtk_widget_set_cursor_from_name` does.
///
/// Unknown names and `url()` values fall back to `Default`: client-side cursor
/// themes are out of scope for M3, and icedtea supports `wp_cursor_shape_v1`.
#[must_use]
pub fn cursor_shape_for(name: &str) -> CursorShape {
    let lowered = name.trim().to_ascii_lowercase();
    match lowered.as_str() {
        "context-menu" => CursorShape::ContextMenu,
        "help" => CursorShape::Help,
        "pointer" => CursorShape::Pointer,
        "progress" => CursorShape::Progress,
        "wait" => CursorShape::Wait,
        "cell" => CursorShape::Cell,
        "crosshair" => CursorShape::Crosshair,
        "text" => CursorShape::Text,
        "vertical-text" => CursorShape::VerticalText,
        "alias" => CursorShape::Alias,
        "copy" => CursorShape::Copy,
        "move" => CursorShape::Move,
        "no-drop" => CursorShape::NoDrop,
        "not-allowed" => CursorShape::NotAllowed,
        "grab" => CursorShape::Grab,
        "grabbing" => CursorShape::Grabbing,
        "e-resize" => CursorShape::EResize,
        "n-resize" => CursorShape::NResize,
        "ne-resize" => CursorShape::NeResize,
        "nw-resize" => CursorShape::NwResize,
        "s-resize" => CursorShape::SResize,
        "se-resize" => CursorShape::SeResize,
        "sw-resize" => CursorShape::SwResize,
        "w-resize" => CursorShape::WResize,
        "ew-resize" => CursorShape::EwResize,
        "ns-resize" => CursorShape::NsResize,
        "nesw-resize" => CursorShape::NeswResize,
        "nwse-resize" => CursorShape::NwseResize,
        "col-resize" => CursorShape::ColResize,
        "row-resize" => CursorShape::RowResize,
        "all-scroll" => CursorShape::AllScroll,
        "zoom-in" => CursorShape::ZoomIn,
        "zoom-out" => CursorShape::ZoomOut,
        _ => CursorShape::Default,
    }
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

    use super::{
        CursorShape, ImplicitGrab, Kinetic, MIN_VELOCITY, Scroll, ScrollSource, cursor_shape_for,
    };
    use std::time::Duration;

    const BTN_LEFT: u32 = 0x110;
    const BTN_RIGHT: u32 = 0x111;

    fn finger(dx: f32, dy: f32, stop: bool) -> Scroll {
        Scroll {
            dx,
            dy,
            source: ScrollSource::Finger,
            stop,
            time_ms: 0,
        }
    }

    #[test]
    fn a_press_captures_the_pointer_until_the_last_button_is_released() {
        // The client-side mirror of the compositor's implicit grab (which the
        // wlr crate's own `implicit_grab_tests` pin on the server side):
        // motion and the release go to the pressed node, not to whatever is
        // under the pointer now.
        let pressed = Node::new("button");
        let elsewhere = Node::new("label");
        let mut grab = ImplicitGrab::default();
        assert!(!grab.is_held());
        assert!(grab.target().is_none());

        assert!(
            grab.press(BTN_LEFT, &pressed),
            "the first press begins the grab"
        );
        assert!(grab.is_held());
        assert!(grab.target().expect("a target").ptr_eq(&pressed));

        assert!(
            !grab.press(BTN_RIGHT, &elsewhere),
            "a second button does not begin a new grab"
        );
        assert!(
            grab.target().expect("a target").ptr_eq(&pressed),
            "and does not move the target"
        );

        assert!(!grab.release(BTN_RIGHT), "one button left: still grabbed");
        assert!(grab.is_held());
        assert!(grab.release(BTN_LEFT), "the last release ends the grab");
        assert!(!grab.is_held());
        assert!(grab.target().is_none());
    }

    #[test]
    fn a_release_of_a_button_that_was_never_pressed_is_ignored() {
        // The compositor sends a release for a button pressed before we had
        // focus; ending the grab on it would drop a real drag.
        let pressed = Node::new("button");
        let mut grab = ImplicitGrab::default();
        grab.press(BTN_LEFT, &pressed);
        assert!(!grab.release(BTN_RIGHT));
        assert!(grab.is_held(), "an unrelated release must not end the grab");
        grab.clear();
        assert!(!grab.is_held(), "a pointer leave clears it outright");
        assert!(
            !grab.release(BTN_LEFT),
            "and a late release afterwards is a no-op"
        );
    }

    #[test]
    fn buttons_past_the_masks_width_are_tracked_one_by_one() {
        // Every code from BTN_LEFT + 31 up used to saturate onto the same
        // bit, so a tablet's stylus buttons (0x130 and up) shared one: the
        // release of either ended a drag the other still held.
        let pressed = Node::new("button");
        let mut grab = ImplicitGrab::default();
        assert!(grab.press(0x130, &pressed), "the first press begins it");
        assert!(!grab.press(0x131, &pressed));
        assert!(
            !grab.release(0x130),
            "0x131 is still down: the grab must survive"
        );
        assert!(grab.is_held());
        assert!(grab.target().is_some());
        assert!(grab.release(0x131), "the last one ends it");
        assert!(!grab.is_held());
        assert!(grab.target().is_none());

        // A low and a high code together, and a release of one never
        // released before.
        grab.press(BTN_LEFT, &pressed);
        grab.press(0x140, &pressed);
        assert!(!grab.release(0x141), "never pressed: a no-op");
        assert!(grab.is_held());
        assert!(!grab.release(BTN_LEFT));
        assert!(grab.release(0x140));
        assert!(!grab.is_held());
    }

    #[test]
    fn only_a_finger_scroll_coasts() {
        let mut kinetic = Kinetic::default();
        for source in [
            ScrollSource::Wheel,
            ScrollSource::WheelTilt,
            ScrollSource::Continuous,
        ] {
            kinetic.feed(
                &Scroll {
                    dx: 0.0,
                    dy: -40.0,
                    source,
                    stop: false,
                    time_ms: 0,
                },
                Duration::from_millis(0),
            );
            kinetic.feed(
                &Scroll {
                    dx: 0.0,
                    dy: 0.0,
                    source,
                    stop: true,
                    time_ms: 10,
                },
                Duration::from_millis(10),
            );
            assert!(
                kinetic.sample(Duration::from_millis(20)).is_none(),
                "{source:?} must not coast: a wheel click is a discrete step"
            );
        }
    }

    #[test]
    fn a_flicked_finger_scroll_coasts_and_then_stops() {
        let mut kinetic = Kinetic::default();
        // 100 px over 100 ms == 1000 px/s.
        kinetic.feed(&finger(0.0, -50.0, false), Duration::from_millis(0));
        kinetic.feed(&finger(0.0, -50.0, false), Duration::from_millis(50));
        kinetic.feed(&finger(0.0, 0.0, true), Duration::from_millis(100));

        let first = kinetic
            .sample(Duration::from_millis(116))
            .expect("it coasts");
        assert!(
            first.1 < 0.0,
            "the coast continues in the flick's direction: {first:?}"
        );
        assert_eq!(
            first.0, 0.0,
            "a purely vertical flick has no horizontal coast"
        );

        let mut deltas = vec![first.1.abs()];
        let mut now = 116u64;
        while let Some(delta) = kinetic.sample(Duration::from_millis(now)) {
            deltas.push(delta.1.abs());
            now += 16;
            assert!(now < 10_000, "the coast never stopped");
        }
        assert!(deltas.len() > 2, "one frame is not a coast: {deltas:?}");
        assert!(
            deltas.windows(2).all(|w| w[1] <= w[0] + 0.001),
            "each frame must move no further than the last: {deltas:?}"
        );
        assert!(
            kinetic.sample(Duration::from_millis(now + 16)).is_none(),
            "once stopped it stays stopped"
        );
    }

    #[test]
    fn the_lift_off_frame_does_not_halve_the_flicks_velocity() {
        // `wl_pointer.axis_stop` carries no distance. Rating that zero as a
        // motion frame blended a 0 px/s instant into the estimate at 50%, so
        // every finger lift-off halved the coast before it began.
        //
        // Two 50 px frames 50 ms apart are 1000 px/s each; the blend leaves
        // 750 px/s (0 -> 500 -> 750), and the stop frame must not touch it.
        let mut kinetic = Kinetic::default();
        kinetic.feed(&finger(0.0, -50.0, false), Duration::from_millis(0));
        kinetic.feed(&finger(0.0, -50.0, false), Duration::from_millis(50));
        kinetic.feed(&finger(0.0, -50.0, false), Duration::from_millis(100));
        kinetic.feed(&finger(0.0, 0.0, true), Duration::from_millis(150));

        // The first sampled frame is velocity * dt, before any decay is
        // applied to that frame's own delta.
        let dt = 0.016_f32;
        let first = kinetic
            .sample(Duration::from_millis(166))
            .expect("it coasts");
        let speed = -first.1 / dt;
        assert!(
            (speed - 875.0).abs() < 25.0,
            "the lift-off halved the flick: {speed} px/s, expected ~875"
        );
    }

    #[test]
    fn a_new_scroll_or_a_cancel_ends_the_coast() {
        let mut kinetic = Kinetic::default();
        kinetic.feed(&finger(0.0, -50.0, false), Duration::from_millis(0));
        kinetic.feed(&finger(0.0, 0.0, true), Duration::from_millis(50));
        assert!(kinetic.sample(Duration::from_millis(66)).is_some());
        kinetic.cancel();
        assert!(
            kinetic.sample(Duration::from_millis(82)).is_none(),
            "touching the list must stop it dead, as GTK does"
        );
    }

    #[test]
    // MIN_VELOCITY is a compile-time constant; clippy is right that this
    // particular assertion can never fail, and it stays anyway as a guard
    // against the constant itself regressing to a non-positive value.
    #[allow(clippy::assertions_on_constants)]
    fn a_zero_or_backwards_time_step_never_produces_nan() {
        // `time_ms` comes from the compositor and the clock from us; neither
        // is guaranteed monotonic across a suspend. A NaN delta would poison
        // every scroll offset downstream.
        let mut kinetic = Kinetic::default();
        kinetic.feed(&finger(0.0, -50.0, false), Duration::from_millis(100));
        kinetic.feed(&finger(0.0, -50.0, false), Duration::from_millis(100));
        kinetic.feed(&finger(0.0, -50.0, false), Duration::from_millis(50));
        kinetic.feed(&finger(0.0, 0.0, true), Duration::from_millis(100));
        for at in [100u64, 100, 50, 200] {
            if let Some((dx, dy)) = kinetic.sample(Duration::from_millis(at)) {
                assert!(dx.is_finite() && dy.is_finite(), "{dx},{dy} is not finite");
            }
        }
        assert!(MIN_VELOCITY > 0.0);
    }

    #[test]
    fn cursor_names_map_to_the_protocols_shapes() {
        // The CSS/GTK cursor names widgets actually use (contract deviation 9:
        // this is a name, not a CSS property -- GTK 4 has neither).
        assert_eq!(cursor_shape_for("default"), CursorShape::Default);
        assert_eq!(cursor_shape_for("text"), CursorShape::Text);
        assert_eq!(cursor_shape_for("pointer"), CursorShape::Pointer);
        assert_eq!(cursor_shape_for("grab"), CursorShape::Grab);
        assert_eq!(cursor_shape_for("grabbing"), CursorShape::Grabbing);
        assert_eq!(cursor_shape_for("col-resize"), CursorShape::ColResize);
        assert_eq!(cursor_shape_for("row-resize"), CursorShape::RowResize);
        assert_eq!(cursor_shape_for("ew-resize"), CursorShape::EwResize);
        assert_eq!(cursor_shape_for("ns-resize"), CursorShape::NsResize);
        assert_eq!(cursor_shape_for("not-allowed"), CursorShape::NotAllowed);
        assert_eq!(cursor_shape_for("progress"), CursorShape::Progress);
        assert_eq!(cursor_shape_for("wait"), CursorShape::Wait);
        assert_eq!(cursor_shape_for("crosshair"), CursorShape::Crosshair);
        assert_eq!(cursor_shape_for("help"), CursorShape::Help);
        assert_eq!(cursor_shape_for("context-menu"), CursorShape::ContextMenu);
        // Case-insensitive, and anything unknown -- including the `url()` GTK
        // themes use for custom cursors, which we do not support -- is Default.
        assert_eq!(cursor_shape_for("TEXT"), CursorShape::Text);
        assert_eq!(cursor_shape_for("url(hand.png)"), CursorShape::Default);
        assert_eq!(cursor_shape_for(""), CursorShape::Default);
        assert_eq!(cursor_shape_for("zoom-sideways"), CursorShape::Default);
    }
}
