//! The M1 widget, now a behaviour over a `css::node::Node`, laid out by
//! [`LayoutTree`] and painted by [`paint_node`].

use std::rc::Rc;

use skia_rs_safe::canvas::Surface;

use crate::anim::Overrides;
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
        }
    }

    /// Recascade, reshape and relayout.
    pub fn restyle(&mut self, sheet: &CompiledSheet, fonts: &mut FontDatabase) {
        let mut cx = MatchCx::new();
        self.style = ComputedStyle::resolve_chain(sheet, &self.node, &self.env, &mut cx);
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

    /// Paint the button at `origin` on `surface`.
    pub fn render(
        &mut self,
        surface: &mut Surface,
        origin: (f32, f32),
        sheet: &CompiledSheet,
        fonts: &mut FontDatabase,
        overrides: Option<&Overrides>,
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
                overrides,
                &mut cx,
            );
        }
        cx.text = self.shaped.as_deref();
        let mut canvas = surface.canvas();
        paint_node(
            &mut canvas,
            &self.label_node,
            &self.label_style,
            &label_alloc,
            overrides,
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
