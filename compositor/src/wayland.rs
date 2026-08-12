//! The one seam between the window model and the Wayland compositor library.
//!
//! Everything the model pushes *out* to a client — geometry, activation,
//! visibility, keyboard focus, close — goes through exactly one of these
//! methods, and everything a client pushes *in* is turned into a
//! [`ToplevelKey`] before it reaches `state.rs`. That is what let the smithay
//! dependency be removed in one commit without touching a line of the model:
//! this file is the whole of what had to be re-implemented afterwards.
//!
//! Every outbound method now really talks to `wlr` once a `Runtime` is
//! attached (`attach`): each one resolves `id`/`toplevel` through the
//! bind/forget maps below and, on a miss (no runtime, no binding, or a
//! `ToplevelId` gone stale since the `run_all` that announced it returned),
//! is a silent no-op rather than a panic. `keyboard_focus` follows the same
//! rule, plus one of its own: `focus_toplevel_keyboard` refuses an unmapped
//! toplevel, and that refusal is treated as "clear focus instead" rather
//! than left to point at whatever the seat's focus happened to be — see its
//! doc.

use std::collections::HashMap;

use icedtea_contract::{Rectangle, WindowId};

/// Identifies one client toplevel.
///
/// A transparent wrapper around the compositor library's own id, which is
/// stable, comparable and hashable and outlives the object it names — so a
/// key held past the client's departure resolves to nothing rather than to
/// freed memory or to a different window. Wrapped rather than used directly
/// so that this file stays the only one that mentions the library's types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ToplevelKey(pub(crate) wlr::ToplevelId);

impl ToplevelKey {
    /// Wrap a library id. Only the handler impls call this.
    pub(crate) fn new(id: wlr::ToplevelId) -> Self {
        ToplevelKey(id)
    }

    /// A key that no live client can have, for tests that drive `State`'s
    /// toplevel entry points without a client.
    ///
    /// `n` distinguishes one test key from another; it has no meaning beyond
    /// that, and a key built this way never resolves to a live toplevel, so
    /// every outbound push through it is a no-op — which is exactly the
    /// property that makes it safe to hand to `State`.
    pub fn for_test(n: u64) -> Self {
        ToplevelKey(wlr::ToplevelId::dangling_nth_for_test(n))
    }
}

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
    /// The compositor library's long-lived handle, once boot has created one.
    ///
    /// `Option` because `State::new` runs before any of it exists — the model
    /// is constructible with no compositor at all, which is what every model
    /// test relies on — and because that is exactly the condition each
    /// outbound method already had to tolerate.
    runtime: Option<wlr::Runtime>,
}

impl Wayland {
    pub fn new() -> Self {
        Self::default()
    }

    /// Hand the seam the compositor library's handle, once boot has one.
    pub fn attach(&mut self, runtime: wlr::Runtime) {
        self.runtime = Some(runtime);
    }

    /// The handle, or `None` in a model-only build (every unit test).
    pub fn runtime(&self) -> Option<&wlr::Runtime> {
        self.runtime.as_ref()
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

    /// Resolve `id` to the runtime handle and toplevel key needed to act on
    /// it, or `None` if either is missing.
    ///
    /// `None` means gone, not error: a model-only build has no runtime, and
    /// a window with no bound toplevel is normal (see `forget`). Callers
    /// treat either case as a silent no-op.
    fn resolve(&self, id: WindowId) -> Option<(&wlr::Runtime, ToplevelKey)> {
        let (Some(runtime), Some(key)) = (self.runtime(), self.toplevel_for(id)) else {
            return None;
        };
        Some((runtime, key))
    }

    /// Stage `content` (already in **content** space — the caller applied
    /// `decoration::content_rect`) plus the three xdg states, and let the
    /// library send one configure carrying all of them.
    ///
    /// Staged rather than sent one field at a time: the library coalesces
    /// every change made in an event-loop turn into a single configure, so a
    /// geometry change and an activation change reach the client together
    /// rather than as two round trips.
    pub fn configure(
        &self,
        id: WindowId,
        content: Rectangle,
        activated: bool,
        maximized: bool,
        fullscreen: bool,
    ) {
        let Some((runtime, key)) = self.resolve(id) else { return };
        runtime.set_toplevel_size(key.0, content.width, content.height);
        runtime.set_toplevel_activated(key.0, activated);
        runtime.set_toplevel_maximized(key.0, maximized);
        runtime.set_toplevel_fullscreen(key.0, fullscreen);
    }

    /// Move the window's scene node. `x`/`y` are **content**-space.
    pub fn set_position(&self, id: WindowId, x: i32, y: i32) {
        let Some((runtime, key)) = self.resolve(id) else { return };
        runtime.set_toplevel_position(key.0, x, y);
    }

    /// Show or hide the window's scene node.
    ///
    /// Hiding rather than unmapping, which is the distinction the smithay
    /// implementation's `Space::unmap_elem` blurred: a window on an inactive
    /// workspace keeps its buffer and its configure state and is simply not
    /// drawn, so returning to that workspace does not make the client
    /// re-render from nothing.
    pub fn set_visible(&self, id: WindowId, visible: bool) {
        let Some((runtime, key)) = self.resolve(id) else { return };
        runtime.set_toplevel_visible(key.0, visible);
    }

    /// Raise the window above its siblings.
    ///
    /// Separate from `set_position` because `behavior.raise_on_focus` decides
    /// whether a focus change alone may restack (ledger item 28): with it
    /// off, a focused window is still activated and configured, it just keeps
    /// its place in the stack.
    pub fn raise(&self, id: WindowId) {
        let Some((runtime, key)) = self.resolve(id) else { return };
        runtime.raise_toplevel(key.0);
    }

    /// Point the seat's keyboard at `id`, or at nothing.
    ///
    /// Both directions matter: `None` really clears the focus rather than
    /// leaving it where it was. The seat kept pointing at a departed surface
    /// in the smithay implementation until that was fixed (re-review New-3),
    /// and the fix belongs here now.
    ///
    /// Idempotent -- the library compares against the seat's current focus
    /// and sends nothing when it already matches -- which is what lets
    /// `sync_seat_focus` call this on every geometry sync.
    ///
    /// LEDGER (task 8): the unmapped handler leaves the model's
    /// `is_visible`/`focused`/`is_backed` all `true` -- the model has no
    /// unmapped concept of its own. `focus_toplevel_keyboard` refuses an
    /// unmapped toplevel and returns `None` in that case; this treats that
    /// refusal the same as "no binding" and falls back to clearing the
    /// seat's keyboard focus rather than leaving it pointed at whatever it
    /// last was (which could be a different, stale surface) or silently
    /// doing nothing.
    pub fn keyboard_focus(&self, id: Option<WindowId>) {
        let Some(runtime) = self.runtime() else { return };
        match id.and_then(|id| self.toplevel_for(id)) {
            Some(key) => {
                if runtime.focus_toplevel_keyboard(key.0).is_none() {
                    runtime.clear_keyboard_focus();
                }
            }
            None => runtime.clear_keyboard_focus(),
        }
    }

    /// Ask the client to close. `false` means there is no client, and the
    /// caller must remove the model row itself; `true` means the caller must
    /// leave the row alone and wait for `toplevel_destroyed`.
    ///
    /// Three states, not two, hide behind that boolean:
    ///
    /// - No binding (`toplevel_for(id)` is `None`): `false`. There never was
    ///   or no longer is a client; nothing to ask.
    /// - A binding *and* a runtime: `true`, and `close_toplevel` is actually
    ///   called.
    /// - A binding but **no runtime attached** (every unit test that never
    ///   calls `attach`): also `true`, but `close_toplevel` is never called
    ///   at all — there is no library handle to call it on. This still
    ///   reports "wait for the destroy" rather than "remove now" because the
    ///   binding is the only fact this branch has to go on, and a bound
    ///   window in a real boot always does have a client behind it.
    ///
    /// The return value reports whether *this seam* still considers `id`
    /// backed (i.e. `bind` was called for it and nothing has `forget`-ten it
    /// since) — not whether `close_toplevel` itself reported success. A
    /// binding whose `close_toplevel` call misses (runtime attached, but the
    /// `ToplevelId` doesn't resolve) can only mean the id went stale — the
    /// `run_all` that announced it has already returned, which every by-id
    /// `wlr` mutator treats as a plain miss, never a panic — and the request
    /// is best-effort in that case: `request_close` still waits for
    /// `toplevel_destroyed` rather than dropping the row out from under a
    /// client that, for all this seam's bookkeeping can tell, is still
    /// there. The cost of that choice is real, not just theoretical: a
    /// binding that goes stale with no destroy ever arriving in between (the
    /// per-`run_all` destroy listener that would fire it is torn down with
    /// the `run_all` that's already returned) leaves its model row waiting
    /// forever — a genuine leak, not merely a stale id being tolerated. See
    /// the task 8 report for why this is accepted rather than fixed here:
    /// it only happens across two separate `run_all` calls, which is outside
    /// what a single close request can detect or a headless test can set up.
    pub fn close(&self, id: WindowId) -> bool {
        let Some(key) = self.toplevel_for(id) else { return false };
        if let Some(runtime) = self.runtime() {
            runtime.close_toplevel(key.0);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binding_is_visible_from_both_directions_and_forgetting_clears_both() {
        let mut w = Wayland::new();
        let id = WindowId(7);
        let key = ToplevelKey::for_test(42);

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

    /// `raise` on a window with no runtime attached is a harmless no-op,
    /// matching every other outbound method's "no runtime, no binding -> silent
    /// no-op" contract.
    #[test]
    fn raising_an_unbound_window_is_harmless() {
        let w = Wayland::new();
        w.raise(WindowId(1));
    }

    #[test]
    fn a_seam_with_no_runtime_stays_a_no_op_rather_than_panicking() {
        let w = Wayland::new();
        assert!(w.runtime().is_none());
        w.keyboard_focus(Some(WindowId(1)));
        w.set_position(WindowId(1), 10, 10);
        w.set_visible(WindowId(1), true);
        assert!(!w.close(WindowId(1)));
    }
}
