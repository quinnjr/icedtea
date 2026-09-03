//! `GtkPicture` — `Kind::Picture`, CSS node `picture`.
//!
//! ```text
//! picture
//! ```
//!
//! A picture is a decoded image file, not a themed icon: it goes through
//! `skia_rs_safe::codec` directly. Decoding is untrusted input — a truncated or
//! hostile file yields `None`, logged once, and the node paints only its box.

use std::path::Path;
use std::rc::Rc;
use std::sync::Once;

use skia_rs_safe::canvas::Canvas;
use skia_rs_safe::paint::Paint;

use crate::css::computed::ComputedStyle;
use crate::css::node::Node;
use crate::layout::{Allocation, Rect};
use crate::view::controller::{Controller, Event, EventCx, PaintCx};
use crate::view::{BuildCx, Kind, Prop, PropName, Props, View};
use crate::widgets::{ContentFit, PictureSource, WidgetEnum};

static DECODE_WARNED: Once = Once::new();

// A single shared table: `encode_bytes_key` and `decode_bytes_key` must see
// the same entries, which two separate `thread_local!` items (even with the
// same name, in different functions) would not (plan reconciliation).
thread_local! {
    static BYTES_TABLE: std::cell::RefCell<Vec<&'static [u8]>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// A `GtkPicture` showing the image file at `path`.
#[must_use]
pub fn picture<Msg: Clone + 'static>(path: &Path) -> View<Msg> {
    View::new(Kind::Picture).prop(
        PropName::Paintable,
        Prop::Str(Rc::from(path.to_string_lossy().as_ref())),
    )
}

/// A `GtkPicture` over already-loaded encoded bytes, for tests and embedded
/// assets.
#[must_use]
pub fn picture_from_bytes<Msg: Clone + 'static>(bytes: &'static [u8]) -> View<Msg> {
    View::new(Kind::Picture).prop(
        PropName::Paintable,
        Prop::Classes(Rc::from(vec![Rc::from(encode_bytes_key(bytes).as_str())])),
    )
}

/// Register `bytes` in the process-wide byte-source table and return its key.
///
/// `Prop` has no byte-slice variant and must stay `PartialEq`-cheap, so an
/// embedded image is addressed by a stable key derived from its pointer and
/// length. The table only ever grows by one entry per distinct `&'static [u8]`.
fn encode_bytes_key(bytes: &'static [u8]) -> String {
    BYTES_TABLE.with(|table| {
        let mut table = table.borrow_mut();
        let index = table
            .iter()
            .position(|entry| std::ptr::eq(*entry, bytes))
            .unwrap_or_else(|| {
                table.push(bytes);
                table.len() - 1
            });
        format!("bytes:{index}")
    })
}

/// Look a key produced by [`encode_bytes_key`] back up.
fn decode_bytes_key(key: &str) -> Option<&'static [u8]> {
    let index: usize = key.strip_prefix("bytes:")?.parse().ok()?;
    BYTES_TABLE.with(|table| table.borrow().get(index).copied())
}

/// `GtkPicture`'s own setters.
pub trait PictureExt<Msg>: Sized {
    /// `GtkPicture:content-fit`.
    fn content_fit(self, fit: ContentFit) -> Self;
    /// `GtkPicture:can-shrink`.
    fn can_shrink(self, on: bool) -> Self;
    /// `GtkPicture:alternative-text`.
    fn alternative_text(self, text: &str) -> Self;
}

impl<Msg: Clone + 'static> PictureExt<Msg> for View<Msg> {
    fn content_fit(self, fit: ContentFit) -> Self {
        self.prop(PropName::Fit, fit.to_prop())
    }
    fn can_shrink(self, on: bool) -> Self {
        self.prop(PropName::Selectable, Prop::Bool(on))
    }
    fn alternative_text(self, text: &str) -> Self {
        self.prop(PropName::Tooltip, Prop::Str(Rc::from(text)))
    }
}

/// `Kind::Picture`'s controller.
pub struct PictureC {
    /// Where the pixels came from.
    pub source: PictureSource,
    /// The decoded image, `None` when the source is missing or undecodable.
    pub decoded: Option<Rc<skia_rs_safe::codec::Image>>,
    /// How the image fills the widget.
    pub fit: ContentFit,
}

impl PictureC {
    /// Decode `source`, never panicking. A failure logs once and yields `None`.
    fn decode(source: &PictureSource) -> Option<Rc<skia_rs_safe::codec::Image>> {
        let bytes: Vec<u8> = match source {
            PictureSource::None => return None,
            PictureSource::File(path) => std::fs::read(path.as_ref()).ok()?,
            PictureSource::Bytes(bytes) => bytes.to_vec(),
        };
        match skia_rs_safe::codec::decode_image(&bytes) {
            Ok(image) => Some(Rc::new(image)),
            Err(_) => {
                DECODE_WARNED.call_once(|| {
                    tracing::warn!("a picture source could not be decoded; nothing is drawn");
                });
                None
            }
        }
    }

    fn source_from(props: &Props) -> PictureSource {
        match props.get(PropName::Paintable) {
            Some(Prop::Str(path)) => PictureSource::File(Rc::from(Path::new(path.as_ref()))),
            Some(Prop::Classes(keys)) => keys
                .first()
                .and_then(|key| decode_bytes_key(key))
                .map_or(PictureSource::None, |bytes| {
                    PictureSource::Bytes(Rc::from(bytes))
                }),
            _ => PictureSource::None,
        }
    }

    /// The destination rect for `fit` inside `content`.
    fn dest(&self, content: Rect, image_w: f32, image_h: f32) -> Rect {
        if image_w <= 0.0 || image_h <= 0.0 || content.is_empty() {
            return Rect::zero();
        }
        let scale = match self.fit {
            ContentFit::Fill => {
                return content;
            }
            ContentFit::Contain => (content.width / image_w).min(content.height / image_h),
            ContentFit::Cover => (content.width / image_w).max(content.height / image_h),
            ContentFit::ScaleDown => (content.width / image_w)
                .min(content.height / image_h)
                .min(1.0),
        };
        let (w, h) = (image_w * scale, image_h * scale);
        Rect::new(
            content.x + (content.width - w) / 2.0,
            content.y + (content.height - h) / 2.0,
            w,
            h,
        )
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for PictureC {
    fn kind(&self) -> Kind {
        Kind::Picture
    }

    fn build(_node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        let source = Self::source_from(props);
        PictureC {
            decoded: Self::decode(&source),
            source,
            fit: ContentFit::from_prop(props.get(PropName::Fit), ContentFit::Contain),
        }
    }

    fn set_prop(&mut self, _node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        match name {
            PropName::Paintable => {
                let mut props = Props::default();
                props.set(PropName::Paintable, value.clone());
                self.source = Self::source_from(&props);
                self.decoded = Self::decode(&self.source);
            }
            PropName::Fit => self.fit = ContentFit::from_prop(Some(value), self.fit),
            _ => {}
        }
    }

    fn on_event(&mut self, _ev: &Event, _cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        Vec::new()
    }

    fn measure(
        &mut self,
        _available: (Option<f32>, Option<f32>),
        _cx: &mut BuildCx<'_>,
    ) -> Option<(f32, f32)> {
        self.decoded
            .as_ref()
            .map(|image| (image.width() as f32, image.height() as f32))
    }

    fn paint(
        &mut self,
        canvas: &mut Canvas<'_>,
        alloc: &Allocation,
        _style: &ComputedStyle,
        _cx: &mut PaintCx<'_>,
    ) -> bool {
        let Some(image) = self.decoded.as_ref() else {
            return false;
        };
        let dest = self.dest(
            alloc.content_box,
            image.width() as f32,
            image.height() as f32,
        );
        if dest.is_empty() {
            return false;
        }
        canvas.draw_image_rect(image, None, &dest.to_skia(), Some(&Paint::new()));
        true
    }
}
