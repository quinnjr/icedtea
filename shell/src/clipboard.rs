//! The pure clipboard-history model + its popover rendering. The model is the
//! `Vec<ClipEntry>` the daemon serves; `panel::popover_body` renders it.

use icedtea_contract::ClipEntry;

#[derive(Default, Debug, Clone)]
pub struct ClipboardModel {
    pub entries: Vec<ClipEntry>,
}

/// One clipboard-history update. `Clone` for the same `Arc::unwrap_or_clone`
/// reason `CompositorUpdate` is.
#[derive(Debug, Clone)]
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
    fn a_clip_update_can_be_cloned_out_of_an_arc() {
        let shared = std::sync::Arc::new(ClipUpdate::History(vec![entry(1, "a")]));
        let mut m = ClipboardModel::default();
        m.apply(std::sync::Arc::unwrap_or_clone(shared.clone()));
        m.apply(std::sync::Arc::unwrap_or_clone(shared));
        assert_eq!(m.entries.len(), 1);
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
