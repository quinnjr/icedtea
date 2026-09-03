//! Contract §4.8's counter app: the whole reactive stack — builders,
//! reconciler, controllers, dispatch, restyle, layout, paint — driven
//! offscreen by a script, asserted on pixels.

use std::rc::Rc;

use icedtea_ui::anim::ManualClock;
use icedtea_ui::css::cascade::CompiledSheet;
use icedtea_ui::icons::IconTheme;
use icedtea_ui::text::FontDatabase;
use icedtea_ui::view::builders::widget;
use icedtea_ui::view::{App, Cmd, Frames, Kind, PropName, ScriptStep, View};
use icedtea_ui::window::{BTN_LEFT, InputEvent};

const W: u32 = 160;
const H: u32 = 60;

/// A sheet with no gradients and no rounding, so a pixel assertion reads
/// exactly one declared colour.
const SHEET: &str = "
    window { background-color: #ffffff; }
    box { background-color: #ffffff; padding: 0; }
    button { background-color: #3584e4; min-width: 40px; min-height: 40px;
             border: 0; padding: 0; color: #ffffff; }
    label { color: #000000; min-width: 40px; min-height: 40px; }
";

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Inc,
    Dec,
}

#[derive(Debug)]
struct Model {
    n: i32,
}

fn update(model: &mut Model, msg: Msg) -> Cmd<Msg> {
    match msg {
        Msg::Inc => model.n += 1,
        Msg::Dec => model.n -= 1,
    }
    Cmd::None
}

fn view(model: &Model) -> View<Msg> {
    widget::<Msg>(Kind::Box)
        .key("root")
        .child(
            widget::<Msg>(Kind::Button)
                .key("dec")
                .prop(PropName::Label, "-")
                .on_click(Msg::Dec),
        )
        .child(
            widget::<Msg>(Kind::Label)
                .key("value")
                .prop(PropName::Label, model.n.to_string()),
        )
        .child(
            widget::<Msg>(Kind::Button)
                .key("inc")
                .prop(PropName::Label, "+")
                .on_click(Msg::Inc),
        )
}

fn app() -> App<Model, Msg> {
    App::new(Model { n: 0 }, update, view)
        .with_sheet(CompiledSheet::compile(SHEET))
        .with_fonts(FontDatabase::new())
        .with_icons(IconTheme::with_name_and_roots("hicolor", vec![]))
}

fn run(script: Vec<ScriptStep<Msg>>) -> Frames {
    app()
        .run_offscreen((W, H), Rc::new(ManualClock::new()), script)
        .expect("the counter app runs offscreen")
}

/// Click at `(x, y)`: enter, press, release.
fn click(x: f64, y: f64) -> Vec<ScriptStep<Msg>> {
    vec![
        ScriptStep::Event(InputEvent::pointer_enter(x, y, 1)),
        ScriptStep::Event(InputEvent::PointerMotion { x, y, time_ms: 0 }),
        ScriptStep::Event(InputEvent::PointerButton {
            button: BTN_LEFT,
            pressed: true,
            serial: 2,
            time_ms: 1,
        }),
        ScriptStep::Event(InputEvent::PointerButton {
            button: BTN_LEFT,
            pressed: false,
            serial: 3,
            time_ms: 2,
        }),
    ]
}

/// How many pixels in the label column are dark — the glyph coverage the
/// contract's "pixel assertions on the label" means.
fn dark_pixels(frames: &Frames, frame: usize) -> usize {
    let mut count = 0;
    for y in 0..H {
        for x in 40..80 {
            if let Some((r, g, b, a)) = frames.pixel(frame, x, y)
                && a > 0x80
                && r < 0x60
                && g < 0x60
                && b < 0x60
            {
                count += 1;
            }
        }
    }
    count
}

#[test]
fn the_counter_renders_its_three_children_in_order() {
    let frames = run(vec![ScriptStep::Capture]);
    assert_eq!(frames.len(), 1);
    assert_eq!(frames.size(), (W, H));

    // The two buttons are Adwaita accent blue; the label between them is not.
    let (r, g, b, a) = frames.pixel(0, 20, 20).expect("inside the '-' button");
    assert_eq!((r, g, b, a), (0x35, 0x84, 0xE4, 0xFF));
    let (r, g, b, a) = frames.pixel(0, 100, 20).expect("inside the '+' button");
    assert_eq!((r, g, b, a), (0x35, 0x84, 0xE4, 0xFF));
    let (r, g, b, _) = frames.pixel(0, 60, 2).expect("above the label glyph");
    assert_eq!(
        (r, g, b),
        (0xFF, 0xFF, 0xFF),
        "the label sits on the box background"
    );
}

#[test]
fn clicking_plus_repaints_the_label_with_the_new_model() {
    let mut script = vec![ScriptStep::Capture];
    script.extend(click(100.0, 20.0));
    script.push(ScriptStep::Capture);
    let frames = run(script);
    assert_eq!(frames.len(), 2);

    let before = dark_pixels(&frames, 0);
    let after = dark_pixels(&frames, 1);
    assert!(before > 0, "the '0' label never rendered any glyph pixels");
    assert!(after > 0, "the '1' label never rendered any glyph pixels");
    assert_ne!(
        before, after,
        "'0' and '1' produced identical glyph coverage, so the label did not repaint"
    );
}

#[test]
fn clicking_minus_and_plus_returns_to_the_starting_pixels() {
    let mut script = vec![ScriptStep::Capture];
    script.extend(click(20.0, 20.0));
    script.extend(click(100.0, 20.0));
    script.push(ScriptStep::Capture);
    let frames = run(script);
    assert_eq!(frames.len(), 2);
    for y in 0..H {
        for x in 0..W {
            assert_eq!(
                frames.pixel(0, x, y),
                frames.pixel(1, x, y),
                "pixel ({x}, {y}) differs after -1 then +1 returned the model to 0"
            );
        }
    }
}

#[test]
fn a_click_outside_every_child_changes_nothing() {
    let mut script = vec![ScriptStep::Capture];
    script.extend(click(159.0, 59.0));
    script.push(ScriptStep::Capture);
    let frames = run(script);
    for y in 0..H {
        for x in 0..W {
            assert_eq!(frames.pixel(0, x, y), frames.pixel(1, x, y));
        }
    }
}

/// M5-D4: an `update` may capture. Both M5 apps need it — settings holds
/// worker senders, shell holds `Rc<dyn CompositorCommands>` so a test can
/// swap the mock in.
///
/// mutation: change `App::new`'s `update` parameter back to
/// `fn(&mut M, Msg) -> Cmd<Msg>`; this stops compiling ("expected fn
/// pointer, found closure").
#[test]
fn a_closure_capturing_state_drives_the_loop() {
    use std::cell::RefCell;

    use icedtea_ui::BUNDLED_ADWAITA_LIGHT;
    use icedtea_ui::widgets::label::label;

    let seen: Rc<RefCell<Vec<u32>>> = Rc::new(RefCell::new(Vec::new()));
    let recorder = Rc::clone(&seen);
    let clock = Rc::new(ManualClock::new());
    let frames = App::new(
        0_u32,
        move |model: &mut u32, msg: u32| {
            *model += msg;
            recorder.borrow_mut().push(*model);
            Cmd::None
        },
        |model: &u32| label(&model.to_string()),
    )
    .with_sheet(CompiledSheet::compile(BUNDLED_ADWAITA_LIGHT))
    .run_offscreen(
        (120, 40),
        clock,
        vec![
            ScriptStep::Message(2),
            ScriptStep::Message(3),
            ScriptStep::Capture,
        ],
    )
    .expect("the offscreen app runs");
    assert_eq!(frames.len(), 1);
    assert_eq!(*seen.borrow(), vec![2, 5], "the closure kept its capture");
}

/// The widening must not break the `fn`-item call sites: the gallery, this
/// file's own counter and every widget test pass plain `fn`s.
///
/// mutation: make `App::new` take `Box<dyn FnMut…>` directly instead of
/// `impl FnMut…`; every existing call site stops compiling.
#[test]
fn an_fn_item_still_coerces_into_app_new() {
    fn update(model: &mut u32, msg: u32) -> Cmd<u32> {
        *model += msg;
        Cmd::None
    }
    fn view(model: &u32) -> View<u32> {
        icedtea_ui::widgets::label::label(&model.to_string())
    }
    let app = App::new(7_u32, update, view);
    assert_eq!(*app.model(), 7);
}
