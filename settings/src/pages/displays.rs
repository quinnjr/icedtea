//! The Displays page: a visual drag canvas of the connected monitors plus
//! per-monitor controls (enabled / resolution / refresh / scale / transform),
//! talking to the running compositor over `zwlr_output_management_v1` via the
//! GTK-free [`crate::outputs`] client.
//!
//! Unlike the other pages this one does **not** go through the redb `Config`
//! working-copy — the compositor persists applied layouts on its side. Edits
//! accumulate into a per-head [`HeadEdit`] set; **Test** and **Apply** ship
//! that set down the protocol, and results (or a lost connection) come back on
//! an async-channel drained from the glib main loop.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{
    Align, Box as GtkBox, Button, DrawingArea, DropDown, Frame, GestureDrag, Grid, Label,
    Orientation, SpinButton, StringList, Switch,
};

use crate::outputs::{Head, HeadEdit, Mode, ModeRequest, OutputsClient, OutputsMsg};
use crate::pages::displays_canvas::{compute_view, head_rect, hit_test, snap, Rect, View};

/// Canvas padding (canvas px) kept clear around the scaled monitor layout.
const CANVAS_MARGIN: f64 = 16.0;
/// How close (layout px) a dragged edge must come before it snaps.
const SNAP_THRESHOLD: f64 = 40.0;

/// The four transform choices the UI exposes (raw `wl_output.transform`).
const TRANSFORM_LABELS: [&str; 4] = ["Normal", "90\u{b0}", "180\u{b0}", "270\u{b0}"];
const TRANSFORM_VALUES: [i32; 4] = [0, 1, 2, 3];

/// Everything the page mutates as the user drags and edits. `heads` is the
/// last snapshot the compositor sent; `edits` is parallel to it (same index)
/// and holds the pending change for each head.
struct DisplaysState {
    heads: Vec<Head>,
    edits: Vec<HeadEdit>,
    selected: Option<usize>,
    dirty: bool,
    /// The layout→canvas mapping from the last draw, reused for drag maths.
    view: View,
    /// An in-flight drag: `(head index, start layout x, start layout y)`.
    drag: Option<(usize, i32, i32)>,
    /// Option lists backing the resolution/refresh dropdowns for the selected
    /// head, so a dropdown index maps back to a concrete value.
    res_options: Vec<(i32, i32)>,
    refresh_options: Vec<i32>,
}

impl DisplaysState {
    fn new() -> Self {
        DisplaysState {
            heads: Vec::new(),
            edits: Vec::new(),
            selected: None,
            dirty: false,
            view: View { scale: 1.0, off_x: CANVAS_MARGIN, off_y: CANVAS_MARGIN },
            drag: None,
            res_options: Vec::new(),
            refresh_options: Vec::new(),
        }
    }
}

/// A fresh edit mirroring a head's current state — the baseline the controls
/// mutate away from.
fn baseline_edit(h: &Head) -> HeadEdit {
    HeadEdit {
        name: h.name.clone(),
        enabled: h.enabled,
        mode: h
            .current_mode
            .map(|m| ModeRequest { width: m.width, height: m.height, refresh_mhz: m.refresh_mhz }),
        position: Some((h.x, h.y)),
        scale: Some(h.scale),
        transform: Some(h.transform),
    }
}

/// Distinct `(w, h)` resolutions a head advertises, largest area first.
fn distinct_resolutions(modes: &[Mode]) -> Vec<(i32, i32)> {
    let mut out: Vec<(i32, i32)> = Vec::new();
    for m in modes {
        if !out.contains(&(m.width, m.height)) {
            out.push((m.width, m.height));
        }
    }
    out.sort_by_key(|&(w, h)| std::cmp::Reverse(w as i64 * h as i64));
    out
}

/// Refresh rates (mHz) a head offers at a given resolution, highest first.
fn refreshes_for(modes: &[Mode], w: i32, h: i32) -> Vec<i32> {
    let mut out: Vec<i32> =
        modes.iter().filter(|m| m.width == w && m.height == h).map(|m| m.refresh_mhz).collect();
    out.sort_unstable_by(|a, b| b.cmp(a));
    out.dedup();
    out
}

fn format_refresh(mhz: i32) -> String {
    format!("{:.2} Hz", mhz as f64 / 1000.0)
}

/// The enabled heads' layout rects, and the head indices they map back to.
fn enabled_rects(st: &DisplaysState) -> (Vec<usize>, Vec<Rect>) {
    let mut idxs = Vec::new();
    let mut rects = Vec::new();
    for (i, e) in st.edits.iter().enumerate() {
        if !e.enabled {
            continue;
        }
        let (w, h) = e
            .mode
            .map(|m| (m.width, m.height))
            .or_else(|| st.heads[i].current_mode.map(|m| (m.width, m.height)))
            .unwrap_or((0, 0));
        if w == 0 || h == 0 {
            continue;
        }
        let scale = e.scale.unwrap_or(1.0);
        let transform = e.transform.unwrap_or(0);
        let (x, y) = e.position.unwrap_or((0, 0));
        idxs.push(i);
        rects.push(head_rect(w, h, scale, transform, x, y));
    }
    (idxs, rects)
}

/// Replace a dropdown's contents and select `selected`.
fn set_dropdown(dd: &DropDown, labels: &[String], selected: usize) {
    let refs: Vec<&str> = labels.iter().map(|s| s.as_str()).collect();
    dd.set_model(Some(&StringList::new(&refs)));
    if !labels.is_empty() {
        dd.set_selected(selected.min(labels.len() - 1) as u32);
    }
}

/// Build the Displays page root widget, wiring it to a live [`OutputsClient`].
pub fn build() -> gtk4::Widget {
    let state = Rc::new(RefCell::new(DisplaysState::new()));
    let populating = Rc::new(Cell::new(false));

    let root = GtkBox::new(Orientation::Vertical, 8);
    root.set_margin_top(12);
    root.set_margin_bottom(12);
    root.set_margin_start(12);
    root.set_margin_end(12);

    // Shown instead of the controls when there is no compositor to talk to.
    let unavailable = Label::new(Some(
        "output management unavailable \u{2014} is the icedtea compositor running?",
    ));
    unavailable.set_wrap(true);
    unavailable.set_halign(Align::Center);
    unavailable.set_valign(Align::Center);
    unavailable.set_vexpand(true);
    unavailable.set_visible(false);
    root.append(&unavailable);

    // The live content: canvas on the left, per-head controls on the right.
    let content = GtkBox::new(Orientation::Horizontal, 12);
    content.set_vexpand(true);
    root.append(&content);

    let canvas = DrawingArea::new();
    canvas.set_hexpand(true);
    canvas.set_vexpand(true);
    canvas.set_content_width(360);
    canvas.set_content_height(240);
    let canvas_frame = Frame::new(None);
    canvas_frame.set_child(Some(&canvas));
    content.append(&canvas_frame);

    let controls = Grid::builder().row_spacing(8).column_spacing(12).build();
    content.append(&controls);

    let enabled_switch = Switch::new();
    enabled_switch.set_halign(Align::Start);
    labeled(&controls, 0, "Enabled", &enabled_switch);

    let res_dd = DropDown::from_strings(&[]);
    labeled(&controls, 1, "Resolution", &res_dd);

    let refresh_dd = DropDown::from_strings(&[]);
    labeled(&controls, 2, "Refresh", &refresh_dd);

    let scale_spin = SpinButton::with_range(0.5, 4.0, 0.25);
    scale_spin.set_digits(2);
    labeled(&controls, 3, "Scale", &scale_spin);

    let transform_dd = DropDown::from_strings(&TRANSFORM_LABELS);
    labeled(&controls, 4, "Transform", &transform_dd);

    let position_label = Label::new(Some("\u{2014}"));
    position_label.set_halign(Align::Start);
    labeled(&controls, 5, "Position", &position_label);

    // Footer: status + Test / Revert / Apply (the page's own, protocol-bound
    // controls — the shared model footer does not touch displays).
    let footer = GtkBox::new(Orientation::Horizontal, 8);
    let status = Label::new(None);
    status.set_hexpand(true);
    status.set_halign(Align::Start);
    let test_btn = Button::with_label("Test");
    let revert_btn = Button::with_label("Revert");
    let apply_btn = Button::with_label("Apply");
    footer.append(&status);
    footer.append(&test_btn);
    footer.append(&revert_btn);
    footer.append(&apply_btn);
    root.append(&footer);

    // ---- draw ---------------------------------------------------------------
    {
        let state = state.clone();
        canvas.set_draw_func(move |_area, cr, width, height| {
            let mut st = state.borrow_mut();
            // Neutral background.
            cr.set_source_rgb(0.12, 0.12, 0.16);
            cr.rectangle(0.0, 0.0, width as f64, height as f64);
            let _ = cr.fill();

            let (idxs, rects) = enabled_rects(&st);
            let view = compute_view(&rects, width as f64, height as f64, CANVAS_MARGIN);
            st.view = view;

            cr.set_font_size(11.0);
            for (slot, rect) in rects.iter().enumerate() {
                let head_idx = idxs[slot];
                let c = view.to_canvas(rect);
                let selected = st.selected == Some(head_idx);
                // Monitor body.
                cr.set_source_rgb(0.26, 0.28, 0.36);
                cr.rectangle(c.x, c.y, c.w, c.h);
                let _ = cr.fill();
                // Border — accent when selected.
                if selected {
                    cr.set_source_rgb(0.54, 0.71, 0.98);
                    cr.set_line_width(2.5);
                } else {
                    cr.set_source_rgb(0.45, 0.47, 0.55);
                    cr.set_line_width(1.0);
                }
                cr.rectangle(c.x, c.y, c.w, c.h);
                let _ = cr.stroke();
                // Labels: connector name, then resolution.
                cr.set_source_rgb(0.90, 0.91, 0.95);
                let name = &st.heads[head_idx].name;
                let e = &st.edits[head_idx];
                let (w, h) = e
                    .mode
                    .map(|m| (m.width, m.height))
                    .unwrap_or_else(|| (rect.w.round() as i32, rect.h.round() as i32));
                cr.move_to(c.x + 6.0, c.y + 16.0);
                let _ = cr.show_text(name);
                cr.move_to(c.x + 6.0, c.y + 30.0);
                let _ = cr.show_text(&format!("{w}\u{d7}{h}"));
            }
        });
    }

    // ---- control repopulation (Revert / selection / HeadsChanged) -----------
    let refresh_controls: Rc<dyn Fn()> = {
        let state = state.clone();
        let populating = populating.clone();
        let enabled_switch = enabled_switch.clone();
        let res_dd = res_dd.clone();
        let refresh_dd = refresh_dd.clone();
        let scale_spin = scale_spin.clone();
        let transform_dd = transform_dd.clone();
        let position_label = position_label.clone();
        Rc::new(move || {
            populating.set(true);
            let mut st = state.borrow_mut();
            match st.selected {
                Some(idx) if idx < st.heads.len() => {
                    let modes = st.heads[idx].modes.clone();
                    let edit = st.edits[idx].clone();
                    enabled_switch.set_sensitive(true);
                    enabled_switch.set_active(edit.enabled);

                    let res_options = distinct_resolutions(&modes);
                    let (cur_w, cur_h) = edit.mode.map(|m| (m.width, m.height)).unwrap_or((0, 0));
                    let res_sel = res_options
                        .iter()
                        .position(|&(w, h)| w == cur_w && h == cur_h)
                        .unwrap_or(0);
                    let res_labels: Vec<String> =
                        res_options.iter().map(|(w, h)| format!("{w}\u{d7}{h}")).collect();
                    set_dropdown(&res_dd, &res_labels, res_sel);
                    res_dd.set_sensitive(!res_options.is_empty());

                    let (sel_w, sel_h) = res_options.get(res_sel).copied().unwrap_or((cur_w, cur_h));
                    let refresh_options = refreshes_for(&modes, sel_w, sel_h);
                    let cur_r = edit.mode.map(|m| m.refresh_mhz).unwrap_or(0);
                    let r_sel =
                        refresh_options.iter().position(|&r| r == cur_r).unwrap_or(0);
                    let r_labels: Vec<String> =
                        refresh_options.iter().map(|&r| format_refresh(r)).collect();
                    set_dropdown(&refresh_dd, &r_labels, r_sel);
                    refresh_dd.set_sensitive(!refresh_options.is_empty());

                    scale_spin.set_sensitive(true);
                    scale_spin.set_value(edit.scale.unwrap_or(1.0));

                    transform_dd.set_sensitive(true);
                    let t = edit.transform.unwrap_or(0);
                    let t_sel = TRANSFORM_VALUES.iter().position(|&v| v == t).unwrap_or(0);
                    transform_dd.set_selected(t_sel as u32);

                    let (px, py) = edit.position.unwrap_or((0, 0));
                    position_label.set_text(&format!("{px}, {py}"));

                    st.res_options = res_options;
                    st.refresh_options = refresh_options;
                }
                _ => {
                    enabled_switch.set_active(false);
                    enabled_switch.set_sensitive(false);
                    set_dropdown(&res_dd, &[], 0);
                    res_dd.set_sensitive(false);
                    set_dropdown(&refresh_dd, &[], 0);
                    refresh_dd.set_sensitive(false);
                    scale_spin.set_sensitive(false);
                    transform_dd.set_sensitive(false);
                    position_label.set_text("\u{2014}");
                    st.res_options.clear();
                    st.refresh_options.clear();
                }
            }
            drop(st);
            populating.set(false);
        })
    };

    // Recompute the footer sensitivity from the dirty flag.
    let update_footer: Rc<dyn Fn()> = {
        let state = state.clone();
        let revert_btn = revert_btn.clone();
        let apply_btn = apply_btn.clone();
        Rc::new(move || {
            let dirty = state.borrow().dirty;
            revert_btn.set_sensitive(dirty);
            apply_btn.set_sensitive(dirty);
        })
    };

    // Reset the pending edits back to the last-known head snapshot.
    let reset_edits: Rc<dyn Fn()> = {
        let state = state.clone();
        Rc::new(move || {
            let mut st = state.borrow_mut();
            st.edits = st.heads.iter().map(baseline_edit).collect();
            st.dirty = false;
            if st.selected.map(|i| i >= st.heads.len()).unwrap_or(true) {
                st.selected = if st.heads.is_empty() { None } else { Some(0) };
            }
        })
    };

    // Rebuild everything from a fresh head snapshot (HeadsChanged).
    let rebuild: Rc<dyn Fn(Vec<Head>)> = {
        let state = state.clone();
        let reset_edits = reset_edits.clone();
        let refresh_controls = refresh_controls.clone();
        let update_footer = update_footer.clone();
        let canvas = canvas.clone();
        Rc::new(move |heads: Vec<Head>| {
            {
                let mut st = state.borrow_mut();
                st.heads = heads;
                st.selected = if st.heads.is_empty() { None } else { Some(0) };
            }
            reset_edits();
            refresh_controls();
            update_footer();
            canvas.queue_draw();
        })
    };

    // ---- control handlers ---------------------------------------------------
    {
        let state = state.clone();
        let populating = populating.clone();
        let update_footer = update_footer.clone();
        let canvas = canvas.clone();
        enabled_switch.connect_active_notify(move |sw| {
            if populating.get() {
                return;
            }
            let mut st = state.borrow_mut();
            if let Some(idx) = st.selected {
                st.edits[idx].enabled = sw.is_active();
                st.dirty = true;
            }
            drop(st);
            update_footer();
            canvas.queue_draw();
        });
    }
    {
        let state = state.clone();
        let populating = populating.clone();
        let refresh_controls = refresh_controls.clone();
        let update_footer = update_footer.clone();
        let canvas = canvas.clone();
        res_dd.connect_selected_notify(move |dd| {
            if populating.get() {
                return;
            }
            let sel = dd.selected() as usize;
            let mut changed = false;
            {
                let mut st = state.borrow_mut();
                if let Some(idx) = st.selected
                    && let Some(&(w, h)) = st.res_options.get(sel)
                {
                    let modes = st.heads[idx].modes.clone();
                    let refreshes = refreshes_for(&modes, w, h);
                    // Keep the current refresh if this resolution offers it,
                    // else take its highest.
                    let cur_r = st.edits[idx].mode.map(|m| m.refresh_mhz);
                    let refresh = cur_r
                        .filter(|r| refreshes.contains(r))
                        .or_else(|| refreshes.first().copied())
                        .unwrap_or(0);
                    st.edits[idx].mode =
                        Some(ModeRequest { width: w, height: h, refresh_mhz: refresh });
                    st.dirty = true;
                    changed = true;
                }
            }
            if changed {
                refresh_controls();
                update_footer();
                canvas.queue_draw();
            }
        });
    }
    {
        let state = state.clone();
        let populating = populating.clone();
        let update_footer = update_footer.clone();
        refresh_dd.connect_selected_notify(move |dd| {
            if populating.get() {
                return;
            }
            let sel = dd.selected() as usize;
            let mut st = state.borrow_mut();
            if let Some(idx) = st.selected
                && let Some(&r) = st.refresh_options.get(sel)
                && let Some(mode) = st.edits[idx].mode.as_mut()
            {
                mode.refresh_mhz = r;
                st.dirty = true;
            }
            drop(st);
            update_footer();
        });
    }
    {
        let state = state.clone();
        let populating = populating.clone();
        let update_footer = update_footer.clone();
        let canvas = canvas.clone();
        scale_spin.connect_value_changed(move |sb| {
            if populating.get() {
                return;
            }
            let mut st = state.borrow_mut();
            if let Some(idx) = st.selected {
                st.edits[idx].scale = Some(sb.value());
                st.dirty = true;
            }
            drop(st);
            update_footer();
            canvas.queue_draw();
        });
    }
    {
        let state = state.clone();
        let populating = populating.clone();
        let update_footer = update_footer.clone();
        let canvas = canvas.clone();
        transform_dd.connect_selected_notify(move |dd| {
            if populating.get() {
                return;
            }
            let sel = dd.selected() as usize;
            let mut st = state.borrow_mut();
            if let Some(idx) = st.selected {
                let t = TRANSFORM_VALUES.get(sel).copied().unwrap_or(0);
                st.edits[idx].transform = Some(t);
                st.dirty = true;
            }
            drop(st);
            update_footer();
            canvas.queue_draw();
        });
    }

    // ---- drag gesture -------------------------------------------------------
    let drag = GestureDrag::new();
    {
        let state = state.clone();
        let refresh_controls = refresh_controls.clone();
        let canvas = canvas.clone();
        drag.connect_drag_begin(move |_g, x, y| {
            let mut st = state.borrow_mut();
            let (idxs, rects) = enabled_rects(&st);
            let view = st.view;
            if let Some(slot) = hit_test(&rects, &view, x, y) {
                let head_idx = idxs[slot];
                let (sx, sy) = st.edits[head_idx].position.unwrap_or((0, 0));
                st.drag = Some((head_idx, sx, sy));
                st.selected = Some(head_idx);
                drop(st);
                refresh_controls();
                canvas.queue_draw();
            } else {
                st.drag = None;
            }
        });
    }
    {
        let state = state.clone();
        let update_footer = update_footer.clone();
        let canvas = canvas.clone();
        let position_label = position_label.clone();
        drag.connect_drag_update(move |_g, ox, oy| {
            let mut st = state.borrow_mut();
            let Some((head_idx, start_x, start_y)) = st.drag else {
                return;
            };
            let view = st.view;
            let (dx, dy) = view.canvas_delta_to_layout(ox, oy);
            let moved = Rect::new(start_x as f64 + dx, start_y as f64 + dy, 0.0, 0.0);
            // Snap against the *other* enabled heads and the origin.
            let (idxs, rects) = enabled_rects(&st);
            let dragged_rect = idxs
                .iter()
                .position(|&i| i == head_idx)
                .map(|slot| rects[slot])
                .unwrap_or(moved);
            let others: Vec<Rect> = idxs
                .iter()
                .zip(rects.iter())
                .filter(|&(&i, _)| i != head_idx)
                .map(|(_, r)| *r)
                .collect();
            let candidate = Rect::new(moved.x, moved.y, dragged_rect.w, dragged_rect.h);
            let (nx, ny) = snap(candidate, &others, SNAP_THRESHOLD);
            st.edits[head_idx].position = Some((nx, ny));
            st.dirty = true;
            position_label.set_text(&format!("{nx}, {ny}"));
            drop(st);
            update_footer();
            canvas.queue_draw();
        });
    }
    {
        let state = state.clone();
        drag.connect_drag_end(move |_g, _ox, _oy| {
            state.borrow_mut().drag = None;
        });
    }
    canvas.add_controller(drag);

    // ---- outputs client + message pump --------------------------------------
    let (tx, rx) = async_channel::unbounded::<OutputsMsg>();
    let main_context = glib::MainContext::default();
    let client = OutputsClient::spawn(&main_context, tx);

    let set_available: Rc<dyn Fn(bool)> = {
        let content = content.clone();
        let unavailable = unavailable.clone();
        let test_btn = test_btn.clone();
        let revert_btn = revert_btn.clone();
        let apply_btn = apply_btn.clone();
        Rc::new(move |available: bool| {
            content.set_visible(available);
            unavailable.set_visible(!available);
            test_btn.set_sensitive(available);
            if !available {
                revert_btn.set_sensitive(false);
                apply_btn.set_sensitive(false);
            }
        })
    };

    match client {
        Ok(client) => {
            let client = Rc::new(client);
            if client.manager_present() {
                set_available(true);
                rebuild(client.heads());
            } else {
                set_available(false);
            }

            // Test / Apply ship the current edit set down the protocol.
            {
                let state = state.clone();
                let client = client.clone();
                let status = status.clone();
                test_btn.connect_clicked(move |_| {
                    let edits = state.borrow().edits.clone();
                    match client.test_configuration(&edits) {
                        Ok(()) => status.set_text("Testing\u{2026}"),
                        Err(err) => status.set_text(&format!("Test failed: {err}")),
                    }
                });
            }
            {
                let state = state.clone();
                let client = client.clone();
                let status = status.clone();
                apply_btn.connect_clicked(move |_| {
                    let edits = state.borrow().edits.clone();
                    match client.build_and_send_configuration(&edits) {
                        Ok(()) => status.set_text("Applying\u{2026}"),
                        Err(err) => status.set_text(&format!("Apply failed: {err}")),
                    }
                });
            }
            {
                let reset_edits = reset_edits.clone();
                let refresh_controls = refresh_controls.clone();
                let update_footer = update_footer.clone();
                let canvas = canvas.clone();
                let status = status.clone();
                revert_btn.connect_clicked(move |_| {
                    reset_edits();
                    refresh_controls();
                    update_footer();
                    canvas.queue_draw();
                    status.set_text("");
                });
            }

            // Drain outputs messages on the glib main loop.
            let status = status.clone();
            glib::spawn_future_local(async move {
                while let Ok(msg) = rx.recv().await {
                    match msg {
                        OutputsMsg::HeadsChanged(heads) => {
                            set_available(true);
                            rebuild(heads);
                            status.set_text("");
                        }
                        OutputsMsg::ApplySucceeded => {
                            state.borrow_mut().dirty = false;
                            update_footer();
                            status.set_text("Applied");
                        }
                        OutputsMsg::ApplyFailed => {
                            reset_edits();
                            refresh_controls();
                            update_footer();
                            canvas.queue_draw();
                            status.set_text("Configuration rejected by the compositor");
                        }
                        OutputsMsg::ApplyCancelled => {
                            status.set_text("Configuration superseded \u{2014} re-reading");
                        }
                        OutputsMsg::ManagerUnavailable | OutputsMsg::Disconnected => {
                            set_available(false);
                            status.set_text("");
                        }
                    }
                }
            });
        }
        Err(err) => {
            // No second wayland connection at all (e.g. no WAYLAND_DISPLAY).
            tracing::warn!(%err, "outputs client unavailable");
            set_available(false);
        }
    }

    root.upcast()
}

/// Attach a labelled control to `grid` at `row`.
fn labeled(grid: &Grid, row: i32, text: &str, widget: &impl IsA<gtk4::Widget>) {
    let label = Label::new(Some(text));
    label.set_halign(Align::Start);
    grid.attach(&label, 0, row, 1, 1);
    grid.attach(widget, 1, row, 1, 1);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(w: i32, h: i32, r: i32, preferred: bool) -> Mode {
        Mode { width: w, height: h, refresh_mhz: r, preferred }
    }

    #[test]
    fn distinct_resolutions_dedups_and_orders_by_area() {
        let modes = [
            mode(1920, 1080, 60000, true),
            mode(1920, 1080, 59940, false),
            mode(3840, 2160, 30000, false),
            mode(1280, 720, 60000, false),
        ];
        let res = distinct_resolutions(&modes);
        assert_eq!(res, vec![(3840, 2160), (1920, 1080), (1280, 720)]);
    }

    #[test]
    fn refreshes_for_filters_by_resolution_highest_first() {
        let modes = [
            mode(1920, 1080, 60000, true),
            mode(1920, 1080, 59940, false),
            mode(1920, 1080, 60000, false),
            mode(3840, 2160, 30000, false),
        ];
        // Only the 1080p refreshes, deduped, highest first.
        assert_eq!(refreshes_for(&modes, 1920, 1080), vec![60000, 59940]);
        assert_eq!(refreshes_for(&modes, 3840, 2160), vec![30000]);
        assert_eq!(refreshes_for(&modes, 800, 600), Vec::<i32>::new());
    }

    #[test]
    fn baseline_edit_mirrors_head() {
        let head = Head {
            name: "DP-1".into(),
            description: "Test".into(),
            enabled: true,
            modes: vec![mode(1920, 1080, 60000, true)],
            current_mode: Some(mode(1920, 1080, 60000, true)),
            x: 10,
            y: 20,
            scale: 1.5,
            transform: 2,
        };
        let e = baseline_edit(&head);
        assert_eq!(e.name, "DP-1");
        assert!(e.enabled);
        assert_eq!(e.mode, Some(ModeRequest { width: 1920, height: 1080, refresh_mhz: 60000 }));
        assert_eq!(e.position, Some((10, 20)));
        assert_eq!(e.scale, Some(1.5));
        assert_eq!(e.transform, Some(2));
    }
}
