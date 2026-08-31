//! Per-widget rest-state and interaction tests (contract §9, P5's gate).
//!
//! Everything here runs through `App::run_offscreen` on a `ManualClock` — no
//! compositor, no Wayland connection. P8's `gallery_gate.rs` and
//! `interaction_gate.rs` are the compositor-backed versions of the same idea.

use std::rc::Rc;
use std::time::Duration;

use icedtea_ui::BUNDLED_ADWAITA_LIGHT;
use icedtea_ui::anim::ManualClock;
use icedtea_ui::css::cascade::CompiledSheet;
use icedtea_ui::view::app::{App, Frames, ScriptStep};
use icedtea_ui::view::{Cmd, View};
use icedtea_ui::window::InputEvent;

/// Run one view offscreen against Adwaita light and return the captured frames.
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
