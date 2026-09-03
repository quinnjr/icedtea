//! `GtkColorDialogButton` and `GtkColorDialog` — `Kind::ColorDialogButton`
//! (node `colorbutton`) and `Kind::ColorDialog` (node `window`, class
//! `.dialog`).
//!
//! ```text
//! colorbutton
//! ╰── button.color
//!     ╰── [content]
//! ```
//!
//! ```text
//! window.dialog
//! ╰── colorchooser
//!     ╰── colorswatch
//!     ┊
//! ```
//!
//! `GtkColorDialog` is a plain `GObject` that launches a **deprecated**
//! `GtkColorChooserDialog`, so there is no GTK widget to copy: icedtea builds
//! the chooser body itself under `window.dialog`, with the node names GTK's own
//! chooser uses (`colorchooser`, `colorswatch`).

use std::rc::Rc;

use crate::css::node::Node;
use crate::css::value::Rgba;
use crate::layout::Rect;
use crate::view::controller::{Controller, Event, EventCx};
use crate::view::{BuildCx, EventKind, Handler, Kind, Prop, PropName, Props, View};
use crate::widgets::{ColorDialogSpec, PointerState, local_rect, shift_event};

/// A `GtkColorDialogButton` showing `rgba`.
#[must_use]
pub fn color_dialog_button<Msg: Clone + 'static>(rgba: Rgba) -> View<Msg> {
    View::new(Kind::ColorDialogButton).prop(PropName::Value, Prop::Float(ColorDialogC::pack(rgba)))
}

/// A `GtkColorDialog` body opened on `rgba`.
#[must_use]
pub fn color_dialog<Msg: Clone + 'static>(rgba: Rgba) -> View<Msg> {
    View::new(Kind::ColorDialog).prop(PropName::Value, Prop::Float(ColorDialogC::pack(rgba)))
}

/// `GtkColorDialogButton`'s own setters.
pub trait ColorDialogButtonExt<Msg>: Sized {
    /// `GtkColorDialog:with-alpha`, bound down from the button.
    fn with_alpha(self, on: bool) -> Self;
    /// The dialog the button launches.
    fn dialog(self, spec: ColorDialogSpec) -> Self;
    // `GtkColorDialogButton:rgba`'s change notification is
    // `View::on_value_changed`, not a method here: an `on_change` taking
    // `Fn(f64)` is shadowed by the inherent `View::on_change(&str)` at every
    // call site, so it could only ever be reached fully qualified — and it
    // wired up the identical `EventKind::ValueChanged`/`Handler::Float` pair
    // `on_value_changed` already does.
}

impl<Msg: Clone + 'static> ColorDialogButtonExt<Msg> for View<Msg> {
    fn with_alpha(self, on: bool) -> Self {
        self.prop(PropName::Ratio, Prop::Bool(on))
    }
    fn dialog(self, spec: ColorDialogSpec) -> Self {
        self.prop(PropName::Ratio, Prop::Bool(spec.with_alpha))
            .prop(PropName::Modal, Prop::Bool(spec.modal))
    }
}

/// `GtkColorDialog`'s own setters.
pub trait ColorDialogExt<Msg>: Sized {
    /// `GtkColorDialog:title`.
    fn title(self, text: &str) -> Self;
    /// `GtkColorDialog:modal`.
    fn modal(self, on: bool) -> Self;
    /// `GtkColorDialog:with-alpha`.
    fn with_alpha(self, on: bool) -> Self;
    /// The dialog's response index.
    fn on_response(self, f: impl Fn(usize) -> Msg + 'static) -> Self;
    // The chosen colour, packed, is `View::on_value_changed` — see
    // [`ColorDialogButtonExt`] for why it is not an `on_change` here.
}

impl<Msg: Clone + 'static> ColorDialogExt<Msg> for View<Msg> {
    fn title(self, text: &str) -> Self {
        self.prop(PropName::Title, Prop::Str(Rc::from(text)))
    }
    fn modal(self, on: bool) -> Self {
        self.prop(PropName::Modal, Prop::Bool(on))
    }
    fn with_alpha(self, on: bool) -> Self {
        self.prop(PropName::Ratio, Prop::Bool(on))
    }
    fn on_response(self, f: impl Fn(usize) -> Msg + 'static) -> Self {
        self.on(EventKind::Response, Handler::Index(Rc::new(f)))
    }
}

/// `Kind::ColorDialogButton`'s controller.
pub struct ColorDialogButtonC {
    /// The chosen colour.
    pub rgba: Rgba,
    /// Whether the dialog is showing.
    pub dialog_open: bool,
    /// The `button.color` subnode.
    pub button: Node,
    /// The `content` subnode painted with `rgba`.
    pub swatch: Node,
    /// A detached, never-attached sink: `ColorDialogButton` takes no
    /// application children, so `child_slot` routes the reconciler's
    /// no-children cleanup here instead of the real root — otherwise every
    /// reconcile would strip `button` (contract §10, this task's deviation).
    pub sink: Node,
    pointer: PointerState,
}

impl<Msg: Clone + 'static> Controller<Msg> for ColorDialogButtonC {
    fn kind(&self) -> Kind {
        Kind::ColorDialogButton
    }

    fn build(node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        let button = Node::with_classes("button", &["color"]);
        node.append_child(&button);
        let swatch = Node::new("content");
        button.append_child(&swatch);
        ColorDialogButtonC {
            rgba: ColorDialogC::unpack(props.float(PropName::Value, 0.0)),
            dialog_open: false,
            button,
            swatch,
            sink: Node::new("sink"),
            pointer: PointerState::default(),
        }
    }

    fn set_prop(&mut self, _node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        if let (PropName::Value, Prop::Float(packed)) = (name, value) {
            self.rgba = ColorDialogC::unpack(*packed);
        }
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        let Some(rect) = local_rect(cx.tree, cx.node, &self.button) else {
            return Vec::new();
        };
        let shifted = shift_event(ev, rect);
        if self.pointer.observe(
            &self.button,
            &shifted,
            Some(Rect::new(0.0, 0.0, rect.width, rect.height)),
        ) {
            self.dialog_open = !self.dialog_open;
            cx.handled = true;
        }
        Vec::new()
    }

    fn paint(
        &mut self,
        canvas: &mut skia_rs_safe::canvas::Canvas<'_>,
        alloc: &crate::layout::Allocation,
        _style: &crate::css::computed::ComputedStyle,
        _cx: &mut crate::view::controller::PaintCx<'_>,
    ) -> bool {
        let rect = alloc.content_box;
        if rect.is_empty() {
            return false;
        }
        canvas.draw_rect(&rect.to_skia(), &crate::paint::fill_paint(self.rgba));
        true
    }
}

/// `Kind::ColorDialog`'s controller — the chooser body icedtea builds itself.
pub struct ColorDialogC {
    /// The chosen colour.
    pub rgba: Rgba,
    /// The offered palette.
    pub palette: Vec<Rgba>,
    /// The custom colour, once one is picked.
    pub custom: Option<Rgba>,
    /// One `colorswatch` node per palette entry.
    pub swatches: Vec<Node>,
    /// One press-tracking [`PointerState`] per entry of `swatches`, parallel
    /// to it. A swatch click is a press-then-release-inside gesture like any
    /// other, so the press has to survive between the two events: a
    /// per-event scratch `PointerState` always reports `pressed == false` on
    /// the release and would never fire [`EventKind::ValueChanged`] at all.
    swatch_pointers: Vec<PointerState>,
    /// A detached, never-attached sink: see `ColorDialogButtonC::sink`.
    pub sink: Node,
}

impl ColorDialogC {
    /// GTK's own default palette, one row per hue plus a greyscale row.
    #[must_use]
    pub fn default_palette() -> Vec<Rgba> {
        const HEX: &[u32] = &[
            0x99c1f1, 0x62a0ea, 0x3584e4, 0x1c71d8, 0x1a5fb4, 0x8ff0a4, 0x57e389, 0x33d17a,
            0x2ec27e, 0x26a269, 0xf9f06b, 0xf8e45c, 0xf6d32d, 0xf5c211, 0xe5a50a, 0xffbe6f,
            0xffa348, 0xff7800, 0xe66100, 0xc64600, 0xf66151, 0xed333b, 0xe01b24, 0xc01c28,
            0xa51d2d, 0xdc8add, 0xc061cb, 0x9141ac, 0x813d9c, 0x613583, 0xcdab8f, 0xb5835a,
            0x986a44, 0x865e3c, 0x63452c, 0xffffff, 0xf6f5f4, 0xdeddda, 0xc0bfbc, 0x9a9996,
            0x77767b, 0x5e5c64, 0x3d3846, 0x241f31, 0x000000,
        ];
        HEX.iter()
            .map(|hex| Rgba {
                r: ((hex >> 16) & 0xff) as f32 / 255.0,
                g: ((hex >> 8) & 0xff) as f32 / 255.0,
                b: (hex & 0xff) as f32 / 255.0,
                a: 1.0,
            })
            .collect()
    }

    /// Pack an `Rgba` into the `f64` a `Handler::Float` carries.
    ///
    /// `0xAARRGGBB` is at most 2^32, which an `f64` represents exactly, so the
    /// round trip is lossless to 8 bits per channel — the precision every
    /// colour picker works at anyway.
    #[must_use]
    pub fn pack(rgba: Rgba) -> f64 {
        let byte = |v: f32| -> u32 {
            if v.is_finite() {
                (v.clamp(0.0, 1.0) * 255.0).round() as u32
            } else {
                0
            }
        };
        f64::from((byte(rgba.a) << 24) | (byte(rgba.r) << 16) | (byte(rgba.g) << 8) | byte(rgba.b))
    }

    /// Unpack. A non-finite or out-of-range value yields opaque black rather
    /// than panicking — the packed colour arrives from an application model.
    #[must_use]
    pub fn unpack(packed: f64) -> Rgba {
        if !packed.is_finite() || packed < 0.0 || packed > f64::from(u32::MAX) {
            return Rgba {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            };
        }
        let bits = packed as u32;
        let channel = |shift: u32| ((bits >> shift) & 0xff) as f32 / 255.0;
        Rgba {
            r: channel(16),
            g: channel(8),
            b: channel(0),
            a: channel(24),
        }
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for ColorDialogC {
    fn kind(&self) -> Kind {
        Kind::ColorDialog
    }

    fn build(node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        node.add_class("dialog");
        let chooser = Node::new("colorchooser");
        node.append_child(&chooser);
        let palette = Self::default_palette();
        let swatches: Vec<Node> = palette
            .iter()
            .map(|_| {
                let swatch = Node::new("colorswatch");
                chooser.append_child(&swatch);
                swatch
            })
            .collect();
        let swatch_pointers: Vec<PointerState> =
            swatches.iter().map(|_| PointerState::default()).collect();
        ColorDialogC {
            rgba: Self::unpack(props.float(PropName::Value, 0.0)),
            palette,
            custom: None,
            swatches,
            swatch_pointers,
            sink: Node::new("sink"),
        }
    }

    fn set_prop(&mut self, _node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        if let (PropName::Value, Prop::Float(packed)) = (name, value) {
            self.rgba = Self::unpack(*packed);
        }
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        let mut picked = None;
        for index in 0..self.swatches.len() {
            let swatch = self.swatches[index].clone();
            let Some(rect) = local_rect(cx.tree, cx.node, &swatch) else {
                continue;
            };
            let shifted = shift_event(ev, rect);
            let Some(state) = self.swatch_pointers.get_mut(index) else {
                continue;
            };
            if state.observe(
                &swatch,
                &shifted,
                Some(Rect::new(0.0, 0.0, rect.width, rect.height)),
            ) && picked.is_none()
            {
                picked = Some(index);
            }
        }
        if let Some(rgba) = picked.and_then(|index| self.palette.get(index).copied()) {
            self.rgba = rgba;
            self.custom = Some(rgba);
            cx.handled = true;
            return cx
                .handlers
                .fire_float(EventKind::ValueChanged, Self::pack(rgba))
                .map_or_else(Vec::new, |m| vec![m]);
        }
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::rc::Rc;
    use std::time::Duration;

    use super::ColorDialogC;
    use crate::anim::{Clock, ManualClock};
    use crate::css::cascade::CompiledSheet;
    use crate::css::computed::ResolveEnv;
    use crate::css::node::Node;
    use crate::css::value::Rgba;
    use crate::layout::{Container, FixedMeasure, LayoutTree};
    use crate::view::controller::{Controller, Event, EventCx, Phase};
    use crate::view::reconcile::BuildCx;
    use crate::view::render::{Animations, StyleMap, layout_tree, node_addr, restyle_tree};
    use crate::view::{Cmd, EventKind, Handler, Handlers, Props};
    use crate::window::focus::FocusRing;
    use crate::window::selection::Clipboard;

    /// Drives `ColorDialogC::on_event` over a really laid-out tree — the
    /// real allocations `local_rect` consults — so a press-then-release
    /// gesture over a palette swatch actually selects that swatch's colour
    /// and fires `EventKind::ValueChanged`.
    #[test]
    fn clicking_a_palette_swatch_selects_its_colour() {
        // mutation: give the swatch hit-test a fresh `PointerState` per
        // event (or never fire `EventKind::ValueChanged`) and the click
        // produces no message and leaves `rgba`/`custom` unchanged.
        let sheet = CompiledSheet::compile("colorswatch { color: #000000; }");
        let mut fonts = crate::text::FontDatabase::probe_only();
        let mut icons = crate::icons::IconTheme::with_name_and_roots("hicolor", vec![]);
        let clock: Rc<dyn Clock> = Rc::new(ManualClock::new());
        let env = ResolveEnv::default();

        let node = Node::new("window");
        let props = Props::default();
        let mut controller = {
            let mut cx = BuildCx {
                sheet: &sheet,
                fonts: &mut fonts,
                icons: &mut icons,
                clock: &clock,
                env: &env,
            };
            <ColorDialogC as Controller<usize>>::build(&node, &props, &mut cx)
        };
        assert!(controller.swatches.len() > 1, "a real palette");

        // Lay the retained tree out for real: every leaf is 20x20, and each
        // container stacks its children on the main axis, so the swatches
        // land on disjoint rectangles.
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
            width: 20.0,
            height: 20.0,
        });
        layout_tree(
            &node,
            &styles,
            &containers,
            &mut tree,
            &env,
            (Some(400.0), Some(800.0)),
            &mut measure,
        )
        .expect("the dialog lays out");

        let root = tree.allocation(&node).expect("root allocation").border_box;
        let centre = |sub: &Node| {
            let b = tree.allocation(sub).expect("subnode allocation").border_box;
            assert!(b.width > 0.0 && b.height > 0.0, "a clickable box");
            (b.x - root.x + b.width / 2.0, b.y - root.y + b.height / 2.0)
        };
        let target_index = 2;
        let swatch_at = centre(&controller.swatches[target_index]);

        let mut handlers: Handlers<usize> = Handlers::default();
        handlers.set(
            EventKind::ValueChanged,
            Handler::Float(Rc::new(|packed| packed as usize)),
        );
        let mut focus = <FocusRing as Default>::default();
        let mut clipboard = Clipboard::offscreen();
        let mut cmds: Vec<Cmd<usize>> = Vec::new();

        let mut click = |controller: &mut ColorDialogC,
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

        let expected_rgba = controller.palette[target_index];
        let fired = click(
            &mut controller,
            swatch_at,
            &mut cmds,
            &mut fonts,
            &mut icons,
        );
        assert_eq!(fired.len(), 1, "one ValueChanged message reaches the app");
        assert_eq!(
            fired[0],
            ColorDialogC::pack(expected_rgba) as usize,
            "the packed colour round-trips through the handler"
        );
        assert_eq!(controller.rgba, expected_rgba);
        assert_eq!(controller.custom, Some(expected_rgba));
    }

    #[test]
    fn pack_unpack_round_trips() {
        let rgba = Rgba {
            r: 0.2,
            g: 0.4,
            b: 0.6,
            a: 0.8,
        };
        let back = ColorDialogC::unpack(ColorDialogC::pack(rgba));
        assert!((back.r - rgba.r).abs() < 1.0 / 255.0);
        assert!((back.g - rgba.g).abs() < 1.0 / 255.0);
        assert!((back.b - rgba.b).abs() < 1.0 / 255.0);
        assert!((back.a - rgba.a).abs() < 1.0 / 255.0);
    }
}
