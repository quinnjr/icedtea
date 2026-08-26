//! The pure clipboard-history model + its popover rendering. The model is the
//! `Vec<ClipEntry>` the daemon serves; render diffs it into a `ListBox`.

use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{Align, Box as GtkBox, Button, Label, ListBox, ListBoxRow, Orientation};
use icedtea_contract::ClipEntry;

use crate::clip_client::ClipCommands;

#[derive(Default, Debug)]
pub struct ClipboardModel {
    pub entries: Vec<ClipEntry>,
}

#[derive(Debug)]
pub enum ClipUpdate {
    History(Vec<ClipEntry>),
}

impl ClipboardModel {
    pub fn apply(&mut self, u: ClipUpdate) {
        match u {
            ClipUpdate::History(v) => self.entries = v,
        }
    }
}

/// Rebuild the popover `list` from `model`: one row per entry (a preview label
/// plus a pin toggle), row activation re-pastes it. `list` is named `#history`
/// so tests can address it.
pub fn render(model: &ClipboardModel, list: &ListBox, clip: &Rc<dyn ClipCommands>) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
    list.set_widget_name("history");

    for entry in &model.entries {
        let row = ListBoxRow::new();
        row.set_widget_name("history-row");
        let hbox = GtkBox::new(Orientation::Horizontal, 6);

        let label = Label::new(Some(&entry.preview));
        label.set_halign(Align::Start);
        label.set_hexpand(true);
        label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        hbox.append(&label);

        let pin = Button::with_label(if entry.pinned { "unpin" } else { "pin" });
        let clip_pin = clip.clone();
        let (pin_id, pin_on) = (entry.id, !entry.pinned);
        pin.connect_clicked(move |_| clip_pin.pin(pin_id, pin_on));
        hbox.append(&pin);

        row.set_child(Some(&hbox));
        list.append(&row);
    }
}

/// Wire `list` row activation to re-paste (`activate`). Called once at build.
pub fn connect_activation(
    list: &ListBox,
    clip: Rc<dyn ClipCommands>,
    model: Rc<std::cell::RefCell<ClipboardModel>>,
) {
    list.connect_row_activated(move |_, row| {
        let idx = row.index();
        if idx >= 0
            && let Some(entry) = model.borrow().entries.get(idx as usize)
        {
            clip.activate(entry.id);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use icedtea_contract::ClipKind;

    fn entry(id: u64, preview: &str) -> ClipEntry {
        ClipEntry {
            id,
            kind: ClipKind::Text,
            preview: preview.into(),
            mime: "text/plain".into(),
            pinned: false,
            source_app: None,
        }
    }

    #[test]
    fn history_update_replaces_entries() {
        let mut m = ClipboardModel::default();
        m.apply(ClipUpdate::History(vec![entry(1, "a"), entry(2, "b")]));
        assert_eq!(m.entries.len(), 2);
        m.apply(ClipUpdate::History(vec![entry(3, "c")]));
        assert_eq!(m.entries.iter().map(|e| e.id).collect::<Vec<_>>(), vec![3]);
    }
}
