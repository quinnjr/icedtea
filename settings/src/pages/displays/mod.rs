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

pub mod state;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{
    Align, Box as GtkBox, Button, DrawingArea, DropDown, Frame, GestureDrag, Grid, Label,
    Orientation, SpinButton, StringList, Switch,
};

use crate::outputs::{Head, ModeRequest, OutputsClient, OutputsMsg};
use crate::pages::displays_canvas::{Rect, compute_view, hit_test, snap};

use state::{
    CANVAS_MARGIN, DisplaysState, SNAP_THRESHOLD, TRANSFORM_LABELS, TRANSFORM_VALUES, all_rects,
    baseline_edit, default_mode_for, distinct_resolutions, enabled_rects, format_refresh,
    reconcile, refreshes_for,
};

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
    // Whether a Test/Apply submission is currently outstanding. While it is,
    // both buttons are disabled so a second request can't overlap the first;
    // the terminal reply (`ApplySucceeded`/`ApplyFailed`/`ApplyCancelled`, or a
    // superseding `HeadsChanged`) clears it and re-enables them. Which kind of
    // request a given reply answers is no longer inferred from shared state —
    // it rides on the reply itself as `is_test` (tagged onto the configuration
    // object), so even a reply that arrives after the flag was cleared is read
    // with the meaning of the request that made it.
    let in_flight = Rc::new(Cell::new(false));

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

            // Draw every head (enabled and disabled) so a disabled monitor stays
            // visible and selectable; disabled ones are drawn greyed. (#10)
            let (idxs, rects, enabled) = all_rects(&st);
            let view = compute_view(&rects, width as f64, height as f64, CANVAS_MARGIN);
            st.view = view;

            cr.set_font_size(11.0);
            for (slot, rect) in rects.iter().enumerate() {
                let head_idx = idxs[slot];
                let is_enabled = enabled[slot];
                let c = view.to_canvas(rect);
                let selected = st.selected == Some(head_idx);
                // Monitor body — dimmed when the head is disabled.
                if is_enabled {
                    cr.set_source_rgb(0.26, 0.28, 0.36);
                } else {
                    cr.set_source_rgb(0.17, 0.18, 0.22);
                }
                cr.rectangle(c.x, c.y, c.w, c.h);
                let _ = cr.fill();
                // Border — accent when selected.
                if selected {
                    cr.set_source_rgb(0.54, 0.71, 0.98);
                    cr.set_line_width(2.5);
                } else if is_enabled {
                    cr.set_source_rgb(0.45, 0.47, 0.55);
                    cr.set_line_width(1.0);
                } else {
                    cr.set_source_rgb(0.34, 0.35, 0.42);
                    cr.set_line_width(1.0);
                }
                cr.rectangle(c.x, c.y, c.w, c.h);
                let _ = cr.stroke();
                // Labels: connector name, then resolution (or "off" when the
                // head is disabled).
                if is_enabled {
                    cr.set_source_rgb(0.90, 0.91, 0.95);
                } else {
                    cr.set_source_rgb(0.60, 0.61, 0.66);
                }
                let name = &st.heads[head_idx].name;
                let e = &st.edits[head_idx];
                let (w, h) = e
                    .mode
                    .map(|m| (m.width, m.height))
                    .unwrap_or_else(|| (rect.w.round() as i32, rect.h.round() as i32));
                cr.move_to(c.x + 6.0, c.y + 16.0);
                let _ = cr.show_text(name);
                cr.move_to(c.x + 6.0, c.y + 30.0);
                if is_enabled {
                    let _ = cr.show_text(&format!("{w}\u{d7}{h}"));
                } else {
                    let _ = cr.show_text("off");
                }
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
                    let res_labels: Vec<String> = res_options
                        .iter()
                        .map(|(w, h)| format!("{w}\u{d7}{h}"))
                        .collect();
                    set_dropdown(&res_dd, &res_labels, res_sel);
                    res_dd.set_sensitive(!res_options.is_empty());

                    let (sel_w, sel_h) =
                        res_options.get(res_sel).copied().unwrap_or((cur_w, cur_h));
                    let refresh_options = refreshes_for(&modes, sel_w, sel_h);
                    let cur_r = edit.mode.map(|m| m.refresh_mhz).unwrap_or(0);
                    let r_sel = refresh_options
                        .iter()
                        .position(|&r| r == cur_r)
                        .unwrap_or(0);
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

    // Recompute the footer sensitivity from the dirty flag. Apply is held
    // insensitive while a Test/Apply is outstanding so a HeadsChanged-driven
    // repopulate can't re-enable it under an in-flight request. (#15)
    let update_footer: Rc<dyn Fn()> = {
        let state = state.clone();
        let in_flight = in_flight.clone();
        let revert_btn = revert_btn.clone();
        let apply_btn = apply_btn.clone();
        Rc::new(move || {
            let dirty = state.borrow().dirty;
            revert_btn.set_sensitive(dirty);
            apply_btn.set_sensitive(dirty && !in_flight.get());
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

    // Reconcile a fresh head snapshot (HeadsChanged / initial enumerate) into
    // the page state, preserving pending edits + selection when the connector
    // set is unchanged and hard-resetting only on an actual hotplug/unplug.
    // Returns whether unapplied edits were discarded, so the caller can say so.
    let rebuild: Rc<dyn Fn(Vec<Head>) -> bool> = {
        let state = state.clone();
        let refresh_controls = refresh_controls.clone();
        let update_footer = update_footer.clone();
        let canvas = canvas.clone();
        Rc::new(move |heads: Vec<Head>| -> bool {
            let dropped = {
                let mut st = state.borrow_mut();
                let r = reconcile(&st.heads, &st.edits, st.selected, st.dirty, &heads);
                st.heads = heads;
                st.edits = r.edits;
                st.selected = r.selected;
                // Any in-flight drag was indexed against the *old* head list;
                // the new list may be shorter or reordered, so the cached index
                // is no longer valid. Drop it so drag_update can't dereference a
                // stale index. (#2)
                st.drag = None;
                if !r.compatible {
                    // A genuine set change resets to the fresh baseline, so
                    // there is nothing unsaved left.
                    st.dirty = false;
                }
                r.dropped
            };
            refresh_controls();
            update_footer();
            canvas.queue_draw();
            dropped
        })
    };

    // ---- control handlers ---------------------------------------------------
    {
        let state = state.clone();
        let populating = populating.clone();
        let refresh_controls = refresh_controls.clone();
        let update_footer = update_footer.clone();
        let canvas = canvas.clone();
        enabled_switch.connect_active_notify(move |sw| {
            if populating.get() {
                return;
            }
            let active = sw.is_active();
            let mut st = state.borrow_mut();
            if let Some(idx) = st.selected {
                st.edits[idx].enabled = active;
                // Enabling a mode-less head with no explicit mode would leave it
                // unplaceable on the canvas and applied at a mode the user never
                // saw; give it a concrete mode from the head's default so it is
                // visible and its mode/refresh edits take effect. (#9)
                if active && st.edits[idx].mode.is_none() {
                    let default_mode = st.heads.get(idx).and_then(default_mode_for);
                    if default_mode.is_some() {
                        st.edits[idx].mode = default_mode;
                    }
                }
                st.dirty = true;
            }
            drop(st);
            refresh_controls();
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
                    st.edits[idx].mode = Some(ModeRequest {
                        width: w,
                        height: h,
                        refresh_mhz: refresh,
                    });
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
        let refresh_controls = refresh_controls.clone();
        let update_footer = update_footer.clone();
        let canvas = canvas.clone();
        refresh_dd.connect_selected_notify(move |dd| {
            if populating.get() {
                return;
            }
            let sel = dd.selected() as usize;
            let mut created = false;
            {
                let mut st = state.borrow_mut();
                if let Some(idx) = st.selected
                    && let Some(&r) = st.refresh_options.get(sel)
                {
                    if let Some(mode) = st.edits[idx].mode.as_mut() {
                        mode.refresh_mhz = r;
                        st.dirty = true;
                    } else if let Some(&(w, h)) = st.res_options.first() {
                        // Mode-less head: the refresh list was built for the
                        // first resolution, so synthesize a full mode from it so
                        // the pick actually takes effect rather than being
                        // dropped for want of an existing mode. (#14)
                        st.edits[idx].mode = Some(ModeRequest {
                            width: w,
                            height: h,
                            refresh_mhz: r,
                        });
                        st.dirty = true;
                        created = true;
                    }
                }
            }
            // Repopulate so a freshly-created mode is reflected in the controls
            // and the canvas (it is now placeable).
            if created {
                refresh_controls();
                canvas.queue_draw();
            }
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
            // Hit-test against ALL heads (including disabled) so a disabled
            // monitor can be re-selected and switched back on. (#10)
            let (idxs, rects, _enabled) = all_rects(&st);
            let view = st.view;
            if let Some(slot) = hit_test(&rects, &view, x, y) {
                let head_idx = idxs[slot];
                let (sx, sy) = st.edits[head_idx].position.unwrap_or((0, 0));
                st.drag = Some(state::Drag {
                    head: head_idx,
                    origin: (x, y),
                    start: (sx, sy),
                });
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
            let Some(state::Drag {
                head: head_idx,
                start: (start_x, start_y),
                ..
            }) = st.drag
            else {
                return;
            };
            // A HeadsChanged mid-drag replaces `edits`/`heads` and clears
            // `drag`, but guard the cached index anyway: if it no longer refers
            // to a live head, the drag is stale — bail rather than index OOB. (#2)
            if head_idx >= st.edits.len() {
                st.drag = None;
                return;
            }
            let view = st.view;
            let (dx, dy) = view.canvas_delta_to_layout(ox, oy);
            let moved = Rect::new(start_x as f64 + dx, start_y as f64 + dy, 0.0, 0.0);
            // The dragged head's own geometry comes from the full set (it may be
            // disabled); snap targets are the *other* enabled heads plus the
            // origin.
            let (aidxs, arects, _enabled) = all_rects(&st);
            let dragged_rect = aidxs
                .iter()
                .position(|&i| i == head_idx)
                .map(|slot| arects[slot])
                .unwrap_or(moved);
            let (eidxs, erects) = enabled_rects(&st);
            let others: Vec<Rect> = eidxs
                .iter()
                .zip(erects.iter())
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
        let in_flight = in_flight.clone();
        let test_btn = test_btn.clone();
        let revert_btn = revert_btn.clone();
        let apply_btn = apply_btn.clone();
        Rc::new(move |available: bool| {
            content.set_visible(available);
            unavailable.set_visible(!available);
            // Don't re-enable Test while a request is still outstanding — an
            // unrelated HeadsChanged calls set_available(true) but must not open
            // a second, overlapping submission. (#15)
            test_btn.set_sensitive(available && !in_flight.get());
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

            // Test / Apply ship the current edit set down the protocol. On a
            // successful submit both buttons go insensitive until the terminal
            // reply re-enables them (#7), so no second request can overlap.
            {
                let state = state.clone();
                let client = client.clone();
                let status = status.clone();
                let in_flight = in_flight.clone();
                let test_btn_w = test_btn.clone();
                let apply_btn_w = apply_btn.clone();
                test_btn.connect_clicked(move |_| {
                    if in_flight.get() {
                        return;
                    }
                    let edits = state.borrow().edits.clone();
                    match client.test_configuration(&edits) {
                        Ok(()) => {
                            in_flight.set(true);
                            test_btn_w.set_sensitive(false);
                            apply_btn_w.set_sensitive(false);
                            status.set_text("Testing\u{2026}");
                        }
                        Err(err) => status.set_text(&format!("Test failed: {err}")),
                    }
                });
            }
            {
                let state = state.clone();
                let client = client.clone();
                let status = status.clone();
                let in_flight = in_flight.clone();
                let test_btn_w = test_btn.clone();
                let apply_btn_w = apply_btn.clone();
                apply_btn.connect_clicked(move |_| {
                    if in_flight.get() {
                        return;
                    }
                    let edits = state.borrow().edits.clone();
                    match client.build_and_send_configuration(&edits) {
                        Ok(()) => {
                            in_flight.set(true);
                            test_btn_w.set_sensitive(false);
                            apply_btn_w.set_sensitive(false);
                            status.set_text("Applying\u{2026}");
                        }
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
            let in_flight = in_flight.clone();
            let test_btn = test_btn.clone();
            glib::spawn_future_local(async move {
                // Clear the in-flight latch on a terminal reply and re-enable
                // the buttons: Test is available whenever the manager is (which
                // it is on any of these replies), and Apply follows the dirty
                // flag via `update_footer`.
                let end_request = {
                    let in_flight = in_flight.clone();
                    let test_btn = test_btn.clone();
                    let update_footer = update_footer.clone();
                    move || {
                        in_flight.set(false);
                        test_btn.set_sensitive(true);
                        update_footer();
                    }
                };
                while let Ok(msg) = rx.recv().await {
                    match msg {
                        OutputsMsg::HeadsChanged(heads) => {
                            // A HeadsChanged is NOT a terminal reply for an
                            // outstanding Test/Apply — the compositor still owes
                            // a Succeeded/Failed/Cancelled for that request. Leave
                            // `in_flight` set so an unrelated head-property update
                            // can't re-enable the buttons and let a second request
                            // overlap the first; only the terminal reply (below)
                            // clears it. (#15)
                            set_available(true);
                            let dropped = rebuild(heads);
                            if dropped {
                                status
                                    .set_text("Displays changed \u{2014} pending edits discarded");
                            } else {
                                status.set_text("");
                            }
                        }
                        OutputsMsg::ApplySucceeded { is_test } => {
                            end_request();
                            if is_test {
                                // A preview succeeded: keep the edits (and the
                                // Apply button) live so the user can commit them.
                                status.set_text("Test succeeded");
                            } else {
                                state.borrow_mut().dirty = false;
                                update_footer();
                                status.set_text("Applied");
                            }
                        }
                        OutputsMsg::ApplyFailed { is_test } => {
                            end_request();
                            if is_test {
                                // A preview was rejected: leave the pending edits
                                // intact so the user can adjust and retry.
                                status.set_text("Test rejected by the compositor");
                            } else {
                                reset_edits();
                                refresh_controls();
                                update_footer();
                                canvas.queue_draw();
                                status.set_text("Configuration rejected by the compositor");
                            }
                        }
                        OutputsMsg::ApplyCancelled => {
                            end_request();
                            status.set_text("Configuration superseded \u{2014} re-reading");
                        }
                        OutputsMsg::ManagerUnavailable | OutputsMsg::Disconnected => {
                            in_flight.set(false);
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
