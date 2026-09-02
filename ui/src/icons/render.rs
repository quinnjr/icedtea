//! Rasterising one located icon file at a requested pixel size.
//!
//! SVG goes through `render_svg_in_container`, which maps the document's
//! `viewBox` into the target box via `preserveAspectRatio` without
//! distorting it — GTK's icon-scaling contract exactly. PNG goes through
//! `decode_image` and `draw_image_rect`.
//!
//! Every failure is `None`, logged once per path: an icon file is untrusted
//! input, and a theme with one corrupt PNG in it must draw `image-missing`
//! there and everything else normally.

use std::cell::RefCell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use skia_rs_safe::canvas::Surface;
use skia_rs_safe::codec::{Image, decode_image};
use skia_rs_safe::core::{Color, Rect};
use skia_rs_safe::paint::Paint;
use skia_rs_safe::svg::{SvgDom, render_svg_in_container};

use super::theme::{IconFile, Palette};
use super::{IconFormat, MAX_ICON_PX, symbolic};

thread_local! {
    /// Paths already complained about, so a broken icon in a hot paint path
    /// logs once rather than every frame.
    static LOGGED: RefCell<HashSet<PathBuf>> = RefCell::new(HashSet::new());
}

/// The most distinct paths remembered as already-logged.
const MAX_LOGGED: usize = 4096;

/// Log `message` for `path`, once per path per thread.
pub(crate) fn log_once(path: &Path, message: &'static str) {
    LOGGED.with(|logged| {
        let mut logged = logged.borrow_mut();
        if logged.len() >= MAX_LOGGED {
            logged.clear();
        }
        if logged.insert(path.to_path_buf()) {
            tracing::debug!(path = %path.display(), "{message}");
        }
    });
}

/// The device pixel size an icon is rasterised at: `size * scale`, clamped
/// into `1..=MAX_ICON_PX`.
///
/// Both inputs come from CSS, where `-gtk-icon-size: 0` and
/// `-gtk-icon-size: 1e9px` are legal declarations.
#[must_use]
pub fn pixel_size(size: u32, scale: u32) -> i32 {
    let px = u64::from(size.max(1)) * u64::from(scale.max(1));
    px.clamp(1, u64::from(MAX_ICON_PX)) as i32
}

/// Rasterise an already-parsed document.
///
/// The parse is the expensive half and is cached by
/// `IconTheme::dom_for`; the recolour clones it per palette.
#[must_use]
pub fn render_dom(dom: &SvgDom, px: i32, symbolic_file: bool, palette: &Palette) -> Option<Image> {
    let recoloured;
    let dom = if symbolic_file {
        recoloured = symbolic::recolour(dom, palette);
        &recoloured
    } else {
        dom
    };
    let mut surface = Surface::new_raster_n32_premul(px, px)?;
    {
        let mut canvas = surface.canvas();
        canvas.clear(Color::TRANSPARENT);
        render_svg_in_container(dom, &mut canvas, px as f32, px as f32);
    }
    surface.make_image_snapshot()
}

/// Render an SVG file into a `px × px` transparent surface.
fn render_svg_file(path: &Path, px: i32, symbolic_file: bool, palette: &Palette) -> Option<Image> {
    let bytes = std::fs::read(path)
        .map_err(|_| log_once(path, "icon file could not be read"))
        .ok()?;
    // `parse_svg` takes `&str`; an icon file need not be valid UTF-8, and a
    // lossy read keeps every ASCII tag and attribute intact.
    let text = String::from_utf8_lossy(&bytes);
    let dom = super::svg_parse_logged(path, &text)?;
    render_dom(&dom, px, symbolic_file, palette)
}

/// Decode a raster file and resample it to `px × px` if it is not already.
fn render_raster_file(path: &Path, px: i32) -> Option<Image> {
    let bytes = std::fs::read(path)
        .map_err(|_| log_once(path, "icon file could not be read"))
        .ok()?;
    let image = decode_image(&bytes)
        .map_err(|_| log_once(path, "icon image could not be decoded"))
        .ok()?;
    if image.width() == px && image.height() == px {
        return Some(image);
    }
    let mut surface = Surface::new_raster_n32_premul(px, px)?;
    {
        let mut canvas = surface.canvas();
        canvas.clear(Color::TRANSPARENT);
        let paint = Paint::new();
        canvas.draw_image_rect(
            &image,
            None,
            &Rect::from_xywh(0.0, 0.0, px as f32, px as f32),
            Some(&paint),
        );
    }
    surface.make_image_snapshot()
}

/// SVG via `skia_rs_safe::svg::{parse_svg, render_svg_in_container}` (which
/// honours `preserveAspectRatio`, i.e. GTK's icon-scaling contract); PNG via
/// `skia_rs_safe::codec::decode_image` + `Canvas::draw_image_rect`.
/// A decode failure resolves to `image-missing` and is **logged once per
/// path**.
///
/// `IconFormat::Xpm` is always `None`: the spec's search finds `.xpm` files
/// and `skia-rs-codec` has no XPM decoder, so the caller falls through to
/// `image-missing` exactly as it does for a corrupt PNG.
#[must_use]
pub fn render(file: &IconFile, size: u32, scale: u32, palette: &Palette) -> Option<Image> {
    let px = pixel_size(size, scale);
    match file.format {
        IconFormat::Svg => render_svg_file(&file.path, px, symbolic::is_symbolic(file), palette),
        IconFormat::Png => render_raster_file(&file.path, px),
        IconFormat::Xpm => {
            log_once(&file.path, "XPM icons are located but not decoded");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{pixel_size, render};
    use crate::css::value::Rgba;
    use crate::icons::test_support::roots;
    use crate::icons::{DirKind, IconFile, IconFormat, MAX_ICON_PX, Palette};
    use skia_rs_safe::canvas::Surface;
    use skia_rs_safe::codec::Image;
    use skia_rs_safe::core::Color;
    use std::path::PathBuf;

    fn rgba(r: u8, g: u8, b: u8) -> Rgba {
        Rgba {
            r: f32::from(r) / 255.0,
            g: f32::from(g) / 255.0,
            b: f32::from(b) / 255.0,
            a: 1.0,
        }
    }

    /// A palette whose four slots are all *different from the defaults*.
    ///
    /// Deliberately not `Palette::for_color`: these are the GNOME 42
    /// success/warning/error hexes, and the vendored Adwaita sheet this crate
    /// ships uses different ones (`theme::DEFAULT_SUCCESS` and friends; contract §10
    /// P7-D49). A test that reads a slot back therefore proves the slot was
    /// actually used, rather than that a default happened to match.
    fn test_palette() -> Palette {
        Palette {
            foreground: rgba(0x35, 0x84, 0xe4),
            success: rgba(0x26, 0xa2, 0x69),
            warning: rgba(0xcd, 0x93, 0x09),
            error: rgba(0xe0, 0x1b, 0x24),
        }
    }

    /// A decoded image's pixel, read the way every other paint test in this
    /// crate reads one: by drawing it onto a raster surface.
    fn pixel(image: &Image, x: i32, y: i32) -> Color {
        let mut surface =
            Surface::new_raster_n32_premul(image.width(), image.height()).expect("surface");
        {
            let mut canvas = surface.canvas();
            canvas.clear(Color::TRANSPARENT);
            canvas.draw_image(image, 0.0, 0.0, None);
        }
        surface
            .pixel_buffer()
            .get_pixel(x, y)
            .expect("pixel is inside the image")
    }

    fn file(relative: &str, format: IconFormat, symbolic: bool, kind: DirKind) -> IconFile {
        IconFile {
            path: roots()[0].join(relative),
            format,
            nominal_size: 16,
            scale: 1,
            symbolic,
            kind,
        }
    }

    // A PNG at its own size comes back untouched -- no resample, no colour
    // shift.
    // Mutation check: always go through the resampling surface and the
    // colour survives but `document-open.png`'s premultiplied bytes take a
    // needless round trip; assert the exact colour so a resample that
    // changed it would be caught.
    #[test]
    fn a_png_renders_at_its_native_size() {
        let image = render(
            &file(
                "MiniTheme/16x16/actions/document-open.png",
                IconFormat::Png,
                false,
                DirKind::Fixed { size: 16 },
            ),
            16,
            1,
            &test_palette(),
        )
        .expect("rendered");
        assert_eq!(image.width(), 16);
        assert_eq!(image.height(), 16);
        assert_eq!(pixel(&image, 8, 8), Color(0xFFFF_0000));
    }

    // And is scaled when the request is bigger, including for HiDPI: the
    // pixel size is `size * scale`.
    // Mutation check: ignore `scale` and the dimensions come back 16x16.
    #[test]
    fn a_png_is_resampled_to_the_requested_device_size() {
        let icon = file(
            "MiniTheme/16x16/actions/document-open.png",
            IconFormat::Png,
            false,
            DirKind::Fixed { size: 16 },
        );
        let image = render(&icon, 16, 2, &test_palette()).expect("rendered");
        assert_eq!(image.width(), 32);
        assert_eq!(image.height(), 32);
        assert_eq!(pixel(&image, 16, 16), Color(0xFFFF_0000));

        let image = render(&icon, 48, 1, &test_palette()).expect("rendered");
        assert_eq!(image.width(), 48);
        assert_eq!(pixel(&image, 24, 24), Color(0xFFFF_0000));
    }

    // An SVG is rendered through its viewBox into the requested container,
    // so a 16-unit document fills a 24px box exactly.
    // Mutation check: pass the document's own width/height as the container
    // and the image comes back 128px wide.
    #[test]
    fn an_svg_renders_into_the_requested_container() {
        let image = render(
            &file(
                "MiniTheme/scalable/actions/document-open.svg",
                IconFormat::Svg,
                false,
                DirKind::Scalable { min: 8, max: 512 },
            ),
            24,
            1,
            &test_palette(),
        )
        .expect("rendered");
        assert_eq!(image.width(), 24);
        assert_eq!(image.height(), 24);
        assert_eq!(pixel(&image, 12, 12), Color(0xFF1C_71D8));
    }

    // THE recolour pixel test the spec and the contract both ask for: the
    // symbolic icon under `color: #3584e4`, with its unclassed half taking
    // the foreground and its `.success` half taking the success slot --
    // through the whole parse/recolour/render path, not just the DOM.
    // Mutation check: skip the `recolour` call and both halves come back
    // #bebebe, the placeholder grey the fixture is authored in.
    #[test]
    fn a_symbolic_icon_is_recoloured_to_the_palette() {
        let image = render(
            &file(
                "MiniTheme/symbolic/actions/document-open-symbolic.svg",
                IconFormat::Svg,
                true,
                DirKind::Scalable { min: 8, max: 512 },
            ),
            16,
            1,
            &test_palette(),
        )
        .expect("rendered");
        assert_eq!(image.width(), 16);
        assert_eq!(pixel(&image, 4, 8), Color(0xFF35_84E4));
        assert_eq!(pixel(&image, 12, 8), Color(0xFF26_A269));
    }

    // A full-colour icon is drawn as authored: no recolouring at all.
    // Mutation check: recolour unconditionally and the SVG's #1c71d8 becomes
    // the palette foreground.
    #[test]
    fn a_non_symbolic_icon_is_never_recoloured() {
        let image = render(
            &file(
                "MiniTheme/scalable/actions/document-open.svg",
                IconFormat::Svg,
                false,
                DirKind::Scalable { min: 8, max: 512 },
            ),
            16,
            1,
            &test_palette(),
        )
        .expect("rendered");
        assert_eq!(pixel(&image, 8, 8), Color(0xFF1C_71D8));
    }

    // Every failure mode is `None`, never a panic: the caller falls through
    // to image-missing.
    // Mutation check: `unwrap()` any of the decodes and the first case
    // panics.
    #[test]
    fn every_undecodable_input_is_none_and_never_a_panic() {
        let palette = test_palette();
        let cases = [
            file(
                "MiniTheme/16x16/actions/broken.png",
                IconFormat::Png,
                false,
                DirKind::Fixed { size: 16 },
            ),
            file(
                "MiniTheme/16x16/actions/does-not-exist.png",
                IconFormat::Png,
                false,
                DirKind::Fixed { size: 16 },
            ),
            file(
                "MiniTheme/16x16/actions",
                IconFormat::Png,
                false,
                DirKind::Fixed { size: 16 },
            ),
            file(
                "MiniTheme/16x16/actions/broken.png",
                IconFormat::Svg,
                false,
                DirKind::Fixed { size: 16 },
            ),
            file(
                "MiniTheme/16x16/actions/document-open.png",
                IconFormat::Xpm,
                false,
                DirKind::Fixed { size: 16 },
            ),
        ];
        for icon in &cases {
            assert!(
                render(icon, 16, 1, &palette).is_none(),
                "{} rendered",
                icon.path.display()
            );
        }

        // Hostile geometry on a good file.
        let good = file(
            "MiniTheme/16x16/actions/document-open.png",
            IconFormat::Png,
            false,
            DirKind::Fixed { size: 16 },
        );
        for (size, scale) in [(0, 0), (0, 1), (1, 0), (u32::MAX, u32::MAX), (u32::MAX, 4)] {
            if let Some(image) = render(&good, size, scale, &palette) {
                assert!(image.width() >= 1 && image.width() <= MAX_ICON_PX as i32);
            }
        }

        // Truncated and garbage SVG text on disk.
        let dir = tempfile::tempdir().expect("tempdir");
        for (name, bytes) in [
            ("empty.svg", b"".as_slice()),
            ("truncated.svg", b"<svg><rect".as_slice()),
            ("nonsense.svg", b"\xff\xfe\x00\x01not xml".as_slice()),
            ("nosvg.svg", b"<html><body>hi</body></html>".as_slice()),
        ] {
            let path = dir.path().join(name);
            std::fs::write(&path, bytes).expect("write");
            let icon = IconFile {
                path: path.clone(),
                format: IconFormat::Svg,
                nominal_size: 16,
                scale: 1,
                symbolic: false,
                kind: DirKind::Scalable { min: 8, max: 512 },
            };
            let _ = render(&icon, 16, 1, &palette);
        }
    }

    // The pixel size is `size * scale`, clamped into `1..=MAX_ICON_PX`.
    // Mutation check: drop the clamp and `pixel_size(u32::MAX, 4)`
    // overflows an i32.
    #[test]
    fn the_pixel_size_is_size_times_scale_clamped() {
        assert_eq!(pixel_size(16, 1), 16);
        assert_eq!(pixel_size(16, 2), 32);
        assert_eq!(pixel_size(0, 0), 1);
        assert_eq!(pixel_size(u32::MAX, u32::MAX), MAX_ICON_PX as i32);
        let _: PathBuf = roots()[0].clone();
    }
}
