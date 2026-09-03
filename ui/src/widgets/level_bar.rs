//! `GtkLevelBar` — `Kind::LevelBar`, CSS node `levelbar`.
//!
//! ```text
//! levelbar[.discrete]
//! ╰── trough
//!     ├── block.filled.level-name
//!     ┊
//!     ├── block.empty
//!     ┊
//! ```
//!
//! Continuous mode renders exactly one filled and one empty block; discrete
//! mode one block per integral step. Filled blocks carry `.filled` plus the
//! name of the highest offset the value has passed, and `.level-name` — GTK's
//! own literal class when no named offset matched.

use std::rc::Rc;

use crate::css::node::Node;
use crate::view::controller::{Controller, Event, EventCx};
use crate::view::{BuildCx, Kind, Prop, PropName, Props, View};
use crate::widgets::{Adjustment, LevelBarMode, WidgetEnum};

/// A `GtkLevelBar` at `value` over the default `0.0..=1.0`.
#[must_use]
pub fn level_bar<Msg: Clone + 'static>(value: f64) -> View<Msg> {
    View::new(Kind::LevelBar).prop(PropName::Value, Prop::Float(value))
}

/// `GtkLevelBar`'s own setters.
pub trait LevelBarExt<Msg>: Sized {
    /// `GtkLevelBar:min-value`.
    fn min_value(self, v: f64) -> Self;
    /// `GtkLevelBar:max-value`.
    fn max_value(self, v: f64) -> Self;
    /// `GtkLevelBar:mode`.
    fn mode(self, mode: LevelBarMode) -> Self;
    /// `gtk_level_bar_add_offset_value`. Repeated calls accumulate.
    fn offset(self, name: &str, value: f64) -> Self;
    /// `GtkLevelBar:inverted`.
    fn inverted(self, on: bool) -> Self;
}

impl<Msg: Clone + 'static> LevelBarExt<Msg> for View<Msg> {
    fn min_value(self, v: f64) -> Self {
        self.prop(PropName::Lower, Prop::Float(v))
    }
    fn max_value(self, v: f64) -> Self {
        self.prop(PropName::Upper, Prop::Float(v))
    }
    fn mode(self, mode: LevelBarMode) -> Self {
        self.prop(PropName::Model, mode.to_prop())
    }
    fn offset(self, name: &str, value: f64) -> Self {
        // Offsets ride in one `Classes` prop as `name=value` pairs: `Prop` has
        // no map variant, and a level bar has at most a handful.
        let mut encoded: Vec<Rc<str>> = match self.props.get(PropName::Detail) {
            Some(Prop::Classes(list)) => list.to_vec(),
            _ => Vec::new(),
        };
        encoded.push(Rc::from(format!("{name}={value}")));
        self.prop(PropName::Detail, Prop::Classes(Rc::from(encoded)))
    }
    fn inverted(self, on: bool) -> Self {
        self.prop(PropName::Inverted, Prop::Bool(on))
    }
}

/// `Kind::LevelBar`'s controller.
pub struct LevelBarC {
    /// Current value.
    pub value: f64,
    /// `GtkLevelBar:min-value`.
    pub min: f64,
    /// `GtkLevelBar:max-value`.
    pub max: f64,
    /// Discrete mode.
    pub discrete: bool,
    /// Named offsets, ascending by value.
    pub offsets: Vec<(Rc<str>, f64)>,
    /// The `trough` subnode.
    pub trough: Node,
    /// The `block` subnodes, left to right.
    pub blocks: Vec<Node>,
}

impl LevelBarC {
    /// The class the filled blocks carry, from the highest offset passed.
    fn level_name(&self) -> Rc<str> {
        let mut best: Option<&(Rc<str>, f64)> = None;
        for offset in &self.offsets {
            if self.value >= offset.1 && best.is_none_or(|b| offset.1 >= b.1) {
                best = Some(offset);
            }
        }
        best.map_or_else(|| Rc::from("level-name"), |(name, _)| Rc::clone(name))
    }

    /// Rebuild the block row and its classes.
    fn apply(&mut self, node: &Node) {
        node.set_state(crate::css::node::PseudoStates::empty(), false);
        if self.discrete {
            node.add_class("discrete");
            node.remove_class("continuous");
        } else {
            node.add_class("continuous");
            node.remove_class("discrete");
        }
        let adj = Adjustment::new(self.value, self.min, self.max);
        let steps = if self.discrete {
            ((self.max - self.min).round().max(1.0) as usize).min(64)
        } else {
            2
        };
        while self.blocks.len() > steps {
            if let Some(extra) = self.blocks.pop() {
                extra.detach();
            }
        }
        while self.blocks.len() < steps {
            let block = Node::new("block");
            self.trough.append_child(&block);
            self.blocks.push(block);
        }
        let level = self.level_name();
        let filled = if self.discrete {
            (adj.fraction() * steps as f64).round() as usize
        } else {
            1
        };
        let level_ref: &str = level.as_ref();
        let filled_classes = ["filled", level_ref, "level-name"];
        let empty_classes = ["empty"];
        for (index, block) in self.blocks.iter().enumerate() {
            let is_filled = index < filled;
            block.set_classes(if is_filled {
                &filled_classes
            } else {
                &empty_classes
            });
        }
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for LevelBarC {
    fn kind(&self) -> Kind {
        Kind::LevelBar
    }

    fn build(node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        let trough = Node::new("trough");
        node.append_child(&trough);
        let mut offsets: Vec<(Rc<str>, f64)> = Vec::new();
        if let Some(Prop::Classes(list)) = props.get(PropName::Detail) {
            for encoded in list.iter() {
                if let Some((name, value)) = encoded.split_once('=')
                    && let Ok(value) = value.parse::<f64>()
                    && value.is_finite()
                {
                    offsets.push((Rc::from(name), value));
                }
            }
        }
        let mut this = LevelBarC {
            value: props.float(PropName::Value, 0.0),
            min: props.float(PropName::Lower, 0.0),
            max: props.float(PropName::Upper, 1.0),
            discrete: matches!(
                LevelBarMode::from_prop(props.get(PropName::Model), LevelBarMode::Continuous),
                LevelBarMode::Discrete
            ),
            offsets,
            trough,
            blocks: Vec::new(),
        };
        this.apply(node);
        // `GtkLevelBar` implements `GtkOrientable`; this task exposes no
        // setter for it, so the node always carries the horizontal class
        // Adwaita's `levelbar.horizontal trough > block` sizing rules key
        // off — without it the blocks get no min-height and the bar renders
        // at zero size. Appended after `apply` so it never lands between
        // `levelbar` and its `.discrete`/`.continuous` class in the
        // rendered tree string the node-tree test matches on.
        node.add_class("horizontal");
        this
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        match (name, value) {
            (PropName::Value, Prop::Float(v)) => self.value = *v,
            (PropName::Lower, Prop::Float(v)) => self.min = *v,
            (PropName::Upper, Prop::Float(v)) => self.max = *v,
            (PropName::Model, Prop::Enum(_)) => {
                self.discrete = matches!(
                    LevelBarMode::from_prop(Some(value), LevelBarMode::Continuous),
                    LevelBarMode::Discrete
                );
            }
            _ => return,
        }
        self.apply(node);
    }

    fn on_event(&mut self, _ev: &Event, _cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        Vec::new()
    }

    fn measure(
        &mut self,
        _available: (Option<f32>, Option<f32>),
        _cx: &mut BuildCx<'_>,
    ) -> Option<(f32, f32)> {
        // `trough`/`block` are subnodes this controller owns directly rather
        // than `View` children (see `ProgressBarC::measure`'s own note on
        // the identical gap): they never get a taffy box of their own, so
        // this leaf's own intrinsic size has to stand in for the trough's
        // CSS minimum directly, or the whole bar collapses to zero.
        // Continuous bars use `levelbar.horizontal trough > block`'s 9px
        // min-height, with no min-width rule at all in Adwaita — mirror
        // `ProgressBar`'s own 150px trough-minimum convention for the
        // horizontal extent. Discrete bars use
        // `levelbar.horizontal.discrete trough > block`'s 2px min-height
        // plus 1px top/bottom margins (4px), and 24px min-width plus 1px
        // left/right margins (26px) per block.
        if self.discrete {
            let steps = self.blocks.len().max(1) as f32;
            Some((steps * 26.0, 4.0))
        } else {
            Some((150.0, 9.0))
        }
    }

    fn paint(
        &mut self,
        canvas: &mut skia_rs_safe::canvas::Canvas<'_>,
        alloc: &crate::layout::Allocation,
        style: &crate::css::computed::ComputedStyle,
        _cx: &mut crate::view::controller::PaintCx<'_>,
    ) -> bool {
        // Same gap `ProgressBarC::paint` closes: `trough`/`block` never
        // reach the layout tree, so this is the only place any of a level
        // bar's blocks actually get drawn. Blocks split the content box
        // evenly (with a 2px gap between them in discrete mode, matching
        // Adwaita's 1px margin on each side of a block); a block's own
        // `.filled`/`.empty` class (set in `apply`) picks full- or
        // reduced-alpha of the widget's own resolved colour, the same
        // "read colour off `style`, not off the subnode" convention
        // `ProgressBarC::paint` uses for its fill.
        let content = alloc.content_box;
        if content.is_empty() || self.blocks.is_empty() {
            return false;
        }
        let steps = self.blocks.len() as f32;
        let gap = if self.discrete { 2.0 } else { 0.0 };
        let block_width = ((content.width - gap * (steps - 1.0).max(0.0)) / steps).max(0.0);
        let filled_color = style.color();
        let empty_color = crate::css::value::Rgba {
            a: filled_color.a * 0.3,
            ..filled_color
        };
        let mut painted = false;
        let mut x = content.x;
        for block in &self.blocks {
            let is_filled = block.classes().iter().any(|c| c.as_str() == "filled");
            let rect = crate::layout::Rect::new(x, content.y, block_width, content.height);
            if !rect.is_empty() {
                let color = if is_filled { filled_color } else { empty_color };
                canvas.draw_rect(&rect.to_skia(), &crate::paint::fill_paint(color));
                painted = true;
            }
            x += block_width + gap;
        }
        painted
    }
}
