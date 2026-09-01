//! `GtkDropDown` — `Kind::DropDown`, CSS node `dropdown`.
//!
//! ```text
//! dropdown
//! ├── button.toggle
//! │   ╰── <content>
//! │        ╰── [arrow]
//! ╰── popover.menu
//!     ╰── contents
//!         ├── [entry.search]
//!         ╰── listview
//!             ╰── row[.activatable]
//!             ┊
//! ```
//!
//! GTK's own block says only "a single node `dropdown`, with the button and
//! popover nodes as children"; the expansion above is the tree it actually
//! renders, and is what contract §5.2 vendors as the fixture.
//!
//! Contract §5.2 types `DropDownC.list` as `ListViewC`, a P6 kind; this
//! controller builds the `listview` node and its `row` children directly, and
//! P6 replaces the field with its own `ListViewC` when `Kind::ListView`
//! lands.
//!
//! The embedded popover is built with `ShowArrow` forced off: GTK's `.menu`
//! popovers render no arrow of their own (the vendored fixture has none under
//! `popover`), unlike the plain `Popover` a caller builds directly.

use std::rc::Rc;

use crate::css::node::{Node, PseudoStates};
use crate::layout::Rect;
use crate::view::controller::{Controller, Event, EventCx};
use crate::view::{BuildCx, EventKind, Handler, Kind, Prop, PropName, Props, View};
use crate::widgets::popover::PopoverC;
use crate::widgets::{ListItem, MatchMode, PointerState, WidgetEnum, local_rect, shift_event};
use crate::window::popup::PopupAnchorPoint;

/// A `GtkDropDown` over string items.
#[must_use]
pub fn drop_down<Msg: Clone + 'static>(items: &[&str]) -> View<Msg> {
    let model: Vec<ListItem> = items
        .iter()
        .enumerate()
        .map(|(index, label)| ListItem::new(index as u64, label))
        .collect();
    drop_down_from(Rc::from(model))
}

/// A `GtkDropDown` over an explicit model.
#[must_use]
pub fn drop_down_from<Msg: Clone + 'static>(items: Rc<[ListItem]>) -> View<Msg> {
    View::new(Kind::DropDown).prop(PropName::Model, Prop::Items(items))
}

/// `GtkDropDown`'s own setters and signals.
pub trait DropDownExt<Msg>: Sized {
    /// `GtkDropDown:selected`.
    fn selected(self, index: usize) -> Self;
    /// `GtkDropDown:enable-search`.
    fn enable_search(self, on: bool) -> Self;
    /// `GtkDropDown:show-arrow`.
    fn show_arrow(self, on: bool) -> Self;
    /// `GtkDropDown:search-match-mode`.
    fn search_match_mode(self, mode: MatchMode) -> Self;
    /// `GtkDropDown:selected`'s change notification.
    fn on_selected(self, f: impl Fn(usize) -> Msg + 'static) -> Self;
}

impl<Msg: Clone + 'static> DropDownExt<Msg> for View<Msg> {
    fn selected(self, index: usize) -> Self {
        self.prop(PropName::Selected, Prop::Int(index as i64))
    }
    fn enable_search(self, on: bool) -> Self {
        self.prop(PropName::EnableSearch, Prop::Bool(on))
    }
    fn show_arrow(self, on: bool) -> Self {
        self.prop(PropName::ShowArrow, Prop::Bool(on))
    }
    fn search_match_mode(self, mode: MatchMode) -> Self {
        self.prop(PropName::SelectionMode, mode.to_prop())
    }
    fn on_selected(self, f: impl Fn(usize) -> Msg + 'static) -> Self {
        self.on(EventKind::Selected, Handler::Index(Rc::new(f)))
    }
}

/// `Kind::DropDown`'s controller.
pub struct DropDownC {
    /// The model.
    pub items: Rc<[ListItem]>,
    /// The selected index, clamped into `items`.
    pub selected: usize,
    /// Whether the popover is showing.
    pub open: bool,
    /// The search entry's text.
    pub search: String,
    /// Indices of `items` currently visible.
    pub filtered: Vec<usize>,
    /// The `button.toggle` subnode.
    pub button: Node,
    /// The embedded popover.
    pub popover: PopoverC,
    /// The `listview` subnode inside the popover's `contents`.
    pub list: Node,
    /// One `row` node per visible item.
    pub rows: Vec<Node>,
    /// One press-tracking [`PointerState`] per entry of `rows`, parallel to
    /// it. A row click is a press-then-release-inside gesture like any other,
    /// so the press has to survive between the two events: a per-event
    /// scratch `PointerState` always reports `pressed == false` on the
    /// release and would never fire [`EventKind::Selected`] at all.
    row_pointers: Vec<PointerState>,
    /// A dedicated, never-attached node: `DropDown` takes no application
    /// children, and routing `child_slot` here (rather than leaving it
    /// unhandled, which would default to `node` itself) keeps the
    /// reconciler's "no application children" cleanup off `button` and
    /// `popover`, which it would otherwise detach on every reconcile.
    pub sink: Node,
    mode: MatchMode,
    pointer: PointerState,
}

impl DropDownC {
    /// Indices of `items` matching `search` under `mode`.
    ///
    /// An empty needle matches everything. Comparison is ASCII-case-insensitive
    /// on both sides, which is what `GtkStringFilter`'s default does; a needle
    /// longer than every label simply matches nothing.
    #[must_use]
    pub fn filter(items: &[ListItem], search: &str, mode: MatchMode) -> Vec<usize> {
        if search.is_empty() {
            return (0..items.len()).collect();
        }
        let needle = search.to_lowercase();
        items
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                let hay = item.text.to_lowercase();
                match mode {
                    MatchMode::Exact => hay == needle,
                    MatchMode::Prefix => hay.starts_with(&needle),
                    MatchMode::Substring => hay.contains(&needle),
                }
            })
            .map(|(index, _)| index)
            .collect()
    }

    /// Rebuild the row nodes from `filtered`.
    fn rebuild_rows(&mut self) {
        for row in self.rows.drain(..) {
            row.detach();
        }
        for &index in &self.filtered {
            let row = Node::with_classes("row", &["activatable"]);
            row.set_state(PseudoStates::SELECTED, index == self.selected);
            self.list.append_child(&row);
            self.rows.push(row);
        }
        // The row nodes themselves are new, so their press state is too.
        self.row_pointers.clear();
        self.row_pointers
            .extend(self.rows.iter().map(|_| PointerState::default()));
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for DropDownC {
    fn kind(&self) -> Kind {
        Kind::DropDown
    }

    fn build(node: &Node, props: &Props, cx: &mut BuildCx<'_>) -> Self {
        let button = Node::with_classes("button", &["toggle"]);
        node.append_child(&button);
        button.append_child(&Node::new("label"));
        // `GtkDropDown:show-arrow` defaults to `TRUE`.
        if props.bool(PropName::ShowArrow, true) {
            button.append_child(&Node::with_classes("arrow", &["down"]));
        }
        let popover_node = Node::with_classes("popover", &["background", "menu"]);
        node.append_child(&popover_node);
        let mut popover_props = Props::default();
        popover_props.set(PropName::ShowArrow, Prop::Bool(false));
        let popover = <PopoverC as Controller<Msg>>::build(&popover_node, &popover_props, cx);
        if props.bool(PropName::EnableSearch, false) {
            popover
                .contents
                .append_child(&Node::with_classes("entry", &["search"]));
        }
        let list = Node::new("listview");
        popover.contents.append_child(&list);

        let items: Rc<[ListItem]> = match props.get(PropName::Model) {
            Some(Prop::Items(items)) => Rc::clone(items),
            _ => Rc::from(Vec::new()),
        };
        let selected = usize::try_from(props.int(PropName::Selected, 0))
            .unwrap_or(0)
            .min(items.len().saturating_sub(1));
        let mode = MatchMode::from_prop(props.get(PropName::SelectionMode), MatchMode::Substring);
        let mut this = DropDownC {
            filtered: Self::filter(&items, "", mode),
            items,
            selected,
            open: false,
            search: String::new(),
            button,
            popover,
            list,
            rows: Vec::new(),
            row_pointers: Vec::new(),
            sink: Node::new("sink"),
            mode,
            pointer: PointerState::default(),
        };
        this.rebuild_rows();
        this
    }

    fn set_prop(&mut self, _node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        match (name, value) {
            (PropName::Model, Prop::Items(items)) => {
                self.items = Rc::clone(items);
                self.selected = self.selected.min(self.items.len().saturating_sub(1));
                self.filtered = Self::filter(&self.items, &self.search, self.mode);
            }
            (PropName::Selected, Prop::Int(index)) => {
                self.selected = usize::try_from(*index)
                    .unwrap_or(0)
                    .min(self.items.len().saturating_sub(1));
            }
            _ => return,
        }
        self.rebuild_rows();
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        if matches!(ev, Event::PopupDone) {
            self.open = false;
            self.button.set_state(PseudoStates::CHECKED, false);
            return Vec::new();
        }
        // Same widget-level focus forwarding `MenuButtonC` does: taking a
        // controller of one's own must not silently drop `on_focus_in` /
        // `on_focus_out`, which `GenericC` fires for every fallback kind.
        match ev {
            Event::FocusIn { .. } => {
                return cx
                    .handlers
                    .fire_unit(EventKind::FocusIn)
                    .into_iter()
                    .collect();
            }
            Event::FocusOut => {
                return cx
                    .handlers
                    .fire_unit(EventKind::FocusOut)
                    .into_iter()
                    .collect();
            }
            _ => {}
        }
        // A row click selects and closes. Every row folds the event into its
        // own retained `PointerState` (hover and `:active` track the pointer
        // even for the rows the gesture does not complete on), and the first
        // row whose press-release gesture completes wins.
        let mut picked = None;
        for position in 0..self.rows.len() {
            let row = self.rows[position].clone();
            let Some(rect) = local_rect(cx.tree, cx.node, &row) else {
                continue;
            };
            let shifted = shift_event(ev, rect);
            let Some(state) = self.row_pointers.get_mut(position) else {
                continue;
            };
            if state.observe(
                &row,
                &shifted,
                Some(Rect::new(0.0, 0.0, rect.width, rect.height)),
            ) && picked.is_none()
            {
                picked = Some(position);
            }
        }
        if let Some(position) = picked {
            let index = self
                .filtered
                .get(position)
                .copied()
                .unwrap_or(self.selected);
            self.selected = index;
            self.open = false;
            self.button.set_state(PseudoStates::CHECKED, false);
            self.popover.close(cx);
            self.rebuild_rows();
            cx.handled = true;
            return cx
                .handlers
                .fire_index(EventKind::Selected, index)
                .map_or_else(Vec::new, |m| vec![m]);
        }
        let Some(rect) = local_rect(cx.tree, cx.node, &self.button) else {
            return Vec::new();
        };
        let shifted = shift_event(ev, rect);
        if self.pointer.observe(
            &self.button,
            &shifted,
            Some(Rect::new(0.0, 0.0, rect.width, rect.height)),
        ) {
            cx.handled = true;
            if self.open {
                self.popover.close(cx);
                self.open = false;
            } else {
                self.popover.open(
                    PopupAnchorPoint::Node(self.button.clone()),
                    (rect.width.max(1.0) as u32, 240),
                    cx,
                );
                self.open = true;
            }
            self.button.set_state(PseudoStates::CHECKED, self.open);
        }
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::time::Duration;

    use super::DropDownC;
    use crate::anim::{Clock, ManualClock};
    use crate::css::cascade::CompiledSheet;
    use crate::css::computed::ResolveEnv;
    use crate::css::node::{Node, PseudoStates};
    use crate::layout::{Container, FixedMeasure, LayoutTree};
    use crate::view::controller::{Controller, Event, EventCx, Phase};
    use crate::view::reconcile::BuildCx;
    use crate::view::render::{Animations, StyleMap, layout_tree, node_addr, restyle_tree};
    use crate::view::{Cmd, EventKind, Handler, Handlers, ListItem, Prop, PropName, Props};
    use crate::window::focus::FocusRing;
    use crate::window::selection::Clipboard;
    use std::rc::Rc;

    /// Drives `DropDownC::on_event` over a really laid-out tree — the real
    /// allocations `local_rect` consults — so the open → click-a-row →
    /// `Selected` path is exercised end to end without the whole `App`
    /// (whose `route`/`aim` cannot currently reach a nested `button.toggle`;
    /// see the ignored `widget_pixels` companion test).
    #[test]
    fn opening_a_drop_down_and_clicking_a_row_selects_that_item() {
        // mutation: give the row hit-test a fresh `PointerState` per event
        // (or never fire `EventKind::Selected`) and the click produces no
        // message and leaves `selected` at 0.
        let sheet = CompiledSheet::compile("dropdown { color: #000000; }");
        let mut fonts = crate::text::FontDatabase::probe_only();
        let mut icons = crate::icons::IconTheme::with_name_and_roots("hicolor", vec![]);
        let clock: Rc<dyn Clock> = Rc::new(ManualClock::new());
        let env = ResolveEnv::default();

        let node = Node::new("dropdown");
        let items: Rc<[ListItem]> = Rc::from(vec![
            ListItem::new(0, "One"),
            ListItem::new(1, "Two"),
            ListItem::new(2, "Three"),
        ]);
        let mut props = Props::default();
        props.set(PropName::Model, Prop::Items(Rc::clone(&items)));
        let mut controller = {
            let mut cx = BuildCx {
                sheet: &sheet,
                fonts: &mut fonts,
                icons: &mut icons,
                clock: &clock,
                env: &env,
            };
            <DropDownC as Controller<usize>>::build(&node, &props, &mut cx)
        };
        assert_eq!(controller.rows.len(), 3, "one row per item");

        // Lay the retained tree out for real: every leaf is 40x20, and each
        // container stacks its children on the main axis, so the button and
        // the three rows all land on disjoint rectangles.
        let mut styles = StyleMap::new();
        let mut anims = Animations::new();
        restyle_tree(&node, &sheet, &env, &mut styles, &mut anims, Duration::ZERO);
        let mut containers: HashMap<_, Container> = HashMap::new();
        for addr in styles.keys() {
            containers.insert(
                *addr,
                Container::Box {
                    direction: crate::layout::BoxDirection::Column,
                },
            );
        }
        containers.insert(
            node_addr(&node),
            Container::Box {
                direction: crate::layout::BoxDirection::Column,
            },
        );
        let mut tree = LayoutTree::new();
        let mut measure = FixedMeasure(taffy::Size {
            width: 40.0,
            height: 20.0,
        });
        layout_tree(
            &node,
            &styles,
            &containers,
            &mut tree,
            &env,
            (Some(400.0), Some(400.0)),
            &mut measure,
        )
        .expect("the dropdown lays out");

        let root = tree.allocation(&node).expect("root allocation").border_box;
        let centre = |sub: &Node| {
            let b = tree.allocation(sub).expect("subnode allocation").border_box;
            assert!(b.width > 0.0 && b.height > 0.0, "a clickable box");
            (b.x - root.x + b.width / 2.0, b.y - root.y + b.height / 2.0)
        };
        let button_at = centre(&controller.button);
        let row_at = centre(&controller.rows[1]);
        assert!(
            (button_at.1 - row_at.1).abs() > f32::EPSILON,
            "the button and the row are separate targets"
        );

        let mut handlers: Handlers<usize> = Handlers::default();
        handlers.set(EventKind::Selected, Handler::Index(Rc::new(|index| index)));
        let mut focus = <FocusRing as Default>::default();
        let mut clipboard = Clipboard::offscreen();
        let mut cmds: Vec<Cmd<usize>> = Vec::new();

        let mut click = |controller: &mut DropDownC,
                         at: (f32, f32),
                         cmds: &mut Vec<Cmd<usize>>,
                         fonts: &mut crate::text::FontDatabase,
                         icons: &mut crate::icons::IconTheme|
         -> Vec<usize> {
            let mut out = Vec::new();
            for ev in [
                Event::PointerDown {
                    button: 0x110,
                    local: at,
                    serial: 1,
                },
                Event::PointerUp {
                    button: 0x110,
                    local: at,
                    serial: 2,
                },
            ] {
                let mut ecx = EventCx {
                    node: &node,
                    handlers: &handlers,
                    tree: &tree,
                    styles: &styles,
                    focus: &mut focus,
                    clipboard: &mut clipboard,
                    icons,
                    fonts,
                    clock: &clock,
                    env: &env,
                    cmds,
                    phase: Phase::Target,
                    handled: false,
                };
                out.extend(Controller::<usize>::on_event(controller, &ev, &mut ecx));
            }
            out
        };

        // 1. Clicking the button opens the popover and checks the toggle.
        let opened = click(
            &mut controller,
            button_at,
            &mut cmds,
            &mut fonts,
            &mut icons,
        );
        assert!(opened.is_empty(), "opening emits no application message");
        assert!(controller.open, "the popover is open");
        assert!(controller.button.states().contains(PseudoStates::CHECKED));

        // 2. Clicking a row selects it, closes the popover and repaints the
        //    button's own state.
        let picked = click(&mut controller, row_at, &mut cmds, &mut fonts, &mut icons);
        assert_eq!(picked, vec![1], "the row's index reaches the application");
        assert_eq!(controller.selected, 1);
        assert!(!controller.open, "picking a row closes the popover");
        assert!(!controller.button.states().contains(PseudoStates::CHECKED));
        assert!(
            controller.rows[1].states().contains(PseudoStates::SELECTED),
            "the selection moved to the clicked row"
        );
        assert!(!controller.rows[0].states().contains(PseudoStates::SELECTED));

        // The open leg asked the compositor for a popup; the close leg took
        // it back down.
        assert!(
            !cmds.is_empty(),
            "opening and closing both go through `cx.cmds`"
        );
    }
}
