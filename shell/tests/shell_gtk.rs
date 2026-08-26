//! GTK integration tests: run the real taskbar against the M4.1 headless
//! compositor harness (a live Wayland display), introspect the GTK widget tree,
//! and simulate clicks.
//!
//! The command surface is a recording mock rather than the live D-Bus proxy:
//! this keeps the test hermetic (no session-bus name to claim, no cross-test
//! races) while still asserting that a real GTK click reaches the taskbar's
//! command surface with the right window id. The `CompositorProxy` D-Bus path itself is
//! three thin `call_method` lines.

use std::cell::RefCell;
use std::rc::Rc;

use icedtea_contract::{
    ClipEntry, ClipKind, Rectangle, Snapshot, WindowId, WindowInfo, WorkspaceInfo,
};
use icedtea_harness::Compositor;
use icedtea_shell::clip_client::ClipCommands;
use icedtea_shell::clipboard::{self, ClipUpdate, ClipboardModel};
use icedtea_shell::compositor_client::CompositorCommands;
use icedtea_shell::gtk4::{self, Box as GtkBox, Button, ListBox, Orientation, Widget, prelude::*};
use icedtea_shell::taskbar::{self, CompositorUpdate, TaskbarModel};

/// Point GDK at the harness compositor and init GTK once. `false` means GTK
/// could not come up — which is a FAILURE by default (the harness provides a
/// display, so a failure means something is wrong), not a silent pass. An
/// operator on a genuinely display-less CI opts out explicitly with
/// `ICEDTEA_ALLOW_NO_GTK`.
fn gtk_init_against(comp: &Compositor) -> bool {
    // SAFETY: one GTK test per binary; set before any GDK use.
    unsafe {
        std::env::set_var("WAYLAND_DISPLAY", &comp.socket);
        std::env::set_var("GDK_BACKEND", "wayland");
        std::env::set_var("GSK_RENDERER", "cairo");
    }
    gtk4::init().is_ok()
}

/// Init GTK or decide what to do about it: return `true` to proceed, `false` to
/// skip (only when opted out), or panic (the default failure).
fn require_gtk(comp: &Compositor) -> bool {
    if gtk_init_against(comp) {
        return true;
    }
    if std::env::var_os("ICEDTEA_ALLOW_NO_GTK").is_some() {
        eprintln!("SKIP: gtk4::init() unavailable and ICEDTEA_ALLOW_NO_GTK is set");
        return false;
    }
    panic!(
        "gtk4::init() failed against the harness compositor; \
         set ICEDTEA_ALLOW_NO_GTK=1 to skip on a display-less CI"
    );
}

#[derive(Default)]
struct MockWm {
    calls: RefCell<Vec<(String, u32)>>,
}
impl CompositorCommands for MockWm {
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

#[derive(Default)]
struct MockClip {
    calls: RefCell<Vec<(String, u64)>>,
}
impl ClipCommands for MockClip {
    fn activate(&self, id: u64) {
        self.calls.borrow_mut().push(("activate".into(), id));
    }
    fn pin(&self, id: u64, _on: bool) {
        self.calls.borrow_mut().push(("pin".into(), id));
    }
    fn remove(&self, id: u64) {
        self.calls.borrow_mut().push(("remove".into(), id));
    }
    fn clear(&self) {
        self.calls.borrow_mut().push(("clear".into(), 0));
    }
}

fn clip_entry(id: u64, preview: &str) -> ClipEntry {
    ClipEntry {
        id,
        kind: ClipKind::Text,
        preview: preview.into(),
        mime: "text/plain".into(),
        pinned: false,
        source_app: None,
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
    children(parent)
        .into_iter()
        .find(|c| c.widget_name() == name)
}

fn buttons(container: &Widget) -> Vec<Button> {
    children(container)
        .into_iter()
        .filter_map(|c| c.downcast::<Button>().ok())
        .collect()
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
        geometry: Rectangle {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        },
        maximized: false,
        minimized: false,
        fullscreen: false,
        focused: false,
        attention: false,
    }
}

#[test]
fn taskbar_renders_windows_and_clicks_reach_the_command_surface() {
    let comp = Compositor::spawn();
    if !require_gtk(&comp) {
        return;
    }

    let bar = GtkBox::new(Orientation::Horizontal, 6);
    let mock = Rc::new(MockWm::default());
    let wm: Rc<dyn CompositorCommands> = mock.clone();
    let mut model = TaskbarModel::default();

    // Seed with two mapped windows and render.
    model.apply(CompositorUpdate::Snapshot(Snapshot {
        seq: 1,
        windows: vec![win(1, "One"), win(2, "Two")],
        workspaces: vec![WorkspaceInfo {
            id: 0,
            name: String::new(),
        }],
        active_workspace: 0,
    }));
    taskbar::render(&model, &bar, &wm);

    // Widget tree: exactly the two window buttons, labelled by title.
    let windows_box = named(&bar, "windows").expect("#windows box");
    assert_eq!(
        labels(&windows_box),
        vec!["One".to_string(), "Two".to_string()]
    );

    // A real GTK click on "One" reaches the command surface as focus_window(1).
    let one = buttons(&windows_box)
        .into_iter()
        .find(|b| b.label().map(|g| g.to_string()).as_deref() == Some("One"))
        .expect("One button");
    one.emit_clicked();
    assert!(
        mock.calls
            .borrow()
            .iter()
            .any(|(k, id)| k == "focus" && *id == 1),
        "focus_window(1) not recorded; calls = {:?}",
        mock.calls.borrow()
    );

    // A Closed update drops the button on the next render.
    model.apply(CompositorUpdate::Closed(1));
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

    // --- clipboard popover ---
    let clip_mock = Rc::new(MockClip::default());
    let clip: Rc<dyn ClipCommands> = clip_mock.clone();
    let clip_model = Rc::new(RefCell::new(ClipboardModel::default()));
    let list = ListBox::new();
    clipboard::connect_activation(&list, clip.clone(), clip_model.clone());

    clip_model.borrow_mut().apply(ClipUpdate::History(vec![
        clip_entry(10, "copied text"),
        clip_entry(11, "second entry"),
    ]));
    clipboard::render(&clip_model.borrow(), &list, &clip);

    // The #history list has one row per entry.
    let rows = children(&list);
    assert_eq!(rows.len(), 2, "expected two history rows");

    // Activating the first row re-pastes it (activate(10)).
    let row = list.row_at_index(0).expect("row 0");
    list.emit_by_name::<()>("row-activated", &[&row]);
    assert!(
        clip_mock
            .calls
            .borrow()
            .iter()
            .any(|(k, id)| k == "activate" && *id == 10),
        "activate(10) not recorded; calls = {:?}",
        clip_mock.calls.borrow()
    );

    // A history update replaces the rows.
    clip_model
        .borrow_mut()
        .apply(ClipUpdate::History(vec![clip_entry(12, "only")]));
    clipboard::render(&clip_model.borrow(), &list, &clip);
    assert_eq!(
        children(&list).len(),
        1,
        "history update did not replace rows"
    );
}
