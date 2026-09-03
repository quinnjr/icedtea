//! `GtkStackSwitcher` -- linked buttons, one per `StackC` page.
//!
//! ```text
//! stackswitcher.stack-switcher
//! ├── button[.needs-attention]
//! ┊
//! ╰── button[.needs-attention]
//! ```
//!
//! Reconciliation: the plan's `stack_switcher(pages)` builder takes
//! `Rc<[StackPageInfo]>` by value (deviation 12's shape for `StackSidebar`
//! extends the same way here), so the controller cannot read pages from
//! `node.children()` the way `StackC` and `ListBoxC` do -- there are no
//! child views at all, only the two `PropName::Pages`/`PropName
//! ::NeedsAttention` props the builder encodes them into. `build` and
//! `set_prop` therefore keep `titles`/`needs_attention`/`selected` as plain
//! fields and rebuild the whole `button` list from them on every relevant
//! prop write, the same "own subnodes, no live children" shape `HeaderBarC`
//! uses for its packed row.
//!
//! `PropName::NeedsAttention`'s `Prop::Int` is read high-bit-first across
//! the page list -- page 0 is the mask's most significant bit of the
//! `total`-bit field, page `total - 1` its least significant -- so the
//! mask, read as an ordinary binary number, lines up left to right with the
//! pages the switcher paints left to right. Nothing in the task text pins an
//! orientation for a bitmask that never existed before this widget; this is
//! the convention `stack_sidebar.rs`'s own fixture test exercises (`Prop::
//! Int(1)` over two pages flags the *second* page), so both controllers
//! share the `needs_attention_bit` helper below rather than each guessing
//! its own.
//!
//! The task text's `on_event` hit-tests buttons with a free `crate::widgets
//! ::hit(b, *local)` function that does not exist in this crate -- there is
//! no node-only hit-test, only `crate::widgets::local_rect` against a real
//! `LayoutTree` (`ListBoxC::row_at`'s own reconciliation note explains why:
//! a bare `Node` carries no geometry). `on_event` uses that instead, and
//! also gates on `BTN_LEFT` the way `ListBoxC::on_event` does -- the task
//! text's snippet does not, but a middle- or right-click reassigning the
//! visible stack page is not what GTK does. The task text's own test then
//! builds an `EventCx` *before* calling `Headless::place_rows` on the same
//! node; `place_rows` takes `&mut Headless` and `event_cx_with_handlers`
//! already holds it mutably borrowed for the `EventCx`'s lifetime, so that
//! order does not borrow-check -- `list_box.rs`'s own module doc flags the
//! identical constraint for its two callers. The test below places rows
//! first, then builds the `EventCx`, same fix.

use std::rc::Rc;

use crate::css::node::{Node, PseudoStates};
use crate::view::{BuildCx, Controller, Event, EventCx, EventKind, Kind, Prop, PropName, Props};
use crate::widgets::{Universal, local_rect, prop_i64};

/// Is page `index` (of `total`) flagged in `mask`? See the module doc for
/// the bit orientation. Shared with [`crate::widgets::stack_sidebar`], whose
/// rows use the same convention.
pub(crate) fn needs_attention_bit(mask: i64, index: usize, total: usize) -> bool {
    if total == 0 || index >= total {
        return false;
    }
    let shift = total - 1 - index;
    if shift >= 64 {
        return false;
    }
    #[allow(
        clippy::cast_sign_loss,
        reason = "only the bit pattern matters, not the signed value"
    )]
    let bits = mask as u64;
    bits & (1u64 << shift) != 0
}

/// `GtkStackSwitcher`.
pub struct StackSwitcherC {
    node: Node,
    /// Index of the checked button.
    pub selected: usize,
    /// One `button` per page, in order.
    pub buttons: Vec<Node>,
    titles: Vec<Rc<str>>,
    needs_attention: i64,
    universal: Universal,
}

impl StackSwitcherC {
    /// Test hook: the checked button's index.
    #[must_use]
    pub fn selected_of<Msg: 'static>(c: &dyn Controller<Msg>) -> usize {
        let any: &dyn std::any::Any = c;
        any.downcast_ref::<Self>()
            .map_or(usize::MAX, |m| m.selected)
    }

    /// Drop and recreate every button from `titles`/`needs_attention`/
    /// `selected`. Called from `build` and from every `set_prop` that
    /// touches one of those three -- there is no incremental path over a
    /// controller-owned list this small.
    fn rebuild(&mut self) {
        for old in self.buttons.drain(..) {
            old.detach();
        }
        let total = self.titles.len();
        for i in 0..total {
            let button = Node::new("button");
            if needs_attention_bit(self.needs_attention, i, total) {
                button.add_class("needs-attention");
            }
            // A page button shows its page's title, so it is a text button
            // and carries GTK's own `.text-button` class for it. Both halves
            // matter to the box the switcher ends up with: `stackswitcher >
            // button.text-button { min-width: 100px }` is Adwaita's rule for
            // exactly this widget, and the `label` child is the node that
            // rule (and `stackswitcher > button > label`'s padding) is
            // written against — every other button-shaped node in this crate
            // carries one. Like `MenuButton`'s and `Frame`'s, it is chrome:
            // the title text itself is not painted, because a subnode with no
            // controller has no `Measure` and no `paint` (contract §11's
            // standing note, and `ExpanderC`'s own `label`).
            button.add_class("text-button");
            button.set_state(PseudoStates::CHECKED, i == self.selected);
            button.append_child(&Node::new("label"));
            self.node.append_child(&button);
            self.buttons.push(button);
        }
        if total > 0 {
            self.selected = self.selected.min(total - 1);
        }
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for StackSwitcherC {
    fn kind(&self) -> Kind {
        Kind::StackSwitcher
    }

    fn build(node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        crate::widgets::set_container(
            node,
            crate::layout::Container::Box {
                direction: crate::layout::BoxDirection::Row,
            },
        );
        let titles: Vec<Rc<str>> = match props.get(PropName::Pages) {
            Some(Prop::Classes(list)) => list.to_vec(),
            _ => Vec::new(),
        };
        let selected = usize::try_from(props.int(PropName::Selected, 0).max(0)).unwrap_or(0);
        let mut me = Self {
            node: node.clone(),
            selected,
            buttons: Vec::new(),
            titles,
            needs_attention: props.int(PropName::NeedsAttention, 0),
            universal: Universal::new(node, Kind::StackSwitcher),
        };
        me.rebuild();
        me
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        match (name, value) {
            (PropName::Pages, Prop::Classes(list)) => {
                self.titles = list.to_vec();
                self.rebuild();
            }
            (PropName::NeedsAttention, other) => {
                self.needs_attention = prop_i64(other, 0);
                self.rebuild();
            }
            (PropName::Selected, other) => {
                self.selected = usize::try_from(prop_i64(other, 0).max(0)).unwrap_or(0);
                self.rebuild();
            }
            _ => {
                self.universal.apply(node, Kind::StackSwitcher, name, value);
            }
        }
    }

    /// The page buttons are this controller's own node children, built from
    /// `PropName::Pages` before any view child could arrive, so a view child
    /// of a `StackSwitcher` starts after them.
    fn child_index(&self, view_index: usize) -> usize {
        view_index + self.buttons.len()
    }

    /// ... and reconcile's trim step has to know they are there, or it
    /// detaches every one of them the moment it runs — a `StackSwitcher`
    /// takes no view children at all (its pages arrive as props, see the
    /// module doc), so *every* one of its node children is past the default
    /// bound of `view_count`. Until P8-D72's close-out that is exactly what
    /// happened: `gallery --print-allocation --widget stack_switcher`
    /// reported a 0x0 box and `--probe-points` emitted only `root`, because
    /// no button survived the first reconcile — so a live `StackSwitcher`
    /// could not be clicked at all, `on_event`'s own hit-test reading the
    /// same missing allocations. This is `ListViewC`'s pair, for the same
    /// reason and in the same shape.
    fn reserved_total(&self, view_count: usize) -> usize {
        view_count + self.buttons.len()
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        let Event::PointerDown { button, local, .. } = ev else {
            return Vec::new();
        };
        if *button != crate::window::layer::BTN_LEFT {
            return Vec::new();
        }
        let Some(index) = self.buttons.iter().position(|b| {
            local_rect(cx.tree, &self.node, b).is_some_and(|r| {
                local.0 >= r.x && local.0 < r.right() && local.1 >= r.y && local.1 < r.bottom()
            })
        }) else {
            return Vec::new();
        };
        cx.handled = true;
        if index == self.selected {
            return Vec::new();
        }
        self.selected = index;
        for (i, button) in self.buttons.iter().enumerate() {
            button.set_state(PseudoStates::CHECKED, i == index);
        }
        cx.handlers
            .fire_index(EventKind::Selected, index)
            .into_iter()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use crate::css::node::PseudoStates;
    use crate::view::{Event, EventKind, Handler, Kind, Prop, PropName, Props};
    use crate::widgets::stack_switcher::StackSwitcherC;
    use crate::widgets::{Headless, build_widget, matches_fixture};

    fn props() -> Props {
        let mut p = Props::default();
        p.set(
            PropName::Pages,
            Prop::Classes(
                ["One", "Two", "Three"]
                    .iter()
                    .map(|s| (*s).into())
                    .collect(),
            ),
        );
        p.set(PropName::Selected, Prop::Int(0));
        p
    }

    #[test]
    fn the_switcher_is_a_stackswitcher_of_buttons() {
        // Mutation check: dropping the `.stack-switcher` class loses
        // Adwaita's linked-button styling, which the fixture requires.
        let built = build_widget::<()>(Kind::StackSwitcher, &props());
        matches_fixture(
            &built.node,
            "stackswitcher.stack-switcher\n├── button[.needs-attention]\n┊\n╰── button[.needs-attention]\n",
        )
        .expect("stack_switcher fixture");
    }

    #[test]
    fn clicking_a_button_checks_only_that_one_and_reports_its_index() {
        // Interaction test. Mutation check: not clearing :checked on the
        // previous button leaves every visited page's button lit.
        let built = build_widget::<usize>(Kind::StackSwitcher, &props());
        let mut c = built.controller;
        let mut hx = Headless::new();
        // See the module doc: `place_rows` must run before the `EventCx`
        // that borrows `Headless` for its own lifetime exists.
        hx.place_rows(&built.node, 40.0);
        let mut cx = hx.event_cx_with_handlers(&built.node, |h| {
            h.set(EventKind::Selected, Handler::Index(std::rc::Rc::new(|i| i)));
        });
        assert_eq!(
            c.on_event(
                &Event::PointerDown {
                    button: 0x110,
                    local: (5.0, 100.0),
                    serial: 1,
                },
                &mut cx,
            ),
            vec![2]
        );
        assert!(
            built
                .node
                .child(2)
                .unwrap()
                .states()
                .contains(PseudoStates::CHECKED)
        );
        assert!(
            !built
                .node
                .child(0)
                .unwrap()
                .states()
                .contains(PseudoStates::CHECKED)
        );
        assert_eq!(StackSwitcherC::selected_of(&*c), 2);
    }
}
