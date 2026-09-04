//! Shared plumbing for the Appearance and Behavior pages: the context every
//! widget's write-back handler closes over, and the [`Page`] handle
//! `main.rs` uses to mount a page's root widget and repopulate it on Revert.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk4::ApplicationWindow;

use crate::model::Model;

pub mod appearance;
pub mod behavior;
pub mod displays;
pub mod displays_canvas;
pub mod keybindings;
pub mod workspaces;

/// The five settings pages, in switcher order.
///
/// One identity for three consumers that must not disagree: the model's
/// `page` field, the `StackSwitcher`'s selection index, and the `Stack`'s
/// `visible-child-name`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageId {
    Appearance,
    Behavior,
    Workspaces,
    Keybindings,
    Displays,
}

impl PageId {
    /// Every page, in the order the switcher shows them.
    pub const ALL: [PageId; 5] = [
        PageId::Appearance,
        PageId::Behavior,
        PageId::Workspaces,
        PageId::Keybindings,
        PageId::Displays,
    ];

    /// The `stack_page` name this page is selected by.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            PageId::Appearance => "appearance",
            PageId::Behavior => "behavior",
            PageId::Workspaces => "workspaces",
            PageId::Keybindings => "keybindings",
            PageId::Displays => "displays",
        }
    }

    /// The switcher button's label.
    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            PageId::Appearance => "Appearance",
            PageId::Behavior => "Behavior",
            PageId::Workspaces => "Workspaces",
            PageId::Keybindings => "Keybindings",
            PageId::Displays => "Displays",
        }
    }

    /// The page a `StackSwitcher` selection index names. An index past the
    /// end clamps to the last page: the index arrives from a widget, and the
    /// cross-cutting rule is that untrusted input never panics.
    #[must_use]
    pub fn from_index(i: usize) -> PageId {
        PageId::ALL[i.min(PageId::ALL.len() - 1)]
    }

    /// This page's position in [`PageId::ALL`].
    #[must_use]
    pub fn index(self) -> usize {
        PageId::ALL
            .iter()
            .position(|p| *p == self)
            .unwrap_or_default()
    }
}

/// The page list a `stack_switcher` takes.
///
/// A function rather than the contract's `PAGES` constant (deviation P1-D3):
/// `StackPageInfo` holds `Rc<str>`, which cannot appear in a `const`.
#[must_use]
pub fn page_infos() -> std::rc::Rc<[icedtea_ui::widgets::StackPageInfo]> {
    PageId::ALL
        .iter()
        .map(|page| icedtea_ui::widgets::StackPageInfo {
            name: std::rc::Rc::from(page.name()),
            title: std::rc::Rc::from(page.title()),
            icon: None,
            needs_attention: false,
        })
        .collect()
}

/// Everything a page's widget handlers need: the shared working-copy model,
/// the app window (used as a transient-for parent by `FileDialog`), a
/// `mark_dirty` callback wired to the footer, and a `populating` guard.
///
/// `populating` is set by [`Page::refresh`] while it programmatically
/// updates widgets to match `model.working` (used by Revert); handlers call
/// [`Ctx::mark_dirty`] rather than the raw callback so that programmatic
/// updates during a refresh never re-mark the model dirty.
#[derive(Clone)]
pub struct Ctx {
    pub model: Rc<RefCell<Model>>,
    pub window: ApplicationWindow,
    pub on_dirty: Rc<dyn Fn()>,
    pub populating: Rc<Cell<bool>>,
}

impl Ctx {
    /// Notify the footer that the working copy may have changed, unless
    /// this write-back is happening as part of a programmatic [`Page::refresh`]
    /// (see [`Ctx::populating`]).
    pub fn mark_dirty(&self) {
        if !self.populating.get() {
            (self.on_dirty)();
        }
    }
}

/// A built page: its root widget (to add to the `Stack`) plus a `refresh`
/// closure that repopulates every widget from `ctx.model.working` — used
/// both to populate the page right after `build()` and again after Revert.
pub struct Page {
    pub root: gtk4::Widget,
    refresh: Rc<dyn Fn()>,
}

impl Page {
    pub fn refresh(&self) {
        (self.refresh)();
    }
}

#[cfg(test)]
mod page_id_tests {
    use super::{PageId, page_infos};

    /// The switcher hands back the index of the button pressed, and the
    /// stack selects by name — so the two orders must be the same one.
    ///
    /// Mutation check: reverse `PageId::ALL`; this test fails on
    /// `from_index(0)`. Restore.
    #[test]
    fn indices_names_and_titles_line_up() {
        assert_eq!(PageId::ALL.len(), 5);
        for (i, page) in PageId::ALL.iter().copied().enumerate() {
            assert_eq!(PageId::from_index(i), page);
            assert_eq!(page.index(), i);
        }
        assert_eq!(PageId::from_index(0), PageId::Appearance);
        assert_eq!(PageId::Appearance.name(), "appearance");
        assert_eq!(PageId::Appearance.title(), "Appearance");
        assert_eq!(PageId::Displays.name(), "displays");
        assert_eq!(PageId::Displays.title(), "Displays");
    }

    /// A `StackSwitcher` selection index arrives from a widget, so an
    /// out-of-range one is untrusted input: it must clamp, never panic.
    #[test]
    fn an_out_of_range_index_clamps_to_the_last_page() {
        assert_eq!(PageId::from_index(99), PageId::Displays);
        assert_eq!(PageId::from_index(usize::MAX), PageId::Displays);
    }

    #[test]
    fn page_infos_mirrors_page_id_all() {
        let infos = page_infos();
        assert_eq!(infos.len(), 5);
        for (info, page) in infos.iter().zip(PageId::ALL) {
            assert_eq!(&*info.name, page.name());
            assert_eq!(&*info.title, page.title());
            assert!(!info.needs_attention);
        }
    }
}
