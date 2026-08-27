//! The M1 widget, now a behaviour over a `css::node::Node`, laid out by
//! [`LayoutTree`] and painted by [`paint_node`].

use std::rc::Rc;

use skia_rs_safe::canvas::Surface;

use crate::anim::{AnimationState, Clock, MonotonicClock, Overrides};
use crate::css::cascade::CompiledSheet;
use crate::css::computed::{ComputedStyle, ResolveEnv};
use crate::css::node::{Node, PseudoStates};
use crate::css::registry::Prop;
use crate::css::select::MatchCx;
use crate::css::value::{FontFamily, FontStyle};
use crate::layout::{Allocation, BoxDirection, Container, LayoutTree, Measure};
use crate::paint::{ImageCache, PaintCx, paint_node};
use crate::text::{FontDatabase, FontQuery, ShapeKey, ShapedText};

/// A themed button: a `button` node with a `label` child.
pub struct Button {
    label: String,
    /// Nothing but this strong handle keeps `node`'s ancestors (the window,
    /// any layer-shell root) reachable: `css::node::Node`'s parent link is a
    /// `Weak`, so without a strong reference somewhere the chain above
    /// `node` is dropped the instant `new`'s `parent` parameter goes out of
    /// scope, and `node.root()` silently degrades to `node` itself.
    _root: Node,
    node: Node,
    label_node: Node,
    style: ComputedStyle,
    label_style: ComputedStyle,
    allocation: Allocation,
    label_allocation: Allocation,
    shaped: Option<Rc<ShapedText>>,
    layout: LayoutTree,
    images: ImageCache,
    env: ResolveEnv,
    /// The clock this widget's transitions and animations run on. Shared with
    /// the `LayerWindow` that drives its frame callbacks, and swapped for a
    /// `ManualClock` in tests.
    clock: Rc<dyn Clock>,
    /// Everything currently animating on this widget.
    anim: AnimationState,
    /// The values `render` paints instead of the computed ones this frame.
    overrides: Overrides,
    /// Whether `restyle` has ever run. The first style is not a change, so it
    /// starts no transitions.
    styled: bool,
}

/// Reports the shaped label's extents for the `label` leaf.
struct LabelMeasure<'a> {
    shaped: Option<&'a Rc<ShapedText>>,
}

impl Measure for LabelMeasure<'_> {
    fn measure(
        &mut self,
        node: &Node,
        _style: &ComputedStyle,
        _known: taffy::Size<Option<f32>>,
        _available: taffy::Size<taffy::AvailableSpace>,
    ) -> taffy::Size<f32> {
        match (&*node.name(), self.shaped) {
            ("label", Some(shaped)) => taffy::Size {
                width: shaped.metrics.width,
                height: shaped.metrics.line_height,
            },
            _ => taffy::Size::ZERO,
        }
    }
}

impl Button {
    /// A button labelled `label`, with `classes`, under `parent`.
    #[must_use]
    pub fn new(label: &str, classes: &[&str], parent: Node) -> Self {
        let node = Node::with_classes("button", classes);
        let label_node = Node::new("label");
        parent.append_child(&node);
        node.append_child(&label_node);
        let env = ResolveEnv::default();
        let empty = Allocation {
            border_box: crate::layout::Rect::zero(),
            content_box: crate::layout::Rect::zero(),
            border: [0.0; 4],
            padding: [0.0; 4],
        };
        Self {
            label: label.to_owned(),
            _root: parent,
            node,
            label_node,
            style: (*ComputedStyle::initial(&env)).clone(),
            label_style: (*ComputedStyle::initial(&env)).clone(),
            allocation: empty,
            label_allocation: empty,
            shaped: None,
            layout: LayoutTree::new(),
            images: ImageCache::new(),
            env,
            clock: Rc::new(MonotonicClock::new()),
            anim: AnimationState::new(),
            overrides: Overrides::default(),
            styled: false,
        }
    }

    /// Recascade, reshape and relayout.
    pub fn restyle(&mut self, sheet: &CompiledSheet, fonts: &mut FontDatabase) {
        let mut cx = MatchCx::new();
        let computed = ComputedStyle::resolve_chain(sheet, &self.node, &self.env, &mut cx);
        self.label_style =
            ComputedStyle::resolve_chain(sheet, &self.label_node, &self.env, &mut cx);

        let families: Rc<[FontFamily]> = self.label_style.get(Prop::FontFamily);
        let face = fonts.match_face(&FontQuery {
            families: &families,
            weight: self.label_style.get(Prop::FontWeight),
            style: FontStyle::Normal,
            stretch: 100.0,
            size_px: self.label_style.font_size_px(),
        });
        self.shaped = face.map(|face| {
            fonts.shape(&ShapeKey {
                text: &self.label,
                face: &face,
                size_px: self.label_style.font_size_px(),
                letter_spacing_px: self.label_style.get(Prop::LetterSpacing),
                features: &[],
                variations: &[],
                transform: self.label_style.get(Prop::TextTransform),
            })
        });

        let previous = if self.styled {
            Some(std::mem::replace(&mut self.style, computed))
        } else {
            self.style = computed;
            None
        };
        self.styled = true;
        let now = self.clock.now();
        self.anim
            .restyle(previous.as_ref(), &self.style, now, sheet);
        self.overrides = self.anim.sample(now);

        let root = self.node.root();
        if self.layout.sync(&root).is_err() {
            return;
        }
        let root_style = ComputedStyle::resolve_chain(sheet, &root, &self.env, &mut cx);
        self.layout
            .set_style(&root, &root_style, Container::default(), &self.env);
        self.layout.set_style(
            &self.node,
            &self.style,
            Container::Box {
                direction: BoxDirection::Row,
            },
            &self.env,
        );
        self.layout.set_style(
            &self.label_node,
            &self.label_style,
            Container::Leaf,
            &self.env,
        );
        let mut measure = LabelMeasure {
            shaped: self.shaped.as_ref(),
        };
        if self
            .layout
            .compute(
                &root,
                taffy::Size {
                    width: taffy::AvailableSpace::MaxContent,
                    height: taffy::AvailableSpace::MaxContent,
                },
                &mut measure,
            )
            .is_ok()
        {
            if let Some(a) = self.layout.allocation(&self.node) {
                self.allocation = a;
            }
            if let Some(a) = self.layout.allocation(&self.label_node) {
                self.label_allocation = a;
            }
        }
    }

    /// Set the pseudo-class state and restyle.
    pub fn set_states(
        &mut self,
        states: PseudoStates,
        sheet: &CompiledSheet,
        fonts: &mut FontDatabase,
    ) {
        self.node.set_states(states);
        self.restyle(sheet, fonts);
    }

    /// Set the label and restyle; a no-op if unchanged.
    pub fn set_label(&mut self, label: &str, sheet: &CompiledSheet, fonts: &mut FontDatabase) {
        if self.label == label {
            return;
        }
        self.label = label.to_owned();
        self.shaped = None;
        self.restyle(sheet, fonts);
    }

    /// The label text.
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// The current pseudo-class state.
    #[must_use]
    pub fn states(&self) -> PseudoStates {
        self.node.states()
    }

    /// The computed style.
    #[must_use]
    pub fn style(&self) -> &ComputedStyle {
        &self.style
    }

    /// The border-box allocation.
    #[must_use]
    pub fn allocation(&self) -> Allocation {
        self.allocation
    }

    /// Drive this widget's transitions and animations from `clock`.
    ///
    /// The `LayerWindow` passes its own clock in so the widget and the frame
    /// pump agree on what "now" means; tests pass a `ManualClock`.
    pub fn set_clock(&mut self, clock: Rc<dyn Clock>) {
        self.clock = clock;
    }

    /// Resample the animation clock.
    ///
    /// Returns `true` when the animated values changed and the widget must be
    /// repainted. Paint only: `tick` deliberately does not relayout, so a
    /// property that animates *and* affects layout animates its appearance
    /// but not its allocation in M2. The widget/event model that would make
    /// per-frame relayout sensible is M3's.
    pub fn tick(&mut self) -> bool {
        let sampled = self.anim.sample(self.clock.now());
        if sampled == self.overrides {
            return false;
        }
        self.overrides = sampled;
        true
    }

    /// Whether this widget still needs frames.
    #[must_use]
    pub fn is_animating(&self) -> bool {
        self.anim.is_active(self.clock.now())
    }

    /// This frame's animated values, layered over the computed style by
    /// `paint_node`.
    #[must_use]
    pub fn overrides(&self) -> &Overrides {
        &self.overrides
    }

    /// Paint the button at `origin` on `surface`.
    pub fn render(
        &mut self,
        surface: &mut Surface,
        origin: (f32, f32),
        sheet: &CompiledSheet,
        fonts: &mut FontDatabase,
    ) {
        let alloc = translated(self.allocation, origin);
        let label_alloc = translated(self.label_allocation, origin);
        let mut cx = PaintCx {
            env: &self.env,
            colors: &sheet.colors,
            fonts,
            images: &mut self.images,
            text: None,
        };
        {
            let mut canvas = surface.canvas();
            paint_node(
                &mut canvas,
                &self.node,
                &self.style,
                &alloc,
                Some(&self.overrides),
                &mut cx,
            );
        }
        cx.text = self.shaped.as_deref();
        let mut canvas = surface.canvas();
        // The label never transitions anything of its own -- the widget's
        // `AnimationState` only ever diffs `self.style` (see `restyle`) -- so
        // it must not receive the button's overrides too, or it would paint
        // with a property (e.g. an animating `background-color`) it never
        // computed for itself.
        paint_node(
            &mut canvas,
            &self.label_node,
            &self.label_style,
            &label_alloc,
            None,
            &mut cx,
        );
    }

    /// Rectangular hit test, matching GTK's own (not radius-aware).
    #[must_use]
    pub fn contains(&self, origin: (f32, f32), x: f64, y: f64) -> bool {
        let a = translated(self.allocation, origin);
        x >= f64::from(a.border_box.x)
            && x < f64::from(a.border_box.right())
            && y >= f64::from(a.border_box.y)
            && y < f64::from(a.border_box.bottom())
    }
}

/// Shift an allocation so the tree's origin lands at `origin`.
fn translated(alloc: Allocation, origin: (f32, f32)) -> Allocation {
    let shift = |r: crate::layout::Rect| {
        crate::layout::Rect::new(r.x + origin.0, r.y + origin.1, r.width, r.height)
    };
    Allocation {
        border_box: shift(alloc.border_box),
        content_box: shift(alloc.content_box),
        ..alloc
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anim::{Clock, ManualClock};
    use crate::css::value::Value;
    use std::rc::Rc;

    /// A window > button node tree, a compiled sheet from `css` and a font
    /// database, restyled once.
    fn fixture(css: &str, label: &str) -> (CompiledSheet, FontDatabase, Button) {
        let sheet = CompiledSheet::compile(css);
        let mut fonts = FontDatabase::probe_only();
        let window = Node::with_classes("window", &["background"]);
        let mut button = Button::new(label, &[], window);
        button.restyle(&sheet, &mut fonts);
        (sheet, fonts, button)
    }

    /// The pixel at `(x, y)` of `surface`, matching `ui/src/paint/`'s own
    /// `pixel` test helper.
    fn pixel(surface: &Surface, x: i32, y: i32) -> skia_rs_safe::core::Color {
        surface.pixel_buffer().get_pixel(x, y).expect("pixel")
    }

    const FADE_BUTTON: &str = "\
window { background-color: rgb(255 255 255); }
button { opacity: 1; transition: opacity 200ms linear; }
button:hover { opacity: 0; }
";

    // Mutation check: drop the `self.anim.restyle(..)` call at the end of
    // `Button::restyle` and the hover change snaps -- `overrides()` is empty
    // and the 100 ms assertion fails.
    #[test]
    fn a_state_change_animates_through_the_widgets_own_clock() {
        let (sheet, mut fonts, mut button) = fixture(FADE_BUTTON, "Click me");
        let clock = Rc::new(ManualClock::new());
        button.set_clock(Rc::clone(&clock) as Rc<dyn Clock>);
        // Re-style once on the manual clock so the *next* restyle has a
        // before-change style recorded against t = 0.
        button.restyle(&sheet, &mut fonts);
        assert!(button.overrides().is_empty());
        assert!(!button.is_animating());

        button.set_states(PseudoStates::HOVER, &sheet, &mut fonts);
        assert!(
            button.is_animating(),
            "hovering starts the opacity transition"
        );

        clock.set_ms(100);
        assert!(
            button.tick(),
            "a mid-transition tick changes the painted values"
        );
        assert_eq!(
            button.overrides().get(Prop::Opacity),
            Some(&Value::Number(0.5))
        );

        clock.set_ms(200);
        assert!(button.tick(), "the final tick clears the override");
        assert!(button.overrides().is_empty());
        assert!(!button.is_animating());
    }

    // Mutation check: have `tick` return `true` unconditionally and the
    // Wayland pump repaints forever -- this assertion is the tripwire.
    #[test]
    fn ticking_an_idle_button_reports_no_change() {
        let (sheet, mut fonts, mut button) = fixture(FADE_BUTTON, "Click me");
        let clock = Rc::new(ManualClock::new());
        button.set_clock(Rc::clone(&clock) as Rc<dyn Clock>);
        button.restyle(&sheet, &mut fonts);
        clock.set_ms(5_000);
        assert!(!button.tick());
        assert!(!button.is_animating());
    }

    // Mutation check: keep passing `None` for `overrides` in
    // `Button::render`'s call to `paint_node` and the two surfaces come out
    // identical, failing the inequality.
    #[test]
    fn the_painted_pixels_follow_the_animated_value_not_the_computed_one() {
        let css = "\
window { background-color: rgb(255 255 255); }
button { min-width: 40px; min-height: 40px; padding: 0; border: 0 solid transparent; \
border-radius: 0; background-color: rgb(255 0 0); background-image: none; \
transition: background-color 200ms linear; }
button:hover { background-color: rgb(0 0 255); }
";
        let (sheet, mut fonts, mut button) = fixture(css, "");
        let clock = Rc::new(ManualClock::new());
        button.set_clock(Rc::clone(&clock) as Rc<dyn Clock>);
        button.restyle(&sheet, &mut fonts);
        button.set_states(PseudoStates::HOVER, &sheet, &mut fonts);

        let mut start = Surface::new_raster_n32_premul(60, 60).expect("surface");
        button.render(&mut start, (0.0, 0.0), &sheet, &mut fonts);

        clock.set_ms(100);
        button.tick();
        let mut middle = Surface::new_raster_n32_premul(60, 60).expect("surface");
        button.render(&mut middle, (0.0, 0.0), &sheet, &mut fonts);

        assert_ne!(
            pixel(&start, 5, 5),
            pixel(&middle, 5, 5),
            "half way through a 200 ms background-color transition the painted \
             pixel must differ from the one painted at t = 0"
        );
    }

    // Controller note: `render` must not hand the button's own `Overrides`
    // to the label's `paint_node` call too -- the label never transitioned
    // anything, so a leaked override would paint it with a property (here,
    // a background colour) it never computed for itself.
    //
    // Mutation check: pass `Some(&self.overrides)` to both `paint_node`
    // calls in `Button::render` and the label picks up the button's
    // mid-transition background colour, failing the "not fully transparent"
    // assertion below.
    #[test]
    fn the_labels_paint_does_not_receive_the_buttons_overrides() {
        // The label has its own, non-transitioning green background;
        // no text, so the sampled pixel can only ever be the label's own
        // background-color, never anti-aliased glyph ink.
        let css = "\
window { background-color: rgb(255 255 255); }
button { min-width: 80px; min-height: 40px; padding: 0; border: 0 solid transparent; \
border-radius: 0; background-color: rgb(255 0 0); background-image: none; \
transition: background-color 200ms linear; }
button:hover { background-color: rgb(0 0 255); }
label { min-width: 20px; min-height: 20px; background-color: rgb(0 255 0); }
";
        let (sheet, mut fonts, mut button) = fixture(css, "");
        let clock = Rc::new(ManualClock::new());
        button.set_clock(Rc::clone(&clock) as Rc<dyn Clock>);
        button.restyle(&sheet, &mut fonts);
        button.set_states(PseudoStates::HOVER, &sheet, &mut fonts);
        clock.set_ms(100);
        button.tick();

        assert!(
            button.overrides().get(Prop::BackgroundColor).is_some(),
            "the fixture must actually be mid-transition for this test to mean anything"
        );

        let label_alloc = translated(button.label_allocation, (0.0, 0.0));
        let x = label_alloc.border_box.x as i32 + 5;
        let y = label_alloc.border_box.y as i32 + 5;

        let mut surface = Surface::new_raster_n32_premul(120, 60).expect("surface");
        button.render(&mut surface, (0.0, 0.0), &sheet, &mut fonts);
        let color = pixel(&surface, x, y);

        assert_eq!(
            color,
            skia_rs_safe::core::Color(0xFF00_FF00),
            "the label's own background-color is an opaque green that never \
             transitions; it must stay that colour instead of picking up the \
             button's mid-transition fill (pixel: {color:?})"
        );
    }
}
