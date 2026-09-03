//! `GtkPopover` — `Kind::Popover`, CSS node `popover`, always `.background`.
//!
//! ```text
//! popover.background[.menu]
//! ├── arrow
//! ╰── contents
//!     ╰── <child>
//! ```
//!
//! Contract ruling R4 makes this a P5 deliverable although the spec files
//! popovers under P6: `MenuButton`, `DropDown`, `ColorDialogButton` and
//! `FontDialogButton` all embed one. P6 builds `PopoverMenu` and
//! `PopoverMenuBar` on top.
//!
//! An **autohide** popover (GTK's "modal") is a real `Surface::Popup` taking
//! `xdg_popup.grab`; a non-autohide one renders inside the parent window's own
//! tree with no grab.

use std::rc::Rc;

use crate::css::node::Node;
use crate::layout::Rect;
use crate::view::cmd::Cmd;
use crate::view::controller::{Controller, Event, EventCx};
use crate::view::{BuildCx, EventKind, Handler, Kind, Prop, PropName, Props, View};
use crate::widgets::{Position, WidgetEnum};
use crate::window::popup::{
    Anchor, ConstraintAdjustment, Gravity, PopupAnchorPoint, PopupKey, Positioner,
};

/// A `GtkPopover` wrapping `child`.
#[must_use]
pub fn popover<Msg: Clone + 'static>(child: View<Msg>) -> View<Msg> {
    View::new(Kind::Popover).child(child)
}

/// `GtkPopover`'s own setters.
pub trait PopoverExt<Msg>: Sized {
    /// `GtkPopover:autohide` — GTK's "modal"; takes the popup grab.
    fn autohide(self, on: bool) -> Self;
    /// `GtkPopover:has-arrow`.
    fn has_arrow(self, on: bool) -> Self;
    /// `GtkPopover:position`.
    fn position(self, pos: Position) -> Self;
    /// `gtk_popover_set_offset`.
    fn offset(self, dx: i32, dy: i32) -> Self;
    /// `GtkPopover:pointing-to`, in the parent window's frame space.
    fn pointing_to(self, rect: Rect) -> Self;
    /// `GtkPopover::closed`.
    fn on_close(self, msg: Msg) -> Self;
}

impl<Msg: Clone + 'static> PopoverExt<Msg> for View<Msg> {
    fn autohide(self, on: bool) -> Self {
        self.prop(PropName::Autohide, Prop::Bool(on))
    }
    fn has_arrow(self, on: bool) -> Self {
        self.prop(PropName::ShowArrow, Prop::Bool(on))
    }
    fn position(self, pos: Position) -> Self {
        self.prop(PropName::Position, pos.to_prop())
    }
    fn offset(self, dx: i32, dy: i32) -> Self {
        self.prop(PropName::Offset, Prop::Edges([dx, dy, 0, 0]))
    }
    fn pointing_to(self, rect: Rect) -> Self {
        self.prop(
            PropName::Anchor,
            Prop::Edges([
                rect.x as i32,
                rect.y as i32,
                rect.width as i32,
                rect.height as i32,
            ]),
        )
    }
    fn on_close(self, msg: Msg) -> Self {
        self.on(EventKind::Close, Handler::Unit(msg))
    }
}

/// `Kind::Popover`'s controller. Embedded by four other P5 widgets.
pub struct PopoverC {
    /// Whether the popover is showing.
    pub open: bool,
    /// GTK's "modal": takes the popup grab.
    pub autohide: bool,
    /// Draw the `arrow` node.
    pub has_arrow: bool,
    /// Which side of the anchor the popover sits on.
    pub position: Position,
    /// The compositor-side popup, while open and autohiding.
    pub popup: Option<PopupKey>,
    /// The `arrow` subnode.
    pub arrow: Node,
    /// The `contents` subnode; every child goes under it.
    pub contents: Node,
    /// The `popover` node itself, so opening and closing can show and hide
    /// it. `PopoverC` is embedded by four other widgets that hand it a node
    /// they built; without keeping it, nothing in this crate could ever flip
    /// a popover's own visibility, which is P8-D71's drop-down half.
    pub root: Node,
    /// Whether closing this popover hides its node.
    ///
    /// Opt-in, through [`PopoverC::hide_when_closed`], and not the default:
    /// `PopoverMenuBar` sizes its `item`s from their menu child and its own
    /// hover-to-switch test reads those boxes, so hiding a menu bar's menus
    /// is its own change. `DropDown` — P8-D71's half — opts in.
    hides: bool,
    offset: (i32, i32),
}

impl PopoverC {
    /// Build the two subnodes under a fresh detached root — the constructor
    /// the positioner unit test uses.
    ///
    /// **Detached is the whole of it: this root is a local, and a caller that
    /// keeps only the `PopoverC` drops the `popover` node on the floor.** It
    /// therefore has no production caller and must not gain one; an embedding
    /// widget appends a `popover` node to its own and calls
    /// `<PopoverC as Controller<Msg>>::build` on that (`DropDownC::build`,
    /// `MenuButtonC::build`). This doc used to say the opposite, and
    /// `MenuButtonC` believed it: its menu body was reconciled into a subtree
    /// nothing was attached to, so it had no allocation in any state and could
    /// not be laid out, painted or hit (contract §10 P8-D72).
    ///
    /// **Test scaffolding only, and `pub` under protest.** It stays public
    /// (rather than `pub(crate)` or `#[cfg(test)]`) purely because the
    /// integration tests under `ui/tests/` are separate crates and cannot see
    /// a crate-private constructor; it is `#[doc(hidden)]` so it never shows
    /// up as part of the widget API, and any production use is a bug.
    #[doc(hidden)]
    #[must_use]
    pub fn for_test(position: Position, autohide: bool, has_arrow: bool) -> Self {
        let root = Node::with_classes("popover", &["background"]);
        let arrow = Node::new("arrow");
        root.append_child(&arrow);
        let contents = Node::new("contents");
        root.append_child(&contents);
        PopoverC {
            open: false,
            autohide,
            has_arrow,
            position,
            popup: None,
            arrow,
            contents,
            root,
            hides: false,
            offset: (0, 0),
        }
    }

    /// Make closing this popover hide its node, and close it now.
    ///
    /// GTK's `visible`: an invisible widget keeps its state but takes no
    /// space, receives no events and paints nothing. Hidden rather than
    /// detached, because GTK's own node trees — and this crate's vendored
    /// §5.2 fixtures with them — list a `dropdown`'s `popover` whether it is
    /// showing or not.
    pub fn hide_when_closed(&mut self) {
        self.hides = true;
        self.reveal(false);
    }

    /// Show or hide the popover's own node — see
    /// [`PopoverC::hide_when_closed`], which is the only thing that arms it.
    pub fn reveal(&self, on: bool) {
        if self.hides {
            crate::widgets::set_displayed(&self.root, on);
        }
    }

    /// The positioner this popover would use for `anchor_rect` at `size`.
    ///
    /// `position` names the side of the anchor the popover sits on, so both the
    /// anchor edge and the gravity take that side: a `Bottom` popover hangs off
    /// the anchor's bottom edge, growing downward.
    #[must_use]
    pub fn positioner(&self, anchor_rect: Rect, size: (u32, u32)) -> Positioner {
        let (anchor, gravity) = match self.position {
            Position::Top => (Anchor::Top, Gravity::Top),
            Position::Bottom => (Anchor::Bottom, Gravity::Bottom),
            Position::Left => (Anchor::Left, Gravity::Left),
            Position::Right => (Anchor::Right, Gravity::Right),
        };
        Positioner {
            anchor_rect,
            size,
            anchor,
            gravity,
            constraint: ConstraintAdjustment::FlipX
                | ConstraintAdjustment::FlipY
                | ConstraintAdjustment::SlideX
                | ConstraintAdjustment::SlideY,
            offset: self.offset,
            reactive: true,
        }
    }

    /// Open against `anchor`, showing `content` on the popup surface.
    ///
    /// `content` is [`crate::view::cmd::Cmd::OpenPopup`]'s payload: the view
    /// the compositor-side surface is built from. `None` means *this
    /// popover's content is already retained in the parent window's own
    /// tree* — which is the case for every popover this crate embeds
    /// (`MenuButton`, `DropDown`, `PopoverMenu`, `ColorDialogButton`,
    /// `FontDialogButton` all append their contents under the widget's own
    /// node) — and then no compositor surface is asked for at all. That is
    /// the point of the parameter: passing a stub payload opened a *blank*
    /// popup over content that was already on screen (contract §10 P6-D39,
    /// limit 1), and opening nothing is both correct and cheaper.
    ///
    /// A caller whose content is not in the parent tree — an application
    /// building a popover's body as a `View` — passes `Some`, and gets a real
    /// `xdg_popup` with that view reconciled into its own surface.
    pub fn open<Msg: Clone + 'static>(
        &mut self,
        anchor: PopupAnchorPoint,
        size: (u32, u32),
        content: Option<Rc<dyn Fn() -> View<Msg>>>,
        cx: &mut EventCx<'_, Msg>,
    ) {
        if self.open {
            return;
        }
        self.open = true;
        // Whichever way the body is carried, opening has to make it visible.
        // Before P8-D71's close-out the `None` branch below returned here
        // having done nothing at all — neither a surface nor a reveal — so a
        // `DropDown`'s list was laid out and painted whether the popover was
        // open or shut, and "open" was a bool no pixel could see.
        self.reveal(true);
        let Some(content) = content else {
            // Nothing to build a surface from: the body is in the parent
            // window's tree and is painted there (P7-D54), and the reveal
            // above is the whole of opening it.
            return;
        };
        if !self.autohide {
            // A non-autohide popover renders in the parent window; nothing to
            // ask the compositor for.
            return;
        }
        let anchor_rect = match &anchor {
            PopupAnchorPoint::Rect(rect) => *rect,
            PopupAnchorPoint::Node(node) => cx
                .tree
                .allocation(node)
                .map_or(Rect::zero(), |a| a.border_box),
        };
        cx.cmds.push(Cmd::OpenPopup {
            anchor,
            positioner: self.positioner(anchor_rect, size),
            view: content,
        });
    }

    /// Close, dropping the popup if one was taken.
    pub fn close<Msg: Clone + 'static>(&mut self, cx: &mut EventCx<'_, Msg>) {
        self.open = false;
        self.reveal(false);
        if let Some(key) = self.popup.take() {
            cx.cmds.push(Cmd::ClosePopup(key));
        }
    }

    fn apply(&self, node: &Node) {
        node.add_class("background");
        if self.has_arrow {
            if self.arrow.parent().is_none() {
                node.insert_child(0, &self.arrow);
            }
        } else {
            self.arrow.detach();
        }
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for PopoverC {
    fn kind(&self) -> Kind {
        Kind::Popover
    }

    fn build(node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        let arrow = Node::new("arrow");
        node.append_child(&arrow);
        let contents = Node::new("contents");
        node.append_child(&contents);
        let offset = match props.get(PropName::Offset) {
            Some(Prop::Edges([dx, dy, _, _])) => (*dx, *dy),
            _ => (0, 0),
        };
        let this = PopoverC {
            open: false,
            autohide: props.bool(PropName::Autohide, true),
            has_arrow: props.bool(PropName::ShowArrow, true),
            position: Position::from_prop(props.get(PropName::Position), Position::Bottom),
            popup: None,
            arrow,
            contents,
            root: node.clone(),
            hides: false,
            offset,
        };
        this.apply(node);
        this
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        match (name, value) {
            (PropName::Autohide, Prop::Bool(on)) => self.autohide = *on,
            (PropName::ShowArrow, Prop::Bool(on)) => self.has_arrow = *on,
            (PropName::Position, Prop::Enum(_)) => {
                self.position = Position::from_prop(Some(value), self.position);
            }
            (PropName::Offset, Prop::Edges([dx, dy, _, _])) => self.offset = (*dx, *dy),
            _ => return,
        }
        self.apply(node);
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        match ev {
            // The compositor dismissed the chain — `xdg_popup.popup_done`.
            Event::PopupDone => {
                self.open = false;
                self.popup = None;
                self.reveal(false);
                cx.handlers
                    .fire_unit(EventKind::Close)
                    .map_or_else(Vec::new, |msg| vec![msg])
            }
            _ => Vec::new(),
        }
    }
}
