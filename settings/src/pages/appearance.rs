//! The Appearance page: bar position/height, corner radius, snap gap,
//! palette colors, and wallpaper.

use icedtea_ui::css::value::Rgba;
use icedtea_ui::widgets::color_dialog::ColorDialogC;

use crate::model::valid_hex;

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

/// The Appearance page.
///
/// P1 ships the page's frame only; P2 fills it in (contract §2.6).
#[must_use]
pub fn view(m: &crate::app::SettingsModel) -> icedtea_ui::view::View<crate::app::Msg> {
    let _ = m;
    icedtea_ui::view::builders::box_(
        icedtea_ui::widgets::types::Orientation::Vertical,
        [icedtea_ui::view::builders::label("Appearance")],
    )
    .id("appearance_page")
    .margin(16, 16, 16, 16)
}

#[cfg(test)]
mod tests {
    use super::{hex_to_packed, hex_to_rgba, packed_to_hex, rgba_to_hex};

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
