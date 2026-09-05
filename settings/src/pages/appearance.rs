//! The Appearance page: bar position/height, corner radius, snap gap,
//! palette colors, and wallpaper.

use icedtea_ui::css::value::Rgba;
use icedtea_ui::layout::Align;
use icedtea_ui::view::View;
use icedtea_ui::view::builders::{
    self as w, DrawingAreaExt, DropDownExt, EntryExt, GridExt, SpinButtonExt,
};
use icedtea_ui::widgets::color_dialog::ColorDialogC;
use icedtea_ui::widgets::types::Orientation;

use crate::app::{ColorSlot, Msg, SettingsModel};
use crate::model::{BAR_POSITIONS, valid_hex};

/// Parse a `#RRGGBB` string into an opaque toolkit colour; falls back to
/// opaque black for anything `valid_hex` rejects (should not happen for
/// values this page itself wrote, but keeps `view` panic-free against a
/// hand-edited db).
#[must_use]
pub fn hex_to_rgba(s: &str) -> Rgba {
    if !valid_hex(s) {
        return Rgba {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        };
    }
    let digits = &s[1..];
    let byte = |i: usize| u8::from_str_radix(&digits[i..i + 2], 16).unwrap_or(0);
    Rgba {
        r: f32::from(byte(0)) / 255.0,
        g: f32::from(byte(2)) / 255.0,
        b: f32::from(byte(4)) / 255.0,
        a: 1.0,
    }
}

/// Format a colour's channels (alpha ignored — the palette has no
/// transparency concept) as `#RRGGBB`.
#[must_use]
pub fn rgba_to_hex(c: Rgba) -> String {
    let chan = |v: f32| {
        if v.is_nan() {
            0
        } else {
            (v.clamp(0.0, 1.0) * 255.0).round() as u8
        }
    };
    format!("#{:02x}{:02x}{:02x}", chan(c.r), chan(c.g), chan(c.b))
}

/// The value a `color_dialog_button` carries for `s` (M3 P5-D23: the colour
/// rides `PropName::Value` as a packed `f64`).
#[must_use]
pub fn hex_to_packed(s: &str) -> f64 {
    ColorDialogC::pack(hex_to_rgba(s))
}

/// The hex a `color_dialog_button`'s `on_value_changed` payload means.
/// `ColorDialogC::unpack` already yields opaque black for a non-finite or
/// out-of-range value, so this never panics on a hostile model.
#[must_use]
pub fn packed_to_hex(packed: f64) -> String {
    rgba_to_hex(ColorDialogC::unpack(packed))
}

/// Image extensions the wallpaper field accepts, lower-case.
///
/// The compositor's wallpaper worker decodes with the same set; anything
/// outside it would be accepted here and silently ignored there, which is
/// what the inline error the `Entry` shows exists to prevent.
pub const WALLPAPER_EXTENSIONS: [&str; 5] = ["png", "jpg", "jpeg", "webp", "bmp"];

/// Validate a typed or portal-supplied wallpaper path.
///
/// `Ok(path)` is what goes into `working.appearance.wallpaper`; `Err(message)`
/// is what the page shows beside the field and is never written to the model.
/// The caller has already decided that an empty string means "no wallpaper",
/// so this function's caller never passes one.
///
/// Untrusted input: the string comes from a text field or a portal reply, so
/// every failure is a message and never a panic.
pub fn validate_wallpaper(text: &str) -> Result<std::path::PathBuf, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err("Enter a path to an image".to_string());
    }
    let path = std::path::PathBuf::from(trimmed);
    let meta = std::fs::metadata(&path).map_err(|_| format!("No such file: {trimmed}"))?;
    if !meta.is_file() {
        return Err(format!("Not a file: {trimmed}"));
    }
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if !WALLPAPER_EXTENSIONS.contains(&ext.as_str()) {
        return Err(format!(
            "Unsupported image type: {}",
            if ext.is_empty() {
                "no extension".to_string()
            } else {
                format!(".{ext}")
            }
        ));
    }
    Ok(path)
}

/// The pixel value a `SpinButton` on this page reports, as the config stores
/// it.
///
/// The GTK page took `sb.value() as i32` over a `0.0..=256.0` adjustment; the
/// toolkit's `SpinButtonC` clamps to its own bounds too, but a hand-edited db
/// or a future range change must not be able to write a negative bar height,
/// and `as i32` on a NaN is 0 by saturation rather than by intent. Rounding
/// (not truncating) is what a 27.999 step lands on.
#[must_use]
pub fn spin_px(v: f64) -> i32 {
    if !v.is_finite() {
        return if v.is_sign_positive() && v.is_infinite() {
            SPIN_MAX_PX
        } else {
            0
        };
    }
    v.round().clamp(0.0, f64::from(SPIN_MAX_PX)) as i32
}

/// The upper bound every pixel spin button on this page shares, matching the
/// GTK `SpinButton::with_range(0.0, 256.0, 1.0)` adjustments.
pub const SPIN_MAX_PX: i32 = 256;

/// How wide and tall a colour swatch draws.
const SWATCH_W: i32 = 40;
const SWATCH_H: i32 = 22;

/// A flat rectangle of `rgba`.
///
/// Deviation P2-D11: `color_dialog_button`'s own chrome lives on a subnode
/// that never gets a taffy allocation, so `ColorDialogButtonC::on_event`'s
/// `local_rect` is always `None` and the widget cannot be clicked in a live
/// window (`ui/src/widgets/scrollbar.rs:309-317` documents the same shape).
/// A `drawing_area` is an ordinary leaf with a real allocation, a real probe
/// point, and P0's three-argument `Prop::Draw`.
#[must_use]
pub fn swatch(rgba: Rgba) -> View<Msg> {
    w::drawing_area(move |canvas, rect, _cx| {
        canvas.draw_rect(&rect.to_skia(), &icedtea_ui::paint::fill_paint(rgba));
    })
    .content_width(SWATCH_W)
    .content_height(SWATCH_H)
}

/// One labelled grid row: the label in column 0, the control in column 1.
fn row(index: u16, text: &str, control: View<Msg>) -> [View<Msg>; 2] {
    [
        w::label(text).halign(Align::Start).at(0, index),
        control.at(1, index),
    ]
}

/// One row whose control spans both columns (the wallpaper buttons and the
/// palette panel, which have no label of their own).
fn wide(index: u16, control: View<Msg>) -> [View<Msg>; 1] {
    [control.at(0, index).span(2, 1)]
}

/// The palette panel, shown while `m.color_picker` names a slot.
fn picker(slot: ColorSlot) -> View<Msg> {
    let palette = ColorDialogC::default_palette();
    let columns = 5u16;
    let buttons = palette.iter().enumerate().map(|(index, rgba)| {
        let packed = ColorDialogC::pack(*rgba);
        let msg = match slot {
            ColorSlot::Background => Msg::BackgroundPicked(packed),
            ColorSlot::Foreground => Msg::ForegroundPicked(packed),
            ColorSlot::Accent => Msg::AccentPicked(packed),
        };
        w::button_from(swatch(*rgba))
            .id(&format!("appearance_swatch_{index}"))
            .key(index)
            .at(index as u16 % columns, index as u16 / columns)
            .on_click(msg)
    });
    w::box_(
        Orientation::Vertical,
        [
            w::grid(buttons).row_spacing(4).column_spacing(4),
            w::button("Close")
                .id("appearance_picker_close")
                .halign(Align::Start)
                .on_click(Msg::ColorPickerClosed),
        ],
    )
    .id("appearance_picker")
}

/// The Appearance page.
#[must_use]
pub fn view(m: &SettingsModel) -> View<Msg> {
    let a = &m.model.working.appearance;
    let position = BAR_POSITIONS
        .iter()
        .position(|p| *p == a.bar_position)
        .unwrap_or(0);

    let mut children: Vec<View<Msg>> = Vec::new();
    children.extend(row(
        0,
        "Bar position",
        w::drop_down(&BAR_POSITIONS)
            .selected(position)
            .id("appearance_bar_position")
            .on_selected(Msg::BarPositionSelected),
    ));
    children.extend(row(
        1,
        "Bar height",
        w::spin_button(f64::from(a.bar_height), 0.0, f64::from(SPIN_MAX_PX))
            .step(1.0)
            .id("appearance_bar_height")
            .on_value_changed(Msg::BarHeightChanged),
    ));
    children.extend(row(
        2,
        "Corner radius",
        w::spin_button(f64::from(a.corner_radius), 0.0, f64::from(SPIN_MAX_PX))
            .step(1.0)
            .id("appearance_corner_radius")
            .on_value_changed(Msg::CornerRadiusChanged),
    ));
    for (index, (text, hex, slot, id)) in [
        (
            "Background",
            a.palette.background.as_str(),
            ColorSlot::Background,
            "appearance_background",
        ),
        (
            "Foreground",
            a.palette.foreground.as_str(),
            ColorSlot::Foreground,
            "appearance_foreground",
        ),
        (
            "Accent",
            a.palette.accent.as_str(),
            ColorSlot::Accent,
            "appearance_accent",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        children.extend(row(
            3 + index as u16,
            text,
            w::button_from(swatch(hex_to_rgba(hex)))
                .id(id)
                .halign(Align::Start)
                .on_click(Msg::ColorPickerOpened(slot)),
        ));
    }
    children.extend(row(
        6,
        "Wallpaper",
        w::entry(&m.wallpaper_text)
            .placeholder("/path/to/image.png")
            .id("appearance_wallpaper_entry")
            .hexpand(true)
            .on_change(|s| Msg::WallpaperEdited(s.to_string())),
    ));
    children.extend(wide(
        7,
        w::box_(
            Orientation::Horizontal,
            [
                w::button("Choose\u{2026}")
                    .id("appearance_wallpaper_browse")
                    .sensitive(m.portal_available)
                    .on_click(Msg::WallpaperBrowse),
                w::button("Clear")
                    .id("appearance_wallpaper_clear")
                    .on_click(Msg::WallpaperCleared),
            ],
        )
        .halign(Align::Start),
    ));
    children.extend(wide(8, wallpaper_status(m)));
    if let Some(slot) = m.color_picker {
        children.extend(wide(9, picker(slot)));
    }

    w::grid(children)
        .row_spacing(10)
        .column_spacing(16)
        .margin(16, 16, 16, 16)
        .id("appearance")
}

/// The row under the wallpaper buttons: the error, or a preview of the image
/// the model currently holds, or nothing to say.
fn wallpaper_status(m: &SettingsModel) -> View<Msg> {
    if let Some(error) = &m.wallpaper_error {
        return w::label(error)
            .halign(Align::Start)
            .classes(&["error"])
            .id("appearance_wallpaper_status");
    }
    match &m.model.working.appearance.wallpaper {
        Some(path) => w::picture(std::path::Path::new(path))
            .height_request(72)
            .halign(Align::Start)
            .id("appearance_wallpaper_status"),
        None => w::label("No wallpaper")
            .halign(Align::Start)
            .id("appearance_wallpaper_status"),
    }
}

#[cfg(test)]
mod tests {
    use super::{hex_to_packed, hex_to_rgba, packed_to_hex, rgba_to_hex, view};
    use icedtea_ui::anim::{Clock, ManualClock};
    use icedtea_ui::css::cascade::CompiledSheet;
    use icedtea_ui::icons::IconTheme;
    use icedtea_ui::text::FontDatabase;
    use icedtea_ui::view::App;

    /// Lay the page out with no compositor and collect every node id.
    fn ids_of(m: crate::app::SettingsModel) -> Vec<String> {
        let sheet = CompiledSheet::compile(icedtea_ui::BUNDLED_ADWAITA_LIGHT);
        let clock: std::rc::Rc<dyn Clock> = std::rc::Rc::new(ManualClock::new());
        let probe = App::new(m, crate::app::update, view)
            .probe(
                (480, 420),
                sheet,
                FontDatabase::new(),
                IconTheme::from_env(),
                clock,
            )
            .expect("the appearance page lays out");
        probe
            .root()
            .descendants()
            .filter_map(|n| n.id().map(|id| id.as_str().to_string()))
            .collect()
    }

    #[test]
    fn the_page_carries_every_id_its_gates_address() {
        let (m, _inbox) = crate::app::tests::test_model();
        let ids = ids_of(m);
        for id in [
            "appearance_bar_position",
            "appearance_bar_height",
            "appearance_corner_radius",
            "appearance_background",
            "appearance_foreground",
            "appearance_accent",
            "appearance_wallpaper_entry",
            "appearance_wallpaper_browse",
            "appearance_wallpaper_clear",
        ] {
            assert!(
                ids.contains(&id.to_string()),
                "{id} is missing from {ids:?}"
            );
        }
        assert!(
            !ids.iter().any(|id| id.starts_with("appearance_swatch_")),
            "the palette panel is closed until a slot is opened"
        );
    }

    #[test]
    fn opening_a_slot_reveals_the_palette_panel() {
        let (mut m, _inbox) = crate::app::tests::test_model();
        m.color_picker = Some(crate::app::ColorSlot::Foreground);
        let ids = ids_of(m);
        assert!(ids.contains(&"appearance_picker".to_string()));
        assert!(ids.contains(&"appearance_picker_close".to_string()));
        assert!(
            ids.contains(&"appearance_swatch_0".to_string()),
            "one button per palette entry"
        );
    }

    #[test]
    fn the_wallpaper_status_row_shows_the_error_when_there_is_one() {
        let (mut m, _inbox) = crate::app::tests::test_model();
        m.wallpaper_error = Some("No such file: /nope.png".to_string());
        let ids = ids_of(m);
        assert!(ids.contains(&"appearance_wallpaper_status".to_string()));
    }

    /// Mutation check: make `hex_to_rgba` divide by 256.0 instead of 255.0;
    /// this round trip fails on `#ffffff`. Restore.
    #[test]
    fn hex_rgba_round_trips() {
        for hex in ["#1e1e2e", "#cdd6f4", "#89b4fa", "#000000", "#ffffff"] {
            let rgba = hex_to_rgba(hex);
            assert_eq!(rgba_to_hex(rgba), hex);
        }
    }

    #[test]
    fn invalid_hex_falls_back_to_black_without_panicking() {
        let black = hex_to_rgba("not-a-color");
        assert_eq!(rgba_to_hex(black), "#000000");
        assert_eq!(
            black.a, 1.0,
            "the fallback is opaque black, not transparent"
        );
    }

    /// A `ColorDialogButton` carries its colour as a packed f64 (M3 P5-D23),
    /// so the page's hex strings have to survive that packing exactly.
    ///
    /// Mutation check: swap `pack`/`unpack` in `packed_to_hex`; this fails.
    #[test]
    fn packed_round_trips_through_the_color_dialog_packing() {
        for hex in ["#1e1e2e", "#cdd6f4", "#89b4fa", "#000000", "#ffffff"] {
            assert_eq!(packed_to_hex(hex_to_packed(hex)), hex);
        }
    }

    #[test]
    fn a_nonsense_packed_value_is_black_not_a_panic() {
        assert_eq!(packed_to_hex(f64::NAN), "#000000");
        assert_eq!(packed_to_hex(-1.0), "#000000");
    }

    #[test]
    fn wallpaper_validation_rejects_a_missing_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("nope.png");
        let err = super::validate_wallpaper(&missing.display().to_string())
            .expect_err("a path that does not exist must not validate");
        assert!(
            err.contains("No such file"),
            "the message names the failure: {err:?}"
        );
    }

    #[test]
    fn wallpaper_validation_rejects_an_unknown_extension() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("wallpaper.txt");
        std::fs::write(&path, b"not an image").expect("write the file");
        let err = super::validate_wallpaper(&path.display().to_string())
            .expect_err("an existing non-image must not validate");
        assert!(
            err.contains("Unsupported image type"),
            "the message names the failure: {err:?}"
        );
    }

    #[test]
    fn wallpaper_validation_accepts_every_supported_extension() {
        let dir = tempfile::tempdir().expect("tempdir");
        for ext in super::WALLPAPER_EXTENSIONS {
            let path = dir.path().join(format!("wallpaper.{ext}"));
            std::fs::write(&path, b"pixels").expect("write the file");
            assert_eq!(
                super::validate_wallpaper(&path.display().to_string()).expect("validates"),
                path,
                "`.{ext}` is in WALLPAPER_EXTENSIONS"
            );
        }
    }

    #[test]
    fn wallpaper_validation_is_case_insensitive_about_the_extension() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("Wallpaper.PNG");
        std::fs::write(&path, b"pixels").expect("write the file");
        assert_eq!(
            super::validate_wallpaper(&path.display().to_string()).expect("validates"),
            path
        );
    }

    #[test]
    fn wallpaper_validation_rejects_a_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let err = super::validate_wallpaper(&dir.path().display().to_string())
            .expect_err("a directory is not a wallpaper");
        assert!(err.contains("Not a file"), "{err:?}");
    }

    #[test]
    fn spin_px_clamps_and_rounds_without_panicking() {
        assert_eq!(super::spin_px(0.0), 0);
        assert_eq!(super::spin_px(27.4), 27);
        assert_eq!(super::spin_px(27.6), 28);
        assert_eq!(super::spin_px(-5.0), 0, "below the range floor");
        assert_eq!(super::spin_px(9_000.0), 256, "above the range ceiling");
        assert_eq!(
            super::spin_px(f64::NAN),
            0,
            "a non-finite value is not a panic"
        );
        assert_eq!(super::spin_px(f64::INFINITY), 256);
    }
}
