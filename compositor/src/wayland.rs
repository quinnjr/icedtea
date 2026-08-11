//! The one seam between the window model and the Wayland compositor library.
//!
//! Everything the model pushes *out* to a client — geometry, activation,
//! visibility, keyboard focus, close — goes through exactly one of these
//! methods, and everything a client pushes *in* is turned into a
//! [`ToplevelKey`] before it reaches `state.rs`. That is what let the smithay
//! dependency be removed in one commit without touching a line of the model:
//! this file is the whole of what had to be re-implemented afterwards.
//!
//! In this commit every outbound method is a deliberate no-op. No client is
//! ever bound in this commit, so [`Wayland::is_backed`] is false in
//! practice — which is exactly the behaviour the model already tolerates:
//! `sync_window_to_scene`'s "a window with no backing surface is a silent
//! no-op" contract predates the port. The model tests therefore pass
//! unchanged, and they are the reason this intermediate state exists at
//! all.

use std::collections::HashMap;

use icedtea_contract::{Rectangle, WindowId};

/// Identifies one client toplevel.
///
/// A `u64` newtype rather than the library's own id type so this file — and
/// so the whole compositor crate — compiles with no compositor library at
/// all. It becomes a wrapper around `wlr::ToplevelId` in the port's next
/// step; nothing outside this file constructs one from an integer, so that
/// change reaches no other module.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ToplevelKey(pub u64);

/// The compositor's Wayland side.
#[derive(Default)]
pub struct Wayland {
    /// Which model window each live toplevel backs, and the reverse.
    ///
    /// Two maps rather than one plus a scan: `sync_window_to_scene` resolves
    /// model → toplevel on every geometry mutation, and destroy resolves
    /// toplevel → model, so both directions are hot.
    toplevel_to_window: HashMap<ToplevelKey, WindowId>,
    window_to_toplevel: HashMap<WindowId, ToplevelKey>,
}

impl Wayland {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that `toplevel` backs model window `id`.
    pub fn bind(&mut self, id: WindowId, toplevel: ToplevelKey) {
        self.toplevel_to_window.insert(toplevel, id);
        self.window_to_toplevel.insert(id, toplevel);
    }

    /// Drop every trace of model window `id`.
    ///
    /// Idempotent: forgetting a window that was never bound is normal, since
    /// model windows created by tests have no toplevel at all.
    pub fn forget(&mut self, id: WindowId) {
        if let Some(key) = self.window_to_toplevel.remove(&id) {
            self.toplevel_to_window.remove(&key);
        }
    }

    pub fn window_for(&self, toplevel: ToplevelKey) -> Option<WindowId> {
        self.toplevel_to_window.get(&toplevel).copied()
    }

    pub fn toplevel_for(&self, id: WindowId) -> Option<ToplevelKey> {
        self.window_to_toplevel.get(&id).copied()
    }

    /// Whether `id` has a client behind it. `false` for model-only windows.
    pub fn is_backed(&self, id: WindowId) -> bool {
        self.window_to_toplevel.contains_key(&id)
    }

    /// Stage `content` (already in **content** space — the caller applied
    /// `decoration::content_rect`) plus the three xdg states, and send the
    /// client a configure.
    pub fn configure(
        &self,
        id: WindowId,
        content: Rectangle,
        activated: bool,
        maximized: bool,
        fullscreen: bool,
    ) {
        let _ = (id, content, activated, maximized, fullscreen);
    }

    /// Move the window's scene node. `x`/`y` are **content**-space.
    pub fn set_position(&self, id: WindowId, x: i32, y: i32) {
        let _ = (id, x, y);
    }

    /// Restack the toplevel's scene node to the top.
    ///
    /// A deliberate no-op until `wlr` ships a scene-node raise mutator —
    /// planned for 0.20.2, after this port's pinned 0.20.1. Kept as a real
    /// method (rather than left unwired) so `sync_window_to_scene` can call
    /// it unconditionally now and task 5 only has to fill this one body in,
    /// not go find every call site that should have raised.
    pub fn raise(&self, toplevel: ToplevelKey) {
        let _ = toplevel;
    }

    /// Show or hide the window's scene node (a window on an inactive
    /// workspace, or a minimized one, is hidden rather than unmapped).
    pub fn set_visible(&self, id: WindowId, visible: bool) {
        let _ = (id, visible);
    }

    /// Point the seat's keyboard at `id`, or at nothing.
    pub fn keyboard_focus(&self, id: Option<WindowId>) {
        let _ = id;
    }

    /// Ask the client to close. Returns whether a request was actually sent —
    /// `false` means there is no client, and the caller must remove the model
    /// row itself because no destroy will ever arrive.
    pub fn close(&self, id: WindowId) -> bool {
        let _ = id;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binding_is_visible_from_both_directions_and_forgetting_clears_both() {
        let mut w = Wayland::new();
        let id = WindowId(7);
        let key = ToplevelKey(42);

        assert!(!w.is_backed(id));
        w.bind(id, key);
        assert_eq!(w.window_for(key), Some(id));
        assert_eq!(w.toplevel_for(id), Some(key));
        assert!(w.is_backed(id));

        w.forget(id);
        assert_eq!(w.window_for(key), None, "the reverse map must be cleared too");
        assert_eq!(w.toplevel_for(id), None);
        assert!(!w.is_backed(id));
    }

    /// Forgetting a model-only window is the common case, not an error: every
    /// window a test creates takes this path.
    #[test]
    fn forgetting_an_unbound_window_is_harmless() {
        let mut w = Wayland::new();
        w.forget(WindowId(1));
        assert!(!w.is_backed(WindowId(1)));
    }

    /// `close` reporting `false` is what tells `request_close` to remove the
    /// model row synchronously instead of waiting for a destroy that never
    /// comes.
    #[test]
    fn closing_an_unbacked_window_reports_that_nothing_was_sent() {
        let w = Wayland::new();
        assert!(!w.close(WindowId(1)));
    }

    /// `raise` on a key nothing is bound to is a harmless no-op, matching
    /// every other outbound method in this commit -- there is no scene-node
    /// mutator behind it yet (see `raise`'s doc).
    #[test]
    fn raising_an_unbound_toplevel_is_harmless() {
        let w = Wayland::new();
        w.raise(ToplevelKey(1));
    }
}
