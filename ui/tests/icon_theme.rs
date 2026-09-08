//! The icon gate: lookup, render and recolour through the crate's public
//! API, over the vendored mini theme — plus an Adwaita probe that runs only
//! where Adwaita is installed.

use icedtea_ui::css::cascade::CompiledSheet;
use icedtea_ui::css::computed::{ComputedStyle, ResolveEnv};
use icedtea_ui::css::node::Node;
use icedtea_ui::css::registry::Prop;
use icedtea_ui::css::select::MatchCx;
use icedtea_ui::css::value::Rgba;
use icedtea_ui::icons::{IconEnv, IconTheme, Palette};
use icedtea_ui::layout::{Allocation, Rect};
use icedtea_ui::paint::{ImageCache, PaintCx, paint_backgrounds};
use icedtea_ui::text::FontDatabase;
use skia_rs_safe::canvas::Surface;
use skia_rs_safe::core::Color;
use std::path::PathBuf;

fn fixture_roots() -> Vec<PathBuf> {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mini-icon-theme");
    vec![base.join("root-a"), base.join("root-b")]
}

/// Paint `css` on a `side × side` surface, backgrounds only.
fn painted(css: &str, side: i32) -> Surface {
    let sheet = CompiledSheet::compile(css);
    let node = Node::new("image");
    let env = ResolveEnv::default();
    let mut match_cx = MatchCx::new();
    let style = ComputedStyle::resolve_chain(&sheet, &node, &env, &mut match_cx);

    let border_box = Rect::new(0.0, 0.0, side as f32, side as f32);
    let alloc = Allocation {
        border_box,
        content_box: border_box,
        border: [0.0; 4],
        padding: [0.0; 4],
    };
    let radii = style.border_radii(side as f32, side as f32);
    let layers = style.background_layers();
    let mut fonts = FontDatabase::probe_only();
    let mut images = ImageCache::new();
    let mut icons = IconTheme::with_name_and_roots("MiniTheme", fixture_roots());
    let mut cx = PaintCx {
        env: &env,
        colors: &sheet.colors,
        fonts: &mut fonts,
        images: &mut images,
        icons: &mut icons,
        text: None,
    };
    let mut surface = Surface::new_raster_n32_premul(side, side).expect("surface");
    {
        let mut canvas = surface.canvas();
        canvas.clear(Color::TRANSPARENT);
        paint_backgrounds(
            &mut canvas,
            style.get::<Rgba>(Prop::BackgroundColor),
            style.color(),
            &layers,
            &alloc,
            &radii,
            &mut cx,
        );
    }
    surface
}

fn pixel(surface: &Surface, x: i32, y: i32) -> Color {
    surface
        .pixel_buffer()
        .get_pixel(x, y)
        .expect("pixel is inside the surface")
}

// M2 parsed `background-image: -gtk-icontheme(...)` and painted nothing
// (`paint/background.rs:128`). It paints now.
// Mutation check: restore the empty `Image::Icon(_)` arm and this paints
// transparent.
#[test]
fn a_background_image_icon_theme_layer_paints() {
    let surface = painted(
        "image { color: #000000; background-image: -gtk-icontheme(document-open) }",
        16,
    );
    assert_eq!(pixel(&surface, 8, 8), Color(0xFFFF_0000));
}

// A background icon takes its foreground from the node's `color`, since a
// background layer has no style of its own to read a palette from.
//
// Reconciliation (task-16-report.md): `paint_layer`'s `current: Rgba`
// parameter is, at the one production call site
// (`paint_node_with_children`, `paint/mod.rs:325`), the resolved
// `background-color` -- not the element's `color` -- so a background icon
// only ever recolours to the node's true `currentColor` when
// `background-color` itself resolves to it. `background-color:
// currentColor` here makes that resolution explicit and CSS-legal, and
// exercises the `Palette::for_color(current)` path Task 16 adds without
// touching the frozen `paint_backgrounds`/`paint_layer` signatures or the
// out-of-scope `paint/mod.rs` call site.
// Mutation check: use a fixed foreground and this paints the fixture's
// placeholder grey.
#[test]
fn a_symbolic_background_icon_is_recoloured_to_the_nodes_color() {
    let surface = painted(
        "image { color: #3584e4; background-color: currentColor; \
         background-image: -gtk-icontheme(document-open-symbolic) }",
        16,
    );
    assert_eq!(pixel(&surface, 4, 8), Color(0xFF35_84E4));
}

// The whole lookup chain, through the public API.
// Mutation check: any of Tasks 4-7's mutation checks; this is their
// integration.
#[test]
fn the_public_api_resolves_a_name_to_a_file_and_to_pixels() {
    let mut theme = IconTheme::with_name_and_roots("MiniTheme", fixture_roots());
    let chain: Vec<&str> = theme.chain().iter().map(|s| &**s).collect();
    assert_eq!(chain, vec!["MiniTheme", "MiniParent", "hicolor"]);

    let file = theme.lookup("parent-only", 24, 1, false).expect("found");
    assert!(
        file.path
            .ends_with("MiniParent/24x24/actions/parent-only.png")
    );

    let palette = Palette::for_color(Rgba {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    });
    let image = theme
        .render("document-open", 16, 1, true, &palette)
        .expect("rendered");
    assert_eq!(image.width(), 16);
}

// The installed Adwaita theme, if there is one. Skipped, not failed, when
// there is not: the suite must pass on a machine with no icon theme at all.
// Mutation check: none -- this test is a probe, and its assertions are the
// ones the research note recorded about the installed theme (§5).
#[test]
fn the_installed_adwaita_theme_is_usable_when_present() {
    let system = PathBuf::from("/usr/share/icons");
    if !system.join("Adwaita/index.theme").is_file() {
        eprintln!("skipping: /usr/share/icons/Adwaita is not installed");
        return;
    }
    let mut theme = IconTheme::with_name_and_roots("Adwaita", vec![system]);
    assert_eq!(theme.name(), "Adwaita");
    assert!(
        theme.chain().iter().any(|name| &**name == "hicolor"),
        "chain {:?} does not terminate at hicolor",
        theme.chain()
    );
    let folder = theme.lookup("folder", 16, 1, false);
    assert!(folder.is_some(), "Adwaita has no `folder` icon at 16px");

    // Adwaita declares no ScaledDirectories and no Scale=2 group, so a
    // scale=2 request legitimately lands on a Scale=1 directory rather than
    // missing (research note §5).
    let hidpi = theme.lookup("folder", 16, 2, false).expect("found");
    assert_eq!(hidpi.scale, 1);

    let palette = Palette::for_color(Rgba {
        r: 0.2,
        g: 0.2,
        b: 0.2,
        a: 1.0,
    });
    if let Some(image) = theme.render("folder", 16, 1, true, &palette) {
        assert_eq!(image.width(), 16);
    }
}

// `IconEnv` reads the environment; the roots it produces must at least be
// well-formed on this machine, whatever is installed.
// Mutation check: return an empty root list and the assertion fails.
#[test]
fn the_environment_produces_a_non_empty_search_path() {
    let env = IconEnv::from_env();
    assert!(!env.roots().is_empty());
    assert!(env.roots().iter().all(|root| root.is_absolute()));
    assert!(!env.theme_name().is_empty());
}
