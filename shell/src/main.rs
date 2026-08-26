//! `icedtea-shell` — a GTK4 layer-shell panel: a window/workspace taskbar
//! driven by `org.icedtea.Compositor`, and a clipboard popover driven by
//! `org.icedtea.Clipboard`. All widgets live on the GTK main thread; D-Bus runs
//! on a worker, bridged by a glib channel.

use std::cell::RefCell;
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{
    Application, ApplicationWindow, Box as GtkBox, Button, CssProvider, ListBox, MenuButton,
    Orientation, Popover,
};
use gtk4_layer_shell::{Edge, Layer, LayerShell};

use icedtea_shell::bridge;
use icedtea_shell::clip_client::{self, ClipCommands, ClipProxy};
use icedtea_shell::clipboard::{self, ClipboardModel};
use icedtea_shell::compositor_client::{self, CompositorCommands, CompositorProxy};
use icedtea_shell::taskbar::{self, TaskbarModel};

const APP_ID: &str = "org.icedtea.Shell";

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    let app = Application::builder().application_id(APP_ID).build();
    app.connect_startup(|_| load_css());
    app.connect_activate(build_panel);
    app.run();
}

fn load_css() {
    let provider = CssProvider::new();
    provider.load_from_data(include_str!("../style.css"));
    if let Some(display) = gtk4::gdk::Display::default() {
        gtk4::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

fn build_panel(app: &Application) {
    let window = ApplicationWindow::new(app);

    // Layer-shell: a bottom bar reserving its own height.
    window.init_layer_shell();
    window.set_layer(Layer::Top);
    window.auto_exclusive_zone_enable();
    // Anchored to the top edge (plus left/right for full width). Top rather
    // than bottom because a bottom bar lands below the visible area on displays
    // whose viewport is shorter than the reported output (e.g. a VM console).
    for edge in [Edge::Left, Edge::Right, Edge::Top] {
        window.set_anchor(edge, true);
    }

    let bar = GtkBox::new(Orientation::Horizontal, 6);
    bar.set_widget_name("bar");
    // A layer surface anchored left/right/top takes its width from the
    // compositor but its height from content; force a definite, visible height
    // and initial size so it commits a real buffer (a 0-height surface never
    // renders).
    bar.set_size_request(-1, 28);
    window.set_child(Some(&bar));
    window.set_default_size(800, 28);

    // The taskbar lives in its own box: `taskbar::render` clears and rebuilds
    // its container, so it must not own the whole bar (the clipboard button is
    // a sibling that has to survive).
    let taskbar_box = GtkBox::new(Orientation::Horizontal, 6);
    taskbar_box.set_hexpand(true);
    bar.append(&taskbar_box);

    wire_taskbar(&taskbar_box);
    wire_clipboard(&bar);

    window.present();
}

fn wire_taskbar(container: &GtkBox) {
    let wm: Rc<dyn CompositorCommands> = match CompositorProxy::new() {
        Ok(wm) => Rc::new(wm),
        Err(err) => {
            tracing::error!(%err, "no session bus; taskbar disabled");
            return;
        }
    };
    let model = Rc::new(RefCell::new(TaskbarModel::default()));
    let tx = {
        let container = container.clone();
        let wm = wm.clone();
        let model = model.clone();
        bridge::channel(move |update| {
            model.borrow_mut().apply(update);
            taskbar::render(&model.borrow(), &container, &wm);
        })
    };
    compositor_client::spawn(tx);
}

fn wire_clipboard(bar: &GtkBox) {
    let clip: Rc<dyn ClipCommands> = match ClipProxy::new() {
        Ok(c) => Rc::new(c),
        Err(err) => {
            tracing::error!(%err, "no session bus; clipboard popover disabled");
            return;
        }
    };
    let model = Rc::new(RefCell::new(ClipboardModel::default()));

    let list = ListBox::new();
    clipboard::connect_activation(&list, clip.clone(), model.clone());

    let content = GtkBox::new(Orientation::Vertical, 4);
    content.append(&list);
    let clear = Button::with_label("Clear");
    {
        let clip = clip.clone();
        clear.connect_clicked(move |_| clip.clear());
    }
    content.append(&clear);

    let popover = Popover::new();
    popover.set_child(Some(&content));
    let menu = MenuButton::new();
    menu.set_label("clip");
    menu.set_popover(Some(&popover));
    bar.append(&menu);

    let tx = {
        let list = list.clone();
        let clip = clip.clone();
        let model = model.clone();
        bridge::channel(move |update| {
            model.borrow_mut().apply(update);
            clipboard::render(&model.borrow(), &list, &clip);
        })
    };
    clip_client::spawn(tx);
}
