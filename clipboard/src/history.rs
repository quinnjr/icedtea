//! The pure clipboard history model: bounded, deduping, with a pinned block
//! exempt from eviction. No I/O — the payload bytes live here, keyed by the
//! monotonic id that crosses D-Bus in [`ClipEntry`].

use icedtea_contract::{ClipEntry, ClipKind};

/// Whether a mutation actually moved the model, so the service only signals
/// `history_changed` on a real change.
#[derive(Debug, PartialEq, Eq)]
pub enum Change {
    Changed,
    Unchanged,
}

struct Item {
    entry: ClipEntry,
    mime: String,
    bytes: Vec<u8>,
}

/// Pinned entries first, then unpinned in MRU order. `max` bounds only the
/// unpinned block.
pub struct History {
    items: Vec<Item>,
    max: usize,
    next_id: u64,
}

impl History {
    pub fn new(max: usize) -> Self {
        History {
            items: Vec::new(),
            max,
            next_id: 1,
        }
    }

    /// Record a fresh capture. A byte-identical repeat of the newest unpinned
    /// entry is a no-op (the dedupe every clipboard manager does). Newest
    /// unpinned entries past `max` are evicted.
    pub fn push(
        &mut self,
        kind: ClipKind,
        mime: String,
        bytes: &[u8],
        source_app: Option<String>,
    ) -> Change {
        if let Some(head) = self.items.iter().find(|i| !i.entry.pinned)
            && head.mime == mime
            && head.bytes == bytes
        {
            return Change::Unchanged;
        }
        let id = self.next_id;
        self.next_id += 1;
        let entry = ClipEntry {
            id,
            kind,
            preview: make_preview(kind, bytes),
            mime: mime.clone(),
            pinned: false,
            source_app,
        };
        // Insert at the front of the unpinned block (just after the pinned one).
        let insert_at = self.items.iter().take_while(|i| i.entry.pinned).count();
        self.items.insert(
            insert_at,
            Item {
                entry,
                mime,
                bytes: bytes.to_vec(),
            },
        );
        self.evict();
        Change::Changed
    }

    fn evict(&mut self) {
        let mut unpinned = 0;
        self.items.retain(|i| {
            if i.entry.pinned {
                return true;
            }
            unpinned += 1;
            unpinned <= self.max
        });
    }

    pub fn pin(&mut self, id: u64, on: bool) -> Change {
        let Some(pos) = self.items.iter().position(|i| i.entry.id == id) else {
            return Change::Unchanged;
        };
        if self.items[pos].entry.pinned == on {
            return Change::Unchanged;
        }
        self.items[pos].entry.pinned = on;
        // Restore the invariant: pinned block first, each block keeping its
        // relative order (`sort_by_key` is stable).
        self.items.sort_by_key(|i| !i.entry.pinned);
        Change::Changed
    }

    pub fn remove(&mut self, id: u64) -> Change {
        let before = self.items.len();
        self.items.retain(|i| i.entry.id != id);
        if self.items.len() == before {
            Change::Unchanged
        } else {
            Change::Changed
        }
    }

    /// Drop every unpinned entry; pins survive.
    pub fn clear(&mut self) -> Change {
        let before = self.items.len();
        self.items.retain(|i| i.entry.pinned);
        if self.items.len() == before {
            Change::Unchanged
        } else {
            Change::Changed
        }
    }

    pub fn snapshot(&self) -> Vec<ClipEntry> {
        self.items.iter().map(|i| i.entry.clone()).collect()
    }

    /// The stored `(mime, payload)` for a re-paste, or `None` if `id` is gone.
    pub fn bytes_for(&self, id: u64) -> Option<(String, Vec<u8>)> {
        self.items
            .iter()
            .find(|i| i.entry.id == id)
            .map(|i| (i.mime.clone(), i.bytes.clone()))
    }
}

fn make_preview(kind: ClipKind, bytes: &[u8]) -> String {
    match kind {
        ClipKind::Image => "[image]".to_string(),
        _ => {
            let s = String::from_utf8_lossy(bytes);
            let t: String = s.chars().take(80).collect();
            t.replace('\n', " ").trim().to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h() -> History {
        History::new(3)
    }

    #[test]
    fn push_dedupes_the_current_head() {
        let mut h = h();
        assert_eq!(
            h.push(ClipKind::Text, "text/plain".into(), b"a", None),
            Change::Changed
        );
        assert_eq!(
            h.push(ClipKind::Text, "text/plain".into(), b"a", None),
            Change::Unchanged
        );
        assert_eq!(h.snapshot().len(), 1);
    }

    #[test]
    fn push_evicts_oldest_unpinned_over_max() {
        let mut h = h();
        for b in [b"a".as_slice(), b"b", b"c", b"d"] {
            h.push(ClipKind::Text, "text/plain".into(), b, None);
        }
        let snap = h.snapshot();
        assert_eq!(snap.len(), 3, "max=3 unpinned");
        assert_eq!(snap[0].preview, "d", "MRU first");
        assert!(snap.iter().all(|e| e.preview != "a"), "oldest evicted");
    }

    #[test]
    fn pin_survives_clear_and_exempts_from_eviction() {
        let mut h = h();
        h.push(ClipKind::Text, "text/plain".into(), b"keep", None);
        let id = h.snapshot()[0].id;
        assert_eq!(h.pin(id, true), Change::Changed);
        for b in [b"x".as_slice(), b"y", b"z", b"w"] {
            h.push(ClipKind::Text, "text/plain".into(), b, None);
        }
        assert!(
            h.snapshot().iter().any(|e| e.id == id && e.pinned),
            "pinned kept past eviction"
        );
        h.clear();
        assert_eq!(
            h.snapshot().iter().filter(|e| e.id == id).count(),
            1,
            "clear keeps pinned"
        );
    }

    #[test]
    fn bytes_for_returns_the_stored_payload() {
        let mut h = h();
        h.push(
            ClipKind::Text,
            "text/plain;charset=utf-8".into(),
            b"payload",
            None,
        );
        let id = h.snapshot()[0].id;
        assert_eq!(
            h.bytes_for(id),
            Some(("text/plain;charset=utf-8".into(), b"payload".to_vec()))
        );
    }
}
