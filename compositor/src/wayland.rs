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

/// Every scene node one server-side-decorated window's title bar is made
/// of, all parented into that window's own toplevel tree.
///
/// One struct rather than three maps because they share a lifetime exactly:
/// they are created together the first time a window needs decorations and
/// destroyed together when it stops (`sync_ssd`) or dies (`remove_ssd`).
/// The two `Option`s inside are the pieces that can legitimately be absent
/// while the band itself exists: a title that rasterized to nothing (an
/// empty title, or no font on the machine with the glyphs for it), and a
/// button rect wlroots refused to create. Neither is a reason to drop the
/// band -- a decoration degrades, it never fails the window.
struct SsdVisual {
    /// The title-bar band, the full width of the frame.
    band: wlr::RectId,
    /// The rasterized title, sitting over the band's left-hand span.
    title: Option<wlr::BufferId>,
    /// Minimize, maximize, close -- `decoration::button_rects`' own order,
    /// left to right, which is also the order `hit_test` maps them in.
    buttons: [Option<wlr::RectId>; 3],
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
    /// One [`SsdVisual`] per window currently wearing server-side
    /// decorations, keyed the same way the toplevel maps are (review finding
    /// I2). `None` (no entry) means gone -- nothing is painted for that
    /// window right now -- the same discipline
    /// `toplevel_to_window`/`window_to_toplevel` already follow. Created
    /// lazily by `sync_ssd` the first time a window needs decorations, every
    /// node parented into that window's own toplevel scene tree
    /// (`Runtime::add_rect_in_toplevel`/`add_buffer_in_toplevel`) so the
    /// whole decoration rides the toplevel's z-order with no separate raise
    /// bookkeeping. Dropped from this map both when the decoration becomes
    /// hidden (`sync_ssd`, which removes rather than merely hides it) and
    /// when the window is gone for good (`remove_ssd`, called from `forget`).
    ssd: HashMap<WindowId, SsdVisual>,
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
        self.remove_ssd(id);
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

    /// Answer a client's xdg-decoration negotiation for `toplevel`.
    ///
    /// Keyed by `ToplevelKey`, not `WindowId`, because this is the one
    /// outbound push that legitimately happens *before* the model has a
    /// window at all: a client creates its decoration object and states its
    /// preference before the initial commit, and `mapped` -- which is what
    /// creates the model row -- has not run yet.
    ///
    /// Silent no-op on a miss, like every other method here: no runtime, a
    /// stale id, or a toplevel whose client never created a decoration
    /// object (by far the most common case -- most clients never bind
    /// `zxdg_decoration_manager_v1` at all) each report `None`, and none of
    /// them is an error.
    pub fn set_decoration_mode(&self, toplevel: ToplevelKey, mode: wlr::DecorationMode) {
        let Some(runtime) = self.runtime() else { return };
        runtime.set_decoration_mode(toplevel.0, mode);
    }

    /// Paint (or update, or hide) window `id`'s whole server-side
    /// decoration: the title-bar band, the rasterized title over it, and the
    /// three button rects.
    ///
    /// Fix for review finding I2, completed: `draw_frame` -- and every
    /// custom render element it built, `Decoration` included -- died with
    /// smithay, so the 28px band `decoration::content_rect` reserves above
    /// every SSD window's content has to be painted some other way. Every
    /// node here is parented into the window's own toplevel scene tree
    /// (`add_rect_in_toplevel`/`add_buffer_in_toplevel`), so the decoration
    /// rides the toplevel: raising, lowering or restacking the window moves
    /// the whole title bar with it with no separate bookkeeping, closing the
    /// z-order defect structurally rather than by re-raising nodes on every
    /// restack.
    ///
    /// Hit-testing is deliberately *not* derived from these nodes.
    /// `decoration::hit_test` answers from the model's frame geometry, the
    /// same geometry this function is handed, so a click and a pixel can
    /// never disagree about which button they mean -- and the scene nodes
    /// stay pure output, with no input role at all.
    ///
    /// `bar` is frame-space, matching `decoration::title_bar_rect`'s own
    /// contract; `content` is `decoration::content_rect`'s output for the
    /// same frame. Both come from the same `w.geometry` in the caller
    /// (`State::sync_window_to_scene`), so they can never drift apart from
    /// each other. `r.x - content.x, r.y - content.y` is a node's position
    /// relative to the toplevel tree's own origin -- these calls' coordinates
    /// are relative to that origin, not the scene root's, the same origin
    /// `set_position` moves via `set_toplevel_position`.
    ///
    /// `title_px` is `(width, height, premultiplied RGBA)` from
    /// `text::rasterize_title`; `None` means the title rasterized to nothing
    /// (an empty title, or no font on this machine that can shape it) and
    /// the band is shown bare -- the spec's error-handling rule that a
    /// decoration degrades rather than failing the window.
    ///
    /// Removes (rather than merely hides) every node when `ssd` is `false`
    /// or the window isn't currently visible: a hidden decoration is cheaper
    /// to recreate than to keep, and unlike the old root-rect scheme there is
    /// no stacking-order reason to keep it around invisible.
    // Nine arguments, deliberately: every one of them is a fact the caller
    // (`State::sync_window_to_scene`) already has and this seam must not
    // re-derive -- the model's geometry, its palette, and its title pixels.
    // Bundling them into a struct would only move the same nine fields one
    // line up at the single call site, while making the "no model types
    // below this seam" rule harder to keep.
    #[allow(clippy::too_many_arguments)]
    pub fn sync_ssd(
        &mut self,
        id: WindowId,
        ssd: bool,
        visible: bool,
        bar: Rectangle,
        content: Rectangle,
        band_color: [f32; 4],
        button_colors: [[f32; 4]; 3],
        title_px: Option<(i32, i32, Vec<u8>)>,
    ) {
        if !ssd || !visible {
            self.remove_ssd(id);
            return;
        }
        let Some(runtime) = self.runtime.clone() else { return };
        let width = bar.width.max(1);
        let height = bar.height.max(1);
        let (rel_x, rel_y) = (bar.x - content.x, bar.y - content.y);

        // The band is the entry: no band, no decoration. It is also created
        // first so that every node added below lands above it in the
        // toplevel tree's own stacking order -- buttons and title over the
        // band, never under it.
        if !self.ssd.contains_key(&id) {
            let Some(key) = self.toplevel_for(id) else { return };
            let Some(band) = runtime.add_rect_in_toplevel(key.0, width, height, band_color) else {
                return;
            };
            self.ssd.insert(
                id,
                SsdVisual {
                    band,
                    title: None,
                    buttons: [None; 3],
                },
            );
        }
        let key = self.toplevel_for(id);
        let Some(visual) = self.ssd.get_mut(&id) else { return };

        runtime.set_rect_size(visual.band, width, height);
        runtime.set_rect_position(visual.band, rel_x, rel_y);
        runtime.set_rect_color(visual.band, band_color);

        // `button_rects` takes the frame, but reads only `x`, `y` and
        // `width` off it -- all three identical in `bar`, which
        // `title_bar_rect` derived from that same frame. Passing `bar`
        // keeps this function from needing the frame as a fourth
        // near-duplicate rectangle argument.
        let rects = crate::decoration::button_rects(bar);
        for (i, r) in rects.iter().enumerate() {
            let slot = &mut visual.buttons[i];
            if slot.is_none() {
                let Some(key) = key else { continue };
                *slot = runtime.add_rect_in_toplevel(
                    key.0,
                    r.width.max(1),
                    r.height.max(1),
                    button_colors[i],
                );
            }
            let Some(rect) = *slot else { continue };
            runtime.set_rect_size(rect, r.width.max(1), r.height.max(1));
            runtime.set_rect_position(rect, r.x - content.x, r.y - content.y);
            runtime.set_rect_color(rect, button_colors[i]);
        }

        match title_px {
            Some((tw, th, px)) => {
                // `update_buffer` rather than remove-and-re-add: it exists
                // for exactly this (a re-titled or resized window), and it
                // keeps the node's place in the stacking order instead of
                // re-adding it on top of whatever was added since. A `None`
                // from it means the id went stale (its parent toplevel was
                // torn down), so the node is dropped and rebuilt.
                let updated = visual
                    .title
                    .and_then(|buffer| runtime.update_buffer(buffer, tw, th, &px));
                if updated.is_none() {
                    if let Some(stale) = visual.title.take() {
                        runtime.remove_buffer(stale);
                    }
                    let Some(key) = key else { return };
                    visual.title = runtime.add_buffer_in_toplevel(key.0, tw, th, &px);
                }
                if let Some(buffer) = visual.title {
                    runtime.set_buffer_position(buffer, rel_x, rel_y);
                }
            }
            None => {
                if let Some(buffer) = visual.title.take() {
                    runtime.remove_buffer(buffer);
                }
            }
        }
    }

    /// Drop `id`'s SSD nodes for good. Called from `forget` so a window's
    /// decoration never outlives the window itself.
    ///
    /// Tolerates every removal reporting `None`: a node may already be gone
    /// because its parent toplevel died first (a toplevel's tree, and every
    /// rect or buffer parented into it, is freed when the toplevel is torn
    /// down), which is a normal race between the two teardown paths, not an
    /// error.
    fn remove_ssd(&mut self, id: WindowId) {
        let Some(visual) = self.ssd.remove(&id) else { return };
        let Some(runtime) = self.runtime() else { return };
        runtime.remove_rect(visual.band);
        if let Some(buffer) = visual.title {
            runtime.remove_buffer(buffer);
        }
        for rect in visual.buttons.into_iter().flatten() {
            runtime.remove_rect(rect);
        }
    }

    /// How many SSD title bars are currently tracked -- one per decorated
    /// window, whatever it is made of. Introspection for tests; nothing in a
    /// real boot needs to count these itself.
    pub fn ssd_rect_count(&self) -> usize {
        self.ssd.len()
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

    /// The pure coordinate math `sync_ssd` must feed
    /// `add_rect_in_toplevel`/`set_rect_position`: the band sits flush with
    /// the toplevel tree's origin horizontally and `TITLE_BAR_HEIGHT` pixels
    /// above it vertically, since `content_rect` moves the content down by
    /// exactly that much.
    #[test]
    fn ssd_rect_relative_offset_is_zero_minus_titlebar() {
        // With no runtime attached the seam is a no-op, so this asserts the
        // pure coordinate math via the helper the impl must use.
        let frame = Rectangle { x: 100, y: 200, width: 400, height: 300 };
        let bar = crate::decoration::title_bar_rect(frame);
        let content = crate::decoration::content_rect(frame, true);
        assert_eq!(
            (bar.x - content.x, bar.y - content.y),
            (0, -crate::decoration::TITLE_BAR_HEIGHT)
        );
    }

    /// No runtime attached (every unit test that never calls `attach`) means
    /// `sync_ssd`/`remove_ssd` are no-ops, matching every other outbound
    /// method's contract -- and in particular never populate `ssd`, since
    /// there is no `RectId` a real call could have returned.
    #[test]
    fn syncing_ssd_with_no_runtime_is_harmless() {
        let mut w = Wayland::new();
        let id = WindowId(3);
        let frame = Rectangle { x: 100, y: 200, width: 400, height: 300 };
        let bar = crate::decoration::title_bar_rect(frame);
        let content = crate::decoration::content_rect(frame, true);
        let color = [1.0, 1.0, 1.0, 1.0];

        w.sync_ssd(id, true, true, bar, content, color, [[0.0; 4]; 3], None);
        assert_eq!(w.ssd_rect_count(), 0);

        w.sync_ssd(id, false, true, bar, content, color, [[0.0; 4]; 3], None);
        assert_eq!(w.ssd_rect_count(), 0);

        w.forget(id);
        assert_eq!(w.ssd_rect_count(), 0);
    }

    /// Even with real title pixels and real button colors, a seam with no
    /// runtime creates nothing and panics on nothing: the buffer-node path
    /// (`add_buffer_in_toplevel`/`update_buffer`) has the same "no runtime,
    /// no binding -> silent no-op" contract the rect path has.
    #[test]
    fn syncing_ssd_with_title_pixels_and_no_runtime_is_harmless() {
        let mut w = Wayland::new();
        let id = WindowId(4);
        let frame = Rectangle { x: 0, y: 0, width: 400, height: 300 };
        let bar = crate::decoration::title_bar_rect(frame);
        let content = crate::decoration::content_rect(frame, true);
        let px = vec![0u8; (bar.width as usize) * (bar.height as usize) * 4];

        w.sync_ssd(
            id,
            true,
            true,
            bar,
            content,
            [0.1, 0.1, 0.1, 1.0],
            [[1.0, 0.0, 0.0, 1.0]; 3],
            Some((bar.width, bar.height, px)),
        );
        assert_eq!(w.ssd_rect_count(), 0);
    }

    /// The button rects `sync_ssd` positions come from
    /// `decoration::button_rects` applied to the *bar*, not the frame -- the
    /// two must agree, since `hit_test` (which answers clicks) reads the
    /// frame while the scene nodes are placed from the bar.
    #[test]
    fn button_rects_agree_whether_derived_from_the_frame_or_the_bar() {
        let frame = Rectangle { x: 100, y: 200, width: 400, height: 300 };
        let bar = crate::decoration::title_bar_rect(frame);
        assert_eq!(
            crate::decoration::button_rects(bar),
            crate::decoration::button_rects(frame)
        );
    }
}
