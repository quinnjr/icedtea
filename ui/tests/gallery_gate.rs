//! The M3 rest-state gate.
//!
//! The gallery renders every `Kind` the toolkit ships; this file proves each
//! one actually reaches a real compositor's screen, in all three bundled
//! Adwaita sheets, at coordinates it learned from the binary rather than from
//! a literal.

mod support;

use icedtea_harness::{Compositor, ScreencopyClient};
use icedtea_ui::gallery::{
    GalleryModel, Sample, SampleShape, kind_name, own_kinds, sample, sample_shape,
};
use icedtea_ui::view::Kind;
use icedtea_ui::widgets::{fixture_matches, node_tree_of};
use support::{
    EntryAllocation, ProbePoint, allocation_sized, entry_allocations, paints_something,
    parse_allocation_line, parse_probe_line, pixel_at, probe_points, spawn_gallery,
    spawn_gallery_widget_sized, wait_for_gallery,
};

/// How far the page scrolls between captures.
///
/// Reconciliation: the task text pins this at a literal `700`, "a little less
/// than the surface height", against its assumed 800px surface. Two things
/// break that literal. The capture is bounded by the *output*, which is 720px
/// here, not by the surface (see [`visible_in_slice`]); and an entry only
/// counts as walked when it is visible *whole*, so the overlap between two
/// slices has to be at least as tall as the tallest entry on the page. With a
/// 720px capture and a 700px step the overlap is 20px, and `scrolled_window`
/// (229px tall) and `list_box` (72px) straddle a slice boundary in every
/// direction and are never seen whole, so the `missing` assertion fails on a
/// page that renders perfectly.
///
/// Deriving the step keeps the guarantee explicit: an entry at `y` of height
/// `h` is whole in the slice at `s` exactly when `s` is in
/// `[y + h - (capturable - 2), y - 2]`, an interval of length
/// `capturable - h - 4`. Making the step no larger than that for the *tallest*
/// entry means every such interval spans at least one multiple of the step, so
/// every entry is seen whole in some slice, whatever the output's height.
fn slice_step(capturable_height: i32, tallest_entry: i32) -> i32 {
    (capturable_height - tallest_entry - 4).max(1)
}

/// Widgets whose entry is visible in the slice captured at `scroll`, given
/// `capturable_height` — how much of the surface a screencopy of this output
/// can actually see.
///
/// Reconciliation: the task text hardcodes this at a `SURFACE_HEIGHT = 800`
/// constant ("the surface's documented default"), but a screencopy capture is
/// bounded by the *output's* resolution, not the layer surface's requested
/// size — a `zwlr_layer_surface_v1` anchored only top-left keeps the size it
/// asked for even when that exceeds the output, and the excess is simply
/// never composited. This harness's headless backend has no public knob for
/// output geometry (`WLR_HEADLESS_OUTPUTS` is a *count*; there is no width or
/// height env var, confirmed against the vendored `wlr` 0.20.28 source) and
/// its one default output is 1280x720, 80px short of the gallery's own
/// 1280x800 default surface. Using the compositor's own `output_size()` here
/// instead of the literal keeps the slice math honest about what a capture
/// can actually contain, in this environment or a taller one.
fn visible_in_slice(
    allocations: &[EntryAllocation],
    scroll: i32,
    capturable_height: i32,
) -> Vec<(String, (i32, i32, i32, i32))> {
    allocations
        .iter()
        .filter_map(|entry| {
            let top = entry.y as i32 - scroll;
            let bottom = top + entry.height as i32;
            (top >= 2 && bottom <= capturable_height - 2).then(|| {
                (
                    entry.widget.clone(),
                    (entry.x as i32, top, entry.width as i32, entry.height as i32),
                )
            })
        })
        .collect()
}

/// Widgets whose entry is entirely the page background at rest, tracked as
/// Part 8 deviation #12
/// (`docs/superpowers/plans/2026-08-27-m3-part8-gallery-gate-docs.md`) pending
/// a scheduled fix in each widget's own controller.
///
/// Re-derived in the M3 close-out's first fix wave, against
/// `gallery --print-allocation` and a full light-theme pass of this gate. The
/// list it replaces was written when thirteen entries measured to nothing;
/// six of those now have a real allocation and paint, and the three fixes
/// that got them there are:
///
/// * `widgets::apply_universal` — `width-request`/`height-request` (and
///   `Classes`/`Id`/`Sensitive`/`Focusable`) now reach every kind, not only
///   the controllers that happened to own a `Universal` — thirty widget
///   modules mentioned it nowhere. That is what
///   `progress_bar` (was `0x19`, now `160x19`) was missing.
/// * `widgets::measure_row`/`paint_row` — a recycling view's pooled rows are
///   bare `Node`s with no instance, so nothing measured or painted them;
///   `list_view` and `grid_view` were 4px-tall rows of empty padding.
/// * `00ddd7f`'s `StackSwitcherC` child-slot fix, which this list was never
///   updated for.
///
/// **M6-0 cleared four of the six and fixed the fifth's defect.** These four
/// now paint at rest and are held to the assertion:
///
/// * `link_button` had a real allocation (36x34) but a `label` subnode with
///   no text; `LinkButtonC` now `set_text`s that label the same way every
///   pooled row is drawn, and a `reserved_total` override keeps it past the
///   reconcile trim that used to detach it.
/// * `window_controls` drew nothing because its min/max/close buttons were
///   empty boxes; each now has an `image` child carrying a symbolic icon bound
///   with `widgets::set_icon` and drawn by `paint_row` — GTK sets these
///   programmatically, so the vendored Adwaita sheet has no rule for them — and
///   a `reserved_total` override keeps the buttons past reconcile.
/// * `alert_dialog` collapsed because its gallery sample gave a width but no
///   height; the sample now sets one, so the `window.dialog.message` has a
///   real box and paints its own Adwaita background and button row.
/// * `font_dialog`'s `fontchooser` was an empty placeholder; it now lists the
///   families the `FontDatabase` can match as real rows with intrinsic
///   height.
///
/// `popover_menu_bar`'s own P8-D72 defect is fixed too (a `reserved_total`
/// override keeps its per-menu `item`s, each now carrying a `label` with the
/// menu name), and it renders that title in every offscreen form; it stays on
/// this list only because this compositor-driven gate cannot locate its small
/// top-anchored title inside the short reported box — see the const's own
/// note. `popover_menu` is the honest permanent exemption: a popup, blank at
/// rest by design. `scrollbar` (`ScrollbarC::paint`) and `check_button`
/// (`CheckButtonC::paint`'s empty-box branch) were cleared earlier by M5-D8.
///
/// **`stack_sidebar` is no longer exempt either.** It has a real allocation
/// (121x80) and paints; what it still gets wrong is which pages it shows
/// (`StackSidebarC`'s eviction defect, contract §10 P8-D72), and that is a
/// content bug this gate does not and should not test for.
///
/// This gate exists precisely to catch "does not paint at rest" — excluding
/// these widgets from the paint assertion does not un-report the defect, it
/// only keeps the gate from blocking on widgets a later, dedicated task must
/// fix. `every_widget_renders_at_rest` still requires each of these to appear
/// whole in some slice (the `missing` check below is not exempted), so a
/// widget that regresses to never being laid out at all still fails the gate.
///
/// Neither `separator` nor `calendar` is here: `separator` looks zero-ish
/// (1x1) but paints, and `calendar` sizes itself for real now
/// (`CalendarC::measure`, 168 wide) and paints its own background; now that
/// [`paints_something`] scans the whole border box instead of an inset 5x5
/// grid, the gate can see both.
///
/// M5-D8 removed `color_dialog`, `check_button` and `scrollbar`: all three now
/// paint at rest (`ColorDialogC::paint`, `CheckButtonC::paint`'s empty-box
/// branch, `ScrollbarC::paint`). `color_dialog_button` was never on the list.
/// M6-0 cleared four widget defects (above) and fixed `popover_menu_bar`'s,
/// leaving `popover_menu` (a popup, blank by design) and `popover_menu_bar`
/// (fixed but not locatable by this compositor-driven gate) on the list.
///
/// Mutation check: re-add `"scrollbar"`; nothing fails, which shows the entry
/// would now be hiding a widget that paints — that is why it is gone. The
/// opposite check is the real one: delete `ScrollbarC::paint` and the
/// light-theme test fails with "scrollbar painted nothing in the light theme".
const KNOWN_BLANK_AT_REST: &[&str] = &[
    // A popup: correctly blank until opened. `popover_menu`'s entry is the
    // closed menu, which draws nothing at rest the same way a real
    // `GtkPopoverMenu` is unmapped until its button is clicked — no rest-state
    // paint to assert. Four of the five widgets that used to live here were
    // cleared in M6-0 and are now held to the assertion: `link_button` (writes
    // its label text and keeps it past reconcile via `reserved_total`),
    // `window_controls` (min/max/close paint symbolic icons bound with
    // `widgets::set_icon` and drawn by `paint_row`), `alert_dialog` (its
    // gallery sample now has a height) and `font_dialog` (its chooser lists
    // the families the database can match).
    //
    // `popover_menu_bar` is NOT here: it paints its titles at rest and is held
    // to that by `the_popover_menu_bar_paints_its_titles_at_rest` below, which
    // asserts in single-widget mode. It cannot be checked by the full-page walk
    // above for an infrastructure reason, not a paint defect: `App::probe`'s
    // cumulative vertical layout runs ~1.3% taller than `App::run` renders, so
    // under the live compositor every widget paints slightly above its reported
    // allocation, the drift growing with page depth (~0 at the top, ~44px at the
    // bottom). Every other bottom-of-page widget is tall enough that its ink
    // still overlaps its drifted box; this uniquely short (27px) bar with thin,
    // centred title ink is the only one a ~44px shift clears entirely. The
    // probe-vs-live divergence itself is a tracked follow-up, M6-FUP1 (it moves
    // every gate's page coordinates, so fixing it is a dedicated layout task;
    // recorded in ui/README.md's known-blank note).
    "popover_menu",
];

/// Paints at rest, but skipped by the full-page walk and asserted instead by
/// `the_popover_menu_bar_paints_its_titles_at_rest_*` in single-widget mode.
///
/// This is NOT "blank at rest" (that is `KNOWN_BLANK_AT_REST`): `popover_menu_bar`
/// renders its titles. The full-page walk simply cannot locate them — `App::probe`
/// lays the page out ~1.3% taller than `App::run` renders it, so a widget's painted
/// ink drifts above its reported box as page depth grows (~44px at the bottom),
/// clearing this uniquely short (27px) bar's whole box while leaving the taller
/// widgets around it overlapping their own drifted ink. Fixing that probe-vs-live
/// divergence moves every gate's page coordinates and is a dedicated layout
/// follow-up (M6-FUP1, recorded in ui/README.md); until then this widget is held
/// to the rest-paint bar out of band.
const PAINTS_BUT_UNLOCATABLE_IN_FULL_WALK: &[&str] = &["popover_menu_bar"];

/// Every own-kind entry paints something, in `theme`.
///
/// "Paints something" is the honest assertion: a widget whose entry rectangle
/// is entirely the window background has not rendered, whatever its node tree
/// says. Colours are not pinned here — that is `themed_button_offscreen.rs`'s
/// job, and pinning 64 of them would be a fixture, not a gate.
fn every_widget_renders_at_rest(theme: &str) {
    let compositor = Compositor::spawn();
    let socket = compositor
        .socket_path()
        .file_name()
        .expect("socket name")
        .to_string_lossy()
        .to_string();
    let (out_w, out_h) = compositor.output_size();
    assert!(
        out_w >= 1280 && out_h >= 480,
        "the gallery gate needs at least a 1280x480 output, got {out_w}x{out_h}"
    );
    let mut screencopy = ScreencopyClient::spawn(&socket);
    let empty = screencopy.capture();
    let background_probe = (out_w as u32 - 3, out_h as u32 / 2);
    let wallpaper = pixel_at(&empty, background_probe.0, background_probe.1)
        .expect("the background probe is inside the frame");

    let allocations = entry_allocations(theme);
    assert!(
        !allocations.is_empty(),
        "the gallery printed no allocations"
    );
    let page_height = allocations
        .iter()
        .map(|a| (a.y + a.height) as i32)
        .max()
        .expect("a non-empty page");
    let tallest = allocations
        .iter()
        .map(|a| a.height as i32)
        .max()
        .expect("a non-empty page");
    let step = slice_step(out_h, tallest);
    let points = probe_points(theme, None);

    let mut seen: Vec<String> = Vec::new();
    let mut diagnostics: Vec<String> = Vec::new();
    let mut scroll = 0;
    while scroll < page_height {
        let gallery = spawn_gallery(&socket, theme, scroll);
        let frame = wait_for_gallery(&mut screencopy, background_probe, wallpaper);
        let page_background =
            pixel_at(&frame, background_probe.0, background_probe.1).expect("inside the frame");
        for (widget, rect) in visible_in_slice(&allocations, scroll, out_h) {
            if seen.contains(&widget) {
                continue;
            }
            if KNOWN_BLANK_AT_REST.contains(&widget.as_str())
                || PAINTS_BUT_UNLOCATABLE_IN_FULL_WALK.contains(&widget.as_str())
            {
                seen.push(widget);
                continue;
            }
            if !paints_something(&frame, rect, page_background) {
                diagnostics.push(format!(
                    "{widget} painted nothing in the {theme} theme; its entry is {rect:?}"
                ));
            }
            for point in points.iter().filter(|p| p.widget == widget) {
                let y = point.y - scroll;
                if pixel_at(&frame, point.x as u32, y as u32).is_none() {
                    diagnostics.push(format!(
                        "{widget}/{} is outside the captured frame at ({}, {y})",
                        point.label, point.x
                    ));
                }
            }
            seen.push(widget);
        }
        drop(gallery);
        scroll += step;
    }
    // Reconciliation: the task text asserts inline, inside the loop. Collecting
    // instead costs nothing and makes a red run name every widget that failed
    // in one ~9-minute pass rather than the first one, which matters when a
    // pass is that expensive.
    assert!(
        diagnostics.is_empty(),
        "{} widget(s) failed to render:\n{}",
        diagnostics.len(),
        diagnostics.join("\n")
    );

    let missing: Vec<&str> = allocations
        .iter()
        .map(|a| a.widget.as_str())
        .filter(|w| !seen.iter().any(|s| s == w))
        .collect();
    assert!(
        missing.is_empty(),
        "no slice ever showed these widgets whole: {missing:?}"
    );
}

/// Mutation check: make `gallery::page` skip `Kind::Switch`'s frame; this test
/// fails with "no slice ever showed these widgets whole: [\"switch\"]".
/// Restore.
#[test]
fn every_widget_renders_at_rest_in_the_light_theme() {
    every_widget_renders_at_rest("light");
}

#[test]
fn every_widget_renders_at_rest_in_the_dark_theme() {
    every_widget_renders_at_rest("dark");
}

#[test]
fn every_widget_renders_at_rest_in_the_high_contrast_theme() {
    every_widget_renders_at_rest("hc");
}

/// `popover_menu_bar` paints its menu titles at rest, in `theme`.
///
/// Held to the same "paints something at rest" bar as every other kind, but
/// verified in single-widget mode (one framed entry at the page origin) rather
/// than by `every_widget_renders_at_rest`'s full-page walk. The walk cannot see
/// this widget's ink: `App::probe`'s cumulative vertical layout runs ~1.3%
/// taller than `App::run` renders, so under the live compositor every widget
/// paints a little above its reported allocation, the drift growing with page
/// depth to ~44px at the bottom -- more than this uniquely short (27px) bar's
/// whole box, though harmless to the taller widgets around it (see the note on
/// `KNOWN_BLANK_AT_REST`). Single-widget mode has no long page, so the drift is
/// ~0 and the reported box coincides with the painted ink.
///
/// Mutation check: revert `PopoverMenuBarC`'s per-menu `label`/`reserved_total`
/// (the M6-0 fix) so the bar trims back to 0x0 -- this fails with
/// "popover_menu_bar painted nothing ...".
fn popover_menu_bar_paints_at_rest(theme: &str) {
    let compositor = Compositor::spawn();
    let socket = compositor
        .socket_path()
        .file_name()
        .expect("socket name")
        .to_string_lossy()
        .to_string();
    let (ow, oh) = compositor.output_size();
    let output = (ow as u32, oh as u32);
    let mut screencopy = ScreencopyClient::spawn(&socket);
    let empty = screencopy.capture();
    let background_probe = (output.0 - 3, output.1 / 2);
    let wallpaper = pixel_at(&empty, background_probe.0, background_probe.1)
        .expect("the background probe is inside the frame");

    let gallery = spawn_gallery_widget_sized(&socket, theme, "popover_menu_bar", output);
    let frame = wait_for_gallery(&mut screencopy, background_probe, wallpaper);
    let page_background =
        pixel_at(&frame, background_probe.0, background_probe.1).expect("inside the frame");

    let alloc = allocation_sized(theme, "popover_menu_bar", output);
    let rect = (
        alloc.x as i32,
        alloc.y as i32,
        alloc.width as i32,
        alloc.height as i32,
    );
    assert!(
        paints_something(&frame, rect, page_background),
        "popover_menu_bar painted nothing at rest in the {theme} theme \
         (single-widget mode); its box is {rect:?}"
    );
    drop(gallery);
}

#[test]
fn the_popover_menu_bar_paints_its_titles_at_rest_in_the_light_theme() {
    popover_menu_bar_paints_at_rest("light");
}

#[test]
fn the_popover_menu_bar_paints_its_titles_at_rest_in_the_dark_theme() {
    popover_menu_bar_paints_at_rest("dark");
}

#[test]
fn the_popover_menu_bar_paints_its_titles_at_rest_in_the_high_contrast_theme() {
    popover_menu_bar_paints_at_rest("hc");
}

/// Capture every slice of `theme` and index the probe pixels by
/// `(widget, label)`.
///
/// Reconciliation: the task text walks slices with a literal `SLICE_STEP` and
/// bounds each point's visibility against a literal `SURFACE_HEIGHT`. Neither
/// constant exists in this file — Task 6 reconciled the same walk to
/// `slice_step(capturable_height, tallest_entry)` against the compositor's own
/// `output_size()`, for the reasons documented on `visible_in_slice` above.
/// This function reuses that same derivation rather than reintroducing the
/// literals it replaced.
fn probe_pixels(theme: &str) -> std::collections::BTreeMap<(String, String), (u8, u8, u8)> {
    let compositor = Compositor::spawn();
    let socket = compositor
        .socket_path()
        .file_name()
        .expect("socket name")
        .to_string_lossy()
        .to_string();
    let (out_w, out_h) = compositor.output_size();
    let mut screencopy = ScreencopyClient::spawn(&socket);
    let empty = screencopy.capture();
    let background_probe = (out_w as u32 - 3, out_h as u32 / 2);
    let wallpaper = pixel_at(&empty, background_probe.0, background_probe.1).expect("inside");

    let allocations = entry_allocations(theme);
    let page_height = allocations
        .iter()
        .map(|a| (a.y + a.height) as i32)
        .max()
        .expect("a non-empty page");
    let tallest = allocations
        .iter()
        .map(|a| a.height as i32)
        .max()
        .expect("a non-empty page");
    let step = slice_step(out_h, tallest);
    let points = probe_points(theme, None);

    let mut sampled = std::collections::BTreeMap::new();
    let mut scroll = 0;
    while scroll < page_height {
        let gallery = spawn_gallery(&socket, theme, scroll);
        let frame = wait_for_gallery(&mut screencopy, background_probe, wallpaper);
        for point in &points {
            let y = point.y - scroll;
            if y < 2 || y >= out_h - 2 {
                continue;
            }
            let key = (point.widget.clone(), point.label.clone());
            if let std::collections::btree_map::Entry::Vacant(slot) = sampled.entry(key)
                && let Some(px) = pixel_at(&frame, point.x as u32, y as u32)
            {
                slot.insert(px);
            }
        }
        drop(gallery);
        scroll += step;
    }
    sampled
}

/// Widgets whose gallery sample is theme-blind by construction, not by
/// defect.
///
/// Found by running this test with per-slice diagnostics: both full walks
/// (light then dark, one compositor each) complete and paint every slice,
/// and these four are the *only* entries with zero differing probe points.
/// Root-caused against `ui/src/gallery.rs`'s own `sample` or the widget's
/// own paint, not guessed:
///
/// - `Kind::Picture` draws a fixed PNG loaded from `sample_png_path()`. A
///   photo does not repaint for a light/dark switch in real Adwaita GTK
///   either.
/// - `Kind::DrawingArea`'s callback fills a hardcoded `#33D17A` rectangle
///   with no reference to the active sheet — the same is true of a real
///   `GtkDrawingArea`'s cairo callback, which is entirely the caller's paint
///   code.
/// - `Kind::Image` samples `IconRef::Theme { name: "folder" }` — a
///   non-symbolic icon. Only a `-symbolic` name would be recoloured to
///   `currentColor` (and even then would hit the M2-inherited
///   background-image-layer `currentColor` gap this part's controller notes
///   name as a deferred minor), so a full-colour `folder` bitmap is
///   correctly identical in both sheets, exactly as it is in a real desktop.
/// - `Kind::ColorDialog` (`ColorDialogC::paint`, `ui/src/widgets/color_dialog.rs`)
///   fills its box with `self.grid(content)` — a palette of fixed `Rgba`
///   swatch values from `default_palette()`. A colour picker's swatches are
///   the colours themselves, not chrome; red is red in both sheets, exactly
///   like a real `GtkColorChooserWidget`'s palette.
///
/// `color_dialog`'s exemption is recorded as **P0-D9** in the M5 contract §6:
/// every probe point this gate samples for a widget is a laid-out node's
/// *centre* (`window::probe_points_of`), and for this widget every one of them
/// lands on palette fill — the node's own centre inside the middle cell, the
/// zero-sized `colorchooser`/`colorswatch` subnodes on the first. Painting the
/// chooser's background or a swatch border from `style` first changes no
/// sampled pixel, so the exemption is the honest record rather than a
/// workaround for a missing paint.
///
/// None of the four reads `Theme` at all, so failing them here would not be
/// deviation 6's "the theme never reached them" (a controller wiring gap) —
/// it would be asserting that a photo, a caller's own drawing, a full-colour
/// icon and a colour swatch grid must repaint for a stylesheet that was
/// never supposed to touch them.
///
/// Mutation check: remove `"picture"` from this list; the test fails with
/// `picture` back in the `unchanged` list (it never differs). Restore.
const THEME_BLIND_BY_DESIGN: &[&str] = &["drawing_area", "image", "picture", "color_dialog"];

/// Per widget, at least one probe point must look different in dark Adwaita.
///
/// Per widget, not per point: a transparent subnode, or one Adwaita styles
/// identically in both sheets, legitimately matches across themes — the
/// contract's test name is kept, its assertion is the honest one (deviation
/// 6). A widget where *nothing* changes is a widget the theme never reached,
/// with [`THEME_BLIND_BY_DESIGN`] carved out for the four that are exempt
/// from that claim on purpose (M5 contract §6 P0-D9). The list is closed at
/// four; a fifth needs its own §6 amendment.
///
/// Mutation check: make `Theme::sheet` return the light sheet for
/// `Theme::Dark`; every widget then matches and this test fails on the first
/// one not in [`THEME_BLIND_BY_DESIGN`]. Restore.
#[test]
fn every_probe_point_differs_between_light_and_dark() {
    let light = probe_pixels("light");
    let dark = probe_pixels("dark");
    let widgets: std::collections::BTreeSet<&String> = light
        .keys()
        .map(|(widget, _)| widget)
        .filter(|w| !THEME_BLIND_BY_DESIGN.contains(&w.as_str()))
        .collect();
    assert!(!widgets.is_empty(), "no probe pixels were sampled at all");
    let mut unchanged = Vec::new();
    for widget in widgets {
        let differs =
            light
                .iter()
                .filter(|((w, _), _)| w == widget)
                .any(|((w, label), light_px)| {
                    dark.get(&(w.clone(), label.clone()))
                        .is_some_and(|dark_px| !support::matches(*light_px, *dark_px))
                });
        if !differs {
            unchanged.push(widget.clone());
        }
    }
    assert!(
        unchanged.is_empty(),
        "these widgets look identical in light and dark Adwaita, so the theme \
         never reached them: {unchanged:?}"
    );
}

/// Never-panic gate for the two stdout parsers: the gate reads a child
/// process's output, and a crashed or half-written child must fail the
/// assertion, not the harness.
///
/// Mutation check: parse a field with `unwrap()` instead of `ok()?`; this test
/// panics instead of passing. Restore.
#[test]
fn hostile_child_output_is_parsed_without_panicking() {
    let hostile = [
        "",
        " ",
        "\u{0}",
        "label",
        "label root",
        "label root 1",
        "label root x y",
        "label root -1 -1",
        "label root 99999999999999999999 0",
        "label root 1 2 3 4 5",
        "лейбл корень 1 2",
        "label root 1.5 2.5",
    ];
    for line in hostile {
        let _ = parse_probe_line(line);
        let _ = parse_allocation_line(line);
    }
    assert_eq!(
        parse_probe_line("check_button check 40 21"),
        Some(ProbePoint {
            widget: "check_button".to_string(),
            label: "check".to_string(),
            x: 40,
            y: 21,
        })
    );
    let alloc = parse_allocation_line("button 12 340 78 34").expect("a well-formed line");
    assert_eq!(alloc.widget, "button");
    assert!((alloc.height - 34.0).abs() < f32::EPSILON);
}

/// `--list` and the page are the same set, and every sub-kind really appears
/// inside its parent's rendered tree.
///
/// This is the test the whole gallery exists for: a `Kind` that reaches no
/// pixel is a `Kind` no other test in this file could ever have covered.
///
/// Mutation check: give `sample_shape` an extra
/// `Kind::Switch => SampleShape::Within(Kind::Box)` arm; this test fails on
/// the `switch` sub-kind assertion (its node name is absent from `box`'s
/// tree). Restore.
#[test]
fn every_kind_appears_in_the_gallery() {
    let listed: Vec<String> = String::from_utf8(
        std::process::Command::new(env!("CARGO_BIN_EXE_gallery"))
            .arg("--list")
            .output()
            .expect("gallery --list runs")
            .stdout,
    )
    .expect("--list is UTF-8")
    .lines()
    .map(str::to_string)
    .collect();
    assert_eq!(
        listed.len(),
        Kind::all().len(),
        "--list must print every Kind"
    );
    for &kind in Kind::all() {
        assert!(
            listed.iter().any(|name| name == kind_name(kind)),
            "{} is missing from --list",
            kind_name(kind)
        );
    }

    let entries: Vec<String> = entry_allocations("light")
        .into_iter()
        .map(|a| a.widget)
        .collect();
    for kind in own_kinds() {
        assert!(
            entries.iter().any(|w| w == kind_name(kind)),
            "{} has no entry on the page",
            kind_name(kind)
        );
    }
    assert_eq!(
        entries.len(),
        own_kinds().len(),
        "the page has extra entries"
    );

    let model = GalleryModel::new(icedtea_ui::gallery::Theme::Light, None);
    for &kind in Kind::all() {
        let SampleShape::Within(parent) = sample_shape(kind) else {
            continue;
        };
        // Reconciliation (contract D15): `StackPage` and `ColumnViewColumn`
        // are documented exceptions where real GTK renders no node of its
        // own at all (`Kind::css_name`'s doc comment cites D15 by name), so
        // asserting their `css_name()` appears in the parent's tree would be
        // asserting a node exists that GTK itself never creates. Every other
        // `SampleShape::Within` kind is still held to the letter of the
        // task's assertion.
        if matches!(kind, Kind::StackPage | Kind::ColumnViewColumn) {
            continue;
        }
        let Sample::Own(view) = sample(parent, &model) else {
            panic!("{}'s parent must be its own entry", kind_name(kind));
        };
        let tree = node_tree_of(parent, &view.props);
        assert!(
            tree.contains(kind.css_name()),
            "{} claims to live inside {}, whose node tree has no {:?} node:\n{tree}",
            kind_name(kind),
            kind_name(parent),
            kind.css_name()
        );
    }
}

/// Own kinds whose vendored fixture cannot be reached through the gallery's
/// own `sample()` props, for reasons that are architectural rather than a
/// widget defect — each is still fully conformance-tested elsewhere, just
/// not through this gate's particular path (`sample()`'s props, alone,
/// through `node_tree_of`).
///
/// - `notebook`: `node_tree_of` builds through
///   [`build_widget`](icedtea_ui::widgets::build_widget), which is
///   `&Props`-only and never sees a view's children. The gallery's real
///   `Notebook` entry gets its two tabs the *production* way — real
///   `Kind::NotebookTab` children, placed by `NotebookC::place` during a full
///   reconcile (see `notebook.rs`'s module doc) — which a bare
///   `build_widget` call can never run, so its tree always renders with zero
///   tabs. The controller also accepts a synthetic tab count through
///   `PropName::Pages`, but that path and the real-children path are
///   independently additive (`notebook.rs`: "the two mechanisms simply
///   append to the same `tabs`/`stack` lists and never both fire for one
///   real widget") — setting `Pages` on the live gallery model to satisfy
///   this gate would render *four* tabs on screen, corrupting the very
///   widget this gate exists to prove renders correctly. `notebook.txt` is
///   already conformance-tested against a synthetic `Pages` count by
///   `ui/tests/node_trees.rs::notebook_matches_its_gtk_fixture`.
/// - `popover_menu`: `popover_menu.txt` is GTK's own doc example for the
///   `.inline-buttons` style specifically (`box.horizontal.inline-buttons`,
///   required, not `[.optional]`) — confirmed by `popover_menu.rs`'s own
///   `matches_fixture` unit tests, which all build `DisplayHint::InlineButtons`
///   to reach it. The gallery's entry deliberately demos the *plain* style
///   (`DisplayHint::Normal`, GTK's actual default), which is a different,
///   equally valid retained tree — `box.vertical`, no vendored fixture of its
///   own — not a narrower instance of the vendored one. `popover_menu.txt` is
///   already conformance-tested against the `InlineButtons` config by
///   `ui/tests/node_trees.rs::popover_menu_matches_its_gtk_fixture`.
///
/// Picking a gallery config purely to satisfy this gate, for either, would
/// either corrupt the live widget (`notebook`) or silently retarget what the
/// gallery demos away from its own default (`popover_menu`); recording the
/// exemption is the honest alternative to hacking the gate or the sample
/// table. See the Task 8 report for the full reconciliation.
const NODE_TREE_FIXTURE_EXEMPT: &[&str] = &["notebook", "popover_menu"];

/// Every widget's retained tree matches the GTK 4.22 "CSS nodes" block
/// vendored for it.
///
/// P5/P6 own both `node_tree_of` and the fixtures; P8 only wires them into the
/// gate, so a widget added without a fixture fails here rather than shipping
/// unmeasured. [`NODE_TREE_FIXTURE_EXEMPT`] carves out the two widgets whose
/// gallery sample cannot reach its own fixture through `sample()`'s props
/// alone for reasons that are architectural, not a conformance gap — both are
/// still matched against that exact fixture in `ui/tests/node_trees.rs`.
///
/// Mutation check: delete one line from any non-exempt fixture; this test
/// fails naming that widget with the matcher's own reason. Restore.
#[test]
fn the_node_tree_of_every_widget_matches_its_gtk_fixture() {
    let dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/gtk4.22-node-trees");
    let model = GalleryModel::new(icedtea_ui::gallery::Theme::Light, None);
    let mut missing = Vec::new();
    // Reconciliation: fixtures are vendored one per *own* entry (58 files,
    // matching `own_kinds().len()`), not one per `Kind::all()` (64). The six
    // sub-kinds (`NotebookTab`, `StackPage`, `ListBoxRow`, `FlowBoxChild`,
    // `ColumnViewColumn`, `PopoverMenuItem`) never render as the root of
    // their own tree — `node_tree_of` builds a tree rooted at `kind`, and a
    // sub-kind's node only ever appears nested inside its parent's tree,
    // which `every_kind_appears_in_the_gallery` already checks. A top-level
    // fixture keyed on a sub-kind's own name would describe a tree that kind
    // never produces standalone.
    for kind in own_kinds() {
        let name = kind_name(kind);
        if NODE_TREE_FIXTURE_EXEMPT.contains(&name) {
            continue;
        }
        let path = dir.join(format!("{name}.txt"));
        let Ok(expected) = std::fs::read_to_string(&path) else {
            missing.push(name);
            continue;
        };
        let Sample::Own(view) = sample(kind, &model) else {
            panic!("{name} is in own_kinds() but sample() didn't return Own");
        };
        let rendered = node_tree_of(kind, &view.props);
        // Reconciliation (P5-D30): fixture blocks carry `[optional]` nodes,
        // `[.optional-class]` markers, `┊`/`⋮` repetition and `<child>`
        // wildcards a literal string diff can't interpret ("matched
        // structurally, not string-diffed"), so this compares through the
        // library's own structural matcher rather than `assert_eq!` on the
        // raw text.
        if let Err(why) = fixture_matches(&expected, &rendered) {
            panic!(
                "{name}'s node tree does not match {}: {why}",
                path.display()
            );
        }
    }
    assert!(
        missing.is_empty(),
        "no vendored GTK node-tree fixture for: {missing:?} (expected \
         ui/tests/fixtures/gtk4.22-node-trees/<name>.txt, one per Kind)"
    );
}

/// The README's widget table is generated from `Kind::all()` and must stay
/// that way: a widget added to the toolkit and not to the table is a widget
/// the next reader will not know exists.
///
/// Mutation check: delete one row from the table between the markers; this
/// test fails naming that widget. Restore.
#[test]
fn the_readme_widget_table_lists_every_kind() {
    let readme =
        std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md"))
            .expect("ui/README.md is readable");
    let table = readme
        .split_once("<!-- widgets:begin -->")
        .expect("the README has a `<!-- widgets:begin -->` marker")
        .1
        .split_once("<!-- widgets:end -->")
        .expect("the README has a `<!-- widgets:end -->` marker")
        .0;
    let listed: Vec<&str> = table
        .lines()
        .filter_map(|line| {
            let cell = line.strip_prefix("| `")?;
            cell.split_once('`')
        })
        .map(|(name, _)| name)
        .collect();
    for &kind in Kind::all() {
        assert!(
            listed.contains(&kind_name(kind)),
            "`{}` is missing from the README's widget table",
            kind_name(kind)
        );
    }
    assert_eq!(
        listed.len(),
        Kind::all().len(),
        "the README table has rows for widgets that do not exist: {listed:?}"
    );
}

/// The README's blank-widget prose and the const cannot drift.
///
/// mutation: drop one name from the README paragraph; this fails and names it.
#[test]
fn the_readme_names_every_known_blank_widget() {
    let readme =
        std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md"))
            .expect("ui/README.md is readable");
    let section = readme
        .split_once("<!-- known-blank:begin -->")
        .expect("the README has a `<!-- known-blank:begin -->` marker")
        .1
        .split_once("<!-- known-blank:end -->")
        .expect("the README has a `<!-- known-blank:end -->` marker")
        .0;
    for widget in KNOWN_BLANK_AT_REST {
        assert!(
            section.contains(&format!("`{widget}`")),
            "the README's blank-widget list does not name `{widget}`"
        );
    }
    let named = section.matches('`').count() / 2;
    assert_eq!(
        named,
        KNOWN_BLANK_AT_REST.len(),
        "the README names {named} blank widgets, the const has {}",
        KNOWN_BLANK_AT_REST.len()
    );
}

/// M5-D11: the README documents every M5 P0 addition. A cheap, exact check —
/// the names are the API, so a rename that skips the docs fails here.
///
/// mutation: delete the "External events" heading from the README; this fails
/// and names it.
#[test]
fn the_readme_documents_the_m5_toolkit_additions() {
    let readme =
        std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md"))
            .expect("ui/README.md is readable");
    for needle in [
        "### External events",
        "### Pointer events at the view layer",
        "### Base-level keysyms",
        "### Probing a live window",
        "### Watching a foreign fd",
        "Inbox",
        "InboxSender",
        "App::with_inbox",
        "App::on_fd",
        "Window::watch_fd",
        "InputEvent::FdReady",
        "Cmd::Task",
        "impl FnMut",
        "Handler::PairButton",
        "on_pointer_up_with_button",
        "BTN_MIDDLE",
        "Keymap::base_keysym",
        "Window::probe_points",
        "$ICEDTEA_PROBE_REPORT",
    ] {
        assert!(
            readme.contains(needle),
            "ui/README.md does not document `{needle}`"
        );
    }
    assert!(
        !readme.contains("seven collapse to a zero-area allocation"),
        "the stale blank-widget sentence is still in the README"
    );
}
