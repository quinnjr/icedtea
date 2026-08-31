//! `GtkImage` — `Kind::Image`, CSS node `image`.
//!
//! ```text
//! image[.normal-icons][.large-icons]
//! ```
//!
//! An image is an `IconRef` resolved through the icon theme. P7 owns the
//! resolution (`IconTheme::render`, contract §9, plan D9); until it lands
//! `resolved` stays `None` and the node paints only its CSS box.

use std::path::Path;
use std::rc::Rc;

use skia_rs_safe::canvas::Canvas;
use skia_rs_safe::paint::Paint;

use crate::css::computed::ComputedStyle;
use crate::css::node::Node;
use crate::css::value::image::IconRef;
use crate::layout::{Allocation, Rect};
use crate::view::controller::{Controller, Event, EventCx, PaintCx};
use crate::view::{BuildCx, Kind, Prop, PropName, Props, View};
use crate::widgets::{IconSize, WidgetEnum};

/// A `GtkImage` showing `icon`.
#[must_use]
pub fn image<Msg: Clone + 'static>(icon: IconRef) -> View<Msg> {
    View::new(Kind::Image).prop(PropName::Icon, Prop::Icon(icon))
}

/// A `GtkImage` showing the themed icon `name`.
#[must_use]
pub fn image_named<Msg: Clone + 'static>(name: &str) -> View<Msg> {
    image(IconRef::Theme {
        name: Rc::from(name),
    })
}

/// `GtkImage`'s own setters.
pub trait ImageExt<Msg>: Sized {
    /// `GtkImage:icon-name`.
    fn icon_name(self, name: &str) -> Self;
    /// `GtkImage:file`.
    fn file(self, path: &Path) -> Self;
    /// `GtkImage:pixel-size`.
    fn pixel_size(self, px: i32) -> Self;
    /// `GtkImage:icon-size`.
    fn icon_size(self, size: IconSize) -> Self;
    /// `GtkImage:use-fallback`.
    fn use_fallback(self, on: bool) -> Self;
}

impl<Msg: Clone + 'static> ImageExt<Msg> for View<Msg> {
    fn icon_name(self, name: &str) -> Self {
        self.prop(
            PropName::Icon,
            Prop::Icon(IconRef::Theme {
                name: Rc::from(name),
            }),
        )
    }
    fn file(self, path: &Path) -> Self {
        self.prop(
            PropName::Icon,
            Prop::Icon(IconRef::Recolor {
                url: Rc::from(path.to_string_lossy().as_ref()),
                palette: None,
            }),
        )
    }
    fn pixel_size(self, px: i32) -> Self {
        self.prop(PropName::IconSize, Prop::Int(i64::from(px)))
    }
    fn icon_size(self, size: IconSize) -> Self {
        self.prop(PropName::IconSize, size.to_prop())
    }
    fn use_fallback(self, on: bool) -> Self {
        self.prop(PropName::Fit, Prop::Bool(on))
    }
}

/// `Kind::Image`'s controller.
pub struct ImageC {
    /// What to draw.
    pub icon: IconRef,
    /// The rasterized icon, once P7's theme can produce one.
    pub resolved: Option<Rc<skia_rs_safe::codec::Image>>,
    /// Requested size in px; 16 when unset, as GTK's `normal` icon size is.
    pub pixel_size: i32,
    size_class: IconSize,
}

impl ImageC {
    fn apply(&self, node: &Node) {
        for candidate in IconSize::all() {
            let class = candidate.css_class();
            if !class.is_empty() {
                node.remove_class(class);
            }
        }
        let class = self.size_class.css_class();
        if !class.is_empty() {
            node.add_class(class);
        }
    }

    /// Resolve `self.icon` through the icon theme.
    ///
    /// `IconTheme::render` is P7's (contract §9, plan D9); `cx` is threaded
    /// through today only so a future P7 fill-in is a body change here, not a
    /// signature change at every call site.
    fn resolve(&mut self, _cx: &mut BuildCx<'_>) {
        self.resolved = None;
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for ImageC {
    fn kind(&self) -> Kind {
        Kind::Image
    }

    fn build(node: &Node, props: &Props, cx: &mut BuildCx<'_>) -> Self {
        let icon = match props.get(PropName::Icon) {
            Some(Prop::Icon(icon)) => icon.clone(),
            _ => IconRef::Theme {
                name: Rc::from("image-missing"),
            },
        };
        let (pixel_size, size_class) = match props.get(PropName::IconSize) {
            Some(Prop::Int(px)) => (
                i32::try_from(*px).unwrap_or(16).clamp(1, 512),
                IconSize::Inherit,
            ),
            Some(Prop::Enum(_)) => {
                let size = IconSize::from_prop(props.get(PropName::IconSize), IconSize::Inherit);
                (
                    match size {
                        IconSize::Large => 32,
                        _ => 16,
                    },
                    size,
                )
            }
            _ => (16, IconSize::Inherit),
        };
        let mut this = ImageC {
            icon,
            resolved: None,
            pixel_size,
            size_class,
        };
        this.apply(node);
        this.resolve(cx);
        this
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, cx: &mut BuildCx<'_>) {
        match (name, value) {
            (PropName::Icon, Prop::Icon(icon)) => self.icon = icon.clone(),
            (PropName::IconSize, Prop::Int(px)) => {
                self.pixel_size = i32::try_from(*px).unwrap_or(16).clamp(1, 512);
                self.size_class = IconSize::Inherit;
            }
            (PropName::IconSize, Prop::Enum(_)) => {
                self.size_class = IconSize::from_prop(Some(value), self.size_class);
                self.pixel_size = match self.size_class {
                    IconSize::Large => 32,
                    _ => 16,
                };
            }
            _ => return,
        }
        self.apply(node);
        self.resolve(cx);
    }

    fn on_event(&mut self, _ev: &Event, _cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        Vec::new()
    }

    fn measure(
        &mut self,
        _available: (Option<f32>, Option<f32>),
        _cx: &mut BuildCx<'_>,
    ) -> Option<(f32, f32)> {
        let size = self.pixel_size as f32;
        Some((size, size))
    }

    fn paint(
        &mut self,
        canvas: &mut Canvas<'_>,
        alloc: &Allocation,
        _style: &ComputedStyle,
        _cx: &mut PaintCx<'_>,
    ) -> bool {
        let Some(icon) = self.resolved.as_ref() else {
            return false;
        };
        let content = alloc.content_box;
        let size = content.width.min(content.height);
        let rect = Rect::new(
            content.x + (content.width - size) / 2.0,
            content.y + (content.height - size) / 2.0,
            size,
            size,
        );
        canvas.draw_image_rect(icon, None, &rect.to_skia(), Some(&Paint::new()));
        true
    }
}
