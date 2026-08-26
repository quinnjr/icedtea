//! The M1 widget: a `button` CSS node with interaction state.

use skia_rs_safe::canvas::Surface;

use crate::css::cascade::CompiledSheet;
use crate::css::computed::ComputedStyle;
use crate::css::select::{CssNode, PseudoStates};
use crate::layout::{Allocation, ButtonLayout};
use crate::paint::paint_button;
use crate::text::{FontStack, ShapedText};

/// A themed button: CSS node identity, interaction state, computed style
/// and allocation, kept together and recomputed on every state change.
///
/// Three things are *cached* across restyles rather than rebuilt, because a
/// restyle happens on every `:hover`/`:active` transition:
///
/// * the shaped label, keyed by `(label, font-size)`;
/// * the parent's computed style, so the ancestor chain is cascaded once
///   instead of once per state change;
/// * the `taffy` tree.
pub struct Button {
    label: String,
    node: CssNode,
    style: ComputedStyle,
    allocation: Allocation,
    /// The label shaped at `shaped_size`; invalidated when either changes.
    shaped: Option<ShapedText>,
    /// The font size `shaped` was shaped at.
    shaped_size: f32,
    /// Identity of the [`FontStack`] `shaped` was shaped with (its address,
    /// as `usize`), so a swapped font stack invalidates the shaping cache
    /// even if the label and font size happen to match.
    shaped_font: Option<usize>,
    /// The window's computed style: fixed for this widget's lifetime, as
    /// long as the sheet it was cascaded from doesn't change.
    parent_style: Option<ComputedStyle>,
    /// Identity of the [`CompiledSheet`] `parent_style` was cascaded from
    /// (its address, as `usize`). A cheap key, not a hash: it only needs to
    /// notice *some other sheet is now in play*, which a swapped
    /// `CompiledSheet` (e.g. a live theme reload) always is.
    parent_style_sheet: Option<usize>,
    layout: ButtonLayout,
}

/// The identity key stored alongside a per-sheet or per-font-stack cache:
/// the referent's address. Cheap, and sufficient to detect "a different
/// sheet/stack is now in play" — the only thing these caches need to know.
fn identity<T>(value: &T) -> usize {
    std::ptr::from_ref(value) as usize
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
            shaped: None,
            shaped_size: f32::NAN,
            shaped_font: None,
            parent_style: None,
            parent_style_sheet: None,
            layout: ButtonLayout::new(),
        }
    }

    /// Re-run cascade, measurement and layout for the current state.
    pub fn restyle(&mut self, sheet: &CompiledSheet, fonts: &FontStack) {
        let sheet_id = identity(sheet);
        if self.parent_style.is_none() || self.parent_style_sheet != Some(sheet_id) {
            // The ancestors' own cascade cannot change while this widget
            // lives *and the sheet stays the same*, so resolve the chain
            // once per sheet and thread it in from here.
            self.parent_style = Some(
                self.node
                    .parent()
                    .map(|parent| ComputedStyle::resolve(sheet, &parent))
                    .unwrap_or_default(),
            );
            self.parent_style_sheet = Some(sheet_id);
        }
        self.style =
            ComputedStyle::resolve_with_parent(sheet, &self.node, self.parent_style.as_ref());

        // `!=` rather than an epsilon: the only thing that ever writes this
        // is a previous shape at exactly this size, and NAN != NAN makes the
        // first call always shape.
        let font_id = identity(fonts);
        #[allow(clippy::float_cmp)]
        if self.shaped.is_none()
            || self.shaped_size != self.style.font_size
            || self.shaped_font != Some(font_id)
        {
            self.shaped = Some(fonts.shape(&self.label, self.style.font_size));
            self.shaped_size = self.style.font_size;
            self.shaped_font = Some(font_id);
        }
        let metrics = self.shaped.as_ref().expect("just shaped").metrics;
        self.allocation = self.layout.compute(&self.style, &metrics);
    }

    /// Replace the pseudo-class state and restyle.
    pub fn set_states(&mut self, states: PseudoStates, sheet: &CompiledSheet, fonts: &FontStack) {
        self.node = self.node.with_states(states);
        self.restyle(sheet, fonts);
    }

    /// Replace the label and restyle, reshaping it.
    pub fn set_label(&mut self, label: &str, sheet: &CompiledSheet, fonts: &FontStack) {
        if self.label == label {
            return;
        }
        self.label = label.to_string();
        self.shaped = None;
        self.restyle(sheet, fonts);
    }

    /// The current label.
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
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
    pub fn render(&self, surface: &mut Surface, origin: (f32, f32)) {
        paint_button(
            surface,
            origin,
            &self.style,
            &self.allocation,
            self.shaped.as_ref(),
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

#[cfg(test)]
mod tests {
    use super::Button;
    use crate::BUNDLED_ADWAITA_LIGHT;
    use crate::css::cascade::CompiledSheet;
    use crate::css::select::{CssNode, PseudoStates};
    use crate::text::FontStack;

    fn fixture(css: &str, label: &str) -> (CompiledSheet, FontStack, Button) {
        let sheet = CompiledSheet::compile(css);
        let fonts = FontStack::system().expect("system font");
        let window = CssNode::new("window", &["background"], PseudoStates::default(), None);
        let mut button = Button::new(label, &[], window);
        button.restyle(&sheet, &fonts);
        (sheet, fonts, button)
    }

    #[test]
    fn the_shaped_label_survives_a_state_change_but_not_a_font_size_change() {
        // A8: the label was shaped three times per paint, each with a fresh
        // font. It is now shaped once per (label, size) -- but the cache
        // must not outlive either of those changing.
        let css = "window { background-color: #fff }\n\
                   button { font-size: 14px }\n\
                   button:hover { font-size: 30px }";
        let (sheet, fonts, mut button) = fixture(css, "Click me");
        let narrow = button.allocation().width;
        assert_eq!(button.style().font_size, 14.0);

        button.set_states(
            PseudoStates {
                hover: true,
                ..PseudoStates::default()
            },
            &sheet,
            &fonts,
        );
        assert_eq!(button.style().font_size, 30.0);
        assert!(
            button.allocation().width > narrow,
            "the label was not reshaped at the new font size: {} vs {narrow}",
            button.allocation().width
        );
    }

    #[test]
    fn setting_the_label_reshapes_it() {
        let (sheet, fonts, mut button) = fixture(BUNDLED_ADWAITA_LIGHT, "Click me");
        let before = button.allocation().width;

        button.set_label("Click me twice over", &sheet, &fonts);
        assert_eq!(button.label(), "Click me twice over");
        assert!(
            button.allocation().width > before,
            "the new label reused the old shaping: {} vs {before}",
            button.allocation().width
        );

        button.set_label("", &sheet, &fonts);
        // An empty Adwaita button is its content-box minimum plus its frame.
        assert_eq!(button.allocation().width, 36.0);
    }

    #[test]
    fn the_cached_parent_style_matches_a_full_ancestor_walk() {
        // The window's `color` must still reach the button through the
        // cached parent style, on the first restyle and every one after.
        let css = "window { color: #ff0000 }\nbutton:hover { border-width: 3px }";
        let (sheet, fonts, mut button) = fixture(css, "x");
        assert_eq!(button.style().color, skia_rs_safe::core::Color(0xFFFF_0000));

        button.set_states(
            PseudoStates {
                hover: true,
                ..PseudoStates::default()
            },
            &sheet,
            &fonts,
        );
        assert_eq!(
            button.style().color,
            skia_rs_safe::core::Color(0xFFFF_0000),
            "inheritance was lost once the parent style came from the cache"
        );
        assert_eq!(button.style().border_width, 3.0);
    }

    #[test]
    fn a_different_sheet_re_derives_the_cached_parent_style() {
        // The parent-style cache used to key on nothing but "have we ever
        // computed it", so swapping in a new `CompiledSheet` (a live theme
        // reload) reused the old ancestor cascade. Key it on the sheet's
        // identity instead: a border painted with `currentColor` must pick
        // up the new sheet's `window { color: ... }`.
        let red_css = "window { color: red }\nbutton { border-color: currentColor }";
        let blue_css = "window { color: blue }\nbutton { border-color: currentColor }";
        let red_sheet = CompiledSheet::compile(red_css);
        let blue_sheet = CompiledSheet::compile(blue_css);
        let fonts = FontStack::system().expect("system font");

        let window = CssNode::new("window", &["background"], PseudoStates::default(), None);
        let mut button = Button::new("x", &[], window);

        button.restyle(&red_sheet, &fonts);
        let red_border = button.style().border_color;

        button.restyle(&blue_sheet, &fonts);
        let blue_border = button.style().border_color;

        assert_ne!(
            red_border, blue_border,
            "the cached parent style survived a sheet swap"
        );
    }
}
