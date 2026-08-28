//! `window-probe` -- the client `ui/tests/window_events.rs` drives.
//!
//! Builds a GTK-shaped node tree by hand (P5/P6 own the widgets that will
//! produce the same trees), maps it as an `xdg_toplevel`, and appends one line
//! per interesting event to `$ICEDTEA_PROBE_REPORT` so a test can assert on
//! what the client actually saw, not only on what it painted.
//!
//! Environment:
//! - `ICEDTEA_UI_THEME`   -- a path to the complete theme to use.
//! - `ICEDTEA_PROBE_MODE` -- `entry` (default) or `menu`.
//! - `ICEDTEA_PROBE_REPORT` -- the report file to append to.

use std::io::Write;
use std::path::PathBuf;

use icedtea_ui::app::{ThemeSource, compile_theme};
use icedtea_ui::css::node::Node;
use icedtea_ui::text::FontDatabase;
use icedtea_ui::window::focus::{
    Binding, FOCUSABLE_CLASS, FocusCause, FocusRing, navigate, window_binding,
};
use icedtea_ui::window::popup::{PopupAnchorPoint, Positioner};
use icedtea_ui::window::{InputEvent, Role, SurfaceSpec, Window};

fn report(line: &str) {
    let Ok(path) = std::env::var("ICEDTEA_PROBE_REPORT") else {
        return;
    };
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{line}");
        let _ = file.flush();
    }
}

fn main() {
    tracing_subscriber::fmt().with_env_filter("warn").init();
    let theme = std::env::var("ICEDTEA_UI_THEME").map_or(ThemeSource::Bundled, |v| {
        ThemeSource::File(PathBuf::from(v))
    });
    let mode = std::env::var("ICEDTEA_PROBE_MODE").unwrap_or_else(|_| "entry".into());
    let sheet = compile_theme(&theme);
    let fonts = FontDatabase::new();

    let mut window = Window::open(
        SurfaceSpec {
            role: Role::Toplevel,
            size: (400, 200),
            title: "window probe".into(),
            app_id: "org.icedtea.WindowProbe".into(),
        },
        sheet,
        fonts,
    )
    .expect("the probe could not open its window");

    // window > box > (entry > text), (menubutton > label)
    let root = window.root().clone();
    let container = Node::new("box");
    root.append_child(&container);
    let entry = Node::with_classes("entry", &[FOCUSABLE_CLASS]);
    entry.set_id(Some("entry"));
    let text = Node::new("text");
    entry.append_child(&text);
    container.append_child(&entry);
    let menubutton = Node::with_classes("menubutton", &[FOCUSABLE_CLASS]);
    menubutton.set_id(Some("menubutton"));
    let label = Node::new("label");
    menubutton.append_child(&label);
    container.append_child(&menubutton);
    window.set_node_text(&text, "");
    window.set_node_text(&label, "Menu");

    // `FocusRing` also has an inherent `default(&self) -> Option<Node>`
    // method (the window's *default-activated* node), which shadows
    // `Default::default()` under `Type::method()` call syntax -- the trait
    // method needs the fully qualified form to be reached at all.
    let mut focus = <FocusRing as Default>::default();
    let mut typed = String::new();
    let mut popup = None;
    loop {
        if window.render().is_err() {
            break;
        }
        let timeout = window.next_deadline();
        let Ok(events) = window.pump(timeout) else {
            break;
        };
        for event in events {
            match event {
                InputEvent::Configure { size, states } => {
                    report(&format!("configure {} {} {states:?}", size.0, size.1));
                }
                InputEvent::Close => return,
                InputEvent::KeyboardEnter { .. } => {
                    if focus.focus().is_none() {
                        focus.set_focus(Some(&entry), FocusCause::Programmatic);
                    }
                    report("keyboard-enter");
                }
                InputEvent::Key(key) => {
                    focus.note_key(&key);
                    match window_binding(&key) {
                        Some(Binding::Move(dir)) => {
                            let next =
                                navigate(&root, window.layout(), focus.focus().as_ref(), dir)
                                    .or_else(|| navigate(&root, window.layout(), None, dir));
                            focus.set_focus(next.as_ref(), FocusCause::Keyboard);
                            report(&format!(
                                "focus {} visible={}",
                                next.and_then(|n| n.id()).map_or_else(
                                    || "none".to_string(),
                                    |id| id.as_str().to_string()
                                ),
                                focus.focus_visible()
                            ));
                        }
                        Some(Binding::Dismiss) => {
                            if let Some(key) = popup.take() {
                                window.close_popup(key);
                            }
                        }
                        Some(_) | None => {
                            if key.pressed
                                && let Some(utf8) = &key.utf8
                                && focus.focus().is_some_and(|n| n.ptr_eq(&entry))
                            {
                                typed.push_str(utf8);
                                window.set_node_text(&text, &typed);
                                report(&format!("typed {typed}"));
                            }
                        }
                    }
                    window.mark_dirty(&root);
                }
                InputEvent::PointerButton { pressed: true, .. } if mode == "menu" => {
                    match window.open_popup(
                        PopupAnchorPoint::Node(menubutton.clone()),
                        Positioner::menu(icedtea_ui::layout::Rect::zero(), (160, 120)),
                    ) {
                        Ok(key) => {
                            popup = Some(key);
                            report("popup-open");
                        }
                        Err(err) => report(&format!("popup-error {err}")),
                    }
                }
                InputEvent::PopupDone(key) => {
                    window.close_popup(key);
                    popup = None;
                    report("popup-done");
                }
                _ => {}
            }
        }
        if window.is_closed() {
            return;
        }
    }
}
