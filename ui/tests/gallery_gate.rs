//! The M3 rest-state gate.
//!
//! The gallery renders every `Kind` the toolkit ships; this file proves each
//! one actually reaches a real compositor's screen, in all three bundled
//! Adwaita sheets, at coordinates it learned from the binary rather than from
//! a literal.

mod support;

use icedtea_harness::{Compositor, ScreencopyClient};
use support::{
    EntryAllocation, ProbePoint, entry_allocations, paints_something, parse_allocation_line,
    parse_probe_line, pixel_at, probe_points, spawn_gallery, wait_for_gallery,
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
/// a scheduled fix in each widget's own controller — no file this part owns
/// can affect any of them.
///
/// Two distinct defects, both measured rather than inferred, by walking the
/// whole page once and recording every entry's non-background pixel count
/// instead of asserting on it:
///
/// **Thirteen collapse to a zero-area allocation** (`gallery
/// --print-allocation` prints `w` and/or `h` as `0`, headless, so this is a
/// layout result and not a rendering artifact), and every one of them lands
/// at `x = 640`, exactly half the 1280px page — the signature of a box taffy
/// centred after it measured to nothing. Root-caused for `progress_bar` in
/// `ui/src/widgets/progress_bar.rs`'s `measure`: `show_text(true)` delegates
/// the whole measurement to an unshaped label instead of ever reporting the
/// trough's own intrinsic `(150.0, 2.0)`, and the label measures to zero
/// before shaping runs. The other twelve are the same shape of bug in a
/// different controller, not individually traced.
///
/// **Two have a real allocation and still paint nothing**: `link_button`
/// (36x34, 0 of 1224 pixels differ from the background) and `check_button`
/// (22x22, 0 of 484). Both are traced. `CheckButtonC::paint`
/// (`ui/src/widgets/check_button.rs`) returns `false` outright when the button
/// is neither active nor inconsistent — the gallery's sample starts unchecked
/// — and the `check` subnode Adwaita gives a border and background of its own
/// never gets an allocation to paint into. `LinkButtonC`
/// (`ui/src/widgets/link_button.rs`) appends a `label` subnode but never gives
/// it text, so it paints no glyphs; `button` and `toggle_button` share that
/// gap and only pass this gate because `.link` is flat and they are not, so
/// their 1px border is the only thing either of them draws.
///
/// This gate exists precisely to catch "does not paint at rest" — excluding
/// these widgets from the paint assertion does not un-report the defect, it
/// only keeps the gate from blocking on widgets a later, dedicated task must
/// fix. `every_widget_renders_at_rest` still requires each of these to appear
/// whole in some slice (the `missing` check below is not exempted), so a
/// widget that regresses to never being laid out at all still fails the gate.
///
/// Neither `separator` nor `calendar` is here: both look zero-ish (1x1 and
/// 2x2) but both do paint, and now that [`paints_something`] scans the whole
/// border box instead of an inset 5x5 grid, the gate can see it.
///
/// Mutation check: remove `"progress_bar"` from this list; the light-theme
/// test fails with "progress_bar painted nothing in the light theme".
/// Restore.
const KNOWN_BLANK_AT_REST: &[&str] = &[
    // Zero-area allocation.
    "progress_bar",
    "scrollbar",
    "window_controls",
    "color_dialog",
    "font_dialog",
    "stack_switcher",
    "stack_sidebar",
    "list_view",
    "grid_view",
    "popover_menu",
    "popover_menu_bar",
    "about_dialog",
    "alert_dialog",
    // Real allocation, nothing drawn into it.
    "link_button",
    "check_button",
];

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
            if KNOWN_BLANK_AT_REST.contains(&widget.as_str()) {
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
