//! Wallpaper decode, scene ordering, and the colours the scene is painted
//! with.
//!
//! Everything here is renderer-agnostic and unit-testable: `SceneLayer`,
//! `scene_order`, `decoration_strip_geometry`, `wallpaper_color` and
//! `hex_to_rgba` are pure, and `WallpaperState`/`spawn_wallpaper_decode` are
//! a worker thread and a decoded image. There is no draw call anywhere in
//! this file, and there will not be one: the compositor renders through a
//! retained scene graph that the compositor library damages and commits
//! itself, so "how a frame is drawn" is not this crate's decision to make.
//! What this crate still decides is *what colour* and *in what order*, which
//! is what remains here.

use std::os::unix::net::UnixStream;

use icedtea_contract::{Appearance, Rectangle};

use crate::decoration;
use crate::window::Window;

/// Z-order layers in a frame, back to front. Used by `scene_order` (pure,
/// tested) and mirrored by the element ordering the compositor library's
/// scene graph is built in.
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
/// via `crate::decoration::title_bar_rect`. Real geometry, derived from each
/// window's real position/size, for whatever scene-graph layer paints the
/// decoration strip.
///
/// These rects are **frame**-relative -- `w.geometry` is the whole frame, and
/// the strip is its top `TITLE_BAR_HEIGHT` rows. Re-review finding New-4: an
/// SSD client is now configured and mapped at
/// `decoration::content_rect(w.geometry, true)`, which starts exactly where
/// these strips end, so the two no longer overlap. Anything that changes one
/// side of that has to change the other.
pub fn decoration_strip_geometry(windows: &[&Window]) -> Vec<Rectangle> {
    windows.iter().map(|w| decoration::title_bar_rect(w.geometry)).collect()
}

/// The buffer-node destination rect for an output: stretch to fill.
///
/// Deviation 4 (design spec): there is no scaling-mode field anywhere in the
/// model or config, so "stretch to fill the output" is the only behaviour
/// this crate implements -- the destination rect is simply the output's own
/// rect, unconditionally.
pub fn wallpaper_dest(output: Rectangle) -> Rectangle {
    output
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

/// Bound of 1: only the most recently decoded image matters, and the main
/// loop drains promptly on every event-loop pass. The bound exists so a
/// wedged consumer can't let a runaway producer pile up allocations, not
/// because backpressure is expected in practice.
const WALLPAPER_CHANNEL_BOUND: usize = 1;

/// Spawns a worker thread that decodes `path` (if any) via the `image`
/// crate into an RGBA buffer and ships the result back over a
/// `crossbeam_channel`. A `None` path, or a decode failure, both resolve to
/// `None` -- the solid wallpaper color remains correct in either case, and
/// the receiver always gets exactly one message either way.
///
/// Decoding a multi-megapixel image is CPU-heavy; keeping it on a worker
/// thread (per the binding threading model) keeps the caller's loop from
/// stalling on it.
///
/// `wake`, when given, is nudged (`backend::wake`) after the result is sent
/// on `tx` -- the same wake-pipe treatment `State::spawn_config_reload`
/// gives the config-reload channel, so a loop blocked in `Until::Stop`'s
/// `dispatch(-1)` sees the decode result the moment it arrives instead of
/// only once some unrelated event wakes it (or, on an otherwise idle
/// compositor, not until shutdown). `None` (no caller wires one) degrades to
/// the pre-wake-pipe behavior: the result sits on the channel until
/// something else wakes the loop.
pub fn spawn_wallpaper_decode(
    path: Option<String>,
    wake: Option<UnixStream>,
) -> crossbeam_channel::Receiver<Option<image::RgbaImage>> {
    let (tx, rx) = crossbeam_channel::bounded(WALLPAPER_CHANNEL_BOUND);
    std::thread::spawn(move || {
        let decoded = path.as_deref().and_then(|p| match image::open(p) {
            Ok(img) => Some(img.to_rgba8()),
            Err(err) => {
                tracing::warn!(path = %p, "failed to decode wallpaper image: {err}");
                None
            }
        });
        let _ = tx.send(decoded);
        if let Some(wake) = wake {
            crate::backend::wake(&wake);
        }
    });
    rx
}

/// Holds the wallpaper image pipeline's state: the most recently decoded
/// buffer, if any. There is no GPU upload here -- that belongs to whatever
/// renderer the compositor library drives, which lands with task 5.
#[derive(Default)]
pub struct WallpaperState {
    decoded: Option<image::RgbaImage>,
}

impl WallpaperState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a freshly decoded image (or a failed/absent decode). Called
    /// once `spawn_wallpaper_decode`'s channel has a result.
    pub fn set_decoded(&mut self, image: Option<image::RgbaImage>) {
        self.decoded = image;
    }

    /// The most recently decoded wallpaper image, if any.
    pub fn decoded(&self) -> Option<&image::RgbaImage> {
        self.decoded.as_ref()
    }
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
            mapped: true,
            client_decorations_requested: None,
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
    fn wallpaper_dest_is_the_full_output() {
        let out = Rectangle { x: 0, y: 0, width: 1920, height: 1080 };
        assert_eq!(wallpaper_dest(out), out);
    }

    #[test]
    fn wallpaper_state_starts_empty_and_records_a_decode() {
        let mut w = WallpaperState::new();
        assert!(w.decoded().is_none());
        let image = image::RgbaImage::new(1, 1);
        w.set_decoded(Some(image.clone()));
        assert_eq!(w.decoded(), Some(&image));
    }
}
