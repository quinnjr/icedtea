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
use icedtea_ui::window::InputEvent;
use icedtea_ui::window::popup::{PopupAnchorPoint, PopupKey, Positioner};

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
