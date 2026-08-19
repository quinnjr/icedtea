//! `icedtea-settings` — a GTK4 config editor for icedtea-wm: an Appearance
//! page and a Behavior page over a shared working-copy [`Model`], with
//! Apply/Revert wired to the on-disk redb store and (best-effort) a running
//! compositor via `org.icedtea.WM`'s `ReloadConfig`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{Application, ApplicationWindow, Box as GtkBox, Button, Label, Orientation, Stack, StackSwitcher};

use icedtea_config::default_db_path;
use icedtea_settings::model::Model;
use icedtea_settings::pages::{appearance, behavior, keybindings, workspaces, Ctx, Page};
use icedtea_settings::wm_reload::{apply_and_reload, ReloadClient, ReloadOutcome};

const APP_ID: &str = "org.icedtea.Settings";

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    let app = Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_window);
    app.run();
}

fn build_window(app: &Application) {
    let db_path = default_db_path();
    let model = Rc::new(RefCell::new(Model::load(&db_path)));
    let client = Rc::new(ReloadClient::new());

    let window = ApplicationWindow::builder()
        .application(app)
        .title("icedtea Settings")
        .default_width(480)
        .default_height(420)
        .build();

    let root = GtkBox::new(Orientation::Vertical, 0);
    window.set_child(Some(&root));

    let stack = Stack::new();
    let switcher = StackSwitcher::builder().stack(&stack).build();
    root.append(&switcher);
    root.append(&stack);
    stack.set_vexpand(true);

    let footer = GtkBox::new(Orientation::Horizontal, 8);
    footer.set_margin_top(8);
    footer.set_margin_bottom(8);
    footer.set_margin_start(8);
    footer.set_margin_end(8);
    let status = Label::new(None);
    status.set_hexpand(true);
    status.set_halign(gtk4::Align::Start);
    let revert_button = Button::with_label("Revert");
    let apply_button = Button::with_label("Apply");
    footer.append(&status);
    footer.append(&revert_button);
    footer.append(&apply_button);
    root.append(&footer);

    // Recomputes the footer from `model.is_dirty()` -- called after every
    // widget write-back (via `Ctx::mark_dirty`) and after Apply/Revert, so
    // it's the single source of truth for the dirty indicator rather than a
    // flag that only ever moves one direction.
    let update_footer: Rc<dyn Fn()> = {
        let model = model.clone();
        let status = status.clone();
        let revert_button = revert_button.clone();
        let apply_button = apply_button.clone();
        Rc::new(move || {
            let dirty = model.borrow().is_dirty();
            status.set_label(if dirty { "Unsaved changes" } else { "" });
            revert_button.set_sensitive(dirty);
            apply_button.set_sensitive(dirty);
        })
    };

    let ctx = Ctx {
        model: model.clone(),
        window: window.clone(),
        on_dirty: update_footer.clone(),
        populating: Rc::new(Cell::new(false)),
    };

    let appearance_page: Page = appearance::build(ctx.clone());
    let behavior_page: Page = behavior::build(ctx.clone());

    // Adding/removing a workspace on the Workspaces page changes the set of
    // generated `workspace:N`/`move_to_workspace:N` rows the Keybindings
    // page shows, so the Workspaces page needs to be able to trigger a
    // refresh there. The Keybindings page doesn't exist yet at the point
    // `workspaces::build` needs the callback, so it's routed through this
    // slot and filled in right after `keybindings::build` runs.
    let keybindings_page_slot: Rc<RefCell<Option<Page>>> = Rc::new(RefCell::new(None));
    let on_workspaces_changed: Rc<dyn Fn()> = {
        let slot = keybindings_page_slot.clone();
        Rc::new(move || {
            if let Some(page) = slot.borrow().as_ref() {
                page.refresh();
            }
        })
    };

    let workspaces_page: Page = workspaces::build(ctx.clone(), on_workspaces_changed);
    let keybindings_page: Page = keybindings::build(ctx.clone());
    let keybindings_root = keybindings_page.root.clone();
    *keybindings_page_slot.borrow_mut() = Some(keybindings_page);

    stack.add_titled(&appearance_page.root, Some("appearance"), "Appearance");
    stack.add_titled(&behavior_page.root, Some("behavior"), "Behavior");
    stack.add_titled(&workspaces_page.root, Some("workspaces"), "Workspaces");
    stack.add_titled(&keybindings_root, Some("keybindings"), "Keybindings");

    update_footer();

    {
        let model = model.clone();
        let client = client.clone();
        let db_path = db_path.clone();
        let status = status.clone();
        let update_footer = update_footer.clone();
        apply_button.connect_clicked(move |_| {
            let cfg = model.borrow().working.clone();
            match apply_and_reload(&cfg, &db_path, &client) {
                Ok(ReloadOutcome::Reloaded) => {
                    let mut model = model.borrow_mut();
                    model.saved = model.working.clone();
                    drop(model);
                    update_footer();
                    status.set_label("Applied");
                }
                Ok(ReloadOutcome::CompositorAbsent) => {
                    let mut model = model.borrow_mut();
                    model.saved = model.working.clone();
                    drop(model);
                    update_footer();
                    status.set_label("Saved; will apply when the compositor starts");
                }
                Err(err) => {
                    status.set_label(&format!("Failed to save: {err}"));
                }
            }
        });
    }

    {
        let model = model.clone();
        let db_path = db_path.clone();
        let update_footer = update_footer.clone();
        let keybindings_page_slot = keybindings_page_slot.clone();
        revert_button.connect_clicked(move |_| {
            model.borrow_mut().revert(&db_path);
            appearance_page.refresh();
            behavior_page.refresh();
            workspaces_page.refresh();
            if let Some(page) = keybindings_page_slot.borrow().as_ref() {
                page.refresh();
            }
            update_footer();
        });
    }

    window.present();
}
