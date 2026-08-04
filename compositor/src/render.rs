//! Wallpaper decode, scene ordering, and the render glue that wires
//! [`crate::backend`]'s redraw path through smithay's damage-tracked
//! renderer.
//!
//! The pure/tested half of this module (`SceneLayer`, `scene_order`,
//! `wallpaper_color`, `hex_to_rgba`) is deliberately renderer-agnostic so it
//! can be exercised without a GL context. The glue half (`draw_frame`,
//! `WallpaperState`, `spawn_wallpaper_decode`) is concrete over
//! [`GlesRenderer`] rather than generic over `R: Renderer` -- `backend.rs`
//! is already fully concrete on `GlesRenderer` (its winit bind call returns
//! `(&mut GlesRenderer, _)` directly), so a generic `draw_frame` would only
//! add bound-juggling with no caller that needs the extra generality.
//!
//! **Deviation from the task-8 brief's threading-model wording:** the brief
//! says the decoded wallpaper is "shipped ... over a crossbeam channel
//! registered as a calloop source." A plain `crossbeam_channel::Receiver`
//! has no raw fd/mio source to hand to `calloop::Generic`, so it cannot
//! literally be registered as a calloop source without extra plumbing (e.g.
//! pairing it with a separate eventfd). `calloop::channel` is smithay's own
//! purpose-built primitive for exactly this "worker thread result delivered
//! into the main loop" shape -- its `Channel<T>` *is* an `EventSource`
//! directly -- and it matches the documented intent for Task 13's (not yet
//! built) async config reload, per `main.rs`'s comment there: "a
//! `calloop::channel` source draining a worker-thread result onto the main
//! loop." `spawn_wallpaper_decode` below follows that same shape instead of
//! the brief's literal wording. The invariant the brief actually cares
//! about -- CPU-heavy decode runs off the render loop, result delivered to
//! the main loop without shared mutable state -- is preserved exactly.

use icedtea_contract::{Appearance, Rectangle};
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::damage::{
    Error as DamageTrackerError, OutputDamageTracker, RenderOutputResult,
};
use smithay::backend::renderer::element::solid::{SolidColorBuffer, SolidColorRenderElement};
use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::element::texture::{TextureBuffer, TextureRenderElement};
use smithay::backend::renderer::element::{Kind, Wrap};
use smithay::backend::renderer::gles::{GlesError, GlesRenderer, GlesTexture};
use smithay::backend::renderer::RendererSuper;
use smithay::desktop::space::{space_render_elements, SpaceRenderElements};
use smithay::desktop::{Space, Window as DesktopWindow};
use smithay::output::Output;
use smithay::utils::{Logical, Point, Size, Transform};

use crate::decoration;
use crate::window::Window;

/// Z-order layers in a frame, back to front. Used by `scene_order` (pure,
/// tested) and mirrored by the element ordering `draw_frame` builds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SceneLayer {
    Wallpaper,
    Windows,
    SnapPreview,
}

/// Returns the layers present in a frame, back to front. `Windows` is
/// omitted when there are none to draw; `SnapPreview` is present only while
/// a drag is showing a snap target.
pub fn scene_order(windows: &[&Window], snap_active: bool) -> Vec<SceneLayer> {
    let mut order = vec![SceneLayer::Wallpaper];
    if !windows.is_empty() {
        order.push(SceneLayer::Windows);
    }
    if snap_active {
        order.push(SceneLayer::SnapPreview);
    }
    order
}

/// The SSD title-bar strip's geometry, one rectangle per window, computed
/// via `crate::decoration::title_bar_rect`. This is the "geometry hook" the
/// brief's Step 3 calls for -- real geometry, derived from each window's
/// real position/size -- paired with the `CustomRenderElements::Decoration`
/// element slot below. Task 10 owns turning this geometry (plus
/// `crate::decoration::button_rects` and whatever SSD styling it designs)
/// into actual painted `Decoration` elements in `draw_frame`; this task only
/// wires the plumbing; the slot is real but stays unpopulated here.
pub fn decoration_strip_geometry(windows: &[&Window]) -> Vec<Rectangle> {
    windows.iter().map(|w| decoration::title_bar_rect(w.geometry)).collect()
}

/// The solid wallpaper color: `appearance.palette.background` converted to
/// RGBA. Used as the frame's clear color, and as the fallback until (or
/// unless) `appearance.wallpaper`'s image has finished decoding.
pub fn wallpaper_color(appearance: &Appearance) -> [f32; 4] {
    hex_to_rgba(&appearance.palette.background)
}

pub fn hex_to_rgba(hex: &str) -> [f32; 4] {
    let hex = hex.trim_start_matches('#');
    let v = u32::from_str_radix(hex, 16).unwrap_or(0x000000);
    let r = ((v >> 16) & 0xff) as f32 / 255.0;
    let g = ((v >> 8) & 0xff) as f32 / 255.0;
    let b = (v & 0xff) as f32 / 255.0;
    [r, g, b, 1.0]
}

// --- Renderer glue -------------------------------------------------------

smithay::backend::renderer::element::render_elements! {
    /// Non-window elements drawn each frame: the (optional) wallpaper
    /// texture, the (optional) translucent snap-preview rectangle, and the
    /// SSD title-bar strip slot (unpopulated until Task 10 designs what a
    /// decorated strip looks like -- see `decoration_strip_geometry`).
    pub CustomRenderElements<=GlesRenderer>;
    Solid=SolidColorRenderElement,
    Texture=TextureRenderElement<GlesTexture>,
    // `Wrap<...>` (rather than a second bare `SolidColorRenderElement`
    // variant) because the macro generates one `From<FieldType>` impl per
    // variant, and two variants with an identical field type collide.
    Decoration=Wrap<SolidColorRenderElement>,
}

smithay::backend::renderer::element::render_elements! {
    /// The full set of elements `draw_frame` hands to the damage tracker,
    /// combining `space`'s mapped windows with our custom elements.
    pub OutputRenderElements<=GlesRenderer>;
    Space=SpaceRenderElements<GlesRenderer, WaylandSurfaceRenderElement<GlesRenderer>>,
    Custom=CustomRenderElements,
}

/// Bound of 1: only the most recently decoded image matters, and the main
/// loop drains promptly on every event-loop pass. The bound exists so a
/// wedged consumer can't let a runaway producer pile up allocations, not
/// because backpressure is expected in practice.
const WALLPAPER_CHANNEL_BOUND: usize = 1;

/// Spawns a worker thread that decodes `path` (if any) via the `image`
/// crate into an RGBA buffer and ships the result back over a
/// `calloop::channel` (see the module-level deviation note for why this is
/// `calloop::channel` rather than a bare `crossbeam_channel::Receiver`).
/// A `None` path, or a decode failure, both resolve to `None` -- the solid
/// wallpaper color remains correct in either case, and the receiver always
/// gets exactly one message either way.
///
/// Decoding a multi-megapixel image is CPU-heavy; keeping it on a worker
/// thread (per the binding threading model) keeps the render loop from
/// stalling on it. The worker only decodes CPU-side -- GL texture upload
/// happens in `WallpaperState::upload_pending`, called from `draw_frame` on
/// the render thread, per the same threading model.
pub fn spawn_wallpaper_decode(
    path: Option<String>,
) -> smithay::reexports::calloop::channel::Channel<Option<image::RgbaImage>> {
    let (tx, channel) = smithay::reexports::calloop::channel::sync_channel(WALLPAPER_CHANNEL_BOUND);
    std::thread::spawn(move || {
        let decoded = path.as_deref().and_then(|p| match image::open(p) {
            Ok(img) => Some(img.to_rgba8()),
            Err(err) => {
                tracing::warn!(path = %p, "failed to decode wallpaper image: {err}");
                None
            }
        });
        let _ = tx.send(decoded);
    });
    channel
}

/// Holds the wallpaper image pipeline's cross-thread state: a decoded
/// buffer waiting for GL upload, and the uploaded texture (once any). No
/// state here is touched from more than one thread -- `set_decoded` is
/// called from the calloop channel callback on the main/render thread, and
/// `upload_pending`/`texture_element` run from `draw_frame` on the render
/// thread.
#[derive(Default)]
pub struct WallpaperState {
    pending: Option<image::RgbaImage>,
    texture: Option<TextureBuffer<GlesTexture>>,
}

impl WallpaperState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a freshly decoded image (or a failed/absent decode) as
    /// pending GL upload. Called from the calloop channel handler that
    /// drains `spawn_wallpaper_decode`'s `Channel`.
    pub fn set_decoded(&mut self, image: Option<image::RgbaImage>) {
        self.pending = image;
    }

    /// Upload a pending decode to a GL texture. Must run on the render
    /// thread (it needs the bound `GlesRenderer`). A failed import is
    /// logged and leaves the solid color as the fallback.
    fn upload_pending(&mut self, renderer: &mut GlesRenderer) {
        let Some(image) = self.pending.take() else {
            return;
        };
        let (width, height) = image.dimensions();
        match TextureBuffer::from_memory(
            renderer,
            &image,
            Fourcc::Abgr8888,
            (width as i32, height as i32),
            false,
            1,
            Transform::Normal,
            None,
        ) {
            Ok(buffer) => self.texture = Some(buffer),
            Err(err) => tracing::warn!("failed to upload wallpaper texture: {err:?}"),
        }
    }

    /// The stretched wallpaper texture element, if a texture has been
    /// uploaded. `target_size` is the output's logical size; the texture is
    /// stretched to fill it regardless of its native dimensions, per the
    /// brief ("image, stretched").
    fn texture_element(&self, target_size: Size<i32, Logical>) -> Option<TextureRenderElement<GlesTexture>> {
        self.texture.as_ref().map(|buffer| {
            TextureRenderElement::from_texture_buffer(
                Point::<f64, _>::from((0.0, 0.0)),
                buffer,
                None,
                None,
                Some(target_size),
                Kind::Unspecified,
            )
        })
    }
}

/// Renders one frame: wallpaper (image if decoded, else solid color) behind
/// the space's mapped windows, with an optional translucent snap-preview
/// rectangle on top. Mirrors anvil's `render_output` (`anvil/src/render.rs`)
/// adapted to insert the wallpaper/preview elements around the window
/// elements instead of anvil's pointer/FPS overlay.
///
/// `snap_preview`, when `Some`, is the target zone's geometry in output
/// logical coordinates; full drag-machine wiring (when this is actually
/// populated) lands in Task 9 -- this is the geometry hook the brief asks
/// for.
///
/// `windows` (our own model, geometry in output logical coordinates) feeds
/// `decoration_strip_geometry` -- the SSD element slot/geometry hook. Full
/// SSD rendering (turning that geometry into painted
/// `CustomRenderElements::Decoration` elements) lands in Task 10; this task
/// only computes the geometry and reserves the slot, it doesn't push any
/// `Decoration` elements yet.
#[allow(clippy::too_many_arguments)]
pub fn draw_frame<'d>(
    renderer: &mut GlesRenderer,
    framebuffer: &mut <GlesRenderer as RendererSuper>::Framebuffer<'_>,
    damage_tracker: &'d mut OutputDamageTracker,
    output: &Output,
    space: &Space<DesktopWindow>,
    windows: &[&Window],
    appearance: &Appearance,
    wallpaper: &mut WallpaperState,
    snap_preview: Option<Rectangle>,
    age: usize,
) -> Result<RenderOutputResult<'d>, DamageTrackerError<GlesError>> {
    wallpaper.upload_pending(renderer);

    // SSD element slot's geometry hook (see `decoration_strip_geometry`):
    // computed every frame from real window geometry, ready for Task 10 to
    // turn into `CustomRenderElements::Decoration` elements.
    let decoration_strip_geometry = decoration_strip_geometry(windows);

    let scale = output.current_scale().fractional_scale();
    let output_logical_size = output
        .current_mode()
        .map(|mode| mode.size.to_f64().to_logical(scale).to_i32_round())
        .unwrap_or_default();

    // Elements are listed topmost-first (matches anvil's `OutputRenderElements`
    // ordering, where custom elements added earlier draw on top).
    let mut elements: Vec<OutputRenderElements> = Vec::new();

    if let Some(rect) = snap_preview {
        let mut color = hex_to_rgba(&appearance.palette.accent);
        color[3] = 0.35; // translucent overlay, not an opaque fill
        let buffer = SolidColorBuffer::new((rect.width, rect.height), color);
        let location = Point::<i32, Logical>::from((rect.x, rect.y)).to_physical_precise_round(scale);
        let element = SolidColorRenderElement::from_buffer(&buffer, location, scale, 1.0, Kind::Unspecified);
        elements.push(OutputRenderElements::Custom(CustomRenderElements::Solid(element)));
    }

    let space_elements =
        space_render_elements::<_, DesktopWindow, _>(renderer, [space], output, 1.0)?;
    elements.extend(space_elements.into_iter().map(OutputRenderElements::Space));

    // Render SSD decoration strips for non-CSD, non-fullscreen windows
    let title_bar_color = hex_to_rgba(&appearance.palette.background);
    let button_color = hex_to_rgba(&appearance.palette.accent);
    for (window, title_bar_rect) in windows.iter().zip(decoration_strip_geometry.iter()) {
        // Skip CSD windows and fullscreen windows
        if window.fullscreen || decoration::is_csd(&window.app_id, None) {
            continue;
        }

        // Render title bar background
        let buffer = SolidColorBuffer::new(
            (title_bar_rect.width, decoration::TITLE_BAR_HEIGHT),
            title_bar_color,
        );
        let location = Point::<i32, Logical>::from((title_bar_rect.x, title_bar_rect.y))
            .to_physical_precise_round(scale);
        let title_bar_element = SolidColorRenderElement::from_buffer(&buffer, location, scale, 1.0, Kind::Unspecified);
        elements.push(OutputRenderElements::Custom(CustomRenderElements::Solid(title_bar_element)));

        // Render buttons
        let buttons = decoration::button_rects(window.geometry);
        for button_rect in buttons.iter() {
            let button_buffer = SolidColorBuffer::new(
                (decoration::BUTTON_WIDTH, decoration::TITLE_BAR_HEIGHT),
                button_color,
            );
            let button_location = Point::<i32, Logical>::from((button_rect.x, button_rect.y))
                .to_physical_precise_round(scale);
            let button_element = SolidColorRenderElement::from_buffer(
                &button_buffer,
                button_location,
                scale,
                1.0,
                Kind::Unspecified,
            );
            elements.push(OutputRenderElements::Custom(CustomRenderElements::Solid(button_element)));
        }
    }

    if let Some(texture_element) = wallpaper.texture_element(output_logical_size) {
        elements.push(OutputRenderElements::Custom(CustomRenderElements::Texture(texture_element)));
    }

    let clear_color = wallpaper_color(appearance);
    damage_tracker.render_output(renderer, framebuffer, age, &elements, clear_color)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_window() -> Window {
        Window {
            id: icedtea_contract::WindowId(1),
            app_id: "a".into(),
            title: "t".into(),
            pid: 1,
            workspace: 0,
            geometry: Rectangle { x: 0, y: 0, width: 10, height: 10 },
            maximized: false,
            minimized: false,
            fullscreen: false,
            focused: true,
        }
    }

    #[test]
    fn order_has_wallpaper_first_snap_last() {
        let w = fake_window();
        let order = scene_order(&[&w], true);
        assert_eq!(order[0], SceneLayer::Wallpaper);
        assert_eq!(order.last(), Some(&SceneLayer::SnapPreview));
        assert!(order.contains(&SceneLayer::Windows));
    }

    #[test]
    fn no_windows_skips_window_layer() {
        assert_eq!(scene_order(&[], true), vec![SceneLayer::Wallpaper, SceneLayer::SnapPreview]);
        assert_eq!(scene_order(&[], false), vec![SceneLayer::Wallpaper]);
    }

    #[test]
    fn hex_conversion() {
        assert_eq!(hex_to_rgba("#ff0000"), [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(hex_to_rgba("#1e1e2e"), [30.0 / 255.0, 30.0 / 255.0, 46.0 / 255.0, 1.0]);
    }

    #[test]
    fn decoration_strip_geometry_uses_title_bar_rect() {
        let w = fake_window();
        assert_eq!(decoration_strip_geometry(&[&w]), vec![decoration::title_bar_rect(w.geometry)]);
    }

    #[test]
    fn decoration_strip_geometry_is_empty_with_no_windows() {
        assert_eq!(decoration_strip_geometry(&[]), Vec::<Rectangle>::new());
    }

    #[test]
    fn decoration_element_slot_constructs() {
        // Proves `CustomRenderElements::Decoration` -- the SSD element slot
        // the brief's Step 3 asks for -- actually exists and type-checks.
        // Task 10 is the one that constructs it from real geometry inside
        // `draw_frame`; this only confirms the slot itself is wired.
        let buffer = SolidColorBuffer::new((10, 10), [1.0, 1.0, 1.0, 1.0]);
        let element = SolidColorRenderElement::from_buffer(&buffer, (0, 0), 1.0, 1.0, Kind::Unspecified);
        let slot = CustomRenderElements::Decoration(Wrap::from(element));
        assert!(matches!(slot, CustomRenderElements::Decoration(_)));
    }
}
