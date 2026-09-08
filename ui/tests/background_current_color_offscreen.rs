//! `currentColor` inside a `background-image` layer must resolve to the
//! node's `color`, not to its `background-color` -- CSS Color L4 §4.4
//! defines `currentColor` in terms of the `color` property everywhere it
//! appears, and a background layer is no exception.
//!
//! A `background-image: image(<color>)` or a gradient's `currentColor` stop
//! is resolved once, at computed-style time, against the node's own `color`
//! (`css/computed.rs`'s `resolve_image`, fed the `color_ctx` built from
//! `own_color` at `computed.rs:~720`) -- so those two forms never carry an
//! unresolved `currentColor` as far as paint. A `-gtk-icontheme()` layer is
//! different: the registry leaves `Image::Icon` untouched at computed-value
//! time (`resolve_image`'s catch-all), so its `currentColor` is only
//! resolved when `paint_layer` builds a `Palette::for_color(current)` --
//! and `current` is exactly the value this bug threaded wrong.

use icedtea_ui::css::cascade::CompiledSheet;
use icedtea_ui::css::computed::{ComputedStyle, ResolveEnv};
use icedtea_ui::css::node::Node;
use icedtea_ui::css::select::MatchCx;
use icedtea_ui::icons::IconTheme;
use icedtea_ui::layout::{Allocation, Rect};
use icedtea_ui::paint::{ImageCache, PaintCx, paint_node};
use icedtea_ui::text::FontDatabase;
use skia_rs_safe::canvas::Surface;
use skia_rs_safe::core::Color;
use std::path::PathBuf;

fn fixture_roots() -> Vec<PathBuf> {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mini-icon-theme");
    vec![base.join("root-a"), base.join("root-b")]
}

/// Paint a flush `side` x `side` node (no border, no padding) from `css`
/// through the crate's public full-pipeline entry ([`paint_node`]) and return
/// the resulting surface.
///
/// The CSS these tests use sets only `background-*`, so `paint_node`'s later
/// stages (borders, shadows, outline, icon-source, text) contribute nothing
/// and the asserted pixels are exactly the background layers' output -- but it
/// exercises the same currentColor threading end-to-end, without reaching into
/// the crate-private `paint_backgrounds`.
fn painted(css: &str, side: i32) -> Surface {
    let sheet = CompiledSheet::compile(css);
    let window = Node::new("window");
    let button = Node::new("image");
    window.append_child(&button);
    let env = ResolveEnv::default();
    let mut cx = MatchCx::new();
    let style = ComputedStyle::resolve_chain(&sheet, &button, &env, &mut cx);

    let border_box = Rect::new(0.0, 0.0, side as f32, side as f32);
    let alloc = Allocation {
        border_box,
        content_box: border_box,
        border: [0.0; 4],
        padding: [0.0; 4],
    };

    let mut fonts = FontDatabase::probe_only();
    let mut images = ImageCache::new();
    let mut icons = IconTheme::with_name_and_roots("MiniTheme", fixture_roots());
    let mut paint_cx = PaintCx {
        env: &env,
        colors: &sheet.colors,
        fonts: &mut fonts,
        images: &mut images,
        icons: &mut icons,
        text: None,
    };

    let mut surface = Surface::new_raster_n32_premul(side, side).expect("raster surface");
    surface.canvas().clear(Color::TRANSPARENT);
    {
        let mut canvas = surface.canvas();
        paint_node(&mut canvas, &button, &style, &alloc, None, &mut paint_cx);
    }
    surface
}

fn pixel(surface: &Surface, x: i32, y: i32) -> Color {
    surface
        .pixel_buffer()
        .get_pixel(x, y)
        .unwrap_or_else(|| panic!("pixel ({x}, {y}) is outside the surface"))
}

#[test]
fn a_solid_image_layers_currentcolor_is_unaffected_by_the_background_color_currentcolor_bug() {
    // NOT a regression pin for the paint-time currentColor fix: it passes on
    // both the buggy and the fixed code. `Image::Solid`'s `currentColor` is
    // resolved at *computed-style* time against the node's own `color`, so it
    // never carries an unresolved `currentColor` into `paint_layer` and the
    // `background-color`-vs-`color` mix-up this fix corrects could not reach
    // it. It stays only to document that the `Image::Solid` path is correct
    // and untouched; the load-bearing pin is the `-gtk-icontheme()` test
    // below (and its sibling in `icon_theme.rs`), whose `Image::Icon` layer
    // *is* recoloured at paint time. Here: `color: red`, `background-color:
    // green`, and an `image(currentColor)` layer must paint red.
    let surface = painted(
        "image { color: #ff0000; background-color: #00ff00; \
         background-image: image(currentColor); \
         border-radius: 0; border: 0 solid transparent; padding: 0 }",
        16,
    );
    assert_eq!(
        pixel(&surface, 8, 8),
        Color(0xFFFF_0000),
        "background-image: image(currentColor) must resolve to the node's color, \
         not its background-color"
    );
}

#[test]
fn a_symbolic_background_icon_layers_currentcolor_is_the_nodes_color_not_its_background_color() {
    // The bug's actual reproduction: `-gtk-icontheme()` is left unresolved
    // by `resolve_image` (its catch-all arm), so a symbolic icon's palette
    // foreground is only ever fixed up at *paint* time, from whatever
    // `current: Rgba` `paint_layer` was handed. Before the fix that was
    // `paint_backgrounds`'s `color` argument -- `background-color` -- so
    // this recoloured to green (#00ff00), the fixture's placeholder colour
    // for anything not #ff0000. After the fix it is `style.color()`, so the
    // icon recolours to red.
    let surface = painted(
        "image { color: #ff0000; background-color: #00ff00; \
         background-image: -gtk-icontheme(document-open-symbolic); \
         border-radius: 0; border: 0 solid transparent; padding: 0 }",
        16,
    );
    assert_eq!(
        pixel(&surface, 4, 8),
        Color(0xFFFF_0000),
        "-gtk-icontheme()'s currentColor must resolve to the node's color, \
         not its background-color"
    );
}
