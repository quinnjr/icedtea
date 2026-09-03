//! `GtkWindowControls` — `Kind::WindowControls`, CSS node `windowcontrols`.
//!
//! ```text
//! windowcontrols[.start][.end][.empty][.native]
//! ├── [image.icon]
//! ├── [button.minimize]
//! ├── [button.maximize]
//! ╰── [button.close]
//! ```
//!
//! [`WindowControlsC::tokens`] is `update_window_buttons`
//! (`gtk/gtkwindowcontrols.c:302-424`) transcribed: the `side` picks the half
//! of `gtk-decoration-layout` before or after the colon, the half is split on
//! `,` and walked **in order**, and `menu` produces no child in 4.22.4. All
//! three buttons are `focusable = false` and never enter the Tab ring; their
//! actions go to `xdg_toplevel` through `Cmd`, not to a GTK window.

use std::rc::Rc;

use crate::css::node::Node;
use crate::layout::Rect;
use crate::view::cmd::Cmd;
use crate::view::controller::{Controller, Event, EventCx};
use crate::view::{BuildCx, Kind, Prop, PropName, Props, View};
use crate::widgets::{PointerState, Side, WidgetEnum, WindowButton, local_rect, shift_event};
use crate::window::SurfaceStates;

/// GTK's own default, `gtk/gtksettings.c:854-856`.
pub const DEFAULT_DECORATION_LAYOUT: &str = "menu:minimize,maximize,close";

/// A `GtkWindowControls` rendering one `side` of the decoration layout.
#[must_use]
pub fn window_controls<Msg: Clone + 'static>(side: Side) -> View<Msg> {
    View::new(Kind::WindowControls).prop(PropName::Side, side.to_prop())
}

/// `GtkWindowControls:decoration-layout`.
pub trait WindowControlsExt<Msg>: Sized {
    /// Override `gtk-decoration-layout` for this widget.
    fn decoration_layout(self, layout: &str) -> Self;
}

impl<Msg: Clone + 'static> WindowControlsExt<Msg> for View<Msg> {
    fn decoration_layout(self, layout: &str) -> Self {
        self.prop(PropName::Decoration, Prop::Str(Rc::from(layout)))
    }
}

/// `Kind::WindowControls`'s controller.
pub struct WindowControlsC {
    /// Which half of the layout this widget renders.
    pub side: Side,
    /// The layout string in force.
    pub layout: Rc<str>,
    /// The emitted children, in layout order.
    pub buttons: Vec<(WindowButton, Node)>,
    /// `true` when nothing was emitted, mirroring `GtkWindowControls:empty`.
    pub empty: bool,
    maximized: bool,
    pointer: PointerState,
}

impl WindowControlsC {
    /// Test hook: the button names this instance actually rendered, in
    /// layout order -- `HeaderBar`'s own tests use it to check that each
    /// side got only its own half of `gtk-decoration-layout`.
    #[must_use]
    pub fn button_names(&self) -> Vec<String> {
        self.buttons
            .iter()
            .map(|(button, _)| button.css_class().to_string())
            .collect()
    }

    /// The tokens `side` contributes, in order.
    ///
    /// `menu` is recognised by the setting's documentation but
    /// `update_window_buttons` has no branch for it in 4.22.4, so it produces
    /// nothing. Unknown tokens are dropped.
    #[must_use]
    pub fn tokens(layout: &str, side: Side) -> Vec<WindowButton> {
        let (start, end) = layout.split_once(':').unwrap_or((layout, ""));
        let half = match side {
            Side::Start => start,
            Side::End => end,
        };
        half.split(',')
            .filter_map(|token| match token.trim() {
                "icon" => Some(WindowButton::Icon),
                "minimize" => Some(WindowButton::Minimize),
                "maximize" => Some(WindowButton::Maximize),
                "close" => Some(WindowButton::Close),
                // `menu` and anything unrecognised emit no child.
                _ => None,
            })
            .collect()
    }

    /// Rebuild the children from `layout`, `side` and the maximized state.
    fn rebuild(&mut self, node: &Node) {
        for (_, child) in self.buttons.drain(..) {
            child.detach();
        }
        for token in Self::tokens(&self.layout, self.side) {
            let child = match token {
                WindowButton::Icon => Node::with_classes("image", &["icon"]),
                WindowButton::Minimize => Node::with_classes("button", &["minimize"]),
                WindowButton::Maximize => Node::with_classes("button", &["maximize"]),
                WindowButton::Close => Node::with_classes("button", &["close"]),
            };
            node.append_child(&child);
            self.buttons.push((token, child));
        }
        self.empty = self.buttons.is_empty();
        for candidate in Side::all() {
            node.remove_class(candidate.css_class());
        }
        node.add_class(self.side.css_class());
        node.remove_class("empty");
        if self.empty {
            node.add_class("empty");
        }
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for WindowControlsC {
    fn kind(&self) -> Kind {
        Kind::WindowControls
    }

    fn build(node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        let mut this = WindowControlsC {
            side: Side::from_prop(props.get(PropName::Side), Side::Start),
            layout: props
                .str(PropName::Decoration)
                .map_or_else(|| Rc::from(DEFAULT_DECORATION_LAYOUT), Rc::from),
            buttons: Vec::new(),
            empty: true,
            maximized: false,
            pointer: PointerState::default(),
        };
        this.rebuild(node);
        this
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        match (name, value) {
            (PropName::Decoration, Prop::Str(layout)) => self.layout = Rc::clone(layout),
            (PropName::Side, Prop::Enum(_)) => {
                self.side = Side::from_prop(Some(value), self.side);
            }
            _ => return,
        }
        self.rebuild(node);
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        // A `Configure` that flipped MAXIMIZED changes the maximize button's
        // icon, so the children are rebuilt — GTK does the same on
        // `notify::maximized`.
        if let Event::Configure { states, .. } = ev {
            let maximized = states.contains(SurfaceStates::MAXIMIZED);
            if maximized != self.maximized {
                self.maximized = maximized;
                self.rebuild(cx.node);
            }
            return Vec::new();
        }
        for (token, child) in &self.buttons {
            let Some(rect) = local_rect(cx.tree, cx.node, child) else {
                continue;
            };
            let shifted = shift_event(ev, rect);
            if self.pointer.observe(
                child,
                &shifted,
                Some(Rect::new(0.0, 0.0, rect.width, rect.height)),
            ) {
                cx.handled = true;
                match token {
                    WindowButton::Minimize => cx.cmds.push(Cmd::Minimize),
                    WindowButton::Maximize => cx.cmds.push(Cmd::ToggleMaximized),
                    WindowButton::Close => cx.cmds.push(Cmd::CloseWindow),
                    WindowButton::Icon => {}
                }
                break;
            }
        }
        Vec::new()
    }
}
