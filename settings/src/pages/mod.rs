//! Shared plumbing for the Appearance and Behavior pages: the context every
//! widget's write-back handler closes over, and the [`Page`] handle
//! `main.rs` uses to mount a page's root widget and repopulate it on Revert.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk4::ApplicationWindow;

use crate::model::Model;

pub mod appearance;
pub mod behavior;
pub mod keybindings;
pub mod workspaces;

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
