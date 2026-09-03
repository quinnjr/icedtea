//! `GtkSeparator` — `Kind::Separator`, CSS node `separator`.
//!
//! ```text
//! separator.horizontal
//! separator.vertical
//! ```
//!
//! A single node with no subnodes, no pointer behaviour and no keyboard
//! behaviour; the line itself is Adwaita's `background-color` on a 1px box.

use crate::css::node::Node;
use crate::view::controller::{Controller, Event, EventCx};
use crate::view::{BuildCx, Kind, Prop, PropName, Props, View};
use crate::widgets::{Orientation, Universal, WidgetEnum};

/// A `GtkSeparator` in `orientation`.
#[must_use]
pub fn separator<Msg: Clone + 'static>(orientation: Orientation) -> View<Msg> {
    View::new(Kind::Separator).prop(PropName::Orientation, orientation.to_prop())
}

/// `Kind::Separator`'s controller. No pointer state: GTK delivers no events
/// to a separator, and neither do we.
pub struct SeparatorC {
    /// The orientation whose class the node carries.
    pub orientation: Orientation,
    /// The `GtkWidget`-universal props (`class`, `id`, `sensitive`, …).
    universal: Universal,
}

impl SeparatorC {
    fn apply(&self, node: &Node) {
        for candidate in Orientation::all() {
            if *candidate != self.orientation {
                node.remove_class(candidate.css_class());
            }
        }
        node.add_class(self.orientation.css_class());
    }
}

impl<Msg> Controller<Msg> for SeparatorC {
    fn kind(&self) -> Kind {
        Kind::Separator
    }

    fn build(node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        let this = SeparatorC {
            orientation: Orientation::from_prop(
                props.get(PropName::Orientation),
                Orientation::Horizontal,
            ),
            universal: Universal::new(node, Kind::Separator),
        };
        this.apply(node);
        this
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        if self.universal.apply(node, Kind::Separator, name, value) {
            return;
        }
        if name == PropName::Orientation {
            self.orientation = Orientation::from_prop(Some(value), self.orientation);
            self.apply(node);
        }
    }

    fn on_event(&mut self, _ev: &Event, _cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        Vec::new()
    }
}
