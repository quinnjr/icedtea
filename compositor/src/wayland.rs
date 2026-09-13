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
    ///
    /// `pub` (not `#[cfg(test)]`, unlike [`PopupKey::for_test`]) only because
    /// `compositor/tests/headless_boot.rs` is a separate crate and needs it;
    /// production code must never call it.
    #[doc(hidden)]
    pub fn for_test(n: u64) -> Self {
        ToplevelKey(wlr::ToplevelId::dangling_nth_for_test(n))
    }
}

/// Identifies one live client popup.
///
/// The popup twin of [`ToplevelKey`], and wrapped for the same reason: this
/// file stays the only one that mentions the compositor library's own id
/// types, so a key held past the popup's departure resolves to nothing rather
/// than to freed memory or to a different popup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PopupKey(pub(crate) wlr::PopupId);

impl PopupKey {
    /// Wrap a library id. Only the handler impls call this.
    pub(crate) fn new(id: wlr::PopupId) -> Self {
        PopupKey(id)
    }

    /// A key that no live client can have, for tests that drive `State`'s
    /// popup entry points without a client.
    ///
    /// Same contract as [`ToplevelKey::for_test`]: `n` only distinguishes one
    /// test key from another, and a key built this way never resolves to a
    /// live popup, so every outbound push through it is a no-op.
    ///
    /// `#[cfg(test)]` because -- unlike [`ToplevelKey::for_test`], which
    /// `tests/headless_boot.rs` builds keys with from outside the crate --
    /// every caller of this one is a unit test in `state.rs` or in this
    /// file's own `tests` module, so it need not be part of the public API.
    #[cfg(test)]
    pub fn for_test(n: u64) -> Self {
        PopupKey(wlr::PopupId::dangling_nth_for_test(n))
    }
}

/// Which kind of client surface backs a model window — the one focus/
/// stacking/SSD/geometry path serves both, and every outbound seam method
/// dispatches on this (M2, XWayland design Decision 2). `Xdg` is a native
/// wlr xdg toplevel; `X11` is a managed (non-override-redirect) Xwayland
/// surface. Override-redirect X11 surfaces are never modelled and so never
/// carry a `SurfaceKey`.
///
/// Retained for the focus path only (`focus_key` / `apply_focus_key`);
/// window ops dispatch via [`backend`](Wayland::backend).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SurfaceKey {
    Xdg(ToplevelKey),
    X11(wlr::XwaylandSurfaceId),
}

/// A window's client backend as an object: the per-op `match SurfaceKey`
/// arms that used to live in every seam method now live exactly once, in
/// the `WindowBackend` impls below. `Wayland::backend` resolves one per
/// call, so adding a surface kind means adding an impl, not re-threading
/// a dozen matches.
///
/// `SurfaceKey` above stays for the focus path (`focus_key` /
/// `apply_focus_key`), whose `None`-means-clear fallback has no backend
/// object to hang on — unifying it would drag `State`'s focus callers
/// along for no parity gain.
#[derive(Clone, Copy)]
struct XdgBackend {
    toplevel: ToplevelKey,
}

/// An X11 window's backend: its surface id plus the pushed-state memo.
/// The memo lives here rather than in a `Wayland` map so the "what did we
/// last push" state travels with the object that decides what to push;
/// `Wayland` persists the whole backend per window (`x11_backends`) across
/// calls.
///
/// `pushed` is `None` until the first push: `None` forces every atom write
/// (a fresh binding has no last-pushed state to compare against), and each
/// push fills the fields it owns. `Some` compares against the memo and
/// skips unchanged atoms.
#[derive(Clone, Copy)]
struct X11Backend {
    sid: wlr::XwaylandSurfaceId,
    pushed: Option<X11PushedState>,
}

/// One backend per surface kind behind [`AnyWindowBackend`].
///
/// Every method takes the `wlr::Runtime` explicitly rather than holding
/// it: the runtime handle is owned by `Wayland`, and borrowing it into a
/// long-lived backend object would tie the object's lifetime to a borrow
/// no call site can sustain. Immutable methods take `&self`; the two that
/// record pushed state (`configure`, `set_minimized`) take `&mut self`
/// and the caller writes the backend back.
trait WindowBackend {
    fn configure(
        &mut self,
        rt: &wlr::Runtime,
        content: Rectangle,
        activated: bool,
        maximized: bool,
        fullscreen: bool,
    );
    fn set_position(&self, rt: &wlr::Runtime, x: i32, y: i32);
    fn set_visible(&self, rt: &wlr::Runtime, visible: bool);
    fn set_minimized(&mut self, rt: &wlr::Runtime, minimized: bool);
    fn raise(&self, rt: &wlr::Runtime, ssd: Option<&SsdVisual>);
    fn close(&self, rt: &wlr::Runtime);
}

/// The SSD node factories plus the content-to-frame origin: one path per
/// surface kind hiding the xdg-tree vs. toplevel-band split from `sync_ssd`.
trait SsdFactory {
    fn ssd_origin(&self, content_x: i32, content_y: i32) -> (i32, i32);
    fn make_ssd_rect(
        &self,
        rt: &wlr::Runtime,
        w: i32,
        h: i32,
        color: [f32; 4],
    ) -> Option<wlr::RectId>;
    fn make_ssd_buffer(
        &self,
        rt: &wlr::Runtime,
        w: i32,
        h: i32,
        px: &[u8],
    ) -> Option<wlr::BufferId>;
}

impl WindowBackend for XdgBackend {
    fn configure(
        &mut self,
        rt: &wlr::Runtime,
        content: Rectangle,
        activated: bool,
        maximized: bool,
        fullscreen: bool,
    ) {
        rt.set_toplevel_size(self.toplevel.0, content.width, content.height);
        rt.set_toplevel_activated(self.toplevel.0, activated);
        rt.set_toplevel_maximized(self.toplevel.0, maximized);
        rt.set_toplevel_fullscreen(self.toplevel.0, fullscreen);
    }

    fn set_position(&self, rt: &wlr::Runtime, x: i32, y: i32) {
        rt.set_toplevel_position(self.toplevel.0, x, y);
    }

    fn set_visible(&self, rt: &wlr::Runtime, visible: bool) {
        rt.set_toplevel_visible(self.toplevel.0, visible);
    }

    fn set_minimized(&mut self, _rt: &wlr::Runtime, _minimized: bool) {
        // xdg-shell has no minimized toplevel state to push (a client
        // requests minimize; the WM just hides the window) — hiding is
        // already carried by `set_visible`.
    }

    fn raise(&self, rt: &wlr::Runtime, _ssd: Option<&SsdVisual>) {
        rt.raise_toplevel(self.toplevel.0);
    }

    fn close(&self, rt: &wlr::Runtime) {
        rt.close_toplevel(self.toplevel.0);
    }
}

impl SsdFactory for XdgBackend {
    fn ssd_origin(&self, content_x: i32, content_y: i32) -> (i32, i32) {
        (content_x, content_y)
    }

    fn make_ssd_rect(
        &self,
        rt: &wlr::Runtime,
        w: i32,
        h: i32,
        color: [f32; 4],
    ) -> Option<wlr::RectId> {
        rt.add_rect_in_toplevel(self.toplevel.0, w, h, color)
    }

    fn make_ssd_buffer(
        &self,
        rt: &wlr::Runtime,
        w: i32,
        h: i32,
        px: &[u8],
    ) -> Option<wlr::BufferId> {
        rt.add_buffer_in_toplevel(self.toplevel.0, w, h, px)
    }
}

impl WindowBackend for X11Backend {
    fn configure(
        &mut self,
        rt: &wlr::Runtime,
        content: Rectangle,
        activated: bool,
        maximized: bool,
        fullscreen: bool,
    ) {
        // An X11 window is configured to the whole **content** rect
        // (position *and* size), not just a size: xdg clients do not
        // know where they are, but X11 clients place themselves and
        // must be told the geometry the WM granted, inside the SSD.
        // Geometry is always sent — it is exactly what changes on a drag
        // frame — but the `_NET_WM_STATE` atoms are memoized so an
        // unchanged one costs no atom write + xwm flush (review finding
        // #6). `None` forces every atom write (see `X11Backend::pushed`);
        // `Some` skips the atoms whose value has not changed since the
        // last push.
        rt.configure_xwayland_surface(
            self.sid,
            wlr::Box2D::new(content.x, content.y, content.width, content.height),
        );
        match self.pushed {
            None => {
                rt.activate_xwayland_surface(self.sid, activated);
                rt.set_xwayland_surface_maximized(self.sid, maximized);
                rt.set_xwayland_surface_fullscreen(self.sid, fullscreen);
                // `minimized` (owned by `set_minimized`) untouched.
                self.pushed = Some(X11PushedState {
                    activated,
                    maximized,
                    fullscreen,
                    minimized: false,
                });
            }
            Some(mut memo) => {
                if memo.activated != activated {
                    rt.activate_xwayland_surface(self.sid, activated);
                }
                if memo.maximized != maximized {
                    rt.set_xwayland_surface_maximized(self.sid, maximized);
                }
                if memo.fullscreen != fullscreen {
                    rt.set_xwayland_surface_fullscreen(self.sid, fullscreen);
                }
                // `minimized` (owned by `set_minimized`) untouched.
                memo.activated = activated;
                memo.maximized = maximized;
                memo.fullscreen = fullscreen;
                self.pushed = Some(memo);
            }
        }
    }

    fn set_position(&self, rt: &wlr::Runtime, x: i32, y: i32) {
        rt.set_xwayland_surface_position(self.sid, x, y);
    }

    fn set_visible(&self, rt: &wlr::Runtime, visible: bool) {
        rt.set_xwayland_surface_visible(self.sid, visible);
    }

    fn set_minimized(&mut self, rt: &wlr::Runtime, minimized: bool) {
        // Skip the atom write + xwm flush when the value is unchanged
        // from what this window last received (see `X11Backend::pushed`).
        // `None` (never pushed) always writes.
        match self.pushed {
            Some(memo) if memo.minimized == minimized => {}
            Some(mut memo) => {
                rt.set_xwayland_surface_minimized(self.sid, minimized);
                memo.minimized = minimized;
                self.pushed = Some(memo);
            }
            None => {
                rt.set_xwayland_surface_minimized(self.sid, minimized);
                self.pushed = Some(X11PushedState {
                    minimized,
                    ..X11PushedState::default()
                });
            }
        }
    }

    fn raise(&self, rt: &wlr::Runtime, ssd: Option<&SsdVisual>) {
        // An xdg toplevel's SSD rides its own scene tree, so one
        // `raise_toplevel` lifts the whole window. An X11 window's
        // scene node lives directly in the toplevel band and its SSD
        // nodes are band siblings, so raising the window means raising
        // the surface node *and* each decoration node above the other
        // windows in the band, then restacking the X11 window itself
        // for stacking parity with X11-native clients.
        rt.raise_xwayland_surface(self.sid);
        if let Some(visual) = ssd {
            rt.raise_rect(visual.band);
            for rect in visual.buttons.into_iter().flatten() {
                rt.raise_rect(rect);
            }
            if let Some(title) = visual.title {
                rt.raise_buffer(title);
            }
            for glyph in visual.button_glyphs.into_iter().flatten() {
                rt.raise_buffer(glyph);
            }
        }
        rt.restack_xwayland_surface(self.sid, None, true);
    }

    fn close(&self, rt: &wlr::Runtime) {
        rt.close_xwayland_surface(self.sid);
    }
}

impl SsdFactory for X11Backend {
    fn ssd_origin(&self, _content_x: i32, _content_y: i32) -> (i32, i32) {
        (0, 0)
    }

    fn make_ssd_rect(
        &self,
        rt: &wlr::Runtime,
        w: i32,
        h: i32,
        color: [f32; 4],
    ) -> Option<wlr::RectId> {
        rt.add_rect_in_band(wlr::Band::Toplevel, w, h, color).ok()
    }

    fn make_ssd_buffer(
        &self,
        rt: &wlr::Runtime,
        w: i32,
        h: i32,
        px: &[u8],
    ) -> Option<wlr::BufferId> {
        rt.add_buffer_in_band(wlr::Band::Toplevel, w, h, px)
    }
}

/// One value per surface kind implementing both backend traits. The
/// business logic lives once per kind, in the `WindowBackend` and
/// `SsdFactory` impls above; this enum's impls are the per-method 2-arm
/// delegates that forward each call to the kind it holds. Callers never
/// match on the kind themselves: they resolve once per call through
/// [`backend`](Wayland::backend) — the dispatch point this enum shares
/// with [`surface_key`](Wayland::surface_key) — then call trait methods.
/// `X11Backend` travels by value (surface id plus an optional four-bool
/// memo — cheaper than a borrow chain through two maps) and the caller
/// writes it back (via `persist_backend`) when a `&mut self` method may
/// have moved its memo.
#[derive(Clone, Copy)]
enum AnyWindowBackend {
    Xdg(XdgBackend),
    X11(X11Backend),
}

impl WindowBackend for AnyWindowBackend {
    fn configure(
        &mut self,
        rt: &wlr::Runtime,
        content: Rectangle,
        activated: bool,
        maximized: bool,
        fullscreen: bool,
    ) {
        match self {
            AnyWindowBackend::Xdg(b) => b.configure(rt, content, activated, maximized, fullscreen),
            AnyWindowBackend::X11(b) => b.configure(rt, content, activated, maximized, fullscreen),
        }
    }

    fn set_position(&self, rt: &wlr::Runtime, x: i32, y: i32) {
        match self {
            AnyWindowBackend::Xdg(b) => b.set_position(rt, x, y),
            AnyWindowBackend::X11(b) => b.set_position(rt, x, y),
        }
    }

    fn set_visible(&self, rt: &wlr::Runtime, visible: bool) {
        match self {
            AnyWindowBackend::Xdg(b) => b.set_visible(rt, visible),
            AnyWindowBackend::X11(b) => b.set_visible(rt, visible),
        }
    }

    fn set_minimized(&mut self, rt: &wlr::Runtime, minimized: bool) {
        match self {
            AnyWindowBackend::Xdg(b) => b.set_minimized(rt, minimized),
            AnyWindowBackend::X11(b) => b.set_minimized(rt, minimized),
        }
    }

    fn raise(&self, rt: &wlr::Runtime, ssd: Option<&SsdVisual>) {
        match self {
            AnyWindowBackend::Xdg(b) => b.raise(rt, ssd),
            AnyWindowBackend::X11(b) => b.raise(rt, ssd),
        }
    }

    fn close(&self, rt: &wlr::Runtime) {
        match self {
            AnyWindowBackend::Xdg(b) => b.close(rt),
            AnyWindowBackend::X11(b) => b.close(rt),
        }
    }
}

impl SsdFactory for AnyWindowBackend {
    fn ssd_origin(&self, content_x: i32, content_y: i32) -> (i32, i32) {
        match self {
            AnyWindowBackend::Xdg(b) => b.ssd_origin(content_x, content_y),
            AnyWindowBackend::X11(b) => b.ssd_origin(content_x, content_y),
        }
    }

    fn make_ssd_rect(
        &self,
        rt: &wlr::Runtime,
        w: i32,
        h: i32,
        color: [f32; 4],
    ) -> Option<wlr::RectId> {
        match self {
            AnyWindowBackend::Xdg(b) => b.make_ssd_rect(rt, w, h, color),
            AnyWindowBackend::X11(b) => b.make_ssd_rect(rt, w, h, color),
        }
    }

    fn make_ssd_buffer(
        &self,
        rt: &wlr::Runtime,
        w: i32,
        h: i32,
        px: &[u8],
    ) -> Option<wlr::BufferId> {
        match self {
            AnyWindowBackend::Xdg(b) => b.make_ssd_buffer(rt, w, h, px),
            AnyWindowBackend::X11(b) => b.make_ssd_buffer(rt, w, h, px),
        }
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
    /// Which [`TitleRaster::generation`] `title` currently holds, or `None`
    /// when there is no title node.
    ///
    /// This is the whole of the re-upload check (review finding M2):
    /// `sync_ssd` runs on every geometry mutation, i.e. once per pointer
    /// motion during a drag, and without this every one of those would
    /// re-upload byte-identical pixels and damage the scene for them. The
    /// caller's memo never reuses a generation, so equality here means "the
    /// exact pixels this node already holds".
    title_generation: Option<u64>,
    /// Minimize, maximize, close -- `decoration::button_rects`' own order,
    /// left to right, which is also the order `hit_test` maps them in.
    buttons: [Option<wlr::RectId>; 3],
    /// The rasterized glyph (en-dash / square / x) drawn over each button
    /// rect, same order as `buttons`. `None` per-slot for the same two
    /// reasons `title` can be `None`: the glyph rasterized to nothing, or
    /// wlroots refused the buffer node.
    button_glyphs: [Option<wlr::BufferId>; 3],
    /// Which `(width, height, fg)` each `button_glyphs` slot currently holds
    /// pixels for -- the glyph analogue of `title_generation`. A button
    /// rect's size never changes without a full frame geometry change (it is
    /// always `BUTTON_WIDTH x TITLE_BAR_HEIGHT`), and a glyph's `fg` only
    /// changes with the palette, so this key changes far less often than
    /// `sync_ssd` runs -- comparing it, like the title generation, turns a
    /// drag's per-motion re-sync back into a cheap position-only update
    /// instead of a re-upload of byte-identical pixels every frame.
    button_glyph_keys: [Option<(i32, i32, [u8; 4])>; 3],
}

/// A rasterized title, borrowed from the caller's own memo.
///
/// Borrowed rather than owned so a sync whose title has not changed costs no
/// copy at all -- the common case by a wide margin, since a drag re-syncs
/// per pointer motion. `generation` names these exact pixels; see
/// `SsdVisual::title_generation`.
pub struct TitleRaster<'a> {
    pub width: i32,
    pub height: i32,
    pub generation: u64,
    pub pixels: &'a [u8],
}

/// A rasterized button glyph, borrowed from the caller's own memo -- the
/// glyph analogue of [`TitleRaster`]. `width`/`height`/`fg` double as the
/// cache key `sync_ssd` compares against `SsdVisual::button_glyph_keys`;
/// there is no separate generation counter because, unlike a title, a
/// glyph's pixels are a pure function of that triple, so the triple itself
/// is all the "has this changed" check needs.
pub struct GlyphRaster<'a> {
    pub width: i32,
    pub height: i32,
    pub fg: [u8; 4],
    pub pixels: &'a [u8],
}

/// Whether a title-bar button is being hovered or actively pressed, so
/// `sync_ssd` can shift its color for feedback. Hover and press are
/// mutually exclusive at any instant (a press implies the pointer is over
/// the button, but the caller only ever reports the stronger of the two),
/// so a single `Option<(usize, ButtonState)>` -- not two separate optional
/// indices -- is `sync_ssd`'s whole contract for this.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonState {
    Hover,
    Pressed,
}

/// Shifts a premultiplied button color for hover/press feedback: brighter
/// (and more opaque) on hover, darker on press. Both scale every channel
/// uniformly, which keeps the result premultiplied (still `rgb <= a`) without
/// needing to unpremultiply first -- scaling all four channels by the same
/// factor preserves that invariant exactly.
fn shift_button_color(base: [f32; 4], state: ButtonState) -> [f32; 4] {
    let factor: f32 = match state {
        ButtonState::Hover => 1.35,
        ButtonState::Pressed => 0.6,
    };
    let scale = |c: f32| (c * factor).clamp(0.0, 1.0);
    [
        scale(base[0]),
        scale(base[1]),
        scale(base[2]),
        scale(base[3]),
    ]
}

/// The `_NET_WM_STATE`-bearing attributes last pushed to a managed X11 window.
///
/// `sync_window_to_scene` re-pushes a window's full client state on every sync —
/// including every pointer-motion frame of an interactive move/resize — and each
/// wlroots `wlr_xwayland_surface_set_*` writes its atom and schedules an xwm
/// flush unconditionally, so without a memo a single drag issues dozens of
/// identical property writes per second (review finding #6). This is that memo:
/// the seam skips a state atom whose value has not changed since it was last
/// pushed. Geometry (position/size) is deliberately *not* memoized here — it
/// legitimately changes on the very frames the drag produces — so it stays an
/// unconditional configure. `activated`/`maximized`/`fullscreen` are owned by
/// [`configure`](Wayland::configure); `minimized` by
/// [`set_minimized`](Wayland::set_minimized).
///
/// The memo tracks the *last value pushed*, not the surface's live atom state,
/// so any future path that changes one of these on the surface outside these two
/// seams must invalidate the window's entry (drop it) or the next matching sync
/// will be suppressed as a redundant no-op.
///
/// The never-pushed marker lives on [`X11Backend::pushed`] (`None` until the
/// first push forces every atom write), not here: this struct only ever
/// holds post-push values.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
struct X11PushedState {
    minimized: bool,
    activated: bool,
    maximized: bool,
    fullscreen: bool,
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
    /// Which model window each managed X11 (Xwayland) surface backs, and
    /// what was last pushed to it — the second half of the [`SurfaceKey`]
    /// generalization (M2). A model `WindowId` is backed by *either* an xdg
    /// toplevel (the maps above) or a managed X11 surface (this map), never
    /// both. Each value carries its surface id plus its pushed-state memo
    /// ([`X11Backend`]); the reverse (surface → window) is not needed here
    /// — `state.rs`'s Xwayland handlers keep their own
    /// `XwaylandSurfaceId → WindowId` side-table for that direction.
    /// Keyed by `WindowId` (never reused, and a remap mints a fresh row),
    /// and dropped in [`forget`](Wayland::forget) with the rest of the
    /// window's state.
    x11_backends: HashMap<WindowId, X11Backend>,
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

    /// Record that managed X11 surface `surface` backs model window `id` — the
    /// Xwayland counterpart of [`bind`](Wayland::bind) (M2). A window is bound
    /// through exactly one of the two.
    pub fn bind_x11(&mut self, id: WindowId, surface: wlr::XwaylandSurfaceId) {
        self.x11_backends
            .entry(id)
            .and_modify(|b| b.sid = surface)
            .or_insert_with(|| X11Backend {
                sid: surface,
                pushed: None,
            });
    }

    /// Drop every trace of model window `id`.
    ///
    /// Idempotent: forgetting a window that was never bound is normal, since
    /// model windows created by tests have no toplevel at all.
    pub fn forget(&mut self, id: WindowId) {
        if let Some(key) = self.window_to_toplevel.remove(&id) {
            self.toplevel_to_window.remove(&key);
        }
        self.x11_backends.remove(&id);
        self.remove_ssd(id);
    }

    pub fn window_for(&self, toplevel: ToplevelKey) -> Option<WindowId> {
        self.toplevel_to_window.get(&toplevel).copied()
    }

    pub fn toplevel_for(&self, id: WindowId) -> Option<ToplevelKey> {
        self.window_to_toplevel.get(&id).copied()
    }

    /// Which client surface backs `id`, or `None` for a model-only window
    /// (every window in a test that never bound one). Derived from
    /// [`backend`](Wayland::backend) so the focus path dispatches through
    /// the same single point as the window-op seams; retained as a
    /// `SurfaceKey` because the focus path (`focus_key` / `apply_focus_key`)
    /// and its `None`-means-clear fallback have no backend object to hang
    /// on.
    fn surface_key(&self, id: WindowId) -> Option<SurfaceKey> {
        self.backend(id).map(|b| match b {
            AnyWindowBackend::Xdg(x) => SurfaceKey::Xdg(x.toplevel),
            AnyWindowBackend::X11(x) => SurfaceKey::X11(x.sid),
        })
    }

    /// Whether `id` has a client behind it — xdg or X11. `false` for
    /// model-only windows.
    pub fn is_backed(&self, id: WindowId) -> bool {
        self.window_to_toplevel.contains_key(&id) || self.x11_backends.contains_key(&id)
    }

    /// Resolve `id` to its owned backend object, or `None` for a model-only
    /// window. This is the single dispatch point for the six window-op
    /// seams below (configure, position, visible, minimized, raise, close)
    /// plus the SSD factories — and shared with [`surface_key`](Wayland::surface_key),
    /// so the focus path resolves through it too. Each caller resolves
    /// once, then calls trait methods, instead of matching per method. `X11Backend` travels by
    /// value (a surface id plus four bools); mutating calls write it back
    /// so the memo persists.
    fn backend(&self, id: WindowId) -> Option<AnyWindowBackend> {
        if let Some(key) = self.window_to_toplevel.get(&id) {
            return Some(AnyWindowBackend::Xdg(XdgBackend { toplevel: *key }));
        }
        self.x11_backends
            .get(&id)
            .cloned()
            .map(AnyWindowBackend::X11)
    }

    /// Write a mutated backend back into the map — but only when the window
    /// still has a row there. `forget` may have dropped the entry between
    /// the resolve and this call, and resurrecting it would leak a backend
    /// for a dead window.
    fn persist_backend(&mut self, id: WindowId, backend: AnyWindowBackend) {
        if let (
            std::collections::hash_map::Entry::Occupied(slot),
            AnyWindowBackend::X11(x11),
        ) = (self.x11_backends.entry(id), backend)
        {
            *slot.into_mut() = x11;
        }
    }

    /// The runtime plus `id`'s backend, or `None` on any miss (no runtime,
    /// no binding). The one resolver for the five dispatching seams below.
    fn resolved_backend(&self, id: WindowId) -> Option<(wlr::Runtime, AnyWindowBackend)> {
        Some((self.runtime().cloned()?, self.backend(id)?))
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
        &mut self,
        id: WindowId,
        content: Rectangle,
        activated: bool,
        maximized: bool,
        fullscreen: bool,
    ) {
        let Some((runtime, mut backend)) = self.resolved_backend(id) else {
            return;
        };
        backend.configure(&runtime, content, activated, maximized, fullscreen);
        self.persist_backend(id, backend);
    }

    /// Move the window's scene node. `x`/`y` are **content**-space.
    pub fn set_position(&self, id: WindowId, x: i32, y: i32) {
        let Some((runtime, backend)) = self.resolved_backend(id) else {
            return;
        };
        backend.set_position(&runtime, x, y);
    }

    /// Show or hide the window's scene node.
    ///
    /// Hiding rather than unmapping, which is the distinction the smithay
    /// implementation's `Space::unmap_elem` blurred: a window on an inactive
    /// workspace keeps its buffer and its configure state and is simply not
    /// drawn, so returning to that workspace does not make the client
    /// re-render from nothing.
    pub fn set_visible(&self, id: WindowId, visible: bool) {
        let Some((runtime, backend)) = self.resolved_backend(id) else {
            return;
        };
        backend.set_visible(&runtime, visible);
    }

    /// Reflect the model's minimized state back to the client.
    ///
    /// xdg-shell has no minimized toplevel state to push (a client requests
    /// minimize; the WM just hides the window), so this is a no-op for an xdg
    /// toplevel — hiding is already carried by [`Self::set_visible`]. X11 is
    /// different: `_NET_WM_STATE_HIDDEN` is a real window property an ICCCM/
    /// EWMH client reads to know it has been iconified, so a WM-initiated
    /// minimize must reach the surface through `set_xwayland_surface_minimized`
    /// (which the xwm turns into the `_NET_WM_STATE_HIDDEN` atom) or an X11 app
    /// never learns it was minimized. Silent no-op on a miss, like every other
    /// seam here.
    pub fn set_minimized(&mut self, id: WindowId, minimized: bool) {
        let Some((runtime, mut backend)) = self.resolved_backend(id) else {
            return;
        };
        backend.set_minimized(&runtime, minimized);
        self.persist_backend(id, backend);
    }

    /// Raise the window above its siblings.
    ///
    /// Separate from `set_position` because `behavior.raise_on_focus` decides
    /// whether a focus change alone may restack (ledger item 28): with it
    /// off, a focused window is still activated and configured, it just keeps
    /// its place in the stack.
    pub fn raise(&self, id: WindowId) {
        let Some((runtime, backend)) = self.resolved_backend(id) else {
            return;
        };
        backend.raise(&runtime, self.ssd.get(&id));
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
        let Some(runtime) = self.runtime() else {
            return;
        };
        Self::apply_focus_key(runtime, self.focus_key(id));
    }

    /// The focus target `id` resolves to — the value form of `surface_key`,
    /// exposed so `State::change_keyboard_focus` can run the same dispatch
    /// inside its IME-active transition guard without duplicating the match.
    pub(crate) fn focus_key(&self, id: Option<WindowId>) -> Option<SurfaceKey> {
        id.and_then(|id| self.surface_key(id))
    }

    /// Run the `keyboard_focus` dispatch against an explicit runtime: the
    /// mechanism `keyboard_focus` uses, factored out so the `State` focus
    /// helper shares it instead of reimplementing the fallback-to-clear on
    /// a refused focus.
    pub(crate) fn apply_focus_key(runtime: &wlr::Runtime, key: Option<SurfaceKey>) {
        match key {
            Some(SurfaceKey::Xdg(key)) => {
                if runtime.focus_toplevel_keyboard(key.0).is_none() {
                    runtime.clear_keyboard_focus();
                }
            }
            Some(SurfaceKey::X11(sid)) => {
                if runtime.focus_xwayland_surface_keyboard(sid).is_none() {
                    runtime.clear_keyboard_focus();
                }
            }
            None => runtime.clear_keyboard_focus(),
        }
    }

    /// Unconstrain `popup` against `constraint` and answer its
    /// `xdg_surface` with a configure.
    ///
    /// `constraint` is in the **root toplevel/layer surface's own coordinate
    /// system**, not layout or output space -- `wlr_xdg_popup_unconstrain_from_box`'s
    /// own header says so, and contract ruling R7 restates it. The caller
    /// ([`crate::state::State::popup_constraint_box`]) does the translation;
    /// this method only converts the model's `Rectangle` to the library's box.
    ///
    /// `false` on a miss (no runtime, popup gone, or the popup's surface not
    /// `initialized` yet, in which case the library skips the configure rather
    /// than tripping wlroots' own assert -- see contract §1.2's
    /// `Popup::send_configure`).
    pub(crate) fn configure_popup(&self, popup: PopupKey, constraint: Rectangle) -> bool {
        let Some(runtime) = self.runtime.as_ref() else {
            return false;
        };
        runtime.configure_popup(
            popup.0,
            &wlr::Box2D::new(
                constraint.x,
                constraint.y,
                constraint.width,
                constraint.height,
            ),
        )
    }

    /// Where `popup` currently *is*, in its chain root's surface coordinates:
    /// `wlr_xdg_popup_get_toplevel_coords` for the origin,
    /// `wlr_xdg_popup_state.geometry` for the size.
    ///
    /// Committed state, not scheduled: between a configure and the client's
    /// ack this still names the old position, which is exactly right for
    /// hit-testing -- a popup keeps taking clicks where it is drawn.
    pub(crate) fn popup_geometry(&self, popup: PopupKey) -> Option<Rectangle> {
        let runtime = self.runtime.as_ref()?;
        let handle = runtime.popup(popup.0)?;
        let (x, y) = handle.toplevel_coords(0, 0);
        let geometry = handle.geometry();
        Some(Rectangle {
            x,
            y,
            width: geometry.width,
            height: geometry.height,
        })
    }

    /// Whether the client asked for its popup to be re-unconstrained whenever
    /// the parent moves (`xdg_positioner.set_reactive`).
    pub(crate) fn popup_is_reactive(&self, popup: PopupKey) -> bool {
        self.runtime
            .as_ref()
            .and_then(|runtime| runtime.popup(popup.0))
            .is_some_and(|handle| handle.is_reactive())
    }

    /// Whether the client sent `xdg_popup.grab` for this popup.
    ///
    /// `#[allow(dead_code)]`: unlike its five neighbours this one has no
    /// `state.rs` caller. wlroots owns the popup grab's whole lifetime
    /// (contract §1.6), so the compositor's policy code asks
    /// [`Self::has_explicit_grab`] -- "is *some* grab up?" -- rather than
    /// per-popup. It is kept as the per-popup read the seam owes the model
    /// for popup-level grab decisions (e.g. which chain to dismiss first).
    #[allow(dead_code)]
    pub(crate) fn popup_is_grabbing(&self, popup: PopupKey) -> bool {
        self.runtime
            .as_ref()
            .is_some_and(|runtime| runtime.popup_is_grabbing(popup.0))
    }

    /// Send `xdg_popup.popup_done` to `popup` and, deepest-first, to every
    /// popup under it. Returns how many were dismissed (`0` on any miss).
    ///
    /// Called from `State::dismiss_popups_of_hidden_roots` -- see that method
    /// for why the compositor closes a menu whose parent it has just hidden.
    pub(crate) fn dismiss_popup(&self, popup: PopupKey) -> usize {
        self.runtime
            .as_ref()
            .map_or(0, |runtime| runtime.dismiss_popup(popup.0))
    }

    /// Whether *some* explicit seat grab is in force right now -- an
    /// xdg-popup grab or a drag-and-drop grab.
    ///
    /// The compositor never installs one of these itself: wlroots owns the
    /// popup grab's whole lifetime (contract §1.6). This is the read that
    /// tells `sync_seat_focus` to keep its hands off the seat's keyboard
    /// while one is up.
    pub(crate) fn has_explicit_grab(&self) -> bool {
        self.runtime
            .as_ref()
            .is_some_and(wlr::Runtime::seat_has_explicit_grab)
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
        let Some(backend) = self.backend(id) else {
            return false;
        };
        if let Some(runtime) = self.runtime() {
            backend.close(runtime);
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
        let Some(runtime) = self.runtime() else {
            return;
        };
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
    /// `title_px` borrows the caller's rasterized title; `None` means the
    /// title rasterized to nothing (an empty title, or no font on this
    /// machine that can shape it) and the band is shown bare -- the spec's
    /// error-handling rule that a decoration degrades rather than failing
    /// the window. Its pixels are uploaded only when its `generation`
    /// differs from what the title node already holds, so an unchanged title
    /// costs a comparison rather than a texture upload.
    ///
    /// `button_glyph_px`, same order as `button_colors`: each slot's
    /// rasterized icon (or `None`, degrading that one button to a bare
    /// color chip -- the same per-node degrade rule `title_px` follows).
    /// Uploaded only when the `(width, height, fg)` it carries differs from
    /// what that button's node already holds
    /// (`SsdVisual::button_glyph_keys`).
    ///
    /// `active_button` names the one button, if any, currently hovered or
    /// pressed (`decoration`'s own hit-test order: 0 minimize, 1 maximize, 2
    /// close) and shifts only that button's color -- brighter on hover,
    /// darker on press (`shift_button_color`) -- leaving the glyph node
    /// itself untouched, since color is a rect property and the glyph
    /// buffer just rides on top of it.
    ///
    /// Removes (rather than merely hides) every node when `ssd` is `false`
    /// or the window isn't currently visible: a hidden decoration is cheaper
    /// to recreate than to keep, and unlike the old root-rect scheme there is
    /// no stacking-order reason to keep it around invisible.
    // Eleven arguments, deliberately: every one of them is a fact the caller
    // (`State::sync_window_to_scene`) already has and this seam must not
    // re-derive -- the model's geometry, its palette, and its title/glyph
    // pixels. Bundling them into a struct would only move the same fields
    // one line up at the single call site, while making the "no model types
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
        title_px: Option<TitleRaster<'_>>,
        button_glyph_px: [Option<GlyphRaster<'_>>; 3],
        active_button: Option<(usize, ButtonState)>,
    ) {
        if !ssd || !visible {
            self.remove_ssd(id);
            return;
        }
        let Some(runtime) = self.runtime.clone() else {
            return;
        };
        // Which client backs this window decides both where the decoration
        // nodes are parented and what origin their coordinates are relative
        // to. An xdg toplevel's SSD lives inside its own toplevel tree, so its
        // nodes are positioned relative to the content origin
        // (`set_toplevel_position` moves that tree). An X11 window's scene
        // node lives directly in `Band::Toplevel`, so its decoration nodes are
        // band siblings positioned in absolute scene coordinates (origin
        // `(0, 0)`), riding the window's z-order via `raise` instead of a
        // parent tree.
        let Some(backend) = self.backend(id) else {
            return;
        };
        let (ox, oy) = backend.ssd_origin(content.x, content.y);
        // Node factories that hide the xdg-tree vs. toplevel-band split so the
        // decoration-building logic below is one path for both surface kinds.
        let make_ssd_rect = |w: i32, h: i32, color: [f32; 4]| -> Option<wlr::RectId> {
            backend.make_ssd_rect(&runtime, w, h, color)
        };
        let make_ssd_buffer = |w: i32, h: i32, px: &[u8]| -> Option<wlr::BufferId> {
            backend.make_ssd_buffer(&runtime, w, h, px)
        };
        let width = bar.width.max(1);
        let height = bar.height.max(1);
        let (rel_x, rel_y) = (bar.x - ox, bar.y - oy);

        // The band is the entry: no band, no decoration. It is also created
        // first so that every node added below lands above it in the
        // tree's own stacking order -- buttons and title over the band, never
        // under it.
        //
        // Not the `entry` API the `map_entry` lint suggests: the value is
        // fallible to build (`make_ssd_rect` can return `None`, on which this must
        // early-return having inserted nothing), which `or_insert_with` cannot
        // express.
        #[allow(clippy::map_entry)]
        if !self.ssd.contains_key(&id) {
            let Some(band) = make_ssd_rect(width, height, band_color) else {
                return;
            };
            self.ssd.insert(
                id,
                SsdVisual {
                    band,
                    title: None,
                    title_generation: None,
                    buttons: [None; 3],
                    button_glyphs: [None; 3],
                    button_glyph_keys: [None; 3],
                },
            );
        }
        let Some(visual) = self.ssd.get_mut(&id) else {
            return;
        };

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
            let color = match active_button {
                Some((hi, state)) if hi == i => shift_button_color(button_colors[i], state),
                _ => button_colors[i],
            };
            let slot = &mut visual.buttons[i];
            if slot.is_none() {
                *slot = make_ssd_rect(r.width.max(1), r.height.max(1), color);
            }
            let Some(rect) = *slot else { continue };
            runtime.set_rect_size(rect, r.width.max(1), r.height.max(1));
            runtime.set_rect_position(rect, r.x - ox, r.y - oy);
            runtime.set_rect_color(rect, color);

            match &button_glyph_px[i] {
                Some(raster) => {
                    let raster_key = (raster.width, raster.height, raster.fg);
                    if visual.button_glyphs[i].is_none()
                        || visual.button_glyph_keys[i] != Some(raster_key)
                    {
                        let updated = visual.button_glyphs[i].and_then(|buffer| {
                            runtime.update_buffer(
                                buffer,
                                raster.width,
                                raster.height,
                                raster.pixels,
                            )
                        });
                        if updated.is_none() {
                            if let Some(stale) = visual.button_glyphs[i].take() {
                                runtime.remove_buffer(stale);
                            }
                            visual.button_glyphs[i] =
                                make_ssd_buffer(raster.width, raster.height, raster.pixels);
                        }
                        visual.button_glyph_keys[i] = if visual.button_glyphs[i].is_some() {
                            Some(raster_key)
                        } else {
                            None
                        };
                    }
                    if let Some(buffer) = visual.button_glyphs[i] {
                        runtime.set_buffer_position(buffer, r.x - ox, r.y - oy);
                    }
                }
                None => {
                    if let Some(buffer) = visual.button_glyphs[i].take() {
                        runtime.remove_buffer(buffer);
                    }
                    visual.button_glyph_keys[i] = None;
                }
            }
        }

        match title_px {
            Some(raster) => {
                // The upload is skipped outright when this node already
                // holds these exact pixels (M2); only the node's *position*
                // is re-set below, which is what a plain move needs.
                if visual.title.is_none() || visual.title_generation != Some(raster.generation) {
                    // `update_buffer` rather than remove-and-re-add: it
                    // exists for exactly this (a re-titled or resized
                    // window), and it keeps the node's place in the stacking
                    // order instead of re-adding it on top of whatever was
                    // added since. A `None` from it means the id went stale
                    // (its parent toplevel was torn down), so the node is
                    // dropped and rebuilt.
                    let updated = visual.title.and_then(|buffer| {
                        runtime.update_buffer(buffer, raster.width, raster.height, raster.pixels)
                    });
                    if updated.is_none() {
                        if let Some(stale) = visual.title.take() {
                            runtime.remove_buffer(stale);
                        }
                        visual.title = make_ssd_buffer(raster.width, raster.height, raster.pixels);
                    }
                    // Only claim the generation if a node actually holds it:
                    // a refused `add_buffer_in_toplevel` must be retried on
                    // the next sync, not remembered as up to date.
                    visual.title_generation = visual.title.map(|_| raster.generation);
                }
                if let Some(buffer) = visual.title {
                    runtime.set_buffer_position(buffer, rel_x, rel_y);
                }
            }
            None => {
                if let Some(buffer) = visual.title.take() {
                    runtime.remove_buffer(buffer);
                }
                visual.title_generation = None;
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
        let Some(visual) = self.ssd.remove(&id) else {
            return;
        };
        let Some(runtime) = self.runtime() else {
            return;
        };
        runtime.remove_rect(visual.band);
        if let Some(buffer) = visual.title {
            runtime.remove_buffer(buffer);
        }
        for rect in visual.buttons.into_iter().flatten() {
            runtime.remove_rect(rect);
        }
        for buffer in visual.button_glyphs.into_iter().flatten() {
            runtime.remove_buffer(buffer);
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
        assert_eq!(
            w.window_for(key),
            None,
            "the reverse map must be cleared too"
        );
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
        let frame = Rectangle {
            x: 100,
            y: 200,
            width: 400,
            height: 300,
        };
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
        let frame = Rectangle {
            x: 100,
            y: 200,
            width: 400,
            height: 300,
        };
        let bar = crate::decoration::title_bar_rect(frame);
        let content = crate::decoration::content_rect(frame, true);
        let color = [1.0, 1.0, 1.0, 1.0];

        w.sync_ssd(
            id,
            true,
            true,
            bar,
            content,
            color,
            [[0.0; 4]; 3],
            None,
            [None, None, None],
            None,
        );
        assert_eq!(w.ssd_rect_count(), 0);

        w.sync_ssd(
            id,
            false,
            true,
            bar,
            content,
            color,
            [[0.0; 4]; 3],
            None,
            [None, None, None],
            None,
        );
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
        let frame = Rectangle {
            x: 0,
            y: 0,
            width: 400,
            height: 300,
        };
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
            Some(TitleRaster {
                width: bar.width,
                height: bar.height,
                generation: 1,
                pixels: &px,
            }),
            [None, None, None],
            Some((2, ButtonState::Hover)),
        );
        assert_eq!(w.ssd_rect_count(), 0);
    }

    /// The button rects `sync_ssd` positions come from
    /// `decoration::button_rects` applied to the *bar*, not the frame -- the
    /// two must agree, since `hit_test` (which answers clicks) reads the
    /// frame while the scene nodes are placed from the bar.
    #[test]
    fn button_rects_agree_whether_derived_from_the_frame_or_the_bar() {
        let frame = Rectangle {
            x: 100,
            y: 200,
            width: 400,
            height: 300,
        };
        let bar = crate::decoration::title_bar_rect(frame);
        assert_eq!(
            crate::decoration::button_rects(bar),
            crate::decoration::button_rects(frame)
        );
    }

    /// `PopupKey::for_test` mints distinct, dangling keys, and every seam
    /// method is a silent no-op against a `Wayland` with no runtime attached
    /// -- the property every unit test in `state.rs` relies on and the one
    /// this file's module doc states for all of its outbound methods.
    ///
    /// Mutation check: make `configure_popup` `unwrap()` the runtime instead
    /// of `?`-ing it and this test panics.
    #[test]
    fn popup_keys_are_distinct_and_every_popup_seam_method_no_ops_without_a_runtime() {
        let a = PopupKey::for_test(1);
        let b = PopupKey::for_test(2);
        assert_eq!(a, PopupKey::for_test(1), "the same n gives the same key");
        assert_ne!(a, b, "distinct n gives distinct keys");

        let wayland = Wayland::new();
        assert!(
            !wayland.configure_popup(
                a,
                Rectangle {
                    x: 0,
                    y: 0,
                    width: 100,
                    height: 100
                }
            ),
            "no runtime: configuring a popup reports failure rather than panicking"
        );
        assert_eq!(wayland.popup_geometry(a), None);
        assert!(!wayland.popup_is_reactive(a));
        assert!(!wayland.popup_is_grabbing(a));
        assert_eq!(wayland.dismiss_popup(a), 0);
        assert!(!wayland.has_explicit_grab());
    }

    /// Write-back persistence: `configure` must persist the mutated X11
    /// memo into the map. Mutation check: delete the `persist_backend`
    /// call and the final assertion fails (the map still holds `None`).
    #[test]
    fn configure_persists_the_x11_memo_write_back() {
        let runtime = wlr::Runtime::new().expect("runtime");
        let mut w = Wayland::new();
        w.attach(runtime);
        let id = WindowId(11);
        w.bind_x11(id, wlr::XwaylandSurfaceId::dangling_for_test());
        assert!(
            w.x11_backends.get(&id).map(|b| b.pushed) == Some(None),
            "a fresh binding has never pushed"
        );

        w.configure(
            id,
            Rectangle {
                x: 0,
                y: 0,
                width: 100,
                height: 100,
            },
            true,
            true,
            true,
        );
        assert!(
            w.x11_backends.get(&id).map(|b| b.pushed)
                == Some(Some(X11PushedState {
                    activated: true,
                    maximized: true,
                    fullscreen: true,
                    minimized: false,
                })),
            "configure must write the pushed memo back into the map"
        );
    }

    /// First-sync: a fresh (`None`) memo forces every atom write, so even
    /// an all-false first configure records `Some(all-false)` afterwards —
    /// the old `None`-forces-all behavior, made explicit.
    #[test]
    fn first_sync_with_all_false_still_records_the_memo() {
        let runtime = wlr::Runtime::new().expect("runtime");
        let mut w = Wayland::new();
        w.attach(runtime);
        let id = WindowId(12);
        w.bind_x11(id, wlr::XwaylandSurfaceId::dangling_for_test());

        w.configure(
            id,
            Rectangle {
                x: 0,
                y: 0,
                width: 100,
                height: 100,
            },
            false,
            false,
            false,
        );
        assert!(
            w.x11_backends.get(&id).map(|b| b.pushed) == Some(Some(X11PushedState::default())),
            "even an all-false first sync leaves a Some memo behind"
        );
    }

    /// `ssd_origin` dispatches on the backend kind: an xdg toplevel's
    /// decoration rides its own tree (content origin), an X11 window's
    /// nodes are band siblings in absolute scene coordinates.
    #[test]
    fn ssd_origin_comes_from_the_backend_kind() {
        let xdg = AnyWindowBackend::Xdg(XdgBackend {
            toplevel: ToplevelKey::for_test(1),
        });
        assert_eq!(xdg.ssd_origin(100, 200), (100, 200));

        let x11 = AnyWindowBackend::X11(X11Backend {
            sid: wlr::XwaylandSurfaceId::dangling_for_test(),
            pushed: None,
        });
        assert_eq!(x11.ssd_origin(100, 200), (0, 0));
    }

    /// bind/forget/is_backed/close round-trip on an X11-bound id: backed
    /// once bound, `close` reports "wait for the destroy" while backed
    /// (the dangling surface makes the runtime call a miss, never a
    /// panic), and unbacked once forgotten.
    #[test]
    fn x11_bind_forget_close_round_trip() {
        let runtime = wlr::Runtime::new().expect("runtime");
        let mut w = Wayland::new();
        let id = WindowId(13);
        assert!(!w.is_backed(id));
        assert!(!w.close(id));

        w.bind_x11(id, wlr::XwaylandSurfaceId::dangling_for_test());
        assert!(w.is_backed(id));

        w.attach(runtime);
        assert!(w.close(id), "a bound id reports a live client");

        w.forget(id);
        assert!(!w.is_backed(id));
        assert!(!w.close(id));
    }
}
