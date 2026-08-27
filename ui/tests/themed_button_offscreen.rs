//! The M1 load-bearing gate: CSS -> cascade -> computed values -> Skia
//! pixels, with no compositor involved.
//!
//! Every expected value below is read out of the vendored Adwaita sheet,
//! so this test fails if any seam in the pipeline is wrong -- a mis-parsed
//! declaration, a selector that stops matching, a cascade that picks the
//! wrong winner, a gradient sampled from the wrong edge, or a paint that
//! puts the wrong bytes down.

use icedtea_ui::BUNDLED_ADWAITA_LIGHT;
use icedtea_ui::css::cascade::CompiledSheet;
use icedtea_ui::css::computed::{Background, BackgroundClip, ComputedStyle, GradientStop};
use icedtea_ui::css::node::{Node, PseudoStates};
use icedtea_ui::text::FontStack;
use icedtea_ui::widget::button::Button;
use skia_rs_safe::canvas::Surface;
use skia_rs_safe::core::Color;

const SURFACE_W: i32 = 240;
const SURFACE_H: i32 = 80;

fn stop(color: u32, position_px: Option<f32>) -> GradientStop {
    GradientStop {
        color: Color(color),
        position_px,
    }
}

/// A window > button node tree, a compiled Adwaita sheet and a system font.
fn labelled_fixture(label: &str, classes: &[&str]) -> (CompiledSheet, FontStack, Button) {
    let sheet = CompiledSheet::compile(BUNDLED_ADWAITA_LIGHT);
    let fonts =
        FontStack::system().expect("no system font found; install dejavu/liberation/noto sans");
    let window = Node::with_classes("window", &["background"]);
    let mut button = Button::new(label, classes, window);
    button.restyle(&sheet, &fonts);
    (sheet, fonts, button)
}

/// The same, labelled "Click me" -- what every pixel assertion here pins.
fn fixture(classes: &[&str]) -> (CompiledSheet, FontStack, Button) {
    labelled_fixture("Click me", classes)
}

/// Render `button` at the surface origin and return the surface.
fn render(button: &Button) -> Surface {
    let mut surface = Surface::new_raster_n32_premul(SURFACE_W, SURFACE_H).expect("raster surface");
    surface.canvas().clear(Color::TRANSPARENT);
    button.render(&mut surface, (0.0, 0.0));
    surface
}

/// Read a pixel back as premultiplied ARGB.
fn pixel(surface: &Surface, x: i32, y: i32) -> Color {
    surface
        .pixel_buffer()
        .get_pixel(x, y)
        .unwrap_or_else(|| panic!("pixel ({x}, {y}) is outside the surface"))
}

#[test]
fn adwaita_button_computed_style_and_pixels_match_the_theme() {
    let (sheet, fonts, mut button) = fixture(&[]);

    // --- 1. Computed values equal Adwaita's resolved values ------------
    // `button` (line 215 of the vendored sheet).
    let normal = button.style().clone();
    assert_eq!(
        normal.background,
        Background::LinearGradientToTop {
            from: stop(0xFFF6_F5F4, Some(2.0)),
            to: stop(0xFFFB_FAFA, None),
        },
        "Adwaita's base button background did not resolve"
    );
    assert_eq!(normal.color, Color(0xFF2E_3436));
    assert_eq!(normal.border_color, Color(0xFFCD_C7C2));
    assert_eq!(normal.border_width, 1.0);
    assert_eq!(normal.border_radius, 5.0);
    assert_eq!(normal.padding, [4.0, 9.0, 4.0, 9.0]);
    assert_eq!(normal.min_width, 16.0);
    assert_eq!(normal.min_height, 24.0);
    assert_eq!(normal.font_size, ComputedStyle::DEFAULT_FONT_SIZE);
    // Adwaita sets no `background-clip` on `button`, so the background fills
    // the border box -- CSS's and GTK's default. The opaque #cdc7c2 border
    // is painted over it, which is why the border-pixel assertion below
    // still reads the border colour and the corner outside the radius is
    // still transparent.
    assert_eq!(normal.background_clip, BackgroundClip::BorderBox);

    let allocation = button.allocation();
    assert!(
        allocation.width > 40.0 && allocation.width < SURFACE_W as f32,
        "implausible allocation width {}",
        allocation.width
    );
    // A2: GTK's `min-height` floors the *content* box, so the border box is
    // `max(line-height, min-height 24) + padding 4 + 4 + border 1 + 1`. At
    // 14px no UI face has a 24px line height, so 34 is exact and font
    // independent -- and it is what GTK 4.22 allocates for this button.
    assert_eq!(
        allocation.height, 34.0,
        "min-height was applied to the border box, not the content box"
    );

    let cx = (allocation.width / 2.0) as i32;
    let cy = (allocation.height / 2.0) as i32;
    // The button's horizontal centre lands *on the label*: "Click me" is
    // centred, so (cx, cy) is a glyph pixel, not background. Background
    // colors are therefore sampled on the same row but in the padding
    // gutter -- inside the 1px border, left of the label's content box, and
    // far enough down the side that the 5px corner arcs do not reach it.
    let bx = 4;
    assert!(
        allocation.label_x > bx as f32,
        "sampling column {bx} is not clear of the label, which starts at {}",
        allocation.label_x
    );

    // --- 2. Pixels: the normal state -----------------------------------
    let surface = render(&button);

    // `linear-gradient(to top, #f6f5f4 2px, #fbfafa)`: below the 2px first
    // stop the fill is flat, so the row just inside the bottom border is
    // exactly the first stop's color.
    let inner_bottom = (allocation.height - 2.0) as i32;
    assert_eq!(
        pixel(&surface, cx, inner_bottom),
        Color(0xFFF6_F5F4),
        "the flat pre-first-stop band of Adwaita's button gradient is wrong"
    );
    let normal_center = pixel(&surface, bx, cy);
    assert!(
        normal_center != Color(0xFFF6_F5F4) && normal_center != Color(0xFFFB_FAFA),
        "the gradient's midpoint {normal_center:?} equals a stop: it is not interpolating"
    );
    assert_eq!(normal_center.alpha(), 255);
    assert!((0xF6..=0xFB).contains(&normal_center.red()));
    assert!((0xF5..=0xFA).contains(&normal_center.green()));
    assert!((0xF4..=0xFA).contains(&normal_center.blue()));

    // A pixel just outside the 5px corner radius is transparent: (0, 0) is
    // ~7.07px from the corner arc's centre, comfortably past r=5 even with
    // anti-aliasing.
    assert_eq!(
        pixel(&surface, 0, 0).alpha(),
        0,
        "the corner outside border-radius: 5px was painted"
    );
    // The border itself is on screen, on the straight run of the top edge.
    assert_eq!(
        pixel(&surface, cx, 0),
        Color(0xFFCD_C7C2),
        "Adwaita's 1px #cdc7c2 button border is missing"
    );

    // --- 3. Toggling :hover changes computed style AND pixels ----------
    button.set_states(PseudoStates::HOVER, &sheet, &fonts);
    let hovered = button.style().clone();
    assert_eq!(
        hovered.background,
        Background::LinearGradientToTop {
            from: stop(0xFFD6_D1CD, None),
            to: stop(0xFFE8_E6E3, Some(1.0)),
        },
        "`button:hover` did not win the cascade"
    );
    assert_ne!(hovered.background, normal.background);

    let hovered_surface = render(&button);
    // The second stop sits 1px above the bottom edge, so everything above
    // it -- the whole centre -- is flat #e8e6e3.
    assert_eq!(
        pixel(&hovered_surface, bx, cy),
        Color(0xFFE8_E6E3),
        "the hover gradient's post-last-stop band is wrong"
    );
    assert_ne!(
        pixel(&hovered_surface, bx, cy),
        normal_center,
        "toggling :hover did not change the painted pixels"
    );

    // --- 4. Toggling :active: a flat `image(<color>)` background --------
    button.set_states(PseudoStates::ACTIVE, &sheet, &fonts);
    assert_eq!(
        button.style().background,
        Background::Solid(Color(0xFFDA_D6D2)),
        "`button:active`'s image(#dad6d2) did not resolve to a flat fill"
    );
    let active_surface = render(&button);
    assert_eq!(
        pixel(&active_surface, bx, cy),
        Color(0xFFDA_D6D2),
        "the :active flat background is not the color the theme declares"
    );
    assert_eq!(pixel(&active_surface, 0, 0).alpha(), 0);
}

#[test]
fn suggested_action_button_is_adwaitas_accent_blue() {
    let (_sheet, _fonts, button) = fixture(&["suggested-action"]);
    let style = button.style().clone();
    assert_eq!(
        style.background,
        Background::LinearGradientToTop {
            from: stop(0xFF2C_7FE3, Some(2.0)),
            to: stop(0xFF35_84E4, None),
        }
    );
    assert_eq!(
        style.color,
        Color(0xFFFF_FFFF),
        "suggested-action text is white"
    );
    assert_eq!(style.border_color, Color(0xFF15_539E));

    let allocation = button.allocation();
    // Same padding-gutter sampling as the gate test: the button's centre
    // column is covered by the (white) label.
    assert!(
        allocation.label_x > 4.0,
        "sampling column 4 is not clear of the label, which starts at {}",
        allocation.label_x
    );
    let surface = render(&button);
    let center = pixel(&surface, 4, (allocation.height / 2.0) as i32);
    // Both stops are within 9/5/1 per channel of `@accent_color` #3584E4,
    // so any point on the gradient is close to it.
    let close = |a: u8, b: u8| i32::from(a).abs_diff(i32::from(b)) <= 10;
    assert!(
        close(center.red(), 0x35) && close(center.green(), 0x84) && close(center.blue(), 0xE4),
        "suggested-action centre {center:?} is not Adwaita's accent blue"
    );
}

#[test]
fn an_empty_adwaita_button_is_the_minimum_content_box_plus_its_frame() {
    // The other half of A2, with no intrinsic size in play at all:
    // min-width 16 + padding 9 + 9 + border 1 + 1 = 36 wide, and
    // min-height 24 + padding 4 + 4 + border 1 + 1 = 34 high. Before the
    // fix this button was 20x27 -- GTK 4.22 makes it 36x34.
    let (_sheet, _fonts, button) = labelled_fixture("", &[]);
    let allocation = button.allocation();
    assert_eq!(allocation.width, 36.0);
    assert_eq!(allocation.height, 34.0);
}

#[test]
fn the_label_is_actually_drawn() {
    let (_sheet, _fonts, button) = fixture(&[]);
    let allocation = button.allocation();
    let surface = render(&button);

    // The label is #2e3436 on a near-white background; count pixels
    // markedly darker than the lightest gradient stop inside the content
    // box. Zero would mean the glyph run never reached the canvas.
    let mut dark = 0u32;
    for y in 6..(allocation.height as i32 - 6) {
        for x in 10..(allocation.width as i32 - 10) {
            if pixel(&surface, x, y).red() < 0xC0 {
                dark += 1;
            }
        }
    }
    assert!(dark > 20, "only {dark} label pixels were painted");
}
