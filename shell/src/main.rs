//! `icedtea-shell` — a GTK4 layer-shell panel: a window/workspace taskbar
//! driven by `org.icedtea.WM`, and a clipboard popover driven by
//! `org.icedtea.Clipboard`. All widgets live on the GTK main thread; D-Bus runs
//! on a worker, bridged by a glib channel.

use std::cell::RefCell;
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{Application, ApplicationWindow, Box as GtkBox, CssProvider, Orientation};
use gtk4_layer_shell::{Edge, Layer, LayerShell};

use icedtea_shell::taskbar::{self, TaskbarModel};
use icedtea_shell::wm_client::{self, WmCommands, WmProxy};
use icedtea_shell::bridge;

const APP_ID: &str = "org.icedtea.Shell";

fn main() {
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
    for edge in [Edge::Left, Edge::Right, Edge::Bottom] {
        window.set_anchor(edge, true);
    }

    let bar = GtkBox::new(Orientation::Horizontal, 6);
    bar.set_widget_name("bar");
    window.set_child(Some(&bar));

    // The taskbar: a shared model + a WM command proxy, re-rendered on every
    // update the worker forwards to this (the GTK main) thread.
    let wm: Rc<dyn WmCommands> = match WmProxy::new() {
        Ok(wm) => Rc::new(wm),
        Err(err) => {
            tracing::error!(%err, "no session bus; taskbar commands disabled");
            window.present();
            return;
        }
    };
    let model = Rc::new(RefCell::new(TaskbarModel::default()));

    let tx = {
        let bar = bar.clone();
        let wm = wm.clone();
        let model = model.clone();
        bridge::channel(move |update| {
            model.borrow_mut().apply(update);
            taskbar::render(&model.borrow(), &bar, &wm);
        })
    };
    wm_client::spawn(tx);

    window.present();
}
