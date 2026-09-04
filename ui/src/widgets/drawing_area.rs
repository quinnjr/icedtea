//! `GtkDrawingArea` — `Kind::DrawingArea`, CSS node `widget`.
//!
//! ```text
//! widget
//! ```
//!
//! GTK sets no CSS name on this class, so the node is `GtkWidget`'s own
//! default. Style it with a class you add.

use std::rc::Rc;

use skia_rs_safe::canvas::Canvas;

use crate::css::computed::ComputedStyle;
use crate::css::node::Node;
use crate::layout::{Allocation, Rect};
use crate::view::controller::{Controller, Event, EventCx, PaintCx};
use crate::view::{BuildCx, EventKind, Handler, Kind, Prop, PropName, Props, View};

/// A `GtkDrawingArea` whose `draw_func` is `draw`.
#[must_use]
pub fn drawing_area<Msg: Clone + 'static>(
    draw: impl Fn(&mut Canvas<'_>, Rect, &mut PaintCx<'_>) + 'static,
) -> View<Msg> {
    View::new(Kind::DrawingArea).prop(PropName::DrawFn, Prop::Draw(Rc::new(draw)))
}

/// `GtkDrawingArea`'s own setters.
pub trait DrawingAreaExt<Msg>: Sized {
    /// `GtkDrawingArea:content-width`.
    fn content_width(self, px: i32) -> Self;
    /// `GtkDrawingArea:content-height`.
    fn content_height(self, px: i32) -> Self;
    /// `GtkDrawingArea::resize`, carrying the new width.
    fn on_resize(self, f: impl Fn(usize) -> Msg + 'static) -> Self;
}

impl<Msg: Clone + 'static> DrawingAreaExt<Msg> for View<Msg> {
    fn content_width(self, px: i32) -> Self {
        self.prop(PropName::WidthRequest, Prop::Int(i64::from(px)))
    }
    fn content_height(self, px: i32) -> Self {
        self.prop(PropName::HeightRequest, Prop::Int(i64::from(px)))
    }
    fn on_resize(self, f: impl Fn(usize) -> Msg + 'static) -> Self {
        self.on(EventKind::Change, Handler::Index(Rc::new(f)))
    }
}

/// `Kind::DrawingArea`'s controller.
pub struct DrawingAreaC {
    /// The application's paint callback.
    #[allow(
        clippy::type_complexity,
        reason = "the contract's own Prop::Draw signature; a type alias would only hide it"
    )]
    pub draw: Rc<dyn Fn(&mut Canvas<'_>, Rect, &mut PaintCx<'_>)>,
    /// `(content-width, content-height)`, the intrinsic size.
    pub content: (i32, i32),
    /// The last allocation the callback saw, for the resize signal.
    pub last_size: (f32, f32),
}

impl<Msg: Clone + 'static> Controller<Msg> for DrawingAreaC {
    fn kind(&self) -> Kind {
        Kind::DrawingArea
    }

    fn build(_node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        DrawingAreaC {
            draw: match props.get(PropName::DrawFn) {
                Some(Prop::Draw(f)) => Rc::clone(f),
                _ => Rc::new(|_, _, _| {}),
            },
            content: (
                i32::try_from(props.int(PropName::WidthRequest, 0)).unwrap_or(0),
                i32::try_from(props.int(PropName::HeightRequest, 0)).unwrap_or(0),
            ),
            last_size: (0.0, 0.0),
        }
    }

    fn set_prop(&mut self, _node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        match (name, value) {
            (PropName::DrawFn, Prop::Draw(f)) => self.draw = Rc::clone(f),
            (PropName::WidthRequest, Prop::Int(px)) => {
                self.content.0 = i32::try_from(*px).unwrap_or(0);
            }
            (PropName::HeightRequest, Prop::Int(px)) => {
                self.content.1 = i32::try_from(*px).unwrap_or(0);
            }
            _ => {}
        }
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        if let Event::Configure { size, .. } = ev {
            let next = (size.0 as f32, size.1 as f32);
            if next != self.last_size {
                self.last_size = next;
                if let Some(msg) = cx.handlers.fire_index(EventKind::Change, size.0 as usize) {
                    return vec![msg];
                }
            }
        }
        Vec::new()
    }

    fn measure(
        &mut self,
        _available: (Option<f32>, Option<f32>),
        _cx: &mut BuildCx<'_>,
    ) -> Option<(f32, f32)> {
        Some((self.content.0.max(0) as f32, self.content.1.max(0) as f32))
    }

    fn paint(
        &mut self,
        canvas: &mut Canvas<'_>,
        alloc: &Allocation,
        _style: &ComputedStyle,
        cx: &mut PaintCx<'_>,
    ) -> bool {
        let content = alloc.content_box;
        if content.is_empty() {
            return false;
        }
        self.last_size = (content.width, content.height);
        // Cloned out of `self` first: the callback borrows nothing of the
        // controller, and holding `&self.draw` across the call would conflict
        // with the `&mut self` this method holds.
        let draw = Rc::clone(&self.draw);
        draw(canvas, content, cx);
        true
    }
}
