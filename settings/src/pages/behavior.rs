//! The Behavior page: three boolean switches, each writing straight into
//! `ctx.model.working.behavior` and calling `ctx.mark_dirty()`.

use std::rc::Rc;

use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{Align, Grid, Label, Switch};

use crate::pages::{Ctx, Page};

/// The Behavior page.
///
/// P1 ships the page's frame only; P2 fills it in (contract §2.6).
#[must_use]
pub fn view(m: &crate::app::SettingsModel) -> icedtea_ui::view::View<crate::app::Msg> {
    let _ = m;
    icedtea_ui::view::builders::box_(
        icedtea_ui::widgets::types::Orientation::Vertical,
        [icedtea_ui::view::builders::label("Behavior")],
    )
    .id("behavior_page")
    .margin(16, 16, 16, 16)
}

fn labeled_row(grid: &Grid, row: i32, text: &str, widget: &Switch) {
    let label = Label::new(Some(text));
    label.set_halign(Align::Start);
    widget.set_halign(Align::Start);
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

    let raise_on_focus = Switch::new();
    labeled_row(&grid, 0, "Raise on focus", &raise_on_focus);
    {
        let ctx = ctx.clone();
        raise_on_focus.connect_state_set(move |_, active| {
            ctx.model.borrow_mut().working.behavior.raise_on_focus = active;
            ctx.mark_dirty();
            glib::Propagation::Proceed
        });
    }

    let hide_bar_on_fullscreen = Switch::new();
    labeled_row(&grid, 1, "Hide bar on fullscreen", &hide_bar_on_fullscreen);
    {
        let ctx = ctx.clone();
        hide_bar_on_fullscreen.connect_state_set(move |_, active| {
            ctx.model
                .borrow_mut()
                .working
                .behavior
                .hide_bar_on_fullscreen = active;
            ctx.mark_dirty();
            glib::Propagation::Proceed
        });
    }

    let snap_enabled = Switch::new();
    labeled_row(&grid, 2, "Snap enabled", &snap_enabled);
    {
        let ctx = ctx.clone();
        snap_enabled.connect_state_set(move |_, active| {
            ctx.model.borrow_mut().working.behavior.snap_enabled = active;
            ctx.mark_dirty();
            glib::Propagation::Proceed
        });
    }

    let refresh: Rc<dyn Fn()> = {
        let ctx = ctx.clone();
        let raise_on_focus = raise_on_focus.clone();
        let hide_bar_on_fullscreen = hide_bar_on_fullscreen.clone();
        let snap_enabled = snap_enabled.clone();
        Rc::new(move || {
            ctx.populating.set(true);
            let behavior = ctx.model.borrow().working.behavior.clone();
            raise_on_focus.set_active(behavior.raise_on_focus);
            hide_bar_on_fullscreen.set_active(behavior.hide_bar_on_fullscreen);
            snap_enabled.set_active(behavior.snap_enabled);
            ctx.populating.set(false);
        })
    };
    refresh();

    Page {
        root: grid.upcast(),
        refresh,
    }
}
