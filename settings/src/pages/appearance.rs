//! The Appearance page: bar position/height, corner radius, snap gap,
//! palette colors, and wallpaper. Every widget's `connect_*` handler writes
//! straight into `ctx.model.working.appearance` and calls `ctx.mark_dirty()`.

use std::rc::Rc;

use gtk4::gdk::RGBA;
use gtk4::prelude::*;
use gtk4::{
    Align, Button, ColorDialog, ColorDialogButton, DropDown, FileDialog, Grid, Label, SpinButton,
};

use crate::model::{BAR_POSITIONS, valid_hex};
use crate::pages::{Ctx, Page};

/// Parse a `#RRGGBB` string into an opaque `RGBA`; falls back to black for
/// anything `valid_hex` rejects (should not happen for values this page
/// itself wrote, but keeps `refresh()` panic-free against a hand-edited db).
fn hex_to_rgba(s: &str) -> RGBA {
    if !valid_hex(s) {
        return RGBA::BLACK;
    }
    let digits = &s[1..];
    let byte = |i: usize| u8::from_str_radix(&digits[i..i + 2], 16).unwrap_or(0);
    RGBA::new(
        byte(0) as f32 / 255.0,
        byte(2) as f32 / 255.0,
        byte(4) as f32 / 255.0,
        1.0,
    )
}

/// Format an `RGBA`'s color channels (alpha is ignored -- the palette has no
/// transparency concept) as `#RRGGBB`.
fn rgba_to_hex(c: &RGBA) -> String {
    let chan = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!(
        "#{:02x}{:02x}{:02x}",
        chan(c.red()),
        chan(c.green()),
        chan(c.blue())
    )
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
            ctx.model.borrow_mut().working.appearance.palette.background = rgba_to_hex(&btn.rgba());
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
            ctx.model.borrow_mut().working.appearance.palette.foreground = rgba_to_hex(&btn.rgba());
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
            ctx.model.borrow_mut().working.appearance.palette.accent = rgba_to_hex(&btn.rgba());
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
            background.set_rgba(&hex_to_rgba(&appearance.palette.background));
            foreground.set_rgba(&hex_to_rgba(&appearance.palette.foreground));
            accent.set_rgba(&hex_to_rgba(&appearance.palette.accent));
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
    use super::*;

    #[test]
    fn hex_rgba_round_trips() {
        for hex in ["#1e1e2e", "#cdd6f4", "#89b4fa", "#000000", "#ffffff"] {
            let rgba = hex_to_rgba(hex);
            assert_eq!(rgba_to_hex(&rgba), hex);
        }
    }

    #[test]
    fn invalid_hex_falls_back_to_black_without_panicking() {
        assert_eq!(hex_to_rgba("not-a-color"), RGBA::BLACK);
    }
}
