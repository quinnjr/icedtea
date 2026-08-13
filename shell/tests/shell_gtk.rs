//! GTK integration tests: run the real taskbar against the M4.1 headless
//! compositor harness (a live Wayland display), introspect the GTK widget tree,
//! and simulate clicks.
//!
//! The command surface is a recording mock rather than the live D-Bus proxy:
//! this keeps the test hermetic (no session-bus name to claim, no cross-test
//! races) while still asserting that a real GTK click reaches the taskbar's
//! command surface with the right window id. The `WmProxy` D-Bus path itself is
//! three thin `call_method` lines.

use std::cell::RefCell;
use std::rc::Rc;

use icedtea_contract::{Rectangle, Snapshot, WindowId, WindowInfo, WorkspaceInfo};
use icedtea_harness::Compositor;
use icedtea_shell::gtk4::{self, prelude::*, Box as GtkBox, Button, Orientation, Widget};
use icedtea_shell::taskbar::{self, TaskbarModel, WmUpdate};
use icedtea_shell::wm_client::WmCommands;

/// Point GDK at the harness compositor and init GTK once. `false` means a
/// headless environment without a usable display — the caller then skips the
/// behavioural assertions (logged), rather than failing.
fn gtk_init_against(comp: &Compositor) -> bool {
    // SAFETY: one GTK test per binary; set before any GDK use.
    unsafe {
        std::env::set_var("WAYLAND_DISPLAY", &comp.socket);
        std::env::set_var("GDK_BACKEND", "wayland");
        std::env::set_var("GSK_RENDERER", "cairo");
    }
    gtk4::init().is_ok()
}

#[derive(Default)]
struct MockWm {
    calls: RefCell<Vec<(String, u32)>>,
}
impl WmCommands for MockWm {
    fn focus_window(&self, id: u32) {
        self.calls.borrow_mut().push(("focus".into(), id));
    }
    fn close_window(&self, id: u32) {
        self.calls.borrow_mut().push(("close".into(), id));
    }
    fn set_workspace(&self, id: u32) {
        self.calls.borrow_mut().push(("workspace".into(), id));
    }
}

fn children(w: &impl IsA<Widget>) -> Vec<Widget> {
    let mut out = Vec::new();
    let mut cursor = w.first_child();
    while let Some(child) = cursor {
        cursor = child.next_sibling();
        out.push(child);
    }
    out
}

fn named(parent: &impl IsA<Widget>, name: &str) -> Option<Widget> {
    children(parent).into_iter().find(|c| c.widget_name() == name)
}

fn buttons(container: &Widget) -> Vec<Button> {
    children(container).into_iter().filter_map(|c| c.downcast::<Button>().ok()).collect()
}

fn labels(container: &Widget) -> Vec<String> {
    buttons(container)
        .iter()
        .map(|b| b.label().map(|g| g.to_string()).unwrap_or_default())
        .collect()
}

fn win(id: u32, title: &str) -> WindowInfo {
    WindowInfo {
        id: WindowId(id),
        app_id: format!("app.{id}"),
        title: title.into(),
        pid: 0,
        workspace: 0,
        geometry: Rectangle { x: 0, y: 0, width: 1, height: 1 },
        maximized: false,
        minimized: false,
        fullscreen: false,
        focused: false,
    }
}

#[test]
fn taskbar_renders_windows_and_clicks_reach_the_command_surface() {
    let comp = Compositor::spawn();
    if !gtk_init_against(&comp) {
        eprintln!("IGNORE: gtk4::init() unavailable in this environment");
        return;
    }

    let bar = GtkBox::new(Orientation::Horizontal, 6);
    let mock = Rc::new(MockWm::default());
    let wm: Rc<dyn WmCommands> = mock.clone();
    let mut model = TaskbarModel::default();

    // Seed with two mapped windows and render.
    model.apply(WmUpdate::Snapshot(Snapshot {
        seq: 1,
        windows: vec![win(1, "One"), win(2, "Two")],
        workspaces: vec![WorkspaceInfo { id: 0, name: String::new() }],
        active_workspace: 0,
    }));
    taskbar::render(&model, &bar, &wm);

    // Widget tree: exactly the two window buttons, labelled by title.
    let windows_box = named(&bar, "windows").expect("#windows box");
    assert_eq!(labels(&windows_box), vec!["One".to_string(), "Two".to_string()]);

    // A real GTK click on "One" reaches the command surface as focus_window(1).
    let one = buttons(&windows_box)
        .into_iter()
        .find(|b| b.label().map(|g| g.to_string()).as_deref() == Some("One"))
        .expect("One button");
    one.emit_clicked();
    assert!(
        mock.calls.borrow().iter().any(|(k, id)| k == "focus" && *id == 1),
        "focus_window(1) not recorded; calls = {:?}",
        mock.calls.borrow()
    );

    // A Closed update drops the button on the next render.
    model.apply(WmUpdate::Closed(1));
    taskbar::render(&model, &bar, &wm);
    let windows_box = named(&bar, "windows").expect("#windows box");
    assert_eq!(labels(&windows_box), vec!["Two".to_string()]);

    // A workspace click reaches the command surface too.
    let ws_box = named(&bar, "workspaces").expect("#workspaces box");
    buttons(&ws_box)[0].emit_clicked();
    assert!(
        mock.calls.borrow().iter().any(|(k, _)| k == "workspace"),
        "workspace switch not recorded"
    );
}
