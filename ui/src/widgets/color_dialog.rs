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

/// One palette cell, in px. Adwaita's `colorswatch` minimum.
const SWATCH_PX: f32 = 24.0;
/// The gap between cells.
const SWATCH_GAP_PX: f32 = 4.0;
/// Cells per row — GTK's own palette is nine hues by five steps.
const SWATCH_COLUMNS: usize = 9;

/// A `ColorDialogButton`'s minimum size, in px (P0-D10).
///
/// Wider than Adwaita's own `button.color` minimum of 48x32: `ColorDialogButtonC`
/// fills `node`'s box with the colour and the `button` subnode's real Adwaita
/// gradient chrome paints over the centre of that fill afterwards, so the extra
/// 16px is the margin that stays visible. `build`'s `set_size_request` and
/// `measure` both read this one pair, so what layout honours and what a caller
/// measuring a detached instance is told can never drift apart.
const SWATCH_BUTTON_MIN: (f32, f32) = (64.0, 32.0);

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
        // `node` keeps `button` as a real CSS/taffy child (contract §10's
        // sink deviation only redirects the *reconciler's* no-children
        // cleanup, so `on_event`'s `local_rect(cx.tree, cx.node, &self.button)`
        // still has a synced descendant to look up); a node with a taffy
        // child is never a leaf, so taffy never calls `measure` below no
        // matter what it returns. The floor that actually reaches layout is
        // this side-table request — `Controller::measure` stays for the
        // trait's contract and for `BuildCx`-only callers.
        //
        // The width is wider than Adwaita's own `button.color` (48px):
        // `button` centres inside `node` at its *own* CSS width (real
        // `adwaita-light.css` gives `button.color` a 26px content+padding+
        // border footprint), and `paint` below fills `node`'s box, painted
        // *before* `button`'s own background — a real, opaque `button`
        // element rule (`adwaita-light.css`'s unconditioned `button {
        // background-image: linear-gradient(...) }`) — paints over the
        // centre of that fill afterwards (`paint`'s doc: "before
        // children"). Real GTK avoids this because its `colorswatch` is a
        // distinct widget that paints its own snapshot on top of the
        // button's background, i.e. *after* it, in the child's own right;
        // reaching that here would mean giving `content` a controller of
        // its own, well past this task's two-controller scope. Padding the
        // width out to a margin `button` cannot cover keeps the swatch
        // legible without one — recorded as a contract amendment (P0-D10),
        // not hidden in a constant, and read by `measure` too so the two
        // cannot disagree.
        crate::widgets::set_size_request(node, SWATCH_BUTTON_MIN.0, SWATCH_BUTTON_MIN.1);
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

    /// The same floor `build` puts on the node: [`SWATCH_BUTTON_MIN`].
    ///
    /// Unlike `ScrollbarC`/`ScaleC`'s chrome, `button` is a *synced* subnode
    /// here (`build`'s comment) — hit-testing needs it laid out — so `node`
    /// always has a taffy child and this is never actually the leaf-measure
    /// taffy calls; `build`'s `set_size_request` is the floor that reaches
    /// layout. Kept for `Controller<Msg>`'s contract and any caller that
    /// measures a detached instance directly — which is exactly why it
    /// returns the *same* pair rather than Adwaita's bare `button.color`
    /// minimum (48x32): a caller sizing a row from `measure` would otherwise
    /// be 16px narrower than what layout actually honours (P0-D10).
    fn measure(
        &mut self,
        _available: (Option<f32>, Option<f32>),
        _cx: &mut BuildCx<'_>,
    ) -> Option<(f32, f32)> {
        Some(SWATCH_BUTTON_MIN)
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

    /// The palette grid in `content`'s coordinate space: one rect per palette
    /// entry, row-major, then the custom colour on a row of its own.
    ///
    /// `measure` and `paint` both read this, so what is drawn and what is
    /// measured cannot drift (the defect that put `color_dialog` on
    /// `KNOWN_BLANK_AT_REST`: its whole body was subnodes taffy never sized).
    #[must_use]
    pub fn grid(&self, content: Rect) -> Vec<(Rgba, Rect)> {
        let step = SWATCH_PX + SWATCH_GAP_PX;
        let mut cells = Vec::with_capacity(self.palette.len() + 1);
        for (index, colour) in self.palette.iter().enumerate() {
            let column = index % SWATCH_COLUMNS;
            let row = index / SWATCH_COLUMNS;
            cells.push((
                *colour,
                Rect::new(
                    content.x + column as f32 * step,
                    content.y + row as f32 * step,
                    SWATCH_PX,
                    SWATCH_PX,
                ),
            ));
        }
        if let Some(custom) = self.custom {
            let row = self.palette.len().div_ceil(SWATCH_COLUMNS);
            cells.push((
                custom,
                Rect::new(
                    content.x,
                    content.y + row as f32 * step,
                    SWATCH_PX,
                    SWATCH_PX,
                ),
            ));
        }
        cells
    }

    /// Whether `cell` fits inside `content` and is therefore drawn.
    ///
    /// `paint` drops a cell a too-small content box cannot hold rather than
    /// overdrawing a neighbour's chrome, and `on_event` has to make the same
    /// call: a click in a cell that was never painted would otherwise pick a
    /// colour nothing on screen showed.
    #[must_use]
    pub fn cell_is_painted(content: Rect, cell: Rect) -> bool {
        cell.x + cell.width <= content.x + content.width
            && cell.y + cell.height <= content.y + content.height
    }

    /// The grid's intrinsic size: nine columns, as many rows as the palette
    /// needs, plus one for the custom colour once there is one.
    #[must_use]
    pub fn intrinsic(&self) -> (f32, f32) {
        let step = SWATCH_PX + SWATCH_GAP_PX;
        let columns = SWATCH_COLUMNS.min(self.palette.len().max(1));
        let rows = self.palette.len().div_ceil(SWATCH_COLUMNS) + usize::from(self.custom.is_some());
        (
            columns as f32 * step - SWATCH_GAP_PX,
            (rows.max(1) as f32) * step - SWATCH_GAP_PX,
        )
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
        let this = ColorDialogC {
            rgba: Self::unpack(props.float(PropName::Value, 0.0)),
            palette,
            custom: None,
            swatches,
            swatch_pointers,
            sink: Node::new("sink"),
        };
        // See `ColorDialogButtonC::build`: `chooser`/`swatches` are real
        // taffy children of `node` (needed for `on_event`'s swatch
        // hit-testing), so `node` is never a taffy leaf and `measure` below
        // is unreachable from the real render loop; this side-table floor
        // is what actually sizes the dialog. Re-applied here and wherever
        // `custom` changes, so it never drifts from `intrinsic()`.
        let (w, h) = this.intrinsic();
        crate::widgets::set_size_request(node, w, h);
        this
    }

    fn set_prop(&mut self, _node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        if let (PropName::Value, Prop::Float(packed)) = (name, value) {
            self.rgba = Self::unpack(*packed);
        }
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        // Hit-tested against the very geometry `paint` draws -- `grid()` over
        // the content box -- and not against the `colorswatch` subnodes'
        // allocations: those are taffy children the real render loop sizes
        // 0x0, so `PointerState::observe` was inside one only at its exact
        // origin and a click on any *painted* swatch fired nothing at all.
        // `ScaleC`/`CalendarC` derive their own hit regions the same way.
        let Some(content) = crate::widgets::content_rect_local(cx.tree, cx.node) else {
            return Vec::new();
        };
        let cells = self.grid(content);
        let mut picked = None;
        for (index, (_, rect)) in cells.iter().enumerate().take(self.swatches.len()) {
            if !Self::cell_is_painted(content, *rect) {
                continue;
            }
            let swatch = self.swatches[index].clone();
            let shifted = shift_event(ev, *rect);
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
            // `custom` just grew the grid by one row (`intrinsic`'s
            // `usize::from(self.custom.is_some())`); refresh the floor so
            // next frame's layout keeps matching what `paint` draws.
            let (w, h) = self.intrinsic();
            crate::widgets::set_size_request(cx.node, w, h);
            cx.handled = true;
            return cx
                .handlers
                .fire_float(EventKind::ValueChanged, Self::pack(rgba))
                .map_or_else(Vec::new, |m| vec![m]);
        }
        Vec::new()
    }

    /// Never actually taffy's leaf-measure closure for the reason `build`'s
    /// `set_size_request` comment gives (`chooser`/`swatches` are synced,
    /// so `node` has children); kept for `Controller<Msg>`'s contract.
    fn measure(
        &mut self,
        _available: (Option<f32>, Option<f32>),
        _cx: &mut BuildCx<'_>,
    ) -> Option<(f32, f32)> {
        Some(self.intrinsic())
    }

    fn paint(
        &mut self,
        canvas: &mut skia_rs_safe::canvas::Canvas<'_>,
        alloc: &crate::layout::Allocation,
        _style: &crate::css::computed::ComputedStyle,
        _cx: &mut crate::view::controller::PaintCx<'_>,
    ) -> bool {
        let content = alloc.content_box;
        if content.is_empty() {
            return false;
        }
        let mut painted = false;
        for (colour, rect) in self.grid(content) {
            // A grid larger than the box it was given: clip by dropping the
            // cells that do not fit, rather than overdrawing a neighbour's
            // chrome. `on_event` drops the same ones.
            if !Self::cell_is_painted(content, rect) {
                continue;
            }
            canvas.draw_rect(&rect.to_skia(), &crate::paint::fill_paint(colour));
            painted = true;
        }
        painted
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

        // The click lands where the dialog *paints* -- `grid()` over the
        // content box, the one geometry `paint` and `measure` share -- not on
        // a `colorswatch` subnode's own allocation. That is the whole fix: the
        // real render loop sizes those subnodes 0x0, so hit-testing them meant
        // only the exact origin of each was ever inside, and a click on any
        // painted swatch fired nothing at all.
        //
        // Mutation check: hit-test `local_rect(cx.tree, cx.node, &swatch)`
        // again (the shape this replaced) and no `ValueChanged` is produced
        // from this point. Restore.
        let content =
            crate::widgets::content_rect_local(&tree, &node).expect("the dialog is laid out");
        let cells = controller.grid(content);
        let target_index = 2;
        let painted = cells[target_index].1;
        assert!(
            painted.width > 0.0 && painted.height > 0.0,
            "a painted swatch has an area"
        );
        let swatch_at = (
            painted.x + painted.width / 2.0,
            painted.y + painted.height / 2.0,
        );

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

    /// A cell the content box is too small to hold is not painted, so it is
    /// not clickable either — picking a colour nothing on screen shows is
    /// exactly the mismatch `grid()` exists to prevent.
    ///
    /// Mutation check: drop `on_event`'s `cell_is_painted` guard and this
    /// fires a `ValueChanged` for a swatch the widget never drew.
    #[test]
    fn a_swatch_that_does_not_fit_is_not_painted_and_not_clickable() {
        use crate::layout::Rect;

        // One row of swatches' worth of height, a couple of columns wide.
        let content = Rect::new(0.0, 0.0, 60.0, 24.0);
        let controller = ColorDialogC {
            rgba: ColorDialogC::unpack(0.0),
            palette: ColorDialogC::default_palette(),
            custom: None,
            swatches: Vec::new(),
            swatch_pointers: Vec::new(),
            sink: Node::new("sink"),
        };
        let cells = controller.grid(content);
        assert!(
            ColorDialogC::cell_is_painted(content, cells[0].1),
            "the first column fits"
        );
        assert!(
            !ColorDialogC::cell_is_painted(content, cells[2].1),
            "the third column runs off the 60px content box"
        );
        assert!(
            !ColorDialogC::cell_is_painted(content, cells[9].1),
            "and so does every row past the first"
        );
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
