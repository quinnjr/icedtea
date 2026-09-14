//! M6 toolkit drag-and-drop acceptance: a real draggable list item dropped
//! onto a real drop target, driven through the live `App` dispatch.
//!
//! Geometry is never hard-coded: each test probes the app first and aims at
//! the allocations the layout actually produced. What IS pinned is the
//! message order (source events before target events, `Drop` before
//! `DragEnd`) and the pixel proof that `drop-target-active` paints exactly
//! while the drag hovers.

use std::cell::RefCell;
use std::rc::Rc;

use icedtea_ui::anim::ManualClock;
use icedtea_ui::css::cascade::CompiledSheet;
use icedtea_ui::dnd::{DragSeat, SeatDragRequest, SeatDragResult};
use icedtea_ui::icons::IconTheme;
use icedtea_ui::text::FontDatabase;
use icedtea_ui::view::Instance;
use icedtea_ui::view::builders::{box_, widget};
use icedtea_ui::view::{App, Cmd, Key, Kind, ScriptStep, View};
use icedtea_ui::widgets::headless::Headless;
use icedtea_ui::widgets::types::Orientation;
use icedtea_ui::window::layer::BTN_LEFT;
use icedtea_ui::window::{InputEvent, SurfaceTarget};

/// The whole contract, observed: source notifications, target hover, the
/// negotiated payload, and the plain click a release can still be.
#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Dropped(String),
    DragStarted,
    DragEnded(bool),
    Entered,
    Moved,
    Left,
    Clicked,
}

/// The draggable chip (a list item — deviation M6-D2's GenericC pair) and
/// the drop target, side by side in one surface.
fn dnd_view(_model: &()) -> View<Msg> {
    box_::<Msg>(
        Orientation::Vertical,
        [
            widget::<Msg>(Kind::ListBoxRow)
                .key("chip")
                .prop(icedtea_ui::view::PropName::Label, "chip")
                .drag_source("chip-payload")
                .on_drag_start(Msg::DragStarted)
                .on_drag_end(Msg::DragEnded)
                .on_click(Msg::Clicked),
            widget::<Msg>(Kind::ListBoxRow)
                .key("target")
                .prop(icedtea_ui::view::PropName::Label, "target")
                .drop_accept("text/plain")
                .on_drag_enter(Msg::Entered)
                .on_drag_motion(|_, _| Msg::Moved)
                .on_drag_leave(Msg::Left)
                .on_drop(|text: &str| Msg::Dropped(text.to_owned())),
        ],
    )
    .key("root")
}

fn sheet() -> CompiledSheet {
    CompiledSheet::compile(
        "window { background-color: #ffffff; }
         box { background-color: #ffffff; }
         row { background-color: #ffffff; min-width: 200px; min-height: 24px; }
         button { background-color: #ffffff; min-width: 200px; min-height: 24px; }
         entry { background-color: #ffffff; min-width: 200px; min-height: 24px; }
         .drop-target-active { background-color: #ff0000; }",
    )
}

fn dnd_app(log: Rc<RefCell<Vec<String>>>) -> App<(), Msg> {
    App::new(
        (),
        move |_model: &mut (), msg: Msg| -> Cmd<Msg> {
            log.borrow_mut().push(match msg {
                Msg::Dropped(text) => format!("dropped:{text}"),
                Msg::DragStarted => "started".to_owned(),
                Msg::DragEnded(dropped) => format!("ended:{dropped}"),
                Msg::Entered => "entered".to_owned(),
                Msg::Moved => "moved".to_owned(),
                Msg::Left => "left".to_owned(),
                Msg::Clicked => "clicked".to_owned(),
            });
            Cmd::None
        },
        dnd_view,
    )
    .with_sheet(sheet())
    .with_fonts(FontDatabase::probe_only())
    .with_icons(IconTheme::with_name_and_roots("hicolor", Vec::new()))
}

fn find<'a>(instances: &'a [Instance<Msg>], name: &str) -> &'a Instance<Msg> {
    fn walk<'a>(instance: &'a Instance<Msg>, name: &str) -> Option<&'a Instance<Msg>> {
        if matches!(&instance.key, Some(Key::Name(key)) if key.as_ref() == name) {
            return Some(instance);
        }
        instance.children.iter().find_map(|child| walk(child, name))
    }
    walk(&instances[0], name).unwrap_or_else(|| panic!("no instance keyed {name}"))
}

/// The dedicated kinds' pair: a `Button` source and an `Entry` target (the
/// controllers the v1 generic-only shape kept out of DnD).
fn dedicated_dnd_view(_model: &()) -> View<Msg> {
    box_::<Msg>(
        Orientation::Vertical,
        [
            widget::<Msg>(Kind::Button)
                .key("chip")
                .prop(icedtea_ui::view::PropName::Label, "chip")
                .drag_source("chip-payload")
                .on_drag_start(Msg::DragStarted)
                .on_drag_end(Msg::DragEnded)
                .on_click(Msg::Clicked),
            widget::<Msg>(Kind::Entry)
                .key("target")
                .prop(icedtea_ui::view::PropName::Text, "target")
                .drop_accept("text/plain")
                .on_drag_enter(Msg::Entered)
                .on_drag_motion(|_, _| Msg::Moved)
                .on_drag_leave(Msg::Left)
                .on_drop(|text: &str| Msg::Dropped(text.to_owned())),
        ],
    )
    .key("root")
}

fn dedicated_dnd_app(log: Rc<RefCell<Vec<String>>>) -> App<(), Msg> {
    App::new(
        (),
        move |_model: &mut (), msg: Msg| -> Cmd<Msg> {
            log.borrow_mut().push(match msg {
                Msg::Dropped(text) => format!("dropped:{text}"),
                Msg::DragStarted => "started".to_owned(),
                Msg::DragEnded(dropped) => format!("ended:{dropped}"),
                Msg::Entered => "entered".to_owned(),
                Msg::Moved => "moved".to_owned(),
                Msg::Left => "left".to_owned(),
                Msg::Clicked => "clicked".to_owned(),
            });
            Cmd::None
        },
        dedicated_dnd_view,
    )
    .with_sheet(sheet())
    .with_fonts(FontDatabase::probe_only())
    .with_icons(IconTheme::with_name_and_roots("hicolor", Vec::new()))
}

/// Centers of the chip and the target, from the layout itself.
fn geometry() -> ((f64, f64), (f64, f64)) {
    geometry_of(dnd_view)
}

/// [`geometry`], over any view function with the same key names.
fn geometry_of(view: fn(&()) -> View<Msg>) -> ((f64, f64), (f64, f64)) {
    let clock: Rc<ManualClock> = Rc::new(ManualClock::new());
    let probe = App::new((), |_: &mut (), _: Msg| Cmd::None, view)
        .with_sheet(sheet())
        .with_fonts(FontDatabase::probe_only())
        .with_icons(IconTheme::with_name_and_roots("hicolor", Vec::new()))
        .probe(
            (240, 120),
            sheet(),
            FontDatabase::probe_only(),
            IconTheme::with_name_and_roots("hicolor", Vec::new()),
            clock,
        )
        .expect("probe lays out");
    let center = |name: &str| {
        let alloc = probe
            .allocation(&find(probe.instances(), name).node)
            .unwrap_or_else(|| panic!("{name} has an allocation"));
        assert!(
            alloc.border_box.width > 0.0 && alloc.border_box.height > 0.0,
            "{name} measured empty"
        );
        (
            f64::from(alloc.border_box.x + alloc.border_box.width / 2.0),
            f64::from(alloc.border_box.y + alloc.border_box.height / 2.0),
        )
    };
    let (chip, target) = (center("chip"), center("target"));
    // The gesture needs room: press-to-threshold inside the chip, and two
    // distinct rows. A degenerate layout would make every test below
    // vacuous, so pin the precondition instead of assuming it.
    assert!(
        (chip.0 - target.0).hypot(chip.1 - target.1)
            > f64::from(icedtea_ui::dnd::DRAG_THRESHOLD_PX) * 2.0,
        "chip {chip:?} and target {target:?} overlap the drag threshold"
    );
    (chip, target)
}

fn enter(x: f64, y: f64, serial: u32) -> ScriptStep<Msg> {
    ScriptStep::Event(InputEvent::PointerEnter {
        x,
        y,
        serial,
        target: SurfaceTarget::Window,
    })
}

fn motion(x: f64, y: f64) -> ScriptStep<Msg> {
    ScriptStep::Event(InputEvent::PointerMotion { x, y, time_ms: 0 })
}

fn button(pressed: bool, serial: u32) -> ScriptStep<Msg> {
    ScriptStep::Event(InputEvent::PointerButton {
        button: BTN_LEFT,
        pressed,
        serial,
        time_ms: 0,
    })
}

/// A scripted seat whose request log a test can read after the run: the
/// `DragSeat` half of §7, driven without a live compositor.
struct RecordingSeat {
    answer: SeatDragResult,
    log: Rc<RefCell<Vec<SeatDragRequest>>>,
}

impl RecordingSeat {
    fn new(answer: SeatDragResult) -> (Self, Rc<RefCell<Vec<SeatDragRequest>>>) {
        let log: Rc<RefCell<Vec<SeatDragRequest>>> = Rc::default();
        (
            RecordingSeat {
                answer,
                log: Rc::clone(&log),
            },
            log,
        )
    }
}

impl DragSeat for RecordingSeat {
    fn offer_drag(&mut self, request: &SeatDragRequest) -> SeatDragResult {
        self.log.borrow_mut().push(request.clone());
        self.answer
    }
}

#[test]
fn press_drag_drop_delivers_the_payload_and_highlights_the_target() {
    let (chip, target) = geometry();
    let log: Rc<RefCell<Vec<String>>> = Rc::default();
    // Step well past the threshold but still inside the 200px-wide chip, so
    // the drag provably begins before the target is reached.
    let past_threshold = (chip.0 + 15.0, chip.1);
    let clock: Rc<ManualClock> = Rc::new(ManualClock::new());
    let frames = dnd_app(Rc::clone(&log))
        .run_offscreen(
            (240, 120),
            Rc::clone(&clock),
            vec![
                ScriptStep::Capture,
                enter(chip.0, chip.1, 1),
                button(true, 2),
                motion(past_threshold.0, past_threshold.1),
                // The target pixel while hovered mid-drag.
                motion(target.0, target.1),
                ScriptStep::Capture,
                button(false, 3),
                ScriptStep::Capture,
            ],
        )
        .expect("the offscreen loop runs");

    // Payload, highlight lifecycle, and ordering — and the disarmed click:
    // a drag is never also a click.
    assert_eq!(
        log.borrow().as_slice(),
        &[
            "started",
            "entered",
            "moved",
            "dropped:chip-payload",
            "ended:true"
        ],
    );

    // Pixel proof: the target paints the drop class exactly while hovered.
    let sample = |frame: usize| frames.pixel(frame, target.0 as u32, target.1 as u32);
    let rest = sample(0).expect("the target pixel exists");
    let hovered = sample(1).expect("the hovered frame exists");
    assert_ne!(
        hovered, rest,
        "hovering the target mid-drag painted nothing"
    );
    let (r, g, b, _) = hovered;
    assert!(
        r > 200 && g < 100 && b < 100,
        "the hovered target is not the drop-target red: {hovered:?}"
    );
    // After the drop the class is gone.
    assert_eq!(
        sample(2),
        Some(rest),
        "the drop-target highlight stuck around after the drop"
    );
}

#[test]
fn release_outside_any_target_cancels_cleanly() {
    let (chip, _) = geometry();
    let log: Rc<RefCell<Vec<String>>> = Rc::default();
    // Bottom-right corner: below the centred rows, root background only.
    let outside = (232.0, 112.0);
    let clock: Rc<ManualClock> = Rc::new(ManualClock::new());
    dnd_app(Rc::clone(&log))
        .run_offscreen(
            (240, 120),
            Rc::clone(&clock),
            vec![
                enter(chip.0, chip.1, 1),
                button(true, 2),
                motion(chip.0 + 15.0, chip.1),
                motion(outside.0, outside.1),
                button(false, 3),
                // No stuck drag state: a plain click still clicks.
                enter(chip.0, chip.1, 4),
                button(true, 5),
                button(false, 6),
            ],
        )
        .expect("the offscreen loop runs");

    assert_eq!(
        log.borrow().as_slice(),
        &["started", "ended:false", "clicked"],
        "release-outside must cancel (never drop) and leave clicks working"
    );
}

#[test]
fn escape_mid_drag_cancels_and_clears_the_highlight() {
    let (chip, target) = geometry();
    let log: Rc<RefCell<Vec<String>>> = Rc::default();
    let clock: Rc<ManualClock> = Rc::new(ManualClock::new());
    dnd_app(Rc::clone(&log))
        .run_offscreen(
            (240, 120),
            Rc::clone(&clock),
            vec![
                enter(chip.0, chip.1, 1),
                button(true, 2),
                motion(chip.0 + 15.0, chip.1),
                motion(target.0, target.1),
                ScriptStep::Event(InputEvent::Key(Headless::key("Escape"))),
                // The held button releases after the cancel: nothing further.
                button(false, 3),
                // And clicks still work afterwards.
                enter(chip.0, chip.1, 4),
                button(true, 5),
                button(false, 6),
            ],
        )
        .expect("the offscreen loop runs");

    assert_eq!(
        log.borrow().as_slice(),
        &[
            "started",
            "entered",
            "moved",
            "left",
            "ended:false",
            "clicked"
        ],
        "Escape must leave the target and end the drag un-dropped"
    );
}

#[test]
fn a_dedicated_button_source_drops_onto_a_dedicated_entry_target() {
    let (chip, target) = geometry_of(dedicated_dnd_view);
    let log: Rc<RefCell<Vec<String>>> = Rc::default();
    let clock: Rc<ManualClock> = Rc::new(ManualClock::new());
    dedicated_dnd_app(Rc::clone(&log))
        .run_offscreen(
            (240, 120),
            Rc::clone(&clock),
            vec![
                enter(chip.0, chip.1, 1),
                button(true, 2),
                motion(chip.0 + 15.0, chip.1),
                motion(target.0, target.1),
                button(false, 3),
            ],
        )
        .expect("the offscreen loop runs");

    assert_eq!(
        log.borrow().as_slice(),
        &[
            "started",
            "entered",
            "moved",
            "dropped:chip-payload",
            "ended:true"
        ],
        "a Button source and an Entry target must take part in DnD \
         (v1's GenericC-only shape kept them out)"
    );
}

#[test]
fn a_dedicated_source_drag_released_over_itself_is_not_a_click() {
    let (chip, _) = geometry_of(dedicated_dnd_view);
    let log: Rc<RefCell<Vec<String>>> = Rc::default();
    let clock: Rc<ManualClock> = Rc::new(ManualClock::new());
    dedicated_dnd_app(Rc::clone(&log))
        .run_offscreen(
            (240, 120),
            Rc::clone(&clock),
            vec![
                enter(chip.0, chip.1, 1),
                button(true, 2),
                // Past the threshold, then back onto the source: the release
                // is inside the Button, so without the drag's press-latch
                // clear it would also click.
                motion(chip.0 + 15.0, chip.1),
                motion(chip.0, chip.1),
                button(false, 3),
            ],
        )
        .expect("the offscreen loop runs");

    assert_eq!(
        log.borrow().as_slice(),
        &["started", "ended:false"],
        "a drag is never also a click, even when released over its source"
    );
}

#[test]
fn a_refusing_seat_sees_the_request_and_leaves_the_drag_internal() {
    let (chip, target) = geometry();
    let log: Rc<RefCell<Vec<String>>> = Rc::default();
    let (seat, seat_log) = RecordingSeat::new(SeatDragResult::Refused);
    let clock: Rc<ManualClock> = Rc::new(ManualClock::new());
    dnd_app(Rc::clone(&log))
        .with_drag_seat(Box::new(seat))
        .run_offscreen(
            (240, 120),
            Rc::clone(&clock),
            vec![
                enter(chip.0, chip.1, 1),
                button(true, 2),
                motion(chip.0 + 15.0, chip.1),
                motion(target.0, target.1),
                button(false, 3),
            ],
        )
        .expect("the offscreen loop runs");

    // A refused offer must not change the toolkit half at all.
    assert_eq!(
        log.borrow().as_slice(),
        &[
            "started",
            "entered",
            "moved",
            "dropped:chip-payload",
            "ended:true"
        ],
    );
    let requests = seat_log.borrow();
    assert_eq!(requests.len(), 1, "exactly one offer per drag");
    // The arming press serial (the second scripted press) and the offered
    // payload, which the real seat validates against M4.2's grant table.
    assert_eq!(requests[0].serial, 2);
    assert_eq!(requests[0].mimes(), vec!["text/plain"]);
    assert_eq!(
        requests[0].payload.data_for(&["text/plain"]),
        Some(("text/plain", b"chip-payload".as_slice()))
    );
}

#[test]
fn an_accepting_seat_still_delivers_an_in_surface_drop() {
    let (chip, target) = geometry();
    let log: Rc<RefCell<Vec<String>>> = Rc::default();
    let (seat, seat_log) = RecordingSeat::new(SeatDragResult::Accepted);
    let clock: Rc<ManualClock> = Rc::new(ManualClock::new());
    dnd_app(Rc::clone(&log))
        .with_drag_seat(Box::new(seat))
        .run_offscreen(
            (240, 120),
            Rc::clone(&clock),
            vec![
                enter(chip.0, chip.1, 1),
                button(true, 2),
                motion(chip.0 + 15.0, chip.1),
                motion(target.0, target.1),
                button(false, 3),
            ],
        )
        .expect("the offscreen loop runs");

    // The `wl_data_device` destination half (enter/motion/drop back into
    // this client) is not wired, so an accepted seat offer must NOT suppress
    // the toolkit's own hit-testing: the in-surface target is still reached
    // and the `Drop` still fires. (`Accepted` means "request issued", not
    // "a destination will be routed to".)
    assert_eq!(
        log.borrow().as_slice(),
        &[
            "started",
            "entered",
            "moved",
            "dropped:chip-payload",
            "ended:true"
        ],
        "an accepting seat must not kill an in-surface drop"
    );
    assert_eq!(seat_log.borrow().len(), 1, "the offer is still issued");
}

#[test]
fn a_sub_threshold_press_is_a_click_not_a_drag() {
    let (chip, _) = geometry();
    let log: Rc<RefCell<Vec<String>>> = Rc::default();
    let clock: Rc<ManualClock> = Rc::new(ManualClock::new());
    dnd_app(Rc::clone(&log))
        .run_offscreen(
            (240, 120),
            Rc::clone(&clock),
            vec![
                enter(chip.0, chip.1, 1),
                button(true, 2),
                motion(chip.0 + 2.0, chip.1 + 2.0),
                button(false, 3),
            ],
        )
        .expect("the offscreen loop runs");

    assert_eq!(
        log.borrow().as_slice(),
        &["clicked"],
        "a press that never crosses the threshold must click, never drag"
    );
}
