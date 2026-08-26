# Pure-Rust GTK-themed UI — M2 Part 2: Node tree and complete selector matching — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

## Contract deviations

Three, all narrow. Everything else in Part 0 §3 / §3.1 / §3.2 is used verbatim.

1. **`Element::opaque()` is `OpaqueElement::new(&*self.0)`, not `OpaqueElement::new(Rc::as_ptr(&self.0))`.**
   `selectors-0.40.0/tree.rs:26` declares `pub fn new<T>(ptr: &T) -> Self`. Passing
   `Rc::as_ptr(&self.0)` (a `*const NodeInner`) would infer `T = *const NodeInner`
   and take the address of a *temporary stack slot holding the pointer* — a
   different, non-unique, dangling-after-the-statement address. `&*self.0`
   produces the `NodeInner` payload address the contract intends, and is exactly
   what M1's `CssNode::opaque` already does.

2. **`css::select::PseudoStates` (M1's 7-bool struct) and `css::node::PseudoStates`
   (the bitflags) coexist for the duration of Part 2.** Part 0 §10.1 retires the
   M1 struct, but that retirement is bundled with deleting `CssNode`, which
   `cascade.rs`, `computed.rs`, `widget/button.rs`, `app.rs`, `paint.rs` and the
   byte-identical M1 gate test `ui/tests/themed_button_offscreen.rs` still consume
   — all of them Part 3's to migrate (§11, P3 "Implements … §10.1"). Part 2 is
   therefore purely additive with respect to `CssNode`: it keeps `CssNode`, its
   `Element` impl and `select::PseudoStates` compiling and green, and imports the
   new flags as `use crate::css::node::PseudoStates as NodeStates;`. Part 3 deletes
   `CssNode`, the M1 struct and the alias in one move.

3. **`matches`/`matches_with_specificity` seed the bloom filter themselves.**
   The contract hands callers `MatchCx::seed_for`; a caller who forgets it would
   match against an *empty* filter, which fast-rejects **every** selector that has
   any ancestor hash — silently losing rules rather than merely losing speed. Both
   entry points therefore call `cx.seed_for(node)` first; `seed_for` is a no-op
   when the filter is already positioned for that node at that tree generation, so
   the 900-rule-per-node cascade loop pays for the ancestor walk once. `seed_for`,
   `push_ancestor` and `pop_ancestor` keep their contract signatures and meaning.

**Flagged, implemented as written (not a deviation):** Part 0 §3 says setting
`FOCUS` propagates *both* `FOCUS_WITHIN` **and** `FOCUS_VISIBLE` up every
ancestor. Propagating `FOCUS_VISIBLE` to ancestors diverges from CSS (where
`:focus-visible` is a property of the focused element alone) and will make
`:focus-visible` rules match container nodes. It is spelled out twice in the
contract, so this plan implements it verbatim and documents the divergence at the
implementation site; if Part 3 finds it wrong, it is a one-line change to
`PseudoStates::DERIVED`.

**Assumption Part 2 executes against:** at the start of this part the crate builds
and `cargo test -p icedtea-ui` is green with Part 1 landed — `bitflags = "2"` is in
`ui/Cargo.toml`, `css/tokens.rs` exists (`css/value.rs` renamed), `css/registry.rs`
and `css/value/**` exist, and `css::select::CssNode` plus `css::cascade::{CompiledRule,
CompiledSheet}` are still present in their M1 shape. Part 2 touches none of Part 1's
files.

---

**Goal:** Replace M1's two-node immutable `CssNode` chain with a real mutable
styled-node tree and a complete `selectors::Element` implementation over it, so
every GTK 4 selector construct — siblings, `:nth-*`, `:root`, `:focus-within`,
ids, `:dir()`, `:drop(active)` — matches correctly and at speed on real trees.

**Architecture:** `css/node.rs` owns the tree: `Node` is an `Rc<NodeInner>` handle
with interior-mutable identity (name, id, classes), `PseudoStates` bitflags,
ordered children, a `Weak` parent, and a per-tree generation counter that every
mutation bumps. `css/select.rs` owns matching: the `GtkPseudoClass` set widens to
GTK's full list, `impl Element for Node` walks that tree for real (siblings,
first-child, emptiness, root, ids, unique hashes), `MatchCx` carries the
caller-owned `SelectorCaches` plus a `BloomFilter` positioned on the node's
ancestors, and `RuleBuckets` indexes compiled rules by the rightmost compound's
id/class/name so a node only sees candidate rules. `CssNode` and its matching path
stay untouched underneath until Part 3 migrates the crate onto `Node`.

**Tech Stack:** Rust (edition 2024, rust-version 1.94), `selectors` 0.40
(`Element`, `SelectorList`, `MatchingContext`, `BloomFilter`, `AncestorHashes`,
`Component`), `cssparser` 0.37 (selector-list parsing, `ToCss`),
`precomputed-hash` 0.1, `bitflags` 2, `tracing`.

**Spec:** `docs/superpowers/specs/2026-08-26-pure-rust-gtk-m2-css-engine-design.md`
(§Section 2 "Node tree & selectors"; §Section 7 testing strategy). Frozen
interface contract: `docs/superpowers/plans/2026-08-26-m2-part0-contract.md`
(§0 module map, §3 Node tree, §3.1 `Element` obligations, §3.2 `MatchCx` /
`RuleBuckets`, §10 migration, §11 P2 boundaries). Parent spec:
`docs/superpowers/specs/2026-08-20-pure-rust-gtk-ui-design.md`.

**Research notes:** `.superpowers/m2-plan-notes/cssparser-selectors.md` (§3 the
`:nth-*` family is built into `selectors`, §5 combinators need real sibling
methods, §6 the bloom filter is hash-based only, §7 no bucketing helper exists —
it is 100% project code), `.superpowers/m2-plan-notes/current-crate.md` (§`select.rs`
— the four constant-stubbed `Element` methods).

## Global Constraints

- Crate: `ui/` = `icedtea-ui`. Part 2 may create/modify **only**
  `ui/src/css/node.rs`, `ui/src/css/select.rs`, `ui/src/css/mod.rs`. No edits to
  `registry.rs`, `tokens.rs`, `value/**`, `parse.rs`, `cascade.rs`, `computed.rs`,
  `layout.rs`, `paint*`, `text.rs`, `anim/`, `widget/`, `app.rs`, `Cargo.toml`.
- Pinned crate versions, unchanged: `cssparser 0.37`, `selectors 0.40`,
  `precomputed-hash 0.1`, `taffy 0.14`, `skia-rs-safe 0.4.0`, `wayland-client 0.31`,
  `bitflags 2` (added by Part 1). No new dependency is added by this part.
- No `smithay`, no `gtk4`/`gtk4-rs`, no GObject. Pure Rust plus already-linked
  system C libraries only.
- Edition 2024, `rust-version` 1.94 — both already set in `ui/Cargo.toml`; do not
  change them.
- Gates, all three green at every commit:
  `cargo test -p icedtea-ui`, `cargo clippy -p icedtea-ui --all-targets -- -D warnings`,
  `cargo fmt --all --check`.
- Every commit message ends with the trailer
  `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`.
- **The M1 pixel gate `ui/tests/themed_button_offscreen.rs` must stay green and
  byte-identical.** Part 0 §10.2 forbids edits of any kind, imports included. It
  consumes `css::select::{CssNode, PseudoStates}`; Part 2 keeps both alive
  (Contract deviation 2).
- The other byte-identical M1 gate files (`src/css/parse.rs`'s 16 tests,
  `src/css/tokens.rs`'s 4, `src/shm.rs`'s 11, `src/wayland.rs`'s 11, `src/lib.rs`'s 1,
  `tests/layer_shell_screencopy.rs`'s 3) are untouched by this part.
- `src/css/select.rs`'s 9 M1 tests are **rewritten, not deleted** (Part 0 §10.3):
  every assertion survives, re-expressed against `Node`. A rewrite that drops an
  assertion fails review.
- Parts execute in order 1 → 6 on the one branch `rebuild/pure-rust-gtk-m2`, so
  Part 1's `Produces` are available; Parts 3–6 consume this part's `Produces`
  exactly as written below.
- Nothing outside `css::registry` names a CSS property string. Part 2 names no
  property strings at all.

---

## File Structure

| File | Status | Responsibility |
|---|---|---|
| `ui/src/css/node.rs` | Create | The styled-node tree: `Node`/`NodeInner`, `PseudoStates` bitflags, `CssString`, `Direction`, `fnv1a`; construction, tree mutation, identity mutation, state and direction, generations, focus derivation, identity hashes. No matching logic. |
| `ui/src/css/select.rs` | Modify | Selector layer: `GtkSelectorImpl`/`GtkSelectorParser`/`GtkPseudoClass` (widened to GTK's full pseudo-class list), `impl Element for Node`, `MatchCx`, `RuleBuckets`, `parse_selector_list`, `matches`, `matches_with_specificity`. Re-exports `CssString`/`Direction` from `node.rs`. Keeps M1's `CssNode`, `NodeData` and `PseudoStates` struct untouched for Part 3. |
| `ui/src/css/mod.rs` | Modify | Declares `pub mod node;`. |

Tests live in `#[cfg(test)] mod tests` inside each of the two files, matching the
crate's established layout (every M1 module tests itself in-file; `ui/tests/` is
reserved for the Wayland/offscreen gates).

---

## Task 1: Node module skeleton — identity, interning, generations

**Files:**
- Create: `ui/src/css/node.rs`
- Modify: `ui/src/css/mod.rs:1-10` (add `pub mod node;`)
- Modify: `ui/src/css/select.rs:1-130` (delete the `CssString`, `fnv1a` and
  `Direction` definitions, re-export them from `node`)

**Interfaces:**
- Consumes: `bitflags 2` (added to `ui/Cargo.toml` by Part 1);
  `precomputed_hash::PrecomputedHash`; `cssparser::ToCss`;
  `selectors::bloom::BLOOM_HASH_MASK`.
- Produces:
  ```rust
  pub(crate) fn fnv1a(bytes: &[u8]) -> u32;
  pub struct CssString { /* private */ }
  impl CssString { pub fn new(text: &str) -> Self; pub fn as_str(&self) -> &str; }
  pub enum Direction { Ltr, Rtl }
  impl Direction { pub fn parse(text: &str) -> Option<Self>; pub(crate) fn as_str(self) -> &'static str; }
  pub struct PseudoStates: u16;   // bitflags; HOVER ACTIVE FOCUS FOCUS_VISIBLE
                                  // FOCUS_WITHIN CHECKED INDETERMINATE DISABLED
                                  // BACKDROP SELECTED DROP_ACTIVE LINK VISITED
  impl PseudoStates { pub const DERIVED: PseudoStates; }
  pub struct Node(/* Rc<NodeInner> */);
  impl Node {
      pub fn new(name: &str) -> Node;
      pub fn with_classes(name: &str, classes: &[&str]) -> Node;
      pub fn name(&self) -> Rc<str>;
      pub fn id(&self) -> Option<CssString>;
      pub fn classes(&self) -> Vec<CssString>;
      pub fn set_id(&self, id: Option<&str>);
      pub fn add_class(&self, class: &str) -> bool;
      pub fn remove_class(&self, class: &str) -> bool;
      pub fn set_classes(&self, classes: &[&str]);
      pub fn generation(&self) -> u64;
      pub fn self_generation(&self) -> u64;
      pub fn ptr_eq(&self, other: &Node) -> bool;
      pub(crate) fn addr(&self) -> usize;
      pub(crate) fn borrow_id<R>(&self, f: impl FnOnce(Option<&CssString>) -> R) -> R;
      pub(crate) fn borrow_classes<R>(&self, f: impl FnOnce(&[CssString]) -> R) -> R;
      pub(crate) fn for_each_identity_hash(&self, f: impl FnMut(u32));
  }
  ```
  `select.rs` re-exports: `pub use crate::css::node::{CssString, Direction};`

- [ ] **Step 1: Write the failing test**

Create `ui/src/css/node.rs` with the module header, then this test module at the
bottom of the file (the production items come in Step 3):

```rust
#[cfg(test)]
mod tests {
    use super::{CssString, Direction, Node, PseudoStates};

    #[test]
    fn a_new_node_carries_its_name_and_classes() {
        let node = Node::with_classes("button", &["suggested-action", "text-button"]);
        assert_eq!(&*node.name(), "button");
        assert_eq!(
            node.classes().iter().map(CssString::as_str).collect::<Vec<_>>(),
            vec!["suggested-action", "text-button"]
        );
        assert!(node.id().is_none(), "a fresh node has no id");
        let bare = Node::new("window");
        assert_eq!(&*bare.name(), "window");
        assert!(bare.classes().is_empty());
    }

    #[test]
    fn identity_mutation_is_idempotent_and_reports_change() {
        let node = Node::new("button");
        assert!(node.add_class("flat"), "adding a new class reports true");
        assert!(!node.add_class("flat"), "adding it twice reports false");
        assert!(node.remove_class("flat"), "removing a present class reports true");
        assert!(!node.remove_class("flat"), "removing an absent class reports false");

        node.set_classes(&["a", "b"]);
        assert_eq!(
            node.classes().iter().map(CssString::as_str).collect::<Vec<_>>(),
            vec!["a", "b"]
        );

        node.set_id(Some("add-color-button"));
        assert_eq!(node.id().as_ref().map(CssString::as_str), Some("add-color-button"));
        node.set_id(None);
        assert!(node.id().is_none());
    }

    #[test]
    fn every_identity_mutation_bumps_both_generations() {
        let node = Node::new("button");
        let mut generation = node.generation();
        let mut self_generation = node.self_generation();

        for mutate in [
            &(|n: &Node| n.set_id(Some("x"))) as &dyn Fn(&Node),
            &|n: &Node| { n.add_class("flat"); },
            &|n: &Node| { n.remove_class("flat"); },
            &|n: &Node| n.set_classes(&["a"]),
        ] {
            mutate(&node);
            assert!(
                node.generation() > generation,
                "tree generation must advance on every identity mutation"
            );
            assert!(
                node.self_generation() > self_generation,
                "self generation must advance on every identity mutation"
            );
            generation = node.generation();
            self_generation = node.self_generation();
        }

        // A no-op mutation must not advance anything: restyle keys off these.
        node.set_classes(&["a"]);
        assert_eq!(node.generation(), generation, "a no-op set_classes is not a change");
        assert_eq!(node.self_generation(), self_generation);
    }

    #[test]
    fn handles_to_the_same_node_are_equal_and_share_state() {
        let node = Node::new("button");
        let handle = node.clone();
        assert!(node.ptr_eq(&handle));
        assert!(!node.ptr_eq(&Node::new("button")), "same name, different node");
        handle.add_class("flat");
        assert_eq!(
            node.classes().iter().map(CssString::as_str).collect::<Vec<_>>(),
            vec!["flat"],
            "a clone is another handle to the same node, not a copy"
        );
    }

    #[test]
    fn identity_hashes_cover_name_id_and_every_class() {
        use selectors::bloom::BLOOM_HASH_MASK;

        let node = Node::with_classes("button", &["flat", "circular"]);
        node.set_id(Some("close"));
        let mut hashes = Vec::new();
        node.for_each_identity_hash(|hash| hashes.push(hash));
        hashes.sort_unstable();

        let mut expected = vec![
            super::fnv1a(b"button") & BLOOM_HASH_MASK,
            super::fnv1a(b"close") & BLOOM_HASH_MASK,
            super::fnv1a(b"flat") & BLOOM_HASH_MASK,
            super::fnv1a(b"circular") & BLOOM_HASH_MASK,
        ];
        expected.sort_unstable();
        assert_eq!(hashes, expected);
    }

    #[test]
    fn css_string_and_direction_keep_their_m1_behaviour() {
        let text = CssString::new("suggested-action");
        assert_eq!(text.as_str(), "suggested-action");
        assert_eq!(text, CssString::new("suggested-action"));
        assert_ne!(text, CssString::new("destructive-action"));

        assert_eq!(Direction::parse("ltr"), Some(Direction::Ltr));
        assert_eq!(Direction::parse("RTL"), Some(Direction::Rtl));
        assert_eq!(Direction::parse("sideways"), None);
        assert_eq!(Direction::default(), Direction::Ltr);
        assert_eq!(PseudoStates::default(), PseudoStates::empty());
    }
}
```

**Mutation checks.**
`a_new_node_carries_its_name_and_classes`: drop the class vector in `with_classes`
→ the `classes()` assertion fails.
`identity_mutation_is_idempotent_and_reports_change`: make `add_class` always
return `true` → the second assertion fails.
`every_identity_mutation_bumps_both_generations`: delete the `touch()` call in
`set_classes` → the loop's `generation()` assertion fails on the fourth mutation;
delete the early return on an unchanged class list → the final no-op assertion
fails.
`handles_to_the_same_node_are_equal_and_share_state`: make `Node::clone` deep-copy
the `NodeInner` → the shared-state assertion fails.
`identity_hashes_cover_name_id_and_every_class`: drop the class loop from
`for_each_identity_hash` → the vectors differ by two elements.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib css::node::tests`
Expected: FAIL to compile — `error[E0433]: failed to resolve: use of undeclared type
`Node`` (plus the same for `CssString`, `Direction`, `PseudoStates`, `fnv1a`), and
`error[E0432]: unresolved import` on `css::node` until `mod.rs` declares it.

- [ ] **Step 3: Write the implementation**

`ui/src/css/mod.rs` — add the module declaration in alphabetical position:

```rust
pub mod cascade;
pub mod colors;
pub mod computed;
pub mod node;
pub mod parse;
pub mod select;
pub mod shorthand;
pub mod value;
```

> If Part 1 has already deleted `colors`/`shorthand` and renamed `value` to
> `tokens`, keep whatever module list is in the file and insert only
> `pub mod node;` between `computed` and `parse`.

`ui/src/css/node.rs` — the whole file above the test module:

```rust
//! The styled-node tree the CSS engine matches, cascades and paints against.
//!
//! GTK widgets are not a DOM, but they are a *tree*: every node has an element
//! name (`window`, `button`, `label`), optional id, style classes, pseudo-class
//! state, a writing direction, a parent and ordered children — and nothing else
//! (no attributes, no namespaces, no shadow roots). [`Node`] models exactly that.
//!
//! M1's `css::select::CssNode` was immutable and had only a parent, which made
//! `:first-child`, `:last-child`, `:only-child`, `:nth-child()`, `+` and `~`
//! unimplementable. This tree is mutable through shared handles instead: a
//! [`Node`] is an `Rc` handle, cloning it yields another handle to the same node,
//! and every mutation bumps a per-tree generation so a restyle pass can tell what
//! changed without diffing.

use std::borrow::Borrow;
use std::cell::{Cell, RefCell};
use std::fmt;
use std::rc::{Rc, Weak};

use cssparser::ToCss;
use precomputed_hash::PrecomputedHash;
use selectors::bloom::BLOOM_HASH_MASK;

/// FNV-1a, 32-bit. Cheap, stable, and adequate for the ancestor-hash
/// filtering `selectors` uses it for.
pub(crate) fn fnv1a(bytes: &[u8]) -> u32 {
    let mut hash: u32 = 0x811C_9DC5;
    for &b in bytes {
        hash ^= u32::from(b);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

/// An interned CSS identifier with a cached hash.
///
/// `selectors` requires `PrecomputedHash` on its `Identifier`, `LocalName`
/// and `NamespaceUrl` types; one newtype covers all of them.
#[derive(Clone, Debug)]
pub struct CssString {
    text: String,
    hash: u32,
}

impl CssString {
    /// Intern `text`, computing its hash once.
    #[must_use]
    pub fn new(text: &str) -> Self {
        Self {
            text: text.to_string(),
            hash: fnv1a(text.as_bytes()),
        }
    }

    /// The underlying text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.text
    }
}

impl PartialEq for CssString {
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text
    }
}

impl Eq for CssString {}

impl Default for CssString {
    fn default() -> Self {
        Self::new("")
    }
}

impl From<&str> for CssString {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl Borrow<str> for CssString {
    fn borrow(&self) -> &str {
        &self.text
    }
}

impl PrecomputedHash for CssString {
    fn precomputed_hash(&self) -> u32 {
        self.hash
    }
}

impl ToCss for CssString {
    fn to_css<W: fmt::Write>(&self, dest: &mut W) -> fmt::Result {
        dest.write_str(&self.text)
    }
}

/// Writing direction, the argument `:dir()` matches against.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Direction {
    /// Left-to-right. GTK's default, and this engine's.
    #[default]
    Ltr,
    /// Right-to-left.
    Rtl,
}

impl Direction {
    /// Parse `ltr`/`rtl`, ASCII-case-insensitively.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        if text.eq_ignore_ascii_case("ltr") {
            Some(Self::Ltr)
        } else if text.eq_ignore_ascii_case("rtl") {
            Some(Self::Rtl)
        } else {
            None
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Ltr => "ltr",
            Self::Rtl => "rtl",
        }
    }
}

bitflags::bitflags! {
    /// The pseudo-class state a node carries.
    ///
    /// `FOCUS_WITHIN` and `FOCUS_VISIBLE` are **derived**, never set directly:
    /// see [`PseudoStates::DERIVED`] and [`Node::set_states`].
    #[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
    pub struct PseudoStates: u16 {
        /// `:hover`
        const HOVER = 1 << 0;
        /// `:active`
        const ACTIVE = 1 << 1;
        /// `:focus`
        const FOCUS = 1 << 2;
        /// `:focus-visible` — derived from `FOCUS`.
        const FOCUS_VISIBLE = 1 << 3;
        /// `:focus-within` — derived from a descendant's `FOCUS`.
        const FOCUS_WITHIN = 1 << 4;
        /// `:checked`
        const CHECKED = 1 << 5;
        /// `:indeterminate`
        const INDETERMINATE = 1 << 6;
        /// `:disabled`
        const DISABLED = 1 << 7;
        /// `:backdrop`
        const BACKDROP = 1 << 8;
        /// `:selected`
        const SELECTED = 1 << 9;
        /// `:drop(active)`
        const DROP_ACTIVE = 1 << 10;
        /// `:link` — GTK never sets it, so it never matches.
        const LINK = 1 << 11;
        /// `:visited` — GTK never sets it, so it never matches.
        const VISITED = 1 << 12;
    }
}

impl PseudoStates {
    /// The flags the tree derives from `FOCUS`. [`Node::set_states`] masks these
    /// out of its argument, and [`Node::states`] adds them back.
    ///
    /// GTK propagates *both* of these up the ancestor chain from the focused
    /// node (contract §3). That is a deliberate divergence from CSS, where
    /// `:focus-visible` belongs to the focused element alone.
    pub const DERIVED: PseudoStates =
        PseudoStates::FOCUS_WITHIN.union(PseudoStates::FOCUS_VISIBLE);
}

/// The interior-mutable payload behind a [`Node`].
struct NodeInner {
    /// Element name, interned for hashing and comparison.
    name: CssString,
    /// The same text as a cheap-to-clone handle, for [`Node::name`].
    name_rc: Rc<str>,
    id: RefCell<Option<CssString>>,
    classes: RefCell<Vec<CssString>>,
    /// Flags the caller set. Never contains [`PseudoStates::DERIVED`].
    own_states: Cell<PseudoStates>,
    /// How many nodes in this subtree (self included) carry `FOCUS`.
    focus_count: Cell<u32>,
    /// `None` == inherit the parent's direction.
    direction: Cell<Option<Direction>>,
    parent: RefCell<Weak<NodeInner>>,
    children: RefCell<Vec<Node>>,
    /// Shared by every node of one tree; bumped by every mutation anywhere.
    tree: RefCell<Rc<Cell<u64>>>,
    /// The value `tree` held when this node was last touched.
    self_generation: Cell<u64>,
}

/// A styled node: name, id, classes, pseudo-class state, direction, children.
///
/// `Clone` is another handle to the *same* node, not a copy — mutating through
/// one handle is visible through all of them, which is what makes a widget layer
/// able to hold onto its node while the tree is restructured around it.
#[derive(Clone)]
pub struct Node(Rc<NodeInner>);

impl fmt::Debug for Node {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Deliberately shallow: `Element` requires `Debug`, and a recursive dump
        // of a real widget tree in an assertion message is unreadable.
        f.debug_struct("Node")
            .field("name", &self.0.name.as_str())
            .field("id", &self.0.id.borrow().as_ref().map(CssString::as_str))
            .field(
                "classes",
                &self
                    .0
                    .classes
                    .borrow()
                    .iter()
                    .map(CssString::as_str)
                    .collect::<Vec<_>>(),
            )
            .field("states", &self.states())
            .field("children", &self.0.children.borrow().len())
            .finish()
    }
}

impl Node {
    /// A detached node with no classes, no id and default state.
    #[must_use]
    pub fn new(name: &str) -> Node {
        Node::with_classes(name, &[])
    }

    /// A detached node with the given style classes, in order.
    #[must_use]
    pub fn with_classes(name: &str, classes: &[&str]) -> Node {
        Node(Rc::new(NodeInner {
            name: CssString::new(name),
            name_rc: Rc::from(name),
            id: RefCell::new(None),
            classes: RefCell::new(classes.iter().map(|c| CssString::new(c)).collect()),
            own_states: Cell::new(PseudoStates::empty()),
            focus_count: Cell::new(0),
            direction: Cell::new(None),
            parent: RefCell::new(Weak::new()),
            children: RefCell::new(Vec::new()),
            tree: RefCell::new(Rc::new(Cell::new(1))),
            self_generation: Cell::new(1),
        }))
    }

    /// This node's element name.
    #[must_use]
    pub fn name(&self) -> Rc<str> {
        Rc::clone(&self.0.name_rc)
    }

    /// This node's id, if one was set.
    #[must_use]
    pub fn id(&self) -> Option<CssString> {
        self.0.id.borrow().clone()
    }

    /// This node's style classes, in insertion order.
    #[must_use]
    pub fn classes(&self) -> Vec<CssString> {
        self.0.classes.borrow().clone()
    }

    /// Set or clear the id.
    pub fn set_id(&self, id: Option<&str>) {
        let next = id.map(CssString::new);
        if *self.0.id.borrow() == next {
            return;
        }
        *self.0.id.borrow_mut() = next;
        self.touch();
    }

    /// Add a style class. `false` if it was already present (nothing changed).
    pub fn add_class(&self, class: &str) -> bool {
        {
            let mut classes = self.0.classes.borrow_mut();
            if classes.iter().any(|c| c.as_str() == class) {
                return false;
            }
            classes.push(CssString::new(class));
        }
        self.touch();
        true
    }

    /// Remove a style class. `false` if it was not present (nothing changed).
    pub fn remove_class(&self, class: &str) -> bool {
        {
            let mut classes = self.0.classes.borrow_mut();
            let Some(position) = classes.iter().position(|c| c.as_str() == class) else {
                return false;
            };
            classes.remove(position);
        }
        self.touch();
        true
    }

    /// Replace the whole class list.
    pub fn set_classes(&self, classes: &[&str]) {
        let next: Vec<CssString> = classes.iter().map(|c| CssString::new(c)).collect();
        if *self.0.classes.borrow() == next {
            return;
        }
        *self.0.classes.borrow_mut() = next;
        self.touch();
    }

    /// The tree-wide generation: any mutation anywhere in this tree bumps it.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.0.tree.borrow().get()
    }

    /// The tree generation at this node's last own mutation. Comparable across
    /// nodes of one tree, and against a watermark taken from [`Node::generation`].
    #[must_use]
    pub fn self_generation(&self) -> u64 {
        self.0.self_generation.get()
    }

    /// Whether two handles refer to the same node.
    #[must_use]
    pub fn ptr_eq(&self, other: &Node) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }

    /// The node's payload address — a stable per-node identity for caches.
    pub(crate) fn addr(&self) -> usize {
        Rc::as_ptr(&self.0) as usize
    }

    /// Borrow the id without cloning it.
    pub(crate) fn borrow_id<R>(&self, f: impl FnOnce(Option<&CssString>) -> R) -> R {
        f(self.0.id.borrow().as_ref())
    }

    /// Borrow the class list without cloning it.
    pub(crate) fn borrow_classes<R>(&self, f: impl FnOnce(&[CssString]) -> R) -> R {
        f(&self.0.classes.borrow())
    }

    /// Feed every identity hash (name, id, each class) to `f`, pre-masked for
    /// `selectors`' bloom filter, whose queries mask with `BLOOM_HASH_MASK`.
    pub(crate) fn for_each_identity_hash(&self, mut f: impl FnMut(u32)) {
        f(self.0.name.precomputed_hash() & BLOOM_HASH_MASK);
        if let Some(id) = self.0.id.borrow().as_ref() {
            f(id.precomputed_hash() & BLOOM_HASH_MASK);
        }
        for class in self.0.classes.borrow().iter() {
            f(class.precomputed_hash() & BLOOM_HASH_MASK);
        }
    }

    /// Bump the tree generation and stamp it onto this node.
    fn touch(&self) {
        let generation = {
            let tree = self.0.tree.borrow();
            let next = tree.get() + 1;
            tree.set(next);
            next
        };
        self.0.self_generation.set(generation);
    }

    /// Bump only the tree generation (used when several nodes change at once).
    fn bump_tree(&self) {
        let tree = self.0.tree.borrow();
        tree.set(tree.get() + 1);
    }
}
```

> `bump_tree` is unused until Task 2; add it there if clippy's `dead_code` fires at
> this commit — the plan keeps it here because Task 2's mutation paths call it and
> the two tasks land minutes apart. If `-D warnings` rejects it now, move the
> function verbatim into Task 2's diff.

`ui/src/css/select.rs` — delete the `CssString` definition and its impls
(select.rs:32-100), the `fnv1a` function (select.rs:39-47) and the `Direction`
enum with its impl (select.rs:105-133), then add the re-export directly under the
existing `use` block:

```rust
// `CssString` and `Direction` live with the tree they describe (contract §3);
// they are re-exported here because every M1 caller reaches them through
// `css::select`, and `ui/tests/themed_button_offscreen.rs` may not be edited.
pub use crate::css::node::{CssString, Direction};
```

Remove the now-unused imports `std::borrow::Borrow` and
`precomputed_hash::PrecomputedHash` from `select.rs`; keep `std::fmt`,
`std::rc::Rc` and the rest.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib css::node::tests`
Expected: PASS — 6 tests.

Then the full gate:
Run: `cargo test -p icedtea-ui`
Expected: PASS — every M1 test still green (`select.rs`'s 9 included: they use the
re-exported `Direction`).

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
cargo fmt --all --check
cargo clippy -p icedtea-ui --all-targets -- -D warnings
git add ui/src/css/node.rs ui/src/css/mod.rs ui/src/css/select.rs
git commit -m "$(cat <<'EOF'
feat(ui/css): add the styled-node module with identity and generations

Node is an Rc handle over interior-mutable identity (name, id, classes)
with a per-tree generation counter every mutation bumps. CssString,
Direction and fnv1a move here from select.rs and are re-exported, so M1's
byte-identical gate tests keep their `css::select::` paths.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Tree structure and mutation

**Files:**
- Modify: `ui/src/css/node.rs` (add tree reads and mutations to `impl Node`; add
  the `Ancestors`/`Descendants` iterators and the free helper `adopt_tree`; extend
  the test module)

**Interfaces:**
- Consumes: Task 1's `Node`, `NodeInner`, `Node::touch`, `Node::bump_tree`,
  `Node::ptr_eq`.
- Produces:
  ```rust
  impl Node {
      pub fn parent(&self) -> Option<Node>;
      pub fn children(&self) -> Vec<Node>;
      pub fn child(&self, index: usize) -> Option<Node>;
      pub fn child_count(&self) -> usize;
      pub fn index_in_parent(&self) -> Option<usize>;
      pub fn root(&self) -> Node;
      pub fn ancestors(&self) -> impl Iterator<Item = Node>;     // parent-first, excludes self
      pub fn descendants(&self) -> impl Iterator<Item = Node>;   // pre-order, excludes self
      pub fn append_child(&self, child: &Node);
      pub fn insert_child(&self, index: usize, child: &Node);
      pub fn remove_child(&self, child: &Node) -> bool;
      pub fn detach(&self);
      pub fn is_ancestor_of(&self, other: &Node) -> bool;
  }
  ```

- [ ] **Step 1: Write the failing test**

Append to `ui/src/css/node.rs`'s `mod tests`:

```rust
    fn names(nodes: &[Node]) -> Vec<String> {
        nodes.iter().map(|n| n.name().to_string()).collect()
    }

    #[test]
    fn children_are_ordered_and_reachable_from_both_ends() {
        let box_node = Node::new("box");
        let first = Node::new("label");
        let second = Node::new("button");
        let third = Node::new("image");
        box_node.append_child(&first);
        box_node.append_child(&second);
        box_node.append_child(&third);

        assert_eq!(box_node.child_count(), 3);
        assert_eq!(names(&box_node.children()), vec!["label", "button", "image"]);
        assert!(box_node.child(1).expect("second child").ptr_eq(&second));
        assert!(box_node.child(3).is_none());
        assert_eq!(second.index_in_parent(), Some(1));
        assert!(second.parent().expect("parent").ptr_eq(&box_node));
        assert!(box_node.parent().is_none());
        assert_eq!(box_node.index_in_parent(), None);
    }

    #[test]
    fn insert_child_places_the_node_at_the_index() {
        let box_node = Node::new("box");
        let a = Node::new("a");
        let b = Node::new("b");
        let c = Node::new("c");
        box_node.append_child(&a);
        box_node.append_child(&c);
        box_node.insert_child(1, &b);
        assert_eq!(names(&box_node.children()), vec!["a", "b", "c"]);

        // An out-of-range index appends rather than panicking: the widget layer
        // must never be able to crash the engine with a stale index.
        let d = Node::new("d");
        box_node.insert_child(99, &d);
        assert_eq!(names(&box_node.children()), vec!["a", "b", "c", "d"]);
    }

    #[test]
    fn appending_an_attached_child_reparents_it() {
        let left = Node::new("box");
        let right = Node::new("box");
        let child = Node::new("button");
        left.append_child(&child);
        assert_eq!(left.child_count(), 1);

        right.append_child(&child);
        assert_eq!(left.child_count(), 0, "the old parent must lose the child");
        assert_eq!(right.child_count(), 1);
        assert!(child.parent().expect("parent").ptr_eq(&right));
    }

    #[test]
    fn remove_and_detach_report_and_orphan() {
        let box_node = Node::new("box");
        let child = Node::new("button");
        let stranger = Node::new("label");
        box_node.append_child(&child);

        assert!(!box_node.remove_child(&stranger), "removing a non-child reports false");
        assert!(box_node.remove_child(&child));
        assert!(child.parent().is_none());
        assert_eq!(box_node.child_count(), 0);

        box_node.append_child(&child);
        child.detach();
        assert!(child.parent().is_none());
        assert_eq!(box_node.child_count(), 0);
        child.detach(); // detaching an orphan is a no-op, not a panic
    }

    #[test]
    fn ancestors_and_descendants_walk_the_whole_tree() {
        let window = Node::new("window");
        let headerbar = Node::new("headerbar");
        let box_node = Node::new("box");
        let button = Node::new("button");
        let label = Node::new("label");
        window.append_child(&headerbar);
        headerbar.append_child(&box_node);
        box_node.append_child(&button);
        button.append_child(&label);

        assert_eq!(
            names(&label.ancestors().collect::<Vec<_>>()),
            vec!["button", "box", "headerbar", "window"],
            "ancestors are parent-first and exclude self"
        );
        assert_eq!(
            names(&window.descendants().collect::<Vec<_>>()),
            vec!["headerbar", "box", "button", "label"],
            "descendants are pre-order and exclude self"
        );
        assert!(label.root().ptr_eq(&window));
        assert!(window.root().ptr_eq(&window));
        assert!(window.is_ancestor_of(&label));
        assert!(!label.is_ancestor_of(&window));
        assert!(!window.is_ancestor_of(&window), "a node is not its own ancestor");
    }

    #[test]
    fn a_node_cannot_be_inserted_into_itself_or_its_own_descendant() {
        let window = Node::new("window");
        let button = Node::new("button");
        window.append_child(&button);

        button.append_child(&window); // would build an Rc cycle and hang every walk
        assert_eq!(button.child_count(), 0, "the cycle must be refused");
        assert!(window.child(0).expect("button").ptr_eq(&button));

        window.append_child(&window);
        assert_eq!(window.child_count(), 1, "a node may not become its own child");
    }

    #[test]
    fn structural_mutation_bumps_the_generation_for_every_handle() {
        let window = Node::new("window");
        let button = Node::new("button");
        let before = window.generation();
        window.append_child(&button);
        assert!(window.generation() > before);
        assert_eq!(
            button.generation(),
            window.generation(),
            "an attached node shares its tree's generation"
        );
        assert!(button.self_generation() > 0);

        let attached = window.generation();
        window.remove_child(&button);
        assert!(window.generation() > attached);
        assert!(
            button.generation() > attached,
            "a detached subtree keeps counting forward, never backwards"
        );
    }

    #[test]
    fn adopting_a_subtree_never_moves_a_generation_backwards() {
        let busy = Node::new("window");
        for index in 0..10 {
            busy.add_class(&format!("c{index}"));
        }
        let quiet = Node::new("box");
        let busy_generation = busy.generation();
        assert!(busy_generation > quiet.generation());

        quiet.append_child(&busy);
        assert!(
            busy.generation() > busy_generation,
            "the adopting tree's counter must jump past the adoptee's"
        );
        assert_eq!(quiet.generation(), busy.generation());
    }
```

**Mutation checks.**
`children_are_ordered_and_reachable_from_both_ends`: make `append_child` insert at
index 0 → the order assertion fails.
`insert_child_places_the_node_at_the_index`: drop the `index.min(len)` clamp →
the out-of-range case panics instead of appending.
`appending_an_attached_child_reparents_it`: remove the `child.detach()` at the top
of `insert_child` → the old parent still reports one child.
`remove_and_detach_report_and_orphan`: return `true` unconditionally from
`remove_child` → the stranger assertion fails.
`ancestors_and_descendants_walk_the_whole_tree`: make `descendants` push children
in forward order onto the stack → `["headerbar", …]` comes back reversed at the
first branching level (add a second child to prove it — the pre-order assertion
covers the linear case, and `is_ancestor_of` covers the rest).
`a_node_cannot_be_inserted_into_itself_or_its_own_descendant`: delete the guard →
the test hangs or overflows the stack in `is_ancestor_of`/`root`.
`structural_mutation_bumps_the_generation_for_every_handle`: skip `adopt_tree` in
`insert_child` → the shared-generation assertion fails.
`adopting_a_subtree_never_moves_a_generation_backwards`: drop the `max` in
`adopt_tree` → `busy.generation()` falls back to the quiet tree's small counter.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib css::node::tests`
Expected: FAIL to compile — `error[E0599]: no method named `append_child` found for
struct `Node`` (and the same for `parent`, `children`, `child`, `child_count`,
`index_in_parent`, `root`, `ancestors`, `descendants`, `remove_child`, `detach`,
`insert_child`, `is_ancestor_of`).

- [ ] **Step 3: Write the implementation**

Add to `impl Node` in `ui/src/css/node.rs` (after the identity methods, before
`touch`):

```rust
    /// This node's parent, if it is attached.
    #[must_use]
    pub fn parent(&self) -> Option<Node> {
        self.0.parent.borrow().upgrade().map(Node)
    }

    /// This node's children, in order.
    #[must_use]
    pub fn children(&self) -> Vec<Node> {
        self.0.children.borrow().clone()
    }

    /// The child at `index`, if any.
    #[must_use]
    pub fn child(&self, index: usize) -> Option<Node> {
        self.0.children.borrow().get(index).cloned()
    }

    /// How many children this node has.
    #[must_use]
    pub fn child_count(&self) -> usize {
        self.0.children.borrow().len()
    }

    /// This node's position among its parent's children; `None` when detached.
    #[must_use]
    pub fn index_in_parent(&self) -> Option<usize> {
        let parent = self.parent()?;
        let index = parent
            .0
            .children
            .borrow()
            .iter()
            .position(|child| child.ptr_eq(self));
        index
    }

    /// The topmost ancestor, or this node when it is detached.
    #[must_use]
    pub fn root(&self) -> Node {
        let mut current = self.clone();
        while let Some(parent) = current.parent() {
            current = parent;
        }
        current
    }

    /// This node's ancestors, parent first, excluding self.
    pub fn ancestors(&self) -> impl Iterator<Item = Node> {
        Ancestors {
            current: self.parent(),
        }
    }

    /// This node's descendants in pre-order, excluding self.
    pub fn descendants(&self) -> impl Iterator<Item = Node> {
        let mut stack: Vec<Node> = self.0.children.borrow().clone();
        stack.reverse();
        Descendants { stack }
    }

    /// Whether `other` is inside this node's subtree (strictly below it).
    #[must_use]
    pub fn is_ancestor_of(&self, other: &Node) -> bool {
        other.ancestors().any(|ancestor| ancestor.ptr_eq(self))
    }

    /// Append `child`, reparenting it if it is attached elsewhere.
    pub fn append_child(&self, child: &Node) {
        self.insert_child(self.child_count(), child);
    }

    /// Insert `child` at `index` (clamped to the end), reparenting if needed.
    ///
    /// Inserting a node into itself or into its own descendant would build an
    /// `Rc` cycle — every tree walk would then never terminate — so it is
    /// refused and logged rather than honoured.
    pub fn insert_child(&self, index: usize, child: &Node) {
        if self.ptr_eq(child) || child.is_ancestor_of(self) {
            tracing::debug!(
                parent = self.0.name.as_str(),
                child = child.0.name.as_str(),
                "refusing to insert a node into itself or into its own descendant"
            );
            return;
        }
        child.detach();

        {
            let mut children = self.0.children.borrow_mut();
            let index = index.min(children.len());
            children.insert(index, child.clone());
        }
        *child.0.parent.borrow_mut() = Rc::downgrade(&self.0);

        let token = self.0.tree.borrow().clone();
        adopt_tree(child, &token);

        let focused = child.0.focus_count.get();
        if focused > 0 {
            self.add_focus_count(focused);
        }
        self.touch();
        child.touch();
    }

    /// Remove `child`. `false` if it was not a child of this node.
    pub fn remove_child(&self, child: &Node) -> bool {
        let position = self
            .0
            .children
            .borrow()
            .iter()
            .position(|candidate| candidate.ptr_eq(child));
        let Some(position) = position else {
            return false;
        };
        self.0.children.borrow_mut().remove(position);
        *child.0.parent.borrow_mut() = Weak::new();

        let focused = child.0.focus_count.get();
        if focused > 0 {
            self.sub_focus_count(focused);
        }

        // The removed subtree becomes a tree of its own, starting past both
        // trees' counters so no handle ever sees a generation go backwards.
        let token = Rc::new(Cell::new(self.0.tree.borrow().get() + 1));
        adopt_tree(child, &token);

        self.touch();
        child.touch();
        true
    }

    /// Remove this node from its parent, if it has one.
    pub fn detach(&self) {
        if let Some(parent) = self.parent() {
            parent.remove_child(self);
        }
    }
```

Add the two iterator types and the adoption helper at module level, after
`impl Node`:

```rust
/// Iterator over a node's ancestors, parent first.
struct Ancestors {
    current: Option<Node>,
}

impl Iterator for Ancestors {
    type Item = Node;

    fn next(&mut self) -> Option<Node> {
        let node = self.current.take()?;
        self.current = node.parent();
        Some(node)
    }
}

/// Pre-order iterator over a node's descendants.
struct Descendants {
    stack: Vec<Node>,
}

impl Iterator for Descendants {
    type Item = Node;

    fn next(&mut self) -> Option<Node> {
        let node = self.stack.pop()?;
        let children = node.0.children.borrow();
        for child in children.iter().rev() {
            self.stack.push(child.clone());
        }
        drop(children);
        Some(node)
    }
}

/// Move `node`'s whole subtree onto `token`, carrying the counter forward so a
/// generation is monotonic for every handle involved.
fn adopt_tree(node: &Node, token: &Rc<Cell<u64>>) {
    let previous = node.0.tree.borrow().get();
    if token.get() < previous {
        token.set(previous);
    }
    *node.0.tree.borrow_mut() = Rc::clone(token);
    for child in node.0.children.borrow().iter() {
        adopt_tree(child, token);
    }
}
```

`add_focus_count`/`sub_focus_count` arrive in Task 3; to keep this commit
compiling, add them now in their final form (Task 3 tests them):

```rust
    /// Add `delta` focused nodes to this node and every ancestor.
    fn add_focus_count(&self, delta: u32) {
        let mut current = Some(self.clone());
        while let Some(node) = current {
            node.0
                .focus_count
                .set(node.0.focus_count.get() + delta);
            node.touch();
            current = node.parent();
        }
    }

    /// Remove `delta` focused nodes from this node and every ancestor.
    fn sub_focus_count(&self, delta: u32) {
        let mut current = Some(self.clone());
        while let Some(node) = current {
            node.0
                .focus_count
                .set(node.0.focus_count.get().saturating_sub(delta));
            node.touch();
            current = node.parent();
        }
    }
```

`bump_tree` from Task 1 is unused after all — delete it if clippy flags it.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib css::node::tests`
Expected: PASS — 14 tests.

Run: `cargo test -p icedtea-ui`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
cargo fmt --all --check
cargo clippy -p icedtea-ui --all-targets -- -D warnings
git add ui/src/css/node.rs
git commit -m "$(cat <<'EOF'
feat(ui/css): give nodes real children, parents and reparenting

Ordered children with a Weak parent, append/insert/remove/detach with
reparenting, ancestor and descendant walks, and a per-tree generation
token that a moved subtree adopts so no handle ever sees a generation go
backwards. Inserting a node into its own descendant is refused: it would
build an Rc cycle and hang every tree walk.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Pseudo-class state and derived focus

**Files:**
- Modify: `ui/src/css/node.rs` (add `states`/`set_states`/`set_state` to `impl Node`;
  extend the test module)

**Interfaces:**
- Consumes: Task 1's `PseudoStates`, `PseudoStates::DERIVED`, `NodeInner::own_states`,
  `NodeInner::focus_count`; Task 2's `add_focus_count`/`sub_focus_count` and the
  attach/detach paths that already adjust them.
- Produces:
  ```rust
  impl Node {
      pub fn states(&self) -> PseudoStates;              // own flags | derived
      pub fn set_states(&self, states: PseudoStates);    // DERIVED masked out
      pub fn set_state(&self, flag: PseudoStates, on: bool);
  }
  ```

- [ ] **Step 1: Write the failing test**

Append to `ui/src/css/node.rs`'s `mod tests`:

```rust
    #[test]
    fn own_states_round_trip_and_derived_flags_are_masked_out() {
        let node = Node::new("button");
        node.set_states(PseudoStates::HOVER | PseudoStates::CHECKED);
        assert_eq!(node.states(), PseudoStates::HOVER | PseudoStates::CHECKED);

        node.set_states(PseudoStates::FOCUS_WITHIN | PseudoStates::FOCUS_VISIBLE);
        assert_eq!(
            node.states(),
            PseudoStates::empty(),
            "derived flags may not be set directly"
        );

        node.set_state(PseudoStates::DISABLED, true);
        assert!(node.states().contains(PseudoStates::DISABLED));
        node.set_state(PseudoStates::DISABLED, false);
        assert!(!node.states().contains(PseudoStates::DISABLED));
    }

    #[test]
    fn focus_derives_focus_within_and_focus_visible_up_the_chain() {
        let window = Node::new("window");
        let box_node = Node::new("box");
        let entry = Node::new("entry");
        let sibling = Node::new("button");
        window.append_child(&box_node);
        box_node.append_child(&entry);
        window.append_child(&sibling);

        entry.set_state(PseudoStates::FOCUS, true);
        assert!(entry.states().contains(PseudoStates::FOCUS));
        for ancestor in [&entry, &box_node, &window] {
            assert!(
                ancestor.states().contains(PseudoStates::FOCUS_WITHIN),
                "focus-within must reach {:?}",
                ancestor.name()
            );
            assert!(
                ancestor.states().contains(PseudoStates::FOCUS_VISIBLE),
                "contract 3: focus-visible propagates with focus-within"
            );
        }
        assert!(
            !sibling.states().contains(PseudoStates::FOCUS_WITHIN),
            "a sibling of the focused node is not focus-within"
        );
        assert!(!window.states().contains(PseudoStates::FOCUS), "focus itself does not propagate");

        entry.set_state(PseudoStates::FOCUS, false);
        for ancestor in [&entry, &box_node, &window] {
            assert!(!ancestor.states().contains(PseudoStates::FOCUS_WITHIN));
            assert!(!ancestor.states().contains(PseudoStates::FOCUS_VISIBLE));
        }
    }

    #[test]
    fn focus_within_follows_a_subtree_across_reparenting() {
        let left = Node::new("window");
        let right = Node::new("window");
        let box_node = Node::new("box");
        let entry = Node::new("entry");
        left.append_child(&box_node);
        box_node.append_child(&entry);
        entry.set_state(PseudoStates::FOCUS, true);
        assert!(left.states().contains(PseudoStates::FOCUS_WITHIN));

        right.append_child(&box_node);
        assert!(
            !left.states().contains(PseudoStates::FOCUS_WITHIN),
            "the old ancestor chain must lose focus-within"
        );
        assert!(
            right.states().contains(PseudoStates::FOCUS_WITHIN),
            "the new ancestor chain must gain it"
        );

        box_node.detach();
        assert!(!right.states().contains(PseudoStates::FOCUS_WITHIN));
        assert!(box_node.states().contains(PseudoStates::FOCUS_WITHIN));
    }

    #[test]
    fn two_focused_descendants_do_not_cancel_each_other() {
        // Nothing forbids the widget layer from focusing two nodes in different
        // branches; a boolean would clear the ancestor flag when the first one
        // blurs, a count does not.
        let window = Node::new("window");
        let a = Node::new("entry");
        let b = Node::new("entry");
        window.append_child(&a);
        window.append_child(&b);
        a.set_state(PseudoStates::FOCUS, true);
        b.set_state(PseudoStates::FOCUS, true);
        a.set_state(PseudoStates::FOCUS, false);
        assert!(
            window.states().contains(PseudoStates::FOCUS_WITHIN),
            "b is still focused"
        );
        b.set_state(PseudoStates::FOCUS, false);
        assert!(!window.states().contains(PseudoStates::FOCUS_WITHIN));
    }

    #[test]
    fn a_state_change_bumps_the_ancestors_self_generation() {
        let window = Node::new("window");
        let entry = Node::new("entry");
        window.append_child(&entry);
        let window_self = window.self_generation();
        let tree = window.generation();

        entry.set_state(PseudoStates::FOCUS, true);
        assert!(
            window.self_generation() > window_self,
            "a restyle pass keys off self_generation to find the ancestors it must redo"
        );
        assert!(window.generation() > tree);

        let unchanged = window.generation();
        entry.set_state(PseudoStates::FOCUS, true);
        assert_eq!(unchanged, window.generation(), "setting a flag twice is not a change");
    }
```

**Mutation checks.**
`own_states_round_trip_and_derived_flags_are_masked_out`: drop the
`difference(DERIVED)` in `set_states` → the masking assertion fails.
`focus_derives_focus_within_and_focus_visible_up_the_chain`: stop walking past the
first parent in `add_focus_count` → `window` misses `FOCUS_WITHIN`; propagate
`FOCUS` itself → the "focus itself does not propagate" assertion fails.
`focus_within_follows_a_subtree_across_reparenting`: delete the `focus_count`
transfer from `insert_child`/`remove_child` → the old chain keeps the flag.
`two_focused_descendants_do_not_cancel_each_other`: replace `focus_count: Cell<u32>`
with a bool → the first blur clears the ancestor and the assertion fails.
`a_state_change_bumps_the_ancestors_self_generation`: delete `node.touch()` inside
`add_focus_count` → the ancestor's `self_generation` never moves; delete the
`previous == states` early return → the no-op assertion fails.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib css::node::tests`
Expected: FAIL to compile — `error[E0599]: no method named `set_states` found for
struct `Node`` (and the same for `states`, `set_state`).

- [ ] **Step 3: Write the implementation**

Add to `impl Node` in `ui/src/css/node.rs`, directly after `set_classes`:

```rust
    /// This node's pseudo-class state, derived flags included.
    #[must_use]
    pub fn states(&self) -> PseudoStates {
        let mut states = self.0.own_states.get();
        if self.0.focus_count.get() > 0 {
            states |= PseudoStates::DERIVED;
        }
        states
    }

    /// Replace this node's own state flags.
    ///
    /// [`PseudoStates::DERIVED`] is masked out of `states`: `:focus-within` and
    /// `:focus-visible` are maintained by the tree from `FOCUS`, never set by a
    /// caller.
    pub fn set_states(&self, states: PseudoStates) {
        let states = states.difference(PseudoStates::DERIVED);
        let previous = self.0.own_states.get();
        if previous == states {
            return;
        }
        self.0.own_states.set(states);
        match (
            previous.contains(PseudoStates::FOCUS),
            states.contains(PseudoStates::FOCUS),
        ) {
            (false, true) => self.add_focus_count(1),
            (true, false) => self.sub_focus_count(1),
            _ => {}
        }
        self.touch();
    }

    /// Turn one state flag on or off, leaving the others alone.
    pub fn set_state(&self, flag: PseudoStates, on: bool) {
        let mut states = self.0.own_states.get();
        states.set(flag.difference(PseudoStates::DERIVED), on);
        self.set_states(states);
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib css::node::tests`
Expected: PASS — 19 tests.

Run: `cargo test -p icedtea-ui`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
cargo fmt --all --check
cargo clippy -p icedtea-ui --all-targets -- -D warnings
git add ui/src/css/node.rs
git commit -m "$(cat <<'EOF'
feat(ui/css): pseudo-class state with tree-derived focus-within

set_states masks out the derived flags; FOCUS maintains a per-subtree
focused-node count so :focus-within and :focus-visible follow the ancestor
chain, survive reparenting, and do not cancel when one of two focused
descendants blurs. Every affected ancestor's self_generation is bumped so a
restyle pass can find them.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Direction inheritance

**Files:**
- Modify: `ui/src/css/node.rs` (add `set_direction`/`direction`; extend the test
  module with the direction tests and the `Direction::parse` never-panic battery)

**Interfaces:**
- Consumes: Task 1's `Direction`, `NodeInner::direction`; Task 2's `parent()`.
- Produces:
  ```rust
  impl Node {
      pub fn set_direction(&self, direction: Option<Direction>);  // None == inherit
      pub fn direction(&self) -> Direction;                       // resolved up the chain
  }
  ```

- [ ] **Step 1: Write the failing test**

Append to `ui/src/css/node.rs`'s `mod tests`:

```rust
    #[test]
    fn direction_is_inherited_until_a_node_sets_its_own() {
        let window = Node::new("window");
        let box_node = Node::new("box");
        let button = Node::new("button");
        window.append_child(&box_node);
        box_node.append_child(&button);

        assert_eq!(button.direction(), Direction::Ltr, "the default is ltr");

        window.set_direction(Some(Direction::Rtl));
        assert_eq!(button.direction(), Direction::Rtl, "inherited from the window");
        assert_eq!(box_node.direction(), Direction::Rtl);

        box_node.set_direction(Some(Direction::Ltr));
        assert_eq!(button.direction(), Direction::Ltr, "the nearest setter wins");
        assert_eq!(window.direction(), Direction::Rtl);

        box_node.set_direction(None);
        assert_eq!(button.direction(), Direction::Rtl, "None goes back to inheriting");
    }

    #[test]
    fn setting_a_direction_bumps_the_generation_only_when_it_changes() {
        let node = Node::new("window");
        let before = node.generation();
        node.set_direction(Some(Direction::Rtl));
        assert!(node.generation() > before);

        let unchanged = node.generation();
        node.set_direction(Some(Direction::Rtl));
        assert_eq!(node.generation(), unchanged, "re-setting the same direction is not a change");
    }

    #[test]
    fn direction_parse_never_panics_on_odd_input() {
        // `:dir()`'s argument reaches this parser straight from a theme file, so
        // it must survive anything a stylesheet can contain.
        for text in [
            "",
            " ",
            "l",
            "ltr ",
            " rtl",
            "LTR",
            "rtl;",
            "ltr(",
            "\u{0}",
            "\u{feff}ltr",
            "ltr\nrtl",
            "🙂",
            "auto",
            "inherit",
            "ltrltrltrltrltrltrltrltrltrltrltrltrltrltrltrltrltrltrltrltr",
            "-1",
            "0",
            "\\",
            "\"rtl\"",
        ] {
            let parsed = Direction::parse(text);
            assert!(
                parsed.is_none() || matches!(parsed, Some(Direction::Ltr) | Some(Direction::Rtl)),
                "`{text}` produced something impossible"
            );
        }
        assert_eq!(Direction::parse("LtR"), Some(Direction::Ltr));
        assert_eq!(Direction::parse("ltr "), None, "whole-value only, no trimming");
    }
```

**Mutation checks.**
`direction_is_inherited_until_a_node_sets_its_own`: return `Direction::default()`
instead of walking the parents → the inherited-from-the-window assertion fails;
treat `None` as `Ltr` → the "goes back to inheriting" assertion fails.
`setting_a_direction_bumps_the_generation_only_when_it_changes`: drop the equality
early return → the no-op assertion fails.
`direction_parse_never_panics_on_odd_input`: swap `eq_ignore_ascii_case` for a
byte-slice index → `""` panics.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib css::node::tests`
Expected: FAIL to compile — `error[E0599]: no method named `set_direction` found for
struct `Node`` (and `direction`).

- [ ] **Step 3: Write the implementation**

Add to `impl Node` in `ui/src/css/node.rs`, after `set_state`:

```rust
    /// Set this node's writing direction, or `None` to inherit the parent's.
    pub fn set_direction(&self, direction: Option<Direction>) {
        if self.0.direction.get() == direction {
            return;
        }
        self.0.direction.set(direction);
        self.touch();
    }

    /// The direction `:dir()` matches against: this node's own, else the nearest
    /// ancestor that sets one, else [`Direction::Ltr`].
    #[must_use]
    pub fn direction(&self) -> Direction {
        if let Some(direction) = self.0.direction.get() {
            return direction;
        }
        for ancestor in self.ancestors() {
            if let Some(direction) = ancestor.0.direction.get() {
                return direction;
            }
        }
        Direction::default()
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib css::node::tests`
Expected: PASS — 22 tests.

Run: `cargo test -p icedtea-ui`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
cargo fmt --all --check
cargo clippy -p icedtea-ui --all-targets -- -D warnings
git add ui/src/css/node.rs
git commit -m "$(cat <<'EOF'
feat(ui/css): inherit writing direction down the node tree

set_direction(None) inherits, a set direction wins for the whole subtree,
and Direction::parse keeps its whole-value, never-panic contract under a
fuzz-ish battery of stylesheet-shaped arguments.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: The complete GTK pseudo-class set

**Files:**
- Modify: `ui/src/css/select.rs` — `GtkPseudoClass` enum (~select.rs:135, after the
  `CssString`/`Direction` removal in Task 1 it sits nearer the top), its `ToCss`
  impl, its `NonTSPseudoClass` impl, `match_pseudo_class_name`, and
  `impl Element for CssNode`'s `match_non_ts_pseudo_class` arm

**Interfaces:**
- Consumes: nothing new.
- Produces:
  ```rust
  pub enum GtkPseudoClass {
      Hover, Active, Checked, Indeterminate, Disabled,
      Focus, FocusVisible, FocusWithin,
      Backdrop, Selected, Link, Visited,
      Dir(Direction), Drop(CssString),
      Other(CssString), OtherFunctional(CssString, CssString),
  }
  ```

- [ ] **Step 1: Write the failing test**

Append to `ui/src/css/select.rs`'s `mod tests`:

```rust
    #[test]
    fn the_gtk_pseudo_class_set_is_modelled_not_swallowed() {
        use super::GtkPseudoClass;

        // Every pseudo-class GTK 4.22 sets on a node must reach the matcher as a
        // modelled variant. Anything landing in `Other` matches nothing, which is
        // how M1 silently dropped :focus-within, :indeterminate, :link, :visited.
        for (text, expected) in [
            ("hover", GtkPseudoClass::Hover),
            ("active", GtkPseudoClass::Active),
            ("checked", GtkPseudoClass::Checked),
            ("indeterminate", GtkPseudoClass::Indeterminate),
            ("disabled", GtkPseudoClass::Disabled),
            ("focus", GtkPseudoClass::Focus),
            ("focus-visible", GtkPseudoClass::FocusVisible),
            ("focus-within", GtkPseudoClass::FocusWithin),
            ("backdrop", GtkPseudoClass::Backdrop),
            ("selected", GtkPseudoClass::Selected),
            ("link", GtkPseudoClass::Link),
            ("visited", GtkPseudoClass::Visited),
            ("FOCUS-WITHIN", GtkPseudoClass::FocusWithin),
        ] {
            assert_eq!(
                super::match_pseudo_class_name(text),
                expected,
                "`:{text}` must be modelled"
            );
        }
        assert!(matches!(
            super::match_pseudo_class_name("hover-ish"),
            GtkPseudoClass::Other(_)
        ));
    }

    #[test]
    fn every_pseudo_class_serialises_back_to_its_own_syntax() {
        use super::GtkPseudoClass;
        use cssparser::ToCss;

        let mut out = String::new();
        for (pseudo, expected) in [
            (GtkPseudoClass::FocusWithin, ":focus-within"),
            (GtkPseudoClass::Indeterminate, ":indeterminate"),
            (GtkPseudoClass::Link, ":link"),
            (GtkPseudoClass::Visited, ":visited"),
        ] {
            out.clear();
            pseudo.to_css(&mut out).expect("serialising to a String cannot fail");
            assert_eq!(out, expected);
        }
    }
```

**Mutation checks.**
`the_gtk_pseudo_class_set_is_modelled_not_swallowed`: leave `"focus-within"` out of
`match_pseudo_class_name` → it falls into `Other` and the assertion fails.
`every_pseudo_class_serialises_back_to_its_own_syntax`: emit `":focuswithin"` →
the string comparison fails.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib css::select::tests`
Expected: FAIL to compile — `error[E0599]: no variant named `Indeterminate` found for
enum `GtkPseudoClass`` (and `FocusWithin`, `Link`, `Visited`).

- [ ] **Step 3: Write the implementation**

In `ui/src/css/select.rs`, extend the enum (keep every existing variant and its
doc comment; add the four new ones):

```rust
    /// `:checked`
    Checked,
    /// `:indeterminate` — a tri-state check button's middle state.
    Indeterminate,
    /// `:disabled`
    Disabled,
    /// `:focus`
    Focus,
    /// `:focus-visible`
    FocusVisible,
    /// `:focus-within` — derived by the node tree from a descendant's focus.
    /// Not built into `selectors` (unlike `:nth-child` and friends), so it is a
    /// custom pseudo-class like `:hover`.
    FocusWithin,
    /// `:backdrop`
    Backdrop,
    /// `:selected`
    Selected,
    /// `:link`. GTK never sets it on a node, so it parses and never matches.
    Link,
    /// `:visited`. GTK never sets it on a node, so it parses and never matches.
    Visited,
```

Extend `impl ToCss for GtkPseudoClass`:

```rust
            Self::Indeterminate => dest.write_str("indeterminate"),
            Self::FocusWithin => dest.write_str("focus-within"),
            Self::Link => dest.write_str("link"),
            Self::Visited => dest.write_str("visited"),
```

Extend `impl NonTSPseudoClass for GtkPseudoClass`:

```rust
    fn is_user_action_state(&self) -> bool {
        matches!(
            self,
            Self::Hover | Self::Active | Self::Focus | Self::FocusVisible | Self::FocusWithin
        )
    }
```

Extend `match_pseudo_class_name`:

```rust
        "checked" => GtkPseudoClass::Checked,
        "indeterminate" => GtkPseudoClass::Indeterminate,
        "disabled" => GtkPseudoClass::Disabled,
        "focus" => GtkPseudoClass::Focus,
        "focus-visible" => GtkPseudoClass::FocusVisible,
        "focus-within" => GtkPseudoClass::FocusWithin,
        "backdrop" => GtkPseudoClass::Backdrop,
        "selected" => GtkPseudoClass::Selected,
        "link" => GtkPseudoClass::Link,
        "visited" => GtkPseudoClass::Visited,
```

Extend M1's `impl Element for CssNode`'s `match_non_ts_pseudo_class` so it stays
exhaustive. `CssNode`'s 7-bool state models none of the new flags, and Part 3
deletes the type outright:

```rust
            GtkPseudoClass::Dir(direction) => self.0.direction == *direction,
            // M1's `CssNode` has no bit for these; `Node` matches them properly.
            // This impl disappears with `CssNode` in Part 3.
            GtkPseudoClass::Indeterminate
            | GtkPseudoClass::FocusWithin
            | GtkPseudoClass::Link
            | GtkPseudoClass::Visited
            | GtkPseudoClass::Drop(_)
            | GtkPseudoClass::Other(_)
            | GtkPseudoClass::OtherFunctional(..) => false,
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib css::select::tests`
Expected: PASS — 11 tests (M1's 9 plus the two new ones). M1's
`unknown_pseudo_classes_parse_but_never_match` still passes: it asserts
`:indeterminate` *parses* and does not match a `CssNode`, both still true.

Run: `cargo test -p icedtea-ui`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
cargo fmt --all --check
cargo clippy -p icedtea-ui --all-targets -- -D warnings
git add ui/src/css/select.rs
git commit -m "$(cat <<'EOF'
feat(ui/css): model :focus-within, :indeterminate, :link and :visited

These four reached M1's matcher as GtkPseudoClass::Other, which never
matches -- Adwaita uses :focus-within and :indeterminate. They are now
first-class variants that serialise back to their own syntax.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: `Element for Node`, `MatchCx`, and matching

**Files:**
- Modify: `ui/src/css/select.rs` — add `MatchCx`, `impl Element for Node`, replace
  `pub fn matches` (select.rs:~587) with the contract's two entry points, and
  rewrite the 9 M1 tests onto `Node`

**Interfaces:**
- Consumes: `css::node::{Node, PseudoStates as NodeStates, Direction, CssString}`;
  Task 5's `GtkPseudoClass`; `selectors::{Element, OpaqueElement, SelectorList}`,
  `selectors::matching::matches_selector`, `selectors::parser::{AncestorHashes, Selector}`,
  `selectors::context::{MatchingForInvalidation, MatchingMode, NeedsSelectorFlags,
  QuirksMode, SelectorCaches}`, `selectors::bloom::BloomFilter`.
- Produces:
  ```rust
  pub struct MatchCx { /* private */ }
  impl MatchCx {
      pub fn new() -> Self;
      pub fn reset(&mut self);
      pub fn seed_for(&mut self, node: &Node);
      // push_ancestor / pop_ancestor arrive in Task 7
  }
  impl Default for MatchCx { fn default() -> Self; }
  impl selectors::Element for Node { type Impl = GtkSelectorImpl; /* complete */ }
  pub fn matches(list: &SelectorList<GtkSelectorImpl>, node: &Node, cx: &mut MatchCx) -> bool;
  pub fn matches_with_specificity(list: &SelectorList<GtkSelectorImpl>, node: &Node,
                                  cx: &mut MatchCx) -> Option<u32>;
  ```
  `matches_with_specificity` returns the **highest** specificity among the
  selectors in `list` that match — the per-selector rule M1's `cascade` already
  applies (`cascade.rs:180-186`), so Part 3 can drop its own `filter(...).max()`.

- [ ] **Step 1: Write the failing test**

Replace `ui/src/css/select.rs`'s `mod tests` **helpers and all 9 M1 tests** with the
`Node` versions below (every M1 assertion is preserved verbatim; only the fixture
construction changes), and append the new tree-structural tests. Keep Task 5's two
tests as they are.

```rust
#[cfg(test)]
mod tests {
    use super::{MatchCx, matches, matches_with_specificity, parse_selector_list};
    use crate::css::node::{Direction, Node, PseudoStates};

    fn window_button(classes: &[&str], states: PseudoStates) -> Node {
        let window = Node::with_classes("window", &["background"]);
        let button = Node::with_classes("button", classes);
        window.append_child(&button);
        button.set_states(states);
        button
    }

    fn hits(selector: &str, node: &Node) -> bool {
        let list =
            parse_selector_list(selector).unwrap_or_else(|| panic!("`{selector}` failed to parse"));
        let mut cx = MatchCx::new();
        matches(&list, node, &mut cx)
    }

    #[test]
    fn element_name_and_child_combinator() {
        let node = window_button(&[], PseudoStates::default());
        assert!(hits("button", &node));
        assert!(hits("window > button", &node));
        assert!(hits("window button", &node));
        assert!(!hits("headerbar > button", &node));
        assert!(!hits("entry", &node));
    }

    #[test]
    fn pseudo_class_state_gates_matching() {
        let plain = window_button(&[], PseudoStates::default());
        let hovered = window_button(&[], PseudoStates::HOVER);
        assert!(!hits("button:hover", &plain));
        assert!(hits("button:hover", &hovered));
        assert!(!hits("button:active", &hovered));
        assert!(
            hits("button", &hovered),
            "state must not break the base match"
        );
    }

    #[test]
    fn style_classes_match() {
        let suggested = window_button(&["suggested-action"], PseudoStates::default());
        assert!(hits(".suggested-action", &suggested));
        assert!(hits("button.suggested-action", &suggested));
        assert!(!hits("button.destructive-action", &suggested));
        assert!(hits(
            "window.background > button.suggested-action",
            &suggested
        ));
    }

    #[test]
    fn unknown_pseudo_classes_parse_but_never_match() {
        // GTK carries pseudo-classes this engine does not model. They must not
        // make the whole rule unparseable (which would silently drop hundreds of
        // Adwaita rules) and must not match either.
        let node = window_button(&[], PseudoStates::default());
        assert!(hits("button", &node));
        assert!(!hits("button:placeholder-shown", &node));
        assert!(parse_selector_list("button:placeholder-shown").is_some());
    }

    #[test]
    fn specificity_follows_css_rules() {
        let one = parse_selector_list("button").unwrap();
        let two = parse_selector_list("button:hover").unwrap();
        let three = parse_selector_list("window > button.suggested-action:hover").unwrap();
        let spec = |l: &selectors::SelectorList<super::GtkSelectorImpl>| l.slice()[0].specificity();
        assert!(spec(&one) < spec(&two));
        assert!(spec(&two) < spec(&three));
    }

    #[test]
    fn adwaitas_real_button_preludes_parse() {
        for prelude in [
            "notebook > header > tabs > arrow, button",
            "notebook > header > tabs > arrow:hover, button:hover",
            "button.suggested-action",
            "button.suggested-action:active, button.suggested-action:checked",
            "columnview.view > header > button, treeview.view > header > button",
        ] {
            assert!(
                parse_selector_list(prelude).is_some(),
                "Adwaita prelude failed to parse: {prelude}"
            );
        }
        let base = parse_selector_list("notebook > header > tabs > arrow, button").unwrap();
        assert_eq!(base.slice().len(), 2);
        let node = window_button(&[], PseudoStates::default());
        let mut cx = MatchCx::new();
        assert!(matches(&base, &node, &mut cx));
    }

    #[test]
    fn dir_matches_the_nodes_direction() {
        let ltr = window_button(&[], PseudoStates::default());
        let rtl = window_button(&[], PseudoStates::default());
        rtl.set_direction(Some(Direction::Rtl));
        assert!(hits("button:dir(ltr)", &ltr));
        assert!(!hits("button:dir(rtl)", &ltr));
        assert!(hits("button:dir(rtl)", &rtl));
        assert!(!hits("button:dir(ltr)", &rtl));
        assert_eq!(ltr.direction(), Direction::Ltr, "default is ltr");
        // A bogus argument parses (so the rule survives) but never matches.
        assert!(!hits("button:dir(sideways)", &ltr));
    }

    #[test]
    fn drop_never_matches_but_keeps_the_rest_of_the_comma_list() {
        // E6: `colorswatch:drop(active), colorswatch` used to fail to parse
        // as a whole, taking the valid `colorswatch` selector with it.
        let node = Node::new("colorswatch");
        let list = parse_selector_list("colorswatch:drop(active), colorswatch")
            .expect("the comma list must parse");
        assert_eq!(list.slice().len(), 2);
        let mut cx = MatchCx::new();
        assert!(matches(&list, &node, &mut cx));
        assert!(!hits("colorswatch:drop(active)", &node));

        // New in M2: the node tree can actually carry the drop state.
        node.set_state(PseudoStates::DROP_ACTIVE, true);
        assert!(hits("colorswatch:drop(active)", &node));
        assert!(!hits("colorswatch:drop(highlight)", &node), "only `active` is modelled");
    }

    #[test]
    fn unknown_functional_pseudo_classes_parse_but_never_match() {
        let node = window_button(&[], PseudoStates::default());
        assert!(parse_selector_list("button:lang(en), button").is_some());
        assert!(!hits("button:lang(en)", &node));
        assert!(hits("button", &node));
    }

    // ---- new in M2: the tree-structural constructs M1 could not express ----

    fn linked_box(count: usize) -> Node {
        let box_node = Node::with_classes("box", &["linked"]);
        for _ in 0..count {
            box_node.append_child(&Node::new("button"));
        }
        box_node
    }

    #[test]
    fn first_last_and_only_child_read_the_real_sibling_list() {
        let three = linked_box(3);
        let first = three.child(0).expect("first");
        let middle = three.child(1).expect("middle");
        let last = three.child(2).expect("last");

        assert!(hits("button:first-child", &first));
        assert!(!hits("button:first-child", &middle));
        assert!(!hits("button:first-child", &last));
        assert!(hits("button:last-child", &last));
        assert!(!hits("button:last-child", &middle));
        assert!(hits("button:not(:first-child)", &middle));
        assert!(!hits("button:only-child", &first));

        let alone = linked_box(1);
        let only = alone.child(0).expect("only");
        assert!(hits("button:only-child", &only));
        assert!(hits("button:first-child", &only));
        assert!(hits("button:last-child", &only));
        assert!(!hits("button:not(:first-child)", &only));
    }

    #[test]
    fn nth_child_counts_from_both_ends() {
        let five = linked_box(5);
        let nodes: Vec<Node> = five.children();
        assert!(hits("button:nth-child(2)", &nodes[1]));
        assert!(!hits("button:nth-child(2)", &nodes[2]));
        assert!(hits("button:nth-child(odd)", &nodes[0]));
        assert!(hits("button:nth-child(odd)", &nodes[2]));
        assert!(hits("button:nth-child(2n)", &nodes[1]));
        assert!(hits("button:nth-child(2n + 1)", &nodes[4]));
        assert!(hits("button:nth-last-child(1)", &nodes[4]));
        assert!(hits("button:nth-last-child(2)", &nodes[3]));
        assert!(!hits("button:nth-last-child(2)", &nodes[4]));
    }

    #[test]
    fn adjacent_and_general_sibling_combinators_match() {
        let box_node = Node::new("box");
        let entry = Node::new("entry");
        let button = Node::new("button");
        let label = Node::new("label");
        box_node.append_child(&entry);
        box_node.append_child(&button);
        box_node.append_child(&label);

        assert!(hits("entry + button", &button));
        assert!(!hits("entry + label", &label), "+ is strictly adjacent");
        assert!(hits("entry ~ label", &label));
        assert!(hits("entry ~ button", &button));
        assert!(!hits("label ~ entry", &entry), "~ looks backwards only");
    }

    #[test]
    fn root_empty_and_id_selectors_match_the_tree() {
        let window = Node::new("window");
        let swatch = Node::new("colorswatch");
        swatch.set_id(Some("add-color-button"));
        window.append_child(&swatch);

        assert!(hits(":root", &window));
        assert!(!hits(":root", &swatch));
        assert!(hits("colorswatch#add-color-button", &swatch));
        assert!(!hits("colorswatch#other", &swatch));
        assert!(hits("#add-color-button:only-child", &swatch));
        assert!(hits("colorswatch:empty", &swatch), "no children == :empty");
        assert!(!hits("window:empty", &window));
    }

    #[test]
    fn focus_within_matches_the_ancestors_of_the_focused_node() {
        let window = Node::new("window");
        let box_node = Node::new("box");
        let entry = Node::new("entry");
        window.append_child(&box_node);
        box_node.append_child(&entry);

        assert!(!hits("window:focus-within", &window));
        entry.set_state(PseudoStates::FOCUS, true);
        assert!(hits("window:focus-within", &window));
        assert!(hits("box:focus-within", &box_node));
        assert!(hits("entry:focus", &entry));
        assert!(!hits("window:focus", &window), ":focus itself does not propagate");
    }

    #[test]
    fn matching_follows_the_tree_after_it_is_restructured() {
        // The nth-index cache inside `MatchCx` is per pass, not per tree: a
        // context reused across a mutation must not answer from stale counts.
        let box_node = linked_box(2);
        let first = box_node.child(0).expect("first");
        let second = box_node.child(1).expect("second");
        let list = parse_selector_list("button:last-child").expect("parses");
        let mut cx = MatchCx::new();

        assert!(!matches(&list, &first, &mut cx));
        assert!(matches(&list, &second, &mut cx));

        box_node.remove_child(&second);
        assert!(
            matches(&list, &first, &mut cx),
            "the surviving child is now the last one"
        );
    }

    #[test]
    fn matches_with_specificity_returns_the_best_matching_selector() {
        let node = window_button(&["suggested-action"], PseudoStates::HOVER);
        let mut cx = MatchCx::new();

        let list = parse_selector_list("button, window > button.suggested-action:hover")
            .expect("parses");
        let best = matches_with_specificity(&list, &node, &mut cx).expect("the list matches");
        let plain = parse_selector_list("button").expect("parses");
        let plain_specificity =
            matches_with_specificity(&plain, &node, &mut cx).expect("`button` matches");
        assert!(
            best > plain_specificity,
            "the most specific matching selector wins, not the first"
        );

        let miss = parse_selector_list("entry, headerbar").expect("parses");
        assert_eq!(matches_with_specificity(&miss, &node, &mut cx), None);
    }

    #[test]
    fn nth_child_of_a_selector_list_is_not_enabled() {
        // GTK CSS has no `:nth-child(An+B of S)`; `GtkSelectorParser` keeps
        // `parse_nth_child_of`'s `false` default, so the form is rejected.
        assert!(parse_selector_list("button:nth-child(2n of .flat)").is_none());
        assert!(parse_selector_list("button:nth-child(2n)").is_some());
    }

    #[test]
    fn selector_parsing_never_panics_on_odd_input() {
        // Theme files are third-party text; a panic here takes the compositor
        // down. Every one of these must return Some or None, never unwind.
        for text in [
            "",
            " ",
            ",",
            "button,",
            ",button",
            "button >",
            "> button",
            "button ~~ label",
            "button::",
            "button::selection::selection",
            "button:",
            "button:()",
            "button:dir()",
            "button:dir(",
            "button:drop()",
            "button:nth-child()",
            "button:nth-child(+)",
            "button:nth-child(2n+)",
            "button:not()",
            "button:not(:not(:not(button)))",
            "#",
            ".",
            "#.",
            "[",
            "[attr=]",
            "button[",
            "\u{0}",
            "🙂",
            "button/*unterminated",
            "button{",
            "*|*",
            "|button",
            "button:is(,)",
            "button:where()",
            &"button ".repeat(200),
            &":not(".repeat(50),
        ] {
            let _ = parse_selector_list(text);
        }
        assert!(parse_selector_list("button").is_some(), "the parser still works");
    }
}
```

**Mutation checks.**
`first_last_and_only_child_read_the_real_sibling_list`: return `None` from
`prev_sibling_element` (M1's stub) → `:first-child` matches every node and
`:not(:first-child)` matches none, failing three assertions.
`nth_child_counts_from_both_ends`: return `None` from `next_sibling_element` →
every `:nth-last-child` assertion fails.
`adjacent_and_general_sibling_combinators_match`: make `prev_sibling_element`
return the parent's child at `index + 1` → `entry + button` fails.
`root_empty_and_id_selectors_match_the_tree`: return `false` from `has_id` (M1's
stub) → the `#add-color-button` assertions fail; return `true` from `is_empty`
(M1's stub) → `window:empty` matches.
`focus_within_matches_the_ancestors_of_the_focused_node`: map `FocusWithin` to
`false` in `match_non_ts_pseudo_class` → two assertions fail.
`matching_follows_the_tree_after_it_is_restructured`: delete the
generation-changed cache reset in `seed_for` → the post-removal assertion fails
from a stale nth-index count.
`matches_with_specificity_returns_the_best_matching_selector`: return the first
matching selector's specificity instead of the max → the `best > plain` assertion
fails.
`selector_parsing_never_panics_on_odd_input`: any `unwrap`/slice-index added to
`parse_selector_list`'s path → the battery panics.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib css::select::tests`
Expected: FAIL to compile — `error[E0432]: unresolved import `super::MatchCx``,
`error[E0432]: unresolved import `super::matches_with_specificity``, and
`error[E0277]: the trait bound `Node: selectors::Element` is not satisfied` at the
`matches` call sites.

- [ ] **Step 3: Write the implementation**

In `ui/src/css/select.rs`, update the imports at the top of the file:

```rust
use std::fmt;
use std::rc::Rc;

use cssparser::{CowRcStr, Parser as CssParser, ParserInput, SourceLocation, ToCss};
use selectors::attr::{AttrSelectorOperation, CaseSensitivity, NamespaceConstraint};
use selectors::bloom::BloomFilter;
use selectors::context::{
    MatchingForInvalidation, MatchingMode, NeedsSelectorFlags, QuirksMode, SelectorCaches,
};
use selectors::matching::{ElementSelectorFlags, MatchingContext, matches_selector};
use selectors::parser::{
    AncestorHashes, NonTSPseudoClass, ParseRelative, PseudoElement as PseudoElementTrait,
    Selector, SelectorParseErrorKind,
};
use selectors::{Element, OpaqueElement, SelectorImpl, SelectorList};

use crate::css::node::Node;
// M1's `CssNode` still uses `select::PseudoStates`, the 7-bool struct below.
// Part 3 deletes both and this alias goes with them.
use crate::css::node::PseudoStates as NodeStates;
```

`matches_selector_list` is no longer used — drop it from the import list.
`std::rc::Rc` stays (M1's `CssNode` holds one).

Replace M1's `pub fn matches` (select.rs:~587) with the matching layer, and add
`MatchCx` and the `Element` impl. Put `MatchCx` directly above them:

```rust
/// The matching state a whole restyle pass shares.
///
/// It carries three things `selectors` wants kept alive across calls:
/// - `SelectorCaches`, whose `nth_index` memoises `:nth-child()` counts so a wide
///   sibling list costs O(n) rather than O(n²) across a pass;
/// - a `BloomFilter` holding the identity hashes of the *ancestors* of the node
///   being matched, which fast-rejects selectors whose ancestor hashes cannot be
///   present;
/// - the tree generation and filter position both were built for, so a stale
///   cache can never answer for a mutated tree.
///
/// [`matches`] and [`matches_with_specificity`] call [`MatchCx::seed_for`]
/// themselves; a caller walking a tree depth-first can instead drive the filter
/// with [`MatchCx::push_ancestor`]/[`MatchCx::pop_ancestor`] and pay for each
/// ancestor once.
pub struct MatchCx {
    caches: SelectorCaches,
    bloom: BloomFilter,
    /// `(addr of the deepest ancestor in `bloom`, tree generation)`. `0` is the
    /// address of "no ancestors", i.e. the filter is empty and that is correct.
    filter_top: Option<(usize, u64)>,
    /// The tree generation `caches` was built against.
    generation: Option<u64>,
    /// How many ancestors are currently inserted.
    depth: usize,
}

impl Default for MatchCx {
    fn default() -> Self {
        Self::new()
    }
}

impl MatchCx {
    /// An empty context, positioned for nothing.
    #[must_use]
    pub fn new() -> Self {
        Self {
            caches: SelectorCaches::default(),
            bloom: BloomFilter::new(),
            filter_top: None,
            generation: None,
            depth: 0,
        }
    }

    /// Forget every cache and empty the filter.
    pub fn reset(&mut self) {
        self.caches = SelectorCaches::default();
        self.bloom.clear();
        self.filter_top = None;
        self.generation = None;
        self.depth = 0;
    }

    /// Position the filter for `node` by walking its ancestors root-first.
    ///
    /// A no-op when the filter already holds exactly `node`'s ancestors at
    /// `node`'s current tree generation, which is the common case when a whole
    /// stylesheet is matched against one node.
    pub fn seed_for(&mut self, node: &Node) {
        let generation = node.generation();
        if self.generation != Some(generation) {
            // The tree changed: every memoised nth-index count is now suspect.
            self.caches = SelectorCaches::default();
            self.generation = Some(generation);
            self.filter_top = None;
        }
        let wanted = node.parent().map_or(0, |parent| parent.addr());
        if self.filter_top == Some((wanted, generation)) {
            return;
        }

        self.bloom.clear();
        self.depth = 0;
        let mut chain: Vec<Node> = node.ancestors().collect();
        chain.reverse();
        for ancestor in &chain {
            self.push_ancestor(ancestor);
        }
        self.filter_top = Some((wanted, generation));
    }
}
```

`push_ancestor`/`pop_ancestor` land in Task 7; to keep this commit compiling and
this task's tests honest, add them now in their final form — Task 7 adds the
`Element::add_element_unique_hashes` implementation they mirror and the tests that
pin their behaviour:

```rust
impl MatchCx {
    /// Insert `node`'s identity hashes, making the filter correct for `node`'s
    /// children.
    pub fn push_ancestor(&mut self, node: &Node) {
        node.for_each_identity_hash(|hash| self.bloom.insert_hash(hash));
        self.depth += 1;
        self.filter_top = Some((node.addr(), node.generation()));
    }

    /// Undo one [`MatchCx::push_ancestor`].
    pub fn pop_ancestor(&mut self, node: &Node) {
        node.for_each_identity_hash(|hash| self.bloom.remove_hash(hash));
        self.depth = self.depth.saturating_sub(1);
        self.filter_top = node
            .parent()
            .map(|parent| (parent.addr(), parent.generation()));
    }
}
```

The `Element` implementation, placed after M1's `impl Element for CssNode`:

```rust
/// The complete `selectors::Element` implementation over the node tree.
///
/// Everything `selectors` can ask about a GTK node is answered for real here:
/// siblings (which drive `+`, `~` and the whole `:nth-*` family), the first
/// child, emptiness, root-ness, ids, classes, the pseudo-class state, and the
/// identity hashes the ancestor bloom filter is built from.
///
/// The methods below are constant because GTK's node model genuinely has no
/// such concept — they are not stubs:
/// * `attr_matches`, `has_attr_in_no_namespace` — GTK nodes have no attributes.
/// * `has_custom_state` — no `:state()` custom states.
/// * `imported_part`, `is_part` — no shadow parts.
/// * `is_html_slot_element`, `assigned_slot`, `parent_node_is_shadow_root`,
///   `containing_shadow_host` — no shadow DOM.
/// * `is_html_element_in_html_document` — not HTML.
/// * `is_link` — `:link`/`:visited` are matched from `PseudoStates`, which GTK
///   never sets, so link-ness is meaningless here.
/// * `is_pseudo_element`, `match_pseudo_element`,
///   `pseudo_element_originating_element` — no pseudo-element boxes (M3+).
/// * `apply_selector_flags` — M2 restyles whole dirty subtrees, so there is no
///   invalidation bookkeeping to record.
impl Element for Node {
    type Impl = GtkSelectorImpl;

    fn opaque(&self) -> OpaqueElement {
        // The payload address: stable for the node's lifetime and unique per
        // node. `OpaqueElement::new` takes `&T`, so the borrow must be of the
        // payload itself -- passing a raw pointer would take the address of a
        // temporary instead.
        OpaqueElement::new(&*self.opaque_payload())
    }

    fn parent_element(&self) -> Option<Self> {
        self.parent()
    }

    fn parent_node_is_shadow_root(&self) -> bool {
        false
    }

    fn containing_shadow_host(&self) -> Option<Self> {
        None
    }

    fn pseudo_element_originating_element(&self) -> Option<Self> {
        None
    }

    fn is_pseudo_element(&self) -> bool {
        false
    }

    fn prev_sibling_element(&self) -> Option<Self> {
        let parent = self.parent()?;
        let index = self.index_in_parent()?;
        if index == 0 {
            return None;
        }
        parent.child(index - 1)
    }

    fn next_sibling_element(&self) -> Option<Self> {
        let parent = self.parent()?;
        let index = self.index_in_parent()?;
        parent.child(index + 1)
    }

    fn first_element_child(&self) -> Option<Self> {
        self.child(0)
    }

    fn is_html_element_in_html_document(&self) -> bool {
        false
    }

    fn has_local_name(&self, local_name: &str) -> bool {
        &*self.name() == local_name
    }

    fn has_namespace(&self, ns: &str) -> bool {
        ns.is_empty()
    }

    fn is_same_type(&self, other: &Self) -> bool {
        self.name() == other.name()
    }

    fn attr_matches(
        &self,
        _ns: &NamespaceConstraint<&CssString>,
        _local_name: &CssString,
        _operation: &AttrSelectorOperation<&CssString>,
    ) -> bool {
        false
    }

    fn match_non_ts_pseudo_class(
        &self,
        pc: &GtkPseudoClass,
        _context: &mut MatchingContext<GtkSelectorImpl>,
    ) -> bool {
        let states = self.states();
        match pc {
            GtkPseudoClass::Hover => states.contains(NodeStates::HOVER),
            GtkPseudoClass::Active => states.contains(NodeStates::ACTIVE),
            GtkPseudoClass::Checked => states.contains(NodeStates::CHECKED),
            GtkPseudoClass::Indeterminate => states.contains(NodeStates::INDETERMINATE),
            GtkPseudoClass::Disabled => states.contains(NodeStates::DISABLED),
            GtkPseudoClass::Focus => states.contains(NodeStates::FOCUS),
            GtkPseudoClass::FocusVisible => states.contains(NodeStates::FOCUS_VISIBLE),
            GtkPseudoClass::FocusWithin => states.contains(NodeStates::FOCUS_WITHIN),
            GtkPseudoClass::Backdrop => states.contains(NodeStates::BACKDROP),
            GtkPseudoClass::Selected => states.contains(NodeStates::SELECTED),
            // GTK never sets these; the bits exist so the grammar is complete.
            GtkPseudoClass::Link => states.contains(NodeStates::LINK),
            GtkPseudoClass::Visited => states.contains(NodeStates::VISITED),
            GtkPseudoClass::Dir(direction) => self.direction() == *direction,
            GtkPseudoClass::Drop(argument) => {
                argument.as_str().eq_ignore_ascii_case("active")
                    && states.contains(NodeStates::DROP_ACTIVE)
            }
            GtkPseudoClass::Other(_) | GtkPseudoClass::OtherFunctional(..) => false,
        }
    }

    fn match_pseudo_element(
        &self,
        _pe: &GtkPseudoElement,
        _context: &mut MatchingContext<GtkSelectorImpl>,
    ) -> bool {
        false
    }

    fn apply_selector_flags(&self, _flags: ElementSelectorFlags) {}

    fn is_link(&self) -> bool {
        false
    }

    fn is_html_slot_element(&self) -> bool {
        false
    }

    fn assigned_slot(&self) -> Option<Self> {
        None
    }

    fn has_id(&self, id: &CssString, case_sensitivity: CaseSensitivity) -> bool {
        self.borrow_id(|own| {
            own.is_some_and(|own| {
                case_sensitivity.eq(own.as_str().as_bytes(), id.as_str().as_bytes())
            })
        })
    }

    fn has_class(&self, name: &CssString, case_sensitivity: CaseSensitivity) -> bool {
        self.borrow_classes(|classes| {
            classes
                .iter()
                .any(|class| case_sensitivity.eq(class.as_str().as_bytes(), name.as_str().as_bytes()))
        })
    }

    fn has_custom_state(&self, _name: &CssString) -> bool {
        false
    }

    fn imported_part(&self, _name: &CssString) -> Option<CssString> {
        None
    }

    fn is_part(&self, _name: &CssString) -> bool {
        false
    }

    fn is_empty(&self) -> bool {
        self.child_count() == 0
    }

    fn is_root(&self) -> bool {
        self.parent().is_none()
    }

    fn add_element_unique_hashes(&self, _filter: &mut BloomFilter) -> bool {
        // Task 7 fills this in; a `false` here makes the filter a permanent
        // no-op, which is only correct while nothing seeds it.
        false
    }
}
```

`opaque_payload` keeps `NodeInner` private to `node.rs`; add it there next to
`addr` (Task 1's block):

```rust
    /// The payload behind this handle, for `selectors`' identity comparisons.
    pub(crate) fn opaque_payload(&self) -> Rc<NodeInner> {
        Rc::clone(&self.0)
    }
```

That returns an owned `Rc`, so `OpaqueElement::new(&*self.opaque_payload())` would
borrow a temporary. Use this shape in `opaque()` instead — it keeps the `Rc` alive
for the whole expression and takes the payload address:

```rust
    fn opaque(&self) -> OpaqueElement {
        let payload = self.opaque_payload();
        OpaqueElement::new(&*payload)
    }
```

The address is the `NodeInner` allocation's, identical for every handle to the
node and stable while any handle lives — which is the whole of `OpaqueElement`'s
contract.

Finally the two entry points, replacing M1's `matches`:

```rust
/// Parse a comma-separated selector list. `None` if the whole list is invalid.
#[must_use]
pub fn parse_selector_list(text: &str) -> Option<SelectorList<GtkSelectorImpl>> {
    let mut input = ParserInput::new(text);
    let mut parser = CssParser::new(&mut input);
    SelectorList::parse(&GtkSelectorParser, &mut parser, ParseRelative::No).ok()
}

/// Whether any selector in `list` matches `node`.
#[must_use]
pub fn matches(list: &SelectorList<GtkSelectorImpl>, node: &Node, cx: &mut MatchCx) -> bool {
    matches_with_specificity(list, node, cx).is_some()
}

/// The highest specificity among the selectors in `list` that match `node`, or
/// `None` when none of them do.
///
/// Specificity is per *selector*, not per rule: a prelude like
/// `notebook > header > tabs > arrow, button` contributes the specificity of
/// whichever of its selectors actually matched.
#[must_use]
pub fn matches_with_specificity(
    list: &SelectorList<GtkSelectorImpl>,
    node: &Node,
    cx: &mut MatchCx,
) -> Option<u32> {
    cx.seed_for(node);
    let MatchCx {
        caches,
        bloom,
        ..
    } = cx;
    let mut context = MatchingContext::new(
        MatchingMode::Normal,
        Some(&*bloom),
        caches,
        QuirksMode::NoQuirks,
        NeedsSelectorFlags::No,
        MatchingForInvalidation::No,
    );
    list.slice()
        .iter()
        .filter(|selector| {
            // `matches_selector_list` passes no hashes, so it can never use the
            // filter; going selector by selector is what turns the bloom filter
            // on (selectors-0.40.0/matching.rs:287-300).
            let hashes = AncestorHashes::new(selector, QuirksMode::NoQuirks);
            matches_selector(selector, 0, Some(&hashes), node, &mut context)
        })
        .map(Selector::specificity)
        .max()
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib css::select::tests`
Expected: PASS — 20 tests (M1's 9 rewritten, Task 5's 2, 9 new).

Run: `cargo test -p icedtea-ui`
Expected: PASS — the M1 gate `tests/themed_button_offscreen.rs` included; it goes
through `CssNode`, which this task did not touch.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
cargo fmt --all --check
cargo clippy -p icedtea-ui --all-targets -- -D warnings
git add ui/src/css/select.rs ui/src/css/node.rs
git commit -m "$(cat <<'EOF'
feat(ui/css): match selectors against the real node tree

impl Element for Node answers every question selectors asks: siblings (so
+, ~ and the whole :nth-* family work), first child, :empty, :root, ids,
classes, and the full pseudo-class set including :focus-within. MatchCx
carries the pass-wide SelectorCaches and drops them when the tree
generation moves, so a mutated tree can never be answered from a stale
nth-index count. matches_with_specificity returns the best matching
selector's specificity, which is what the cascade needs.

select.rs's 9 M1 tests are re-expressed against Node with every assertion
intact; CssNode and its Element impl stay for Part 3.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: Ancestor bloom filter

**Files:**
- Modify: `ui/src/css/select.rs` — implement `Element::add_element_unique_hashes`
  for `Node`; add the bloom tests

**Interfaces:**
- Consumes: Task 1's `Node::for_each_identity_hash`; Task 6's `MatchCx`,
  `push_ancestor`, `pop_ancestor`, `seed_for`, `matches`.
- Produces: `impl Element for Node { fn add_element_unique_hashes(&self, filter: &mut BloomFilter) -> bool }`
  — inserts the node's name, id and class hashes and returns `true`.

- [ ] **Step 1: Write the failing test**

Append to `ui/src/css/select.rs`'s `mod tests`:

```rust
    #[test]
    fn unique_hashes_are_inserted_and_reported() {
        use selectors::Element;
        use selectors::bloom::{BLOOM_HASH_MASK, BloomFilter};

        let node = Node::with_classes("button", &["flat"]);
        node.set_id(Some("close"));
        let mut filter = BloomFilter::new();
        assert!(
            node.add_element_unique_hashes(&mut filter),
            "returning false makes the ancestor filter a permanent no-op"
        );
        for text in ["button", "flat", "close"] {
            assert!(
                filter.might_contain_hash(
                    crate::css::node::fnv1a(text.as_bytes()) & BLOOM_HASH_MASK
                ),
                "`{text}` must be in the filter"
            );
        }
    }

    #[test]
    fn push_and_pop_ancestor_are_exact_inverses() {
        let window = Node::with_classes("window", &["background"]);
        let button = Node::new("button");
        window.append_child(&button);

        let mut cx = MatchCx::new();
        cx.push_ancestor(&window);
        assert!(hits("window.background > button", &button));
        cx.pop_ancestor(&window);
        cx.reset();
        assert!(
            hits("window.background > button", &button),
            "a popped filter must leave no residue that rejects a real match"
        );
    }

    #[test]
    fn the_bloom_filter_never_changes_an_answer() {
        // The filter may only *fast-reject* selectors that could not match. Any
        // rule it rejects that actually matches is a silently lost style.
        use crate::css::cascade::CompiledSheet;

        let sheet = CompiledSheet::compile(crate::BUNDLED_ADWAITA_LIGHT);
        let window = Node::with_classes("window", &["background", "csd"]);
        let headerbar = Node::with_classes("headerbar", &["titlebar"]);
        let box_node = Node::with_classes("box", &["linked", "horizontal"]);
        let button = Node::with_classes("button", &["suggested-action", "text-button"]);
        let label = Node::new("label");
        window.append_child(&headerbar);
        headerbar.append_child(&box_node);
        box_node.append_child(&Node::new("entry"));
        box_node.append_child(&button);
        button.append_child(&label);
        button.set_states(PseudoStates::HOVER);

        let mut with_filter = MatchCx::new();
        for node in [&window, &headerbar, &box_node, &button, &label] {
            let mut filtered = Vec::new();
            let mut unfiltered = Vec::new();
            for (index, rule) in sheet.rules.iter().enumerate() {
                if matches(&rule.selectors, node, &mut with_filter) {
                    filtered.push(index);
                }
                // A fresh context that has never been seeded still holds an empty
                // filter, so re-seed it per node to compare like with like: the
                // reference answer comes from matching every selector directly.
                let mut plain = MatchCx::new();
                plain.seed_for(node);
                if super::matches_unfiltered(&rule.selectors, node, &mut plain) {
                    unfiltered.push(index);
                }
            }
            assert_eq!(
                filtered,
                unfiltered,
                "the bloom filter changed the answer for {:?}",
                node.name()
            );
            assert!(!filtered.is_empty(), "{:?} must match something", node.name());
        }
    }
```

**Mutation checks.**
`unique_hashes_are_inserted_and_reported`: keep the `false` return → the first
assertion fails; drop the class hashes → `flat` is missing.
`push_and_pop_ancestor_are_exact_inverses`: use `insert_hash` in `pop_ancestor` →
the counting filter never empties and, once `reset` is removed from the test, a
real match is rejected. (`reset()` is in the test to prove the *filter* is clean;
delete the `pop_ancestor` body and the assertion still passes only because of the
reset — so also mutate `reset` to a no-op to see the second assertion fail.)
`the_bloom_filter_never_changes_an_answer`: insert the node's *own* hashes in
`seed_for` in addition to its ancestors' → no failure (the filter only ever
over-approximates), but **omit** any ancestor's hashes — e.g. stop the ancestor
walk one level short — and descendant-combinator rules like
`window.background headerbar button` drop out of `filtered`, failing the equality.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib css::select::tests`
Expected: FAIL — `unique_hashes_are_inserted_and_reported` fails on
`returning false makes the ancestor filter a permanent no-op`, and the file fails
to compile with `error[E0425]: cannot find function `matches_unfiltered` in module
`super``.

- [ ] **Step 3: Write the implementation**

In `ui/src/css/select.rs`, replace the placeholder in `impl Element for Node`:

```rust
    fn add_element_unique_hashes(&self, filter: &mut BloomFilter) -> bool {
        // Name, id and every class. `selectors` masks its queries with
        // `BLOOM_HASH_MASK` (matching.rs:161), so the hashes are pre-masked in
        // `Node::for_each_identity_hash` and the two sides agree exactly.
        self.for_each_identity_hash(|hash| filter.insert_hash(hash));
        true
    }
```

Add the unfiltered reference matcher next to `matches_with_specificity` — it is the
control arm the filter is checked against, and the only way to prove a fast-reject
never changes an answer:

```rust
/// [`matches`] with the ancestor filter switched off.
///
/// The bloom filter may only ever *reject* selectors that could not match;
/// this is the reference implementation that claim is tested against, and the
/// escape hatch for a caller that cannot position a filter.
#[must_use]
pub fn matches_unfiltered(
    list: &SelectorList<GtkSelectorImpl>,
    node: &Node,
    cx: &mut MatchCx,
) -> bool {
    let MatchCx { caches, .. } = cx;
    let mut context = MatchingContext::new(
        MatchingMode::Normal,
        None,
        caches,
        QuirksMode::NoQuirks,
        NeedsSelectorFlags::No,
        MatchingForInvalidation::No,
    );
    list.slice()
        .iter()
        .any(|selector| matches_selector(selector, 0, None, node, &mut context))
}
```

`fnv1a` must be reachable from the test: in `ui/src/css/node.rs` it is already
`pub(crate)`, so `crate::css::node::fnv1a` resolves inside the crate's own tests.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib css::select::tests`
Expected: PASS — 23 tests.

Run: `cargo test -p icedtea-ui`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
cargo fmt --all --check
cargo clippy -p icedtea-ui --all-targets -- -D warnings
git add ui/src/css/select.rs
git commit -m "$(cat <<'EOF'
feat(ui/css): switch on the ancestor bloom filter

add_element_unique_hashes now inserts a node's name, id and class hashes
(pre-masked to match how selectors queries them) and returns true, which
M1 stubbed to false -- a permanent no-op. matches_unfiltered is the
reference arm proving, over all 900 Adwaita rules and five nodes of a real
tree, that the fast-reject never changes an answer.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: Rule buckets

**Files:**
- Modify: `ui/src/css/select.rs` — add `RuleBuckets`, `BucketKey`, `bucket_key`;
  add the bucketing tests

**Interfaces:**
- Consumes: `crate::css::cascade::CompiledRule` (M1 shape: `pub selectors:
  SelectorList<GtkSelectorImpl>`, `pub declarations`, `pub source_order`);
  `selectors::parser::Component`; Task 1's `Node::borrow_id`/`borrow_classes`/`name`.
- Produces:
  ```rust
  pub struct RuleBuckets { /* private */ }
  impl RuleBuckets {
      pub fn build(rules: &[CompiledRule]) -> Self;
      pub fn candidates(&self, node: &Node) -> Vec<usize>;   // ascending, deduped
      pub fn len(&self) -> usize;                            // rules indexed
      pub fn is_empty(&self) -> bool;
  }
  impl Default for RuleBuckets { fn default() -> Self; }
  ```
  Part 3 stores one on `CompiledSheet` (contract §4) and feeds `candidates()` into
  the cascade loop.

- [ ] **Step 1: Write the failing test**

Append to `ui/src/css/select.rs`'s `mod tests`:

```rust
    #[test]
    fn buckets_index_by_the_rightmost_compound() {
        use super::RuleBuckets;
        use crate::css::cascade::CompiledSheet;

        // id beats class beats name; a rightmost compound with none of the three
        // (`* > :first-child`) must land in the universal bucket.
        let sheet = CompiledSheet::compile(concat!(
            "button { color: red; }\n",
            ".suggested-action { color: green; }\n",
            "#close { color: blue; }\n",
            "window > * { color: black; }\n",
            "box > :first-child { color: white; }\n",
        ));
        assert_eq!(sheet.rules.len(), 5, "the fixture must compile whole");
        let buckets = RuleBuckets::build(&sheet.rules);
        assert_eq!(buckets.len(), 5);

        let window = Node::new("window");
        let button = Node::with_classes("button", &["suggested-action"]);
        window.append_child(&button);
        let candidates = buckets.candidates(&button);
        // button (name) + .suggested-action (class) + the two universals.
        assert_eq!(candidates, vec![0, 1, 3, 4]);
        assert!(
            !candidates.contains(&2),
            "a rule bucketed by id must not reach a node without that id"
        );

        button.set_id(Some("close"));
        assert_eq!(buckets.candidates(&button), vec![0, 1, 2, 3, 4]);

        let entry = Node::new("entry");
        window.append_child(&entry);
        assert_eq!(
            buckets.candidates(&entry),
            vec![3, 4],
            "only the universal bucket applies"
        );
    }

    #[test]
    fn a_rule_lands_in_a_bucket_for_every_selector_it_has() {
        use super::RuleBuckets;
        use crate::css::cascade::CompiledSheet;

        let sheet = CompiledSheet::compile("notebook > header > tabs > arrow, button { color: red; }");
        let buckets = RuleBuckets::build(&sheet.rules);
        assert_eq!(buckets.candidates(&Node::new("arrow")), vec![0]);
        assert_eq!(buckets.candidates(&Node::new("button")), vec![0]);
        assert!(buckets.candidates(&Node::new("entry")).is_empty());
    }

    #[test]
    fn every_matching_adwaita_rule_is_offered_as_a_candidate() {
        // Bucketing is only sound if it never hides a rule that would match.
        use super::RuleBuckets;
        use crate::css::cascade::CompiledSheet;

        let sheet = CompiledSheet::compile(crate::BUNDLED_ADWAITA_LIGHT);
        let buckets = RuleBuckets::build(&sheet.rules);

        let window = Node::with_classes("window", &["background", "csd"]);
        let headerbar = Node::with_classes("headerbar", &["titlebar"]);
        let box_node = Node::with_classes("box", &["linked"]);
        let button = Node::with_classes("button", &["suggested-action"]);
        let label = Node::new("label");
        let swatch = Node::new("colorswatch");
        swatch.set_id(Some("add-color-button"));
        window.append_child(&headerbar);
        headerbar.append_child(&box_node);
        box_node.append_child(&button);
        button.append_child(&label);
        headerbar.append_child(&swatch);
        button.set_states(PseudoStates::HOVER);

        let mut cx = MatchCx::new();
        for node in [&window, &headerbar, &box_node, &button, &label, &swatch] {
            let candidates = buckets.candidates(node);
            let mut matched = 0usize;
            for (index, rule) in sheet.rules.iter().enumerate() {
                if matches(&rule.selectors, node, &mut cx) {
                    matched += 1;
                    assert!(
                        candidates.binary_search(&index).is_ok(),
                        "rule {index} matches {:?} but was not a candidate",
                        node.name()
                    );
                }
            }
            assert!(matched > 0, "{:?} must match something", node.name());
            assert!(
                candidates.len() < sheet.rules.len(),
                "bucketing must actually narrow {} rules for {:?}",
                sheet.rules.len(),
                node.name()
            );
        }
    }
```

**Mutation checks.**
`buckets_index_by_the_rightmost_compound`: prefer the *name* over the id in
`bucket_key` → `#close` lands in `by_name` under no name and the `vec![0, 1, 3, 4]`
assertion picks up index 2; drop the universal bucket → indices 3 and 4 vanish.
`a_rule_lands_in_a_bucket_for_every_selector_it_has`: bucket only
`rule.selectors.slice()[0]` → `button` stops being a candidate.
`every_matching_adwaita_rule_is_offered_as_a_candidate`: iterate the *whole*
selector instead of stopping at the first combinator (i.e. call `next_sequence()`)
→ `window > button` gets bucketed under `window`, and the button node loses a
matching rule, failing the soundness assertion. Removing the class bucket lookup
from `candidates` fails it too.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib css::select::tests`
Expected: FAIL to compile — `error[E0432]: unresolved import `super::RuleBuckets``.

- [ ] **Step 3: Write the implementation**

Add to `ui/src/css/select.rs` (after `RuleBuckets`' consumers, near `MatchCx`), plus
`use std::collections::HashMap;` and `use crate::css::cascade::CompiledRule;` at the
top of the file:

```rust
/// Compiled rules indexed by the rightmost compound selector's id, class or
/// element name, so a node is only offered the rules that can possibly match it.
///
/// `selectors` 0.40 has no bucketing of its own (research/cssparser-selectors.md
/// §7); this is built on `Selector::iter()`, whose first sequence is exactly the
/// rightmost compound.
#[derive(Debug, Default)]
pub struct RuleBuckets {
    by_id: HashMap<String, Vec<usize>>,
    by_class: HashMap<String, Vec<usize>>,
    by_name: HashMap<String, Vec<usize>>,
    universal: Vec<usize>,
    len: usize,
}

/// Which bucket one selector belongs in.
enum BucketKey {
    Id(String),
    Class(String),
    Name(String),
    /// The rightmost compound names no id, class or element — `* > :first-child`,
    /// `:root`, `::selection`. These rules must be offered to every node.
    Universal,
}

/// The bucket for one selector: id, else the first class, else the element name,
/// else universal. Only the rightmost compound is considered — `Selector::iter()`
/// stops at the first combinator, and `next_sequence()` is deliberately not
/// called.
fn bucket_key(selector: &Selector<GtkSelectorImpl>) -> BucketKey {
    let mut id = None;
    let mut class = None;
    let mut name = None;
    for component in selector.iter() {
        match component {
            Component::ID(value) => id = Some(value.as_str().to_string()),
            Component::Class(value) => {
                if class.is_none() {
                    class = Some(value.as_str().to_string());
                }
            }
            Component::LocalName(local) => name = Some(local.name.as_str().to_string()),
            _ => {}
        }
    }
    if let Some(id) = id {
        BucketKey::Id(id)
    } else if let Some(class) = class {
        BucketKey::Class(class)
    } else if let Some(name) = name {
        BucketKey::Name(name)
    } else {
        BucketKey::Universal
    }
}

impl RuleBuckets {
    /// Index `rules` by their rightmost compounds. A rule with several selectors
    /// is indexed once per selector.
    #[must_use]
    pub fn build(rules: &[CompiledRule]) -> Self {
        let mut buckets = RuleBuckets {
            len: rules.len(),
            ..RuleBuckets::default()
        };
        for (index, rule) in rules.iter().enumerate() {
            for selector in rule.selectors.slice() {
                match bucket_key(selector) {
                    BucketKey::Id(key) => buckets.by_id.entry(key).or_default().push(index),
                    BucketKey::Class(key) => buckets.by_class.entry(key).or_default().push(index),
                    BucketKey::Name(key) => buckets.by_name.entry(key).or_default().push(index),
                    BucketKey::Universal => buckets.universal.push(index),
                }
            }
        }
        for list in buckets
            .by_id
            .values_mut()
            .chain(buckets.by_class.values_mut())
            .chain(buckets.by_name.values_mut())
            .chain(std::iter::once(&mut buckets.universal))
        {
            list.dedup();
        }
        buckets
    }

    /// Every rule index that could match `node`, ascending and deduped.
    ///
    /// Sound by construction: a rule is only left out when the rightmost
    /// compound of *every* one of its selectors names an id, class or element
    /// this node does not have, which no matcher could then satisfy.
    #[must_use]
    pub fn candidates(&self, node: &Node) -> Vec<usize> {
        let mut out = self.universal.clone();
        node.borrow_id(|id| {
            if let Some(id) = id
                && let Some(list) = self.by_id.get(id.as_str())
            {
                out.extend_from_slice(list);
            }
        });
        node.borrow_classes(|classes| {
            for class in classes {
                if let Some(list) = self.by_class.get(class.as_str()) {
                    out.extend_from_slice(list);
                }
            }
        });
        if let Some(list) = self.by_name.get(&*node.name()) {
            out.extend_from_slice(list);
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    /// How many rules are indexed.
    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether no rules are indexed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}
```

> `if let … && let …` is edition-2024 let-chains, which this crate's rust-version
> 1.94 supports. If clippy objects, nest the two `if let`s.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p icedtea-ui --lib css::select::tests`
Expected: PASS — 26 tests.

Run: `cargo test -p icedtea-ui`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
cargo fmt --all --check
cargo clippy -p icedtea-ui --all-targets -- -D warnings
git add ui/src/css/select.rs
git commit -m "$(cat <<'EOF'
feat(ui/css): bucket compiled rules by their rightmost compound

Rules are indexed by the rightmost compound's id, class or element name --
selectors 0.40 ships no bucketing, so this is built on Selector::iter(),
whose first sequence is that compound. Soundness is pinned against all 900
Adwaita rules: every rule that actually matches a node of a real tree is
offered as a candidate for it, and the candidate set is strictly smaller
than the sheet.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 9: The Adwaita tree-structural regression

**Files:**
- Modify: `ui/src/css/select.rs` — add the regression test and the module-header
  documentation update

**Interfaces:**
- Consumes: everything Parts 1–8 of this plan produced; `crate::BUNDLED_ADWAITA_LIGHT`;
  `crate::css::parse::parse_stylesheet`; `crate::css::cascade::CompiledSheet`.
- Produces: no new API. This is the part's gate.

- [ ] **Step 1: Derive the pinned rule count, then write the failing test**

First re-derive the constant the test pins, so it is a measurement and not a
memory:

```bash
grep -c -E ':(first|last|only)-child' ui/themes/adwaita-light.css
```

Expected output: `44`. Every prelude in the vendored sheet that uses one of these
three pseudo-classes is on its own line, so the line count is the rule count. If
this prints something else, the vendored theme changed: use the printed number in
the test below and say so in the commit message.

Append to `ui/src/css/select.rs`'s `mod tests`:

```rust
    /// The rules M1 could not match: a two-node chain made `:first-child`,
    /// `:last-child` and `:only-child` match everything and `:not(:first-child)`
    /// match nothing. Derived from the vendored sheet with
    /// `grep -c -E ':(first|last|only)-child' ui/themes/adwaita-light.css`.
    const ADWAITA_TREE_STRUCTURAL_RULES: usize = 44;

    #[test]
    fn adwaitas_tree_structural_rules_are_all_present_and_parse() {
        use crate::css::parse::parse_stylesheet;

        let sheet = parse_stylesheet(crate::BUNDLED_ADWAITA_LIGHT);
        let structural: Vec<&str> = sheet
            .rules
            .iter()
            .map(|rule| rule.selector_text.as_str())
            .filter(|text| {
                text.contains(":first-child")
                    || text.contains(":last-child")
                    || text.contains(":only-child")
            })
            .collect();
        assert_eq!(
            structural.len(),
            ADWAITA_TREE_STRUCTURAL_RULES,
            "the vendored Adwaita sheet's tree-structural rule count moved"
        );
        for text in &structural {
            assert!(
                parse_selector_list(text).is_some(),
                "a tree-structural prelude must survive the selector parser: {text}"
            );
        }
    }

    #[test]
    fn adwaitas_tree_structural_selectors_match_a_real_tree() {
        // Each of these is a verbatim selector from the vendored sheet. Under
        // M1's constant-stub Element they all answered wrongly.
        let list_box = Node::with_classes("box", &["boxed-list"]);
        let first = Node::new("row");
        let middle = Node::new("row");
        let last = Node::new("row");
        for row in [&first, &middle, &last] {
            list_box.append_child(row);
        }
        assert!(hits(".boxed-list > row:first-child", &first));
        assert!(!hits(".boxed-list > row:first-child", &middle));
        assert!(hits(".boxed-list > row:last-child", &last));
        assert!(!hits(".boxed-list > row:last-child", &middle));

        // `.linked.vertical > button:not(:first-child)`
        let linked = Node::with_classes("box", &["linked", "vertical"]);
        let top = Node::new("button");
        let bottom = Node::new("button");
        linked.append_child(&top);
        linked.append_child(&bottom);
        assert!(!hits(".linked.vertical > button:not(:first-child)", &top));
        assert!(hits(".linked.vertical > button:not(:first-child)", &bottom));
        assert!(hits(".linked.vertical > button:not(:last-child)", &top));

        // `notebook > header.top > tabs:not(:only-child)`
        let notebook = Node::new("notebook");
        let header = Node::with_classes("header", &["top"]);
        let tabs = Node::new("tabs");
        let arrow = Node::new("arrow");
        notebook.append_child(&header);
        header.append_child(&tabs);
        assert!(
            !hits("notebook > header.top > tabs:not(:only-child)", &tabs),
            "an only child must not match :not(:only-child)"
        );
        header.append_child(&arrow);
        assert!(hits("notebook > header.top > tabs:not(:only-child)", &tabs));
        assert!(hits("notebook > header.top > tabs:not(:only-child):first-child", &tabs));

        // `dropdown.linked button:nth-child(2):dir(ltr)`
        let dropdown = Node::with_classes("dropdown", &["linked"]);
        let one = Node::new("button");
        let two = Node::new("button");
        dropdown.append_child(&one);
        dropdown.append_child(&two);
        assert!(!hits("dropdown.linked button:nth-child(2):dir(ltr)", &one));
        assert!(hits("dropdown.linked button:nth-child(2):dir(ltr)", &two));
        dropdown.set_direction(Some(Direction::Rtl));
        assert!(!hits("dropdown.linked button:nth-child(2):dir(ltr)", &two));
        assert!(hits("dropdown.linked button:nth-child(2):dir(rtl)", &two));

        // `.linked.vertical > entry:drop(active):not(:only-child) + button`
        let vertical = Node::with_classes("box", &["linked", "vertical"]);
        let entry = Node::new("entry");
        let button = Node::new("button");
        vertical.append_child(&entry);
        vertical.append_child(&button);
        assert!(!hits(
            ".linked.vertical > entry:drop(active):not(:only-child) + button",
            &button
        ));
        entry.set_state(PseudoStates::DROP_ACTIVE, true);
        assert!(hits(
            ".linked.vertical > entry:drop(active):not(:only-child) + button",
            &button
        ));

        // `colorswatch#add-color-button:only-child`
        let grid = Node::new("colorchooser");
        let swatch = Node::new("colorswatch");
        swatch.set_id(Some("add-color-button"));
        grid.append_child(&swatch);
        assert!(hits("colorswatch#add-color-button:only-child", &swatch));
        grid.append_child(&Node::new("colorswatch"));
        assert!(!hits("colorswatch#add-color-button:only-child", &swatch));

        // `textview > text > selection:focus-within`
        let textview = Node::new("textview");
        let text = Node::new("text");
        let selection = Node::new("selection");
        let caret = Node::new("cursor");
        textview.append_child(&text);
        text.append_child(&selection);
        selection.append_child(&caret);
        assert!(!hits("textview > text > selection:focus-within", &selection));
        caret.set_state(PseudoStates::FOCUS, true);
        assert!(hits("textview > text > selection:focus-within", &selection));
    }

    #[test]
    fn a_real_adwaita_tree_matches_more_rules_than_a_flat_one() {
        // The end-to-end proof that the tree is load-bearing: the same button
        // node, once given real siblings and ancestors, matches a strictly
        // larger set of Adwaita rules than it does standing alone.
        use crate::css::cascade::CompiledSheet;

        let sheet = CompiledSheet::compile(crate::BUNDLED_ADWAITA_LIGHT);
        let mut cx = MatchCx::new();

        let lonely = Node::new("button");
        let lonely_hits = sheet
            .rules
            .iter()
            .filter(|rule| matches(&rule.selectors, &lonely, &mut cx))
            .count();

        let window = Node::with_classes("window", &["background"]);
        let box_node = Node::with_classes("box", &["linked"]);
        let sibling = Node::new("button");
        let button = Node::new("button");
        window.append_child(&box_node);
        box_node.append_child(&sibling);
        box_node.append_child(&button);
        let embedded_hits = sheet
            .rules
            .iter()
            .filter(|rule| matches(&rule.selectors, &button, &mut cx))
            .count();

        assert!(
            embedded_hits > lonely_hits,
            "a button inside a linked box must match more rules ({embedded_hits}) \
             than a detached one ({lonely_hits})"
        );
    }
```

**Mutation checks.**
`adwaitas_tree_structural_rules_are_all_present_and_parse`: narrow the filter to
`:first-child` only → the count assertion fails.
`adwaitas_tree_structural_selectors_match_a_real_tree`: restore any one of M1's
four constant stubs (`prev_sibling_element`, `next_sibling_element`,
`first_element_child`, `is_empty`) → at least three assertions fail; map
`Drop(_)` back to `false` → the `:drop(active) + button` assertion fails; map
`FocusWithin` to `false` → the `textview` assertion fails.
`a_real_adwaita_tree_matches_more_rules_than_a_flat_one`: make `parent_element`
return `None` → descendant rules stop matching and `embedded_hits == lonely_hits`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p icedtea-ui --lib css::select::tests::adwaita`
Expected: PASS for `adwaitas_tree_structural_rules_are_all_present_and_parse` and
the other two — this task adds no production code, so all three must be green on
first run. **That is the point of the task**: they are the regression net over
Tasks 1–8. To confirm they are load-bearing rather than vacuous, temporarily
revert `first_element_child` to `None` in `impl Element for Node`, re-run, and
observe `adwaitas_tree_structural_selectors_match_a_real_tree` fail on
`:not(:only-child)`; then restore it.

Run: `cargo test -p icedtea-ui --lib css::select::tests`
Expected: PASS — 29 tests.

- [ ] **Step 3: Update the module documentation**

Replace `ui/src/css/select.rs`'s file header (select.rs:1-8) — it still describes
M1's world, where "no siblings" was the design:

```rust
//! Selector matching over the node tree, through Servo's `selectors` crate.
//!
//! GTK widgets are not a DOM, but they are a tree: [`crate::css::node::Node`]
//! has an element name, an optional id, style classes, pseudo-class state, a
//! writing direction, a parent and ordered children — and nothing else (no
//! attributes, no namespaces, no shadow roots). `Node` implements
//! [`selectors::Element`] completely over that model, so the upstream matcher
//! drives selection with no fork and no shim: combinators (descendant, `>`,
//! `+`, `~`), the whole `:nth-*` family, `:root`, `:empty`, ids, classes, and
//! GTK's pseudo-class set including `:focus-within`, `:indeterminate` and
//! `:dir()`.
//!
//! [`MatchCx`] carries the state a restyle pass reuses — the nth-index caches
//! and an ancestor bloom filter — and drops it when the tree generation moves.
//! [`RuleBuckets`] narrows a stylesheet to the rules a node could match, which
//! `selectors` itself does not provide.
//!
//! M1's `CssNode` (immutable, parent-only) is still here because the rest of the
//! crate has not been migrated onto `Node` yet; Part 3 of M2 deletes it.
```

- [ ] **Step 4: Run the full gate**

Run: `cargo test -p icedtea-ui`
Expected: PASS — every M1 test, the M1 pixel gate `tests/themed_button_offscreen.rs`
byte-identical and green, and this part's 29 select tests plus 22 node tests.

Run: `cargo test --workspace`
Expected: PASS.

Run: `cargo clippy -p icedtea-ui --all-targets -- -D warnings`
Expected: no output.

Run: `cargo fmt --all --check`
Expected: no output.

- [ ] **Step 5: Commit**

```bash
git add ui/src/css/select.rs
git commit -m "$(cat <<'EOF'
test(ui/css): pin the Adwaita rules M1's node model could not match

44 rules in the vendored sheet use :first-child/:last-child/:only-child,
and M1's constant-stub Element answered all of them wrongly. This pins the
count, proves every one of those preludes parses, and matches seven
verbatim Adwaita selectors -- sibling, nth-child, :dir, :drop(active),
:only-child, an id and :focus-within -- against real trees, plus the
end-to-end check that an embedded button matches strictly more rules than
a detached one.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
EOF
)"
```

---

## Self-Review

### 1. Spec coverage

Spec §Section 2 "Node tree & selectors" and contract §3 / §3.1 / §3.2, bullet by
bullet:

| Spec / contract requirement | Task |
|---|---|
| `Node` = `Rc<NodeInner>`; `RefCell` fields | 1 |
| `name`, `id: Option<String>`, `classes` (interned) | 1 |
| `states: PseudoStates` — hover, active, focus, focus-visible, focus-within, checked, indeterminate, disabled, backdrop, selected, link/visited, drop-active | 1 (flags), 3 (behaviour) |
| `focus-within` derived from descendants | 3 |
| `direction: Ltr\|Rtl`, inherited from the parent unless set | 4 |
| `parent: Weak`, `children: Vec<Node>` in order | 2 |
| `append/insert/remove_child`, `detach` | 2 |
| `set_states`, `add/remove_class`, `set_classes`, `set_direction`, `set_id` | 1, 3, 4 |
| Mutations bump a per-tree `generation`; `self_generation` | 1 (identity), 2 (structure), 3 (state), 4 (direction) |
| Reads: `parent`, `children`, `child`, `child_count`, `index_in_parent`, `root`, `ancestors`, `descendants`, `ptr_eq` | 2 |
| `Element` impl complete: `parent_element`, `prev/next_sibling_element`, `first_element_child`, `is_empty`, `is_root`, `has_id`, `has_class`, `opaque`, `has_local_name`/`has_namespace`/`is_same_type` | 6 |
| Full GTK pseudo-class list `:link :visited :active :hover :focus :focus-within :focus-visible :disabled :checked :indeterminate :backdrop :selected :not() :dir() :drop(active) :root :nth-child() :nth-last-child() :first-child :last-child :only-child` | 5 (the custom ones), 6 (matching; the tree-structural ones are `selectors` built-ins driven by the real sibling methods) |
| Combinators descendant / `>` / `+` / `~`, universal `*` | 6 |
| `attr_matches`, `has_custom_state`, `imported_part`, `is_part`, `is_html_slot_element` etc. stay `false`, documented on the impl | 6 |
| `:nth-child(… of S)` not enabled (`parse_nth_child_of` default) | 6 (documented + pinned by a test) |
| `add_element_unique_hashes` inserts name + classes + id and returns `true` | 7 |
| Rules bucketed by the rightmost compound's name/id/class | 8 |
| One caller-owned `MatchingContext` with the bloom filter over ancestors | 6 (`MatchCx`), 7 (filter) |
| Restyle re-matches only changed nodes — the generation/`self_generation` hooks that make it possible | 1–4 (produced), consumed by Part 3 |
| `MatchCx::{new, push_ancestor, pop_ancestor, reset, seed_for}` | 6, 7 |
| `RuleBuckets::{build, candidates}` | 8 |
| `parse_selector_list`, `matches`, `matches_with_specificity` | 6 |
| `Direction`/`CssString` keep M1 definitions, moved to `node.rs`, re-exported from `select.rs` | 1 |
| `GtkPseudoClass` gains `FocusWithin`, `Indeterminate`, `Link`, `Visited`; `Other`/`OtherFunctional` keep parse-but-never-match | 5 |
| P2 gate: the Adwaita sibling regression | 9 |
| P2 gate: `add_element_unique_hashes` returns `true` | 7 |
| P2 gate: select.rs's 9 M1 tests preserved | 6 |
| Spec §7 "never panics (fuzz-ish tests over odd inputs)" applied to this part's parsing surface (`Direction::parse`, `parse_selector_list`) | 4, 6 |
| Spec §7 "every load-bearing test records a mutation check" | every task |

No gap. Everything else in the spec (registry, values, cascade, computed, layout,
paint, animation, fonts, the coverage instrument) belongs to Parts 1, 3, 4, 5, 6.

### 2. Placeholder scan

Searched for `TBD`, `TODO`, `implement later`, `fill in`, `add appropriate`,
`handle edge cases`, `similar to Task`, `write tests for the above`: none present.
Every code step carries complete, compilable code; every test step carries the
test body. Three places give a conditional instruction rather than a fixed one, and
each names the exact condition and the exact action:

- Task 1 Step 3 — if Part 1 already reshaped `css/mod.rs`'s module list, insert
  only `pub mod node;` and keep the rest as found.
- Task 1 Step 3 / Task 2 Step 3 — `bump_tree` is written in Task 1 and deleted in
  Task 2 if `-D warnings` flags it as dead; both tasks say so explicitly.
- Task 9 Step 1 — re-derive `ADWAITA_TREE_STRUCTURAL_RULES` with the exact `grep`
  before pinning it, and use what it prints. The expected value (`44`) was measured
  against the vendored sheet while writing this plan.

### 3. Type consistency vs the contract

- `Node`, `PseudoStates`, `Direction`, `CssString` — names, methods and signatures
  as contract §3, with `states`/`set_states`/`set_state` returning and taking
  `PseudoStates`, `set_direction(Option<Direction>)`, `direction() -> Direction`,
  `name() -> Rc<str>`, `id() -> Option<CssString>`, `classes() -> Vec<CssString>`.
- `MatchCx::{new, push_ancestor, pop_ancestor, reset, seed_for}` — exactly contract
  §3.2, `&mut self` throughout, `push_ancestor`/`pop_ancestor` taking `&Node`.
- `RuleBuckets::{build(&[CompiledRule]) -> Self, candidates(&Node) -> Vec<usize>}`
  — exactly contract §3.2. `len`/`is_empty` are additions, which §"Every signature
  below is normative: a part may add private items freely" permits (they are the
  clippy-mandated companion to `len` and are used by Task 8's tests).
- `parse_selector_list(&str) -> Option<SelectorList<GtkSelectorImpl>>`,
  `matches(&SelectorList<GtkSelectorImpl>, &Node, &mut MatchCx) -> bool`,
  `matches_with_specificity(...) -> Option<u32>` — exactly contract §3.2.
  `matches_unfiltered` is an addition (Task 7), used as the control arm of the
  bloom-filter soundness test.
- `GtkPseudoClass` variant names match contract §3.1's "gains `FocusWithin`,
  `Indeterminate`, `Link`, `Visited`".
- Cross-task names: `for_each_identity_hash` (Task 1) is called by Task 6's
  `push_ancestor`/`pop_ancestor` and Task 7's `add_element_unique_hashes` under
  that one name; `addr` (Task 1) is used by `MatchCx::seed_for`/`push_ancestor`/
  `pop_ancestor` under that one name; `borrow_id`/`borrow_classes` (Task 1) are
  used by Task 6's `has_id`/`has_class` and Task 8's `candidates` under those
  names; `add_focus_count`/`sub_focus_count` are defined once (Task 2's diff) and
  called from Task 2's attach/detach and Task 3's `set_states`;
  `adopt_tree`/`touch` are single-definition, multi-caller and consistent.
- Consumed from outside the part: `CompiledRule.selectors`,
  `CompiledSheet::compile`, `parse_stylesheet(...).rules[..].selector_text`,
  `crate::BUNDLED_ADWAITA_LIGHT` — all present in the tree today in exactly these
  shapes, and contract §4 keeps `CompiledSheet::compile` and §10.1 keeps
  `parse::StyleRule` unchanged.
