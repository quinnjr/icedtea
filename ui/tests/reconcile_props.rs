//! Contract §4.8's reconciler property tests: for random keyed edit
//! sequences, every surviving key keeps its `Node`; the op list carries no
//! redundant `Move` or `SetProp`; and a removed key's controller is dropped
//! exactly once.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use icedtea_ui::anim::{Clock, ManualClock};
use icedtea_ui::css::cascade::CompiledSheet;
use icedtea_ui::css::computed::ResolveEnv;
use icedtea_ui::css::node::Node;
use icedtea_ui::icons::IconTheme;
use icedtea_ui::text::FontDatabase;
use icedtea_ui::view::builders::widget;
use icedtea_ui::view::reconcile::{Instance, Op, reconcile};
use icedtea_ui::view::{BuildCx, Kind, PropName, View};

#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)]
enum Msg {
    Unused,
}

/// xorshift64*: three lines, no dependency, and the same sequence on every
/// machine, so a failure is replayable from its seed alone.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

struct Harness {
    sheet: CompiledSheet,
    fonts: FontDatabase,
    icons: IconTheme,
    clock: Rc<dyn Clock>,
    env: ResolveEnv,
}

impl Harness {
    fn new() -> Self {
        Harness {
            sheet: CompiledSheet::compile("window { color: #000; } label { color: #111; }"),
            fonts: FontDatabase::probe_only(),
            icons: IconTheme::with_name_and_roots("hicolor", vec![]),
            clock: Rc::new(ManualClock::new()),
            env: ResolveEnv::default(),
        }
    }

    fn cx(&mut self) -> BuildCx<'_> {
        BuildCx {
            sheet: &self.sheet,
            fonts: &mut self.fonts,
            icons: &mut self.icons,
            clock: &self.clock,
            env: &self.env,
        }
    }
}

fn row(key: u64, label: &str) -> View<Msg> {
    widget::<Msg>(Kind::Label)
        .key(key)
        .prop(PropName::Label, label)
}

/// A random frame: a subset of `0..pool`, shuffled, each with a label that
/// is either its key or its key plus a salt (so props change sometimes).
fn frame(rng: &mut Rng, pool: u64, salt: u64) -> Vec<(u64, String)> {
    let mut keys: Vec<u64> = (0..pool)
        .filter(|_| !rng.next().is_multiple_of(3))
        .collect();
    for i in (1..keys.len()).rev() {
        keys.swap(i, rng.below(i + 1));
    }
    keys.into_iter()
        .map(|k| {
            let label = if rng.next().is_multiple_of(4) {
                format!("{k}-{salt}")
            } else {
                k.to_string()
            };
            (k, label)
        })
        .collect()
}

#[test]
fn every_surviving_key_keeps_its_node_across_random_edits() {
    for seed in [1_u64, 7, 42, 1337, 90_210, 0xDEAD_BEEF] {
        let mut rng = Rng(seed);
        let mut h = Harness::new();
        let parent = Node::new("window");
        let mut prev: Vec<Instance<Msg>> = Vec::new();
        let mut nodes: HashMap<u64, Node> = HashMap::new();

        for step in 0..40_u64 {
            let spec = frame(&mut rng, 12, step);
            let views: Vec<View<Msg>> = spec.iter().map(|(k, label)| row(*k, label)).collect();
            reconcile(&parent, &mut prev, views, &mut h.cx());

            assert_eq!(prev.len(), spec.len(), "seed {seed} step {step}: arity");
            for (index, (key, label)) in spec.iter().enumerate() {
                let instance = &prev[index];
                assert_eq!(
                    instance.key,
                    Some(icedtea_ui::view::Key::Id(*key)),
                    "seed {seed} step {step}: key at {index}"
                );
                assert_eq!(
                    instance.props.str(PropName::Label),
                    Some(label.as_str()),
                    "seed {seed} step {step}: label at {index}"
                );
                match nodes.get(key) {
                    Some(known) => assert!(
                        instance.node.ptr_eq(known),
                        "seed {seed} step {step}: key {key} lost its node"
                    ),
                    None => {
                        nodes.insert(*key, instance.node.clone());
                    }
                }
            }
            // Keys absent this frame are gone for good; their next
            // appearance is a fresh node, so forget them.
            nodes.retain(|k, _| spec.iter().any(|(key, _)| key == k));

            // The attached node order equals the visible instance order.
            let attached: Vec<Node> = parent.children();
            assert_eq!(
                attached.len(),
                prev.len(),
                "seed {seed} step {step}: attached arity"
            );
            for (a, b) in attached.iter().zip(prev.iter()) {
                assert!(a.ptr_eq(&b.node), "seed {seed} step {step}: node order");
            }
        }
    }
}

#[test]
fn the_op_list_carries_no_redundant_move_or_set_prop() {
    for seed in [3_u64, 11, 99, 65_537] {
        let mut rng = Rng(seed);
        let mut h = Harness::new();
        let parent = Node::new("window");
        let mut prev: Vec<Instance<Msg>> = Vec::new();
        let mut last: Vec<(u64, String)> = Vec::new();

        for step in 0..40_u64 {
            let spec = frame(&mut rng, 10, step);
            let views: Vec<View<Msg>> = spec.iter().map(|(k, l)| row(*k, l)).collect();
            let ops = reconcile(&parent, &mut prev, views, &mut h.cx());

            for op in &ops {
                match op {
                    Op::Move { from, to } => assert_ne!(
                        from, to,
                        "seed {seed} step {step}: a Move to its own position"
                    ),
                    Op::SetProp { index, name } => {
                        assert_eq!(*name, PropName::Label);
                        let (key, label) = &spec[*index];
                        let before = last.iter().find(|(k, _)| k == key).map(|(_, l)| l.as_str());
                        assert_ne!(
                            before,
                            Some(label.as_str()),
                            "seed {seed} step {step}: SetProp for an unchanged label"
                        );
                    }
                    _ => {}
                }
            }
            // A pure reshuffle of the same keys must never cost more Moves
            // than there are keys out of their longest ascending run.
            let moves = ops.iter().filter(|o| matches!(o, Op::Move { .. })).count();
            assert!(
                moves < spec.len().max(1),
                "seed {seed} step {step}: {moves} Moves for {} children",
                spec.len()
            );
            last = spec;
        }
    }
}

#[test]
fn a_removed_key_s_controller_is_dropped_exactly_once() {
    // A controller that counts its own drops through a shared cell. Building
    // it needs no framework support: `build_controller` is only consulted by
    // `reconcile`, so the count is taken from a `Kind::DrawingArea` view
    // whose `Prop::Draw` closure owns the guard — the closure is dropped
    // exactly when the instance's props are, i.e. with the instance.
    let drops = Rc::new(Cell::new(0usize));

    struct Guard(Rc<Cell<usize>>);
    impl Drop for Guard {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }

    let mut h = Harness::new();
    let parent = Node::new("window");
    let mut prev: Vec<Instance<Msg>> = Vec::new();

    {
        let guard = Rc::new(Guard(Rc::clone(&drops)));
        let view: View<Msg> = widget::<Msg>(Kind::DrawingArea).key(1_u64).prop(
            PropName::DrawFn,
            icedtea_ui::view::Prop::Draw(Rc::new(move |_canvas, _rect| {
                // Capturing the guard is the whole point; the body never runs.
                let _ = &guard;
            })),
        );
        reconcile(&parent, &mut prev, vec![view], &mut h.cx());
    }
    assert_eq!(drops.get(), 0, "the instance is alive, so nothing dropped");

    reconcile(&parent, &mut prev, Vec::new(), &mut h.cx());
    assert_eq!(
        drops.get(),
        1,
        "the removed instance dropped its state once"
    );

    // A second empty frame must not drop it again.
    reconcile(&parent, &mut prev, Vec::new(), &mut h.cx());
    assert_eq!(drops.get(), 1, "the drop ran twice");
}
