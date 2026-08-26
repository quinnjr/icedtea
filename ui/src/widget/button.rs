//! The M1 widget: a `button` CSS node with interaction state.

use skia_rs_safe::canvas::Surface;

use crate::css::cascade::CompiledSheet;
use crate::css::computed::ComputedStyle;
use crate::css::select::{CssNode, PseudoStates};
use crate::layout::{Allocation, layout_button};
use crate::paint::paint_button;
use crate::text::FontStack;

/// A themed button: CSS node identity, interaction state, computed style
/// and allocation, kept together and recomputed on every state change.
pub struct Button {
    label: String,
    node: CssNode,
    style: ComputedStyle,
    allocation: Allocation,
}

impl Button {
    /// Create a `button` node with `classes`, parented to `parent`.
    ///
    /// The style and allocation are the type's defaults until
    /// [`restyle`](Self::restyle) runs.
    #[must_use]
    pub fn new(label: &str, classes: &[&str], parent: CssNode) -> Self {
        Self {
            label: label.to_string(),
            node: CssNode::new("button", classes, PseudoStates::default(), Some(parent)),
            style: ComputedStyle::default(),
            allocation: Allocation {
                width: 0.0,
                height: 0.0,
                label_x: 0.0,
                label_y: 0.0,
            },
        }
    }

    /// Re-run cascade, measurement and layout for the current state.
    pub fn restyle(&mut self, sheet: &CompiledSheet, fonts: &FontStack) {
        self.style = ComputedStyle::resolve(sheet, &self.node);
        let metrics = fonts.measure(&self.label, self.style.font_size);
        self.allocation = layout_button(&self.style, &metrics);
    }

    /// Replace the pseudo-class state and restyle.
    pub fn set_states(&mut self, states: PseudoStates, sheet: &CompiledSheet, fonts: &FontStack) {
        self.node = self.node.with_states(states);
        self.restyle(sheet, fonts);
    }

    /// The current pseudo-class state.
    #[must_use]
    pub fn states(&self) -> PseudoStates {
        self.node.states()
    }

    /// The computed style from the last [`restyle`](Self::restyle).
    #[must_use]
    pub fn style(&self) -> &ComputedStyle {
        &self.style
    }

    /// The allocation from the last [`restyle`](Self::restyle).
    #[must_use]
    pub fn allocation(&self) -> Allocation {
        self.allocation
    }

    /// Paint this button at `origin` on `surface`.
    pub fn render(&self, surface: &mut Surface, origin: (f32, f32), fonts: &FontStack) {
        paint_button(
            surface,
            origin,
            &self.style,
            &self.allocation,
            &self.label,
            fonts,
        );
    }

    /// Whether surface point `(x, y)` falls inside this button's border box
    /// when the button is drawn at `origin`. Rectangular, not radius-aware:
    /// GTK's own hit testing is rectangular too.
    #[must_use]
    pub fn contains(&self, origin: (f32, f32), x: f64, y: f64) -> bool {
        let (ox, oy) = (f64::from(origin.0), f64::from(origin.1));
        x >= ox
            && y >= oy
            && x < ox + f64::from(self.allocation.width)
            && y < oy + f64::from(self.allocation.height)
    }
}
