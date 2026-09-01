//! `GtkOverlay` -- one main child with siblings painted over it.
//!
//! ```text
//! overlay
//! ├── <child>
//! ╰── <overlay child>[.left][.right][.top][.bottom]
//! ```
//! (`gtk/gtkoverlay.c`)
//!
//! Every child lands in the same [`Container::Grid`] cell (one column, one
//! row) so stacking needs no bespoke arithmetic: the main child (index 0)
//! fills it, and every overlay child (index 1+) is pinned into it too, so
//! it paints on top instead of auto-flowing into an implicit second row.
//!
//! `Controller::build` runs before the reconciler attaches a container's
//! children (`grid::GridC`'s own doc comment), so `OverlayC::classify`
//! calling `node.children()` at build time sees none yet. The real
//! classification -- the positional class, and each child's
//! [`ChildLayout`] -- happens in [`classify_overlay_child`], called from
//! `widgets::flush_layout` every frame via `mark_overlay_children`'s
//! pending flag, the same deferral `grid_from_children` uses.

use crate::css::node::Node;
use crate::layout::{Align, ChildLayout, Container, GridPlacement};
use crate::view::{BuildCx, Controller, Event, EventCx, Kind, Prop, PropName, Props};
use crate::widgets::{Universal, mark_overlay_children, props_of, set_container};

/// One cell, at `(0, 0)`, spanning one column and one row -- every child of
/// an overlay is pinned here so they stack instead of auto-flowing into a
/// second implicit row.
const ONE_CELL: GridPlacement = GridPlacement {
    column: 0,
    row: 0,
    column_span: 1,
    row_span: 1,
};

/// `GtkOverlay`.
pub struct OverlayC {
    /// `(node, measure_overlay, clip_overlay)` per overlaid child.
    pub overlays: Vec<(Node, bool, bool)>,
    universal: Universal,
}

impl OverlayC {
    /// The positional class an overlay child's alignment implies, or `None`
    /// when it is not pinned to an edge.
    #[must_use]
    pub fn positional_class(halign: Align, valign: Align) -> Option<&'static str> {
        match (halign, valign) {
            (Align::Start, _) => Some("left"),
            (Align::End, _) => Some("right"),
            (_, Align::Start) => Some("top"),
            (_, Align::End) => Some("bottom"),
            _ => None,
        }
    }

    /// The container's natural width: the main child's, widened only by the
    /// overlays that opted into measuring.
    #[must_use]
    pub fn natural_width(overlays: &[(f32, bool)], main: f32) -> f32 {
        overlays
            .iter()
            .filter(|(_, measure)| *measure)
            .map(|(w, _)| *w)
            .fold(main, f32::max)
    }

    /// Rebuild `self.overlays` from whatever children `node` has right now.
    ///
    /// Called from `build`/`set_prop`, both of which may run before the
    /// reconciler has attached any real children (`build`'s doc comment on
    /// the module) -- so this is a best-effort snapshot for API callers,
    /// not the mechanism the layout/paint passes rely on. That mechanism is
    /// `classify_overlay_child`, re-run from `flush_layout` every frame.
    fn classify(&mut self, node: &Node) {
        self.overlays.clear();
        for child in node.children().iter().skip(1) {
            let props = props_of(child);
            self.overlays.push((
                child.clone(),
                props.bool(PropName::MeasureOverlay, false),
                props.bool(PropName::ClipOverlay, false),
            ));
        }
    }
}

/// One overlay child's placement, derived from its own recorded props.
///
/// `index` `0` is the main child: it fills the whole cell regardless of any
/// `Halign`/`Valign` it was given, exactly as `GtkOverlay`'s own main child
/// does. Every later index is an overlay child: its own `Halign`/`Valign`
/// (`Align::Fill` absent, matching every other widget's default) place it
/// within the shared cell, and the positional class Adwaita's
/// `overlay > .top` (etc.) styling reads is written onto its node here, the
/// one place both the main and headless-test paths call through.
pub(crate) fn classify_overlay_child(index: usize, child: &Node) -> ChildLayout {
    if index == 0 {
        return ChildLayout {
            halign: Align::Fill,
            valign: Align::Fill,
            hexpand: true,
            vexpand: true,
            grid: Some(ONE_CELL),
            ..ChildLayout::default()
        };
    }
    let props = props_of(child);
    let halign = match props.get(PropName::Halign) {
        Some(Prop::Align(a)) => *a,
        _ => Align::Fill,
    };
    let valign = match props.get(PropName::Valign) {
        Some(Prop::Align(a)) => *a,
        _ => Align::Fill,
    };
    for class in ["left", "right", "top", "bottom"] {
        child.remove_class(class);
    }
    if let Some(class) = OverlayC::positional_class(halign, valign) {
        child.add_class(class);
    }
    ChildLayout {
        halign,
        valign,
        grid: Some(ONE_CELL),
        // Out of flow: an overlay child must never widen the single track
        // the main child fills, the way an ordinary same-cell sibling
        // would (`layout::LayoutTree::child_style`'s doc comment).
        absolute: true,
        ..ChildLayout::default()
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for OverlayC {
    fn kind(&self) -> Kind {
        Kind::Overlay
    }

    fn build(node: &Node, _props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        // Overlays stack: every child occupies the same cell, so a
        // single-track grid is exactly right and taffy does the rest, once
        // `classify_overlay_child` pins each child into that one cell.
        // `homogeneous: true` makes that one track a `1fr` track (an `auto`
        // track sizes to its content instead), so the main child actually
        // fills the whole overlay rather than shrinking to its own content.
        set_container(
            node,
            Container::Grid {
                columns: 1,
                rows: 1,
                column_spacing: 0.0,
                row_spacing: 0.0,
                column_homogeneous: true,
                row_homogeneous: true,
            },
        );
        mark_overlay_children(node);
        let mut me = Self {
            overlays: Vec::new(),
            universal: Universal::new(node, Kind::Overlay),
        };
        me.classify(node);
        me
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        match name {
            PropName::MeasureOverlay
            | PropName::ClipOverlay
            | PropName::Halign
            | PropName::Valign => {
                self.classify(node);
            }
            other => {
                self.universal.apply(node, Kind::Overlay, other, value);
            }
        }
    }

    fn on_event(&mut self, _ev: &Event, _cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use crate::layout::Align;
    use crate::view::builders::{label, overlay};
    use crate::view::{Cmd, Kind, Props, ScriptStep, View};
    use crate::widgets::offscreen::{frames, px};
    use crate::widgets::overlay::OverlayC;
    use crate::widgets::{build_widget, matches_fixture};

    #[test]
    fn an_overlay_is_the_child_then_the_overlay_children() {
        // Mutation check: inserting overlays before the main child paints
        // them under it, which is the whole point of the widget inverted.
        let built = build_widget::<()>(Kind::Overlay, &Props::default());
        matches_fixture(&built.node, "overlay\n├── <child>\n╰── <overlay child>\n")
            .expect("overlay fixture");
    }

    #[test]
    fn an_edge_aligned_overlay_child_gets_that_positional_class() {
        // Mutation check: skipping the positional class means Adwaita's
        // `overlay > .top` shadow never renders on an overlaid header.
        assert_eq!(
            OverlayC::positional_class(Align::Start, Align::Fill),
            Some("left")
        );
        assert_eq!(
            OverlayC::positional_class(Align::Fill, Align::End),
            Some("bottom")
        );
        assert_eq!(
            OverlayC::positional_class(Align::Center, Align::Center),
            None
        );
    }

    #[derive(Clone)]
    enum Msg {}

    fn update(_: &mut (), msg: Msg) -> Cmd<Msg> {
        match msg {}
    }

    fn view(_: &()) -> View<Msg> {
        // Reconciliation: the reconciler gives the app's own top-level view
        // no `ChildLayout` (`view::app::rebuild`'s `Container::Box{Column}`
        // for the window has no per-child override for it), so it lands
        // centred at its own natural size rather than filling the surface
        // -- `frame.rs`'s own pixel test hits the identical gap. A
        // three-line main child gives the overlay real vertical room for
        // `valign(Start)` to move within, where a single matched line pair
        // would not.
        overlay(label("under\nunder\nunder").class("under"))
            .overlay(label("over").class("over").valign(Align::Start))
    }

    #[test]
    fn the_overlay_child_paints_over_the_main_child_at_rest() {
        // Rest-state pixel test. Mutation check: painting children in
        // reverse order leaves the top strip showing the main child.
        let out = frames((), update, view, (200, 60), vec![ScriptStep::Capture]);
        // Reconciliation: the top-level view lands centred at its own
        // natural size (this file's `view` doc comment), so with a
        // single-line main child the whole overlay is only one line tall
        // and `valign(Start)` has no room to move within it -- hence the
        // three-line main child, which makes the overlay's own natural
        // height span nearly the full 60px surface. `(100, 8)` is inside
        // its first rendered line's ink; `(100, 58)` is below the last of
        // its three lines, back on the plain background.
        let bottom = px(&out, 0, 100, 58);
        let top = px(&out, 0, 100, 8);
        assert_ne!(top, bottom, "the overlaid child owns the top strip");
    }

    #[test]
    fn a_non_measuring_overlay_does_not_grow_the_container() {
        // Interaction test through the measure path. Mutation check: always
        // measuring overlays makes a floating action button widen its page.
        let small = OverlayC::natural_width(&[(120.0, false)], 200.0);
        let measured = OverlayC::natural_width(&[(400.0, true)], 200.0);
        assert_eq!(small, 200.0);
        assert_eq!(measured, 400.0);
    }
}
