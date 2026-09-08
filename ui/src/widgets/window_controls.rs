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

    /// The child's node kind, CSS class and symbolic icon name for `token`,
    /// given whether the surface is maximized. Folding both the class and
    /// icon-name selection into one match (a single tuple return) means a new
    /// `WindowButton` variant is threaded through exactly once, instead of
    /// being picked twice a few lines apart; it also lets the icon mapping —
    /// including the maximize/restore swap — be unit tested without touching
    /// the node tree at all.
    #[must_use]
    fn button_spec(
        token: WindowButton,
        maximized: bool,
    ) -> (&'static str, &'static str, Option<&'static str>) {
        match token {
            WindowButton::Icon => ("image", "icon", None),
            WindowButton::Minimize => ("button", "minimize", Some("window-minimize-symbolic")),
            WindowButton::Maximize if maximized => {
                ("button", "maximize", Some("window-restore-symbolic"))
            }
            WindowButton::Maximize => ("button", "maximize", Some("window-maximize-symbolic")),
            WindowButton::Close => ("button", "close", Some("window-close-symbolic")),
        }
    }

    /// Rebuild the children from `layout`, `side` and the maximized state.
    fn rebuild(&mut self, node: &Node) {
        // A `windowcontrols` is a horizontal strip of buttons; without an
        // explicit row container it establishes no flex context and measured
        // 0x0 even once its buttons had icons.
        crate::widgets::set_container(
            node,
            crate::layout::Container::Box {
                direction: crate::layout::BoxDirection::Row,
            },
        );
        for (_, child) in self.buttons.drain(..) {
            child.detach();
        }
        for token in Self::tokens(&self.layout, self.side) {
            // Each control button carries an `image` child, exactly as GTK's
            // own `gtk_button_new_from_icon_name` gives it one, and the
            // symbolic icon name is bound straight onto that node with
            // `set_icon` — GTK sets these programmatically, so the vendored
            // Adwaita sheet carries no `-gtk-icon-source` rule for them.
            // `paint_row` draws the bound glyph the same controllerless way it
            // draws a pooled row's text; without it the buttons were empty
            // boxes and the whole strip drew nothing at rest. The maximize
            // button shows the restore glyph while the surface is maximized,
            // GTK's own `notify::maximized` behaviour.
            let (node_kind, class, icon_name) = Self::button_spec(token, self.maximized);
            let child = Node::with_classes(node_kind, &[class]);
            if let Some(name) = icon_name {
                let image = Node::new("image");
                crate::widgets::set_icon(
                    &image,
                    crate::css::value::image::IconRef::Theme {
                        name: Rc::from(name),
                    },
                );
                child.append_child(&image);
            }
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

    /// The control buttons (and the icon) are this controller's own children,
    /// built from the decoration layout before any view child could arrive
    /// (this widget takes none), so reconcile's trim step must be told they
    /// are there — otherwise it detaches every one the first time it runs and
    /// the strip collapses to 0x0, the P8-D72 `StackSwitcher` defect in the
    /// same shape.
    fn child_index(&self, view_index: usize) -> usize {
        view_index + self.buttons.len()
    }

    fn reserved_total(&self, view_count: usize) -> usize {
        view_count + self.buttons.len()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widgets::Headless;

    fn props(layout: &str, side: Side) -> Props {
        let mut props = Props::default();
        props.set(PropName::Decoration, Prop::Str(Rc::from(layout)));
        props.set(PropName::Side, side.to_prop());
        props
    }

    /// `WindowControlsC::button_spec` is the single place the icon mapping
    /// lives now, so this pins the class + icon name for every non-maximize
    /// token directly, without going through the node tree.
    #[test]
    fn button_spec_maps_each_token_to_its_class_and_icon() {
        assert_eq!(
            WindowControlsC::button_spec(WindowButton::Icon, false),
            ("image", "icon", None)
        );
        assert_eq!(
            WindowControlsC::button_spec(WindowButton::Minimize, false),
            ("button", "minimize", Some("window-minimize-symbolic"))
        );
        assert_eq!(
            WindowControlsC::button_spec(WindowButton::Close, false),
            ("button", "close", Some("window-close-symbolic"))
        );
    }

    /// Mutation check: a backwards swap here (restore at rest, maximize while
    /// maximized) would leave the button showing the wrong affordance in
    /// both states, and `gallery_gate`'s "paints something" cannot tell the
    /// two symbolic names apart.
    #[test]
    fn button_spec_swaps_the_maximize_icon_with_the_maximized_state() {
        assert_eq!(
            WindowControlsC::button_spec(WindowButton::Maximize, false),
            ("button", "maximize", Some("window-maximize-symbolic"))
        );
        assert_eq!(
            WindowControlsC::button_spec(WindowButton::Maximize, true),
            ("button", "maximize", Some("window-restore-symbolic"))
        );
    }

    /// At rest, each rendered button's `image` child is present exactly when
    /// `button_spec` says it should be -- `rebuild` wiring the icon-bearing
    /// children to the right buttons, not just the mapping in isolation.
    #[test]
    fn at_rest_each_button_carries_an_image_child_iff_its_spec_has_an_icon() {
        let node = Node::new("windowcontrols");
        let mut hx = Headless::new();
        let controller = <WindowControlsC as Controller<()>>::build(
            &node,
            &props(DEFAULT_DECORATION_LAYOUT, Side::End),
            &mut hx.cx(),
        );
        assert_eq!(
            controller.button_names(),
            vec!["minimize", "maximize", "close"]
        );
        assert!(!controller.maximized);
        for (token, child) in &controller.buttons {
            let (_, _, icon_name) = WindowControlsC::button_spec(*token, controller.maximized);
            let image_children = child
                .children()
                .into_iter()
                .filter(|c| c.name().as_ref() == "image")
                .count();
            assert_eq!(
                image_children,
                usize::from(icon_name.is_some()),
                "token {token:?} should carry an image child iff it has an icon"
            );
        }
        // The maximize button, specifically, is the one whose icon flips.
        let maximize = controller
            .buttons
            .iter()
            .find(|(token, _)| *token == WindowButton::Maximize)
            .expect("maximize button present");
        assert_eq!(maximize.1.children().len(), 1);
    }

    /// A `Configure` that flips `MAXIMIZED` rebuilds the children with the
    /// restore icon, mirroring GTK's own `notify::maximized` -- the exact
    /// wiring the LOW/MEDIUM findings were about, exercised end to end
    /// through the real `on_event` path rather than by calling `rebuild`
    /// directly.
    #[test]
    fn a_configure_flipping_maximized_swaps_the_maximize_button_icon() {
        let node = Node::new("windowcontrols");
        let mut hx = Headless::new();
        let mut controller = <WindowControlsC as Controller<()>>::build(
            &node,
            &props(DEFAULT_DECORATION_LAYOUT, Side::End),
            &mut hx.cx(),
        );
        assert!(!controller.maximized);
        assert_eq!(
            WindowControlsC::button_spec(WindowButton::Maximize, controller.maximized).2,
            Some("window-maximize-symbolic")
        );

        let mut cx = hx.event_cx::<()>(&node);
        controller.on_event(
            &Event::Configure {
                size: (800, 600),
                states: SurfaceStates::MAXIMIZED,
            },
            &mut cx,
        );

        assert!(controller.maximized);
        assert_eq!(
            WindowControlsC::button_spec(WindowButton::Maximize, controller.maximized).2,
            Some("window-restore-symbolic")
        );
        // The button that got rebuilt is still there, still carrying exactly
        // one image child -- `rebuild` didn't drop or duplicate it.
        let maximize = controller
            .buttons
            .iter()
            .find(|(token, _)| *token == WindowButton::Maximize)
            .expect("maximize button present after rebuild");
        assert_eq!(maximize.1.children().len(), 1);
    }
}
