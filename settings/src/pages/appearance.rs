//! The Appearance page: bar position/height, corner radius, snap gap,
//! palette colors, and wallpaper. Every widget's `connect_*` handler writes
//! straight into `ctx.model.working.appearance` and calls `ctx.mark_dirty()`.

use std::rc::Rc;

use gtk4::gdk::RGBA;
use gtk4::prelude::*;
use gtk4::{
    Align, Button, ColorDialog, ColorDialogButton, DropDown, FileDialog, Grid, Label, SpinButton,
};
use icedtea_ui::css::value::Rgba;
use icedtea_ui::widgets::color_dialog::ColorDialogC;

use crate::model::{BAR_POSITIONS, valid_hex};
use crate::pages::{Ctx, Page};

/// Parse a `#RRGGBB` string into an opaque toolkit colour; falls back to
/// opaque black for anything `valid_hex` rejects (should not happen for
/// values this page itself wrote, but keeps `view` panic-free against a
/// hand-edited db).
///
/// Same parse as the GDK version it replaces — `model::valid_hex` then
/// `u8::from_str_radix` per channel — with `icedtea_ui`'s colour type in
/// place of `gdk::RGBA`.
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

fn labeled_row(grid: &Grid, row: i32, text: &str, widget: &impl IsA<gtk4::Widget>) {
    let label = Label::new(Some(text));
    label.set_halign(Align::Start);
    grid.attach(&label, 0, row, 1, 1);
    grid.attach(widget, 1, row, 1, 1);
}

pub fn build(ctx: Ctx) -> Page {
    let grid = Grid::builder()
        .row_spacing(10)
        .column_spacing(16)
        .margin_top(16)
        .margin_bottom(16)
        .margin_start(16)
        .margin_end(16)
        .build();
    let mut row = 0;

    let bar_position = DropDown::from_strings(&BAR_POSITIONS);
    labeled_row(&grid, row, "Bar position", &bar_position);
    row += 1;
    {
        let ctx = ctx.clone();
        bar_position.connect_selected_notify(move |dd| {
            if ctx.populating.get() {
                return;
            }
            if let Some(pos) = BAR_POSITIONS.get(dd.selected() as usize) {
                ctx.model.borrow_mut().working.appearance.bar_position = pos.to_string();
            }
            ctx.mark_dirty();
        });
    }

    let bar_height = SpinButton::with_range(0.0, 256.0, 1.0);
    labeled_row(&grid, row, "Bar height", &bar_height);
    row += 1;
    {
        let ctx = ctx.clone();
        bar_height.connect_value_changed(move |sb| {
            if ctx.populating.get() {
                return;
            }
            ctx.model.borrow_mut().working.appearance.bar_height = sb.value() as i32;
            ctx.mark_dirty();
        });
    }

    let corner_radius = SpinButton::with_range(0.0, 256.0, 1.0);
    labeled_row(&grid, row, "Corner radius", &corner_radius);
    row += 1;
    {
        let ctx = ctx.clone();
        corner_radius.connect_value_changed(move |sb| {
            if ctx.populating.get() {
                return;
            }
            ctx.model.borrow_mut().working.appearance.corner_radius = sb.value() as i32;
            ctx.mark_dirty();
        });
    }

    let snap_gap = SpinButton::with_range(0.0, 256.0, 1.0);
    labeled_row(&grid, row, "Snap gap", &snap_gap);
    row += 1;
    {
        let ctx = ctx.clone();
        snap_gap.connect_value_changed(move |sb| {
            if ctx.populating.get() {
                return;
            }
            ctx.model.borrow_mut().working.appearance.snap_gap = sb.value() as i32;
            ctx.mark_dirty();
        });
    }

    let background = ColorDialogButton::new(Some(ColorDialog::new()));
    labeled_row(&grid, row, "Background", &background);
    row += 1;
    {
        let ctx = ctx.clone();
        background.connect_rgba_notify(move |btn| {
            if ctx.populating.get() {
                return;
            }
            let g = btn.rgba();
            ctx.model.borrow_mut().working.appearance.palette.background = rgba_to_hex(Rgba {
                r: g.red(),
                g: g.green(),
                b: g.blue(),
                a: g.alpha(),
            });
            ctx.mark_dirty();
        });
    }

    let foreground = ColorDialogButton::new(Some(ColorDialog::new()));
    labeled_row(&grid, row, "Foreground", &foreground);
    row += 1;
    {
        let ctx = ctx.clone();
        foreground.connect_rgba_notify(move |btn| {
            if ctx.populating.get() {
                return;
            }
            let g = btn.rgba();
            ctx.model.borrow_mut().working.appearance.palette.foreground = rgba_to_hex(Rgba {
                r: g.red(),
                g: g.green(),
                b: g.blue(),
                a: g.alpha(),
            });
            ctx.mark_dirty();
        });
    }

    let accent = ColorDialogButton::new(Some(ColorDialog::new()));
    labeled_row(&grid, row, "Accent", &accent);
    row += 1;
    {
        let ctx = ctx.clone();
        accent.connect_rgba_notify(move |btn| {
            if ctx.populating.get() {
                return;
            }
            let g = btn.rgba();
            ctx.model.borrow_mut().working.appearance.palette.accent = rgba_to_hex(Rgba {
                r: g.red(),
                g: g.green(),
                b: g.blue(),
                a: g.alpha(),
            });
            ctx.mark_dirty();
        });
    }

    let wallpaper_path = Label::new(None);
    wallpaper_path.set_halign(Align::Start);
    wallpaper_path.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
    labeled_row(&grid, row, "Wallpaper", &wallpaper_path);
    row += 1;

    let wallpaper_buttons = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    let choose = Button::with_label("Choose\u{2026}");
    let clear = Button::with_label("Clear");
    wallpaper_buttons.append(&choose);
    wallpaper_buttons.append(&clear);
    grid.attach(&wallpaper_buttons, 1, row, 1, 1);
    {
        let ctx = ctx.clone();
        let wallpaper_path = wallpaper_path.clone();
        choose.connect_clicked(move |_| {
            let parent = ctx.window.clone();
            let ctx = ctx.clone();
            let wallpaper_path = wallpaper_path.clone();
            FileDialog::builder()
                .title("Choose wallpaper")
                .build()
                .open(
                    Some(&parent),
                    None::<&gtk4::gio::Cancellable>,
                    move |result| {
                        if let Ok(file) = result
                            && let Some(path) = file.path()
                        {
                            let path = path.display().to_string();
                            wallpaper_path.set_label(&path);
                            ctx.model.borrow_mut().working.appearance.wallpaper = Some(path);
                            ctx.mark_dirty();
                        }
                    },
                );
        });
    }
    {
        let ctx = ctx.clone();
        let wallpaper_path = wallpaper_path.clone();
        clear.connect_clicked(move |_| {
            wallpaper_path.set_label("None");
            ctx.model.borrow_mut().working.appearance.wallpaper = None;
            ctx.mark_dirty();
        });
    }

    let refresh: Rc<dyn Fn()> = {
        let ctx = ctx.clone();
        let bar_position = bar_position.clone();
        let bar_height = bar_height.clone();
        let corner_radius = corner_radius.clone();
        let snap_gap = snap_gap.clone();
        let background = background.clone();
        let foreground = foreground.clone();
        let accent = accent.clone();
        let wallpaper_path = wallpaper_path.clone();
        Rc::new(move || {
            ctx.populating.set(true);
            let appearance = ctx.model.borrow().working.appearance.clone();
            let idx = BAR_POSITIONS
                .iter()
                .position(|p| *p == appearance.bar_position)
                .unwrap_or(0);
            bar_position.set_selected(idx as u32);
            bar_height.set_value(appearance.bar_height as f64);
            corner_radius.set_value(appearance.corner_radius as f64);
            snap_gap.set_value(appearance.snap_gap as f64);
            let c = hex_to_rgba(&appearance.palette.background);
            background.set_rgba(&RGBA::new(c.r, c.g, c.b, c.a));
            let c = hex_to_rgba(&appearance.palette.foreground);
            foreground.set_rgba(&RGBA::new(c.r, c.g, c.b, c.a));
            let c = hex_to_rgba(&appearance.palette.accent);
            accent.set_rgba(&RGBA::new(c.r, c.g, c.b, c.a));
            wallpaper_path.set_label(appearance.wallpaper.as_deref().unwrap_or("None"));
            ctx.populating.set(false);
        })
    };
    refresh();

    Page {
        root: grid.upcast(),
        refresh,
    }
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
