//! `GtkWindow` -- `Kind::Window`, CSS node `window.background`.
//!
//! ```text
//! window.background [.csd / .solid-csd / .ssd] [.maximized / .fullscreen / .tiled]
//! ├── <child>
//! ╰── <titlebar child>.titlebar [.default-decoration]
//! ```
//!
//! The window is its own root node -- there is no chrome subnode the way
//! `HeaderBar`'s `windowhandle` or `ActionBar`'s `revealer` are -- so the
//! view's own children land flat on it in declaration order, and
//! [`WindowC::place`] only needs to tag whichever one carries
//! [`PropName::Section`] `"titlebar"` (`crate::view::builders::slot`, via
//! `.titlebar()`) with the `.titlebar` class; `window()`/`.titlebar()`
//! already put it last, so no reparenting is needed the way `ActionBar`'s
//! three sections need. `place` runs from [`Controller::reserved_total`]
//! the same way `ActionBarC::place`/`HeaderBarC::place` do, so `titlebar`
//! and `default_widget` are `RefCell`s rather than the plain fields the
//! design sketch shows -- `HeaderBarC::title`'s own doc comment gives the
//! same reason: `reserved_total` is `&self`, not `&mut self`.

use std::cell::RefCell;

use crate::css::node::{Node, PseudoStates};
use crate::layout::{BoxDirection, Container};
use crate::view::cmd::Cmd;
use crate::view::controller::{Controller, Event, EventCx};
use crate::view::{BuildCx, Kind, Prop, PropName, Props};
use crate::widgets::types::Decoration;
use crate::widgets::{Universal, props_of, set_container};
use crate::window::SurfaceStates;
use crate::window::focus::FocusRing;
use crate::window::keyboard::Mods;

/// `Kind::Window`'s controller.
pub struct WindowC {
    /// The `xdg_toplevel` states the compositor last reported.
    pub states: SurfaceStates,
    /// The child tagged `slot("titlebar")`, once [`WindowC::place`] has
    /// found it. `RefCell` -- see the module doc.
    pub titlebar: RefCell<Option<Node>>,
    /// The window's focus ring.
    pub focus: FocusRing,
    /// The descendant Enter activates, once [`WindowC::place`] has found
    /// it. `RefCell` -- see the module doc.
    pub default_widget: RefCell<Option<Node>>,
    /// Who draws this window's decorations.
    pub decoration: Decoration,
    /// The window's own root node -- `reserved_total` is `&self` with no
    /// `Node` of its own, so `place` needs a stored reference the same way
    /// `ActionBarC`/`HeaderBarC` keep their chrome subnode's.
    node: Node,
    universal: Universal,
}

impl WindowC {
    /// Push `SurfaceStates` onto the node's classes and `:backdrop`.
    ///
    /// The three state classes are mutually exclusive and *removed* when the
    /// state goes away -- a window that is maximized, then unmaximized, then
    /// tiled must not accumulate all three.
    pub fn apply_states(&self, node: &Node) {
        for class in ["maximized", "fullscreen", "tiled"] {
            node.remove_class(class);
        }
        if self.states.contains(SurfaceStates::FULLSCREEN) {
            node.add_class("fullscreen");
        } else if self.states.contains(SurfaceStates::MAXIMIZED) {
            node.add_class("maximized");
        } else if self.states.intersects(
            SurfaceStates::TILED_LEFT
                | SurfaceStates::TILED_RIGHT
                | SurfaceStates::TILED_TOP
                | SurfaceStates::TILED_BOTTOM,
        ) {
            node.add_class("tiled");
        }
        node.set_state(
            PseudoStates::BACKDROP,
            !self.states.contains(SurfaceStates::ACTIVATED),
        );
    }

    /// Sort the node's flat-attached children: whichever one carries
    /// `Section == "titlebar"` gets the `.titlebar` class and is recorded;
    /// whichever descendant carries the `.default` class (`.default_widget()`
    /// marks it) is recorded as the Enter target. Runs from
    /// [`Controller::reserved_total`], after the reconciler has attached
    /// every real child to the node and before it trims anything past the
    /// bound this returns.
    fn place(&self) {
        let mut titlebar = None;
        for child in self.node.children() {
            if props_of(&child).str(PropName::Section) == Some("titlebar") {
                child.add_class("titlebar");
                titlebar = Some(child);
            }
        }
        *self.titlebar.borrow_mut() = titlebar;
        let default = self
            .node
            .descendants()
            .find(|d| d.classes().iter().any(|c| c.as_str() == "default"));
        *self.default_widget.borrow_mut() = default;
    }

    fn apply_decoration(&mut self, node: &Node, decoration: Decoration) {
        for class in ["csd", "solid-csd", "ssd"] {
            node.remove_class(class);
        }
        node.add_class(match decoration {
            Decoration::Csd => "csd",
            Decoration::SolidCsd => "solid-csd",
            Decoration::Ssd => "ssd",
        });
        self.decoration = decoration;
    }
}

/// `GtkWindow`, wrapping `child` as its sole application child.
#[must_use]
pub fn window<Msg: Clone + 'static>(child: crate::view::View<Msg>) -> crate::view::View<Msg> {
    crate::view::View::new(Kind::Window).child(child)
}

/// `GtkWindow`'s own property setters, chained after [`window`].
///
/// Scoped to its own trait, rather than plain inherent `View<Msg>` methods,
/// for the same reason [`crate::widgets::action_bar::ActionBarExt`]'s own
/// doc comment gives: `.modal`/`.icon_name` already exist over the same
/// props for `ColorDialog`/`FontDialog` and `Image`, and an inherent method
/// would silently win over both at every call site.
pub trait WindowExt<Msg>: Sized {
    /// `GtkWindow:titlebar` -- replaces the default title bar with `view`.
    fn titlebar(self, view: crate::view::View<Msg>) -> Self;
    /// `GtkWindow:resizable`.
    fn resizable(self, v: impl Into<Prop>) -> Self;
    /// `GtkWindow:modal`.
    fn modal(self, v: impl Into<Prop>) -> Self;
    /// `GtkWindow:deletable`.
    fn deletable(self, v: impl Into<Prop>) -> Self;
    /// `GtkWindow:decorated`.
    fn decorated(self, v: impl Into<Prop>) -> Self;
    /// `GtkWindow:default-width`/`:default-height`.
    fn default_size(self, width: i32, height: i32) -> Self;
    /// `GtkWindow:icon-name`.
    fn icon_name(self, name: &str) -> Self;
}

impl<Msg: Clone + 'static> WindowExt<Msg> for crate::view::View<Msg> {
    fn titlebar(self, view: crate::view::View<Msg>) -> Self {
        let mut me = self;
        me.children
            .push(crate::view::builders::slot(view, "titlebar"));
        me
    }
    fn resizable(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::Resizable, v)
    }
    fn modal(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::Modal, v)
    }
    fn deletable(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::Deletable, v)
    }
    fn decorated(self, v: impl Into<Prop>) -> Self {
        self.prop(PropName::Decorated, v)
    }
    fn default_size(self, width: i32, height: i32) -> Self {
        self.prop(PropName::DefaultWidth, Prop::Int(i64::from(width)))
            .prop(PropName::DefaultHeight, Prop::Int(i64::from(height)))
    }
    fn icon_name(self, name: &str) -> Self {
        self.prop(PropName::IconName, Prop::Str(std::rc::Rc::from(name)))
    }
}

/// Mark `self` as the window's default widget -- the one Enter activates
/// (`GtkWindow:default-widget`). Not scoped to a `WindowExt`-style trait: it
/// is set on the *target* widget (a dialog's "OK" button, nested arbitrarily
/// deep under the window's child), not on the window itself, so there is
/// nothing for it to collide with the way `.modal`/`.icon_name` do.
///
/// [`WindowC::place`] finds it back by the `.default` class this adds,
/// walking every descendant after the reconciler has attached the whole
/// subtree -- there is no `Prop::Node` a builder could hand the window
/// directly, the same gap [`crate::view::builders::View::label_widget`]'s
/// unread [`PropName::LabelWidget`] marker leaves for `Frame`.
impl<Msg: Clone + 'static> crate::view::View<Msg> {
    /// See the trait-level doc above.
    #[must_use]
    pub fn default_widget(self) -> Self {
        self.class("default")
            .prop(PropName::DefaultWidget, Prop::Bool(true))
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for WindowC {
    fn kind(&self) -> Kind {
        Kind::Window
    }

    fn build(node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        set_container(
            node,
            Container::Box {
                direction: BoxDirection::Column,
            },
        );
        let decoration = Decoration::from_u16(
            u16::try_from(props.int(
                PropName::Decoration,
                i64::from(Decoration::default() as u16),
            ))
            .unwrap_or(Decoration::default() as u16),
        );
        let mut me = Self {
            states: SurfaceStates::empty(),
            titlebar: RefCell::new(None),
            focus: <FocusRing as Default>::default(),
            default_widget: RefCell::new(None),
            decoration: Decoration::default(),
            node: node.clone(),
            universal: Universal::new(node, Kind::Window),
        };
        me.apply_decoration(node, decoration);
        me
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        match name {
            PropName::Decoration => {
                let decoration =
                    Decoration::from_u16(crate::widgets::prop_u16(value, self.decoration as u16));
                self.apply_decoration(node, decoration);
            }
            PropName::Title
            | PropName::Resizable
            | PropName::Modal
            | PropName::Deletable
            | PropName::Decorated
            | PropName::DefaultWidth
            | PropName::DefaultHeight
            | PropName::IconName => {
                // `xdg_toplevel`/the compositor own the title, resizability,
                // modality, deletability, decoration-by-the-shell, initial
                // size and icon -- P3's `window::SurfaceStates`/`Cmd` plane
                // carries them there. Nothing here affects the node's
                // classes or layout, so there is nothing to apply.
            }
            other => {
                self.universal.apply(node, Kind::Window, other, value);
            }
        }
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        match ev {
            Event::Configure { states, .. } => {
                self.states = *states;
                self.apply_states(cx.node);
            }
            Event::Key(key) if key.pressed => {
                use xkbcommon::xkb::keysyms;
                let sym = u32::from(key.keysym);
                let mods = key.effective_mods();
                let cmd = if sym == keysyms::KEY_F4 && mods == Mods::ALT {
                    Some(Cmd::CloseWindow)
                } else if sym == keysyms::KEY_F10 && mods == Mods::LOGO {
                    Some(Cmd::ToggleMaximized)
                } else if sym == keysyms::KEY_h && mods == Mods::ALT {
                    Some(Cmd::Minimize)
                } else {
                    None
                };
                if let Some(cmd) = cmd {
                    // The compositor owns window state; the toolkit only
                    // asks. Hiding the surface here would desynchronise it.
                    cx.cmds.push(cmd);
                    cx.handled = true;
                } else if matches!(
                    sym,
                    s if s == keysyms::KEY_Return
                        || s == keysyms::KEY_KP_Enter
                        || s == keysyms::KEY_ISO_Enter
                ) && let Some(default) = self.default_widget.borrow().clone()
                {
                    cx.cmds.push(Cmd::Focus(default));
                    cx.handled = true;
                }
            }
            _ => {}
        }
        Vec::new()
    }

    fn reserved_total(&self, view_count: usize) -> usize {
        // No chrome subnode of its own -- see the module doc -- so every
        // view child is real and none is reserved ahead of it.
        self.place();
        view_count
    }
}

#[cfg(test)]
mod tests {
    use crate::css::node::PseudoStates;
    use crate::view::{Cmd, Event, Kind, Prop, PropName, Props};
    use crate::widgets::types::Decoration;
    use crate::widgets::{Headless, build_widget, matches_fixture};
    use crate::window::SurfaceStates;

    fn props(decoration: Decoration) -> Props {
        let mut p = Props::default();
        p.set(PropName::Decoration, Prop::Enum(decoration.to_u16()));
        p.set(PropName::Title, Prop::Str("Files".into()));
        p
    }

    #[test]
    fn a_window_is_window_background_with_the_decoration_class() {
        // Mutation check: emitting `.csd` for an SSD window makes Adwaita
        // draw a client shadow and rounded corners the compositor is already
        // drawing, which double-decorates every real toplevel.
        let built = build_widget::<()>(Kind::Window, &props(Decoration::Ssd));
        matches_fixture(&built.node, "window.background\n╰── <child>\n").expect("window fixture");
        assert!(built.node.classes().iter().any(|c| c.as_str() == "ssd"));
        let csd = build_widget::<()>(Kind::Window, &props(Decoration::Csd));
        assert!(csd.node.classes().iter().any(|c| c.as_str() == "csd"));
    }

    #[test]
    fn surface_states_drive_the_state_classes_and_backdrop() {
        // Mutation check: leaving `.maximized` on after an unmaximize keeps
        // the square corners forever; forgetting BACKDROP leaves an inactive
        // window painted as active.
        let built = build_widget::<()>(Kind::Window, &props(Decoration::Csd));
        let mut c = built.controller;
        let mut hx = Headless::new();
        let mut cx = hx.event_cx(&built.node);
        c.on_event(
            &Event::Configure {
                size: (800, 600),
                states: SurfaceStates::MAXIMIZED,
            },
            &mut cx,
        );
        assert!(
            built
                .node
                .classes()
                .iter()
                .any(|c| c.as_str() == "maximized")
        );
        assert!(built.node.states().contains(PseudoStates::BACKDROP));
        c.on_event(
            &Event::Configure {
                size: (800, 600),
                states: SurfaceStates::ACTIVATED,
            },
            &mut cx,
        );
        assert!(
            !built
                .node
                .classes()
                .iter()
                .any(|c| c.as_str() == "maximized")
        );
        assert!(!built.node.states().contains(PseudoStates::BACKDROP));
    }

    #[test]
    fn the_window_key_bindings_reach_the_compositor_as_commands() {
        // gtk/gtkwindow.c:1307-1359. Mutation check: handling these in the
        // toolkit (hiding the surface for `close`) desynchronises the
        // compositor's window list from the app's.
        let built = build_widget::<()>(Kind::Window, &props(Decoration::Csd));
        let mut c = built.controller;
        let mut hx = Headless::new();
        let mut cx = hx.event_cx(&built.node);
        c.on_event(
            &Event::Key(Headless::key_with_mods("F4", &["Alt"])),
            &mut cx,
        );
        assert!(cx.cmds.iter().any(|c| matches!(c, Cmd::CloseWindow)));
        c.on_event(
            &Event::Key(Headless::key_with_mods("F10", &["Logo"])),
            &mut cx,
        );
        assert!(cx.cmds.iter().any(|c| matches!(c, Cmd::ToggleMaximized)));
    }
}
