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
}
