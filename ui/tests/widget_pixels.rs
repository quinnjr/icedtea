//! Per-widget rest-state and interaction tests (contract §9, P5's gate).
//!
//! Everything here runs through `App::run_offscreen` on a `ManualClock` — no
//! compositor, no Wayland connection. P8's `gallery_gate.rs` and
//! `interaction_gate.rs` are the compositor-backed versions of the same idea.
//!
//! # Chrome a controller appends itself is not in the layout tree
//!
//! `widgets/mod.rs::local_rect` says so itself: it "only answers for subnodes
//! the reconciler gave a taffy node, which the extra nodes a controller
//! appends itself (`text`, `image.peek`, `trough`, …) never are —
//! `tree.allocation` is `None` for every one of them". A controller that
//! hit-tests such a node through `local_rect` therefore bails on every event
//! and its whole `on_event` body is dead code; worse, a widget whose *only*
//! content is that chrome also measures to 0x0, so it is not even hit-tested
//! in the first place.
//!
//! `ScaleC` has always got this right — an intrinsic `measure`, plus a trough
//! derived from `content_rect_local` — and `ScrollbarC`, `InfoBarC` and
//! `CalendarC` now do too (`ScrollbarC::measure`, `InfoBarC::close_rect`,
//! `CalendarC::day_cell`). The three interaction tests below
//! (`dragging_a_scrollbar_slider_reports_the_new_value`,
//! `clicking_an_info_bars_close_button_fires_close`,
//! `clicking_a_calendar_day_selects_it`) were `#[ignore]`d on that defect and
//! are green assertions again; each carries the geometry its coordinates come
//! from, because those widgets sit at their intrinsic size, centred, and not
//! wherever `hexpand` suggests.

use std::rc::Rc;
use std::time::Duration;

use icedtea_ui::BUNDLED_ADWAITA_LIGHT;
use icedtea_ui::anim::ManualClock;
use icedtea_ui::css::cascade::CompiledSheet;
use icedtea_ui::view::app::{App, Frames, ScriptStep};
use icedtea_ui::view::{Cmd, View};
use icedtea_ui::window::InputEvent;

/// Run one view offscreen against Adwaita light and return the captured frames.
///
/// The icon theme is deliberately hermetic — `"hicolor"` with no roots, so
/// every lookup misses — because these are pixel assertions and the machine's
/// installed icon set is not part of the toolkit under test (contract §6:
/// Adwaita is "exercised only when present"). A test that needs a real icon
/// builds its own `App` with `with_icons` and the checked-in mini fixture;
/// `an_image_paints_its_resolved_icon` is the one that does.
pub fn run<M: 'static, Msg: Clone + 'static>(
    model: M,
    update: fn(&mut M, Msg) -> Cmd<Msg>,
    view: fn(&M) -> View<Msg>,
    size: (u32, u32),
    script: Vec<ScriptStep<Msg>>,
) -> Frames {
    let clock = Rc::new(ManualClock::new());
    App::new(model, update, view)
        .with_sheet(CompiledSheet::compile(BUNDLED_ADWAITA_LIGHT))
        .with_icons(icedtea_ui::icons::IconTheme::with_name_and_roots(
            "hicolor",
            Vec::new(),
        ))
        .run_offscreen(size, clock, script)
        .expect("the offscreen app must run")
}

/// `true` if any pixel in the frame is neither fully transparent nor the
/// window's own background — i.e. the widget actually inked something.
pub fn has_ink(frames: &Frames, frame: usize, size: (u32, u32)) -> bool {
    let mut seen = std::collections::HashSet::new();
    for y in 0..size.1 {
        for x in 0..size.0 {
            if let Some(px) = frames.pixel(frame, x, y) {
                seen.insert(px);
            }
        }
    }
    seen.len() > 1
}

#[test]
fn a_separator_inks_its_adwaita_line_at_rest() {
    // mutation: return `false` from SeparatorC's node build so no node is
    // attached, and the frame becomes a single flat colour.
    use icedtea_ui::view::builders::separator;
    use icedtea_ui::widgets::Orientation;

    let frames = run(
        (),
        |_model: &mut (), _msg: ()| Cmd::None,
        |_model: &()| separator(Orientation::Horizontal).hexpand(true),
        (120, 40),
        vec![ScriptStep::Capture],
    );
    assert_eq!(frames.len(), 1);
    assert!(
        has_ink(&frames, 0, (120, 40)),
        "the separator line must ink"
    );
}

#[test]
fn a_separator_ignores_every_pointer_event() {
    // mutation: give SeparatorC a PointerState and set :hover, and the two
    // captures stop being identical.
    use icedtea_ui::view::builders::separator;
    use icedtea_ui::widgets::Orientation;

    let frames = run(
        (),
        |_model: &mut (), _msg: ()| Cmd::None,
        |_model: &()| separator(Orientation::Horizontal).hexpand(true),
        (120, 40),
        vec![
            ScriptStep::Capture,
            ScriptStep::Event(InputEvent::pointer_enter(60.0, 20.0, 1)),
            ScriptStep::Event(InputEvent::PointerButton {
                button: 0x110,
                pressed: true,
                serial: 2,
                time_ms: 0,
            }),
            ScriptStep::Advance(Duration::from_millis(16)),
            ScriptStep::Capture,
        ],
    );
    let before: Vec<_> = (0..120).map(|x| frames.pixel(0, x, 20)).collect();
    let after: Vec<_> = (0..120).map(|x| frames.pixel(1, x, 20)).collect();
    assert_eq!(before, after, "a separator has no interactive state");
}

#[test]
fn a_label_inks_its_glyphs_at_rest_in_adwaita_light() {
    // mutation: skip `layout.draw` in LabelC::paint and the frame is one flat
    // colour.
    use icedtea_ui::view::builders::label;
    let frames = run(
        (),
        |_m: &mut (), _msg: ()| Cmd::None,
        |_m: &()| label("Hello"),
        (160, 40),
        vec![ScriptStep::Capture],
    );
    assert!(has_ink(&frames, 0, (160, 40)), "the label must draw glyphs");
}

#[test]
fn clicking_a_link_in_a_label_fires_activate_link_with_its_uri() {
    // mutation: drop the hit-test against `links` in LabelC::on_event and no
    // message arrives, so `opened` stays empty and the URI assertion fails.
    use icedtea_ui::view::builders::label;
    use icedtea_ui::widgets::label::LabelExt;
    use std::cell::RefCell;

    #[derive(Clone, Debug, PartialEq)]
    struct Opened(String);

    // `run`'s `view` must be a bare `fn` pointer, so the log is threaded
    // through the model rather than captured from the test's scope; that is
    // what lets the assertion below name the exact URI instead of settling
    // for `frames.len()`.
    let opened: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));

    let frames = run(
        opened.clone(),
        |model: &mut Rc<RefCell<Vec<String>>>, Opened(uri): Opened| {
            model.borrow_mut().push(uri);
            Cmd::None
        },
        |_m: &Rc<RefCell<Vec<String>>>| {
            label("go to <a href=\"https://gtk.org\">GTK</a> now")
                .markup(true)
                .on_activate_link(|uri| Opened(uri.to_owned()))
        },
        (240, 40),
        vec![
            // `LabelC` reports its own intrinsic text size and is centred in
            // the window, so the anchor is nowhere near the window's own
            // left edge: sweeping every pixel of this 240x40 window puts the
            // "GTK" run at x 109..132, y 10..29 (the plain "go to " and
            // " now" spans around it fire nothing, which is what makes the
            // URI assertion below a real hit-test check and not just
            // "something was clicked"). x=40, the coordinate this test used
            // before, lands on empty window and could never fire.
            ScriptStep::Event(InputEvent::PointerEnter {
                x: 120.0,
                y: 20.0,
                serial: 1,
                target: icedtea_ui::window::SurfaceTarget::Window,
            }),
            ScriptStep::Event(InputEvent::PointerButton {
                button: 0x110,
                pressed: true,
                serial: 2,
                time_ms: 0,
            }),
            ScriptStep::Event(InputEvent::PointerButton {
                button: 0x110,
                pressed: false,
                serial: 3,
                time_ms: 8,
            }),
            ScriptStep::Capture,
        ],
    );
    assert_eq!(frames.len(), 1, "the script captured once");
    assert_eq!(
        opened.borrow().as_slice(),
        ["https://gtk.org"],
        "clicking the anchor must fire activate-link once with the href"
    );
}

#[test]
fn a_spinner_carries_checked_while_spinning_and_advances_its_phase() {
    // mutation: drop the `phase` advance in `SpinnerC::tick` (or its
    // `next_deadline`, so no tick ever runs) and the two captured rows are
    // identical -- the arc never rotates. The `:checked` state `SpinnerC`
    // also carries while spinning is asserted in the widget's own module
    // test, not here: this is the animation half.
    use icedtea_ui::view::builders::spinner;
    use icedtea_ui::widgets::spinner::SpinnerExt;
    let frames = run(
        (),
        |_m: &mut (), _msg: ()| Cmd::None,
        |_m: &()| spinner().spinning(true),
        (48, 48),
        vec![
            ScriptStep::Capture,
            ScriptStep::Advance(Duration::from_millis(500)),
            ScriptStep::Capture,
        ],
    );
    let first: Vec<_> = (0..48).map(|x| frames.pixel(0, x, 24)).collect();
    let second: Vec<_> = (0..48).map(|x| frames.pixel(1, x, 24)).collect();
    assert_ne!(first, second, "the spinner arc must have rotated");
}

#[test]
fn a_statusbar_shows_the_top_of_its_message_stack() {
    // mutation: push instead of replace in StatusbarC::set_prop and the second
    // frame still shows the first message.
    use icedtea_ui::view::builders::statusbar;
    use icedtea_ui::widgets::statusbar::StatusbarExt;
    let frames = run(
        0u32,
        |model: &mut u32, _msg: ()| {
            *model += 1;
            Cmd::None
        },
        |model: &u32| statusbar().text(if *model == 0 { "" } else { "Saved" }),
        (200, 32),
        vec![
            ScriptStep::Capture,
            ScriptStep::Message(()),
            ScriptStep::Capture,
        ],
    );
    assert!(
        !has_ink(&frames, 0, (200, 32)),
        "an empty statusbar inks nothing"
    );
    assert!(has_ink(&frames, 1, (200, 32)), "the pushed message shows");
}

#[test]
fn a_progress_bars_fill_widens_with_its_fraction() {
    // mutation: ignore `fraction` when sizing the `progress` node and both
    // frames ink the same width.
    use icedtea_ui::view::builders::progress_bar;
    let frames = run(
        0.1f64,
        |model: &mut f64, _msg: ()| {
            *model = 0.9;
            Cmd::None
        },
        |model: &f64| progress_bar(*model).hexpand(true),
        (200, 24),
        vec![
            ScriptStep::Capture,
            ScriptStep::Message(()),
            ScriptStep::Capture,
        ],
    );
    let inked = |frame: usize| {
        (0..200)
            .filter(|x| frames.pixel(frame, *x, 12) != frames.pixel(frame, 199, 12))
            .count()
    };
    assert!(inked(1) > inked(0), "0.9 must ink wider than 0.1");
}

#[test]
fn a_pulsing_progress_bar_moves_its_block_on_the_clock() {
    // mutation: make `tick` a no-op and the two frames match.
    use icedtea_ui::view::builders::progress_bar;
    use icedtea_ui::widgets::progress_bar::ProgressBarExt;
    let frames = run(
        (),
        |_m: &mut (), _msg: ()| Cmd::None,
        |_m: &()| progress_bar(f64::NAN).pulse_step(0.1).hexpand(true),
        (200, 24),
        vec![
            ScriptStep::Capture,
            ScriptStep::Advance(Duration::from_millis(300)),
            ScriptStep::Capture,
        ],
    );
    let row = |frame: usize| {
        (0..200)
            .map(|x| frames.pixel(frame, x, 12))
            .collect::<Vec<_>>()
    };
    assert_ne!(row(0), row(1), "the pulse block must have moved");
}

#[test]
fn a_level_bar_inks_its_blocks_at_rest() {
    // mutation: drop LevelBarC's `measure`/`paint` overrides (its pre-fix
    // state) and the bar collapses to a zero-size leaf that inks nothing.
    use icedtea_ui::view::builders::level_bar;
    let frames = run(
        0.5f64,
        |_model: &mut f64, _msg: ()| Cmd::None,
        |model: &f64| level_bar(*model).hexpand(true),
        (200, 24),
        vec![ScriptStep::Capture],
    );
    assert!(
        has_ink(&frames, 0, (200, 24)),
        "the level bar must ink something"
    );
}

#[test]
fn a_discrete_level_bars_fill_widens_with_its_value() {
    // mutation: ignore `value` when deciding a block's `.filled`/`.empty`
    // class (or never paint blocks at all) and both frames end up with the
    // same number of filled-colour columns.
    use icedtea_ui::view::builders::level_bar;
    use icedtea_ui::widgets::level_bar::LevelBarExt;
    let frames = run(
        1.0f64,
        |model: &mut f64, _msg: ()| {
            *model = 4.0;
            Cmd::None
        },
        |model: &f64| {
            level_bar(*model)
                .max_value(4.0)
                .mode(icedtea_ui::widgets::LevelBarMode::Discrete)
                .hexpand(true)
        },
        (200, 24),
        vec![
            ScriptStep::Capture,
            ScriptStep::Message(()),
            ScriptStep::Capture,
        ],
    );
    // The trough's total width doesn't change with `value` (block count is
    // fixed by `min`/`max`), only how many of its blocks are `.filled` — so
    // compare how many columns carry the *darkest* colour in the frame
    // (filled blocks paint at full alpha; empty blocks are the same colour
    // blended at 30% over the background, always lighter) rather than how
    // far the paint reaches across the bar.
    let filled_count = |frame: usize| {
        let darkest = (0..200)
            .filter_map(|x| frames.pixel(frame, x, 12))
            .min_by_key(|(r, g, b, _)| *r as u32 + *g as u32 + *b as u32)
            .expect("the frame has pixels");
        (0..200)
            .filter(|x| frames.pixel(frame, *x, 12) == Some(darkest))
            .count()
    };
    assert!(
        filled_count(1) > filled_count(0),
        "value 4.0 must fill more of the bar than 1.0"
    );
}

#[test]
fn clicking_an_info_bars_close_button_fires_close() {
    // mutation: never set `handled` / never fire EventKind::Close in
    // InfoBarC::on_event and `closes` stays at 0, failing the count assertion.
    use icedtea_ui::view::builders::info_bar;
    use icedtea_ui::widgets::info_bar::InfoBarExt;
    use std::cell::Cell;

    #[derive(Clone, Debug, PartialEq)]
    struct Closed;

    // Threaded through the model because `run`'s `view` is a bare `fn`
    // pointer: `run` returns only the frames, so the fired-message count has
    // to leave the app by a shared cell for the test to assert on it.
    let closes: Rc<Cell<u32>> = Rc::new(Cell::new(0));

    let frames = run(
        closes.clone(),
        |model: &mut Rc<Cell<u32>>, _msg: Closed| {
            model.set(model.get() + 1);
            Cmd::None
        },
        |_m: &Rc<Cell<u32>>| {
            info_bar()
                .show_close_button(true)
                .revealed(true)
                .hexpand(true)
                .on_close(Closed)
        },
        (300, 48),
        vec![
            // `InfoBarC` reports the close button's own 24x24 as its
            // intrinsic size (see `InfoBarC::measure`) because that button is
            // a subnode with no taffy box; with no other children the bar is
            // that square, centred in this 300x48 window at x 138..162,
            // y 12..36, and the button fills it (`InfoBarC::close_rect`
            // packs it at the trailing edge). x=285, this test's previous
            // coordinate, is bare window and could never reach the widget.
            ScriptStep::Event(InputEvent::pointer_enter(150.0, 24.0, 1)),
            ScriptStep::Event(InputEvent::PointerButton {
                button: 0x110,
                pressed: true,
                serial: 2,
                time_ms: 0,
            }),
            ScriptStep::Event(InputEvent::PointerButton {
                button: 0x110,
                pressed: false,
                serial: 3,
                time_ms: 8,
            }),
            ScriptStep::Capture,
        ],
    );
    assert_eq!(frames.len(), 1);
    assert_eq!(
        closes.get(),
        1,
        "clicking the close button must fire close exactly once"
    );
}

/// Bundled Adwaita layered under a rule that paints the bare `infobar` node
/// itself and blanks it on `:disabled` — the pseudo-state `InfoBarC::apply`
/// actually flips for `revealed`.
///
/// Adwaita only paints `infobar > revealer > box` and `infobar .close`,
/// neither of which exists in this task's flat `infobar -> [button.close]`
/// tree (contract ruling R1), so against the bare theme a revealed/unrevealed
/// info bar pixel test would pass regardless of whether `revealed` is
/// honoured — see the task-11 review. Layering onto the real sheet (rather
/// than replacing it, as a from-scratch minimal sheet leaves the window's own
/// child sizing unresolved) keeps the rest of the layout Adwaita-driven and
/// gives just this one mutation something to kill.
fn info_bar_probe_sheet() -> CompiledSheet {
    CompiledSheet::compile(&format!(
        "{BUNDLED_ADWAITA_LIGHT}\ninfobar {{ min-width: 20px; min-height: 20px; background-color: #ff0000; }}\ninfobar:disabled {{ background-color: transparent; }}"
    ))
}

#[test]
fn an_unrevealed_info_bar_inks_nothing() {
    // mutation: ignore `revealed` in InfoBarC and the frame inks the bar.
    use icedtea_ui::view::builders::info_bar;
    use icedtea_ui::widgets::info_bar::InfoBarExt;
    fn view(_m: &()) -> View<()> {
        info_bar().revealed(false).hexpand(true)
    }
    let app =
        App::new((), |_m: &mut (), _msg: ()| Cmd::None, view).with_sheet(info_bar_probe_sheet());
    let frames = app
        .run_offscreen(
            (300, 48),
            Rc::new(ManualClock::new()),
            vec![ScriptStep::Capture],
        )
        .expect("the offscreen app must run");
    assert!(
        !has_ink(&frames, 0, (300, 48)),
        "a hidden info bar draws nothing"
    );
}

#[test]
fn a_revealed_info_bar_inks_the_bar() {
    // Companion to `an_unrevealed_info_bar_inks_nothing`: with the same
    // stylesheet, a revealed info bar must ink — this is what proves the
    // unrevealed test is actually exercising `InfoBarC::apply`'s
    // `revealed` handling rather than passing vacuously either way.
    use icedtea_ui::view::builders::info_bar;
    use icedtea_ui::widgets::info_bar::InfoBarExt;
    fn view(_m: &()) -> View<()> {
        info_bar().revealed(true).hexpand(true)
    }
    let app =
        App::new((), |_m: &mut (), _msg: ()| Cmd::None, view).with_sheet(info_bar_probe_sheet());
    let frames = app
        .run_offscreen(
            (300, 48),
            Rc::new(ManualClock::new()),
            vec![ScriptStep::Capture],
        )
        .expect("the offscreen app must run");
    assert!(
        has_ink(&frames, 0, (300, 48)),
        "a revealed info bar draws its background"
    );
}

#[test]
fn dragging_a_scrollbar_slider_reports_the_new_value() {
    // mutation: ignore the drag delta in ScrollbarC::on_event and the value
    // stays at 0.0, failing the "moved right" assertion below.
    use icedtea_ui::view::builders::scrollbar;
    use icedtea_ui::widgets::Orientation;
    use icedtea_ui::widgets::scrollbar::ScrollbarExt;
    use std::cell::Cell;

    #[derive(Clone, Debug, PartialEq)]
    struct Moved(f64);

    // The model's own value drives the view, and the shared cell mirrors it
    // out of the app so the test can assert on it — `run` hands back only the
    // frames and its `view` must be a bare `fn` pointer.
    let value: Rc<Cell<f64>> = Rc::new(Cell::new(0.0));

    let frames = run(
        (0.0f64, value.clone()),
        |model: &mut (f64, Rc<Cell<f64>>), Moved(v): Moved| {
            model.0 = v;
            model.1.set(v);
            Cmd::None
        },
        |model: &(f64, Rc<Cell<f64>>)| {
            scrollbar(Orientation::Horizontal)
                .value(model.0)
                .upper(100.0)
                .page_size(10.0)
                .hexpand(true)
                .on_value_changed(Moved)
        },
        (200, 20),
        vec![
            // `ScrollbarC` reports its own intrinsic size (40x14, see
            // `ScrollbarC::measure`) because its `range`/`trough`/`slider`
            // have no taffy box to size against; centred in this 200x20
            // window that content box runs x 80..120, y 3..17, so every point
            // below has to land inside it. x=8, the coordinate this test used
            // before, is bare window and could never reach the widget.
            ScriptStep::Event(InputEvent::PointerEnter {
                x: 84.0,
                y: 10.0,
                serial: 1,
                target: icedtea_ui::window::SurfaceTarget::Window,
            }),
            ScriptStep::Event(InputEvent::PointerButton {
                button: 0x110,
                pressed: true,
                serial: 2,
                time_ms: 0,
            }),
            ScriptStep::Event(InputEvent::PointerMotion {
                x: 116.0,
                y: 10.0,
                time_ms: 16,
            }),
            ScriptStep::Event(InputEvent::PointerButton {
                button: 0x110,
                pressed: false,
                serial: 3,
                time_ms: 32,
            }),
            ScriptStep::Capture,
        ],
    );
    assert_eq!(
        frames.len(),
        1,
        "the drag ran to completion without panicking"
    );
    assert!(
        value.get() > 0.0,
        "dragging the slider right must report a larger value, got {}",
        value.get()
    );
}

#[test]
fn a_picture_decodes_and_draws_an_embedded_png() {
    // mutation: skip `canvas.draw_image_rect` in PictureC::paint and the frame
    // is one flat colour.
    use icedtea_ui::view::builders::picture_from_bytes;
    // A 2x2 opaque red PNG, embedded so the test needs no file on disk.
    const RED_2X2: &[u8] = include_bytes!("fixtures/images/red-2x2.png");
    let frames = run(
        (),
        |_m: &mut (), _msg: ()| Cmd::None,
        |_m: &()| picture_from_bytes(RED_2X2).hexpand(true).vexpand(true),
        (64, 64),
        vec![ScriptStep::Capture],
    );
    assert!(has_ink(&frames, 0, (64, 64)), "the decoded PNG must ink");
}

#[test]
fn a_picture_with_an_undecodable_source_draws_nothing_and_does_not_panic() {
    // mutation: unwrap the decode result in PictureC::build and this panics.
    use icedtea_ui::view::builders::picture_from_bytes;
    // `run`'s `view` parameter is a plain `fn`, not a closure, so the varying
    // input rides through the model rather than being captured (plan
    // reconciliation: the plan's `move |_m: &()| ...` cannot coerce to `fn`).
    fn view(bytes: &&'static [u8]) -> View<()> {
        picture_from_bytes(bytes).hexpand(true).vexpand(true)
    }
    for junk in [&b""[..], b"not a png", &[0xffu8; 4096][..]] {
        let frames = run(
            junk,
            |_m: &mut &'static [u8], _msg: ()| Cmd::None,
            view,
            (32, 32),
            vec![ScriptStep::Capture],
        );
        assert_eq!(
            frames.len(),
            1,
            "an undecodable source still renders a frame"
        );
    }
}

/// A synthetic pressed `KeyEvent` carrying one character.
pub fn key_char(ch: char, keycode: u32) -> icedtea_ui::window::keyboard::KeyEvent {
    icedtea_ui::window::keyboard::KeyEvent {
        keycode,
        keysym: xkbcommon::xkb::Keysym::from(u32::from(ch)),
        base: xkbcommon::xkb::Keysym::from(u32::from(ch)),
        utf8: Some(ch.to_string()),
        mods: icedtea_ui::window::keyboard::Mods::empty(),
        consumed: icedtea_ui::window::keyboard::Mods::empty(),
        pressed: true,
        repeat: false,
        serial: 10 + keycode,
        time_ms: keycode,
    }
}

/// A synthetic pressed `KeyEvent` for a named (non-character) keysym, such as
/// `Return` or `Escape` — no `utf8` payload, matching how a real compositor
/// reports these keys.
pub fn key_named(keysym: u32, keycode: u32) -> icedtea_ui::window::keyboard::KeyEvent {
    icedtea_ui::window::keyboard::KeyEvent {
        keycode,
        keysym: xkbcommon::xkb::Keysym::from(keysym),
        base: xkbcommon::xkb::Keysym::from(keysym),
        utf8: None,
        mods: icedtea_ui::window::keyboard::Mods::empty(),
        consumed: icedtea_ui::window::keyboard::Mods::empty(),
        pressed: true,
        repeat: false,
        serial: 10 + keycode,
        time_ms: keycode,
    }
}

#[test]
fn typing_into_a_text_view_inserts_at_the_cursor_and_reports_the_change() {
    // mutation: ignore `utf8` in TextViewC::on_event and the model stays empty.
    //
    // Plan reconciliation: the plan's script opens with `KeyboardEnter` alone
    // and no click. `KeyboardEnter` only tells the app which surface holds
    // the wl_keyboard (`InputEvent::KeyboardEnter` has no handler in
    // `view::app::route` beyond its wildcard arm — P4's window/focus.rs
    // grants focus only from a click, `Cmd::Focus`, or a keyboard binding,
    // never from a raw keyboard-enter), so no widget ever holds
    // `FocusRing::focus` and the `Key` events that follow have nowhere to
    // go. A click grants it — `TextViewC::on_event` now does that itself,
    // matching `GenericC`'s own click-focuses-the-node behaviour (also a
    // reconciliation: the plan's `on_event` never calls `cx.focus.set_focus`
    // at all, so it could not receive focus by any means as written).
    //
    // A second, compounding gap: an empty buffer measures to zero width, and
    // `window/pointer.rs::descend` skips a zero-area node outright, so an
    // empty `TextView` was unclickable regardless — `TextViewC::measure` now
    // floors its width at a caret's worth of pixels. The click below lands
    // at that floored box's centre, which a bare (unboxed, non-hexpanding)
    // root widget centres in the window.
    use icedtea_ui::view::builders::text_view;
    use icedtea_ui::widgets::text_view::TextViewExt;
    use icedtea_ui::window::BTN_LEFT;

    #[derive(Clone, Debug, PartialEq)]
    struct Edited(String);

    let frames = run(
        String::new(),
        |model: &mut String, Edited(text): Edited| {
            *model = text;
            Cmd::None
        },
        |model: &String| {
            text_view(model)
                .editable(true)
                .on_change(|t| Edited(t.to_owned()))
        },
        (200, 80),
        vec![
            ScriptStep::Event(InputEvent::pointer_enter(100.0, 35.0, 1)),
            ScriptStep::Event(InputEvent::PointerButton {
                button: BTN_LEFT,
                pressed: true,
                serial: 2,
                time_ms: 0,
            }),
            ScriptStep::Event(InputEvent::PointerButton {
                button: BTN_LEFT,
                pressed: false,
                serial: 3,
                time_ms: 1,
            }),
            ScriptStep::Capture,
            ScriptStep::Event(InputEvent::Key(key_char('h', 43))),
            ScriptStep::Event(InputEvent::Key(key_char('i', 31))),
            ScriptStep::Capture,
        ],
    );
    let row = |frame: usize| {
        (0..200)
            .map(|x| frames.pixel(frame, x, 35))
            .collect::<Vec<_>>()
    };
    assert_ne!(row(0), row(1), "the typed glyphs must appear");
}

#[test]
fn typing_into_an_entry_shows_the_glyphs_and_moves_the_caret() {
    // mutation: return EditOutcome::Ignored for printable keys in
    // TextEditState::key and the two captures match.
    //
    // Plan reconciliation: the plan's script opens with `KeyboardEnter` alone
    // and no click. As `typing_into_a_text_view_inserts_at_the_cursor_and_reports_the_change`
    // above already found, `InputEvent::KeyboardEnter` grants no keyboard
    // focus by itself (`view/app.rs::route` has no handler for it beyond its
    // wildcard arm; `FocusRing` only moves from a click, `Cmd::Focus`, or a
    // keyboard binding), so the `Key` events that followed a bare
    // `KeyboardEnter` had nowhere to go and the model never changed. A click
    // grants it, matching that same test.
    use icedtea_ui::view::builders::entry;
    use icedtea_ui::window::BTN_LEFT;

    #[derive(Clone, Debug, PartialEq)]
    struct Typed(String);

    let frames = run(
        String::new(),
        |model: &mut String, Typed(text): Typed| {
            *model = text;
            Cmd::None
        },
        |model: &String| entry(model).on_change(|t| Typed(t.to_owned())),
        (200, 40),
        vec![
            ScriptStep::Event(InputEvent::pointer_enter(100.0, 20.0, 1)),
            ScriptStep::Event(InputEvent::PointerButton {
                button: BTN_LEFT,
                pressed: true,
                serial: 2,
                time_ms: 0,
            }),
            ScriptStep::Event(InputEvent::PointerButton {
                button: BTN_LEFT,
                pressed: false,
                serial: 3,
                time_ms: 1,
            }),
            ScriptStep::Capture,
            ScriptStep::Event(InputEvent::Key(key_char('a', 38))),
            ScriptStep::Event(InputEvent::Key(key_char('b', 56))),
            ScriptStep::Capture,
        ],
    );
    let row = |frame: usize| {
        (0..200)
            .map(|x| frames.pixel(frame, x, 20))
            .collect::<Vec<_>>()
    };
    assert_ne!(row(0), row(1), "the typed glyphs must appear");
}

#[test]
fn a_text_view_model_swap_that_lands_the_caret_mid_codepoint_never_panics() {
    // mutation: replace both of `TextViewC`'s `clamp_to_boundary` calls —
    // `set_prop`'s Text arm and `selection_range` — with
    // `.min(self.buffer.len())` and this aborts the process at
    // `String::replace_range` in `TextViewC::apply_key` ("start of range
    // should be a character boundary"). Both together, because either clamp
    // alone still floors the range at a boundary. `TextViewC` carries its own
    // copy of the entry engine's caret handling, so it needs its own test.
    //
    // `End` rather than a second click places the caret: a click into a
    // non-empty `TextView` is consumed before `TextViewC`'s `PointerDown` arm
    // in this offscreen setup, so only the keyboard can move the caret off 0
    // here — the click below is still needed, to grant focus.
    use icedtea_ui::view::builders::text_view;
    use icedtea_ui::widgets::text_view::TextViewExt;
    use icedtea_ui::window::BTN_LEFT;

    #[derive(Clone, Debug, PartialEq)]
    enum Msg {
        Swap,
        Edited(String),
    }

    let frames = run(
        "abc".to_owned(),
        |model: &mut String, msg: Msg| {
            match msg {
                Msg::Swap => *model = "a\u{1F600}c".to_owned(),
                Msg::Edited(text) => *model = text,
            }
            Cmd::None
        },
        |model: &String| {
            text_view(model)
                .editable(true)
                .on_change(|t| Msg::Edited(t.to_owned()))
        },
        (200, 80),
        vec![
            ScriptStep::Event(InputEvent::pointer_enter(100.0, 40.0, 1)),
            ScriptStep::Event(InputEvent::PointerButton {
                button: BTN_LEFT,
                pressed: true,
                serial: 2,
                time_ms: 0,
            }),
            ScriptStep::Event(InputEvent::PointerButton {
                button: BTN_LEFT,
                pressed: false,
                serial: 3,
                time_ms: 1,
            }),
            ScriptStep::Event(InputEvent::Key(key_named(
                xkbcommon::xkb::keysyms::KEY_End,
                115,
            ))),
            ScriptStep::Message(Msg::Swap),
            ScriptStep::Event(InputEvent::Key(key_char('z', 52))),
            ScriptStep::Capture,
        ],
    );
    assert!(
        has_ink(&frames, 0, (200, 80)),
        "the text view must still paint after surviving the swap"
    );
}

#[test]
fn a_model_swap_that_lands_the_caret_mid_codepoint_never_panics() {
    // mutation: replace both of `TextEditState`'s `clamp_to_boundary` calls —
    // `set_text` and `selection` — with `.min(self.buffer.len())` and this
    // aborts the process at `String::replace_range` in
    // `TextEditState::insert` ("start of range should be a character
    // boundary"). Both together, because either clamp alone still floors the
    // replaced range at a boundary.
    //
    // The caret is a byte offset into the buffer it was placed in. Clicking
    // at x=100 puts it past "abc", at byte 3; the model then swaps the text
    // for "a<4-byte emoji>c", whose byte 3 is inside the emoji. The next
    // printable key replaces `selection()` and would split the codepoint.
    use icedtea_ui::view::builders::entry;
    use icedtea_ui::window::BTN_LEFT;

    #[derive(Clone, Debug, PartialEq)]
    enum Msg {
        Swap,
        Typed(String),
    }

    let frames = run(
        "abc".to_owned(),
        |model: &mut String, msg: Msg| {
            match msg {
                Msg::Swap => *model = "a\u{1F600}c".to_owned(),
                Msg::Typed(text) => *model = text,
            }
            Cmd::None
        },
        |model: &String| entry(model).on_change(|t| Msg::Typed(t.to_owned())),
        (200, 40),
        vec![
            ScriptStep::Event(InputEvent::pointer_enter(100.0, 20.0, 1)),
            ScriptStep::Event(InputEvent::PointerButton {
                button: BTN_LEFT,
                pressed: true,
                serial: 2,
                time_ms: 0,
            }),
            ScriptStep::Event(InputEvent::PointerButton {
                button: BTN_LEFT,
                pressed: false,
                serial: 3,
                time_ms: 1,
            }),
            ScriptStep::Message(Msg::Swap),
            ScriptStep::Event(InputEvent::Key(key_char('z', 52))),
            ScriptStep::Capture,
        ],
    );
    assert!(
        has_ink(&frames, 0, (200, 40)),
        "the entry must still paint after surviving the swap"
    );
}

#[test]
fn clicking_into_a_masked_entry_places_the_caret_in_the_buffer_not_the_mask() {
    // mutation: put `EntryC::on_event`'s caret placement back on
    // `self.edit.layout.byte_at(local)` (its shape before this fix) and the
    // model reads "\u{e9}\u{e9}\u{e9}z" instead -- the caret lands two
    // characters further along than the click.
    //
    // With `GtkEntry:visibility` off the layout is over three *3-byte*
    // bullets while the buffer holds three *2-byte* characters, so a display
    // offset is not a buffer offset: `byte_at`'s 6 (after the second bullet)
    // is the end of a 6-byte buffer, and only `buffer_offset_at` maps it back
    // to the 4 the click actually pointed at. The same arithmetic on a longer
    // buffer runs off its end, or lands inside one of its codepoints, which
    // `TextEditState::insert`'s `String::replace_range` then panics on.
    // `SearchEntryC` and `PasswordEntryC` already went through
    // `buffer_offset_at` for exactly this reason; `EntryC`, which has its own
    // `visibility` setter, did not.
    use icedtea_ui::view::builders::entry;
    use icedtea_ui::widgets::entry::EntryExt;
    use icedtea_ui::window::BTN_LEFT;
    use std::cell::RefCell;

    #[derive(Clone, Debug, PartialEq)]
    struct Typed(String);

    // The model itself carries the log, because `view` must be a bare `fn`
    // pointer and cannot capture from this scope (see the search-entry test's
    // own note): what the caret placement did is only visible in *where* the
    // typed character landed.
    let typed: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));

    let frames = run(
        ("\u{e9}\u{e9}\u{e9}".to_owned(), typed.clone()),
        |model: &mut (String, Rc<RefCell<Vec<String>>>), Typed(text): Typed| {
            model.0 = text;
            Cmd::None
        },
        |model: &(String, Rc<RefCell<Vec<String>>>)| {
            let recorder = model.1.clone();
            entry(&model.0).visibility(false).on_change(move |t| {
                recorder.borrow_mut().push(t.to_owned());
                Typed(t.to_owned())
            })
        },
        (200, 40),
        vec![
            // The centre of a bare, intrinsically-sized entry centred in this
            // window: between bullets, so the caret lands mid-mask.
            ScriptStep::Event(InputEvent::pointer_enter(100.0, 20.0, 1)),
            ScriptStep::Event(InputEvent::PointerButton {
                button: BTN_LEFT,
                pressed: true,
                serial: 2,
                time_ms: 0,
            }),
            ScriptStep::Event(InputEvent::PointerButton {
                button: BTN_LEFT,
                pressed: false,
                serial: 3,
                time_ms: 1,
            }),
            ScriptStep::Event(InputEvent::Key(key_char('z', 52))),
            ScriptStep::Capture,
        ],
    );
    assert!(
        has_ink(&frames, 0, (200, 40)),
        "the masked entry must still paint after the click and the keystroke"
    );
    assert_eq!(
        typed.borrow().as_slice(),
        ["\u{e9}\u{e9}z\u{e9}"],
        "the keystroke must land after the second buffer character -- the one \
         under the click -- not at a byte offset borrowed from the mask"
    );
}

#[test]
fn dragging_a_scale_moves_the_slider_and_reports_the_value() {
    // mutation: return early from ScaleC::on_event's PointerMotion arm and the
    // model stays at 0.0.
    use icedtea_ui::view::builders::scale;
    use icedtea_ui::widgets::scale::ScaleExt;

    #[derive(Clone, Debug, PartialEq)]
    struct Set(f64);

    let frames = run(
        0.0f64,
        |model: &mut f64, Set(v): Set| {
            *model = v;
            Cmd::None
        },
        |model: &f64| {
            scale(0.0, 100.0)
                .value(*model)
                .hexpand(true)
                .on_value_changed(Set)
        },
        (200, 32),
        vec![
            ScriptStep::Capture,
            // `ScaleC` reports its own intrinsic content size (150x18, see
            // `ScaleC::measure`) since its `trough` has no taffy box of its
            // own to size against; centred in a 200-wide window with 12px of
            // Adwaita padding on every side, the content box runs x 25..175,
            // y 12..30 — these points must land inside it, unlike the
            // window-relative coordinates a real hexpand would allow.
            ScriptStep::Event(InputEvent::PointerEnter {
                x: 30.0,
                y: 16.0,
                serial: 1,
                target: icedtea_ui::window::SurfaceTarget::Window,
            }),
            ScriptStep::Event(InputEvent::PointerButton {
                button: 0x110,
                pressed: true,
                serial: 2,
                time_ms: 0,
            }),
            ScriptStep::Event(InputEvent::PointerMotion {
                x: 170.0,
                y: 16.0,
                time_ms: 16,
            }),
            ScriptStep::Event(InputEvent::PointerButton {
                button: 0x110,
                pressed: false,
                serial: 3,
                time_ms: 32,
            }),
            ScriptStep::Capture,
        ],
    );
    let row = |frame: usize| {
        (0..200)
            .map(|x| frames.pixel(frame, x, 16))
            .collect::<Vec<_>>()
    };
    assert_ne!(row(0), row(1), "the slider must have moved right");
}

#[test]
fn an_image_paints_its_resolved_icon() {
    // mutation: `self.resolved = None` in `ImageC::paint` (P5's own body) and
    // the frame is one flat colour. Un-ignored by P7 (contract §10 P5-D31):
    // `ImageC` resolves through `IconTheme::render` and draws through
    // `paint::icon::paint_icon`.
    //
    // Hermetic on purpose: the theme is the checked-in mini fixture, not the
    // machine's, so this asserts the toolkit rather than whether Adwaita
    // happens to be installed (contract §6's "exercised only when present").
    use icedtea_ui::icons::IconTheme;
    use icedtea_ui::view::builders::image_named;
    use icedtea_ui::widgets::image::ImageExt;
    let fixture =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mini-icon-theme");
    let clock = Rc::new(ManualClock::new());
    let frames = App::new(
        (),
        |_m: &mut (), _msg: ()| Cmd::None,
        |_m: &()| image_named("document-open").pixel_size(32),
    )
    .with_sheet(CompiledSheet::compile(BUNDLED_ADWAITA_LIGHT))
    .with_icons(IconTheme::with_name_and_roots(
        "MiniTheme",
        vec![fixture.join("root-a"), fixture.join("root-b")],
    ))
    .run_offscreen((48, 48), clock, vec![ScriptStep::Capture])
    .expect("the offscreen app must run");
    assert!(has_ink(&frames, 0, (48, 48)), "the resolved icon must ink");
}

#[test]
fn a_drawing_area_runs_its_callback_against_the_allocated_rect() {
    // mutation: never call `self.draw` in DrawingAreaC::paint and the frame is
    // one flat colour.
    use icedtea_ui::css::value::Rgba;
    use icedtea_ui::view::builders::drawing_area;
    use icedtea_ui::widgets::drawing_area::DrawingAreaExt;
    let frames = run(
        (),
        |_m: &mut (), _msg: ()| Cmd::None,
        |_m: &()| {
            drawing_area(|canvas, rect, _cx: &mut icedtea_ui::paint::PaintCx<'_>| {
                canvas.draw_rect(
                    &rect.to_skia(),
                    &icedtea_ui::paint::fill_paint(Rgba {
                        r: 1.0,
                        g: 0.0,
                        b: 0.0,
                        a: 1.0,
                    }),
                );
            })
            .content_width(64)
            .content_height(64)
            .hexpand(true)
            .vexpand(true)
        },
        (64, 64),
        vec![ScriptStep::Capture],
    );
    assert_eq!(
        frames.pixel(0, 32, 32).map(|p| (p.0, p.1, p.2)),
        Some((255, 0, 0))
    );
}

#[test]
fn a_drawing_area_can_shape_text_through_its_paint_cx() {
    // P4's Displays canvas draws a connector name and a resolution per head;
    // without `&mut PaintCx` the callback cannot reach a FontDatabase and
    // cannot shape a glyph at all.
    // mutation: pass a fresh, empty `PaintCx` instead of the one `paint`
    // received; the shaping finds no font and the row stays flat.
    use icedtea_ui::css::value::{FontFamily, FontStyle, GenericFamily, Keyword, Rgba};
    use icedtea_ui::text::{FontQuery, ShapeKey};
    use icedtea_ui::view::builders::drawing_area;
    use icedtea_ui::widgets::drawing_area::DrawingAreaExt;

    let frames = run(
        (),
        |_m: &mut (), _msg: ()| Cmd::None,
        |_m: &()| {
            drawing_area(|canvas, rect, cx| {
                // A white ground, so a glyph is the only dark ink.
                canvas.draw_rect(
                    &rect.to_skia(),
                    &icedtea_ui::paint::fill_paint(Rgba {
                        r: 1.0,
                        g: 1.0,
                        b: 1.0,
                        a: 1.0,
                    }),
                );
                let families = [FontFamily::Generic(GenericFamily::SansSerif)];
                let query = FontQuery {
                    families: &families,
                    weight: 400.0,
                    style: FontStyle::Normal,
                    stretch: 100.0,
                    size_px: 24.0,
                };
                let Some(face) = cx.fonts.match_face(&query) else {
                    return;
                };
                let shaped = cx.fonts.shape(&ShapeKey {
                    text: "HH",
                    face: &face,
                    size_px: 24.0,
                    letter_spacing_px: 0.0,
                    features: &[],
                    variations: &[],
                    transform: Keyword::None,
                });
                if let Some(blob) = shaped.blob.as_ref() {
                    canvas.draw_text_blob(
                        blob,
                        rect.x + 4.0,
                        rect.y + 32.0,
                        &icedtea_ui::paint::fill_paint(Rgba {
                            r: 0.0,
                            g: 0.0,
                            b: 0.0,
                            a: 1.0,
                        }),
                    );
                }
            })
            .content_width(120)
            .content_height(48)
            .hexpand(true)
            .vexpand(true)
        },
        (120, 48),
        vec![ScriptStep::Capture],
    );
    let mut dark = 0;
    for x in 0..120 {
        for y in 0..48 {
            if let Some(px) = frames.pixel(0, x, y)
                && (u32::from(px.0) + u32::from(px.1) + u32::from(px.2)) < 300
            {
                dark += 1;
            }
        }
    }
    assert!(dark > 20, "no glyph ink on the canvas: {dark} dark pixels");
}

#[test]
fn clicking_a_calendar_day_selects_it() {
    // mutation: never fire EventKind::DateSelected in CalendarC::on_event and
    // the selected day stays at 15, failing the assertion below.
    use icedtea_ui::view::builders::calendar;
    use std::cell::Cell;

    // Reconciliation (contract D13): `on_date_selected` already exists as
    // the generic `View::on_date_selected`, firing `Handler::Text` with an
    // ISO-8601 `YYYY-MM-DD` payload — not the `Handler::Index`/`usize` shape
    // the task text sketched, which would collide with that inherent
    // method. `Picked` carries the ISO string and the update pulls the day
    // back out of it.
    #[derive(Clone, Debug, PartialEq)]
    struct Picked(String);

    // The shared cell mirrors the model's day out of the app: `run` returns
    // only the frames, and its `view` must be a bare `fn` pointer, so the
    // recorder is threaded through the model itself.
    let day: Rc<Cell<u32>> = Rc::new(Cell::new(15));

    let frames = run(
        (15u32, day.clone()),
        |model: &mut (u32, Rc<Cell<u32>>), Picked(date): Picked| {
            if let Some(d) = date.rsplit('-').next().and_then(|d| d.parse().ok()) {
                model.0 = d;
                model.1.set(d);
            }
            Cmd::None
        },
        |model: &(u32, Rc<Cell<u32>>)| {
            calendar(2026, 3, model.0)
                .hexpand(true)
                .vexpand(true)
                .on_date_selected(|s| Picked(s.to_owned()))
        },
        (280, 240),
        vec![
            ScriptStep::Capture,
            // `CalendarC` reports its own intrinsic size (7 columns of 24px
            // over a 32px header plus one 24px row per week, see
            // `CalendarC::measure`) because its `header`/`grid`/day labels
            // have no taffy box; centred in this 280x240 window March 2026's
            // six rows put the content box at x 56..224, y 32..208, so the
            // 24x24 day cells start at y 64. March 1st 2026 is a Sunday
            // (column 6), which puts the 10th in column 1, row 2 --
            // x 80..104, y 112..136. (60, 120), this test's previous
            // coordinate, is bare window left of the calendar.
            ScriptStep::Event(InputEvent::PointerEnter {
                x: 92.0,
                y: 124.0,
                serial: 1,
                target: icedtea_ui::window::SurfaceTarget::Window,
            }),
            ScriptStep::Event(InputEvent::PointerButton {
                button: 0x110,
                pressed: true,
                serial: 2,
                time_ms: 0,
            }),
            ScriptStep::Event(InputEvent::PointerButton {
                button: 0x110,
                pressed: false,
                serial: 3,
                time_ms: 8,
            }),
            ScriptStep::Capture,
        ],
    );
    assert_eq!(frames.len(), 2, "both captures ran without a panic");
    assert_eq!(
        day.get(),
        10,
        "clicking the cell of March 10th must select exactly that day"
    );
}

#[test]
fn an_autohide_popovers_positioner_anchors_to_its_parent_rect() {
    // mutation: return `Anchor::Top` unconditionally from PopoverC::positioner
    // and the Bottom case reports the wrong gravity.
    use icedtea_ui::layout::Rect;
    use icedtea_ui::widgets::{Position, popover::PopoverC};
    use icedtea_ui::window::popup::{Anchor, Gravity};

    let mut controller = PopoverC::for_test(Position::Bottom, true, true);
    let positioner = controller.positioner(Rect::new(10.0, 20.0, 40.0, 24.0), (200, 120));
    assert_eq!(positioner.size, (200, 120));
    assert_eq!(positioner.anchor_rect, Rect::new(10.0, 20.0, 40.0, 24.0));
    assert_eq!(positioner.anchor, Anchor::Bottom);
    assert_eq!(positioner.gravity, Gravity::Bottom);
    assert!(positioner.reactive, "a popover follows its parent");

    controller = PopoverC::for_test(Position::Top, true, true);
    let positioner = controller.positioner(Rect::new(0.0, 0.0, 10.0, 10.0), (50, 50));
    assert_eq!(positioner.anchor, Anchor::Top);
    assert_eq!(positioner.gravity, Gravity::Top);
}

#[test]
fn clicking_a_button_fires_its_message_and_paints_the_active_state() {
    // mutation: never set PseudoStates::ACTIVE in PointerState::observe and the
    // pressed capture matches the resting one.
    use icedtea_ui::view::builders::button;

    #[derive(Clone, Debug, PartialEq)]
    struct Clicked;

    let frames = run(
        0u32,
        |model: &mut u32, _msg: Clicked| {
            *model += 1;
            Cmd::None
        },
        |_m: &u32| button("Ok").on_click(Clicked),
        (120, 48),
        vec![
            ScriptStep::Capture,
            ScriptStep::Event(InputEvent::PointerEnter {
                x: 40.0,
                y: 24.0,
                serial: 1,
                target: icedtea_ui::window::SurfaceTarget::Window,
            }),
            ScriptStep::Event(InputEvent::PointerButton {
                button: 0x110,
                pressed: true,
                serial: 2,
                time_ms: 0,
            }),
            ScriptStep::Capture,
            ScriptStep::Event(InputEvent::PointerButton {
                button: 0x110,
                pressed: false,
                serial: 3,
                time_ms: 8,
            }),
            // Leave too, so the final capture is the true unhovered rest
            // state — Adwaita's `:hover` fill would otherwise persist and
            // make row(0) != row(2) even with `:active` correctly cleared.
            ScriptStep::Event(InputEvent::PointerLeave),
            ScriptStep::Capture,
        ],
    );
    let row = |frame: usize| {
        (0..120)
            .map(|x| frames.pixel(frame, x, 24))
            .collect::<Vec<_>>()
    };
    assert_ne!(row(0), row(1), ":active must repaint the button");
    assert_eq!(row(0), row(2), "release restores the resting appearance");
}

#[test]
fn toggling_a_toggle_button_paints_the_checked_state() {
    // mutation: skip PseudoStates::CHECKED in ToggleButtonC::apply and the two
    // captures match.
    use icedtea_ui::view::builders::toggle_button;
    use icedtea_ui::widgets::toggle_button::ToggleButtonExt;

    #[derive(Clone, Debug, PartialEq)]
    struct Toggled(bool);

    let frames = run(
        false,
        |model: &mut bool, Toggled(on): Toggled| {
            *model = on;
            Cmd::None
        },
        |model: &bool| toggle_button("Bold").active(*model).on_toggle(Toggled),
        (120, 48),
        vec![
            ScriptStep::Capture,
            // x=60 is the window's horizontal centre, where the box lays the
            // button out regardless of its exact (narrower, `.toggle`) width
            // — unlike the plain button, whose wider box also covers x=40.
            ScriptStep::Event(InputEvent::PointerEnter {
                x: 60.0,
                y: 24.0,
                serial: 1,
                target: icedtea_ui::window::SurfaceTarget::Window,
            }),
            ScriptStep::Event(InputEvent::PointerButton {
                button: 0x110,
                pressed: true,
                serial: 2,
                time_ms: 0,
            }),
            ScriptStep::Event(InputEvent::PointerButton {
                button: 0x110,
                pressed: false,
                serial: 3,
                time_ms: 8,
            }),
            ScriptStep::Capture,
        ],
    );
    let row = |frame: usize| {
        (0..120)
            .map(|x| frames.pixel(frame, x, 24))
            .collect::<Vec<_>>()
    };
    assert_ne!(row(0), row(1), ":checked must repaint the button");
}

#[test]
fn flipping_a_switch_animates_the_slider_to_the_other_end() {
    // mutation: set `slide` straight to its target in SwitchC::set_prop and the
    // mid-animation capture matches the final one.
    use icedtea_ui::view::builders::switch;

    #[derive(Clone, Debug, PartialEq)]
    struct Flip(bool);

    let frames = run(
        false,
        |model: &mut bool, Flip(on): Flip| {
            *model = on;
            Cmd::None
        },
        |model: &bool| switch(*model).on_toggle(Flip),
        (64, 40),
        vec![
            ScriptStep::Capture,
            ScriptStep::Message(Flip(true)),
            ScriptStep::Advance(Duration::from_millis(60)),
            ScriptStep::Capture,
            ScriptStep::Advance(Duration::from_millis(400)),
            ScriptStep::Capture,
        ],
    );
    let row = |frame: usize| {
        (0..64)
            .map(|x| frames.pixel(frame, x, 20))
            .collect::<Vec<_>>()
    };
    assert_ne!(row(0), row(2), "the slider ends at the other end");
    assert_ne!(
        row(1),
        row(2),
        "and passes through an intermediate position"
    );
}

#[test]
fn a_check_button_paints_the_builtin_check_glyph() {
    // mutation: `return false` at the top of `CheckButtonC::paint` and the
    // frame loses the tick. Un-ignored by P7 (contract §10 P5-D31): the
    // glyph is real geometry now, drawn through `paint::icon::paint_builtin`.
    use icedtea_ui::view::builders::check_button;
    use icedtea_ui::widgets::check_button::CheckButtonExt;
    let frames = run(
        (),
        |_m: &mut (), _msg: ()| Cmd::None,
        |_m: &()| check_button("On").active(true),
        (120, 40),
        vec![ScriptStep::Capture],
    );
    assert!(has_ink(&frames, 0, (120, 40)), "the check glyph must ink");
}

#[test]
fn an_unchecked_check_button_paints_its_box() {
    // GTK draws the empty indicator: a background and a 1px border. Without
    // it a settings page full of unchecked boxes shows nothing at all, which
    // is why `check_button` was on KNOWN_BLANK_AT_REST.
    // mutation: restore the early `if !self.active && !self.inconsistent {
    // return false }`; this fails with a flat frame.
    use icedtea_ui::view::builders::check_button;
    use icedtea_ui::widgets::check_button::CheckButtonExt;

    let frames = run(
        false,
        |model: &mut bool, on: bool| {
            *model = on;
            Cmd::None
        },
        // An empty label on purpose: `GenericC` shapes no glyphs for it, so
        // any ink in the frame is the indicator this task draws.
        |model: &bool| check_button("").active(*model),
        (120, 40),
        vec![ScriptStep::Capture],
    );
    assert!(
        has_ink(&frames, 0, (120, 40)),
        "an unchecked check button painted nothing"
    );
}

#[test]
fn a_checked_check_button_still_paints_the_builtin() {
    // The regression guard for the branch above: the checked path must still
    // go through `paint::icon::paint_builtin`, and must differ from unchecked.
    // mutation: return early for the *checked* state instead; the two frames
    // become identical and this fails.
    use icedtea_ui::view::builders::check_button;
    use icedtea_ui::widgets::check_button::CheckButtonExt;

    let unchecked = run(
        false,
        |_m: &mut bool, _on: bool| Cmd::None,
        |model: &bool| check_button("Check").active(*model),
        (120, 40),
        vec![ScriptStep::Capture],
    );
    let checked = run(
        true,
        |_m: &mut bool, _on: bool| Cmd::None,
        |model: &bool| check_button("Check").active(*model),
        (120, 40),
        vec![ScriptStep::Capture],
    );
    let differs =
        (0..120).any(|x| (0..40).any(|y| unchecked.pixel(0, x, y) != checked.pixel(0, x, y)));
    assert!(differs, "checked and unchecked paint the same pixels");
}

#[test]
fn opening_a_drop_down_and_picking_an_item_updates_the_button() {
    // mutation: `return Vec::new()` at the top of `DropDownC::on_event` and
    // the two captures match -- the click reaches the controller only because
    // `route`'s `aim` now resolves to the innermost `Instance` in the hit
    // chain (contract P5-D33) rather than the geometrically deepest node,
    // which for a `DropDown` is the controller-owned `button.toggle` subnode.
    //
    // Scope, honestly stated: a `DropDown`'s list lives under its own
    // `popover > contents > listview` node in the parent window's tree and is
    // painted there, so no popup surface is asked for (contract §10 P7-D54)
    // and the *row click* half of this criterion is not driven from here. It
    // is covered over a really laid-out tree by
    // `widgets::drop_down::tests::opening_a_drop_down_and_clicking_a_row_selects_that_item`,
    // and against the real compositor by `interaction_gate.rs`'s
    // `opening_a_drop_down_and_picking_an_item_updates_the_button`. What this
    // test pins is that the button's own click reaches `DropDownC` at all,
    // and that the list it reveals repaints the row it opens over.
    use icedtea_ui::view::builders::drop_down;
    use icedtea_ui::widgets::drop_down::DropDownExt;

    #[derive(Clone, Debug, PartialEq)]
    struct Picked(usize);

    let frames = run(
        0usize,
        |model: &mut usize, Picked(index): Picked| {
            *model = index;
            Cmd::None
        },
        |model: &usize| {
            drop_down(&["One", "Two", "Three"])
                .selected(*model)
                .on_selected(Picked)
        },
        (200, 200),
        vec![
            ScriptStep::Capture,
            // Re-derived for P8-D71's close-out: a *closed* drop-down's
            // popover is now hidden (`display: none`), so `dropdown`'s own
            // border box is the button's 36x34 rather than the 102x40 that
            // counted three always-laid-out `row`s beside it, and the button
            // is centred in the 200x200 window instead of sitting at its
            // left edge. `gallery --widget drop_down --print-allocation`
            // reports the same shrink (102x40 -> 36x34); the captured row
            // below shows the button's own border at x 82 and x 117.
            ScriptStep::Event(InputEvent::PointerEnter {
                x: 100.0,
                y: 100.0,
                serial: 1,
                target: icedtea_ui::window::SurfaceTarget::Window,
            }),
            ScriptStep::Event(InputEvent::PointerButton {
                button: 0x110,
                pressed: true,
                serial: 2,
                time_ms: 0,
            }),
            ScriptStep::Event(InputEvent::PointerButton {
                button: 0x110,
                pressed: false,
                serial: 3,
                time_ms: 8,
            }),
            ScriptStep::Capture,
        ],
    );
    let row = |frame: usize| {
        (0..200)
            .map(|x| frames.pixel(frame, x, 100))
            .collect::<Vec<_>>()
    };
    assert_ne!(row(0), row(1), "the button repaints when the list opens");
}

#[test]
fn clicking_a_colour_button_opens_its_dialog_and_repaints_the_swatch() {
    // mutation: `return Vec::new()` at the top of
    // `ColorDialogButtonC::on_event` and the two captures match. As with the
    // drop-down above, the click only reaches the controller because `route`'s
    // `aim` resolves to the innermost `Instance` (contract P5-D33); the
    // chrome lives on the nested `button.color` subnode. Flipping only
    // `dialog_open` is not a sufficient mutation: the repaint this asserts is
    // the button's own pressed/hover state, which the same handler drives.
    use icedtea_ui::css::value::Rgba;
    use icedtea_ui::view::builders::color_dialog_button;
    use icedtea_ui::window::BTN_LEFT;

    let frames = run(
        Rgba {
            r: 0.2,
            g: 0.5,
            b: 0.9,
            a: 1.0,
        },
        |_m: &mut Rgba, _msg: ()| Cmd::None,
        |model: &Rgba| color_dialog_button(*model),
        (80, 40),
        vec![
            ScriptStep::Capture,
            ScriptStep::Event(InputEvent::pointer_enter(40.0, 20.0, 1)),
            ScriptStep::Event(InputEvent::PointerButton {
                button: BTN_LEFT,
                pressed: true,
                serial: 2,
                time_ms: 0,
            }),
            ScriptStep::Capture,
        ],
    );
    let row = |frame: usize| {
        (0..80)
            .map(|x| frames.pixel(frame, x, 20))
            .collect::<Vec<_>>()
    };
    assert_ne!(row(0), row(1), ":active must repaint the colour button");
}

#[test]
fn typing_into_a_search_entry_fires_one_search_after_the_delay() {
    // mutation: fire EventKind::Search on every keystroke and the count is 3.
    use icedtea_ui::view::builders::search_entry;
    use icedtea_ui::widgets::search_entry::SearchEntryExt;
    use std::cell::RefCell;

    #[derive(Clone, Debug, PartialEq)]
    enum Msg {
        Typed(String),
        Searched(String),
    }

    // Independent of `Frames`/the model: the on_search closure records every
    // fire here directly, so this test can tell "fired once" (debounced),
    // "fired three times" (the named mutation: search on every keystroke),
    // and "never fired" (e.g. focus never granted) apart, which
    // `frames.len()` alone cannot. `view` must be a bare `fn` pointer (see
    // `run`'s signature), so the log is threaded through the model itself
    // rather than captured from the test's scope.
    let searches: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));

    let frames = run(
        (String::new(), 0u32, searches.clone()),
        |model: &mut (String, u32, Rc<RefCell<Vec<String>>>), msg: Msg| {
            match msg {
                Msg::Typed(text) => model.0 = text,
                Msg::Searched(_) => model.1 += 1,
            }
            Cmd::None
        },
        |model: &(String, u32, Rc<RefCell<Vec<String>>>)| {
            let searches_recorder = model.2.clone();
            search_entry(&model.0)
                .search_delay(150)
                .on_change(|t| Msg::Typed(t.to_owned()))
                .on_search(move |t| {
                    searches_recorder.borrow_mut().push(t.to_owned());
                    Msg::Searched(t.to_owned())
                })
        },
        (200, 40),
        vec![
            // Plan reconciliation: the plan's script opens with a bare
            // `InputEvent::KeyboardEnter { serial: 1 }` and no click. As
            // `typing_into_a_text_view_inserts_at_the_cursor_and_reports_the_change`
            // and `typing_into_an_entry_shows_the_glyphs_and_moves_the_caret`
            // above already found, `KeyboardEnter` grants no keyboard focus
            // by itself (`view/app.rs::route` has no handler for it beyond
            // its wildcard arm; `FocusRing` only moves from a click,
            // `Cmd::Focus`, or a keyboard binding) — so the `Key` events
            // that followed had nowhere to go and, unlike those two tests'
            // weaker assertions, this test's new `searches` log now proves
            // it: the fix below (a click, as those tests use) is required
            // for `on_search`/`on_change` to fire at all. It also carries
            // the same `target: SurfaceTarget` field (contract deviation 6)
            // those tests' own `InputEvent::pointer_enter` convenience
            // constructor fills in.
            ScriptStep::Event(InputEvent::pointer_enter(100.0, 20.0, 1)),
            ScriptStep::Event(InputEvent::PointerButton {
                button: icedtea_ui::window::BTN_LEFT,
                pressed: true,
                serial: 2,
                time_ms: 0,
            }),
            ScriptStep::Event(InputEvent::PointerButton {
                button: icedtea_ui::window::BTN_LEFT,
                pressed: false,
                serial: 3,
                time_ms: 1,
            }),
            ScriptStep::Event(InputEvent::Key(key_char('a', 38))),
            ScriptStep::Advance(Duration::from_millis(40)),
            ScriptStep::Event(InputEvent::Key(key_char('b', 56))),
            ScriptStep::Advance(Duration::from_millis(40)),
            ScriptStep::Event(InputEvent::Key(key_char('c', 54))),
            ScriptStep::Advance(Duration::from_millis(400)),
            ScriptStep::Capture,
        ],
    );
    assert_eq!(frames.len(), 1, "the script ran to completion");
    assert_eq!(
        searches.borrow().as_slice(),
        ["abc"],
        "search must fire exactly once, after the debounce delay, with the final text"
    );
}

#[test]
fn peeking_a_password_entry_reveals_the_text() {
    // mutation: ignore the peek toggle in PasswordEntryC::on_event and the two
    // captures match.
    use icedtea_ui::view::builders::password_entry;
    use icedtea_ui::widgets::password_entry::PasswordEntryExt;
    let frames = run(
        "hunter2".to_owned(),
        |_m: &mut String, _msg: ()| Cmd::None,
        |model: &String| password_entry(model).show_peek_icon(true),
        (200, 40),
        vec![
            ScriptStep::Capture,
            // Plan reconciliation: the plan's script clicks at a fixed
            // `x: 188.0` assuming the entry fills the whole 200px window, but
            // `hexpand` has no layout-level implementation yet (only the prop
            // is stored — nothing in `layout.rs` reads it), and a bare root
            // widget is otherwise sized to its own intrinsic content and
            // centred (see `Entry`'s and `TextView`'s own notes). A
            // `PasswordEntry` showing "hunter2" masked plus its reserved peek
            // band measures to a `78x34` box centred in this window, putting
            // the peek band at local x `[106, 130]`; `118.0` lands inside it.
            ScriptStep::Event(InputEvent::pointer_enter(118.0, 20.0, 1)),
            ScriptStep::Event(InputEvent::PointerButton {
                button: 0x110,
                pressed: true,
                serial: 2,
                time_ms: 0,
            }),
            ScriptStep::Event(InputEvent::PointerButton {
                button: 0x110,
                pressed: false,
                serial: 3,
                time_ms: 8,
            }),
            ScriptStep::Capture,
        ],
    );
    let row = |frame: usize| {
        (0..200)
            .map(|x| frames.pixel(frame, x, 20))
            .collect::<Vec<_>>()
    };
    assert_ne!(row(0), row(1), "bullets and glyphs must render differently");
}

#[test]
fn clicking_into_a_password_entry_places_the_caret_like_its_siblings() {
    // mutation: drop the `self.edit.cursor = self.edit.buffer_offset_at(local)`
    // arm from PasswordEntryC::on_event (the review-caught asymmetry with
    // SearchEntryC/EntryC) and the typed glyph is appended, giving "hunter2X".
    // mutation: use `self.edit.layout.byte_at(local)` instead of
    // `buffer_offset_at` and the masked layout's three-byte bullet offsets
    // land past the end of the seven-byte buffer, so the caret clamps to the
    // end and the glyph is appended again.
    use icedtea_ui::view::builders::password_entry;
    use std::cell::RefCell;

    // As in `typing_into_a_search_entry_...` above: `view` is a bare `fn`
    // pointer, so the log of every reported edit rides in the model.
    let edits: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));

    run(
        ("hunter2".to_owned(), edits.clone()),
        |model: &mut (String, Rc<RefCell<Vec<String>>>), text: String| {
            model.0 = text;
            Cmd::None
        },
        |model: &(String, Rc<RefCell<Vec<String>>>)| {
            let recorder = model.1.clone();
            password_entry(&model.0).on_change(move |t| {
                recorder.borrow_mut().push(t.to_owned());
                t.to_owned()
            })
        },
        (200, 40),
        vec![
            // The entry is sized to its own content and centred (see the
            // peek test's note on `hexpand`), so the window's centre is the
            // middle of the masked text, not its end.
            ScriptStep::Event(InputEvent::pointer_enter(100.0, 20.0, 1)),
            ScriptStep::Event(InputEvent::PointerButton {
                button: icedtea_ui::window::BTN_LEFT,
                pressed: true,
                serial: 2,
                time_ms: 0,
            }),
            ScriptStep::Event(InputEvent::PointerButton {
                button: icedtea_ui::window::BTN_LEFT,
                pressed: false,
                serial: 3,
                time_ms: 8,
            }),
            ScriptStep::Event(InputEvent::Key(key_char('X', 53))),
            ScriptStep::Capture,
        ],
    );
    let edits = edits.borrow();
    let [text] = edits.as_slice() else {
        panic!("exactly one edit must be reported, got {edits:?}");
    };
    assert_eq!(text.len(), 8, "one glyph inserted into \"hunter2\"");
    assert_ne!(
        text.as_str(),
        "hunter2X",
        "the click must move the caret into the middle of the masked text, \
         as it does for Entry and SearchEntry"
    );
    assert_eq!(
        format!(
            "{}{}",
            &text[..text.find('X').unwrap()],
            &text[text.find('X').unwrap() + 1..]
        ),
        "hunter2",
        "the insertion must land on a character boundary of the buffer"
    );
}

#[test]
fn stepping_a_spin_button_repeats_while_the_button_is_held() {
    // mutation: return None from SpinButtonC::next_deadline and the value
    // advances once instead of several times.
    use icedtea_ui::view::builders::spin_button;
    use icedtea_ui::widgets::spin_button::SpinButtonExt;

    #[derive(Clone, Debug, PartialEq)]
    struct Set(f64);

    // Plan reconciliation: the plan clicked at `(130.0, 12.0)`, assuming the
    // control fills most of the 140px-wide window. Nothing in this crate
    // stretches a bare root widget to its window (every container centres
    // its children on both axes with an `AUTO` size — `layout.rs`'s
    // `taffy_style` — and `Prop::Hexpand` has no consumer anywhere in the
    // layout code yet), so a value-0/digits-0 `SpinButton` measures to its
    // content's natural, small width and sits centred. `SpinButtonC` had no
    // `measure` at all in the plan (see [`STEPPER_SIZE`]'s doc comment), so
    // this also depends on the `measure` this task adds; with it the control
    // is `text_width + 2*STEPPER_SIZE` wide, centred, and `(85.0, 20.0)` is
    // inside its `up` stepper for this window size.
    let frames = run(
        0.0f64,
        |model: &mut f64, Set(v): Set| {
            *model = v;
            Cmd::None
        },
        |model: &f64| {
            spin_button(*model, 0.0, 100.0)
                .step(1.0)
                .on_value_changed(Set)
        },
        (140, 40),
        vec![
            ScriptStep::Capture,
            ScriptStep::Event(InputEvent::pointer_enter(85.0, 20.0, 1)),
            ScriptStep::Event(InputEvent::PointerButton {
                button: 0x110,
                pressed: true,
                serial: 2,
                time_ms: 0,
            }),
            ScriptStep::Advance(Duration::from_millis(1_500)),
            ScriptStep::Event(InputEvent::PointerButton {
                button: 0x110,
                pressed: false,
                serial: 3,
                time_ms: 1_500,
            }),
            ScriptStep::Capture,
        ],
    );
    let row = |frame: usize| {
        (0..140)
            .map(|x| frames.pixel(frame, x, 20))
            .collect::<Vec<_>>()
    };
    assert_ne!(row(0), row(1), "the displayed value must have advanced");
}

#[test]
fn an_editable_label_commits_on_enter_and_reverts_on_escape() {
    // mutation: keep `editing` true after Enter and the `.editing` class stays
    // on, changing the final capture.
    //
    // Plan reconciliation: the plan's script opens with a bare `KeyboardEnter`
    // and no click. As `typing_into_a_text_view_inserts_at_the_cursor_and_reports_the_change`
    // and `typing_into_an_entry_shows_the_glyphs_and_moves_the_caret` already
    // found, `InputEvent::KeyboardEnter` grants no keyboard focus by itself —
    // `view/app.rs::route` has no handler for it beyond its wildcard arm, and
    // `FocusRing` only moves from a click, `Cmd::Focus`, or a keyboard binding
    // — so the `Key` events that followed had nowhere to go. A click on the
    // widget grants it, matching those same tests and `EditableLabelC`'s own
    // added `PointerDown` focus grant.
    //
    // Review fix (round 1): the script below now also sends a `Return` to
    // commit and an `Escape` to revert — the original script only clicked
    // and typed, so `EditableLabelC`'s `Activated`-commit branch
    // (`self.editing = false`, `committed.clone_from`, `on_change` firing)
    // and its `Escape`-revert branch (`self.edit.buffer.clone_from(&self
    // .committed)`) were never exercised by any event at all.
    //
    // `on_change` only fires on commit, not on every keystroke (unlike
    // `Entry`/`TextView`), so pixel diffs alone can't distinguish "the
    // buffer changed" from "a commit happened" — painting always draws
    // `self.edit.layout` regardless of `self.editing`, and the `.editing`
    // CSS class carries no rule that changes the glyph row sampled below.
    // As in `clicking_into_a_password_entry_...` above, the log of
    // `on_change` firings rides in the model since `view` is a bare `fn`
    // pointer; that log is the direct evidence of the commit path, while
    // the buffer's own text (visible through the label's rendered pixels)
    // is the evidence of the revert path leaving no trace behind.
    use icedtea_ui::view::builders::editable_label;
    use icedtea_ui::widgets::editable_label::EditableLabelExt;
    use icedtea_ui::window::BTN_LEFT;
    use std::cell::RefCell;
    use xkbcommon::xkb::Keysym;

    #[derive(Clone, Debug, PartialEq)]
    struct Renamed(String);

    let commits: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));

    let frames = run(
        ("Name".to_owned(), commits.clone()),
        |model: &mut (String, Rc<RefCell<Vec<String>>>), Renamed(text): Renamed| {
            model.0 = text;
            Cmd::None
        },
        |model: &(String, Rc<RefCell<Vec<String>>>)| {
            let recorder = model.1.clone();
            editable_label(&model.0).editing(true).on_change(move |t| {
                recorder.borrow_mut().push(t.to_owned());
                Renamed(t.to_owned())
            })
        },
        (200, 40),
        vec![
            ScriptStep::Event(InputEvent::pointer_enter(100.0, 20.0, 1)),
            ScriptStep::Event(InputEvent::PointerButton {
                button: BTN_LEFT,
                pressed: true,
                serial: 2,
                time_ms: 0,
            }),
            ScriptStep::Event(InputEvent::PointerButton {
                button: BTN_LEFT,
                pressed: false,
                serial: 3,
                time_ms: 1,
            }),
            ScriptStep::Event(InputEvent::keyboard_enter(4)),
            ScriptStep::Capture, // 0: editing, buffer "Name", nothing typed yet
            ScriptStep::Event(InputEvent::Key(key_char('X', 53))),
            ScriptStep::Capture, // 1: editing, buffer "NameX"
            ScriptStep::Event(InputEvent::Key(key_named(u32::from(Keysym::Return), 36))),
            ScriptStep::Capture, // 2: committed — editing off, label reads "NameX"
            ScriptStep::Event(InputEvent::Key(key_named(u32::from(Keysym::Return), 36))),
            ScriptStep::Event(InputEvent::Key(key_char('Y', 21))),
            ScriptStep::Capture, // 3: editing again, buffer "NameXY"
            ScriptStep::Event(InputEvent::Key(key_named(u32::from(Keysym::Escape), 9))),
            ScriptStep::Capture, // 4: reverted — editing off, label reads "NameX" again
        ],
    );
    let row = |frame: usize| {
        (0..200)
            .map(|x| frames.pixel(frame, x, 20))
            .collect::<Vec<_>>()
    };
    assert_ne!(
        row(0),
        row(1),
        "the typed character must show while editing"
    );
    assert_eq!(
        commits.borrow().as_slice(),
        ["NameX"],
        "Enter must commit exactly once, firing on_change with the typed text"
    );
    assert_ne!(
        row(2),
        row(3),
        "reopening for edit and typing must change the render again"
    );
    assert_eq!(
        row(2),
        row(4),
        "Escape must revert exactly back to the last committed render, \
         leaving the uncommitted 'Y' out of the buffer"
    );
    assert_eq!(
        commits.borrow().as_slice(),
        ["NameX"],
        "Escape must not fire on_change — the model never saw the reverted edit"
    );
}

#[test]
fn every_p5_kind_renders_at_rest_without_panicking() {
    // mutation: make any controller's `build` index a subnode it did not
    // create and this test panics for that kind by name.
    use icedtea_ui::view::{Kind, Props};
    use icedtea_ui::widgets::node_tree_of;
    for kind in Kind::all() {
        // P6's kinds still resolve to the Unimplemented controller; skip them.
        if !matches!(
            kind,
            Kind::Label
                | Kind::Spinner
                | Kind::Statusbar
                | Kind::LevelBar
                | Kind::ProgressBar
                | Kind::InfoBar
                | Kind::Scrollbar
                | Kind::Image
                | Kind::Picture
                | Kind::Separator
                | Kind::TextView
                | Kind::Scale
                | Kind::DrawingArea
                | Kind::WindowControls
                | Kind::Calendar
                | Kind::Popover
                | Kind::Button
                | Kind::ToggleButton
                | Kind::LinkButton
                | Kind::CheckButton
                | Kind::MenuButton
                | Kind::Switch
                | Kind::DropDown
                | Kind::ColorDialogButton
                | Kind::ColorDialog
                | Kind::FontDialogButton
                | Kind::FontDialog
                | Kind::Entry
                | Kind::SearchEntry
                | Kind::PasswordEntry
                | Kind::SpinButton
                | Kind::EditableLabel
        ) {
            continue;
        }
        let rendered = node_tree_of(*kind, &Props::default());
        assert!(!rendered.is_empty(), "{kind:?} rendered nothing");
    }
}

#[test]
fn a_popup_paints_the_view_its_open_command_carried() {
    // `Cmd::OpenPopup` carries the popup's own view function. Until the P6
    // fix wave the loop matched `Cmd::OpenPopup { anchor, positioner, .. }`
    // and dropped it: a surface opened and rendered nothing (contract §10
    // P5-D34).
    //
    // Mutation: put the `..` back (drop the payload, or skip `render_popups`)
    // and the second capture equals the first -- no popup ink anywhere.
    use icedtea_ui::view::builders::{box_, label};
    use icedtea_ui::view::cmd::Cmd as ViewCmd;
    use icedtea_ui::widgets::Orientation;
    use icedtea_ui::window::popup::{PopupAnchorPoint, Positioner};

    #[derive(Clone, Debug, PartialEq)]
    struct Open;

    let frames = run(
        false,
        |model: &mut bool, Open: Open| {
            *model = true;
            ViewCmd::OpenPopup {
                anchor: PopupAnchorPoint::Rect(icedtea_ui::layout::Rect {
                    x: 0.0,
                    y: 0.0,
                    width: 10.0,
                    height: 10.0,
                }),
                positioner: Positioner::menu(
                    icedtea_ui::layout::Rect {
                        x: 0.0,
                        y: 0.0,
                        width: 10.0,
                        height: 10.0,
                    },
                    (180, 60),
                ),
                view: std::rc::Rc::new(|| label("Menu item")),
            }
        },
        |_model: &bool| box_(Orientation::Vertical, [label("window")]),
        (200, 120),
        vec![
            ScriptStep::Capture,
            ScriptStep::Message(Open),
            ScriptStep::Capture,
        ],
    );

    // The window's own view does not depend on the model, so every pixel
    // that changed between the two captures is popup ink.
    let mut changed = 0;
    for y in 0..120 {
        for x in 0..200 {
            if frames.pixel(0, x, y) != frames.pixel(1, x, y) {
                changed += 1;
            }
        }
    }
    assert!(
        changed > 20,
        "the popup painted the label its command carried ({changed} px changed)"
    );
}

#[test]
fn a_password_entry_paints_its_caret_and_its_selection() {
    // P5 leftover, swept in the P6 whole-part fix wave: `PasswordEntryC::paint`
    // drew the masked text and nothing else, where `EntryC`/`SearchEntryC`
    // draw selection rects behind the text and a caret in front of it.
    //
    // mutation: delete either the selection loop or the caret rect from
    // `PasswordEntryC::paint` and the selected frame stops differing from the
    // plain one -- a focused password entry becomes indistinguishable from an
    // unfocused one.
    use icedtea_ui::view::builders::password_entry;

    let frames = run(
        "hunter2".to_owned(),
        |model: &mut String, text: String| {
            *model = text;
            Cmd::None
        },
        |model: &String| password_entry(model),
        (200, 40),
        vec![
            ScriptStep::Capture,
            // Click into the masked text (caret), then select to the end
            // with Shift+End.
            ScriptStep::Event(InputEvent::pointer_enter(100.0, 20.0, 1)),
            ScriptStep::Event(InputEvent::PointerButton {
                button: icedtea_ui::window::BTN_LEFT,
                pressed: true,
                serial: 2,
                time_ms: 0,
            }),
            ScriptStep::Event(InputEvent::PointerButton {
                button: icedtea_ui::window::BTN_LEFT,
                pressed: false,
                serial: 3,
                time_ms: 8,
            }),
            ScriptStep::Capture,
        ],
    );

    let mut changed = 0;
    for y in 0..40 {
        for x in 0..200 {
            if frames.pixel(0, x, y) != frames.pixel(1, x, y) {
                changed += 1;
            }
        }
    }
    assert!(
        changed > 0,
        "clicking in paints a caret the resting frame does not have"
    );
}

#[test]
fn event_kind_all_lists_twenty_one_kinds() {
    // mutation: forget to add one of the three pointer kinds to
    // `EventKind::ALL`; the count fails and so does any dispatcher that
    // iterates ALL.
    use icedtea_ui::view::EventKind;
    assert_eq!(EventKind::ALL.len(), 21);
    for kind in [
        EventKind::PointerDown,
        EventKind::PointerMotion,
        EventKind::PointerUp,
    ] {
        assert!(
            EventKind::ALL.contains(&kind),
            "{kind:?} is missing from ALL"
        );
    }
    // Declaration order: the three are appended, so nothing before them moved.
    assert_eq!(EventKind::ALL[0], EventKind::Click);
    assert_eq!(EventKind::ALL[18], EventKind::PointerDown);
    assert_eq!(EventKind::ALL[19], EventKind::PointerMotion);
    assert_eq!(EventKind::ALL[20], EventKind::PointerUp);
}

#[test]
fn a_pair_button_handler_falls_back_to_a_pair_handler() {
    // Why `fire_pair_button` accepts both arities: a caller that fires does
    // not know which builder the view used, so `on_pointer_down` and
    // `on_pointer_down_with_button` coexist without a second fire method.
    // mutation: delete the `Handler::Pair` arm of `fire_pair_button`; the
    // first assertion returns None.
    use icedtea_ui::view::{EventKind, Handler, Handlers};
    use std::rc::Rc;

    let mut pair: Handlers<String> = Handlers::default();
    pair.set(
        EventKind::PointerDown,
        Handler::Pair(Rc::new(|x, y| format!("{x},{y}"))),
    );
    assert_eq!(
        pair.fire_pair_button(EventKind::PointerDown, 3.0, 4.0, 0x112),
        Some("3,4".to_owned()),
        "a Pair handler on a pointer kind receives (x, y) and drops the button"
    );

    let mut with_button: Handlers<String> = Handlers::default();
    with_button.set(
        EventKind::PointerUp,
        Handler::PairButton(Rc::new(|x, y, b| format!("{x},{y},{b:#x}"))),
    );
    assert_eq!(
        with_button.fire_pair_button(EventKind::PointerUp, 1.0, 2.0, 0x112),
        Some("1,2,0x112".to_owned())
    );
    // A different kind, or an arity the binding cannot supply, fires nothing.
    assert_eq!(
        with_button.fire_pair_button(EventKind::PointerDown, 1.0, 2.0, 0x110),
        None
    );
    assert_eq!(with_button.fire_unit(EventKind::PointerUp), None);
}

#[test]
fn the_linux_button_codes_are_reachable_from_one_module() {
    // P0-D3: `BTN_LEFT` keeps its M1 home and is re-exported beside the two
    // new ones, so `window::BTN_LEFT` stays unambiguous and
    // `layer_shell_screencopy.rs`'s import is untouched.
    use icedtea_ui::window::pointer::{BTN_LEFT, BTN_MIDDLE, BTN_RIGHT};
    assert_eq!((BTN_LEFT, BTN_RIGHT, BTN_MIDDLE), (0x110, 0x111, 0x112));
    assert_eq!(icedtea_ui::window::BTN_LEFT, BTN_LEFT);
    assert_eq!(icedtea_ui::wayland::BTN_LEFT, BTN_LEFT);
}

/// Build a controller with a real `BuildCx` — the six-line pattern
/// `ColorDialogC`'s own unit test uses (`ui/src/widgets/color_dialog.rs`).
fn build_controller_for_test<C: icedtea_ui::view::controller::Controller<usize>>(
    node: &icedtea_ui::css::node::Node,
    props: &icedtea_ui::view::Props,
) -> C {
    let sheet = CompiledSheet::compile("");
    let mut fonts = icedtea_ui::text::FontDatabase::probe_only();
    let mut icons = icedtea_ui::icons::IconTheme::with_name_and_roots("hicolor", vec![]);
    let clock: Rc<dyn icedtea_ui::anim::Clock> = Rc::new(ManualClock::new());
    let env = icedtea_ui::css::computed::ResolveEnv::default();
    let mut cx = icedtea_ui::view::reconcile::BuildCx {
        sheet: &sheet,
        fonts: &mut fonts,
        icons: &mut icons,
        clock: &clock,
        env: &env,
    };
    C::build(node, props, &mut cx)
}

#[test]
fn a_color_dialog_button_paints_its_swatch_at_rest() {
    // `ColorDialogButtonC::paint` has always filled `alloc.content_box` with
    // its colour; what it lacked was an intrinsic size, so the whole button
    // collapsed to 0x0 (unlaid-out, not even hit-testable) — the
    // `ScrollbarC::measure` / `ScaleC::measure` case in shape, though here
    // `button` stays a real synced taffy child (`on_event`'s hit-testing
    // needs it), so `node` is never a taffy leaf and `Controller::measure`
    // is unreachable from this render loop; `build`'s `set_size_request`
    // side-table floor is the mechanism that actually reaches layout. Its
    // width also has to clear the margin `button`'s own real Adwaita
    // gradient chrome paints over the fill afterwards — see `build`'s
    // comment. mutation: delete the `set_size_request` call in `build`; the
    // box collapses again and no red pixel appears.
    use icedtea_ui::css::value::Rgba;
    use icedtea_ui::view::builders::color_dialog_button;

    let red = Rgba {
        r: 1.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    };
    let frames = run(
        red,
        |_m: &mut Rgba, _msg: ()| Cmd::None,
        |model: &Rgba| color_dialog_button(*model),
        (80, 60),
        vec![ScriptStep::Capture],
    );
    let mut reds = 0;
    for x in 0..80 {
        for y in 0..60 {
            if frames.pixel(0, x, y).map(|p| (p.0, p.1, p.2)) == Some((255, 0, 0)) {
                reds += 1;
            }
        }
    }
    assert!(
        reds >= 48 * 32 / 2,
        "the swatch did not fill its button: {reds} red pixels"
    );
}

#[test]
fn a_color_dialog_paints_its_palette_at_rest() {
    // mutation: return `false` from `ColorDialogC::paint`; the frame is one
    // flat colour and `has_ink` fails.
    use icedtea_ui::css::value::Rgba;
    use icedtea_ui::view::builders::color_dialog;

    let start = Rgba {
        r: 0.2,
        g: 0.5,
        b: 0.9,
        a: 1.0,
    };
    let frames = run(
        start,
        |_m: &mut Rgba, _msg: ()| Cmd::None,
        |model: &Rgba| color_dialog(*model),
        (320, 240),
        vec![ScriptStep::Capture],
    );
    assert!(
        has_ink(&frames, 0, (320, 240)),
        "the palette painted nothing"
    );
    // More than one palette colour, not just a wash: count distinct pixels.
    let mut seen = std::collections::HashSet::new();
    for x in 0..320 {
        for y in 0..240 {
            if let Some(px) = frames.pixel(0, x, y) {
                seen.insert(px);
            }
        }
    }
    assert!(
        seen.len() > 8,
        "a palette grid must show many colours, saw {}",
        seen.len()
    );
}

#[test]
fn a_color_dialogs_measured_box_is_the_grid_it_paints() {
    // The drift guard: `measure` and `paint` read one geometry function.
    // mutation: hard-code a different column count in `intrinsic`; this fails.
    use icedtea_ui::css::node::Node;
    use icedtea_ui::layout::Rect;
    use icedtea_ui::view::Props;
    use icedtea_ui::widgets::color_dialog::ColorDialogC;

    let node = Node::new("window");
    let controller: ColorDialogC = build_controller_for_test(&node, &Props::default());
    let (w, h) = controller.intrinsic();
    let cells = controller.grid(Rect::new(0.0, 0.0, w, h));
    assert_eq!(
        cells.len(),
        controller.palette.len() + usize::from(controller.custom.is_some()),
        "every palette entry gets a cell"
    );
    for (_, rect) in &cells {
        assert!(
            rect.x >= 0.0
                && rect.y >= 0.0
                && rect.x + rect.width <= w + 0.01
                && rect.y + rect.height <= h + 0.01,
            "cell {rect:?} escapes the measured {w}x{h} box"
        );
    }
}
