//! `GtkPopoverMenu` — `Kind::PopoverMenu`, CSS node `popover.menu` — and its
//! items, `Kind::PopoverMenuItem` (`button.model`).
//!
//! ```text
//! popover.background.menu
//! ├── arrow
//! ╰── contents
//!     ╰── [box.horizontal.<hint>]
//!         ├── button.model
//!         │   ╰── label
//!         ┊
//!         ╰── button.model
//!             ╰── label
//! ```
//!
//! `PopoverMenu` shares `Popover`'s CSS node name (`Kind::css_name` maps
//! both to `"popover"`), and this controller embeds a real [`PopoverC`]
//! directly on its own root rather than a nested one -- `arrow`/`contents`
//! are this node's own children, exactly as a plain `Popover`'s are.
//!
//! Reconciliation (interfaces vs. the task's own Step 1 test): the task's
//! "Produces" section names the mnemonic lookup `mnemonic_target(&self, ch)`
//! and Step 3's sample impl defines exactly that, but the test module calls
//! `PopoverMenuC::mnemonic_of(&c, ch)` instead -- an associated function
//! taking `&dyn Controller<Msg>` (the shape every other test-hook in this
//! crate uses, e.g. `ListBoxC::selected_rows`), not a `&self` method. Both
//! names are kept: `mnemonic_target` is the documented instance method,
//! `mnemonic_of` is the thin `Any`-downcasting wrapper the tests call --
//! through `c.as_ref()`, not the task text's bare `&c` (`c` there is a
//! `Box<dyn Controller<Msg>>`, and `&Box<dyn Controller<Msg>>>` does not
//! infer against a `&dyn Controller<Msg>` parameter the way `.as_ref()`
//! does; `ListBoxC::selected_rows`'s own call site already uses `.as_ref()`
//! for the same reason).
//!
//! Reconciliation: the task's on-event prose says activation "pushes
//! `Cmd::ClosePopup(self.popover.key())`", but `PopoverC` has no `key()`
//! accessor -- `PopoverC::close` already does exactly this (takes
//! `self.popup` and pushes `Cmd::ClosePopup` when it was `Some`), so this
//! calls that instead of inventing a new accessor.
//!
//! Reconciliation: this build is driven entirely by `PropName::Menus` (a
//! flat `Prop::Classes` list of item labels), `PropName::Section` and
//! `PropName::DisplayHint` -- there is no per-item `Kind::PopoverMenuItem`
//! child in the retained tree (`build_widget`/`node_tree_of`, which the
//! task's own fixture test uses, never attach real children -- see
//! `list_box.rs`'s module doc for the established precedent). `popover_menu`
//! below flattens its `Kind::PopoverMenuItem` argument views into exactly
//! these three props at construction time (the same trick
//! `builders::stack_switcher` already plays on `StackPageInfo`), so an
//! item's own per-item `Handler` (if any were set) is not carried through --
//! only the whole menu's `.on_item_activated`/`EventKind::Activate` (already
//! a universal `View` setter) fires, with the activated item's index.
//!
//! Reconciliation: no per-item submenu name is tracked in this flat shape,
//! so `Right` (open) is a documented no-op; `Left` still closes `self.
//! submenu` when one was recorded, keeping the field meaningful for a caller
//! that sets it directly.

use std::rc::Rc;

use crate::css::node::Node;
use crate::css::value::image::IconRef;
use crate::layout::LayoutTree;
use crate::view::cmd::Cmd;
use crate::view::controller::{Controller, Event, EventCx};
use crate::view::{BuildCx, EventKind, Kind, Prop, PropName, Props, View};
use crate::widgets::local_rect;
use crate::widgets::popover::PopoverC;
use crate::widgets::types::{DisplayHint, MenuFlags};
use crate::window::popup::PopupKey;

/// A `GtkPopoverMenu` over `items` (each a [`popover_menu_item`]).
///
/// Only the first item's `.section(name, hint)` is honoured -- this crate's
/// flat, single-section shape (see the module doc's reconciliation note);
/// a later `.section` on a different item does not open a second section.
#[must_use]
pub fn popover_menu<Msg: Clone + 'static>(items: impl IntoIterator<Item = View<Msg>>) -> View<Msg> {
    let mut labels: Vec<Rc<str>> = Vec::new();
    let mut section: Option<Rc<str>> = None;
    let mut hint = DisplayHint::Normal;
    for item in items {
        if let Some(label) = item.props.str(PropName::Label) {
            labels.push(Rc::from(label));
        }
        if let Some(name) = item.props.str(PropName::Section) {
            section.get_or_insert_with(|| Rc::from(name));
            if let Some(Prop::Enum(raw)) = item.props.get(PropName::DisplayHint) {
                hint = DisplayHint::from_u16(*raw);
            }
        }
    }
    let mut view =
        View::new(Kind::PopoverMenu).prop(PropName::Menus, Prop::Classes(Rc::from(labels)));
    if let Some(name) = section {
        view = view.prop(PropName::Section, Prop::Str(name));
    }
    view.prop(PropName::DisplayHint, Prop::Enum(hint.to_u16()))
}

/// One [`popover_menu`] item, labelled `label` (may carry a `_` mnemonic).
#[must_use]
pub fn popover_menu_item<Msg: Clone + 'static>(label: &str) -> View<Msg> {
    View::new(Kind::PopoverMenuItem).prop(PropName::Label, Prop::Str(Rc::from(label)))
}

/// [`popover_menu`]'s own setters.
pub trait PopoverMenuExt<Msg>: Sized {
    /// `GtkPopoverMenu:flags`.
    fn flags(self, flags: MenuFlags) -> Self;
    /// `GtkPopoverMenu:visible-submenu`.
    fn visible_submenu(self, name: &str) -> Self;
}

impl<Msg: Clone + 'static> PopoverMenuExt<Msg> for View<Msg> {
    fn flags(self, flags: MenuFlags) -> Self {
        self.prop(PropName::Flags, Prop::Int(i64::from(flags.bits())))
    }
    fn visible_submenu(self, name: &str) -> Self {
        self.prop(PropName::VisibleSubmenu, Prop::Str(Rc::from(name)))
    }
}

/// [`popover_menu_item`]'s own setters.
pub trait PopoverMenuItemExt<Msg>: Sized {
    /// `GtkPopoverMenuItem`'s icon (`GMenuModel`'s `icon` attribute).
    fn icon(self, icon: IconRef) -> Self;
    /// The item's accelerator label (`GMenuModel`'s `accel` attribute).
    fn accel(self, accel: &str) -> Self;
    /// Opens a new section named `name`, with `hint` as its `display-hint`.
    fn section(self, name: &str, hint: DisplayHint) -> Self;
    /// Names the submenu this item opens.
    fn submenu(self, name: &str) -> Self;
}

impl<Msg: Clone + 'static> PopoverMenuItemExt<Msg> for View<Msg> {
    fn icon(self, icon: IconRef) -> Self {
        self.prop(PropName::Icon, Prop::Icon(icon))
    }
    fn accel(self, accel: &str) -> Self {
        self.prop(PropName::Accel, Prop::Str(Rc::from(accel)))
    }
    fn section(self, name: &str, hint: DisplayHint) -> Self {
        self.prop(PropName::Section, Prop::Str(Rc::from(name)))
            .prop(PropName::DisplayHint, Prop::Enum(hint.to_u16()))
    }
    fn submenu(self, name: &str) -> Self {
        self.prop(PropName::Submenu, Prop::Str(Rc::from(name)))
    }
}

/// `Kind::PopoverMenu`'s controller.
pub struct PopoverMenuC {
    /// The embedded `Popover` chrome -- see the module doc.
    pub popover: PopoverC,
    /// One `button.model` node per item, in order.
    pub items: Vec<Node>,
    /// Keyboard cursor into `items`.
    pub cursor: Option<usize>,
    /// A submenu popup this menu opened; `Left` closes it.
    pub submenu: Option<PopupKey>,
    /// `(section name, its box)`, in the order sections were built. This
    /// controller's flat prop shape only ever builds at most one.
    pub sections: Vec<(Rc<str>, Node)>,
    /// The items' raw labels, mnemonic markup intact.
    labels: Vec<Rc<str>>,
    section_name: Rc<str>,
    hint: DisplayHint,
}

impl PopoverMenuC {
    /// The item a mnemonic character selects: the first item whose label
    /// carries `_<ch>`, ASCII-case-insensitively.
    ///
    /// Inside a `PopoverMenu` this fires *without* `Alt`
    /// (`gtk/gtkpopovermenu.c`), which is why it is a method here and not a
    /// window-level shortcut.
    #[must_use]
    pub fn mnemonic_target(&self, ch: char) -> Option<usize> {
        let want = ch.to_ascii_lowercase();
        self.labels.iter().position(|label| {
            let mut chars = label.chars().peekable();
            while let Some(c) = chars.next() {
                if c == '_'
                    && let Some(next) = chars.peek()
                    && next.to_ascii_lowercase() == want
                {
                    return true;
                }
            }
            false
        })
    }

    /// Test hook: [`PopoverMenuC::mnemonic_target`] through a `&dyn
    /// Controller<Msg>`, the shape this crate's other test hooks
    /// (`ListBoxC::selected_rows`) already use.
    #[must_use]
    pub fn mnemonic_of<Msg: Clone + 'static>(c: &dyn Controller<Msg>, ch: char) -> Option<usize> {
        let any: &dyn std::any::Any = c;
        any.downcast_ref::<Self>()
            .and_then(|me| me.mnemonic_target(ch))
    }

    /// The extra CSS class `hint` contributes to its section's `box`, or
    /// `None` for `Normal`.
    fn hint_class(hint: DisplayHint) -> Option<&'static str> {
        match hint {
            DisplayHint::Normal => None,
            DisplayHint::InlineButtons => Some("inline-buttons"),
            DisplayHint::Circular => Some("circular"),
            DisplayHint::HorizontalButtons => Some("horizontal-buttons"),
        }
    }

    /// Rebuild `items`/`sections` from `labels`/`section_name`/`hint`,
    /// detaching whatever was there before. Called once from `build` and
    /// again from `set_prop` on every `Menus`/`Section`/`DisplayHint`
    /// change (including the harmless re-application `build_controller`'s
    /// post-`build` prop sweep makes with the same values `build` already
    /// consumed).
    fn rebuild_items(&mut self) {
        for (_, section_box) in self.sections.drain(..) {
            section_box.detach();
        }
        self.items.clear();
        if self.labels.is_empty() {
            return;
        }
        let orientation = if self.hint == DisplayHint::Normal {
            "vertical"
        } else {
            "horizontal"
        };
        let mut classes = vec![orientation];
        if let Some(extra) = Self::hint_class(self.hint) {
            classes.push(extra);
        }
        let section_box = Node::with_classes("box", &classes);
        self.popover.contents.append_child(&section_box);
        for _ in &self.labels {
            let item = Node::with_classes("button", &["model"]);
            item.append_child(&Node::new("label"));
            section_box.append_child(&item);
            self.items.push(item);
        }
        self.sections
            .push((Rc::clone(&self.section_name), section_box));
    }

    /// The item a point (in this controller's own root-node space) lands in.
    fn item_at(&self, tree: &LayoutTree, root: &Node, point: (f32, f32)) -> Option<usize> {
        self.items.iter().position(|item| {
            local_rect(tree, root, item).is_some_and(|r| {
                point.0 >= r.x && point.0 < r.right() && point.1 >= r.y && point.1 < r.bottom()
            })
        })
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for PopoverMenuC {
    fn kind(&self) -> Kind {
        Kind::PopoverMenu
    }

    fn build(node: &Node, props: &Props, cx: &mut BuildCx<'_>) -> Self {
        let popover = <PopoverC as Controller<Msg>>::build(node, props, cx);
        let labels: Vec<Rc<str>> = match props.get(PropName::Menus) {
            Some(Prop::Classes(list)) => list.to_vec(),
            _ => Vec::new(),
        };
        let hint = match props.get(PropName::DisplayHint) {
            Some(Prop::Enum(raw)) => DisplayHint::from_u16(*raw),
            _ => DisplayHint::default(),
        };
        let section_name: Rc<str> = props
            .str(PropName::Section)
            .map_or_else(|| Rc::from(""), Rc::from);
        let mut this = Self {
            popover,
            items: Vec::new(),
            cursor: None,
            submenu: None,
            sections: Vec::new(),
            labels,
            section_name,
            hint,
        };
        this.rebuild_items();
        this
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, cx: &mut BuildCx<'_>) {
        match (name, value) {
            (PropName::Menus, Prop::Classes(list)) => {
                self.labels = list.to_vec();
                self.rebuild_items();
            }
            (PropName::Section, Prop::Str(s)) => {
                self.section_name = Rc::clone(s);
                self.rebuild_items();
            }
            (PropName::DisplayHint, Prop::Enum(raw)) => {
                self.hint = DisplayHint::from_u16(*raw);
                self.rebuild_items();
            }
            _ => <PopoverC as Controller<Msg>>::set_prop(&mut self.popover, node, name, value, cx),
        }
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        use xkbcommon::xkb::keysyms;

        match ev {
            Event::PopupDone => {
                self.popover.open = false;
                self.popover.popup = None;
                self.submenu = None;
                Vec::new()
            }
            Event::Key(key) if key.pressed => {
                let mnemonic = char::from_u32(xkbcommon::xkb::keysym_to_utf32(key.keysym))
                    .filter(|c| !c.is_control());
                if let Some(index) = mnemonic.and_then(|ch| self.mnemonic_target(ch)) {
                    cx.handled = true;
                    let msg = cx.handlers.fire_index(EventKind::Activate, index);
                    self.popover.close(cx);
                    return msg.into_iter().collect();
                }
                let count = self.items.len();
                match u32::from(key.keysym) {
                    keysyms::KEY_Down => {
                        self.cursor = Some(
                            self.cursor
                                .map_or(0, |c| (c + 1).min(count.saturating_sub(1))),
                        );
                        cx.handled = true;
                    }
                    keysyms::KEY_Up => {
                        self.cursor = Some(self.cursor.map_or(0, |c| c.saturating_sub(1)));
                        cx.handled = true;
                    }
                    keysyms::KEY_Return | keysyms::KEY_KP_Enter | keysyms::KEY_space => {
                        if let Some(cursor) = self.cursor {
                            cx.handled = true;
                            let msg = cx.handlers.fire_index(EventKind::Activate, cursor);
                            self.popover.close(cx);
                            return msg.into_iter().collect();
                        }
                    }
                    // No per-item submenu is tracked in this flat prop shape
                    // (module doc); `Left` still closes one if `submenu` was
                    // set directly.
                    keysyms::KEY_Left => {
                        if let Some(key) = self.submenu.take() {
                            cx.cmds.push(Cmd::ClosePopup(key));
                            cx.handled = true;
                        }
                    }
                    _ => {}
                }
                Vec::new()
            }
            Event::PointerUp { button, local, .. } if *button == crate::window::layer::BTN_LEFT => {
                if let Some(index) = self.item_at(cx.tree, cx.node, *local) {
                    cx.handled = true;
                    let msg = cx.handlers.fire_index(EventKind::Activate, index);
                    self.popover.close(cx);
                    return msg.into_iter().collect();
                }
                Vec::new()
            }
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::view::{Event, EventKind, Handler, Kind, Prop, PropName, Props};
    use crate::widgets::popover_menu::PopoverMenuC;
    use crate::widgets::types::DisplayHint;
    use crate::widgets::{Headless, build_widget, matches_fixture};

    fn props() -> Props {
        let mut p = Props::default();
        p.set(
            PropName::Menus,
            Prop::Classes(
                ["_Open", "_Save", "Quit"]
                    .iter()
                    .map(|s| (*s).into())
                    .collect(),
            ),
        );
        p.set(PropName::Section, Prop::Str("main".into()));
        p.set(
            PropName::DisplayHint,
            Prop::Enum(DisplayHint::InlineButtons.to_u16()),
        );
        p
    }

    #[test]
    fn menu_items_are_button_model_under_contents() {
        // Mutation check: naming the items `menuitem` (GTK3's node) fails
        // the fixture and every Adwaita `.menu button.model` rule.
        // `props()` builds exactly three items ("_Open", "_Save", "Quit"),
        // so the fixture lists three `button.model` entries outright rather
        // than leaning on the `┊` repeat marker -- which (per
        // `node_tree::matches_fixture`'s own doc) marks whatever pattern is
        // deepest on the parse stack, i.e. `label` here, not the repeating
        // `button.model` it sits under; three literal entries side-step that
        // instead of fighting it.
        let built = build_widget::<()>(Kind::PopoverMenu, &props());
        let fixture = "popover.background.menu\n\
                        ├── arrow\n\
                        ╰── contents\n\
                        \x20   ╰── [box.horizontal.inline-buttons]\n\
                        \x20       ├── button.model\n\
                        \x20       │   ╰── label\n\
                        \x20       ├── button.model\n\
                        \x20       │   ╰── label\n\
                        \x20       ╰── button.model\n\
                        \x20           ╰── label\n";
        matches_fixture(&built.node, fixture).expect("popover_menu fixture");
    }

    #[test]
    fn a_mnemonic_fires_without_alt_inside_a_menu() {
        // GTK's documented divergence. Mutation check: requiring Alt makes
        // every menu mnemonic dead once the menu is open.
        let built = build_widget::<usize>(Kind::PopoverMenu, &props());
        let mut c = built.controller;
        let mut hx = Headless::new();
        let mut cx = hx.event_cx_with_handlers(&built.node, |h| {
            h.set(EventKind::Activate, Handler::Index(std::rc::Rc::new(|i| i)));
        });
        assert_eq!(
            c.on_event(&Event::Key(Headless::key("s")), &mut cx),
            vec![1]
        );
        assert_eq!(PopoverMenuC::mnemonic_of(c.as_ref(), 'o'), Some(0));
        assert_eq!(PopoverMenuC::mnemonic_of(c.as_ref(), 'z'), None);
    }

    #[test]
    fn a_hostile_item_label_never_panics_the_mnemonic_scan() {
        // Labels come from the application model.
        for label in ["", "_", "__", "_\u{0}", "\u{feff}_x", &"_".repeat(10_000)] {
            let mut p = Props::default();
            p.set(
                PropName::Menus,
                Prop::Classes([label].iter().map(|s| (*s).into()).collect()),
            );
            let built = build_widget::<()>(Kind::PopoverMenu, &p);
            let _ = PopoverMenuC::mnemonic_of(built.controller.as_ref(), '_');
        }
    }
}
