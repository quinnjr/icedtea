//! P5's toolkit enablement: popup keys reach the model, input reaches popup
//! surfaces, and an open popup tracks the model (contract §6 P5-D1/D2/D4).
//!
//! Offscreen throughout: `run_offscreen` builds `Cmd::OpenPopup`'s payload
//! into a real retained tree and composites it, so every property here is
//! observable with no compositor.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use icedtea_ui::anim::ManualClock;
use icedtea_ui::css::cascade::CompiledSheet;
use icedtea_ui::layout::Rect;
use icedtea_ui::text::FontDatabase;
use icedtea_ui::view::builders::{box_, button};
use icedtea_ui::view::{App, Cmd, PopupEvent, ScriptStep};
use icedtea_ui::widgets::Orientation;
use icedtea_ui::window::popup::{PopupAnchorPoint, PopupKey, Positioner};
use icedtea_ui::window::{BTN_LEFT, InputEvent, SurfaceTarget};

#[derive(Clone, Debug, PartialEq)]
enum Msg {
    Open,
    Opened(PopupKey),
    Dismissed(PopupKey),
    // Only Task 1's scenario is implemented here; later tasks in this SDD
    // exercise row-picking through this same file's `Model`/`Msg`.
    #[allow(dead_code, reason = "used by later tasks sharing this test file")]
    Picked(u32),
}

/// The model every test in this file drives.
#[derive(Default)]
struct Model {
    open: Option<PopupKey>,
    #[allow(dead_code, reason = "used by later tasks sharing this test file")]
    picked: Vec<u32>,
    #[allow(dead_code, reason = "used by later tasks sharing this test file")]
    rows: Vec<u32>,
}

fn sheet() -> CompiledSheet {
    CompiledSheet::compile(
        "window { background-color: #ffffff; } \
         box { background-color: #ffffff; } \
         button { min-width: 40px; min-height: 20px; background-color: #808080; }",
    )
}

#[test]
fn an_app_learns_the_key_of_the_popup_it_opened_and_of_its_dismissal() {
    let log: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = log.clone();
    let clock = Rc::new(ManualClock::new());

    let app = App::new(
        Model::default(),
        move |m: &mut Model, msg: Msg| match msg {
            Msg::Open => Cmd::OpenPopup {
                anchor: PopupAnchorPoint::Rect(Rect::new(0.0, 0.0, 40.0, 20.0)),
                positioner: Positioner::menu(Rect::new(0.0, 0.0, 40.0, 20.0), (80, 40)),
                view: Rc::new(|| box_(Orientation::Vertical, [button::<Msg>("row")])),
            },
            Msg::Opened(key) => {
                m.open = Some(key);
                sink.borrow_mut().push(format!("opened {}", key.raw()));
                Cmd::None
            }
            Msg::Dismissed(key) => {
                if m.open == Some(key) {
                    m.open = None;
                }
                sink.borrow_mut().push(format!("dismissed {}", key.raw()));
                Cmd::None
            }
            Msg::Picked(_) => Cmd::None,
        },
        |_m: &Model| box_(Orientation::Horizontal, [button::<Msg>("clip")]),
    )
    .with_sheet(sheet())
    .with_fonts(FontDatabase::probe_only())
    .on_popup(|ev| match ev {
        PopupEvent::Opened(key) => Some(Msg::Opened(key)),
        PopupEvent::Dismissed(key) => Some(Msg::Dismissed(key)),
        #[allow(unreachable_patterns, reason = "PopupEvent is #[non_exhaustive]")]
        _ => None,
    });

    app.run_offscreen(
        (200, 60),
        clock,
        vec![
            ScriptStep::Message(Msg::Open),
            ScriptStep::Advance(Duration::from_millis(16)),
            ScriptStep::Event(InputEvent::PopupDone(PopupKey::from_raw(0))),
            ScriptStep::Advance(Duration::from_millis(16)),
        ],
    )
    .expect("offscreen run");

    assert_eq!(
        log.borrow().as_slice(),
        ["opened 0".to_string(), "dismissed 0".to_string()],
        "on_popup must report both halves of a popup's life, in order"
    );
}

#[test]
fn a_click_inside_a_popup_reaches_the_popups_own_handler() {
    let clock = Rc::new(ManualClock::new());
    let picked: Rc<RefCell<Vec<u32>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = picked.clone();

    let app = App::new(
        Model::default(),
        move |m: &mut Model, msg: Msg| match msg {
            Msg::Open => Cmd::OpenPopup {
                anchor: PopupAnchorPoint::Rect(Rect::new(0.0, 0.0, 40.0, 20.0)),
                positioner: Positioner::menu(Rect::new(0.0, 0.0, 40.0, 20.0), (80, 40)),
                view: Rc::new(|| {
                    box_(
                        Orientation::Vertical,
                        [button::<Msg>("row").on_click(Msg::Picked(7))],
                    )
                }),
            },
            Msg::Opened(key) => {
                m.open = Some(key);
                Cmd::None
            }
            Msg::Dismissed(_) => {
                m.open = None;
                Cmd::None
            }
            Msg::Picked(id) => {
                sink.borrow_mut().push(id);
                Cmd::None
            }
        },
        |_m: &Model| box_(Orientation::Horizontal, [button::<Msg>("clip")]),
    )
    .with_sheet(sheet())
    .with_fonts(FontDatabase::probe_only())
    .on_popup(|ev| match ev {
        PopupEvent::Opened(key) => Some(Msg::Opened(key)),
        PopupEvent::Dismissed(key) => Some(Msg::Dismissed(key)),
        #[allow(unreachable_patterns, reason = "PopupEvent is #[non_exhaustive]")]
        _ => None,
    });

    let popup = SurfaceTarget::Popup(PopupKey::from_raw(0));
    app.run_offscreen(
        (200, 60),
        clock,
        vec![
            ScriptStep::Message(Msg::Open),
            ScriptStep::Advance(Duration::from_millis(16)),
            // Enter the *popup* surface: its coordinates are its own, so the
            // row's centre is a few pixels in, not wherever the window's tree
            // happens to have a button.
            ScriptStep::Event(InputEvent::PointerEnter {
                x: 20.0,
                y: 10.0,
                serial: 1,
                target: popup,
            }),
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
            ScriptStep::Advance(Duration::from_millis(16)),
        ],
    )
    .expect("offscreen run");

    assert_eq!(
        picked.borrow().as_slice(),
        [7],
        "a click on a popup surface must be hit-tested against that popup's tree"
    );
}

#[test]
fn a_click_on_the_window_still_reaches_the_window_after_a_popup_opened() {
    let clock = Rc::new(ManualClock::new());
    let picked: Rc<RefCell<Vec<u32>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = picked.clone();

    let app = App::new(
        Model::default(),
        move |m: &mut Model, msg: Msg| match msg {
            Msg::Open => Cmd::OpenPopup {
                anchor: PopupAnchorPoint::Rect(Rect::new(0.0, 0.0, 40.0, 20.0)),
                positioner: Positioner::menu(Rect::new(0.0, 0.0, 40.0, 20.0), (80, 40)),
                view: Rc::new(|| {
                    box_(
                        Orientation::Vertical,
                        [button::<Msg>("row").on_click(Msg::Picked(7))],
                    )
                }),
            },
            Msg::Opened(key) => {
                m.open = Some(key);
                Cmd::None
            }
            Msg::Dismissed(_) => {
                m.open = None;
                Cmd::None
            }
            Msg::Picked(id) => {
                sink.borrow_mut().push(id);
                Cmd::None
            }
        },
        |_m: &Model| {
            box_(
                Orientation::Horizontal,
                [button::<Msg>("clip").on_click(Msg::Picked(1))],
            )
        },
    )
    .with_sheet(sheet())
    .with_fonts(FontDatabase::probe_only())
    .on_popup(|ev| match ev {
        PopupEvent::Opened(key) => Some(Msg::Opened(key)),
        PopupEvent::Dismissed(key) => Some(Msg::Dismissed(key)),
        #[allow(unreachable_patterns, reason = "PopupEvent is #[non_exhaustive]")]
        _ => None,
    });

    let popup = SurfaceTarget::Popup(PopupKey::from_raw(0));
    app.run_offscreen(
        (200, 60),
        clock,
        vec![
            ScriptStep::Message(Msg::Open),
            ScriptStep::Advance(Duration::from_millis(16)),
            ScriptStep::Event(InputEvent::PointerEnter {
                x: 20.0,
                y: 10.0,
                serial: 1,
                target: popup,
            }),
            ScriptStep::Event(InputEvent::PointerLeave),
            // Back on the window: the enter carries `SurfaceTarget::Window`,
            // which is what puts routing back on the window's tree. The
            // window's box centres its lone child (`layout::container_style`,
            // "GTK's box centres a child that asked for nothing"), so the
            // "clip" button sits at (80, 20)-(120, 40) in this 200x60 canvas
            // -- its centre, not the popup row's local (20, 10), is what a
            // click on the window itself must land on.
            ScriptStep::Event(InputEvent::pointer_enter(100.0, 30.0, 4)),
            ScriptStep::Event(InputEvent::PointerButton {
                button: BTN_LEFT,
                pressed: true,
                serial: 5,
                time_ms: 2,
            }),
            ScriptStep::Event(InputEvent::PointerButton {
                button: BTN_LEFT,
                pressed: false,
                serial: 6,
                time_ms: 3,
            }),
            ScriptStep::Advance(Duration::from_millis(16)),
        ],
    )
    .expect("offscreen run");

    assert_eq!(
        picked.borrow().as_slice(),
        [1],
        "leaving a popup must put routing back on the window's own tree"
    );
}
