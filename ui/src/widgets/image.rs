//! `GtkImage` — `Kind::Image`, CSS node `image`.
//!
//! ```text
//! image[.normal-icons][.large-icons]
//! ```
//!
//! An image is an `IconRef` resolved through the icon theme
//! (`IconTheme::render`) and drawn through `paint::icon::paint_icon`, the
//! single entry point contract §6 names.

use std::path::Path;
use std::rc::Rc;

use skia_rs_safe::canvas::Canvas;

use crate::css::computed::ComputedStyle;
use crate::css::node::Node;
use crate::css::value::Rgba;
use crate::css::value::image::IconRef;
use crate::icons::Palette;
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
    /// The rasterised icon, prefetched at build time and refreshed by the
    /// last paint (which is the first moment the node's `color` — and so its
    /// symbolic palette — is known).
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
    /// A build-time prefetch: it warms `IconTheme`'s lookup and rasterisation
    /// caches (and tells `measure` whether there is anything to show) using
    /// the default palette, because a node's computed `color` does not exist
    /// until the restyle that follows. `paint` re-resolves against the real
    /// palette; both hit the same memo table, so the second resolve is a hash
    /// probe.
    fn resolve(&mut self, cx: &mut BuildCx<'_>) {
        let size = self.pixel_size.clamp(1, 512) as u32;
        let scale = cx.icons.scale();
        let palette = Palette::for_color(Rgba {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        });
        self.resolved = match &self.icon {
            IconRef::Theme { name } => {
                let symbolic = name.ends_with("-symbolic");
                cx.icons.render(name, size, scale, symbolic, &palette)
            }
            IconRef::Recolor { url, .. } => {
                let path = std::path::PathBuf::from(url.as_ref());
                cx.icons.render_path(path, size, scale, true, &palette)
            }
            // `-gtk-scaled()` picks its arm from the output scale, which is a
            // paint-time decision; nothing is prefetched for one.
            IconRef::Scaled { .. } => None,
        };
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
        style: &ComputedStyle,
        cx: &mut PaintCx<'_>,
    ) -> bool {
        let content = alloc.content_box;
        // The requested pixel size, clipped to what was actually allocated:
        // GTK never stretches an icon past its box.
        let size = (self.pixel_size.clamp(1, 512) as f32)
            .min(content.width)
            .min(content.height);
        if !(size.is_finite() && size > 0.0) {
            return false;
        }
        let rect = Rect::new(
            content.x + (content.width - size) / 2.0,
            content.y + (content.height - size) / 2.0,
            size,
            size,
        );
        // Re-resolve against the node's real palette before drawing, so
        // `resolved` is what was last shown rather than what the build-time
        // prefetch guessed, and so a symbolic icon follows `color`.
        #[allow(
            clippy::cast_sign_loss,
            clippy::cast_possible_truncation,
            reason = "`size` is finite, positive and at most 512 by the clamp above"
        )]
        let px = size as u32;
        self.resolved = crate::paint::icon::resolve_icon(&self.icon, px, style, cx);
        if self.resolved.is_none() {
            return false;
        }
        crate::paint::icon::paint_icon(canvas, &self.icon, rect, style, cx);
        true
    }
}
