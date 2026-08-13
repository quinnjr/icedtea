//! The compositor's own state: `State` owns the window model
//! (`WindowManager`) and configuration, and fans out `contract::Event`s
//! produced by model mutations onto a crossbeam channel for the D-Bus side
//! to consume.
//!
//! This is the model-only shape of `State`, mid-port (wlr-port milestone 1,
//! task 4): every push out to a client goes through `wayland: Wayland`
//! (`wayland.rs`) rather than through smithay's `Space`/`ToplevelSurface`
//! types directly, so this file has no compositor-library dependency at
//! all. `Wayland`'s methods are no-ops until task 5 backs them with `wlr`;
//! until then every mutation here behaves exactly as it does in any test
//! that never binds a toplevel.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use icedtea_config::Config;
use icedtea_contract::{AltTabState, Event, Rectangle, SeqEvent, WindowId};

use crate::input;
use crate::layout::{self, SnapZone};
use crate::render::{self, WallpaperState};
use crate::window::WindowManager;

/// The model's placeholder toplevel size: staged as both the new window's
/// frame geometry and the client's first configure, until the client's own
/// first commit says otherwise. One constant rather than the literal
/// `(640, 400)` repeated at each site (`new_toplevel`'s cascade placement,
/// `initial_commit`'s configure, and the test that asserts on it) so the
/// three/four call sites cannot drift apart from each other.
pub const PLACEHOLDER_SIZE: (i32, i32) = (640, 400);

/// Sentinel [`LayerEntry::output`] value meaning "no output existed at
/// all when this surface was announced" (review finding M5). `u32::MAX`
/// rather than `0`: `next_output_index` starts at `0` and only
/// increments, so `0` is a real, common output index the very first
/// hotplugged output takes, while reaching `u32::MAX` real outputs is not
/// a case this process will ever see -- the same "far outside the real
/// id space" reasoning [`wlr::LayerSurfaceId::dangling_for_test`] uses
/// for its own sentinel.
const NO_OUTPUT: u32 = u32::MAX;

/// Output geometry information (simplified from smithay's `Output`).
pub struct OutputSurface {
    pub geometry: icedtea_contract::Rectangle,
    /// `geometry` shrunk by any exclusive-zone layer surfaces anchored to
    /// this output's edges. Equal to `geometry` when none reserve space.
    /// Maintained solely by [`State::arrange_layers`]; every placement
    /// consumer that means "the space windows may occupy" (tiling,
    /// maximize) reads this instead of `geometry` -- fullscreen is the one
    /// exception, since it covers panels by definition.
    pub usable: icedtea_contract::Rectangle,
}

impl OutputSurface {
    /// Construct with no exclusive zones yet reserved -- `usable` starts
    /// equal to `geometry`, exactly as `create_output` documents.
    pub fn new(geometry: icedtea_contract::Rectangle) -> Self {
        Self { geometry, usable: geometry }
    }
}

/// The model's own record of a wlr-layer-shell surface: everything
/// [`State::arrange_layers`] and [`State::configure_layer`] need, kept in
/// the model rather than re-read from the library's `LayerSurface` handle on
/// every arrangement pass -- the handle only lives for the duration of one
/// handler call (see [`wlr::LayerSurface`]'s own doc), so anything an
/// unrelated later call (a different surface's commit, a hotplug) needs has
/// to be copied out while the handle is live. Updated wholesale on every
/// `layer_surface_commit`, since wlr-layer-shell clients routinely re-anchor
/// or resize their exclusive zone after mapping.
pub struct LayerEntry {
    /// The model output index (`State::outputs`' key) this surface is
    /// placed on. Resolved once, at `new_layer_surface`, from
    /// [`wlr::LayerSurface::output_id`] via `output_ids`, falling back to
    /// `output_for_pointer` -- see `new_layer_surface`'s own doc. May be
    /// [`NO_OUTPUT`], a sentinel meaning "no output existed at all when
    /// this surface was announced" (review finding M5); every reader that
    /// looks it up in `self.outputs` already treats a miss as "nothing to
    /// do yet", which is exactly right for the sentinel too, and
    /// `State::resolve_orphaned_layers` re-homes it the moment any output
    /// exists.
    pub output: u32,
    /// This entry's position in a total, stable order across every layer
    /// surface this compositor has ever announced -- assigned once, at
    /// `new_layer_surface`, from `State::next_layer_sequence`.
    ///
    /// Exists only for [`State::configure_layer`]'s N6 fix: two
    /// same-edge, same-output panels must not draw on top of each other,
    /// which needs a deterministic placement order, and
    /// [`wlr::LayerSurfaceId`] has no such order exposed (`Hash`/`Eq`
    /// only, and its inner value is `pub(crate)` to the `wlr` crate, not
    /// this one) -- so this crate mints its own rather than one it cannot
    /// read.
    pub sequence: u64,
    /// Carried for the model's own record; not consumed by
    /// [`State::arrange_layers`]/[`State::configure_layer`] -- the crate
    /// itself reparents this surface's scene node into the right band
    /// whenever a commit reports a different layer
    /// (`Runtime::reparent_layer_surface_if_changed`), so nothing here has
    /// to act on a change to it.
    pub layer: wlr::Layer,
    pub anchor: wlr::Anchor,
    /// The raw value from [`wlr::LayerSurface::exclusive_zone`]: `0` or
    /// negative means "reserve nothing" (any negative value additionally
    /// asks not to be moved to avoid occlusion, which this compositor's
    /// manual placement has no use for); only a positive value reserves
    /// space in [`State::arrange_layers`].
    pub exclusive: i32,
    /// The client's last-requested size (`0` on either axis means "the
    /// compositor decides" for that axis) -- the input `configure_layer`'s
    /// placement rule reads, not the size that method actually chose.
    pub size: (u32, u32),
    /// Whether this surface currently wants keyboard focus
    /// ([`wlr::LayerSurface::keyboard_interactive`]). `false` until the
    /// surface's first commit populates it (see that accessor's own
    /// timing doc) -- `new_layer_surface` always inserts `false` here.
    pub interactive: bool,
    /// Whether this surface is currently mapped (has a buffer and is on
    /// screen). `false` at `new_layer_surface` (review finding J2: a
    /// surface that never mapped, or that unmapped and is waiting to
    /// remap, must not reserve space); flipped by
    /// `layer_surface_mapped`/`layer_surface_unmapped`.
    /// [`State::arrange_layers`]'s fold skips every `!mapped` entry.
    pub mapped: bool,
    /// The `(width, height, x, y)` [`State::configure_layer`] most
    /// recently computed for this surface, or `None` before its first
    /// call. Set just before the runtime-gated
    /// `configure_layer_surface`/`set_layer_surface_position` calls, not
    /// only after they run, so this is testable without a live
    /// `wlr::Runtime` attached (every unit test in this file) -- in
    /// production a runtime is always attached by the time any handler
    /// runs, so this never records a placement that was not actually put
    /// on the wire. Re-review finding Minor-1's storm guard: `configure_layer`
    /// compares its freshly computed placement against this before doing
    /// anything else, so calling it again with an unchanged input --
    /// which `arrange_layers` now does for every mapped panel on every
    /// pass -- costs one comparison rather than a wire round trip.
    pub last_configured: Option<(u32, u32, i32, i32)>,
    /// The client's requested margin, `(top, right, bottom, left)` --
    /// wlr-layer-shell's own field order. Always `(0, 0, 0, 0)` for now:
    /// task 7 (N8) added this field so the rest of the placement plumbing
    /// has somewhere to read a margin from, but `wlr` 0.20.12's
    /// `LayerSurface` exposes no accessor to actually capture the
    /// client's requested value (`anchor`/`exclusive_zone`/`desired_size`/
    /// `keyboard_interactive` all exist; `margin` does not) -- verified
    /// against both 0.20.11 and 0.20.12's `layer.rs`. Capturing a real
    /// value in `layer_surface_commit` and insetting placement/exclusive
    /// folds by it is deferred until a future `wlr` release adds the
    /// accessor (tracked as a 0.20.13 additive gap); nothing here may
    /// synthesize a margin from anything else in the meantime.
    pub margin: (i32, i32, i32, i32),
}

/// Shrink `rect` by one layer entry's positive exclusive zone along
/// whichever single edge it is anchored to: `top`-anchored (regardless of
/// whether `left`/`right` are also set -- "single edge or
/// edge+both-perpendicular" both carve the same way) shrinks the top,
/// `bottom` the bottom, `left` the left, `right` the right.
/// `exclusive <= 0` reserves nothing, per wlr-layer-shell's own
/// definition (see [`LayerEntry::exclusive`]'s doc), and a surface
/// anchored to both edges of an axis at once (e.g. `top` and `bottom`)
/// has no single edge to carve into and is left unchanged -- the same
/// "nothing sane to do" case [`State::configure_layer`]'s placement rule
/// falls back to centering for.
///
/// A free function rather than a method, and shared by
/// [`State::arrange_layers`] (folding every output's `usable`) and
/// [`State::usable_before`] (a single panel's own placement base): the
/// two calls need to compute an identical fold, and a second
/// hand-written copy of this arithmetic is one more place for them to
/// drift apart.
///
/// H2: every arithmetic op here is saturating. `exclusive` is captured
/// (clamped) at `new_layer_surface`/`layer_surface_commit` before it ever
/// reaches this function, but this function has no way to enforce that on
/// its own -- and it folds over every mapped layer entry on every
/// `arrange_layers`/`usable_before` pass, so a second, defense-in-depth
/// guard here costs nothing and turns "two large-exclusive panels overflow
/// `rect.x`/`rect.y`" from a debug-build panic (release: silent i32
/// wraparound corruption) into a saturated, still-sane rect.
fn fold_exclusive_zone(mut rect: Rectangle, anchor: wlr::Anchor, exclusive: i32) -> Rectangle {
    if exclusive <= 0 {
        return rect;
    }
    // Task 7 (N8/exclusive-edge), corrected post-review (2f04984's Major
    // finding): matches wlroots' own `wlr_layer_surface_v1_get_exclusive_
    // edge`, which is the protocol spec's own rule
    // (`wlr-layer-shell-unstable-v1.xml`'s `set_exclusive_zone` doc,
    // conformance-tested by WLCS's `is_positioned_to_accommodate_other_
    // surfaces_exclusive_zone`): a positive exclusive zone is only
    // meaningful -- reserves space at all -- for exactly two anchor
    // shapes per axis: anchored to **one edge alone** (e.g. `ANCHOR_TOP`
    // with no `left`/`right`), or anchored to **that edge plus both
    // perpendicular edges** (e.g. `TOP | LEFT | RIGHT`, spanning the
    // other axis). Anchored to only two perpendicular edges (a corner,
    // e.g. `TOP | LEFT`), only two parallel edges (e.g. `LEFT | RIGHT`
    // with neither `top` nor `bottom`), or all four edges: reserves
    // nothing, same as the protocol's own "treated the same as zero"
    // wording.
    //
    // `left == right` below means "both set, or both unset" -- i.e.
    // either the perpendicular axis fully spans (the second legal shape)
    // or is not anchored at all (the first legal shape); a corner like
    // `TOP | LEFT` has `left=true, right=false`, `left == right` is
    // `false`, and correctly falls through to no reservation.
    //
    // 2f04984 first tightened this from the pre-task-7 rule
    // (`anchor.top != anchor.bottom`, which correctly matched single-edge
    // but *also* wrongly matched corners) straight to requiring both
    // perpendicular edges unconditionally -- fixing the corner
    // over-reservation but silently dropping the single-edge-alone case
    // to zero reservation, a real regression for the single most common
    // panel shape (a bar anchored to one edge, spanning nothing else).
    // No test caught it: every exclusive-zone test until this fix used
    // `TOP | LEFT | RIGHT` (`top_panel_entry`). See
    // `a_single_edge_only_anchor_still_reserves_its_exclusive_zone` and
    // `a_corner_anchored_exclusive_zone_reserves_nothing`, which now both
    // pass against this rule.
    let (left, right, top, bottom) = (anchor.left, anchor.right, anchor.top, anchor.bottom);
    if top && !bottom && (left == right) {
        rect.y = rect.y.saturating_add(exclusive);
        rect.height = rect.height.saturating_sub(exclusive).max(0);
    } else if bottom && !top && (left == right) {
        rect.height = rect.height.saturating_sub(exclusive).max(0);
    } else if left && !right && (top == bottom) {
        rect.x = rect.x.saturating_add(exclusive);
        rect.width = rect.width.saturating_sub(exclusive).max(0);
    } else if right && !left && (top == bottom) {
        rect.width = rect.width.saturating_sub(exclusive).max(0);
    }
    rect
}

/// H2: bound a client-controlled exclusive zone to something sane before it
/// is stored, so two panels with pathological `exclusive_zone` requests (a
/// hostile or buggy client can request anything up to `i32::MAX`) cannot
/// together carve past an output's own extent -- the belt to
/// `fold_exclusive_zone`'s saturating-arithmetic suspenders. Clamped
/// against `max(output.width, output.height)`, since a zone only ever
/// folds a single axis and either axis's whole span is already generous
/// headroom for a real panel; `None` (no output resolved yet, e.g. the
/// [`NO_OUTPUT`] sentinel) leaves the value unclamped here and relies
/// entirely on the saturating fold. `exclusive <= 0` ("reserve nothing",
/// see [`LayerEntry::exclusive`]'s doc) passes through untouched -- only
/// the reservation itself is bounded, not the "don't reserve" sentinel
/// space.
fn clamp_exclusive_zone(exclusive: i32, output: Option<&OutputSurface>) -> i32 {
    if exclusive <= 0 {
        return exclusive;
    }
    let bound = output.map(|o| o.geometry.width.max(o.geometry.height)).unwrap_or(i32::MAX);
    exclusive.min(bound)
}

/// Whether [`State::sync_seat_focus`] must leave the seat's real keyboard
/// focus alone because an interactive layer surface still holds it --
/// review finding J3's guard, extracted as a pure function of `layer_focus`
/// and `layers` so its exact logic is unit-testable without a live
/// `wlr::Runtime` (`sync_seat_focus`'s tail, `wayland.keyboard_focus`, is a
/// no-op with none attached, which would make every branch of the guard
/// look identical to a test that could only observe side effects on
/// `wayland`).
///
/// `true` only when `layer_focus` names an entry that is still `mapped`:
/// a stale `layer_focus` (the entry unmapped or was removed without
/// going through `layer_surface_unmapped`/`destroyed` -- defensively,
/// should not happen, but this is the one guard standing between that and
/// a permanently dead keyboard) must not block the model's own focus from
/// ever being asserted again.
/// One `lower_*_to_bottom` call [`State::sync_wallpaper_nodes`] issues, in
/// the order it issues them.
///
/// Extracted (with [`wallpaper_lower_plan`]) purely so review finding C1's
/// ordering contract is *assertable*: `wlr` exposes no scene z-query, and
/// `BufferId`/`RectId` have no `dangling_for_test` constructor, so neither
/// the resulting stacking order nor the ids involved can be observed from a
/// test. What can be pinned is the sequence of lower calls the sync will
/// make, which is where the whole bug lived.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LowerStep {
    /// The freshly created wallpaper node at this index in the caller's
    /// created-in-this-pass list.
    Wallpaper(usize),
    /// The boot-time full-output background rect (`lib.rs::run()`).
    Background,
}

/// The order in which [`State::sync_wallpaper_nodes`] must lower the
/// `created` wallpaper nodes it just made, plus the background rect.
///
/// **The contract: the background rect is lowered LAST.** `lower_*_to_bottom`
/// moves its target to the very bottom of the root's children, so the last
/// call wins the bottom. Lowering a wallpaper node last (what the code did
/// before finding C1) buried it under the opaque background rect and the
/// wallpaper was never visible; lowering the background last leaves it at
/// the very bottom with the wallpaper immediately above it and everything
/// else above that.
///
/// Empty when nothing was created: an existing node is already correctly
/// stacked, and re-lowering the background for a pass that changed nothing
/// would be pure churn.
fn wallpaper_lower_plan(created: usize, has_background: bool) -> Vec<LowerStep> {
    if created == 0 {
        return Vec::new();
    }
    let mut plan: Vec<LowerStep> = (0..created).map(LowerStep::Wallpaper).collect();
    if has_background {
        plan.push(LowerStep::Background);
    }
    plan
}

/// One axis of a layer surface's placement: its size along that axis and
/// its position, from the two anchors on that axis, the client's
/// `desired` size (0 = "compositor decides"), and the usable box's
/// `origin`/`span` on that axis.
///
/// The wlr-layer-shell rule, per axis (review finding I3):
///
/// | anchors on the axis | size                        | position                          |
/// |---------------------|-----------------------------|-----------------------------------|
/// | both edges          | span the box                | box origin                        |
/// | exactly one edge    | `desired`, else 30px        | flush against the anchored edge   |
/// | neither edge        | `desired`, else span the box| centered (origin when it spans)   |
///
/// The three fallbacks are each the protocol's own answer to a 0 the
/// client left for the compositor to choose: an axis anchored to both
/// edges *must* span it (a 0 there is not merely allowed but expected);
/// an axis anchored to exactly one edge is a panel's thickness, where 30px
/// is this compositor's house default; an axis anchored to neither with no
/// desired size is the protocol's fill case (all-four-anchors and
/// zero-anchors lockers/launchers alike) and gets the whole usable box
/// rather than an arbitrary 200px.
///
/// Worked examples: a `TOP|LEFT|RIGHT` 800x30 panel gets `(800, box.x)`
/// horizontally (both edges) and `(30, box.y)` vertically (one edge,
/// desired honored) -- unchanged from before. A `TOP|RIGHT` 300x100
/// notification gets `(300, box.x + box.width - 300)` and `(100, box.y)`:
/// its own size, flush into the corner. A four-edge-anchored 0x0 locker
/// gets the whole box on both axes.
fn layer_axis_placement(start: bool, end: bool, desired: u32, origin: i32, span: i32) -> (i32, i32) {
    // Finding 7, security: `desired` is client-controlled and otherwise
    // casts straight to `i32` -- a value at or past `i32::MAX` would wrap
    // negative on the cast, handing a negative "size" into the arithmetic
    // below. Clamped to the output's own span first: no legitimate panel
    // needs to claim more than the box it is being placed in.
    let desired = desired.min(span.max(0) as u32);
    let size = if start && end {
        span
    } else if desired != 0 {
        desired as i32
    } else if start || end {
        30
    } else {
        span
    };
    let pos = if start || (start == end && size >= span) {
        origin
    } else if end {
        origin + span - size
    } else {
        origin + (span - size) / 2
    };
    (size, pos)
}

fn layer_holds_keyboard_focus(
    layer_focus: Option<wlr::LayerSurfaceId>,
    layers: &HashMap<wlr::LayerSurfaceId, LayerEntry>,
) -> bool {
    layer_focus.is_some_and(|id| layers.get(&id).is_some_and(|entry| entry.mapped))
}

/// Finding 6, testing: the client's `wlr::Edges` -> `input::ResizeEdges`
/// mapping [`State::begin_client_resize`] uses, extracted as a pure
/// field-by-field translation so it is unit-testable without a live
/// `wlr::Runtime` -- `begin_client_resize`'s own success path (a pointer
/// actually positioned over the window, `pointer_pressed` true) has no
/// headless-runtime way to inject a pointer position at all, so this
/// mapping is what stays testable of it.
fn resize_edges_from_wlr(edges: wlr::Edges) -> input::ResizeEdges {
    input::ResizeEdges { top: edges.top, bottom: edges.bottom, left: edges.left, right: edges.right }
}

/// Translate the compositor library's modifier booleans into the model's
/// bitflags.
///
/// A free function taking four `bool`s rather than a method on
/// `wlr::Modifiers`, because that type belongs to another crate and this
/// mapping is the compositor's own decision -- and because a pure function is
/// the only part of the key path that can be unit-tested at all: a
/// `wlr::KeyEvent` cannot be constructed outside a live seat.
///
/// `logo` is the Super / Windows key, which is what this project's `SUPER`
/// binding token means (`config/src/defaults.rs`).
pub fn to_model_modifiers(logo: bool, ctrl: bool, alt: bool, shift: bool) -> input::Modifiers {
    let mut out = input::Modifiers::empty();
    if logo {
        out |= input::Modifiers::SUPER;
    }
    if ctrl {
        out |= input::Modifiers::CTRL;
    }
    if alt {
        out |= input::Modifiers::ALT;
    }
    if shift {
        out |= input::Modifiers::SHIFT;
    }
    out
}

/// Whether an in-progress alt-tab session should end on this key event.
///
/// `watched` is the modifier flags [`input::modifiers_for_tokens`] derives
/// from the configured `cycle:alt_tab` binding -- not hardcoded to SUPER, so
/// a rebind (e.g. to ALT+Tab) still ends its session on the right key.
///
/// Two independent checks, not one, because neither alone is enough:
///
/// - `keysym_is_modifier(watched, keysym)` on a *release* (`!pressed`)
///   catches the modifier's own release **on that very event**. This is the
///   one that matters: wlroots' `keyboard_key_update` emits the `key` signal
///   *before* it calls `xkb_state_update_key`/`keyboard_modifier_update`
///   (`types/wlr_keyboard.c`), so `event.modifiers()` on the modifier key's
///   own release event still reports that modifier held -- `mods` cannot
///   see its own key going up. Only the keysym can.
/// - `!mods.contains(watched)` is the fallback for every event *after*
///   that one (or for any case the keysym check doesn't cover): once the
///   library's modifier state has actually updated, a session that somehow
///   missed its modifier's release event still ends on the very next key,
///   rather than lingering until an unrelated later sync.
///
/// A free function taking plain values rather than `&wlr::KeyEvent`, for the
/// same reason `to_model_modifiers` is one: `wlr::KeyEvent::new` is
/// `pub(crate)` to the `wlr` crate, so no test outside it can construct a
/// real one, and this is the only shape of the decision a test *can* drive.
pub fn alt_tab_should_end(watched: input::Modifiers, mods: input::Modifiers, pressed: bool, keysym: u32) -> bool {
    let modifier_released_this_event = !pressed && input::keysym_is_modifier(watched, keysym);
    modifier_released_this_event || !mods.contains(watched)
}

/// Input passed to `State::handle_pointer`, in output logical coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerEvent {
    /// Button pressed while the pointer is over `id`.
    Press { id: WindowId, pointer: (i32, i32) },
    /// Pointer moved during an in-progress drag.
    Motion { pointer: (i32, i32) },
    /// Button released, ending an in-progress drag (a no-op if none is
    /// active).
    Release { pointer: (i32, i32) },
}

pub struct State {
    pub window_manager: WindowManager,
    pub config: Config,
    /// Outbound event channel. Every message carries the `seq` its mutation
    /// produced (review finding I2) so a subscriber can order signals
    /// against a `GetState()` snapshot and detect gaps.
    pub dbus_tx: crossbeam_channel::Sender<SeqEvent>,
    /// The compositor's Wayland side -- the one seam between this model and
    /// a client. See `wayland.rs`'s module doc.
    pub wayland: crate::wayland::Wayland,
    pub start_time: Instant,
    /// Wallpaper decode state (`render::spawn_wallpaper_decode` produces it;
    /// there is no renderer in this crate to upload or draw it yet).
    pub wallpaper: WallpaperState,
    /// The drag machine's current snap-preview target, in output logical
    /// coordinates, or `None` when no drag is in a snap-preview state. Set
    /// by the drag machine (Task 9); this is the geometry hook `render.rs`
    /// consumes.
    pub snap_preview: Option<icedtea_contract::Rectangle>,
    /// The scene rect node backing `snap_preview`, or `None` when no drag is
    /// showing one. Reconciled by `sync_snap_preview`, which must run
    /// immediately after every `snap_preview` assignment -- see that
    /// method's doc.
    snap_preview_rect: Option<wlr::RectId>,
    /// Output geometries keyed by output index. Used by fullscreen toggle.
    pub outputs: HashMap<u32, OutputSurface>,
    /// The next index `new_output` hands out. Monotonic, never reused --
    /// `outputs.len()` was tried first and is wrong the moment an output is
    /// removed and a new one added: `len()` computes the same index the
    /// removed output had, colliding with whatever the model (or a
    /// client-facing consumer) still remembers about it (review finding M2).
    next_output_index: u32,
    /// Incremented once per `LoopHandler::should_stop` call, i.e. once per
    /// event-loop turn regardless of what woke it (see that method's doc).
    /// Not read anywhere in a real boot -- this exists so a test can prove a
    /// negative ("the loop did not busy-spin") that no other observable
    /// state can: a wedged wake pipe (review finding C1) doesn't fail any
    /// assertion about *what* the loop did, only about how relentlessly it
    /// did nothing, and turn count is the only signal for that.
    pub turns: u64,
    /// Incremented once per `OutputHandler::frame` call. Review finding I1's
    /// test coverage: proves `new_output`'s `schedule_frame()` call actually
    /// results in at least one `frame` callback, not just that it compiles
    /// and doesn't panic.
    pub frames: u64,
    /// Saved window geometries before fullscreen toggle, keyed by window ID.
    /// Used to restore non-fullscreen geometry when exiting fullscreen.
    ///
    /// Kept separate from `snap_saved_geometry` (Task 11 review #2): both
    /// `toggle_fullscreen` and `snap` used to share one `saved_geometry` map
    /// keyed only by `WindowId`, so snapping a window and then
    /// fullscreening it clobbered the pre-snap geometry with the snapped
    /// one, and `snap_restore` after unfullscreening silently restored
    /// nothing (it returns `Some(())` on a missing entry). Two independent
    /// save slots per window means each feature's restore point survives
    /// the other feature running in between.
    pub fullscreen_saved_geometry: HashMap<WindowId, icedtea_contract::Rectangle>,
    /// Saved window geometry before a `snap`, keyed by window ID. Used by
    /// `snap_restore`. See `fullscreen_saved_geometry`'s doc for why this is
    /// a separate map.
    pub snap_saved_geometry: HashMap<WindowId, icedtea_contract::Rectangle>,
    /// Saved window geometry before a maximize, keyed by window ID. Its own
    /// slot for the same reason `fullscreen_saved_geometry` and
    /// `snap_saved_geometry` are separate: maximizing a snapped window must
    /// not clobber snap's restore point (review finding I5).
    pub maximized_saved_geometry: HashMap<WindowId, icedtea_contract::Rectangle>,
    /// Alt-tab cycling state, driven by `apply_action("cycle:alt_tab")` and
    /// ended by `end_alt_tab` (see its doc for the chosen end condition --
    /// driving this from a real keyboard filter is task 5's job).
    pub alt_tab: input::AltTabMachine,
    /// Pointer-driven window move/snap state machine.
    pub drag: input::DragMachine,
    /// Pointer-driven interactive-resize state machine, stepped by the same
    /// pointer motion/release path the drag machine uses. A client-driven
    /// resize request will start one again once task 5 wires the inbound
    /// half of that protocol back up.
    pub resize: input::ResizeMachine,
    /// Set by `apply_action("quit")`; the event loop (task 5 reintroduces
    /// one) checks this each iteration and calls `stop()` once true.
    pub quitting: bool,
    /// Last known pointer position in output logical coordinates. Task 5's
    /// input plumbing updates this on every pointer-motion event;
    /// button-only events (which carry no position of their own) read it
    /// back to build `PointerEvent::Press`/`Release`.
    pub pointer_location: (i32, i32),
    /// Whether a pointer button is currently held down. Maintained at the
    /// top of `SeatHandler::pointer_button`, before any routing, so
    /// `begin_client_move`/`begin_client_resize` can enforce the policy this
    /// crate substitutes for the seat/serial the library deliberately does
    /// not forward with `request_move`/`request_resize` (see `wlr::Toplevel
    /// Handler`'s doc on those methods): honor an interactive move/resize
    /// request only while a real button-down backs it, never on the
    /// client's claim alone.
    pub pointer_pressed: bool,
    /// On-disk location `reload_config_from_disk`/`handle_command`'s
    /// `ReloadConfig` worker thread reads from. `None` (the boot-time
    /// default) means "use `icedtea_config::default_db_path()`" -- this is
    /// only ever overridden by tests, which need an isolated temp DB rather
    /// than the real XDG path.
    pub config_path: Option<PathBuf>,
    /// Set by `set_config_reload_sender` once the caller has somewhere to
    /// send reload results (in production, `lib.rs`'s `run()`, which also
    /// keeps the paired receiver -- see `config_reload_rx`).
    /// `handle_command`'s `ReloadConfig` arm sends the freshly-loaded
    /// `Config` back over this from a worker thread so the redb I/O + JSON
    /// parse never blocks the caller's loop; `None` before that wiring
    /// exists is a no-op (nothing to reload into).
    config_reload_tx: Option<crossbeam_channel::Sender<Config>>,
    /// Paired receiving half of `config_reload_tx`, drained once per turn by
    /// `drain_config_reload`. `None` until `set_config_reload_receiver`
    /// wires it -- tests that only exercise the sender side (the reload
    /// tests below) never set this.
    config_reload_rx: Option<crossbeam_channel::Receiver<Config>>,
    /// Which model output index each library output id maps to.
    ///
    /// Kept because `OutputHandler::destroyed` is given only an id, and the
    /// model's geometry map is keyed by index.
    output_ids: HashMap<wlr::OutputId, u32>,
    /// The scene rect painted behind everything, once boot has made one.
    background: Option<wlr::RectId>,
    /// The fd source SIGINT/SIGTERM write to. Compared in `fd_ready` so that
    /// a future second source cannot be mistaken for this one.
    shutdown_source: Option<wlr::SourceId>,
    /// The D-Bus service's command channel, drained once per loop turn (by
    /// `fd_ready`'s `cmd_wake_source` arm, and as a backstop by
    /// `should_stop`).
    cmd_rx: Option<crossbeam_channel::Receiver<crate::dbus::DbCommand>>,
    /// The fd source `WmInterface::send` (`dbus.rs`) nudges after every
    /// command it forwards onto `cmd_rx`. Compared in `fd_ready` the same
    /// way `shutdown_source` is; without it a command sent while the loop is
    /// blocked in `dispatch(-1)` would sit undrained until some unrelated
    /// event happened to wake the loop anyway.
    cmd_wake_source: Option<wlr::SourceId>,
    /// The fd source `spawn_config_reload`'s worker thread nudges after
    /// sending a freshly-loaded config over `config_reload_tx`. Same
    /// purpose as `cmd_wake_source`, for `drain_config_reload`.
    config_reload_wake_source: Option<wlr::SourceId>,
    /// Write half of the pipe registered as `config_reload_wake_source`.
    /// Kept here (not just handed to the one worker thread that exists when
    /// `set_config_reload_wake` runs) because `spawn_config_reload` can spin
    /// up a fresh worker on every reload trigger, and each one needs its own
    /// `try_clone`d handle -- see that method's doc.
    config_reload_wake: Option<std::os::unix::net::UnixStream>,
    /// The receiving half of `render::spawn_wallpaper_decode`'s channel,
    /// drained once per turn by `drain_wallpaper` (mirrors `config_reload_rx`
    /// for the wallpaper decode worker). `None` until `set_wallpaper_receiver`
    /// wires it -- boot is the only caller, since there is exactly one decode
    /// worker per process lifetime.
    wallpaper_rx: Option<crossbeam_channel::Receiver<Option<image::RgbaImage>>>,
    /// Whether `drain_wallpaper` has ever successfully received a result on
    /// `wallpaper_rx`. Distinguishes the two ways the channel can report
    /// `Disconnected`: before this is `true`, disconnect-without-a-message
    /// means the worker really did exit early (panicked) and is worth a
    /// warn; after it, the worker sent its one message and exited exactly as
    /// designed, and the sender dropping is the expected, silent end of its
    /// lifetime (review finding M4).
    wallpaper_received: bool,
    /// The fd source the wallpaper decode worker nudges after it sends its
    /// result. Compared in `fd_ready` the same way `config_reload_wake_source`
    /// is, so a result produced while the loop is idle in `Until::Stop`'s
    /// blocking `dispatch(-1)` is picked up immediately rather than sitting
    /// unseen until an unrelated event happens to wake the loop.
    wallpaper_wake_source: Option<wlr::SourceId>,
    /// Write half of the pipe registered as `wallpaper_wake_source`, kept
    /// alive here for the same reason `config_reload_wake` is (review
    /// finding C1, fixed the same way `shutdown_source`'s sibling comment
    /// and `spawn_config_reload` both already document): the wallpaper
    /// decode worker is only ever handed a `try_clone`d copy, never this
    /// original. Without an owner that outlives the worker thread, the
    /// worker's clone was the *only* live write half, and it dropped the
    /// moment the thread exited after its one send -- leaving the
    /// registered read end with a permanent `EPOLLHUP` on libwayland's
    /// level-triggered loop, so `fd_ready`'s wallpaper arm (and
    /// `drain_wallpaper`'s "likely panicked" warn, before the M4 fix above)
    /// fired every single turn forever, for the rest of the process's life.
    wallpaper_wake: Option<std::os::unix::net::UnixStream>,
    /// One buffer scene node per output currently showing the decoded
    /// wallpaper image, keyed by the same model output index `outputs` is.
    /// Empty whenever `wallpaper.decoded()` is `None` (pre-decode, or a
    /// decode that failed) -- `sync_wallpaper_nodes` tears every node down
    /// in that case, leaving the solid `background` rect (`lib.rs::run()`)
    /// as the only thing on screen. `pub(crate)` so `wallpaper_node_count`
    /// can read it for test introspection without exposing the map itself.
    wallpaper_nodes: HashMap<u32, wlr::BufferId>,
    /// Font enumeration and glyph raster caches for `text::rasterize_title`.
    ///
    /// Deviation 5: built lazily, once, and never rebuilt.
    /// `FontSystem::new` walks every font directory on the machine (tens of
    /// milliseconds), and `SwashCache` is pure memoization of glyph rasters,
    /// so both belong to the process rather than to a call. A `OnceCell`
    /// rather than an eager field because the overwhelming majority of this
    /// crate's `State`s -- every unit test -- never rasterize anything at
    /// all, and must not pay for a font scan to construct a model.
    fonts: std::cell::OnceCell<(cosmic_text::FontSystem, cosmic_text::SwashCache)>,
    /// Per-window memo of the last title raster: the inputs it was made
    /// from, the pixels that came out, and the generation that names them.
    ///
    /// `sync_window_to_scene` runs on every geometry mutation -- once per
    /// pointer motion during a drag -- while a title's *pixels* only change
    /// when the text, the width available for it, or the resolved text color
    /// changes. This memo is what keeps an unchanged title from being
    /// re-shaped on every one of those syncs; the `generation` is what keeps
    /// it from being re-*uploaded* (review finding M2), which is the more
    /// expensive half: the seam remembers which generation each title node
    /// currently holds and skips `update_buffer` -- and the scene damage
    /// that comes with it -- when it already matches. The pixels are
    /// borrowed across the seam rather than cloned, so a cache hit costs a
    /// map lookup and nothing else.
    ///
    /// `None` on the pixel side is a *cached negative*: this title
    /// rasterized to nothing (empty, or unshapeable on this machine) and
    /// must not be retried on every sync. Dropped in `forget_window` and in
    /// the two teardown paths that bypass it (`apply_config`, and
    /// `sync_window_to_scene`'s vanished-row arm), so a window's raster
    /// never outlives it.
    title_rasters: HashMap<WindowId, TitleRasterEntry>,
    /// Source of `TitleRasterEntry::generation`. Monotonic and never reused,
    /// so "the seam holds generation N" can only ever mean one exact set of
    /// pixels -- across windows as well as within one.
    next_title_generation: u64,
    /// Decoration preferences stated by clients that have no model window
    /// yet, in `Window::client_decorations_requested`'s own three-valued
    /// spelling.
    ///
    /// A client creates its `zxdg_toplevel_decoration_v1` and calls
    /// `set_mode` on it *before* its initial commit, which is well before
    /// `mapped` creates the model row -- so the preference arrives with
    /// nowhere to put it. Parking it here and applying it at `new_toplevel`
    /// is what keeps an explicit "I draw my own decorations" from being
    /// silently dropped and the window handed a band it asked not to have.
    /// Entries are removed when the toplevel is bound (`new_toplevel`) or
    /// dies unmapped (`forget_toplevel`), so this never outgrows the set of
    /// unmapped toplevels.
    pending_decorations: HashMap<crate::wayland::ToplevelKey, Option<bool>>,
    /// Every live wlr-layer-shell surface's model-side bookkeeping, keyed
    /// by the library's own id. Populated at `new_layer_surface`, kept
    /// current on every `layer_surface_commit`, and removed only at
    /// `layer_surface_destroyed` -- an unmap leaves the entry in place
    /// (see `layer_surface_unmapped`'s doc, mirroring the toplevel
    /// unmapped/mapped distinction `Window::mapped` already draws).
    pub layers: HashMap<wlr::LayerSurfaceId, LayerEntry>,
    /// The keyboard-interactive layer surface currently holding seat
    /// keyboard focus, if any. Tracked separately from the model's own
    /// window focus because a layer surface has no `WindowId` at all --
    /// this is what `layer_surface_unmapped`/`layer_surface_destroyed`
    /// check before calling `sync_seat_focus` to hand focus back to
    /// whatever the model says is focused.
    layer_focus: Option<wlr::LayerSurfaceId>,
    /// Source of [`LayerEntry::sequence`]. Monotonic and never reused, the
    /// same shape as `next_title_generation`.
    next_layer_sequence: u64,
    /// Rasterized button-glyph pixels, keyed by `(button index, width,
    /// height, fg)`. Unlike `title_rasters` this needs no per-window entry
    /// or generation counter: a glyph's pixels are a pure function of that
    /// key (the text for each index is fixed, the cell size is always
    /// `BUTTON_WIDTH x TITLE_BAR_HEIGHT`, and `fg` never varies with the
    /// window), so in practice this map holds exactly three entries for the
    /// whole process's life -- one rasterization per button, ever. `None` is
    /// a cached negative, the same convention `title_rasters` uses.
    button_glyph_cache: HashMap<ButtonGlyphKey, Option<Vec<u8>>>,
    /// The title-bar button, if any, the pointer currently sits over --
    /// `(window, button index)`, `decoration::button_at`'s own index order.
    /// Recomputed on every pointer motion (`update_ssd_hover`); scene-only
    /// state, so changing it re-syncs the affected window(s)'s decoration
    /// but never emits a `contract::Event` -- there is no model mutation
    /// here, only a color the seam draws.
    ssd_hover: Option<(WindowId, usize)>,
    /// The title-bar button, if any, currently held down -- set for the
    /// span of `handle_pointer_press`'s own button-action branch, so the
    /// pressed color is what that press's `sync_window_to_scene` call sees
    /// before the action it triggers (close/maximize/minimize) runs. Same
    /// "scene-only, no `Event`" rule as `ssd_hover`.
    ssd_press: Option<(WindowId, usize)>,
}

/// What a cached title raster depends on: the title text, the pixel width
/// available for it, and the *resolved* foreground color. Anything else
/// changing -- position, height, workspace -- cannot change the pixels.
///
/// The color rather than a `focused` flag (review finding M3): focus is
/// only one of the two things that pick the text color, the palette being
/// the other. Keying on the resolved bytes covers both, and covers them
/// exactly -- a reload that changed some unrelated part of the config
/// re-uses the raster, while one that changed `palette.foreground`
/// invalidates it even if the reload someday stops destroying every window.
type TitleRasterKey = (String, i32, [u8; 4]);

/// A rasterized title's pixels: `(width, height, premultiplied RGBA)`.
type TitlePixels = (i32, i32, Vec<u8>);

/// What a cached button-glyph raster depends on: which button
/// (`decoration::button_rects`' index), the cell it was shaped into, and the
/// (constant) glyph color -- see `button_glyph_cache`'s own doc for why that
/// is the whole key, with no per-window component.
type ButtonGlyphKey = (usize, i32, i32, [u8; 4]);

/// One window's memoized title raster.
struct TitleRasterEntry {
    /// The inputs these pixels were shaped from; a miss against this is the
    /// only thing that shapes.
    key: TitleRasterKey,
    /// Names this exact set of pixels for the seam's upload check. Bumped
    /// only when the pixels are re-shaped.
    generation: u64,
    /// `None` is a cached negative -- nothing shaped, show the bare band.
    pixels: Option<TitlePixels>,
}

/// Left inset of the title text inside the band, in pixels.
const TITLE_PAD_X: i32 = 8;

/// The three button glyphs, `decoration::button_rects`' own order: an
/// en-dash for minimize (the universal "make this go away downward" mark),
/// a hollow square for maximize (an unfilled window outline), and a
/// multiplication-x for close. Plain Unicode rather than an icon font or an
/// embedded bitmap -- the same reasoning `rasterize_title` already commits
/// to for text -- so `cosmic-text`'s ordinary font-fallback path draws them,
/// with no new asset for this crate to ship or a machine to be missing.
const BUTTON_GLYPHS: [&str; 3] = ["\u{2013}", "\u{25A1}", "\u{2715}"];

/// The glyph color painted into every button, regardless of which button or
/// which window: white reads against both the semi-transparent foreground
/// chip (minimize/maximize) and the opaque accent close button, for every
/// palette this config format can express -- one fewer color the config
/// would otherwise need a field for.
const BUTTON_GLYPH_FG: [u8; 4] = [255, 255, 255, 255];

/// `color` at `alpha`, premultiplied -- every channel scaled, not just the
/// alpha one, because the wlroots scene graph composites premultiplied
/// colors and a rect whose RGB outran its alpha would come out brighter than
/// the same color opaque.
fn premultiply(color: [f32; 4], alpha: f32) -> [f32; 4] {
    let a = (color[3] * alpha).clamp(0.0, 1.0);
    [color[0] * a, color[1] * a, color[2] * a, a]
}

impl State {
    pub fn new(config: Config, dbus_tx: crossbeam_channel::Sender<SeqEvent>) -> Self {
        let workspace_names = config.workspace_names.clone();
        // Review finding I4: report unusable bindings once, here, instead of
        // from inside the per-key-press lookup.
        input::warn_about_keybindings(&config.keybindings);

        Self {
            window_manager: WindowManager::new(workspace_names),
            config,
            dbus_tx,
            wayland: crate::wayland::Wayland::new(),
            start_time: Instant::now(),
            wallpaper: WallpaperState::new(),
            snap_preview: None,
            snap_preview_rect: None,
            outputs: HashMap::new(),
            next_output_index: 0,
            turns: 0,
            frames: 0,
            fullscreen_saved_geometry: HashMap::new(),
            snap_saved_geometry: HashMap::new(),
            maximized_saved_geometry: HashMap::new(),
            alt_tab: input::AltTabMachine::new(),
            drag: input::DragMachine::new(),
            resize: input::ResizeMachine::new(),
            quitting: false,
            pointer_location: (0, 0),
            pointer_pressed: false,
            config_path: None,
            config_reload_tx: None,
            config_reload_rx: None,
            output_ids: HashMap::new(),
            background: None,
            shutdown_source: None,
            cmd_rx: None,
            cmd_wake_source: None,
            config_reload_wake_source: None,
            config_reload_wake: None,
            wallpaper_rx: None,
            wallpaper_received: false,
            wallpaper_wake_source: None,
            wallpaper_wake: None,
            wallpaper_nodes: HashMap::new(),
            fonts: std::cell::OnceCell::new(),
            title_rasters: HashMap::new(),
            next_title_generation: 0,
            pending_decorations: HashMap::new(),
            layers: HashMap::new(),
            layer_focus: None,
            next_layer_sequence: 0,
            button_glyph_cache: HashMap::new(),
            ssd_hover: None,
            ssd_press: None,
        }
    }

    /// Wire up the config-reload result channel. Both halves are kept: the
    /// sender goes to the worker thread `spawn_config_reload` spins up, and
    /// the loop drains the receiver itself (via `drain_config_reload`) now
    /// that there is no event-source abstraction to do it.
    pub fn set_config_reload_sender(&mut self, tx: crossbeam_channel::Sender<Config>) {
        self.config_reload_tx = Some(tx);
    }

    /// Keep the receiving half so [`Self::drain_config_reload`] has
    /// somewhere to read from. Separate from `set_config_reload_sender`
    /// because the tests wire only the sender and read the receiver
    /// themselves.
    pub fn set_config_reload_receiver(&mut self, rx: crossbeam_channel::Receiver<Config>) {
        self.config_reload_rx = Some(rx);
    }

    /// Apply any config a reload worker has finished loading.
    ///
    /// Called once per event-loop turn. Non-blocking: `try_recv` on an empty
    /// channel is the overwhelmingly common case and must cost nothing.
    pub fn drain_config_reload(&mut self) {
        let Some(rx) = self.config_reload_rx.as_ref() else { return };
        let pending: Vec<Config> = rx.try_iter().collect();
        for cfg in pending {
            let _ = self.apply_reloaded_config(cfg);
        }
    }

    /// Apply the wallpaper decode worker's result, if it has sent one.
    ///
    /// Called once per event-loop turn, the same shape as
    /// `drain_config_reload`: non-blocking (`try_recv` on an empty channel
    /// is the overwhelmingly common case, both before the worker finishes
    /// and forever after, since it sends exactly one result and exits), and
    /// panic-free on a disconnected sender -- `spawn_wallpaper_decode`'s
    /// thread has either not sent yet, sent once, or panicked, and none of
    /// those is a reason for this handler to abort the process.
    pub fn drain_wallpaper(&mut self) {
        let Some(rx) = self.wallpaper_rx.as_ref() else { return };
        match rx.try_recv() {
            Ok(image) => {
                self.wallpaper.set_decoded(image);
                self.wallpaper_received = true;
                self.sync_wallpaper_nodes();
            }
            // Review finding M4: `Disconnected` means two different things
            // depending on `wallpaper_received`. Before a result has ever
            // arrived, the worker exiting without sending one really is
            // abnormal (a panic) and is worth a warn. After one has arrived,
            // the sender dropping is just the worker reaching the end of its
            // one-shot lifetime -- `spawn_wallpaper_decode` sends exactly
            // once and returns -- and this arm is reached on every
            // subsequent turn's poll, so warning here would be a false
            // "likely panicked" on every turn of an otherwise healthy,
            // long-idle compositor.
            Err(crossbeam_channel::TryRecvError::Disconnected) if !self.wallpaper_received => {
                tracing::warn!(
                    "wallpaper decode worker thread exited without producing a result \
                     (it likely panicked); wallpaper stays solid-color"
                );
                // Only warn once: this arm's condition would otherwise stay
                // true and re-fire the same warn on every future turn too.
                self.wallpaper_received = true;
            }
            Err(crossbeam_channel::TryRecvError::Disconnected) => {}
            Err(crossbeam_channel::TryRecvError::Empty) => {}
        }
    }

    /// How many wallpaper buffer nodes are currently tracked. Introspection
    /// for tests -- nothing in a real boot needs to count these itself
    /// (mirrors `Wayland::ssd_rect_count`).
    pub fn wallpaper_node_count(&self) -> usize {
        self.wallpaper_nodes.len()
    }

    /// Creates/updates one wallpaper buffer node per output from the
    /// decoded image, stretched to fill (`render::wallpaper_dest`); removes
    /// nodes for outputs that are gone. Idempotent -- calling this again
    /// with nothing changed just re-sets each node's position/size and
    /// re-lowers it, which are all no-ops on an already-correct scene.
    ///
    /// **Contract (task 7/14): nodes are never pixel-refreshed in place.**
    /// The existing-node branch below only repositions/resizes; it never
    /// calls `update_buffer` (or re-creates the node) to show a *different*
    /// image's pixels under an unchanged output index. That branch was
    /// unreachable until task 14 gave `apply_reloaded_config` a wallpaper
    /// swap path -- reachable now, so the contract has to be stated rather
    /// than left implicit: a wallpaper change must clear first
    /// (`wallpaper.set_decoded(None)` + a call here to tear every node
    /// down) before the fresh decode's result calls this again to rebuild
    /// them. `apply_reloaded_config` enforces exactly that ordering; nothing
    /// here checks it, so a caller that skips the clear would silently keep
    /// showing the old image at the old node's size/position rather than
    /// picking up the new one.
    ///
    /// Called whenever either side of the sync could have changed: a fresh
    /// decode landing (`drain_wallpaper`), or the output set changing
    /// (`OutputHandler::new_output`/`destroyed`). Not a model mutation --
    /// these are scene bookkeeping only, so this never touches
    /// `window_manager` or emits an event (the exactly-one-event-per-mutation
    /// invariant has nothing to say about a rect existing on screen).
    ///
    /// With no runtime attached (every unit test that never calls
    /// `wayland.attach`), every `wlr` call below is a no-op that returns
    /// `None`/does nothing, so this degrades to a harmless no-op that also
    /// never populates `wallpaper_nodes` -- there is no `BufferId` a real
    /// call could have returned to put in the map.
    pub fn sync_wallpaper_nodes(&mut self) {
        let Some(runtime) = self.wayland.runtime().cloned() else { return };

        let Some(img) = self.wallpaper.decoded() else {
            // No decoded image (pre-decode, or a decode that failed): tear
            // every node down and fall back to the solid background rect.
            // `remove_buffer` tolerates a stale id (already gone via its own
            // toplevel teardown, though wallpaper nodes are root buffers so
            // that specific race does not apply here) -- just `None`, never
            // a panic.
            for (_, node) in self.wallpaper_nodes.drain() {
                runtime.remove_buffer(node);
            }
            return;
        };

        let width = img.width() as i32;
        let height = img.height() as i32;
        // Never panic in handler-reachable code: a zero-dimension decode
        // (which `image` should never actually produce, but nothing here
        // proves it can't) skips every output rather than handing wlroots a
        // buffer `add_buffer`/`update_buffer` would reject anyway.
        if width == 0 || height == 0 {
            return;
        }
        let rgba = img.as_raw().as_slice();

        // Freshly created nodes, in creation order. Collected rather than
        // lowered inline because the lowering *order* is the whole
        // correctness question here -- see `wallpaper_lower_plan`.
        let mut created: Vec<wlr::BufferId> = Vec::new();
        for (index, surface) in &self.outputs {
            let dest = render::wallpaper_dest(surface.geometry);
            if let Some(&node) = self.wallpaper_nodes.get(index) {
                runtime.set_buffer_position(node, dest.x, dest.y);
                runtime.set_buffer_dest_size(node, dest.width, dest.height);
            } else {
                match runtime.add_buffer(width, height, rgba) {
                    Ok(node) => {
                        runtime.set_buffer_position(node, dest.x, dest.y);
                        runtime.set_buffer_dest_size(node, dest.width, dest.height);
                        created.push(node);
                        self.wallpaper_nodes.insert(*index, node);
                    }
                    // Finding 5, errors: the failure used to be silently
                    // dropped, leaving that output with no wallpaper node
                    // and no trace of why -- named with the output index so
                    // a multi-output setup's log points at the one output
                    // actually missing its wallpaper.
                    Err(err) => {
                        tracing::warn!(output = index, ?err, "failed to create wallpaper buffer node for output");
                    }
                }
            }
        }
        // Review finding C1: every wallpaper node used to be lowered inline
        // right here, *after* `lib.rs::run()` had already lowered the
        // opaque background rect at boot -- and `lower_*_to_bottom` moves
        // its target to the very bottom of the root's children, so the
        // most recently lowered node wins the bottom. That put every
        // wallpaper node *underneath* the full-output opaque rect and the
        // wallpaper was never visible at all. The background rect goes
        // last now, which is what actually puts it at the very bottom and
        // leaves the wallpaper directly above it -- solid color as the
        // pre-decode/decode-failure fallback, not a permanent occlusion.
        for step in wallpaper_lower_plan(created.len(), self.background.is_some()) {
            match step {
                LowerStep::Wallpaper(i) => {
                    if let Some(&node) = created.get(i) {
                        runtime.lower_buffer_to_bottom(node);
                    }
                }
                LowerStep::Background => {
                    if let Some(bg) = self.background {
                        runtime.lower_rect_to_bottom(bg);
                    }
                }
            }
        }

        // Drop nodes for outputs that are gone.
        let gone: Vec<u32> = self
            .wallpaper_nodes
            .keys()
            .filter(|index| !self.outputs.contains_key(index))
            .copied()
            .collect();
        for index in gone {
            if let Some(node) = self.wallpaper_nodes.remove(&index) {
                runtime.remove_buffer(node);
            }
        }
    }

    /// Reconcile the snap-preview scene rect with `self.snap_preview`:
    /// create it (accent color at 35% alpha, premultiplied) the moment a
    /// drag starts showing a target, reposition/resize it in place as the
    /// target changes, and remove it the moment there is no longer one to
    /// show.
    ///
    /// Call this immediately after every assignment to `snap_preview` --
    /// the two fields are meant to be read as one unit, and nothing else
    /// keeps them in sync. With no runtime attached (every unit test that
    /// never calls `wayland.attach`) this is a no-op, the same degradation
    /// `sync_wallpaper_nodes` documents for the identical reason: there is
    /// no `RectId` a real call could have returned to put in
    /// `snap_preview_rect`.
    pub fn sync_snap_preview(&mut self) {
        let Some(runtime) = self.wayland.runtime().cloned() else { return };
        match self.snap_preview {
            Some(rect) => match self.snap_preview_rect {
                Some(id) => {
                    runtime.set_rect_position(id, rect.x, rect.y);
                    runtime.set_rect_size(id, rect.width, rect.height);
                }
                None => {
                    let color = premultiply(
                        render::hex_to_rgba(&self.config.appearance.palette.accent),
                        0.35,
                    );
                    if let Ok(id) =
                        runtime.add_rect_in_band(wlr::Band::Overlay, rect.width, rect.height, color)
                    {
                        runtime.set_rect_position(id, rect.x, rect.y);
                        self.snap_preview_rect = Some(id);
                    }
                }
            },
            None => {
                if let Some(id) = self.snap_preview_rect.take() {
                    runtime.remove_rect(id);
                }
            }
        }
    }

    /// Ask the event loop to stop.
    ///
    /// Sets the same flag `apply_action("quit")` does rather than signalling a
    /// loop handle, because the loop is now driven by `wlr` and asks the state
    /// whether to stop (`LoopHandler::should_stop`) instead of being told.
    pub fn stop(&mut self) {
        self.quitting = true;
    }

    /// Record an output's geometry. Backends call this once they know the mode.
    ///
    /// The smithay version of this also created a protocol global and mapped
    /// the output into a `Space`; both now belong to the compositor library
    /// (the global comes with the backend, the placement with the scene's
    /// output layout), so all that is left here is the geometry map that
    /// `snap`, `set_fullscreen_target`, `set_maximized_target` and
    /// `handle_pointer_motion` read.
    pub fn create_output(&mut self, index: u32, geometry: icedtea_contract::Rectangle) {
        self.outputs.insert(index, OutputSurface::new(geometry));
    }

    /// The output whose box contains the pointer; falls back to the lowest
    /// index. Placement (cascade origin, maximize/fullscreen/snap target)
    /// uses this instead of the implicit single output, so every one of
    /// those consumers moves with the pointer once a second output exists.
    ///
    /// With no attached runtime (every unit test in this file, and any
    /// caller ahead of `wayland.attach`), `pointer_position` is unavailable
    /// -- the fallback path also covers a pointer that landed outside every
    /// known output's box, which can happen for an instant right after a
    /// hotplug removes the one it was over.
    pub fn output_for_pointer(&self) -> Option<u32> {
        if let Some(runtime) = self.wayland.runtime() {
            let (px, py) = runtime.pointer_position();
            let (px, py) = (px as i32, py as i32);
            let hit = self.outputs.iter().find(|(_, out)| out.geometry.contains(px, py));
            if let Some((&idx, _)) = hit {
                return Some(idx);
            }
        }
        self.outputs.keys().min().copied()
    }

    /// The output whose box contains `geometry`'s frame center, falling
    /// back to [`Self::output_for_pointer`] (whose own fallback is the
    /// lowest index) when no output's box contains it.
    ///
    /// Review finding J1: `arrange_layers` used to re-home every
    /// maximized window through `output_for_pointer` alone, which is a
    /// *pointer*-driven disambiguator every one of its other callers gets
    /// to use because they all run inside a user-initiated action
    /// (maximize, snap, drag) where the pointer genuinely names the
    /// window in question. `arrange_layers` runs from an unrelated
    /// client's layer-surface commit -- a status bar's clock tick is
    /// enough -- so the pointer carries no information about which
    /// maximized window is being re-laid-out, and using it teleported a
    /// maximized window on output 1 onto output 0's `usable` rect the
    /// moment a panel on output 1 committed while the pointer merely
    /// happened to be sitting over output 0. A window's own frame center
    /// is what actually names its output; mirrors
    /// `migrate_windows_from`'s identical containment test.
    fn output_for_window(&self, geometry: Rectangle) -> Option<u32> {
        let cx = geometry.x + geometry.width / 2;
        let cy = geometry.y + geometry.height / 2;
        let hit = self.outputs.iter().find(|(_, out)| out.geometry.contains(cx, cy));
        if let Some((&idx, _)) = hit {
            return Some(idx);
        }
        self.output_for_pointer()
    }

    /// Finding 4, maintainability: the `usable` rect of whatever output
    /// [`Self::output_for_pointer`] names, extracted once rather than
    /// inlined at each pointer-correct call site (`snap`,
    /// `handle_pointer_motion`, `new_toplevel`'s cascade placement) --
    /// those genuinely run inside a user-initiated, pointer-driven action
    /// (see `output_for_window`'s own doc for the distinction from a
    /// client-request/D-Bus path, which is not one of these). `None` when
    /// no output resolves at all.
    fn usable_geo_for_pointer(&self) -> Option<Rectangle> {
        self.output_for_pointer().and_then(|idx| self.outputs.get(&idx)).map(|o| o.usable)
    }

    /// Recompute every output's `usable` rect from its layer surfaces'
    /// exclusive zones, then re-sync every maximized window whose target
    /// rect actually changed so maximize immediately tracks the new
    /// usable area.
    ///
    /// Reset-then-fold: every output's `usable` starts back at `geometry`
    /// (a panel that shrank its zone, moved output, unmapped, or was
    /// destroyed must give its space back, not just never claim more of
    /// it) and each *mapped* (review finding J2 -- an unmapped surface
    /// reserves nothing, see [`LayerEntry::mapped`]'s doc) layer entry's
    /// positive exclusive zone shrinks the respective edge via
    /// [`fold_exclusive_zone`] -- `exclusive <= 0` reserves nothing, per
    /// wlr-layer-shell's own definition (see [`LayerEntry::exclusive`]'s
    /// doc).
    ///
    /// Fullscreen windows are deliberately never touched here at all --
    /// not filtered out after the fact, never in the affected set to
    /// begin with -- because fullscreen geometry is defined to keep
    /// `geometry`, never `usable` (`set_fullscreen_target`'s own doc), so
    /// no exclusive-zone change this method makes could ever be relevant
    /// to one; re-syncing them on every panel commit was pure waste
    /// (review finding M4).
    ///
    /// A maximized window's target is only applied -- `set_geometry` +
    /// `sync_window_to_scene` -- when it actually differs from the
    /// window's current geometry (review finding M4): `set_geometry`
    /// emits `WindowUpdated` unconditionally, so without this guard a
    /// panel redrawing its clock once a second produced one D-Bus signal
    /// and one client `configure` per maximized window, per panel frame,
    /// with byte-identical geometry.
    ///
    /// Each maximized window's output is its own frame center's, not a
    /// single pointer-derived one (review finding J1) -- see
    /// `output_for_window`'s own doc for the multi-output regression this
    /// closes.
    ///
    /// Windows are collected into a `Vec` before any is synced (review
    /// pattern this crate already follows elsewhere, e.g.
    /// `migrate_windows_from`): `sync_window_to_scene` reads `self`
    /// broadly, so mutating `window_manager` while still mid-iteration
    /// over it would not borrow-check.
    pub fn arrange_layers(&mut self) {
        for output in self.outputs.values_mut() {
            output.usable = output.geometry;
        }
        for entry in self.layers.values() {
            if !entry.mapped {
                continue;
            }
            let Some(output) = self.outputs.get_mut(&entry.output) else { continue };
            output.usable = fold_exclusive_zone(output.usable, entry.anchor, entry.exclusive);
        }

        // Re-review finding Minor-1: the fold above can change a *later*
        // (higher-`sequence`) mapped panel's own placement input --
        // `usable_before`'s output -- without that panel ever committing
        // itself, e.g. an earlier panel growing its zone or unmapping.
        // Nothing else would ever re-offer it a fresh placement, so it
        // stayed positioned where it was last configured until its own
        // next commit happened to come along. Reconfiguring every mapped
        // panel here closes that, and does not reopen M4's storm class:
        // `configure_layer` only actually sends when the computed
        // placement differs from the one it last sent (`LayerEntry::
        // last_configured`), so an arrange pass that changed nothing for
        // a given panel costs one comparison, not a wire round trip.
        let mapped_layers: Vec<wlr::LayerSurfaceId> =
            self.layers.iter().filter(|(_, e)| e.mapped).map(|(&id, _)| id).collect();
        for id in mapped_layers {
            self.configure_layer(id);
        }

        let gap = self.config.appearance.snap_gap;
        // N11: a maximized window that is minimized, or sitting on a
        // workspace that is not the active one, is not on screen -- this
        // sweep re-syncing it anyway did no visible harm by itself (nothing
        // draws it), but it still emitted a `WindowUpdated` and warped its
        // stored geometry to whatever `usable` happens to be *right now* on
        // an output the user cannot see, which is wrong the moment the
        // window is later shown again (unminimized, or its workspace
        // switched to) with a panel layout that has since changed underfoot
        // with no configure of its own. `continue` for both up front, same
        // "not actually visible" test `windows_in_workspace` already uses.
        let active_workspace = self.window_manager.active_workspace();
        let affected: Vec<WindowId> = self
            .window_manager
            .windows()
            .filter(|w| w.maximized && !w.minimized && w.workspace == active_workspace)
            .map(|w| w.id)
            .collect();
        for id in affected {
            let Some(w) = self.window_manager.get(id) else { continue };
            let Some(output_idx) = self.output_for_window(w.geometry) else { continue };
            let Some(output_geo) = self.outputs.get(&output_idx).map(|o| o.usable) else { continue };
            let target = layout::maximized_geometry(output_geo, gap);
            if target == w.geometry {
                continue;
            }
            let _ = self.window_manager.set_geometry(id, target);
            self.sync_window_to_scene(id);
        }
        self.emit_pending();
    }

    /// The `usable` rect for `output_idx` as every *other*, mapped,
    /// positive-exclusive-zone entry whose [`LayerEntry::sequence`] is
    /// strictly less than `before` has already carved it -- `None` if
    /// `output_idx` names no live output.
    ///
    /// This is [`Self::configure_layer`]'s N6 fix: `arrange_layers`'
    /// `output.usable` already folds *every* entry together (itself
    /// included), which is right for "how much space is left for
    /// windows" but wrong as a placement base for an individual panel --
    /// self-excluding shrinks a panel by its own reservation, and folding
    /// every other entry with no ordering stacks two same-edge panels on
    /// top of each other rather than one beside the other. Folding only
    /// entries ordered strictly before this one gives a deterministic
    /// stack instead: whichever panel was announced first sits flush
    /// against the edge, the next stacks outward from it.
    fn usable_before(&self, output_idx: u32, before: u64) -> Option<Rectangle> {
        let mut rect = self.outputs.get(&output_idx)?.geometry;
        for entry in self.layers.values() {
            if entry.output != output_idx || entry.sequence >= before || !entry.mapped {
                continue;
            }
            rect = fold_exclusive_zone(rect, entry.anchor, entry.exclusive);
        }
        Some(rect)
    }

    /// Pure half of `configure_layer` (task 7): choose `id`'s layer
    /// surface's `(width, height, x, y)` for its output box, the
    /// placement rule from task 20's brief, without touching
    /// `last_configured` or sending anything over the wire. Extracted so
    /// the rule itself -- particularly N7, honoring `desired_size` on
    /// whichever axis is not anchored to both of its edges -- is directly
    /// unit-testable without a live `wlr::Runtime`; `configure_layer` is
    /// now compute-then-send: call this, then decide whether to record
    /// and answer.
    ///
    /// `None`, panic-free, if the entry or its output has vanished (a
    /// commit racing a hotplug-removed output, or the `NO_OUTPUT`
    /// sentinel -- see [`LayerEntry::output`]'s doc).
    pub fn compute_layer_placement(&self, id: wlr::LayerSurfaceId) -> Option<(u32, u32, i32, i32)> {
        let entry = self.layers.get(&id)?;
        let (output_idx, sequence, a, (desired_w, desired_h)) =
            (entry.output, entry.sequence, entry.anchor, entry.size);
        // N6: placed against the space every earlier-announced panel on
        // this output has already carved, not the raw output box --
        // otherwise two same-edge exclusive panels draw on top of each
        // other. See `usable_before`'s own doc.
        let box_ = self.usable_before(output_idx, sequence)?;

        // Placement is per-axis and independent (review finding I3) -- see
        // `layer_axis_placement`. The rule task 20 shipped keyed on
        // `a.top != a.bottom` / `a.left != a.right` for the *whole*
        // placement, which spanned a corner-anchored notification across
        // the full output width (discarding its desired width outright) and
        // dropped a four-edge-anchored 0x0 locker into a centered 200x200
        // box instead of filling the output.
        let (w, x) = layer_axis_placement(a.left, a.right, desired_w, box_.x, box_.width);
        let (h, y) = layer_axis_placement(a.top, a.bottom, desired_h, box_.y, box_.height);

        let (w, h) = (w.max(0) as u32, h.max(0) as u32);
        Some((w, h, x, y))
    }

    /// Choose `id`'s layer surface's size and position for its output box
    /// (the placement rule from task 20's brief) and answer with
    /// `configure_layer_surface` + `set_layer_surface_position`.
    ///
    /// A no-op, panic-free, if the entry or its output has vanished (a
    /// commit racing a hotplug-removed output, or the `NO_OUTPUT`
    /// sentinel -- see [`LayerEntry::output`]'s doc), no runtime is
    /// attached (every unit test in this file), or the computed placement
    /// is unchanged from [`LayerEntry::last_configured`] -- see that
    /// field's own doc (review finding Minor-1): this makes the method
    /// safe to call unconditionally, which `arrange_layers` now does for
    /// every mapped panel on every pass.
    pub fn configure_layer(&mut self, id: wlr::LayerSurfaceId) {
        let Some(placement) = self.compute_layer_placement(id) else { return };
        let (w, h, x, y) = placement;
        // Minor-1's storm guard: unchanged since the last time this was
        // computed is a no-op. `None` (never computed before) never
        // matches, so a surface's first `configure_layer` -- from
        // `new_layer_surface` or its own first commit -- always goes out;
        // the mandatory-configure contract is unaffected.
        if self.layers.get(&id).is_some_and(|e| e.last_configured == Some(placement)) {
            return;
        }
        // Recorded before the runtime-gated send below, not after: this
        // is what `configure_layer` computed and is *treating* as current
        // regardless of whether a runtime happened to be attached to
        // actually put it on the wire (every unit test in this file has
        // none). Production always has one attached by the time any of
        // this crate's handlers run, so the gap that would open --
        // recording a placement this method never actually sent -- cannot
        // happen outside a test.
        if let Some(entry) = self.layers.get_mut(&id) {
            entry.last_configured = Some(placement);
        }
        let Some(runtime) = self.wayland.runtime() else { return };
        runtime.configure_layer_surface(id, w, h);
        runtime.set_layer_surface_position(id, x, y);
    }

    /// Re-home every [`LayerEntry`] whose `output` no longer names a live
    /// output (the [`NO_OUTPUT`] sentinel, or an index a since-removed
    /// output left behind) onto a surviving one -- the layer analogue of
    /// [`Self::migrate_windows_from`], but pull rather than push: this
    /// scans every entry rather than only the ones a specific removed
    /// output owned, so the same sweep also recovers a surface that had
    /// no output *at all* when it was announced (review finding M5) the
    /// moment any output exists.
    ///
    /// Re-resolution is per-entry (task 8, M2): each orphaned entry's own
    /// last-configured placement's frame center is tested against every
    /// surviving output's box, falling back to the lowest surviving index
    /// only when the entry has no placement yet (`last_configured: None`)
    /// or its center hits none of them -- not a single
    /// `output_for_pointer`-derived survivor applied to the whole batch,
    /// which let an unrelated commit's pointer position decide where every
    /// orphaned surface (however placed) landed.
    ///
    /// A no-op, correctly, when no output exists at all: every entry's
    /// resolution falls through to the lowest-index fallback, which itself
    /// yields nothing, and every orphaned entry is left exactly where it
    /// was for the next call (the next `new_output`) to try again --
    /// which is what makes the "no output at all when announced" case
    /// self-heal instead of hanging its client forever (review finding
    /// M5).
    ///
    /// Called from both `OutputHandler::new_output` (a fresh or returning
    /// output may be exactly what an orphaned entry was waiting for) and
    /// `OutputHandler::destroyed` (an entry the dying output owned needs
    /// somewhere else to go right away, not just whenever the next output
    /// happens to appear).
    fn resolve_orphaned_layers(&mut self) {
        let orphaned: Vec<wlr::LayerSurfaceId> = self
            .layers
            .iter()
            .filter(|(_, entry)| !self.outputs.contains_key(&entry.output))
            .map(|(&id, _)| id)
            .collect();
        if orphaned.is_empty() {
            return;
        }
        // M2/M5: cloned once, not re-borrowed per entry -- `set_layer_surface_output`
        // below runs inside the loop, alongside a mutable borrow of
        // `self.layers`, which a live `&wlr::Runtime` borrowed from
        // `self.wayland` would conflict with. `Runtime` is cheap to clone
        // (an `Rc`-shaped handle), the same pattern `OutputHandler::new_output`
        // already uses ahead of its own per-output mutations.
        let runtime = self.wayland.runtime().cloned();
        for id in &orphaned {
            // Each entry re-homes to the output its own last-known
            // placement's frame center actually sits over (mirrors
            // `migrate_windows_from`'s per-window containment test) --
            // *not* a single pointer-derived survivor for the whole batch
            // (M2: an unrelated panel commit elsewhere no longer decides
            // where a status bar on a hot-removed display ends up). A
            // surface with no placement yet (`last_configured: None`,
            // never configured) falls back to the lowest surviving index,
            // same as `migrate_windows_from`'s own fallback.
            let geometry = self
                .layers
                .get(id)
                .and_then(|e| e.last_configured)
                .map(|(w, h, x, y)| Rectangle { x, y, width: w as i32, height: h as i32 });
            let hit = geometry.and_then(|g| {
                let cx = g.x + g.width / 2;
                let cy = g.y + g.height / 2;
                self.outputs.iter().find(|(_, out)| out.geometry.contains(cx, cy)).map(|(&idx, _)| idx)
            });
            let Some(survivor) = hit.or_else(|| self.outputs.keys().min().copied()) else { continue };
            if let Some(entry) = self.layers.get_mut(id) {
                entry.output = survivor;
            }
            if let Some(rt) = &runtime
                && let Some(output_id) = self.wlr_output_id_for(survivor)
            {
                rt.set_layer_surface_output(*id, output_id);
            }
            self.configure_layer(*id);
        }
        self.arrange_layers();
    }

    /// The live `wlr::OutputId` behind this crate's own `u32` index, the
    /// reverse of `output_ids`' own direction (`wlr::OutputId -> u32`) --
    /// needed wherever a consumer must call back into the runtime by
    /// output rather than by this crate's index, e.g.
    /// `set_layer_surface_output`. `None` if `index` names no live output
    /// (already removed, or never inserted -- the `NO_OUTPUT` sentinel,
    /// say). A linear scan over `output_ids`, which stays tiny (one entry
    /// per live output).
    fn wlr_output_id_for(&self, index: u32) -> Option<wlr::OutputId> {
        self.output_ids.iter().find_map(|(&oid, &idx)| (idx == index).then_some(oid))
    }

    /// N9: react to `entry.interactive` changing on an already-mapped
    /// surface -- `layer_surface_commit`'s post-map counterpart to
    /// `layer_surface_mapped`'s at-map take-focus branch, which only ever
    /// runs once (at map) and so misses a surface that starts
    /// non-interactive and flips the flag on a later commit while already
    /// on screen (an auto-hide launcher's menu, say).
    ///
    /// Flipping to `true` while mapped and nothing else already holds
    /// layer focus (`layer_holds_keyboard_focus`, mirroring the guard
    /// `sync_seat_focus` itself reads) takes it: `layer_focus` is recorded
    /// success-gated on the actual grab, exactly like
    /// `layer_surface_mapped`'s own take-focus branch -- an unconditional
    /// record ahead of the runtime call (this method's first cut) is the
    /// J3/Important-1 split-brain class reopened: a legitimate
    /// `focus_layer_keyboard` miss (no seat, a stale id, a null surface,
    /// or wlroots' own surface-mapped flag disagreeing with
    /// `LayerEntry::mapped` at commit time) would leave `layer_focus`
    /// claiming a focus the seat never actually held, and
    /// `sync_seat_focus`'s guard would then refuse every toplevel the
    /// keyboard with no self-heal until this id unmapped, was destroyed,
    /// or flipped `interactive` back off. With no `wlr::Runtime` attached
    /// (every unit test in this file), the grab is treated as taken --
    /// gating on `focus_layer_keyboard`'s own `Option` unconditionally (as
    /// if a live runtime were always present) would make this
    /// unobservable outside a running compositor.
    ///
    /// Flipping to `false` while this surface held focus releases it and
    /// hands the seat back to whatever the model says is focused
    /// (`sync_seat_focus`), the same hand-back `layer_surface_unmapped`
    /// and `layer_surface_destroyed` already perform.
    fn sync_layer_interactive_focus(&mut self, id: wlr::LayerSurfaceId, was_interactive: bool, now_interactive: bool) {
        if now_interactive && !was_interactive {
            let mapped = self.layers.get(&id).is_some_and(|e| e.mapped);
            if !mapped || layer_holds_keyboard_focus(self.layer_focus, &self.layers) {
                return;
            }
            // Review finding HIGH (task 8 re-review): `layer_focus` used to
            // be recorded unconditionally, ahead of the runtime call --
            // when `focus_layer_keyboard` legitimately misses (no seat, a
            // stale id, a null surface, or wlroots' own surface-mapped
            // flag disagreeing with `LayerEntry::mapped` at commit time),
            // the seat's *real* keyboard focus never moved, but
            // `layer_focus` claimed it had. `layer_holds_keyboard_focus`
            // then reported `true` and `sync_seat_focus`'s guard refused
            // every toplevel the keyboard, with no self-heal until this id
            // unmapped, was destroyed, or flipped `interactive` back off
            // -- the same split-brain class J3/Important-1 already closed
            // for map/unmap, reopened here. Success-gated exactly like
            // `layer_surface_mapped`'s own take-focus branch: with no
            // runtime attached (every unit test in this file), `took` is
            // `true` so the bookkeeping stays observable without a live
            // `wlr::Runtime`; with one attached, only an actual grab
            // records the claim.
            let took = match self.wayland.runtime() {
                Some(rt) => rt.focus_layer_keyboard(id).is_some(),
                None => true,
            };
            if took {
                self.layer_focus = Some(id);
            } else {
                // Finding 8, errors: mirrors `layer_surface_mapped`'s own
                // trace for the same failure -- a keyboard grab that
                // silently didn't take used to leave no clue why a
                // post-map-interactive panel never got input.
                tracing::debug!(?id, "layer surface became interactive after map but did not take keyboard focus");
            }
        } else if !now_interactive && was_interactive && self.layer_focus == Some(id) {
            self.layer_focus = None;
            self.sync_seat_focus();
        }
    }

    /// Hot-remove semantics: every window whose frame center sat inside the
    /// dead output's box (`dead`, captured by the caller before the entry
    /// left `self.outputs`) is moved onto the surviving output with the
    /// lowest index (clamped into its box, or centered if it does not fit
    /// -- see the centering branch's own comment for F, task 8) and
    /// re-synced. Called from `OutputHandler::destroyed`.
    ///
    /// With no surviving output, this is a deliberate no-op: there is
    /// nowhere to move a window to, and leaving geometry alone (rather than
    /// clamping into an empty rect, which would collapse every affected
    /// window to a single point) is the only choice that doesn't invent a
    /// placement nothing asked for.
    pub fn migrate_windows_from(&mut self, dead: Rectangle) {
        let Some(survivor_idx) = self.outputs.keys().min().copied() else { return };
        let Some(survivor) = self.outputs.get(&survivor_idx).map(|o| o.geometry) else { return };

        let affected: Vec<WindowId> = self
            .window_manager
            .windows()
            .filter(|w| {
                let cx = w.geometry.x + w.geometry.width / 2;
                let cy = w.geometry.y + w.geometry.height / 2;
                dead.contains(cx, cy)
            })
            .map(|w| w.id)
            .collect();

        for id in affected {
            let Some(w) = self.window_manager.get(id) else { continue };
            let geometry = w.geometry;

            // Preserve the window's offset from the dead output's origin,
            // clamped so the frame fits inside the survivor's box. A frame
            // wider/taller than the survivor itself is centered instead
            // (task 8, F: a `max` bound below its `min` bound would panic
            // `clamp`, and pinning it to the survivor's own origin drew it
            // hard against one corner with all the overflow bleeding off a
            // single edge -- centering spreads the unavoidable overflow
            // symmetrically, which is what every other oversized-window
            // placement in this crate already does). The subtraction below
            // is total, not `clamp`ed -- an oversized frame legitimately
            // produces a negative offset, and there is no bound to violate.
            let new_x = if geometry.width >= survivor.width {
                survivor.x + (survivor.width - geometry.width) / 2
            } else {
                (survivor.x + (geometry.x - dead.x)).clamp(survivor.x, survivor.x + survivor.width - geometry.width)
            };
            let new_y = if geometry.height >= survivor.height {
                survivor.y + (survivor.height - geometry.height) / 2
            } else {
                (survivor.y + (geometry.y - dead.y)).clamp(survivor.y, survivor.y + survivor.height - geometry.height)
            };

            self.window_manager.set_geometry(id, Rectangle { x: new_x, y: new_y, ..geometry });
            self.sync_window_to_scene(id);
        }
        self.emit_pending();
    }

    /// Drain `window_manager.pending_events` onto `dbus_tx`. Must be called
    /// after every mutation of `window_manager` so subscribers observe it.
    ///
    /// Each event goes out as a `SeqEvent` carrying the `seq` its mutation
    /// produced (review finding I2); `apply_reloaded_config` pushes
    /// `apply_config`'s events through `WindowManager::push_event` for the
    /// same reason, so there is exactly one queue and one counter.
    pub fn emit_pending(&mut self) {
        for ev in self.window_manager.pending_events.drain(..) {
            let _ = self.dbus_tx.send(ev);
        }
    }

    /// Queue an event for the next `emit_pending()` drain without it having
    /// come from a `window_manager` mutation (e.g. `AltTabState`, which is
    /// driven by `alt_tab`, not `window_manager`).
    ///
    /// Task 11 re-review #4: this used to push straight onto
    /// `pending_events`, bypassing `WindowManager::bump()` -- every
    /// `AltTabState` went out without ever advancing `seq`, so
    /// `snapshot().seq` couldn't be used to detect that an alt-tab change
    /// had happened. Routed through `note_event()` (which bumps then
    /// returns the same `pending_events` vec) so it participates in the
    /// same sequence counter as every other event.
    pub fn emit(&mut self, ev: Event) {
        self.window_manager.push_event(ev);
    }

    // --- Model <-> Wayland reconciliation (review finding C1) ---
    //
    // Before this, `new_toplevel`'s `map_element(window, (0, 0), false)` was
    // the only call that ever positioned anything in the `Space`, and no
    // `send_configure`/`send_close` existed anywhere in the tree: every
    // client rendered at (0,0), was never told its size, and never learned
    // it had been asked to close, fullscreen, or maximize. The model, the
    // D-Bus surface, and the tests were all self-consistent -- it was the
    // model -> Wayland edge that was missing, and no single task owned it.
    //
    // The whole edge is these three functions plus their call sites: every
    // geometry/state mutation ends in `sync_window_to_scene`, every
    // workspace/visibility change ends in `sync_scene`, and every close goes
    // through `request_close`.

    /// Push model window `id`'s geometry, visibility, and xdg state out to its
    /// client: position its scene node at the model's position, hide it when
    /// the window isn't on the active workspace (review finding I1), and
    /// configure the toplevel with the model's size plus the
    /// maximized/fullscreen/activated states.
    ///
    /// Call this at the tail of *every* geometry or state mutation. A window
    /// with no backing client is a silent no-op.
    ///
    /// Coordinate spaces (re-review minor 3, carried over verbatim from the
    /// smithay implementation because the invariant is unchanged): the model
    /// (`window.rs`, `window_at`, `decoration::hit_test`, drag offsets) is in
    /// *frame* space; the scene node's position and the staged size are in
    /// *content* space -- for an SSD window they differ by `TITLE_BAR_HEIGHT`.
    /// Any consumer of scene coordinates (pointer hit-testing into client
    /// surfaces, above all) must convert with `decoration::content_rect`,
    /// never compare the two spaces directly.
    ///
    /// Note (re-review minor 4): the early return means the trailing
    /// `sync_seat_focus` only runs for windows that still have a client.
    /// Focus-clearing on removal paths must not rely on this tail --
    /// `forget_window` calls `sync_focus_change(None)` explicitly for exactly
    /// that reason.
    pub fn sync_window_to_scene(&mut self, id: WindowId) {
        if !self.wayland.is_backed(id) {
            return;
        }
        let Some(w) = self.window_manager.get(id) else {
            // The model window is gone but the client is still here (a close
            // that raced the client's destroy): hide it and drop the binding
            // rather than showing a window nothing tracks.
            self.wayland.set_visible(id, false);
            self.wayland.forget(id);
            // The other teardown path that bypasses `forget_window`
            // (review finding M1): the row vanished without going through
            // it, so the raster it left behind has to be collected here.
            self.title_rasters.remove(&id);
            if self.ssd_hover.is_some_and(|(hid, _)| hid == id) {
                self.ssd_hover = None;
            }
            if self.ssd_press.is_some_and(|(hid, _)| hid == id) {
                self.ssd_press = None;
            }
            return;
        };
        let (geo, fullscreen, maximized, focused) = (w.geometry, w.fullscreen, w.maximized, w.focused);
        let visible = self.window_manager.is_visible(w);
        // Re-review finding New-4: the model's geometry is the *frame*; an
        // SSD window's client owns only the band below the title bar.
        let ssd = crate::decoration::has_ssd(&w.app_id, w.client_decorations_requested, fullscreen);
        let content = crate::decoration::content_rect(geo, ssd);

        // Review finding I2: paint (or hide) the SSD decoration. `geo`,
        // not `content` -- `title_bar_rect` is frame-space, same as
        // `content_rect`'s input, and the two are computed from the same
        // `geo` precisely so they can never disagree about where the band
        // ends and the client's content begins.
        let bar = crate::decoration::title_bar_rect(geo);
        let bar_color = crate::render::hex_to_rgba(&self.config.appearance.palette.background);
        let button_colors = self.button_colors();
        // Only rasterize for a window that will actually show a band: a CSD
        // or fullscreen window's title is never drawn, and shaping it would
        // be pure waste on the most common client kind there is.
        let decorated = ssd && visible;
        // All three button rects share one size (`BUTTON_WIDTH x
        // TITLE_BAR_HEIGHT`); index 0 stands in for the cell every glyph is
        // rasterized into.
        let button_cell = crate::decoration::button_rects(bar)[0];
        let (gw, gh) = (button_cell.width.max(1), button_cell.height.max(1));
        if decorated {
            let title = w.title.clone();
            self.ensure_title_raster(id, title, bar, focused);
            self.ensure_button_glyphs(gw, gh);
        }
        // Whichever button this window currently shows pressed, or failing
        // that hovered -- a press implies the pointer is over that same
        // button, so it always takes precedence when both happen to be set.
        let pressed = self.ssd_press.filter(|(hid, _)| *hid == id);
        let active_button = pressed
            .or_else(|| self.ssd_hover.filter(|(hid, _)| *hid == id))
            .map(|(_, idx)| {
                let state = if pressed.is_some() {
                    crate::wayland::ButtonState::Pressed
                } else {
                    crate::wayland::ButtonState::Hover
                };
                (idx, state)
            });
        // Field-wise borrow so the seam can read the memos' pixels in place:
        // the alternative is cloning them into owned buffers on every sync
        // -- i.e. on every pointer motion of a drag -- to hand across a call
        // that, on a cache hit, will not even look at them (M2).
        let Self { wayland, title_rasters, button_glyph_cache, .. } = self;
        let title_px = decorated
            .then(|| title_rasters.get(&id))
            .flatten()
            .and_then(|entry| {
                entry.pixels.as_ref().map(|(width, height, pixels)| crate::wayland::TitleRaster {
                    width: *width,
                    height: *height,
                    generation: entry.generation,
                    pixels,
                })
            });
        let button_glyph_px: [Option<crate::wayland::GlyphRaster<'_>>; 3] = std::array::from_fn(|i| {
            if !decorated {
                return None;
            }
            let key = (i, gw, gh, BUTTON_GLYPH_FG);
            button_glyph_cache
                .get(&key)
                .and_then(|entry| entry.as_ref())
                .map(|pixels| crate::wayland::GlyphRaster { width: gw, height: gh, fg: BUTTON_GLYPH_FG, pixels })
        });
        wayland.sync_ssd(
            id,
            ssd,
            visible,
            bar,
            content,
            bar_color,
            button_colors,
            title_px,
            button_glyph_px,
            active_button,
        );

        self.wayland.set_visible(id, visible);
        if visible {
            self.wayland.set_position(id, content.x, content.y);
            // Ledger item 28 / recommendation 4: `behavior.raise_on_focus`
            // decides whether a focus change alone may restack. With it off a
            // focused window is still activated and configured, it just keeps
            // its place in the stack.
            //
            // LEDGER DECISION (task 8): the pre-port smithay code also raised
            // on *any* geometry change, with no `raise_on_focus` check at
            // all -- `Space::map_element` was both the move and the raise in
            // one call, so every move restacked whether or not the mover was
            // the focused window. That does not return here. It was an
            // artifact of `map_element`'s API shape, not a documented
            // behavior the seam owes: a pure move not restacking is the
            // better floating-WM behavior, and it is what `raise_on_focus`'s
            // own name promises -- a knob that says "raise on focus," not
            // "raise on focus or move." Reintroducing moved-implies-raise
            // would make the knob lie for drag-move, which is by far the most
            // common source of geometry changes.
            if focused && self.config.behavior.raise_on_focus {
                self.wayland.raise(id);
            }
        }
        self.wayland
            .configure(id, content, focused && visible, maximized, fullscreen);

        // Keyboard focus is the other half of "focus reached the client".
        self.sync_seat_focus();
    }

    /// The three title-bar button colors, in `decoration::button_rects`'
    /// order: minimize, maximize, close.
    ///
    /// Close is the palette's accent -- it is the destructive one and the
    /// one a user aims at without looking. The other two are the foreground
    /// color at 40% alpha, which reads as a subdued chip against the band
    /// whatever the palette is, without inventing two more color names the
    /// config has no field for.
    ///
    /// Premultiplied, like every other color that reaches a scene node: the
    /// wlroots scene graph composites premultiplied alpha, so scaling only
    /// the alpha channel and leaving RGB at full strength would paint the
    /// chips brighter than opaque foreground rather than fainter.
    fn button_colors(&self) -> [[f32; 4]; 3] {
        let fg = crate::render::hex_to_rgba(&self.config.appearance.palette.foreground);
        let accent = crate::render::hex_to_rgba(&self.config.appearance.palette.accent);
        let chip = premultiply(fg, 0.4);
        [chip, chip, accent]
    }

    /// Make sure window `id`'s memoized title raster matches `title`, the
    /// band `bar` and the focus state -- shaping only when one of those
    /// actually changed.
    ///
    /// Memoized through `title_rasters`: see that field's doc for why a drag
    /// must not re-shape (or re-upload) the same string sixty times a
    /// second, and why a negative result is cached too. The caller reads the
    /// pixels back out of the map rather than taking them from here, which
    /// is what keeps a cache hit free of any copy at all.
    ///
    /// The title node spans the band minus the three buttons, so a long
    /// title runs out of room before it runs under the close button rather
    /// than being drawn beneath it.
    fn ensure_title_raster(&mut self, id: WindowId, title: String, bar: Rectangle, focused: bool) {
        // Finding 3, security: cap the title before it becomes the cache
        // key, not just before it reaches `rasterize_title` -- otherwise
        // two distinct multi-kilobyte titles that agree on their first
        // `MAX_TITLE_BYTES` bytes would shape identically but still cache
        // (and compare) as different keys, defeating the point of capping.
        // `rasterize_title` caps again internally (its own contract, kept
        // independent of this caller), so this is belt and suspenders on
        // the same bound, not two different ones.
        let title = if title.len() > crate::text::MAX_TITLE_BYTES { crate::text::cap_title(&title).to_owned() } else { title };
        let width = (bar.width - 3 * crate::decoration::BUTTON_WIDTH).max(1);
        let height = bar.height.max(1);

        // Unfocused windows get the same text at 60% strength rather than a
        // second palette color: the config has one foreground, and dimming
        // it is the least surprising way to say "this window is not the
        // active one" in a title bar that is otherwise identical.
        //
        // Dimmed in RGB, not in alpha, and that is not a style choice:
        // cosmic-text's `SwashCache::with_pixels` builds each mask-glyph
        // pixel as `coverage << 24 | base.0 & 0xFF_FF_FF` -- it takes the
        // base color's *channels* and discards its alpha outright (the
        // upstream source says as much, in a `TODO: blend base alpha?`).
        // Asking for translucent text by lowering the alpha byte would
        // therefore change nothing at all on screen.
        let palette = crate::render::hex_to_rgba(&self.config.appearance.palette.foreground);
        let dim = if focused { 1.0 } else { 0.6 };
        let to_u8 = |c: f32| (c.clamp(0.0, 1.0) * dim * 255.0).round() as u8;
        let fg = [to_u8(palette[0]), to_u8(palette[1]), to_u8(palette[2]), 255];

        let key: TitleRasterKey = (title, width, fg);
        if self.title_rasters.get(&id).is_some_and(|entry| entry.key == key) {
            return;
        }

        self.fonts
            .get_or_init(|| (cosmic_text::FontSystem::new(), cosmic_text::SwashCache::new()));
        // `get_mut` cannot miss after `get_or_init`; the `else` is the
        // panic-free spelling of that, not a case with any behavior of its
        // own.
        let Some((fonts, swash)) = self.fonts.get_mut() else { return };
        let pixels = crate::text::rasterize_title(fonts, swash, &key.0, width, height, TITLE_PAD_X, fg)
            .map(|pixels| (width, height, pixels));
        // A fresh generation for fresh pixels: this is what tells the seam
        // its title node is out of date (M2). Bumped on a negative result
        // too -- "the title now shapes to nothing" is a change the seam has
        // to act on, by dropping the node.
        let generation = self.next_title_generation;
        self.next_title_generation += 1;
        self.title_rasters.insert(id, TitleRasterEntry { key, generation, pixels });
    }

    /// Make sure every `BUTTON_GLYPHS` entry has a rasterized `width x
    /// height` @ `BUTTON_GLYPH_FG` entry in `button_glyph_cache`, shaping
    /// only the ones that are missing.
    ///
    /// No per-window key, unlike `ensure_title_raster`: a button glyph's
    /// pixels depend only on its index, the cell size, and the (constant)
    /// glyph color, none of which vary per window -- so the cache this fills
    /// is shared by every decorated window in the compositor, and steady
    /// -state calls here are a `contains_key` check per index, not a shape.
    fn ensure_button_glyphs(&mut self, width: i32, height: i32) {
        for (i, glyph) in BUTTON_GLYPHS.iter().enumerate() {
            let key = (i, width, height, BUTTON_GLYPH_FG);
            if self.button_glyph_cache.contains_key(&key) {
                continue;
            }
            self.fonts
                .get_or_init(|| (cosmic_text::FontSystem::new(), cosmic_text::SwashCache::new()));
            // Same panic-free spelling as `ensure_title_raster`'s: `get_mut`
            // cannot miss right after `get_or_init`.
            let Some((fonts, swash)) = self.fonts.get_mut() else { return };
            let pixels = crate::text::rasterize_glyph(fonts, swash, glyph, width, height, BUTTON_GLYPH_FG);
            self.button_glyph_cache.insert(key, pixels);
        }
    }

    /// The active workspace's focused window id, if any. Callers capture this
    /// *before* a focus-changing mutation and hand it to `sync_focus_change`.
    fn focused_id(&self) -> Option<WindowId> {
        self.window_manager.focused_window().map(|w| w.id)
    }

    /// Point the seat's keyboard at whatever the model currently says is
    /// focused -- or at nothing, when it says nothing is.
    ///
    /// Re-review finding New-3: seat keyboard focus used to be set-only, so
    /// after the last window on a workspace was closed, minimized, or left
    /// behind by a workspace switch, the seat kept pointing at a surface the
    /// user could no longer see. Driving the target from the model -- not from
    /// whichever window a caller happens to be syncing -- makes this
    /// idempotent, which is why `sync_window_to_scene` can call it
    /// unconditionally. The library compares against the seat's current focus
    /// and does nothing when it already matches, so an ordinary geometry sync
    /// does not churn leave/enter pairs at the client.
    fn sync_seat_focus(&mut self) {
        // Review finding J3: this used to be unconditional, which meant
        // *any* `sync_window_to_scene`-driven resync -- a drag-motion
        // frame, a title change, a workspace switch, `arrange_layers`
        // itself -- silently reasserted the model's toplevel focus over
        // an interactive layer surface's, because the crate's seat model
        // has no separate "layer focus" slot: the next call to
        // `focus_toplevel_keyboard`/`clear_keyboard_focus` (which this
        // method's tail makes, via `wayland.keyboard_focus`) replaces
        // whatever `focus_layer_keyboard` last gave a layer surface. So a
        // keyboard-interactive panel lost the keyboard within one frame
        // of being given it. While `layer_focus` names a still-mapped
        // entry, this leaves the seat's real focus alone entirely; only
        // the paths that actually drop layer focus
        // (`layer_surface_unmapped`/`destroyed`, plus
        // `release_layer_focus` -- see re-review finding Important-1)
        // clear the field first, which is what lets a later call here
        // reach the toplevel branch again. `layer_holds_keyboard_focus`'s
        // own unit tests exercise this guard's exact logic directly;
        // there is no test that observes its effect on a real seat's
        // keyboard focus end-to-end, for a harness limitation documented
        // on `arrange_layers_leaves_layer_focus_alone_and_unmap_clears_it`
        // (this file's `mod tests`).
        if layer_holds_keyboard_focus(self.layer_focus, &self.layers) {
            return;
        }
        let focused = self
            .focused_id()
            .filter(|&id| self.window_manager.is_visible_id(id))
            .filter(|&id| self.wayland.is_backed(id));
        self.wayland.keyboard_focus(focused);
    }

    /// Drop `layer_focus`, if held, so the next `sync_seat_focus` reaches
    /// its toplevel branch instead of being blocked by
    /// `layer_holds_keyboard_focus`'s guard.
    ///
    /// Re-review finding Important-1: round 1's `sync_seat_focus` guard
    /// closed the *passive* leak (`arrange_layers` and any other
    /// `sync_window_to_scene`-driven resync must never steal focus from a
    /// mapped interactive layer surface -- that must stay closed, and
    /// still is: nothing below calls this), but implemented only the
    /// guard's first clause. Missing was its second: *something* has to
    /// clear `layer_focus` when a toplevel focus is genuinely,
    /// user-, D-Bus-, or alt-tab-intentionally being asserted over it,
    /// or no toplevel can ever take the keyboard again once any
    /// interactive layer surface has ever mapped. Called at the top of
    /// every EXPLICIT toplevel-focus assertion -- `handle_pointer_press`
    /// (click-to-focus), `DbCommand::Focus`, and the `cycle:alt_tab`
    /// action's per-step focus -- each of which calls
    /// `sync_focus_change`/`sync_seat_focus` of its own accord shortly
    /// after, which is what actually pushes the toplevel focus out; this
    /// method itself makes no seat call; it only clears the field the
    /// guard reads.
    fn release_layer_focus(&mut self) {
        self.layer_focus = None;
    }

    /// Reconcile a focus transition all the way out to the clients.
    /// `previous` is what `focused_id()` returned *before* the model
    /// mutation; pass `None` when the previously focused window is already
    /// gone from the model (a close/destroy).
    ///
    /// Re-review findings New-1 and New-2. New-1: every focus path synced
    /// only the window that *gained* focus, so the one that lost it kept its
    /// `Activated` xdg state and its client went on rendering itself as the
    /// active window -- moving focus A -> B left two windows looking focused.
    /// New-2: when the focused toplevel was destroyed the model picked a
    /// successor (`forget_window` -> `focus_mru_in_workspace`) that was never
    /// synced, so the successor's client was never activated and never
    /// received seat keyboard focus -- the keyboard was dead until the user
    /// clicked something. Both are the same missing step: a focus change has
    /// two ends, and both have to be pushed.
    ///
    /// Wayland-side only: the model mutation that moved focus already emitted
    /// its own `WindowUpdated` events (`WindowManager::focus` emits for the
    /// window losing focus as well as the one gaining it), so nothing here
    /// emits.
    fn sync_focus_change(&mut self, previous: Option<WindowId>) {
        let current = self.focused_id();
        if let Some(prev) = previous.filter(|prev| Some(*prev) != current) {
            self.sync_window_to_scene(prev);
        }
        match current {
            Some(id) => self.sync_window_to_scene(id),
            // Nothing focused: `sync_window_to_scene` isn't reached at all,
            // so clear the seat here (New-3).
            None => self.sync_seat_focus(),
        }
    }

    /// Minimize or restore window `id`, handing focus to the workspace's MRU
    /// successor when the focused window is the one being minimized, then
    /// reconcile both ends of the transition out to the clients.
    ///
    /// Re-review Important 1: `DbCommand::Minimize` mutated the model and
    /// synced only `id`, bypassing the focus-transition path entirely --
    /// minimizing the focused window from the taskbar (the common entry
    /// point) left the model's focus on a now-invisible window, activated no
    /// successor, and `sync_seat_focus`'s visibility filter then cleared the
    /// seat: a dead keyboard with windows still on screen, New-2's symptom
    /// through a different door. The title-bar button already did this
    /// correctly; this is that arm's body, extracted so "minimize a window"
    /// has exactly one implementation, matching the maximize/fullscreen
    /// handlers' pattern.
    fn set_minimized_and_reconcile(&mut self, id: WindowId, value: bool) -> Option<()> {
        let previous = self.focused_id();
        let target = self.window_manager.get(id)?;
        let workspace = target.workspace;
        let was_focused_on_own_workspace = target.focused;
        self.window_manager.set_minimized(id, value)?;
        if value && was_focused_on_own_workspace {
            // Task 6: resolve `id`'s *own* workspace, not whichever one is
            // currently active -- `DbCommand::Minimize` can target a window
            // on an inactive workspace, and `focused_id()`/`previous` above
            // only ever reflects the active one. `refocus_after_hide` clears
            // the pointer outright when nothing qualifies, so a minimized
            // window can never linger as that workspace's `focused_window`
            // (masked only at the seat by `sync_seat_focus`'s visibility
            // filter until now).
            self.window_manager.refocus_after_hide(workspace);
        }
        // `id` itself always needs a sync (its visibility just changed),
        // even when it wasn't the focused window; `sync_focus_change` then
        // handles the successor and the seat (New-1/2/3). On restore, if the
        // model's focus pointer never left `id`, the visibility filter now
        // passes again and this same pair re-activates it and returns it the
        // keyboard.
        self.sync_window_to_scene(id);
        self.sync_focus_change(previous);
        Some(())
    }

    /// Reconcile every model window with its client at once. Used after
    /// changes that can alter many windows' visibility in one go (workspace
    /// switch, config reload).
    ///
    /// Per-window syncing covers the focus transition's two ends on its own
    /// (every window is synced, including whichever one just lost focus), but
    /// the trailing `sync_seat_focus` is still needed for New-3's
    /// "switched to an empty workspace" case: with nothing focused *and*
    /// possibly nothing to iterate, the loop body may never run.
    pub fn sync_scene(&mut self) {
        let ids: Vec<WindowId> = self.window_manager.windows().map(|w| w.id).collect();
        for id in ids {
            self.sync_window_to_scene(id);
        }
        self.sync_seat_focus();
    }

    /// Ask window `id` to close.
    pub fn request_close(&mut self, id: WindowId) {
        if self.wayland.close(id) {
            // A real client: the model row stays until the client actually
            // destroys its toplevel, which lands in `forget_toplevel`.
            return;
        }
        self.wayland.forget(id);
        self.forget_window(id);
        self.emit_pending();
    }

    /// Drop every trace of `id` from the model and its side tables, then
    /// hand focus to whatever is left on the active workspace (review
    /// finding I6's coherence requirement, applied to closes as well as
    /// moves: an action right after a close must not no-op on a dangling
    /// focus pointer).
    fn forget_window(&mut self, id: WindowId) {
        self.wayland.forget(id);
        self.window_manager.remove_window(id);
        self.fullscreen_saved_geometry.remove(&id);
        self.snap_saved_geometry.remove(&id);
        self.maximized_saved_geometry.remove(&id);
        // The scene nodes went with `wayland.forget` above; this is their
        // CPU-side memo, and a window's title pixels must not outlive it
        // (ids are never reused, so a stale entry would simply leak).
        self.title_rasters.remove(&id);
        // Same reasoning: a hover/press pointed at a window that no longer
        // exists must not linger to be read by some later window's sync.
        if self.ssd_hover.is_some_and(|(hid, _)| hid == id) {
            self.ssd_hover = None;
        }
        if self.ssd_press.is_some_and(|(hid, _)| hid == id) {
            self.ssd_press = None;
        }
        if self.window_manager.focused_window().is_none() {
            let active = self.window_manager.active_workspace();
            self.window_manager.refocus_after_hide(active);
        }
        // Re-review finding New-2: picking a successor in the model was only
        // half the job -- it also has to reach the client (`Activated`) and
        // the seat (keyboard focus), and when the workspace has no successor
        // left the seat's focus has to be cleared rather than left pointing
        // at the destroyed surface. `previous` is `None` because `id` is
        // already out of the model by this point, so there is nothing left
        // to sync on the losing end.
        self.sync_focus_change(None);
    }

    /// Toggle fullscreen state for a window. When entering fullscreen, saves the
    /// current geometry and sets geometry to the output rect. When exiting, restores
    /// the saved geometry. Emits WindowUpdated event.
    pub fn toggle_fullscreen(&mut self, id: WindowId) -> Option<()> {
        let w = self.window_manager.get(id)?;
        let target = !w.fullscreen;
        self.set_fullscreen_target(id, target)
    }

    /// Set fullscreen to an explicit `target` value, saving/restoring
    /// geometry exactly like `toggle_fullscreen` (which is now a thin
    /// `target = !current` wrapper around this). Added for task 12's
    /// `DbCommand::Fullscreen(id, toggle)` D-Bus command, whose `toggle`
    /// argument (despite the name -- it's the interface method's parameter
    /// name from the brief) is an explicit target state, not a flip
    /// request; a `FullscreenWindow(id, true)` call on an
    /// already-fullscreen window must stay a no-op rather than treating the
    /// current geometry as a fresh "pre-fullscreen" save point and
    /// clobbering the real one.
    pub fn set_fullscreen_target(&mut self, id: WindowId, target: bool) -> Option<()> {
        let w = self.window_manager.get(id)?;
        if w.fullscreen == target {
            return Some(());
        }
        // H1: resolved via the window's own frame center
        // (`output_for_window`), not the pointer -- this is reached from
        // client requests and `DbCommand::Fullscreen`, neither of which
        // correlates the pointer with the target window, so the old
        // `output_for_pointer` call fullscreened onto whatever output the
        // mouse happened to be resting on. `w.geometry` is read here,
        // before the borrow on `w` would otherwise need to outlive the
        // mutable calls below.
        let geometry = w.geometry;
        let output_geo = self.output_for_window(geometry).and_then(|idx| self.outputs.get(&idx)).map(|o| o.geometry)?;
        self.window_manager.set_fullscreen(id, target)?;
        if target {
            // Save current geometry before entering fullscreen
            if let Some(w) = self.window_manager.get(id) {
                self.fullscreen_saved_geometry.insert(id, w.geometry);
            }
            self.window_manager.set_geometry(id, output_geo)?;
        } else {
            // Restore saved geometry when exiting fullscreen
            let saved = self.fullscreen_saved_geometry.remove(&id)?;
            self.window_manager.set_geometry(id, saved)?;
        }
        self.sync_window_to_scene(id);
        self.emit_pending();
        Some(())
    }

    /// Toggle maximized state for `id`.
    pub fn toggle_maximized(&mut self, id: WindowId) -> Option<()> {
        let target = !self.window_manager.get(id)?.maximized;
        self.set_maximized_target(id, target)
    }

    /// Set maximized to an explicit `target`, computing and applying real
    /// geometry (the output rect inset by `snap_gap`) with its own restore
    /// slot, mirroring `set_fullscreen_target`.
    ///
    /// Review finding I5: `toggle_maximized` used to flip a flag and emit,
    /// with no geometry and no configure -- the maximize button, the
    /// `MaximizeWindow` D-Bus method, and the client's own
    /// `xdg_toplevel.set_maximized` were all visual no-ops.
    pub fn set_maximized_target(&mut self, id: WindowId, target: bool) -> Option<()> {
        let w = self.window_manager.get(id)?;
        if w.maximized == target {
            return Some(());
        }
        // Maximize honors the usable area (panels' exclusive zones carve
        // into it); fullscreen, just below, deliberately keeps the full
        // `geometry` instead.
        //
        // H1: resolved via the window's own frame center
        // (`output_for_window`), not the pointer -- see
        // `set_fullscreen_target`'s identical fix just above for why.
        let geometry = w.geometry;
        let output_geo = self.output_for_window(geometry).and_then(|idx| self.outputs.get(&idx)).map(|o| o.usable)?;
        self.window_manager.set_maximized(id, target)?;
        if target {
            let current = self.window_manager.get(id)?.geometry;
            self.maximized_saved_geometry.insert(id, current);
            let gap = self.config.appearance.snap_gap;
            self.window_manager.set_geometry(id, layout::maximized_geometry(output_geo, gap))?;
        } else if let Some(saved) = self.maximized_saved_geometry.remove(&id) {
            self.window_manager.set_geometry(id, saved)?;
        }
        self.sync_window_to_scene(id);
        self.emit_pending();
        Some(())
    }

    /// Apply a client-requested maximize/unmaximize for `toplevel`, reporting
    /// whether the model actually changed -- i.e. whether
    /// `sync_window_to_scene` will have sent the client a configure of its
    /// own. `false` (unknown toplevel, state already as requested, or a
    /// failed precondition such as no known output) tells the caller to
    /// answer with a bare configure instead, so the request is never left
    /// unanswered.
    ///
    /// Wired to `ToplevelHandler::request_maximize`.
    pub fn reconcile_maximized(&mut self, toplevel: crate::wayland::ToplevelKey, target: bool) -> bool {
        let Some(id) = self.wayland.window_for(toplevel) else { return false };
        let changes = self.window_manager.get(id).is_some_and(|w| w.maximized != target);
        changes && self.set_maximized_target(id, target).is_some()
    }

    /// Fullscreen counterpart of [`Self::reconcile_maximized`]; same
    /// "did a configure actually go out" contract. Wired to
    /// `ToplevelHandler::request_fullscreen`.
    pub fn reconcile_fullscreen(&mut self, toplevel: crate::wayland::ToplevelKey, target: bool) -> bool {
        let Some(id) = self.wayland.window_for(toplevel) else { return false };
        let changes = self.window_manager.get(id).is_some_and(|w| w.fullscreen != target);
        changes && self.set_fullscreen_target(id, target).is_some()
    }

    /// Synchronously reload the config from `self.config_path` (falling
    /// back to `icedtea_config::default_db_path()` when unset) and apply it.
    /// Returns the events `apply_config` produced (which already includes a
    /// trailing `Event::ConfigReloaded` -- see that method's doc -- so
    /// nothing is pushed again here).
    ///
    /// This is the *synchronous* load+apply path -- both the load and the
    /// apply happen on whatever thread calls it, in one blocking call. It
    /// exists for tests (task-13's Step-1 test calls it against a temp DB,
    /// where the extra thread hop of the async path would just be noise)
    /// and is *not* wired to either live reload trigger: per the module's
    /// threading requirement, the redb I/O + JSON parse must never run on
    /// the render loop, so neither `handle_command`'s `ReloadConfig` arm nor
    /// `apply_action`'s `"reload"` arm calls this directly -- they both go
    /// through `spawn_config_reload`'s worker-thread + channel path instead,
    /// which lands on `apply_reloaded_config` (the same apply+emit tail this
    /// method also uses) once the load finishes off-loop.
    ///
    /// NOTE (brief deviation): the brief's Step-3 sample pushed a *second*
    /// `Event::ConfigReloaded(self.config.appearance.clone())` after calling
    /// `apply_config`. `apply_config` (task 11) already appends its own
    /// `Event::ConfigReloaded(cfg.appearance.clone())` to the vec it
    /// returns, so following the sample literally would emit the signal
    /// twice per reload -- a genuine duplicate-event bug, not just
    /// transcription noise (the standing human ruling is to fix genuine
    /// sample bugs and document them here). Dropped; `apply_config`'s event
    /// is the one and only `ConfigReloaded` this path emits.
    pub fn reload_config_from_disk(&mut self) -> Vec<Event> {
        let path = self.config_path.clone().unwrap_or_else(icedtea_config::default_db_path);
        let cfg = icedtea_config::load_or_default(&path);
        self.apply_reloaded_config(cfg)
    }

    /// Apply a `Config` (already loaded, whether synchronously by
    /// `reload_config_from_disk` or on a worker thread by
    /// `handle_command`'s `ReloadConfig` async path): calls `apply_config`,
    /// pushes its events onto the ordinary seq-tagged event queue, and flushes them
    /// immediately (via `emit_pending`) so the D-Bus emitter thread
    /// observes `ConfigReloaded` (and any `WindowClosed`/`WorkspaceList`/
    /// terminal `AltTabState` it carries) right away rather than waiting for
    /// some unrelated later mutation to flush the queue. Returns the same
    /// events, for callers (like `reload_config_from_disk`'s test) that want
    /// to inspect what was produced.
    pub fn apply_reloaded_config(&mut self, cfg: Config) -> Vec<Event> {
        // Captured before `apply_config` overwrites `self.config` -- this is
        // the only way to tell whether the wallpaper *path* changed rather
        // than merely being reapplied.
        let old_wallpaper = self.config.appearance.wallpaper.clone();

        let events = self.apply_config(cfg);
        // Review finding I2: these used to be extended onto a separate
        // `pending_config_events` vec that bypassed the sequence counter
        // entirely, so the reload's events went out with no seq of their
        // own. `apply_config`'s returned `events` (the summary
        // `WorkspaceList`/`ConfigReloaded`, plus a terminal `AltTabState` if
        // a cycle was in flight) are the ones without a seq yet; pushing them
        // through the ordinary queue gives each a real, monotonically
        // increasing seq after any per-window events `apply_config` already
        // queued (e.g. a workspace migration's `WindowUpdated`), preserving
        // their order.
        for ev in &events {
            self.window_manager.push_event(ev.clone());
        }

        // Task 14 gap-close: `apply_config` swaps `self.config` in but never
        // touched anything downstream of it -- the background rect kept the
        // old color, a wallpaper path change never re-decoded, and
        // (already covered above via `apply_config`'s own
        // `input::warn_about_keybindings` call) keybinding revalidation was
        // the one piece that already worked.
        if let Some(bg) = self.background
            && let Some(runtime) = self.wayland.runtime()
        {
            runtime.set_rect_color(bg, render::wallpaper_color(&self.config.appearance));
        }
        if self.config.appearance.wallpaper != old_wallpaper {
            // Carried-forward obligation (task 7 review): clear the decoded
            // image and tear down every wallpaper node *before* the fresh
            // decode can land, so `sync_wallpaper_nodes`'s existing-node
            // branch (which never refreshes pixels in place -- see its own
            // doc) is never asked to show stale pixels under a new path.
            self.wallpaper.set_decoded(None);
            self.sync_wallpaper_nodes();
            let path = self.config.appearance.wallpaper.clone();
            self.spawn_wallpaper(path);
        }
        // `apply_config` now preserves every window row across the reload, so
        // this re-sync repaints the surviving windows against the swapped-in
        // appearance (palette/bar colors) -- exactly the case this
        // unconditional call was left in place to cover.
        self.sync_scene();

        self.emit_pending();
        events
    }

    /// Spawn the off-loop worker thread shared by both async reload
    /// triggers -- `handle_command`'s `DbCommand::ReloadConfig` arm and
    /// `apply_action`'s `"reload"` arm (bound to `SUPER+SHIFT+r` by
    /// default). Loads `self.config_path` (falling back to
    /// `icedtea_config::default_db_path()`) on the worker thread and ships
    /// the result back over `config_reload_tx`; `drain_config_reload` picks
    /// it up on the next turn once it arrives. A no-op if `config_reload_tx`
    /// hasn't been wired yet (only possible before `set_config_reload_sender`
    /// has run).
    fn spawn_config_reload(&self) {
        let Some(tx) = self.config_reload_tx.clone() else { return };
        let path = self.config_path.clone().unwrap_or_else(icedtea_config::default_db_path);
        // `try_clone` dup(2)s the wake pipe's write half so the worker
        // thread owns a handle it can move across the `'static` bound
        // rather than borrowing `self`'s. `None` (no test wires this, and a
        // failed `try_clone` degrades the same way) means no wake is sent
        // and `drain_config_reload`'s periodic poll from `should_stop`
        // remains the only way the result is ever picked up -- exactly the
        // pre-wake-pipe behavior, not a regression.
        let wake = self.config_reload_wake.as_ref().and_then(|w| w.try_clone().ok());
        std::thread::spawn(move || {
            let cfg = icedtea_config::load_or_default(&path);
            let _ = tx.send(cfg);
            if let Some(wake) = wake {
                crate::backend::wake(&wake);
            }
        });
    }

    /// Spawn the wallpaper decode worker and wire up the receiving end of
    /// its result channel. The one and only caller in a real boot is
    /// `run()`; tests that want to exercise the exact wake-pipe path a real
    /// boot takes call this too, rather than `render::spawn_wallpaper_decode`
    /// directly.
    ///
    /// Mirrors `spawn_config_reload`'s wake-pipe handling exactly (review
    /// finding C1): the worker thread is handed a `try_clone`d copy of
    /// `self.wallpaper_wake`, never the original. Before this fix, callers
    /// passed the wake pipe's only write half straight into the worker
    /// thread by value; it dropped the moment that (one-shot, exits after
    /// its single send) thread ended, leaving the registered read end with a
    /// permanent `EPOLLHUP` on libwayland's level-triggered loop for the
    /// rest of the process's life -- `fd_ready`'s wallpaper arm firing, and
    /// `drain_wallpaper` re-entering its (now also fixed, see M4)
    /// "disconnected" arm, on every single turn forever. Keeping the
    /// original alive on `State` (`set_wallpaper_wake`, called once at boot)
    /// closes that off the same way `config_reload_wake` already does for
    /// its own channel.
    pub fn spawn_wallpaper(&mut self, path: Option<String>) {
        let wake = self.wallpaper_wake.as_ref().and_then(|w| w.try_clone().ok());
        let rx = crate::render::spawn_wallpaper_decode(path, wake);
        self.wallpaper_rx = Some(rx);
    }

    /// Central dispatcher for every [`crate::dbus::DbCommand`] received from
    /// the D-Bus service thread (see `dbus.rs`'s module doc for the
    /// threading model). Mirrors `apply_action`'s shape: `?`-propagates a
    /// failed precondition as `None` (skipping the trailing
    /// `emit_pending()`, which would be a no-op anyway since nothing queued
    /// on a failed mutation), `Some(())` on success.
    ///
    /// `ReloadConfig` is the one arm that doesn't mutate anything itself:
    /// per this task's threading requirement, the redb I/O + JSON parse
    /// must never block the render loop, so this spawns a worker thread
    /// (see `reload_config_from_disk`'s doc) that loads the config off-loop
    /// and ships the result back over `config_reload_tx`; `drain_config_reload`
    /// is what actually calls `apply_reloaded_config` once the load
    /// finishes. If `config_reload_tx` hasn't been wired yet, this is a
    /// no-op.
    pub fn handle_command(&mut self, cmd: crate::dbus::DbCommand) -> Option<()> {
        use crate::dbus::DbCommand;
        match cmd {
            DbCommand::Focus(id) => {
                let previous = self.focused_id();
                self.window_manager.focus(id)?;
                // Re-review finding Important-1: an explicit toplevel-focus
                // assertion -- see `release_layer_focus`'s own doc. Round-2
                // finding Important-2: after the `?`, not before -- `id`
                // can name a stale/unknown window (this arm is reachable
                // from a D-Bus caller with a wire id this compositor never
                // heard of), and an early return must leave `layer_focus`
                // exactly as it was, not silently defeat a panel's
                // keyboard grab for a focus assertion that never actually
                // happened.
                self.release_layer_focus();
                self.sync_focus_change(previous);
            }
            DbCommand::Close(id) => self.request_close(id),
            DbCommand::Minimize(id, value) => self.set_minimized_and_reconcile(id, value)?,
            DbCommand::Maximize(id, value) => self.set_maximized_target(id, value)?,
            DbCommand::Fullscreen(id, value) => self.set_fullscreen_target(id, value)?,
            DbCommand::SetWorkspace(id) => {
                self.switch_workspace(id)?;
            }
            DbCommand::MoveToWorkspace(id, workspace) => self.move_to_workspace(id, workspace)?,
            DbCommand::GetState(reply_tx) => {
                let _ = reply_tx.send(self.window_manager.snapshot());
                // No model mutation happened; nothing new to flush. Return
                // early so the unconditional `emit_pending()` below (a
                // no-op here, but let's not rely on that) stays meaningful
                // for every other arm.
                return Some(());
            }
            DbCommand::ReloadConfig => {
                self.spawn_config_reload();
                return Some(());
            }
            DbCommand::Quit => self.quitting = true,
        }
        self.emit_pending();
        Some(())
    }

    /// Switch the active workspace to `workspace`, give it a focused window
    /// if it has a focusable one, and reconcile the scene so only its
    /// windows are visible (review findings I1 and I6).
    pub fn switch_workspace(&mut self, workspace: u32) -> Option<()> {
        if !self.window_manager.set_active_workspace(workspace) {
            return None;
        }
        // Review finding I2, second half: `is_none()` alone let a focus
        // pointer that names an *unfocusable* row (unmapped, or minimized --
        // the same shape, which predates this branch) survive a workspace
        // switch and go on answering `close`/`maximize`/`snap` while the
        // seat correctly refused it. `release_focus` on the unmap path is
        // what stops the unmapped case from arising at all; this is the
        // belt-and-braces re-pick for anything that still slips through.
        let current = self.window_manager.focused_window().map(|w| w.id);
        if current.is_none_or(|id| !self.window_manager.is_visible_id(id)) {
            self.window_manager.refocus_after_hide(workspace);
        }
        self.sync_scene();
        Some(())
    }

    /// Move `id` to `workspace`, switch to it, and focus the moved window
    /// there -- leaving the origin workspace focused on whatever it has left.
    ///
    /// Review finding I6: `set_workspace` cleared the origin's focus pointer
    /// and never set the destination's, so after `MoveToWorkspace` the
    /// active workspace had no focused window and the very next
    /// `close`/`fullscreen`/`snap` action silently no-opped.
    pub fn move_to_workspace(&mut self, id: WindowId, workspace: u32) -> Option<()> {
        let origin = self.window_manager.get(id)?.workspace;
        self.window_manager.set_workspace(id, workspace)?;
        if origin != workspace {
            self.window_manager.refocus_after_hide(origin);
        }
        if !self.window_manager.set_active_workspace(workspace) {
            return None;
        }
        self.window_manager.focus(id);
        self.sync_scene();
        Some(())
    }

    /// Get the decoration action for a given window at the specified local coordinates.
    /// Returns the action if the window is focused and not fullscreen, None otherwise.
    /// Note: This gates on focused state (click-to-focus design), not CSD state.
    pub fn decoration_action_for(&self, id: WindowId, local: (i32, i32)) -> Option<crate::decoration::DecorationAction> {
        let w = self.window_manager.get(id)?;
        if !w.focused || w.fullscreen {
            return None;
        }
        Some(crate::decoration::hit_test(w.geometry, local))
    }

    /// Apply a decoration action to a window: Close removes it, Maximize toggles maximized state,
    /// Minimize minimizes, Move would be handled by the input layer (not here).
    pub fn apply_decoration_action(&mut self, id: WindowId, action: crate::decoration::DecorationAction) {
        use crate::decoration::DecorationAction;
        match action {
            DecorationAction::Close => {
                // Asks the client to close (and lets its own destroy drive
                // the model removal) rather than dropping the model row out
                // from under a still-running client -- see `request_close`.
                self.request_close(id);
            }
            DecorationAction::Maximize => {
                let _ = self.toggle_maximized(id);
            }
            DecorationAction::Minimize => {
                let _ = self.set_minimized_and_reconcile(id, true);
            }
            DecorationAction::Move => {
                // Move is handled by the input layer (pointer drag),
                // not by a discrete window-manager mutation.
            }
            DecorationAction::None => {
                // No action.
            }
        }
        self.emit_pending();
    }

    /// Dispatch a keybinding action string (e.g. from `handle_key`, or a
    /// D-Bus-triggered action). Returns `None` for unrecognized actions or
    /// when a precondition (no focused window, unknown output, etc.) isn't
    /// met; `Some(())` on success. Always drains pending events on success.
    pub fn apply_action(&mut self, action: &str) -> Option<()> {
        let mut parts = action.splitn(2, ':');
        let base = parts.next()?;
        let arg = parts.next().map(|s| s.to_string());
        match base {
            "close" => {
                let id = self.window_manager.focused_window()?.id;
                self.request_close(id);
            }
            "maximize" => {
                let id = self.window_manager.focused_window()?.id;
                self.toggle_maximized(id)?;
            }
            "fullscreen" => {
                let id = self.window_manager.focused_window()?.id;
                self.toggle_fullscreen(id)?;
            }
            "quit" => self.quitting = true,
            // Task 13 threading requirement (binding, carried over from the
            // task-7 threading-model ruling): the redb I/O + JSON parse must
            // never block the render loop, for the keybinding-triggered
            // reload just as much as the D-Bus `ReloadConfig` method --
            // `handle_command`'s `ReloadConfig` arm and this arm share the
            // exact same worker-thread dispatch for that reason.
            "reload" => self.spawn_config_reload(),
            // NOTE (brief deviation): the sample matched on the literal
            // `"cycle:alt_tab"` here, but `action.splitn(2, ':')` above
            // already split that string into `base = "cycle"`, `arg =
            // Some("alt_tab")` -- a match on `base` can never see the colon,
            // so as written this arm was as unreachable as the
            // `"snap:restore"` arm noted above. Matching `"cycle"` and
            // checking `arg` makes it reachable (and leaves room for other
            // `cycle:*` variants later without another silent dead arm).
            //
            // Task 11 review #1: a session's entry list is now captured
            // once by `AltTabMachine::start` and kept stable for the whole
            // session -- steps read it back via `self.alt_tab.entries()`
            // instead of recomputing `alt_tab_entries()` (and therefore a
            // possibly different length/order) on every keypress. The
            // session itself is ended by `end_alt_tab` (see its doc for the
            // chosen end condition), not here.
            "cycle" => {
                if arg.as_deref() != Some("alt_tab") {
                    return None;
                }
                if !self.alt_tab.is_active() {
                    let entries = self.window_manager.alt_tab_entries();
                    if entries.is_empty() {
                        return None;
                    }
                    self.alt_tab.start(entries);
                } else {
                    self.alt_tab.step(true);
                }
                let idx = self.alt_tab.index();
                let entries = self.alt_tab.entries().to_vec();
                if let Some(wid) = entries.get(idx).copied() {
                    // Re-review finding Important-1: an explicit
                    // toplevel-focus assertion -- see `release_layer_focus`'s
                    // own doc.
                    self.release_layer_focus();
                    let previous = self.focused_id();
                    self.window_manager.focus(wid);
                    self.sync_focus_change(previous);
                }
                self.emit(Event::AltTabState(AltTabState { active: true, entries, index: idx }));
            }
            "spawn" => {
                let cmd = arg?;
                std::process::Command::new("sh").arg("-c").arg(&cmd).spawn().ok();
            }
            // Task 11 review #4: `n: u32` from an externally-parseable
            // action string (this dispatcher's own doc says it's also
            // reachable "from a D-Bus-triggered action") minus 1 is unsigned
            // subtraction -- `"workspace:0"` panicked the whole compositor
            // in debug builds (and silently wrapped to `u32::MAX` in
            // release). `checked_sub` turns that into a clean `None`
            // instead. Separately, `set_active_workspace` returns `bool`
            // ("does this workspace exist") and was being discarded, so an
            // out-of-range `"workspace:7"` against the 4-workspace default
            // used to report success (`Some(())`) having silently done
            // nothing; it's now propagated as a real failure.
            "workspace" => {
                let n: u32 = arg?.parse().ok()?;
                let idx = n.checked_sub(1)?;
                self.switch_workspace(idx)?;
            }
            "move_to_workspace" => {
                let id = self.window_manager.focused_window()?.id;
                let n: u32 = arg?.parse().ok()?;
                let idx = n.checked_sub(1)?;
                self.move_to_workspace(id, idx)?;
            }
            // NOTE (brief deviation): the sample dispatch code had a second,
            // unreachable `"snap:restore" => ...` match arm alongside this
            // one. `action.splitn(2, ':')` always assigns `base = "snap"`
            // and `arg = Some("restore")` for the action string
            // `"snap:restore"` -- matching on `base` can therefore never
            // observe `"snap:restore"` as a whole. The restore case is
            // folded into this arm's `match arg?.as_str()` instead, which is
            // the only way it's reachable.
            "snap" => {
                let id = self.window_manager.focused_window()?.id;
                match arg?.as_str() {
                    "left" => self.snap(id, SnapZone::Left)?,
                    "right" => self.snap(id, SnapZone::Right)?,
                    "up" => self.snap(id, SnapZone::Top)?,
                    "down" => self.snap(id, SnapZone::Bottom)?,
                    "restore" => self.snap_restore(id)?,
                    _ => return None,
                }
            }
            _ => return None,
        }
        self.emit_pending();
        Some(())
    }

    /// End an in-progress alt-tab session, if one is active: marks the
    /// machine inactive and emits the terminal `AltTabState { active: false,
    /// .. }` so the shell's overlay (the `contract::Event` consumer) can
    /// dismiss itself. A no-op (no event emitted) if no session is active,
    /// so callers can call this unconditionally.
    ///
    /// Task 11 review #1: nothing called `AltTabMachine::end()` anywhere in
    /// the compositor, so a session never terminated -- `is_active()` stayed
    /// `true` forever after the first `SUPER+Tab` and the shell's overlay
    /// had no way to ever be told to close. The brief/spec don't name an
    /// explicit end mechanism, so the one real WMs use was picked: alt-tab
    /// ends when the *configured `cycle:alt_tab` binding's* modifier is
    /// released -- SUPER for the default binding, but not hardcoded to it
    /// (see `watched_alt_tab_modifiers`, `alt_tab_should_end`). `SeatHandler::key`
    /// (`wlr::SeatHandler` impl below) calls this via `alt_tab_should_end`
    /// on every key event, which covers "released the modifier while still
    /// holding Tab" and "released Tab first, then the modifier" alike, and
    /// -- the fix-round correction -- recognizes the modifier's own release
    /// event by its keysym rather than relying solely on the (there, stale)
    /// modifier-held booleans; see `alt_tab_should_end`'s doc for why the
    /// booleans alone miss that event.
    pub fn end_alt_tab(&mut self) {
        if !self.alt_tab.is_active() {
            return;
        }
        let entries = self.alt_tab.entries().to_vec();
        let idx = self.alt_tab.index();
        self.alt_tab.end();
        self.emit(Event::AltTabState(AltTabState { active: false, entries, index: idx }));
        self.emit_pending();
    }

    /// The modifier flags `SeatHandler::key` should watch for alt-tab's end
    /// condition: whatever the configured `cycle:alt_tab` binding names, or
    /// `SUPER` (the default binding's own modifier) if that action has no
    /// binding at all -- which cannot happen with `default_config()` but is
    /// not assumed here, since a config can in principle drop the binding
    /// entirely while alt-tab is mid-session from before the reload.
    fn watched_alt_tab_modifiers(&self) -> input::Modifiers {
        self.config
            .keybindings
            .get("cycle:alt_tab")
            .map(|combo| input::modifiers_for_tokens(&combo.modifiers))
            .unwrap_or(input::Modifiers::SUPER)
    }

    /// Snap `id` to `zone` on the (first) output, saving its pre-snap
    /// geometry so `snap_restore` can undo it.
    /// Snap `id` to `zone` on the (first) output, saving its pre-snap
    /// geometry so `snap_restore` can undo it. A no-op returning `None` when
    /// `behavior.snap_enabled` is off (review finding I3: the flag had no
    /// consumers, so a user who disabled snapping still got snapping from
    /// both the `snap:*` actions and the drag machine).
    pub fn snap(&mut self, id: WindowId, zone: SnapZone) -> Option<()> {
        if !self.config.behavior.snap_enabled {
            return None;
        }
        let output_geo = self.usable_geo_for_pointer()?;
        let gap = self.config.appearance.snap_gap;
        let current = self.window_manager.get(id)?.geometry;
        self.snap_saved_geometry.entry(id).or_insert(current);
        self.window_manager.set_geometry(id, layout::snapped_geometry(output_geo, zone, gap))?;
        self.sync_window_to_scene(id);
        Some(())
    }

    /// Restore `id`'s geometry as it was before its most recent `snap`, if
    /// any was saved. Deliberately *not* gated on `snap_enabled`: a window
    /// snapped before the flag was turned off must still be restorable.
    pub fn snap_restore(&mut self, id: WindowId) -> Option<()> {
        if let Some(orig) = self.snap_saved_geometry.remove(&id) {
            let snapped = self.window_manager.get(id)?.geometry;
            let restored = layout::restored_geometry(orig, snapped);
            self.window_manager.set_geometry(id, restored)?;
            self.sync_window_to_scene(id);
        }
        Some(())
    }

    /// Apply a new `Config` to a *live* session: swap in the new
    /// keybindings/appearance/behavior and reconcile the workspace list,
    /// **without closing any client**. Returns the events the caller should
    /// queue (via `apply_reloaded_config`).
    ///
    /// Window rows survive the reload untouched -- their `WindowId`, focus,
    /// focus MRU, workspace assignment, geometry, client bindings, and
    /// mapped/minimized/maximized/fullscreen state all persist, as do the
    /// saved restore-geometry maps and title rasters that key off those ids.
    /// The manager is never rebuilt, so `next_id`/`seq` advance monotonically
    /// on their own and no id is ever reissued. This is the whole point of
    /// the M3 rewrite: a `SUPER+SHIFT+r` / D-Bus `ReloadConfig` no longer
    /// abandons every client to a permanently-invisible, unmodelled limbo.
    ///
    /// A reload does still cancel *transient* UI state, because it can be
    /// mid-interaction: `drag`, `resize`, `snap_preview`, and any alt-tab
    /// session are reset. Resetting an active alt-tab is a state change with
    /// no natural event, so the terminal `AltTabState{active: false, ..}` is
    /// appended to `events` (rather than via `end_alt_tab()`, which would
    /// flush on its own) so a shell overlay rendered from the last
    /// `active: true` gets its dismiss signal in order with the reload's
    /// other events.
    ///
    /// The workspace list is reconciled in place by
    /// [`WindowManager::set_workspace_names`]: names are renamed/extended/
    /// truncated, and a window on a now-removed workspace index migrates to
    /// workspace 0. One `WorkspaceList` (and one `ConfigReloaded`) is emitted.
    pub fn apply_config(&mut self, cfg: Config) -> Vec<Event> {
        let mut events: Vec<Event> = Vec::new();

        // A reload can land mid-interaction: drop any in-flight drag/resize
        // and clear the snap preview. Unlike the old behavior, this does NOT
        // close the windows those interactions referenced -- the rows stay.
        self.drag = input::DragMachine::new();
        self.resize = input::ResizeMachine::new();
        // Resetting an active alt-tab session is a state change with no
        // natural event, so emit the terminal `AltTabState{active: false}`
        // here (appended to `events` so it drains in order with the reload's
        // `WorkspaceList`/`ConfigReloaded`, instead of `end_alt_tab()`
        // jumping the queue via its own flush).
        if self.alt_tab.is_active() {
            events.push(Event::AltTabState(AltTabState {
                active: false,
                entries: self.alt_tab.entries().to_vec(),
                index: self.alt_tab.index(),
            }));
        }
        self.alt_tab = input::AltTabMachine::new();
        self.snap_preview = None;
        self.sync_snap_preview();

        // Review finding I4: revalidate keybindings against the new config.
        input::warn_about_keybindings(&cfg.keybindings);
        // Reconcile the workspace list in place -- no window row is dropped;
        // a window on a removed workspace index migrates to workspace 0.
        self.window_manager.set_workspace_names(cfg.workspace_names.clone());

        events.push(Event::WorkspaceList(self.window_manager.workspace_info()));
        events.push(Event::ConfigReloaded(cfg.appearance.clone()));
        self.config = cfg;
        events
    }

    /// Resolve `mods`+`keysym` against the configured keybindings and apply
    /// the matched action, if any.
    pub fn handle_key(&mut self, mods: input::Modifiers, keysym: u32) -> Option<()> {
        let action = input::match_action(&self.config.keybindings, mods, keysym)?;
        self.apply_action(&action)
    }

    /// Dispatch a pointer event (output logical coordinates) through the
    /// drag/snap state machine. See `PointerEvent` for the three cases.
    pub fn handle_pointer(&mut self, event: PointerEvent) -> Option<()> {
        match event {
            PointerEvent::Press { id, pointer } => self.handle_pointer_press(id, pointer),
            PointerEvent::Motion { pointer } => self.handle_pointer_motion(pointer),
            PointerEvent::Release { pointer } => self.handle_pointer_release(pointer),
        }
    }

    /// Pointer button press at `pointer` (output logical coordinates) on
    /// window `id`: focuses the window (click-to-focus), then, if the press
    /// hits the title bar's move area, begins a drag; otherwise applies
    /// whatever discrete decoration action (close/maximize/minimize) was
    /// hit.
    ///
    /// Task 11 re-review #2: this used to skip straight to
    /// `decoration_action_for`, which gates on `w.focused` -- so pressing
    /// an unfocused window's title bar or buttons did nothing at all (no
    /// focus change, no drag, no click action) since the gate always failed
    /// on the first click. Focusing first (unconditionally; `focus` on an
    /// already-focused window is a no-op) makes the very click that should
    /// raise a window also be the click that acts on it, matching ordinary
    /// click-to-focus window manager behavior.
    fn handle_pointer_press(&mut self, id: WindowId, pointer: (i32, i32)) -> Option<()> {
        let previous = self.focused_id();
        self.window_manager.focus(id)?;
        // Re-review finding Important-1: click-to-focus is an explicit
        // toplevel-focus assertion -- see `release_layer_focus`'s own doc.
        // Round-2 finding Important-2: after the `?`, not before -- an
        // early return here (an unknown `id`) must leave `layer_focus`
        // untouched, not clear it for a focus assertion that never
        // actually happened.
        self.release_layer_focus();
        // Focus changes the client's activation state, so it has to reach
        // the client too (C1) -- both ends of the transition, not just the
        // new one (New-1).
        self.sync_focus_change(previous);
        // Task 11 re-review round 3 #2: flush right after `focus()`
        // mutates, before any of the `?`-early-returns below (e.g.
        // `decoration_action_for` returning `None` for a fullscreen
        // window) can skip the trailing `emit_pending()` call at the end
        // of this function and leave the focus event sitting queued until
        // some unrelated later flush.
        self.emit_pending();
        let geo = self.window_manager.get(id)?.geometry;
        // Despite its parameter name, `decoration_action_for`/`hit_test`
        // compares against `title_bar_rect`/`button_rects`, which are
        // themselves in the window's absolute (output) geometry space (see
        // the existing `decoration_action_for_returns_close_on_button_click`
        // test, which passes `geo.x + geo.width - 5` -- an absolute
        // coordinate -- as its "local" point). So `pointer` is passed
        // through unconverted here; only the drag `grab_offset` below is a
        // genuine window-relative offset.
        let action = self.decoration_action_for(id, pointer)?;
        if action == crate::decoration::DecorationAction::Move {
            let grab_offset = (pointer.0 - geo.x, pointer.1 - geo.y);
            self.drag.begin(id, grab_offset);
        } else {
            // A button action: give it its pressed color for the span of
            // applying it. Not a model mutation -- `ssd_press` is scene-only
            // state, so this costs one extra `sync_window_to_scene` call and
            // no additional `contract::Event`.
            if let Some(idx) = crate::decoration::button_at(geo, pointer) {
                self.ssd_press = Some((id, idx));
                self.sync_window_to_scene(id);
            }
            self.apply_decoration_action(id, action);
            // The action already ran (close/maximize/minimize); nothing is
            // still "held down." Clearing before this function's own
            // `sync_window_to_scene` (below `apply_decoration_action`'s own,
            // via e.g. `toggle_maximized`) keeps a window that outlives the
            // click (maximize) from being left with a stuck pressed tint.
            self.ssd_press = None;
            self.sync_window_to_scene(id);
        }
        self.emit_pending();
        Some(())
    }

    /// Recompute which title-bar button, if any, `pointer` (output logical
    /// coordinates) sits over, and re-sync whichever window(s) that changes
    /// -- the one that lost the hover, the one that gained it, or both.
    ///
    /// Scene-only state (`ssd_hover`'s own doc): no `contract::Event` is
    /// ever queued from here, only a `sync_window_to_scene` push to the
    /// seam.
    fn update_ssd_hover(&mut self, pointer: (i32, i32)) {
        let hovered = self.window_at_point(pointer).and_then(|id| {
            let geo = self.window_manager.get(id)?.geometry;
            crate::decoration::button_at(geo, pointer).map(|idx| (id, idx))
        });
        if hovered == self.ssd_hover {
            return;
        }
        let old_id = self.ssd_hover.map(|(id, _)| id);
        let new_id = hovered.map(|(id, _)| id);
        self.ssd_hover = hovered;
        if let Some(id) = old_id {
            self.sync_window_to_scene(id);
        }
        if let Some(id) = new_id.filter(|id| Some(*id) != old_id) {
            self.sync_window_to_scene(id);
        }
    }

    /// Honors a client move request (`xdg_toplevel.move`): only when the
    /// pointer is pressed and currently over `id`'s window; begins the
    /// existing `DragMachine` grab. A no-op otherwise -- per
    /// `wlr::ToplevelHandler::request_move`'s doc, an interactive move that
    /// never starts is legal, not a protocol violation, so there is nothing
    /// to answer.
    ///
    /// Finding 6, testing: the success path (guards pass, `self.drag.begin`
    /// actually runs) is untested -- it needs a live `wlr::Runtime` to read
    /// a real `pointer_position()` from, and there is no headless-runtime
    /// way to inject one in this crate's unit tests, the same gap
    /// `arrange_layers_leaves_layer_focus_alone_and_unmap_clears_it`
    /// documents for a live keyboard device.
    fn begin_client_move(&mut self, id: WindowId) {
        if !self.pointer_pressed {
            return;
        }
        let Some(rt) = self.wayland.runtime() else { return };
        let (px, py) = rt.pointer_position();
        let pointer = (px as i32, py as i32);
        if self.window_at_point(pointer) != Some(id) {
            return;
        }
        let Some(geo) = self.window_manager.get(id).map(|w| w.geometry) else { return };
        // Baseline behavior: an interactive move raises and focuses, the
        // same as `handle_pointer_press`'s click-to-focus-then-drag path.
        let previous = self.focused_id();
        if self.window_manager.focus(id).is_none() {
            return;
        }
        self.sync_focus_change(previous);
        let grab_offset = (pointer.0 - geo.x, pointer.1 - geo.y);
        self.drag.begin(id, grab_offset);
        self.emit_pending();
    }

    /// Same guards as [`Self::begin_client_move`], for `xdg_toplevel.resize`:
    /// maps the client's `wlr::Edges` onto `input::ResizeEdges`
    /// ([`resize_edges_from_wlr`]) and begins the existing `ResizeMachine`
    /// grab.
    ///
    /// Finding 6, testing: same untested-success-path limitation as
    /// `begin_client_move` -- see its doc. `resize_edges_from_wlr` is
    /// exactly the part of this method that stays testable without a live
    /// pointer.
    fn begin_client_resize(&mut self, id: WindowId, edges: wlr::Edges) {
        if !self.pointer_pressed {
            return;
        }
        let Some(rt) = self.wayland.runtime() else { return };
        let (px, py) = rt.pointer_position();
        let pointer = (px as i32, py as i32);
        if self.window_at_point(pointer) != Some(id) {
            return;
        }
        let Some(geo) = self.window_manager.get(id).map(|w| w.geometry) else { return };
        self.resize.begin(id, resize_edges_from_wlr(edges), geo, pointer);
    }

    /// Pointer motion at `pointer` (output logical coordinates) during an
    /// in-progress drag: updates the snap-zone preview (rendering consumes
    /// `self.snap_preview`).
    fn handle_pointer_motion(&mut self, pointer: (i32, i32)) -> Option<()> {
        // Hover feedback tracks the pointer independently of drag/resize --
        // it must run even on every one of the early returns below, since
        // e.g. `self.drag.window_id()?` failing (no drag in progress, the
        // ordinary case whenever the pointer merely moves over a title bar)
        // must not skip it.
        self.update_ssd_hover(pointer);
        // An interactive resize (started by the client's `resize_request`)
        // takes precedence: it owns the pointer until the button is
        // released.
        if let Some(id) = self.resize.window_id() {
            let geometry = self.resize.geometry_for(pointer)?;
            self.window_manager.set_geometry(id, geometry)?;
            self.sync_window_to_scene(id);
            self.emit_pending();
            return Some(());
        }
        self.drag.window_id()?;
        let output_geo = self.usable_geo_for_pointer()?;
        let threshold = self.config.appearance.snap_gap.max(1) * 4;
        // Review finding I3: with snapping disabled the drag machine is
        // never told about zones at all, so no preview is staged *and*
        // `handle_pointer_release` can only ever produce a plain move.
        if !self.config.behavior.snap_enabled {
            self.snap_preview = None;
            self.sync_snap_preview();
            return Some(());
        }
        self.drag.motion(pointer, output_geo, threshold);
        self.snap_preview = self
            .drag
            .preview_zone()
            .map(|zone| layout::snapped_geometry(output_geo, zone, self.config.appearance.snap_gap));
        self.sync_snap_preview();
        Some(())
    }

    /// Pointer button release at `pointer` (output logical coordinates):
    /// ends the drag, either snapping the window to the previewed zone or
    /// moving it to `pointer - grab_offset`.
    fn handle_pointer_release(&mut self, pointer: (i32, i32)) -> Option<()> {
        if self.resize.window_id().is_some() {
            let final_geometry = self.resize.geometry_for(pointer);
            let id = self.resize.end()?;
            if let Some(geometry) = final_geometry {
                self.window_manager.set_geometry(id, geometry)?;
            }
            self.sync_window_to_scene(id);
            self.emit_pending();
            return Some(());
        }
        let id = self.drag.window_id()?;
        let grab_offset = self.drag.grab_offset();
        self.snap_preview = None;
        self.sync_snap_preview();
        match self.drag.end() {
            input::DragResult::Snapped(zone) => {
                self.snap(id, zone)?;
            }
            input::DragResult::Moved => {
                let geo = self.window_manager.get(id)?.geometry;
                let new_pos = (pointer.0 - grab_offset.0, pointer.1 - grab_offset.1);
                self.window_manager.set_geometry(
                    id,
                    Rectangle { x: new_pos.0, y: new_pos.1, width: geo.width, height: geo.height },
                )?;
                self.sync_window_to_scene(id);
            }
            input::DragResult::Restored => {}
        }
        self.emit_pending();
        Some(())
    }

    /// A client mapped a new toplevel. Creates the model window at a cascade
    /// position, binds it to `toplevel`, and reconciles focus.
    pub fn new_toplevel(&mut self, toplevel: crate::wayland::ToplevelKey, app_id: &str, title: &str, pid: u32) {
        // Default toplevel size: real geometry arrives from the client's
        // first commit, which isn't known yet; `PLACEHOLDER_SIZE` is the
        // model's placeholder until then. Position cascades off whatever is
        // already mapped so new windows don't stack exactly on top of each
        // other.
        let occupied: Vec<icedtea_contract::Rectangle> = self
            .window_manager
            .windows_in_workspace(self.window_manager.active_workspace())
            .iter()
            .map(|w| w.geometry)
            .collect();
        let output_geo = self
            .usable_geo_for_pointer()
            .unwrap_or(icedtea_contract::Rectangle { x: 0, y: 0, width: 1920, height: 1080 });
        // Review finding M7: cascade positions wrap inside the output (and
        // count only this workspace's windows) so the Nth window can't open
        // off-screen with no title bar to grab.
        let (x, y) = layout::cascade_point_in(&occupied, PLACEHOLDER_SIZE, 24, output_geo);
        let geometry =
            icedtea_contract::Rectangle { x, y, width: PLACEHOLDER_SIZE.0, height: PLACEHOLDER_SIZE.1 };
        // `add_window` autofocuses, so this is a focus change like any other
        // (New-1): capture the outgoing focus before it happens.
        let previous = self.focused_id();
        let id = self.window_manager.add_window(app_id, title, pid, geometry);
        self.wayland.bind(id, toplevel);
        // Any decoration preference this client stated before it had a model
        // row (the normal ordering -- see `pending_decorations`) lands now.
        // The client itself was already answered at `initial_commit`, with
        // this same preference and the app-id both in hand (L1), so there is
        // nothing to re-negotiate here -- only the model to bring in line
        // with what the client was told.
        let requested = self.pending_decorations.remove(&toplevel).flatten();
        if requested.is_some() {
            self.window_manager.set_client_decorations_requested(id, requested);
        }
        // Explicit sync of the new window first (re-review minor 2):
        // `sync_focus_change` also reaches it today, but only because
        // `add_window` autofocuses onto the active workspace.
        self.sync_window_to_scene(id);
        self.sync_focus_change(previous);
        self.emit_pending();
    }

    /// A client destroyed its toplevel.
    ///
    /// Named `forget_toplevel` rather than `toplevel_destroyed` (task 8):
    /// `wlr::ToplevelHandler::toplevel_destroyed` now exists as a same-named
    /// trait method on `State`, and while the two don't collide (an inherent
    /// method always wins over a trait method of the same name during
    /// resolution, so `self.toplevel_destroyed(key)` would keep working), the
    /// call site inside the trait impl would read as a same-name recursive
    /// call to a reader who can't see that resolution rule. Renaming the
    /// inherent method removes the ambiguity at the source instead of relying
    /// on it being resolved correctly.
    pub fn forget_toplevel(&mut self, toplevel: crate::wayland::ToplevelKey) {
        // Before the early return: a toplevel that dies without ever
        // mapping still had a decoration preference parked for it, and
        // nothing else would ever come to collect it.
        self.pending_decorations.remove(&toplevel);
        let Some(id) = self.wayland.window_for(toplevel) else { return };
        self.wayland.forget(id);
        self.forget_window(id);
        self.emit_pending();
    }

    /// A client changed its title.
    ///
    /// The scene sync is not optional bookkeeping: the title is *drawn* now
    /// (`sync_ssd`'s buffer node), so a retitle that only updated the model
    /// would leave the old string on screen until some unrelated geometry
    /// change happened to re-sync the window.
    pub fn toplevel_title_changed(&mut self, toplevel: crate::wayland::ToplevelKey, title: &str) {
        let Some(id) = self.wayland.window_for(toplevel) else { return };
        if self.window_manager.set_title(id, title.to_string()).is_some() {
            self.sync_window_to_scene(id);
            self.emit_pending();
        }
    }

    /// The topmost model window at `point` (frame space), or `None`.
    ///
    /// The model is the authority on this rather than the scene: the scene
    /// knows nothing about workspaces or minimization, and `visible_windows`
    /// is already MRU-ordered, which is a correct topmost-first order for
    /// click-to-focus (review finding I1).
    pub fn window_at_point(&self, point: (i32, i32)) -> Option<WindowId> {
        self.window_manager.window_at(point).map(|w| w.id)
    }

    pub fn set_background(&mut self, rect: wlr::RectId) {
        self.background = Some(rect);
    }

    pub fn background(&self) -> Option<wlr::RectId> {
        self.background
    }

    /// The scene rect node currently backing `snap_preview`, if any.
    /// Introspection for tests -- mirrors `background()`.
    pub fn snap_preview_rect(&self) -> Option<wlr::RectId> {
        self.snap_preview_rect
    }

    pub fn set_shutdown_source(&mut self, id: wlr::SourceId) {
        self.shutdown_source = Some(id);
    }

    pub fn set_command_receiver(&mut self, rx: crossbeam_channel::Receiver<crate::dbus::DbCommand>) {
        self.cmd_rx = Some(rx);
    }

    /// The `SourceId` `fd_ready` compares against to route a wake to
    /// `drain_pending_commands`. See `cmd_wake_source`'s field doc.
    pub fn set_cmd_wake_source(&mut self, id: wlr::SourceId) {
        self.cmd_wake_source = Some(id);
    }

    /// The `SourceId` `fd_ready` compares against to route a wake to
    /// `drain_config_reload`. See `config_reload_wake_source`'s field doc.
    pub fn set_config_reload_wake_source(&mut self, id: wlr::SourceId) {
        self.config_reload_wake_source = Some(id);
    }

    /// The write half `spawn_config_reload` nudges its worker threads with.
    /// See `config_reload_wake`'s field doc for why it lives on `State`
    /// rather than being handed to one worker directly.
    pub fn set_config_reload_wake(&mut self, write: std::os::unix::net::UnixStream) {
        self.config_reload_wake = Some(write);
    }

    /// Keep the receiving half of `render::spawn_wallpaper_decode`'s channel
    /// so [`Self::drain_wallpaper`] has somewhere to read from.
    pub fn set_wallpaper_receiver(&mut self, rx: crossbeam_channel::Receiver<Option<image::RgbaImage>>) {
        self.wallpaper_rx = Some(rx);
    }

    /// The `SourceId` `fd_ready` compares against to route a wake to
    /// `drain_wallpaper`. See `wallpaper_wake_source`'s field doc.
    pub fn set_wallpaper_wake_source(&mut self, id: wlr::SourceId) {
        self.wallpaper_wake_source = Some(id);
    }

    /// Keep the wake pipe's write half alive for the process's life. See
    /// `wallpaper_wake`'s field doc (review finding C1) for why this must be
    /// called with the *original*, not a clone -- callers hand
    /// `render::spawn_wallpaper_decode` a `try_clone`d copy instead.
    pub fn set_wallpaper_wake(&mut self, write: std::os::unix::net::UnixStream) {
        self.wallpaper_wake = Some(write);
    }

    /// Apply every D-Bus command that arrived since the last turn.
    ///
    /// Collected before applying, rather than iterated lazily: `handle_command`
    /// takes `&mut self`, and the receiver lives in `self`.
    fn drain_pending_commands(&mut self) {
        let Some(rx) = self.cmd_rx.as_ref() else { return };
        let pending: Vec<crate::dbus::DbCommand> = rx.try_iter().collect();
        for cmd in pending {
            self.handle_command(cmd);
        }
    }
}

// --- Compositor library handlers ---
//
// Every method below runs underneath an `extern "C"` frame: a panic escaping
// one aborts the process rather than failing anything. So there is no
// `unwrap`, no `expect`, no `assert!`, and no indexing in any of these
// bodies — a condition that cannot be handled is recorded in `State` and
// acted on once control is back on the loop.

impl wlr::OutputHandler for State {
    fn new_output(&mut self, output: &wlr::Output<'_>) {
        let Some(runtime) = self.wayland.runtime().cloned() else { return };
        // Renderer before enable: the DRM backend's enabling commit needs a
        // framebuffer, which wlroots can only allocate once the output has
        // its renderer/allocator. Nested backends tolerate either order,
        // which is how enable-first survived until the first real-DRM boot.
        if let Err(err) = runtime.init_output(output) {
            tracing::error!(?err, "could not give output a renderer");
            return;
        }
        if let Err(err) = output.enable_with_preferred_mode() {
            tracing::error!(?err, "could not enable output");
            return;
        }

        let (width, height) = output.size();
        // `next_output_index` is a monotonic counter, not `outputs.len()`
        // (review finding M2): `len()` recomputes the same index a
        // just-removed output had the moment a new one is added, colliding
        // with whatever the model (or a client-facing consumer) still
        // remembers about the old one.
        let index = self.next_output_index;
        self.next_output_index += 1;
        // The layout box is the output's real position (and, once placed,
        // its mode-derived size) in the shared multi-output coordinate
        // space; a `None` -- the output not yet in the layout, or a
        // 0x0-mode output, see `output_layout_box`'s own doc -- falls back
        // to the single-output-at-origin behavior this replaced.
        let geometry = runtime
            .output_layout_box(output.id())
            .map(|(x, y, w, h)| icedtea_contract::Rectangle { x, y, width: w, height: h })
            .unwrap_or(icedtea_contract::Rectangle { x: 0, y: 0, width, height });
        self.create_output(index, geometry);
        self.output_ids.insert(output.id(), index);
        // A new output needs its own wallpaper node (if a decode has
        // already landed) at this output's own size -- nothing else calls
        // `sync_wallpaper_nodes` when the output set grows.
        self.sync_wallpaper_nodes();

        // The background covers the whole output. Sized here rather than at
        // creation because the mode is not known until now.
        if let Some(rect) = self.background {
            runtime.set_rect_size(rect, width, height);
            runtime.set_rect_position(rect, 0, 0);
        }

        // Existing windows (there are none at boot, but a hotplugged output
        // is the same code path) need their geometry pushed at the new size.
        self.sync_scene();
        self.emit_pending();

        // Review finding M5b: a layer surface announced before any output
        // existed (parked under the `NO_OUTPUT` sentinel), or left behind
        // by an output `destroyed` with no survivor at the time, gets its
        // first (or next) real home and configure the moment any output
        // -- this one -- exists.
        self.resolve_orphaned_layers();

        // Review finding I1: nothing else owns the first frame -- it used to
        // arrive only incidentally, whenever the background rect's own
        // damage happened to trigger one. `schedule_frame` (public since
        // 0.20.1) asks wlroots to fire `OutputHandler::frame` for this
        // output on its own, so a freshly enabled output that draws nothing
        // still gets a `frame` callback and a first commit.
        output.schedule_frame();
    }

    fn frame(&mut self, output: &wlr::Output<'_>) {
        self.frames += 1;
        let Some(runtime) = self.wayland.runtime() else { return };
        // A rejected commit is routine — wlroots rejects one when nothing
        // changed — so it is logged at debug and never escalated.
        if let Err(err) = runtime.commit_output(output) {
            tracing::debug!(?err, "scene commit rejected");
        }
    }

    fn destroyed(&mut self, id: wlr::OutputId) {
        // `remove` on an unknown id, not indexing: this can name an output
        // this handler was never told about (see the library's own docs), and
        // a panic here aborts.
        if let Some(index) = self.output_ids.remove(&id) {
            if let Some(dead) = self.outputs.remove(&index) {
                // Every window whose frame center sat on the dead output
                // moves onto the survivor before anything else notices the
                // output is gone.
                self.migrate_windows_from(dead.geometry);
            }
            // The gone output's wallpaper node (if any) must go with it --
            // nothing else calls `sync_wallpaper_nodes` when the output set
            // shrinks, and a stale node would otherwise sit in the scene
            // pointing at nothing.
            self.sync_wallpaper_nodes();

            // Review finding M5a: every layer surface `entry.output ==
            // index` owned is now orphaned exactly like `dead`'s windows
            // were -- re-home them onto a survivor (or leave them parked,
            // with no survivor, for `resolve_orphaned_layers`'s own doc's
            // self-heal via the next `new_output`) so a bar on an
            // undocked external display keeps being configured instead of
            // going silent forever.
            self.resolve_orphaned_layers();
        }
    }
}

impl wlr::FdHandler for State {
    fn fd_ready(&mut self, source: wlr::SourceId, fd: std::os::fd::BorrowedFd<'_>, _readiness: wlr::Readiness) {
        // Drain whatever byte(s) woke this source before acting on it, for
        // every arm below: libwayland's loop is level-triggered, so a
        // handler that leaves data behind is called again every turn
        // forever.
        let mut buf = [0u8; 32];
        if Some(source) == self.shutdown_source {
            let _ = rustix::io::read(fd, &mut buf);
            tracing::info!("shutdown signal received");
            self.quitting = true;
            return;
        }
        if Some(source) == self.cmd_wake_source {
            let _ = rustix::io::read(fd, &mut buf);
            self.drain_pending_commands();
            return;
        }
        if Some(source) == self.config_reload_wake_source {
            let _ = rustix::io::read(fd, &mut buf);
            self.drain_config_reload();
            return;
        }
        if Some(source) == self.wallpaper_wake_source {
            let _ = rustix::io::read(fd, &mut buf);
            self.drain_wallpaper();
        }
    }
}

impl wlr::LoopHandler for State {
    fn should_stop(&mut self) -> bool {
        self.turns += 1;
        // Not called from C — this is the one handler that may panic safely —
        // but it is still not a place to. `fd_ready`'s `cmd_wake_source` and
        // `config_reload_wake_source` arms above are what actually pull the
        // loop out of a blocked `dispatch(-1)` the instant either channel
        // has something; these two calls are a backstop that runs on every
        // turn regardless of *why* it woke, so a command or reload result
        // is never left sitting past whatever else already woke the loop
        // for an unrelated reason (a frame, input, the shutdown source).
        self.drain_pending_commands();
        self.drain_config_reload();
        // Review finding I3: the wallpaper decode channel gets the same
        // per-turn backstop as the other two, for the same reason -- a
        // result that arrived while something unrelated woke the loop must
        // not sit past this turn just because it wasn't *this* wake source.
        self.drain_wallpaper();
        self.quitting
    }
}

impl wlr::ToplevelHandler for State {
    fn new_toplevel(&mut self, toplevel: &wlr::Toplevel<'_>) {
        // Nothing is created in the model yet: at this point the client has
        // sent no buffer and no size, and it may never map at all. The model
        // row is created on `mapped`, which is the first moment a window
        // genuinely exists on screen — and the moment the smithay
        // implementation's `new_toplevel` was standing in for.
        let _ = toplevel;
    }

    fn initial_commit(&mut self, toplevel: &wlr::Toplevel<'_>) {
        // xdg-shell requires a configure here. Staging the model's
        // placeholder size means the client's very first buffer is already
        // the right size, rather than being resized one frame later --
        // except the size the client must be told is the *content* size,
        // not the model's frame size: `mapped`/`new_toplevel` hasn't run
        // yet, so there is no model window and no `client_decorations_requested`
        // to read, but a window's default SSD-or-not answer only depends on
        // `app_id` (`decoration::has_ssd`'s `requested: None` case), which
        // `Toplevel::app_id` already has at this point. Skipping
        // `content_rect` here would configure a default (SSD) window's
        // client one `TITLE_BAR_HEIGHT` too tall -- the exact one-frame-late
        // resize this comment already claims not to have.
        let key = crate::wayland::ToplevelKey::new(toplevel.id());
        let app_id = toplevel.app_id().unwrap_or_default();
        // Review finding L1: this is also the first -- and only -- moment
        // before the client's first frame at which both halves of the
        // decoration answer are known: the app-id (from `toplevel`) and any
        // preference the client stated on its decoration object (parked in
        // `pending_decorations`, since `set_mode` precedes this commit).
        // Answering here rather than from `request_decoration_mode` is what
        // makes the client's *first* decoration configure the correct one:
        // an earlier `set_decoration_mode` is staged rather than sent, and
        // staging is last-write-wins, so this overwrites the provisional
        // answer instead of adding a second configure after it.
        let requested = self.pending_decorations.get(&key).copied().flatten();
        let ssd = crate::decoration::has_ssd(&app_id, requested, false);
        self.wayland.set_decoration_mode(
            key,
            if ssd { wlr::DecorationMode::ServerSide } else { wlr::DecorationMode::ClientSide },
        );
        let Some(runtime) = self.wayland.runtime() else { return };
        let placeholder = icedtea_contract::Rectangle {
            x: 0,
            y: 0,
            width: PLACEHOLDER_SIZE.0,
            height: PLACEHOLDER_SIZE.1,
        };
        let content = crate::decoration::content_rect(placeholder, ssd);
        runtime.set_toplevel_size(toplevel.id(), content.width, content.height);
    }

    fn mapped(&mut self, toplevel: &wlr::Toplevel<'_>) {
        let id = toplevel.id();
        let key = crate::wayland::ToplevelKey::new(id);
        if let Some(window_id) = self.wayland.window_for(key) {
            // Remapped after an unmap: the model row survived, so this is a
            // visibility change rather than a new window. Task 14: the
            // model itself now tracks that state, so flip it back before
            // syncing -- otherwise `is_visible`/`alt_tab_entries` would keep
            // treating a window the client just remapped as still absent.
            tracing::info!(?id, ?window_id, "toplevel remapped");
            self.window_manager.set_mapped(window_id, true);
            self.sync_window_to_scene(window_id);
            self.emit_pending();
            return;
        }
        let app_id = toplevel.app_id().unwrap_or_default();
        let title = toplevel.title().unwrap_or_default();
        let pid = toplevel.pid().unwrap_or(0);
        tracing::info!(?id, %app_id, %title, pid, "toplevel mapped");
        self.new_toplevel(key, &app_id, &title, pid);
    }

    fn unmapped(&mut self, id: wlr::ToplevelId) {
        // An unmap is not a destroy: the client may map again with the same
        // id. Hide it, and let the model keep the row.
        //
        // Task 14: the model now has its own "unmapped" concept
        // (`Window::mapped`, `WindowManager::set_mapped`) instead of the
        // ledgered gap task 11 documented here -- `is_visible` and
        // `alt_tab_entries` both gate on it, and `focus`/
        // `focus_mru_in_workspace` refuse an unmapped window as a candidate.
        // So an unmap that hits the focused window now really does move
        // focus to the next mapped candidate on the same workspace, the
        // same way `set_minimized_and_reconcile` already does for
        // minimizing the focused window; `window_manager.get(window)` still
        // returns the row (unchanged geometry/title/etc, per `set_mapped`'s
        // own doc), and `is_backed` stays `true` -- only mapped-ness and
        // its consequences on visibility/focus change.
        //
        // The seat trap task 11 closed still matters here too:
        // `sync_focus_change` re-derives keyboard focus from the model
        // (`sync_seat_focus`, at the tail of `sync_window_to_scene` or
        // directly when nothing is focused), so a focused window unmapping
        // with nothing to hand focus to still clears the seat rather than
        // leaving it pointed at a hidden surface.
        tracing::info!(?id, "toplevel unmapped");
        let key = crate::wayland::ToplevelKey::new(id);
        let Some(window) = self.wayland.window_for(key) else { return };
        self.wayland.set_visible(window, false);
        let previous = self.focused_id();
        self.window_manager.set_mapped(window, false);
        // Review finding I2: this used to re-pick only when the unmapping
        // window was the *active* workspace's focus, so an unmap on an
        // inactive workspace left that workspace's `focused_window` pointing
        // at the now-unmapped row -- dead to the seat, but still live to
        // `apply_action("close"/"maximize"/"fullscreen"/"snap")` and the
        // decoration actions once the user switched back.
        // `release_focus` resolves the window's *own* workspace, so the
        // active case behaves exactly as before and the inactive case is
        // covered too; when no successor exists it clears the pointer,
        // matching `remove_window`.
        self.window_manager.release_focus(window);
        self.sync_focus_change(previous);
        self.emit_pending();
    }

    fn title_changed(&mut self, toplevel: &wlr::Toplevel<'_>) {
        let id = toplevel.id();
        let key = crate::wayland::ToplevelKey::new(id);
        let title = toplevel.title().unwrap_or_default();
        tracing::info!(?id, %title, "toplevel title changed");
        self.toplevel_title_changed(key, &title);
    }

    fn toplevel_destroyed(&mut self, id: wlr::ToplevelId) {
        // Safe against an id we were never told about (the library documents
        // that this can happen): `forget_toplevel` resolves through the map
        // and returns early on a miss, and never indexes.
        tracing::info!(?id, "toplevel destroyed");
        self.forget_toplevel(crate::wayland::ToplevelKey::new(id));
    }

    fn request_maximize(&mut self, toplevel: &wlr::Toplevel<'_>, maximize: bool) {
        let key = crate::wayland::ToplevelKey::new(toplevel.id());
        if !self.reconcile_maximized(key, maximize) {
            // The model didn't change (unknown toplevel, no output yet, or
            // already at the requested state) -- the dispatch layer answers
            // with a bare configure regardless, so xdg-shell's "every
            // request gets a configure" contract is honored either way.
            // Nothing to do here.
        }
    }

    fn request_fullscreen(&mut self, toplevel: &wlr::Toplevel<'_>, fullscreen: bool) {
        let key = crate::wayland::ToplevelKey::new(toplevel.id());
        if !self.reconcile_fullscreen(key, fullscreen) {
            // Same contract as `request_maximize` above: dispatch already
            // answers with a bare configure when the model didn't change.
        }
    }

    fn request_move(&mut self, id: wlr::ToplevelId) {
        let key = crate::wayland::ToplevelKey::new(id);
        let Some(window_id) = self.wayland.window_for(key) else { return };
        self.begin_client_move(window_id);
    }

    fn request_resize(&mut self, id: wlr::ToplevelId, edges: wlr::Edges) {
        let key = crate::wayland::ToplevelKey::new(id);
        let Some(window_id) = self.wayland.window_for(key) else { return };
        self.begin_client_resize(window_id, edges);
    }

    /// xdg-decoration negotiation, both halves in one place: record what the
    /// client asked for in the model, then answer with what this compositor
    /// is actually going to do.
    ///
    /// The answer is `decoration::has_ssd`, not an echo of the preference:
    /// that predicate is already the single definition of "do we draw a
    /// title bar for this window" (`sync_window_to_scene`, `content_rect`,
    /// the hit-test), so routing the reply through it is what keeps the
    /// client's belief and the compositor's own drawing from ever
    /// disagreeing. A client asking for client-side decorations gets them
    /// (`is_csd` honors an explicit request); a client asking for
    /// server-side, or stating no preference at all, gets our band.
    ///
    /// A toplevel with no model window yet -- the normal case, since a
    /// decoration is created before the initial commit and `mapped` has not
    /// run -- has its preference parked in `pending_decorations` and is
    /// still answered here, because a request must never be left unanswered
    /// (the dispatch layer would otherwise impose its own blanket
    /// server-side default). That answer is made from the preference alone:
    /// there is no app-id to read from a bare `ToplevelId`. It is only
    /// provisional, and it is *staged* rather than sent while the surface is
    /// uninitialized, so `initial_commit` -- which has both the app-id and
    /// the parked preference -- overwrites it before anything reaches the
    /// client (review finding L1).
    fn request_decoration_mode(
        &mut self,
        id: wlr::ToplevelId,
        preference: Option<wlr::DecorationMode>,
    ) {
        // `client_decorations_requested` is the model's own spelling of the
        // same three-valued answer: "the client wants to draw them itself",
        // "the client wants the server to", "the client did not say".
        let requested = match preference {
            Some(wlr::DecorationMode::ClientSide) => Some(true),
            Some(wlr::DecorationMode::ServerSide) => Some(false),
            None => None,
        };
        let key = crate::wayland::ToplevelKey::new(id);
        let window = self.wayland.window_for(key);
        let (app_id, fullscreen) = match window.and_then(|w| self.window_manager.get(w)) {
            Some(w) => (w.app_id.clone(), w.fullscreen),
            None => (String::new(), false),
        };
        match window {
            Some(window) => {
                self.window_manager
                    .set_client_decorations_requested(window, requested);
            }
            None => {
                self.pending_decorations.insert(key, requested);
            }
        }

        let ssd = crate::decoration::has_ssd(&app_id, requested, fullscreen);
        let mode = if ssd {
            wlr::DecorationMode::ServerSide
        } else {
            wlr::DecorationMode::ClientSide
        };
        self.wayland.set_decoration_mode(key, mode);

        // The band may have to appear or vanish, and the client's content
        // rect changes with it. A window that isn't in the model yet has
        // nothing to sync -- its first sync comes with `mapped`.
        if let Some(window) = window {
            self.sync_window_to_scene(window);
            self.emit_pending();
        }
    }

    /// A client created a wlr-layer-shell surface. Resolve its output --
    /// what it asked for (`output_id` via `output_ids`), or the output
    /// under the pointer (which itself falls back to the lowest index) --
    /// and park it under the [`NO_OUTPUT`] sentinel if neither resolves,
    /// i.e. no output exists yet at all (review finding M5): the surface
    /// is *not* dropped, because `State::resolve_orphaned_layers` re-homes
    /// every `NO_OUTPUT` entry (and configures it) the moment `new_output`
    /// next fires -- dropping it here left a bar launched before the first
    /// output settled waiting forever for a configure that would never
    /// come, even after one did arrive. Always record it and always answer
    /// -- `configure_layer` is a no-op, correctly, on the sentinel, and is
    /// otherwise safe even before this surface's first commit
    /// (`Runtime::configure_layer_surface` stages pre-initial-commit
    /// answers rather than sending them, see its own doc) -- it is
    /// mandatory: nothing else in this crate's dispatch layer answers a
    /// layer surface that no handler ever does.
    fn new_layer_surface(&mut self, surface: &wlr::LayerSurface<'_>) {
        let id = surface.id();
        let client_output = surface.output_id().and_then(|oid| self.output_ids.get(&oid).copied());
        let output = client_output.or_else(|| self.output_for_pointer());
        let sequence = self.next_layer_sequence;
        self.next_layer_sequence += 1;
        // H2: bound the client-controlled exclusive zone at capture time --
        // see `clamp_exclusive_zone`'s own doc.
        let exclusive = clamp_exclusive_zone(surface.exclusive_zone(), output.and_then(|idx| self.outputs.get(&idx)));
        self.layers.insert(
            id,
            LayerEntry {
                output: output.unwrap_or(NO_OUTPUT),
                sequence,
                layer: surface.layer(),
                anchor: surface.anchor(),
                exclusive,
                size: surface.desired_size(),
                // Always `false` here regardless of what the client asked
                // for -- `keyboard_interactive` reads `current`, which is
                // entirely zeroed until this surface's first commit (see
                // that accessor's own doc). `layer_surface_commit` is
                // where the real value lands.
                interactive: false,
                // False until `layer_surface_mapped` (review finding J2):
                // a surface with no buffer yet must not reserve space.
                mapped: false,
                last_configured: None,
                // N8: no accessor to capture this from -- see
                // `LayerEntry::margin`'s own doc.
                margin: (0, 0, 0, 0),
            },
        );
        // The client left the output unset: this crate chose one on its
        // behalf (`client_output.is_none()`, above), so the raw layer
        // surface's own `output` field needs to agree with the model --
        // `set_layer_surface_output` is the 0.20.12 API for that
        // assignment. A no-op, correctly, with no runtime attached (every
        // unit test in this file) or no live output to name yet (`output`
        // is `None`, parked under `NO_OUTPUT`; `resolve_orphaned_layers`
        // picks this back up the moment one exists).
        if client_output.is_none()
            && let (Some(idx), Some(runtime)) = (output, self.wayland.runtime())
            && let Some(output_id) = self.wlr_output_id_for(idx)
        {
            runtime.set_layer_surface_output(id, output_id);
        }
        self.configure_layer(id);
    }

    /// Every commit of an already-announced layer surface: refresh the
    /// entry from the surface's current request (anchors, exclusive zone
    /// and size all commonly change after mapping) and answer again --
    /// `configure_layer` reads the entry it just updated, so a client that
    /// re-anchors gets reconfigured for its new placement, not its old
    /// one. Then re-derive every output's usable area, since this
    /// surface's exclusive zone may have just changed.
    ///
    /// A miss on `self.layers` cannot happen for any id this crate ever
    /// hands a handler -- `new_layer_surface` always inserts an entry now
    /// (see its own doc on the `NO_OUTPUT` sentinel) -- but the `let …
    /// else` stays as the defensive, panic-free answer for an id this
    /// handler was never told about, the same posture every other handler
    /// in this file takes.
    fn layer_surface_commit(&mut self, surface: &wlr::LayerSurface<'_>) {
        let id = surface.id();
        let Some(output_idx) = self.layers.get(&id).map(|e| e.output) else { return };
        // H2: bound the client-controlled exclusive zone at capture time --
        // see `clamp_exclusive_zone`'s own doc. Looked up before the
        // `entry` borrow below, since both read `self.outputs`.
        let exclusive = clamp_exclusive_zone(surface.exclusive_zone(), self.outputs.get(&output_idx));
        let Some(entry) = self.layers.get_mut(&id) else { return };
        entry.layer = surface.layer();
        entry.anchor = surface.anchor();
        entry.exclusive = exclusive;
        entry.size = surface.desired_size();
        let was_interactive = entry.interactive;
        entry.interactive = surface.keyboard_interactive();
        let now_interactive = entry.interactive;
        self.configure_layer(id);
        self.arrange_layers();
        // N9: a surface that only becomes keyboard-interactive *after* it
        // mapped (a menu that opens with no interactivity, then flips it
        // on once the user drives it) never passes through
        // `layer_surface_mapped`'s own take-focus branch, since that only
        // ever runs once, at map time. See `sync_layer_interactive_focus`'s
        // own doc for the take/release rule this drives.
        self.sync_layer_interactive_focus(id, was_interactive, now_interactive);
    }

    /// The layer surface now has a buffer and is on screen: it starts
    /// reserving its exclusive zone (review finding J2 -- `mapped = true`
    /// before `arrange_layers`, which is what makes this a real fold
    /// rather than the guaranteed no-op it used to be). A
    /// keyboard-interactive surface then takes seat keyboard focus --
    /// `focus_layer_keyboard` refuses an unmapped surface (`None` while
    /// unmapped, per its own doc), which is exactly why this waits for
    /// `mapped` rather than acting from `layer_surface_commit`, where
    /// `interactive` first becomes known but the surface may still be
    /// unmapped.
    fn layer_surface_mapped(&mut self, id: wlr::LayerSurfaceId) {
        if let Some(entry) = self.layers.get_mut(&id) {
            entry.mapped = true;
        }
        self.arrange_layers();
        let Some(entry) = self.layers.get(&id) else { return };
        if !entry.interactive {
            return;
        }
        let Some(runtime) = self.wayland.runtime() else { return };
        if runtime.focus_layer_keyboard(id).is_some() {
            self.layer_focus = Some(id);
        } else {
            // Finding 8, errors: an interactive panel that just mapped and
            // failed to take keyboard focus used to fail silently, leaving
            // no trace of why an auto-hide launcher/locker never got
            // keyboard input.
            tracing::debug!(?id, "interactive layer surface mapped but did not take keyboard focus");
        }
    }

    /// The layer surface should no longer be displayed (not a destroy --
    /// see this method's own trait doc; the entry survives so a remap
    /// finds it again). Stops reserving its exclusive zone (review finding
    /// J2 -- `mapped = false` before `arrange_layers`, which is what makes
    /// this call a real fold instead of a no-op: an unmapped panel used to
    /// leave a permanent hole in the workspace until it was destroyed
    /// outright). If it held keyboard focus, hand focus back to whatever
    /// the model says is focused: `sync_seat_focus` re-derives the seat's
    /// keyboard target from `window_manager` rather than leaving it
    /// pointed at a surface that just stopped being shown.
    fn layer_surface_unmapped(&mut self, id: wlr::LayerSurfaceId) {
        if let Some(entry) = self.layers.get_mut(&id) {
            entry.mapped = false;
            // Review finding I1: wlroots resets the surface's `initialized`
            // flag on every unmap (documented in wlr 0.20.11's `layer.rs`;
            // the crate deliberately refuses to synthesize a fallback
            // configure), so a remap needs a *fresh* mandatory configure --
            // but the remap commit recomputes the identical placement, and
            // `configure_layer`'s storm guard would suppress the send
            // against a surviving `last_configured`. The surface would then
            // never become `mapped`, which is the only thing
            // `arrange_layers`' sweep reconfigures, and the client hangs
            // forever: the auto-hide-panel / toggle-launcher sequence.
            // Forgetting the placement here is what makes the next
            // `configure_layer` unconditionally send.
            entry.last_configured = None;
        }
        if self.layer_focus == Some(id) {
            self.layer_focus = None;
            self.sync_seat_focus();
        }
        self.arrange_layers();
    }

    /// The layer surface is gone for good. Same focus hand-back as
    /// `layer_surface_unmapped` (a destroy while mapped and focused is
    /// legal -- a client can drop its surface without ever unmapping it
    /// first), plus removing the entry, which `layer_surface_unmapped`
    /// deliberately does not. Panic-free on an id this handler was never
    /// told about (the trait's own doc: this can happen) -- both the
    /// focus check and `HashMap::remove` are no-ops on a miss.
    fn layer_surface_destroyed(&mut self, id: wlr::LayerSurfaceId) {
        if self.layer_focus == Some(id) {
            self.layer_focus = None;
            self.sync_seat_focus();
        }
        self.layers.remove(&id);
        self.arrange_layers();
    }
}
impl wlr::SeatHandler for State {
    fn key(&mut self, event: &wlr::KeyEvent<'_>) -> bool {
        let m = event.modifiers();
        let mods = to_model_modifiers(m.logo(), m.ctrl(), m.alt(), m.shift());

        // Alt-tab's chosen end condition: the session ends when the
        // configured `cycle:alt_tab` binding's modifier is no longer down.
        // See `alt_tab_should_end`'s doc for why this event's own `keysym`,
        // not just its (stale, on a modifier's own release) `mods`, has to
        // be consulted.
        if self.alt_tab.is_active() {
            let watched = self.watched_alt_tab_modifiers();
            if alt_tab_should_end(watched, mods, event.pressed(), event.keysym()) {
                // Emits its own event internally; nothing to flush here.
                self.end_alt_tab();
            }
        }

        // Releases are never consumed: a client that is sent a press but not
        // its release believes the key is still held forever.
        if !event.pressed() {
            return false;
        }

        let consumed = self.handle_key(mods, event.keysym()).is_some();
        // `handle_key` -> `apply_action` mutates the model, and every
        // mutation owes exactly one event; flush here rather than at the
        // next unrelated sync.
        self.emit_pending();
        consumed
    }

    fn pointer_motion(&mut self, x: f64, y: f64, _time_msec: u32) {
        // The model works in integer output-logical coordinates; the library
        // reports scene coordinates, which for the slice's single output at
        // the layout origin are the same space. `as i32` truncates toward
        // zero, which is what a pixel index wants.
        let pointer = (x as i32, y as i32);
        self.pointer_location = pointer;
        self.handle_pointer(PointerEvent::Motion { pointer });
        self.emit_pending();
    }

    fn pointer_button(&mut self, x: f64, y: f64, button: u32, pressed: bool, _time_msec: u32) {
        // BTN_LEFT only: every decoration interaction this compositor has is
        // a left-click one, and forwarding the rest to the client unchanged
        // is the correct behaviour for them. Hoisted above `pointer_pressed`
        // below (review: state.rs:2050-2056) -- both need it before the
        // early return this gate gives everything past it.
        const BTN_LEFT: u32 = 0x110;

        // Set before any routing (Deviation 8): `begin_client_move`/
        // `begin_client_resize` gate an interactive move/resize on this, and
        // it must already reflect *this* event by the time anything below
        // reads it -- including a request that arrives interleaved with the
        // button event itself. Gated on `BTN_LEFT` -- the same button the
        // grab/release path below acts on exclusively -- because this is a
        // single scalar, not a per-button set: an ungated assignment means a
        // right/middle release while the left button is still held wrongly
        // clears it (spuriously rejecting a legit move grab), and a lone
        // right-click press wrongly sets it. "Left button held" is the only
        // reading that matches the grab semantics throughout this file.
        if button == BTN_LEFT {
            self.pointer_pressed = pressed;
        }

        // Recorded before the `BTN_LEFT` gate below, not after: this is the
        // same `pointer_location` `pointer_motion` updates, and a
        // button-only event (which carries no position of its own once
        // `handle_pointer` reads it back) must not skip the update just
        // because the button that arrived happens to not be the left one --
        // a right-click at a new position must still leave
        // `pointer_location` correct for whatever left-click/drag comes
        // next.
        let pointer = (x as i32, y as i32);
        self.pointer_location = pointer;

        if button != BTN_LEFT {
            return;
        }

        if pressed {
            // The model answers "what is under the pointer", not the scene:
            // the scene knows nothing about workspaces or minimization, and
            // `window_at_point` is already MRU-ordered, which is a correct
            // topmost-first order (review finding I1).
            let Some(id) = self.window_at_point(pointer) else { return };
            self.handle_pointer(PointerEvent::Press { id, pointer });
        } else {
            self.handle_pointer(PointerEvent::Release { pointer });
        }
        self.emit_pending();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use icedtea_config::default_config;
    use icedtea_contract::Rectangle;

    const DEFAULT_GEO: Rectangle = Rectangle { x: 0, y: 0, width: 640, height: 400 };

    #[test]
    fn state_emits_pending_events_on_channel() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        state.window_manager.add_window("app", "t", 1, DEFAULT_GEO);
        state.emit_pending();
        assert!(matches!(rx.try_recv().map(|e| e.event), Ok(Event::WindowOpened(_))));
    }

    #[test]
    fn decoration_action_for_returns_close_on_button_click() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        let id = state.window_manager.add_window("app", "t", 1, DEFAULT_GEO);
        state.window_manager.set_geometry(id, Rectangle { x: 100, y: 100, width: 600, height: 400 }).unwrap();
        state.window_manager.focus(id).unwrap();
        let geo = Rectangle { x: 100, y: 100, width: 600, height: 400 };
        // Click on rightmost button (close button)
        let close_pt = (geo.x + geo.width - 5, geo.y + 5);
        assert_eq!(state.decoration_action_for(id, close_pt), Some(crate::decoration::DecorationAction::Close));
    }

    #[test]
    fn decoration_action_for_returns_none_for_unfocused_window() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        let id1 = state.window_manager.add_window("app1", "t1", 1, DEFAULT_GEO);
        let _id2 = state.window_manager.add_window("app2", "t2", 2, DEFAULT_GEO);
        // _id2 is now focused; id1 is unfocused
        state.window_manager.set_geometry(id1, Rectangle { x: 100, y: 100, width: 600, height: 400 }).unwrap();
        let geo = Rectangle { x: 100, y: 100, width: 600, height: 400 };
        let close_pt = (geo.x + geo.width - 5, geo.y + 5);
        // Should return None because id1 is not focused
        assert_eq!(state.decoration_action_for(id1, close_pt), None);
    }

    #[test]
    fn decoration_action_for_returns_none_for_fullscreen_window() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        state.outputs.insert(0, OutputSurface::new(Rectangle { x: 0, y: 0, width: 1920, height: 1080 }));
        let id = state.window_manager.add_window("app", "t", 1, DEFAULT_GEO);
        state.window_manager.focus(id).unwrap();
        state.toggle_fullscreen(id).unwrap();
        let geo = Rectangle { x: 0, y: 0, width: 1920, height: 1080 };
        let close_pt = (geo.x + 5, geo.y + 5);
        // Should return None because window is fullscreen
        assert_eq!(state.decoration_action_for(id, close_pt), None);
    }

    #[test]
    fn decoration_action_for_ignores_csd_and_still_returns_close() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        let id = state.window_manager.add_window("org.gtk.App", "t", 1, DEFAULT_GEO);
        state.window_manager.set_geometry(id, Rectangle { x: 100, y: 100, width: 600, height: 400 }).unwrap();
        state.window_manager.focus(id).unwrap();
        state.window_manager.set_client_decorations_requested(id, Some(true)).unwrap();
        let geo = Rectangle { x: 100, y: 100, width: 600, height: 400 };
        let close_pt = (geo.x + geo.width - 5, geo.y + 5);
        // Should work even though client requested decorations (decoration_action_for doesn't filter by CSD)
        // CSD filtering happens at rendering time in draw_frame
        assert_eq!(state.decoration_action_for(id, close_pt), Some(crate::decoration::DecorationAction::Close));
    }

    #[test]
    fn toggle_fullscreen_flips_state_and_geometry() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        state.outputs.insert(0, OutputSurface::new(Rectangle { x: 0, y: 0, width: 1920, height: 1080 }));
        let id = state.window_manager.add_window("app", "t", 1, DEFAULT_GEO);
        state.toggle_fullscreen(id).unwrap();
        let w = state.window_manager.get(id).unwrap();
        assert!(w.fullscreen);
        assert_eq!(w.geometry, Rectangle { x: 0, y: 0, width: 1920, height: 1080 });
    }

    // NOTE (brief deviation): the brief's Step-1 sample for
    // `apply_action_switches_workspaces_and_snaps` calls
    // `state.window_manager.add_window("app", "t", 1)` (3 args), never
    // registers an output, and adds the window *before* switching
    // workspaces. Three real bugs, not just transcription noise:
    //   1. 3-arg `add_window` no longer compiles against the new 5-arg
    //      signature (this task's own change).
    //   2. `snap()` reads `self.outputs` (empty by default in `State::new`)
    //      to find output geometry, despite the brief's own prose saying
    //      "the tests assume ... a 1000x800 output" -- nothing in the
    //      sample ever inserts one.
    //   3. `WindowManager::focused_window()` is scoped to the *active*
    //      workspace (established well before this task -- see
    //      `window.rs`'s `move_to_workspace_keeps_focus_valid` test, which
    //      asserts exactly this). Adding the window on workspace 0 and then
    //      switching to workspace 2 (index 1) leaves workspace 1 with no
    //      focused window at all, so `apply_action("snap:left")` -- which
    //      dispatches through `focused_window()` -- would return `None`
    //      and the `.unwrap()` would panic. Switching workspace *first*,
    //      then adding the window (which auto-focuses on whatever is
    //      currently active), keeps the window's workspace and the active
    //      workspace in agreement, matching how a real "switch to an empty
    //      workspace, open something there, then snap it" sequence would
    //      actually behave.
    // All three are fixed below; the assertions are unchanged from the
    // brief.
    #[test]
    fn apply_action_switches_workspaces_and_snaps() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        state.outputs.insert(0, OutputSurface::new(Rectangle { x: 0, y: 0, width: 1000, height: 800 }));
        state.apply_action("workspace:2").unwrap();
        assert_eq!(state.window_manager.active_workspace(), 1);
        let id = state.window_manager.add_window("app", "t", 1, DEFAULT_GEO);
        state.apply_action("snap:left").unwrap();
        assert_eq!(state.window_manager.get(id).unwrap().geometry.width, 1000 / 2 - 16);
        state.apply_action("snap:restore").unwrap();
        assert_eq!(state.window_manager.get(id).unwrap().geometry.width, 640);
    }

    #[test]
    fn apply_config_reconciles_workspace_names_without_dropping_windows() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        // default_config has 4 workspaces; the window stays on workspace 0.
        let a = state.window_manager.add_window("a", "a", 1, DEFAULT_GEO);
        let mut cfg = icedtea_config::default_config();
        cfg.workspace_names = vec!["A".into(), "B".into()];
        let events = state.apply_config(cfg);

        // Workspace list reconciled in place to the new names/count.
        let info = state.window_manager.workspace_info();
        assert_eq!(info.len(), 2);
        assert_eq!(info[0].name, "A");
        assert_eq!(info[1].name, "B");
        assert!(events.iter().any(|e| matches!(e, Event::WorkspaceList(_))));

        // The window on a still-existing workspace index survives with its
        // assignment intact -- and no WindowClosed was emitted for it.
        assert!(state.window_manager.get(a).is_some(), "window survives a reload");
        assert_eq!(state.window_manager.get(a).unwrap().workspace, 0);
        assert!(!events.iter().any(|e| matches!(e, Event::WindowClosed(_))));
    }

    /// A reload that removes the workspace a window sits on migrates that
    /// window to workspace 0 rather than dropping it (the decided semantics
    /// of `WindowManager::set_workspace_names`).
    #[test]
    fn apply_config_migrates_windows_off_removed_workspaces_to_zero() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        // default_config has 4 workspaces; park the window on workspace 2.
        let a = state.window_manager.add_window("a", "a", 1, DEFAULT_GEO);
        state.window_manager.set_workspace(a, 2).unwrap();
        assert_eq!(state.window_manager.get(a).unwrap().workspace, 2);

        let mut cfg = icedtea_config::default_config();
        cfg.workspace_names = vec!["only".into()]; // removes workspaces 1..4
        let events = state.apply_config(cfg);

        let w = state.window_manager.get(a).expect("window survives, is not closed");
        assert_eq!(w.workspace, 0, "a window on a removed workspace migrates to 0");
        assert!(!w.focused);
        assert!(!events.iter().any(|e| matches!(e, Event::WindowClosed(_))));
    }

    /// M1 (review): truncating the workspace list below the active index must
    /// clamp `active_workspace` back to 0 AND emit exactly one
    /// `WorkspaceSet{active: true}` -- `WorkspaceList` does not carry the
    /// active index, so subscribers would otherwise keep showing the vanished
    /// workspace until a manual switch.
    #[test]
    fn apply_config_emits_workspace_set_when_active_workspace_is_truncated() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        // default_config has 4 workspaces; make workspace 3 active.
        assert!(state.window_manager.set_active_workspace(3));
        assert_eq!(state.window_manager.active_workspace(), 3);
        // Drain setup events so we only observe the reload's.
        state.window_manager.pending_events.clear();

        let mut cfg = icedtea_config::default_config();
        cfg.workspace_names = vec!["only".into()]; // removes workspaces 1..4
        let _ = state.apply_config(cfg);

        assert_eq!(state.window_manager.active_workspace(), 0, "active clamps into range");
        assert!(
            state
                .window_manager
                .pending_events
                .iter()
                .any(|se| matches!(se.event, Event::WorkspaceSet { id: 0, active: true })),
            "clamping the active workspace must emit a WorkspaceSet"
        );
    }

    /// Review finding: the brief's own sample test for this name is vacuous
    /// in a plain unit-test harness -- `sync_window_to_scene` early-returns
    /// on `!is_backed(id)`, and nothing in the sample binds the window to a
    /// toplevel, so `apply_reloaded_config`'s recolor/resync block is a
    /// guaranteed no-op regardless of whether it exists (confirmed by
    /// deleting that block and rerunning: the sample still passed).
    ///
    /// This version closes that gap the way the harness actually allows:
    /// `wayland::ToplevelKey::for_test` + `Wayland::bind` make the window
    /// `is_backed` without needing a real `wlr::Runtime` (only the
    /// scene-graph painting inside `sync_ssd` needs one -- see its own
    /// `let Some(runtime) = ... else { return }` guard -- but
    /// `ensure_title_raster` runs *before* that guard, so the raster cache,
    /// this crate's palette-keyed memo, still gets driven for a backed
    /// window even in a runtime-less unit test). Seeding a cached raster
    /// under the *old* palette and asserting its pixels differ after a
    /// reload with a *new* palette is a genuine regression guard: deleting
    /// `apply_reloaded_config`'s `self.sync_scene()` call (which is what
    /// reaches `sync_window_to_scene` -> `ensure_title_raster` for every
    /// surviving window) leaves the pre-reload pixels in place and fails
    /// this test.
    #[test]
    fn reload_rethemes_surviving_windows_and_recolors_background() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(icedtea_config::default_config(), tx);
        // "app" is not GTK-style, so `decoration::has_ssd` gives it a
        // server-side title bar -- `sync_window_to_scene` only rasterizes a
        // title for a decorated, visible window.
        let id = state.window_manager.add_window("app", "t", 1, Rectangle { x: 0, y: 0, width: 300, height: 200 });
        state.wayland.bind(id, crate::wayland::ToplevelKey::for_test(1));

        // Drive one sync under the *old* palette to seed the cache, then
        // capture its pixels.
        state.sync_scene();
        let before = state
            .title_rasters
            .get(&id)
            .expect("a decorated, backed window must have a cached title raster")
            .pixels
            .clone();

        let mut cfg = icedtea_config::default_config();
        cfg.appearance.palette.background = "#abcdef".into();
        cfg.appearance.palette.foreground = "#123456".into();
        state.apply_reloaded_config(cfg);

        assert!(state.window_manager.get(id).is_some(), "window survived");
        assert_eq!(state.config.appearance.palette.background, "#abcdef");
        let after = state
            .title_rasters
            .get(&id)
            .expect("the raster survives the reload (Task 4 preserve contract)")
            .pixels
            .clone();
        assert_ne!(
            before, after,
            "a reload with a new foreground must have re-driven sync_window_to_scene, \
             re-rasterizing the title against the new palette-resolved color"
        );
    }

    #[test]
    fn reload_reswaps_wallpaper_only_on_path_change() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(icedtea_config::default_config(), tx);
        state.wallpaper.set_decoded(Some(image::RgbaImage::from_pixel(1, 1, image::Rgba([0, 0, 0, 255]))));
        let mut cfg = icedtea_config::default_config();
        cfg.appearance.wallpaper = Some("/nonexistent/x.png".into());
        state.apply_reloaded_config(cfg);
        assert!(state.wallpaper.decoded().is_none(), "path change clears the stale wallpaper before redecode");
    }

    /// The negative counterpart the review flagged as missing: reloading
    /// with the *same* wallpaper path (including the default `None`) must
    /// leave the already-decoded image in place rather than clearing it and
    /// re-spawning a decode for nothing.
    #[test]
    fn reload_leaves_wallpaper_untouched_when_the_path_is_unchanged() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(icedtea_config::default_config(), tx);
        let img = image::RgbaImage::from_pixel(1, 1, image::Rgba([7, 7, 7, 255]));
        state.wallpaper.set_decoded(Some(img.clone()));

        // Same wallpaper path as `default_config()` (`None`), only an
        // unrelated field changes.
        let mut cfg = icedtea_config::default_config();
        cfg.appearance.palette.background = "#abcdef".into();
        assert_eq!(cfg.appearance.wallpaper, state.config.appearance.wallpaper, "path unchanged");
        state.apply_reloaded_config(cfg);

        assert_eq!(
            state.wallpaper.decoded(),
            Some(&img),
            "an unchanged wallpaper path must not clear the already-decoded image"
        );
    }

    #[test]
    fn close_action_removes_focused() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        let id = state.window_manager.add_window("app", "t", 1, DEFAULT_GEO);
        state.apply_action("close").unwrap();
        assert!(state.window_manager.get(id).is_none());
    }

    #[test]
    fn alt_tab_cycles_focus() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        let a = state.window_manager.add_window("a", "a", 1, DEFAULT_GEO);
        let b = state.window_manager.add_window("b", "b", 2, DEFAULT_GEO);
        state.apply_action("cycle:alt_tab").unwrap();
        assert!(state.window_manager.get(a).unwrap().focused);
        state.apply_action("cycle:alt_tab").unwrap();
        assert!(state.window_manager.get(b).unwrap().focused);
        state.apply_action("cycle:alt_tab").unwrap();
        assert!(state.window_manager.get(a).unwrap().focused);
    }

    #[test]
    fn handle_key_dispatches_bound_action() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        let id = state.window_manager.add_window("app", "t", 1, DEFAULT_GEO);
        // Default config binds SUPER+q to "close" (see config/src/defaults.rs).
        state.handle_key(input::Modifiers::SUPER, input::key_name_to_keysym("KEY_q")).unwrap();
        assert!(state.window_manager.get(id).is_none());
    }

    #[test]
    fn handle_key_ignores_unbound_combo() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        assert_eq!(state.handle_key(input::Modifiers::empty(), 0x12345), None);
    }

    #[test]
    fn handle_pointer_drag_moves_window_without_snap() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        state.outputs.insert(0, OutputSurface::new(Rectangle { x: 0, y: 0, width: 1000, height: 800 }));
        let id = state.window_manager.add_window("app", "t", 1, Rectangle { x: 100, y: 100, width: 640, height: 400 });
        state.window_manager.focus(id).unwrap();
        // Press inside the title bar's move area (not on a button).
        state.handle_pointer(PointerEvent::Press { id, pointer: (120, 105) }).unwrap();
        // Drag to a point away from any snap edge.
        state.handle_pointer(PointerEvent::Motion { pointer: (400, 400) }).unwrap();
        assert_eq!(state.snap_preview, None);
        state.handle_pointer(PointerEvent::Release { pointer: (400, 400) }).unwrap();
        let geo = state.window_manager.get(id).unwrap().geometry;
        // grab_offset was (20, 5); released at (400, 400) => top-left (380, 395).
        assert_eq!((geo.x, geo.y), (380, 395));
    }

    #[test]
    fn handle_pointer_drag_to_edge_snaps() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        state.outputs.insert(0, OutputSurface::new(Rectangle { x: 0, y: 0, width: 1000, height: 800 }));
        let id = state.window_manager.add_window("app", "t", 1, Rectangle { x: 100, y: 100, width: 640, height: 400 });
        state.window_manager.focus(id).unwrap();
        state.handle_pointer(PointerEvent::Press { id, pointer: (120, 105) }).unwrap();
        state.handle_pointer(PointerEvent::Motion { pointer: (2, 400) }).unwrap();
        assert!(state.snap_preview.is_some());
        state.handle_pointer(PointerEvent::Release { pointer: (2, 400) }).unwrap();
        let geo = state.window_manager.get(id).unwrap().geometry;
        assert_eq!(geo.width, 1000 / 2 - 16);
        assert_eq!(state.snap_preview, None);
    }

    // --- Review fix-round tests (task-11-review.md) ---

    /// Important #1: a session's entry list is frozen at `start()` and used
    /// for the whole session even if `window_manager` changes underneath it
    /// mid-cycle -- stepping must not panic or desync just because a window
    /// in the frozen list got removed.
    #[test]
    fn alt_tab_session_survives_churn_and_ends_cleanly() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        let a = state.window_manager.add_window("a", "a", 1, DEFAULT_GEO);
        let b = state.window_manager.add_window("b", "b", 2, DEFAULT_GEO);
        let c = state.window_manager.add_window("c", "c", 3, DEFAULT_GEO);
        // `alt_tab_entries()` is id-ordered: [a, b, c]. First cycle starts
        // the session and focuses entries[0] = a.
        state.apply_action("cycle:alt_tab").unwrap();
        assert!(state.alt_tab.is_active());
        assert!(state.window_manager.get(a).unwrap().focused);

        // Churn mid-session: a window in the frozen entry list disappears.
        state.window_manager.remove_window(b);

        // Stepping must not panic even though the stored entries still
        // reference the now-gone `b` (index 1); `WindowManager::focus`
        // quietly no-ops on a missing id instead of this call failing.
        state.apply_action("cycle:alt_tab").unwrap(); // steps to index 1 (b, gone)
        state.apply_action("cycle:alt_tab").unwrap(); // steps to index 2 (c)
        assert!(state.alt_tab.is_active());
        assert_eq!(state.alt_tab.entries().len(), 3, "entry list must stay frozen across churn");
        assert!(state.window_manager.get(c).unwrap().focused);

        // Drain events so we can inspect exactly what `end_alt_tab` sends.
        let _ = rx.try_iter().count();
        state.end_alt_tab();
        assert!(!state.alt_tab.is_active());
        let events: Vec<_> = rx.try_iter().collect();
        assert!(
            events.iter().any(|e| matches!(e.event, Event::AltTabState(AltTabState { active: false, .. }))),
            "end_alt_tab must emit a terminal AltTabState so the shell overlay can dismiss"
        );

        // A no-op call afterwards must not emit a second terminal event.
        state.end_alt_tab();
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn end_alt_tab_is_a_noop_when_no_session_is_active() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        assert!(!state.alt_tab.is_active());
        state.end_alt_tab();
        assert!(rx.try_recv().is_err(), "no session was active, nothing should be emitted");
    }

    /// Important #2: `saved_geometry` used to be a single map shared by
    /// `toggle_fullscreen` and `snap`; snapping then fullscreening a window
    /// clobbered the pre-snap restore point with the snapped geometry, and
    /// `snap_restore` after exiting fullscreen silently did nothing.
    #[test]
    fn snap_then_fullscreen_then_unfullscreen_then_snap_restore_round_trips() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        state.outputs.insert(0, OutputSurface::new(Rectangle { x: 0, y: 0, width: 1000, height: 800 }));
        let original = Rectangle { x: 50, y: 60, width: 300, height: 200 };
        let id = state.window_manager.add_window("app", "t", 1, original);
        state.window_manager.focus(id).unwrap();

        state.snap(id, SnapZone::Left).unwrap();
        assert_eq!(state.window_manager.get(id).unwrap().geometry.width, 1000 / 2 - 16);

        state.toggle_fullscreen(id).unwrap(); // enter fullscreen from the snapped geometry
        assert!(state.window_manager.get(id).unwrap().fullscreen);
        state.toggle_fullscreen(id).unwrap(); // exit fullscreen
        assert!(!state.window_manager.get(id).unwrap().fullscreen);
        // Exiting fullscreen must restore the *snapped* geometry, not the
        // pre-snap original -- fullscreen's own restore point is untouched
        // by snap's map.
        assert_eq!(state.window_manager.get(id).unwrap().geometry.width, 1000 / 2 - 16);

        state.snap_restore(id).unwrap();
        assert_eq!(state.window_manager.get(id).unwrap().geometry, original);
    }

    /// A live reload must PRESERVE every window row and its client -- no
    /// `WindowClosed` burst -- while still swapping in the new appearance.
    /// This is the M3 contract that replaced the old "rebuild the manager
    /// and close everything" behavior.
    #[test]
    fn apply_config_preserves_windows_and_does_not_close_them() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(icedtea_config::default_config(), tx);
        let a = state.window_manager.add_window("a", "a", 1, Rectangle { x: 0, y: 0, width: 100, height: 100 });
        let b = state.window_manager.add_window("b", "b", 1, Rectangle { x: 10, y: 10, width: 100, height: 100 });
        let mut cfg = icedtea_config::default_config();
        cfg.appearance.palette.background = "#123456".into();
        let events = state.apply_config(cfg);
        assert!(state.window_manager.get(a).is_some() && state.window_manager.get(b).is_some(),
                "windows survive a reload");
        assert!(!events.iter().any(|e| matches!(e, Event::WindowClosed(_))),
                "no WindowClosed burst on reload");
        assert_eq!(state.config.appearance.palette.background, "#123456");
    }

    /// A post-reload `add_window` must never reuse a pre-reload id: because
    /// the manager is no longer rebuilt, `next_id` advances monotonically on
    /// its own without any explicit floor.
    #[test]
    fn apply_config_keeps_id_counter_monotonic() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        let a = state.window_manager.add_window("a", "a", 1, DEFAULT_GEO);
        let b = state.window_manager.add_window("b", "b", 2, DEFAULT_GEO);
        let _ = state.apply_config(icedtea_config::default_config());
        let c = state.window_manager.add_window("c", "c", 3, DEFAULT_GEO);
        assert!(c.0 > a.0 && c.0 > b.0, "ids never regress across a reload");
    }

    /// Important #4: `n - 1` on a `u32` action argument must never panic,
    /// and an out-of-range workspace index must be a reported failure, not
    /// a silent no-op success.
    #[test]
    fn workspace_zero_does_not_panic_and_fails_cleanly() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        assert_eq!(state.apply_action("workspace:0"), None);
        assert_eq!(state.window_manager.active_workspace(), 0, "a failed switch must not move the active workspace");
    }

    #[test]
    fn workspace_out_of_range_fails_instead_of_reporting_success() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        // Default config has 4 workspaces (config/src/defaults.rs); 99 is
        // well out of range.
        assert_eq!(state.apply_action("workspace:99"), None);
        assert_eq!(state.window_manager.active_workspace(), 0);
    }

    #[test]
    fn move_to_workspace_zero_does_not_panic_and_fails_cleanly() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        state.window_manager.add_window("app", "t", 1, DEFAULT_GEO);
        assert_eq!(state.apply_action("move_to_workspace:0"), None);
    }

    // --- Re-review fix-round tests (task-11-review.md round 2) ---

    /// Important #2: pressing an unfocused window must focus it, and that
    /// focus must take effect *before* the decoration action for the same
    /// click is evaluated (so the click that raises a window can also act
    /// on it in one motion, matching ordinary click-to-focus behavior).
    #[test]
    fn press_on_unfocused_window_focuses_then_hits_its_decorations() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        let a_geo = Rectangle { x: 0, y: 0, width: 640, height: 400 };
        let a = state.window_manager.add_window("a", "a", 1, a_geo);
        let b = state.window_manager.add_window("b", "b", 2, Rectangle { x: 700, y: 0, width: 640, height: 400 });
        // `b` was added last, so it's focused; `a` is not.
        assert!(!state.window_manager.get(a).unwrap().focused);
        assert!(state.window_manager.get(b).unwrap().focused);

        // Press on `a`'s title bar (not a button): before the fix,
        // `decoration_action_for` would gate on `a.focused` (false) and
        // this whole call would return `None`, doing nothing.
        state.handle_pointer(PointerEvent::Press { id: a, pointer: (20, 5) }).unwrap();
        assert!(state.window_manager.get(a).unwrap().focused);
        assert!(!state.window_manager.get(b).unwrap().focused);

        // Now that `a` is focused, a press on its close button must close
        // it -- proving the *same* click sequence both focuses and acts.
        let close_pt = (a_geo.x + a_geo.width - 5, a_geo.y + 5);
        state.handle_pointer(PointerEvent::Press { id: a, pointer: close_pt }).unwrap();
        assert!(state.window_manager.get(a).is_none());
    }

    /// Important #3 (adjacent staleness bugs on the reload seam): a reload
    /// mid-drag/mid-alt-tab must not leave `drag`/`alt_tab`/`snap_preview`
    /// referencing windows that `apply_config` is about to discard.
    #[test]
    fn apply_config_resets_drag_alt_tab_and_snap_preview() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        state.outputs.insert(0, OutputSurface::new(Rectangle { x: 0, y: 0, width: 1000, height: 800 }));
        let a = state.window_manager.add_window("a", "a", 1, DEFAULT_GEO);
        let b = state.window_manager.add_window("b", "b", 2, DEFAULT_GEO);

        state.drag.begin(a, (5, 5));
        state.alt_tab.start(vec![a, b]);
        state.snap_preview = Some(Rectangle { x: 0, y: 0, width: 500, height: 800 });
        assert!(state.drag.window_id().is_some());
        assert!(state.alt_tab.is_active());
        assert!(state.snap_preview.is_some());

        let _ = state.apply_config(icedtea_config::default_config());

        assert!(state.drag.window_id().is_none(), "drag must not survive a reload mid-drag");
        assert!(!state.alt_tab.is_active(), "alt-tab session must not survive a reload mid-cycle");
        assert!(state.snap_preview.is_none(), "a stale snap preview must not render forever after reload");
    }

    /// Important #4: `snapshot().seq` must never go backwards across a
    /// reload, and `State::emit` (used for `AltTabState`) must advance the
    /// same counter as every other event.
    #[test]
    fn apply_config_does_not_regress_seq() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        state.window_manager.add_window("a", "a", 1, DEFAULT_GEO);
        state.window_manager.add_window("b", "b", 2, DEFAULT_GEO);
        let seq_before = state.window_manager.snapshot().seq;
        assert!(seq_before > 0);

        let _ = state.apply_config(icedtea_config::default_config());

        assert!(
            state.window_manager.snapshot().seq >= seq_before,
            "a fresh WindowManager's seq must be floored at the pre-reload high-water mark"
        );
    }

    #[test]
    fn emit_advances_seq() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        let seq_before = state.window_manager.snapshot().seq;
        state.emit(Event::AltTabState(AltTabState { active: true, entries: vec![], index: 0 }));
        assert!(
            state.window_manager.snapshot().seq > seq_before,
            "State::emit must bump seq like every other pending-event producer"
        );
    }

    // --- Re-review fix-round-3 tests (task-11-review.md round 3) ---

    /// Important #1: a reload mid-alt-tab-cycle must emit the terminal
    /// `AltTabState{active: false, ..}` so a shell overlay rendered from
    /// the session's last `active: true` event has a dismiss signal,
    /// instead of `apply_config` just silently resetting the machine.
    #[test]
    fn apply_config_mid_cycle_emits_alt_tab_inactive() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        state.window_manager.add_window("a", "a", 1, DEFAULT_GEO);
        state.window_manager.add_window("b", "b", 2, DEFAULT_GEO);
        state.apply_action("cycle:alt_tab").unwrap();
        assert!(state.alt_tab.is_active());

        let events = state.apply_config(icedtea_config::default_config());

        assert!(
            events.iter().any(|e| matches!(e, Event::AltTabState(AltTabState { active: false, .. }))),
            "reload mid-cycle must emit the terminal AltTabState"
        );
        assert!(!state.alt_tab.is_active());
    }

    #[test]
    fn apply_config_when_alt_tab_inactive_emits_no_extra_alt_tab_state() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        assert!(!state.alt_tab.is_active());
        let events = state.apply_config(icedtea_config::default_config());
        assert!(!events.iter().any(|e| matches!(e, Event::AltTabState(_))));
    }

    /// Minor #2: `handle_pointer_press`'s `focus()` mutation must flush
    /// immediately, not get deferred by an early `?`-return further down
    /// the same function (e.g. `decoration_action_for` returning `None`
    /// for a fullscreen window).
    #[test]
    fn press_on_unfocused_fullscreen_window_flushes_focus_immediately() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        state.outputs.insert(0, OutputSurface::new(Rectangle { x: 0, y: 0, width: 1000, height: 800 }));
        let a = state.window_manager.add_window("a", "a", 1, DEFAULT_GEO);
        let _b = state.window_manager.add_window("b", "b", 2, DEFAULT_GEO);
        state.window_manager.set_fullscreen(a, true).unwrap();
        let _ = rx.try_iter().count(); // drain setup events
        assert!(!state.window_manager.get(a).unwrap().focused, "b was added last and is focused, not a");

        // `decoration_action_for` returns `None` for a fullscreen window,
        // so this call itself returns `None` -- but the focus mutation
        // must already be visible on the channel by the time it does.
        assert_eq!(state.handle_pointer(PointerEvent::Press { id: a, pointer: (20, 5) }), None);
        assert!(state.window_manager.get(a).unwrap().focused);
        let events: Vec<_> = rx.try_iter().collect();
        assert!(
            events
                .iter()
                .any(|e| matches!(&e.event, Event::WindowUpdated { id, update } if *id == a && update.focused == Some(true))),
            "focus event must be flushed immediately, not deferred until an unrelated later flush"
        );
    }

    // --- Task 13: config hot reload over `ReloadConfig` ---

    #[test]
    fn reload_config_applies_new_workspaces_and_emits() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut state = State::new(icedtea_config::default_config(), tx);
        // Persist a config with different workspace names, then reload from disk.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cfg.redb");
        let db = icedtea_config::open(&path).unwrap();
        let mut cfg = icedtea_config::default_config();
        cfg.workspace_names = vec!["A".into(), "B".into(), "C".into()];
        cfg.save(&db).unwrap();
        drop(db);

        state.config_path = Some(path.clone());
        let events = state.reload_config_from_disk();
        assert_eq!(state.window_manager.workspace_info().len(), 3);
        assert!(events.iter().any(|e| matches!(e, Event::ConfigReloaded(_))));
        let emitted = rx.try_iter().collect::<Vec<_>>();
        assert!(emitted.iter().any(|e| matches!(e.event, Event::ConfigReloaded(_))));
    }

    /// `handle_command`'s `ReloadConfig` arm must go through the async
    /// worker path -- it only spawns a thread and returns, never blocking
    /// on the load itself -- and must be a harmless no-op when
    /// `config_reload_tx` hasn't been wired (as in every other test in this
    /// module, which never call `set_config_reload_sender`).
    #[test]
    fn handle_command_reload_without_sender_wired_is_a_harmless_noop() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        assert_eq!(state.handle_command(crate::dbus::DbCommand::ReloadConfig), Some(()));
    }

    /// The async worker path actually delivers a reloaded config back to
    /// the channel `set_config_reload_sender` was given, and
    /// `apply_reloaded_config` applies + emits it exactly like the sync
    /// path.
    #[test]
    fn handle_command_reload_spawns_worker_that_delivers_config() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut state = State::new(icedtea_config::default_config(), tx);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cfg.redb");
        let db = icedtea_config::open(&path).unwrap();
        let mut cfg = icedtea_config::default_config();
        cfg.workspace_names = vec!["X".into(), "Y".into()];
        cfg.save(&db).unwrap();
        drop(db);
        state.config_path = Some(path);

        let (reload_tx, reload_rx) = crossbeam_channel::unbounded::<Config>();
        state.set_config_reload_sender(reload_tx);

        assert_eq!(state.handle_command(crate::dbus::DbCommand::ReloadConfig), Some(()));

        // The worker thread runs off-loop; block briefly for its result
        // (mirrors how `drain_config_reload` would pick it up on the next
        // turn).
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let mut received = None;
        while std::time::Instant::now() < deadline {
            match reload_rx.try_recv() {
                Ok(cfg) => {
                    received = Some(cfg);
                    break;
                }
                Err(crossbeam_channel::TryRecvError::Empty) => {
                    std::thread::sleep(std::time::Duration::from_millis(5))
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => break,
            }
        }
        let cfg = received.expect("worker thread must deliver a Config over the reload channel");
        assert_eq!(cfg.workspace_names, vec!["X".to_string(), "Y".to_string()]);

        state.apply_reloaded_config(cfg);
        assert_eq!(state.window_manager.workspace_info().len(), 2);
        let emitted = rx.try_iter().collect::<Vec<_>>();
        assert!(emitted.iter().any(|e| matches!(e.event, Event::ConfigReloaded(_))));
    }

    /// The keybinding-triggered `"reload"` action (`SUPER+SHIFT+r` by
    /// default) must go through the exact same async worker path as the
    /// D-Bus `ReloadConfig` command -- per the task-13 threading
    /// requirement, neither may block the render loop with redb I/O.
    #[test]
    fn apply_action_reload_spawns_worker_that_delivers_config() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(icedtea_config::default_config(), tx);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cfg.redb");
        let db = icedtea_config::open(&path).unwrap();
        let mut cfg = icedtea_config::default_config();
        cfg.workspace_names = vec!["P".into(), "Q".into()];
        cfg.save(&db).unwrap();
        drop(db);
        state.config_path = Some(path);

        let (reload_tx, reload_rx) = crossbeam_channel::unbounded::<Config>();
        state.set_config_reload_sender(reload_tx);

        assert_eq!(state.apply_action("reload"), Some(()));

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let mut received = None;
        while std::time::Instant::now() < deadline {
            match reload_rx.try_recv() {
                Ok(cfg) => {
                    received = Some(cfg);
                    break;
                }
                Err(crossbeam_channel::TryRecvError::Empty) => {
                    std::thread::sleep(std::time::Duration::from_millis(5))
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => break,
            }
        }
        let cfg = received.expect("worker thread must deliver a Config over the reload channel");
        assert_eq!(cfg.workspace_names, vec!["P".to_string(), "Q".to_string()]);
    }

    #[test]
    fn apply_action_reload_without_sender_wired_is_a_harmless_noop() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        assert_eq!(state.apply_action("reload"), Some(()));
    }

    // --- Final-review fix-round tests ---

    fn state_with_output(width: i32, height: i32) -> (State, crossbeam_channel::Receiver<SeqEvent>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        state.outputs.insert(0, OutputSurface::new(Rectangle { x: 0, y: 0, width, height }));
        (state, rx)
    }

    /// C1/I5: maximize computes and applies real geometry against its own
    /// restore slot, and toggling back returns exactly the pre-maximize
    /// geometry -- it used to flip a flag and nothing else.
    #[test]
    fn maximize_applies_output_geometry_and_restores_it() {
        let (mut state, _rx) = state_with_output(1000, 800);
        let original = Rectangle { x: 40, y: 50, width: 300, height: 200 };
        let id = state.window_manager.add_window("app", "t", 1, original);

        state.toggle_maximized(id).unwrap();
        let w = state.window_manager.get(id).unwrap();
        assert!(w.maximized);
        assert_eq!(w.geometry, layout::maximized_geometry(Rectangle { x: 0, y: 0, width: 1000, height: 800 }, 8));

        state.toggle_maximized(id).unwrap();
        let w = state.window_manager.get(id).unwrap();
        assert!(!w.maximized);
        assert_eq!(w.geometry, original);
    }

    /// I5: maximize's restore slot is independent of snap's, exactly like
    /// fullscreen's -- snapping between maximize and unmaximize must not
    /// clobber either restore point.
    #[test]
    fn maximize_and_snap_keep_independent_restore_points() {
        let (mut state, _rx) = state_with_output(1000, 800);
        let original = Rectangle { x: 40, y: 50, width: 300, height: 200 };
        let id = state.window_manager.add_window("app", "t", 1, original);
        state.window_manager.focus(id).unwrap();

        state.snap(id, SnapZone::Left).unwrap();
        let snapped = state.window_manager.get(id).unwrap().geometry;
        state.set_maximized_target(id, true).unwrap();
        state.set_maximized_target(id, false).unwrap();
        assert_eq!(state.window_manager.get(id).unwrap().geometry, snapped, "unmaximize returns to the snapped geometry");
        state.snap_restore(id).unwrap();
        assert_eq!(state.window_manager.get(id).unwrap().geometry, original);
    }

    /// I5: `MaximizeWindow` over D-Bus is not a visual no-op any more, and
    /// an explicit target that already holds stays a no-op (it must not
    /// re-save the current geometry as a fresh restore point).
    #[test]
    fn maximize_command_is_idempotent_on_its_target() {
        let (mut state, _rx) = state_with_output(1000, 800);
        let original = Rectangle { x: 40, y: 50, width: 300, height: 200 };
        let id = state.window_manager.add_window("app", "t", 1, original);
        state.handle_command(crate::dbus::DbCommand::Maximize(id, true)).unwrap();
        let maximized = state.window_manager.get(id).unwrap().geometry;
        state.handle_command(crate::dbus::DbCommand::Maximize(id, true)).unwrap();
        assert_eq!(state.window_manager.get(id).unwrap().geometry, maximized);
        state.handle_command(crate::dbus::DbCommand::Maximize(id, false)).unwrap();
        assert_eq!(state.window_manager.get(id).unwrap().geometry, original, "the restore point survived the no-op");
    }

    /// H1: `set_maximized_target` resolves the output via the window's own
    /// frame center (`output_for_window`), not the pointer -- with two
    /// outputs and no runtime attached, `output_for_pointer`'s own fallback
    /// is the *lowest* index (output 0), which is exactly the wrong output
    /// for a window that lives on output 1. Reached from client requests
    /// and `DbCommand::Maximize`, neither of which carries any pointer
    /// correlation with the target window at all.
    #[test]
    fn set_maximized_target_resolves_via_the_windows_own_output_not_the_pointer() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(icedtea_config::default_config(), tx);
        state.config.appearance.snap_gap = 0;
        state.create_output(0, Rectangle { x: 0, y: 0, width: 800, height: 600 });
        state.create_output(1, Rectangle { x: 800, y: 0, width: 800, height: 600 });

        let id = state.window_manager.add_window("app", "t", 1, Rectangle { x: 810, y: 10, width: 300, height: 200 });
        state.set_maximized_target(id, true).unwrap();

        let geo = state.window_manager.get(id).unwrap().geometry;
        assert_eq!(geo, state.outputs[&1].usable, "must maximize onto output 1's usable rect, not output 0's");
        assert!(geo.x >= 800, "must not have resolved onto output 0, got {geo:?}");
    }

    /// H1's fullscreen counterpart: `set_fullscreen_target` resolves via the
    /// window's own frame center too, and keeps `.geometry` (not `.usable`)
    /// once it does -- fullscreen covers panels by definition.
    #[test]
    fn set_fullscreen_target_resolves_via_the_windows_own_output_not_the_pointer() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(icedtea_config::default_config(), tx);
        state.create_output(0, Rectangle { x: 0, y: 0, width: 800, height: 600 });
        state.create_output(1, Rectangle { x: 800, y: 0, width: 800, height: 600 });

        let id = state.window_manager.add_window("app", "t", 1, Rectangle { x: 810, y: 10, width: 300, height: 200 });
        state.set_fullscreen_target(id, true).unwrap();

        let geo = state.window_manager.get(id).unwrap().geometry;
        assert_eq!(geo, state.outputs[&1].geometry, "must fullscreen onto output 1's geometry, not output 0's");
        assert!(geo.x >= 800, "must not have resolved onto output 0, got {geo:?}");
    }

    /// Re-review Important 1: `DbCommand::Minimize` goes through the same
    /// implementation as the title-bar button, so minimizing the focused
    /// window over D-Bus (the taskbar path) hands focus to the workspace's
    /// MRU successor instead of leaving the model's focus pointer on a
    /// now-invisible window and the keyboard dead.
    #[test]
    fn dbus_minimize_of_focused_window_hands_focus_to_successor() {
        let (mut state, _rx) = state_with_output(1000, 800);
        let geo = Rectangle { x: 10, y: 10, width: 300, height: 200 };
        let a = state.window_manager.add_window("app", "a", 1, geo);
        let b = state.window_manager.add_window("app", "b", 2, geo);
        state.window_manager.focus(b).unwrap();

        state.handle_command(crate::dbus::DbCommand::Minimize(b, true)).unwrap();
        assert!(state.window_manager.get(b).unwrap().minimized);
        assert_eq!(
            state.window_manager.focused_window().map(|w| w.id),
            Some(a),
            "focus handed to the MRU successor, matching the title-bar button"
        );
    }

    /// I3: `behavior.snap_enabled` is actually consumed -- with snapping
    /// off, neither the `snap:*` actions nor a drag to an edge snap
    /// anything, and the drag still performs a plain move.
    #[test]
    fn snap_disabled_blocks_actions_and_drag_snapping() {
        let (mut state, _rx) = state_with_output(1000, 800);
        state.config.behavior.snap_enabled = false;
        let geo = Rectangle { x: 100, y: 100, width: 640, height: 400 };
        let id = state.window_manager.add_window("app", "t", 1, geo);
        state.window_manager.focus(id).unwrap();

        assert_eq!(state.apply_action("snap:left"), None, "the snap action must not fire when snapping is off");
        assert_eq!(state.window_manager.get(id).unwrap().geometry, geo);

        // A drag to the left edge shows no preview and ends as a plain move.
        state.handle_pointer(PointerEvent::Press { id, pointer: (120, 105) }).unwrap();
        state.handle_pointer(PointerEvent::Motion { pointer: (2, 400) }).unwrap();
        assert_eq!(state.snap_preview, None, "no snap preview may be drawn when snapping is off");
        state.handle_pointer(PointerEvent::Release { pointer: (2, 400) }).unwrap();
        let moved = state.window_manager.get(id).unwrap().geometry;
        assert_eq!((moved.x, moved.y), (2 - 20, 400 - 5), "the drag still moves the window");
        assert_eq!((moved.width, moved.height), (geo.width, geo.height), "…without resizing it");
    }

    /// I3 (the other direction): the default config leaves snapping on, so
    /// nothing about the existing behavior changes.
    #[test]
    fn snap_enabled_by_default_still_snaps() {
        let (mut state, _rx) = state_with_output(1000, 800);
        assert!(state.config.behavior.snap_enabled);
        let id = state.window_manager.add_window("app", "t", 1, DEFAULT_GEO);
        state.window_manager.focus(id).unwrap();
        assert_eq!(state.apply_action("snap:left"), Some(()));
        assert_eq!(state.window_manager.get(id).unwrap().geometry.width, 1000 / 2 - 16);
    }

    /// I6: after `MoveToWorkspace` the destination has a focused window, so
    /// the *next* action isn't a silent no-op -- and the origin workspace is
    /// left focused on whatever it still has.
    #[test]
    fn move_to_workspace_focuses_the_moved_window_and_reseats_the_origin() {
        let (mut state, _rx) = state_with_output(1000, 800);
        let a = state.window_manager.add_window("a", "a", 1, DEFAULT_GEO);
        let b = state.window_manager.add_window("b", "b", 2, DEFAULT_GEO);
        // `b` is focused (added last) and is the one that moves.
        state.handle_command(crate::dbus::DbCommand::MoveToWorkspace(b, 1)).unwrap();

        assert_eq!(state.window_manager.active_workspace(), 1);
        assert_eq!(state.window_manager.focused_window().map(|w| w.id), Some(b), "the moved window is focused there");
        // The origin kept a coherent focus rather than a dangling pointer.
        state.switch_workspace(0).unwrap();
        assert_eq!(state.window_manager.focused_window().map(|w| w.id), Some(a));
    }

    /// I6, via the keybinding path: the action that used to leave the
    /// destination unfocused is followed by a `fullscreen` that must act on
    /// the moved window.
    #[test]
    fn action_after_move_to_workspace_is_not_a_noop() {
        let (mut state, _rx) = state_with_output(1000, 800);
        let id = state.window_manager.add_window("app", "t", 1, DEFAULT_GEO);
        state.apply_action("move_to_workspace:2").unwrap();
        assert_eq!(state.apply_action("fullscreen"), Some(()), "the next action must find a focused window");
        assert!(state.window_manager.get(id).unwrap().fullscreen);
    }

    /// I1/I6: switching to a workspace that has windows focuses its MRU
    /// head, so actions work there immediately; switching to an empty one
    /// leaves focus cleanly absent rather than pointing elsewhere.
    #[test]
    fn switch_workspace_focuses_that_workspaces_mru_head() {
        let (mut state, _rx) = state_with_output(1000, 800);
        let a = state.window_manager.add_window("a", "a", 1, DEFAULT_GEO);
        state.switch_workspace(1).unwrap();
        assert!(state.window_manager.focused_window().is_none(), "workspace 2 is empty");
        assert_eq!(state.apply_action("close"), None, "…so an action there is a clean no-op");
        state.switch_workspace(0).unwrap();
        assert_eq!(state.window_manager.focused_window().map(|w| w.id), Some(a));
    }

    /// I2: every event reaching the D-Bus channel carries the seq its
    /// mutation produced, those seqs strictly increase, and the last one
    /// matches `snapshot().seq` -- which is what lets a subscriber order
    /// signals against a `GetState()` snapshot.
    #[test]
    fn emitted_events_carry_monotonic_seq_matching_the_snapshot() {
        let (mut state, rx) = state_with_output(1000, 800);
        let id = state.window_manager.add_window("app", "t", 1, DEFAULT_GEO);
        state.emit_pending();
        state.handle_command(crate::dbus::DbCommand::Fullscreen(id, true)).unwrap();
        state.handle_command(crate::dbus::DbCommand::SetWorkspace(1)).unwrap();

        let events: Vec<SeqEvent> = rx.try_iter().collect();
        assert!(events.len() >= 3, "expected several events, got {}", events.len());
        let seqs: Vec<u64> = events.iter().map(|e| e.seq).collect();
        assert!(seqs.windows(2).all(|w| w[1] > w[0]), "seqs must strictly increase: {seqs:?}");
        assert_eq!(*seqs.last().unwrap(), state.window_manager.snapshot().seq);
        assert!(seqs[0] > 0, "seq 0 means 'nothing has happened yet' and must never be emitted");
    }

    /// I2: the reload path used to bypass the counter entirely (its events
    /// went out through a separate queue with no seq of their own).
    #[test]
    fn reloaded_config_events_carry_seq_and_never_regress() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        state.window_manager.add_window("a", "a", 1, DEFAULT_GEO);
        state.emit_pending();
        let before = state.window_manager.snapshot().seq;
        let drained: Vec<SeqEvent> = rx.try_iter().collect();
        assert!(drained.iter().all(|e| e.seq <= before));

        state.apply_reloaded_config(icedtea_config::default_config());
        let events: Vec<SeqEvent> = rx.try_iter().collect();
        assert!(!events.is_empty());
        assert!(events.iter().all(|e| e.seq > before), "reload events must advance past the pre-reload high-water mark");
        let seqs: Vec<u64> = events.iter().map(|e| e.seq).collect();
        assert!(seqs.windows(2).all(|w| w[1] > w[0]), "seqs must strictly increase: {seqs:?}");
        assert!(events.iter().any(|e| matches!(e.event, Event::ConfigReloaded(_))));
    }

    /// C1: closing a model window with no backing client surface still
    /// removes it synchronously (there is no client to send `close` to and
    /// no destroy will ever arrive), and focus lands somewhere sensible.
    #[test]
    fn request_close_without_a_surface_removes_and_reseats_focus() {
        let (mut state, _rx) = state_with_output(1000, 800);
        let a = state.window_manager.add_window("a", "a", 1, DEFAULT_GEO);
        let b = state.window_manager.add_window("b", "b", 2, DEFAULT_GEO);
        state.request_close(b);
        assert!(state.window_manager.get(b).is_none());
        assert_eq!(state.window_manager.focused_window().map(|w| w.id), Some(a), "focus must not dangle after a close");
    }

    /// C1: closing must not leave restore points behind for an id that can
    /// never come back.
    #[test]
    fn closing_clears_saved_geometry_slots() {
        let (mut state, _rx) = state_with_output(1000, 800);
        let id = state.window_manager.add_window("app", "t", 1, DEFAULT_GEO);
        state.window_manager.focus(id).unwrap();
        state.snap(id, SnapZone::Left).unwrap();
        state.set_maximized_target(id, true).unwrap();
        state.set_fullscreen_target(id, true).unwrap();
        assert!(state.snap_saved_geometry.contains_key(&id));
        state.request_close(id);
        assert!(!state.snap_saved_geometry.contains_key(&id));
        assert!(!state.maximized_saved_geometry.contains_key(&id));
        assert!(!state.fullscreen_saved_geometry.contains_key(&id));
    }

    /// C1 (`resize_request`'s model half): an interactive resize driven by
    /// the pointer path updates the model geometry as it goes and commits
    /// the final one on release.
    #[test]
    fn interactive_resize_updates_geometry_and_ends_on_release() {
        let (mut state, _rx) = state_with_output(1000, 800);
        let geo = Rectangle { x: 100, y: 100, width: 400, height: 300 };
        let id = state.window_manager.add_window("app", "t", 1, geo);
        state.resize.begin(id, input::ResizeEdges { bottom: true, right: true, ..Default::default() }, geo, (500, 400));

        state.handle_pointer(PointerEvent::Motion { pointer: (560, 430) }).unwrap();
        assert_eq!(
            state.window_manager.get(id).unwrap().geometry,
            Rectangle { x: 100, y: 100, width: 460, height: 330 }
        );

        state.handle_pointer(PointerEvent::Release { pointer: (600, 500) }).unwrap();
        assert_eq!(
            state.window_manager.get(id).unwrap().geometry,
            Rectangle { x: 100, y: 100, width: 500, height: 400 }
        );
        assert!(state.resize.window_id().is_none(), "the resize ended on release");
        // A later motion with no resize and no drag is a clean no-op.
        assert_eq!(state.handle_pointer(PointerEvent::Motion { pointer: (700, 700) }), None);
    }

    /// M2: `State::new` accepts an arbitrary `Config`, including one with no
    /// workspace names -- that must not leave a reachable index panic.
    #[test]
    fn state_with_no_configured_workspaces_does_not_panic() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut cfg = default_config();
        cfg.workspace_names = vec![];
        let mut state = State::new(cfg, tx);
        let id = state.window_manager.add_window("app", "t", 1, DEFAULT_GEO);
        assert_eq!(state.window_manager.workspace_info().len(), 1);
        assert_eq!(state.window_manager.focused_window().map(|w| w.id), Some(id));
    }

    // --- Re-review fix round: New-1 / New-2 / New-3 ---
    //
    // These cover the *model* half of the focus-sync work plus the fact that
    // every focus path now runs `sync_focus_change`/`sync_seat_focus` without
    // panicking. The Wayland half proper -- that the losing window's client
    // is staged `Activated = false`, that the successor's client is staged
    // `Activated = true`, and that `wayland.keyboard_focus(Some(id))` is
    // called -- is **not** observable here: no window in this module's
    // tests is ever bound to a toplevel (`state.wayland.is_backed` is false
    // for all of them), so `sync_window_to_scene` short-circuits before
    // touching client state and `wayland.keyboard_focus` (a no-op in this
    // commit regardless) never fires. See the fix report for the
    // manual-trace argument that stands in for those.

    /// New-2: the successor `forget_window` picks is reconciled, not just
    /// chosen -- and the model never ends up with a dangling focus pointer.
    #[test]
    fn closing_the_focused_window_reconciles_the_mru_successor() {
        let (mut state, _rx) = state_with_output(1000, 800);
        let a = state.window_manager.add_window("a", "a", 1, DEFAULT_GEO);
        let b = state.window_manager.add_window("b", "b", 2, DEFAULT_GEO);
        assert_eq!(state.focused_id(), Some(b));

        state.request_close(b);
        assert_eq!(state.focused_id(), Some(a), "the successor is focused, not left dangling");
        assert!(state.window_manager.get(a).unwrap().focused);
    }

    /// New-2/New-3: with no successor left there is nothing to activate, and
    /// the model's focus pointer ends up cleared rather than pointing at the
    /// destroyed window.
    #[test]
    fn closing_the_last_window_leaves_nothing_focused_and_clears_the_seat() {
        let (mut state, _rx) = state_with_output(1000, 800);
        let id = state.window_manager.add_window("a", "a", 1, DEFAULT_GEO);
        state.request_close(id);
        assert_eq!(state.focused_id(), None);
    }

    /// New-1: a focus change reports the window that *lost* focus as
    /// unfocused in the model, which is the state `sync_focus_change` then
    /// pushes to that window's client as `Activated = false`.
    #[test]
    fn click_to_focus_unfocuses_the_previous_window() {
        let (mut state, _rx) = state_with_output(1000, 800);
        let a = state.window_manager.add_window("a", "a", 1, DEFAULT_GEO);
        let b = state.window_manager.add_window("b", "b", 2, DEFAULT_GEO);
        assert!(state.window_manager.get(b).unwrap().focused);

        state.handle_pointer(PointerEvent::Press { id: a, pointer: (5, 5) });
        assert!(state.window_manager.get(a).unwrap().focused);
        assert!(!state.window_manager.get(b).unwrap().focused, "the old focus is dropped");
    }

    /// New-3: switching to an empty workspace leaves nothing focused, and
    /// `sync_scene`'s trailing `sync_seat_focus` runs even though the
    /// per-window loop has nothing visible to say about it.
    #[test]
    fn switching_to_an_empty_workspace_clears_focus_and_the_seat() {
        let (mut state, _rx) = state_with_output(1000, 800);
        let id = state.window_manager.add_window("a", "a", 1, DEFAULT_GEO);
        assert_eq!(state.focused_id(), Some(id));

        state.switch_workspace(1).unwrap();
        assert_eq!(state.focused_id(), None, "workspace 2 is empty");

        // And switching back re-focuses the workspace's MRU head.
        state.switch_workspace(0).unwrap();
        assert_eq!(state.focused_id(), Some(id));
    }

    /// New-3: minimizing the focused window hands focus on when there is a
    /// successor, and leaves nothing focused when there isn't.
    #[test]
    fn minimizing_the_focused_window_moves_focus_on() {
        let (mut state, _rx) = state_with_output(1000, 800);
        let a = state.window_manager.add_window("a", "a", 1, DEFAULT_GEO);
        let b = state.window_manager.add_window("b", "b", 2, DEFAULT_GEO);

        state.apply_decoration_action(b, crate::decoration::DecorationAction::Minimize);
        assert!(state.window_manager.get(b).unwrap().minimized);
        assert_eq!(state.focused_id(), Some(a));

        state.apply_decoration_action(a, crate::decoration::DecorationAction::Minimize);
        // Task 6: with no successor, `refocus_after_hide` clears the focus
        // pointer outright rather than leaving it on the now-hidden `a` --
        // the model itself must never report a hidden window as focused, not
        // just have the seat mask it via `sync_seat_focus`'s `is_visible_id`
        // filter.
        assert_eq!(state.focused_id(), None);
        assert!(!state.window_manager.is_visible_id(a));
    }

    /// Task 6: `DbCommand::Minimize` (`set_minimized_and_reconcile`) must
    /// resolve the minimized window's *own* workspace, not whichever one is
    /// currently active -- `focused_id()` only ever reflects the active
    /// workspace, so minimizing a focused window on an inactive one used to
    /// skip the refocus branch entirely and could leave that workspace's
    /// pointer stale once something did populate it.
    #[test]
    fn minimizing_the_focused_window_on_an_inactive_workspace_leaves_no_stale_focus() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        // Make `a` workspace 2's *genuinely* focused window via the real
        // focus path (`add_window` focuses on the currently-active
        // workspace), not a raw `set_workspace` -- that unconditionally
        // clears `.focused` and never sets the destination's pointer, so a
        // test built on it would pass even with the old, unfixed guard.
        state.window_manager.set_active_workspace(2);
        let a = state.window_manager.add_window("a", "a", 1, Rectangle { x: 0, y: 0, width: 10, height: 10 });
        assert_eq!(state.window_manager.focused_window().map(|w| w.id), Some(a));
        assert!(state.window_manager.get(a).unwrap().focused);
        // Switch away to ws 1 (now inactive workspace 2 still points at `a`),
        // minimize `a`, then switch back.
        state.window_manager.set_active_workspace(1);
        state.set_minimized_and_reconcile(a, true);
        state.window_manager.set_active_workspace(2);
        assert!(
            state.window_manager.focused_window().is_none()
                || state.window_manager.focused_window().map(|w| !w.minimized).unwrap_or(true),
            "a minimized window must not remain the workspace's focus"
        );
        assert!(!state.window_manager.get(a).unwrap().focused, "the hidden window's own focused flag must clear too");
    }

    /// New-4: the inset the sync path applies is the shared
    /// `decoration::content_rect`, keyed off the same `has_ssd` predicate the
    /// renderer uses -- so a decorated window's client is configured a title
    /// bar shorter and a title bar lower, and a CSD one is left alone.
    /// (`sync_window_to_scene` itself can't be observed without a real
    /// toplevel; this pins the geometry contract it consumes.)
    #[test]
    fn ssd_inset_applies_to_decorated_windows_only() {
        use crate::decoration::{content_rect, has_ssd, TITLE_BAR_HEIGHT};
        let (mut state, _rx) = state_with_output(1000, 800);
        let geo = Rectangle { x: 10, y: 20, width: 400, height: 300 };
        let ssd = state.window_manager.add_window("org.example.Ssd", "t", 1, geo);
        let csd = state.window_manager.add_window("org.gtk.Csd", "t", 2, geo);

        let w = state.window_manager.get(ssd).unwrap();
        assert!(has_ssd(&w.app_id, w.client_decorations_requested, w.fullscreen));
        assert_eq!(
            content_rect(w.geometry, true),
            Rectangle { x: 10, y: 20 + TITLE_BAR_HEIGHT, width: 400, height: 300 - TITLE_BAR_HEIGHT }
        );

        let w = state.window_manager.get(csd).unwrap();
        assert!(!has_ssd(&w.app_id, w.client_decorations_requested, w.fullscreen));
        assert_eq!(content_rect(w.geometry, false), geo, "CSD windows are untouched");

        // Fullscreen drops the strip, so the client gets the whole output.
        state.set_fullscreen_target(ssd, true).unwrap();
        let w = state.window_manager.get(ssd).unwrap();
        assert!(!has_ssd(&w.app_id, w.client_decorations_requested, w.fullscreen));
        assert_eq!(content_rect(w.geometry, false), w.geometry);
    }

    /// Direct `SeatHandler::key` coverage, previously impossible (the M1
    /// gap): `wlr::KeyEvent::for_test` builds a synthetic press with no live
    /// keyboard behind it.
    ///
    /// `wlr::Modifiers` has no public constructor with flags set (only the
    /// `logo()`/`ctrl()`/`alt()`/`shift()` accessors), so this can't drive
    /// the default `quit` binding (super+shift+q) directly -- the fallback
    /// the task 3 brief calls for is a modifier-free binding inserted into
    /// the config before `State::new`, exercised with `Modifiers::default()`.
    /// `apply_action` dispatches on the keybindings map's own key as the
    /// action name (see its `match base` arms), so the override keeps the
    /// name `"quit"` and only replaces the combo -- a differently-named
    /// entry would never reach the `"quit"` arm at all.
    #[test]
    fn seat_key_matches_a_binding_and_consumes_the_event() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut config = default_config();
        config.keybindings.insert(
            "quit".into(),
            icedtea_config::KeyCombo { modifiers: vec![], key: "KEY_F12".into() },
        );
        let mut state = State::new(config, tx);
        let keysym = crate::input::key_name_to_keysym("KEY_F12");
        let ev = wlr::KeyEvent::for_test(keysym, wlr::Modifiers::default(), true, 1);

        let consumed = wlr::SeatHandler::key(&mut state, &ev);

        assert!(consumed, "a bound combo must be consumed, not forwarded");
        assert!(state.quitting, "the quit action must have run");
    }

    #[test]
    fn seat_key_without_a_binding_is_forwarded() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        let ev = wlr::KeyEvent::for_test(0x61 /* 'a' */, wlr::Modifiers::default(), true, 1);
        assert!(!wlr::SeatHandler::key(&mut state, &ev));
    }

    #[test]
    fn wallpaper_nodes_track_outputs() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        state.create_output(0, Rectangle { x: 0, y: 0, width: 800, height: 600 });
        let img = image::RgbaImage::from_pixel(2, 2, image::Rgba([9, 9, 9, 255]));
        state.wallpaper.set_decoded(Some(img));
        state.sync_wallpaper_nodes();
        // No runtime: node creation returns None, so the map stays empty --
        // the assertion here is that the call is a clean no-op without a
        // runtime.
        assert!(state.wallpaper_nodes.is_empty());
    }

    /// Task 10: `ToplevelHandler::request_maximize`'s model half --
    /// `reconcile_maximized` actually mutates the model, and reports whether
    /// it did so the dispatch layer knows when a bare configure is enough.
    #[test]
    fn a_client_maximize_request_reaches_the_model() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        state.create_output(0, Rectangle { x: 0, y: 0, width: 800, height: 600 });
        let id = state.window_manager.add_window("app", "t", 1, Rectangle { x: 10, y: 10, width: 200, height: 100 });
        let key = crate::wayland::ToplevelKey::for_test(1);
        state.wayland.bind(id, key);
        assert!(state.reconcile_maximized(key, true), "model must change");
        assert!(state.window_manager.get(id).expect("window").maximized);
        assert!(!state.reconcile_maximized(key, true), "idempotent request must report no change");
    }

    /// Task 10 (Deviation 8): a move request that arrives with no button
    /// held must not start a grab -- the crate deliberately forwards no
    /// seat/serial with `request_move`, so the compositor enforces its own
    /// pointer-pressed policy instead of trusting the client's claim.
    #[test]
    fn a_move_request_without_a_pressed_pointer_is_ignored() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        let id = state.window_manager.add_window("app", "t", 1, Rectangle { x: 0, y: 0, width: 100, height: 100 });
        state.pointer_pressed = false;
        state.begin_client_move(id);
        assert!(state.drag.window_id().is_none(), "no grab without a pressed pointer");
    }

    /// Review fix (task 10): `pointer_pressed` is a single scalar tracking
    /// "the left button is held", not a per-button set -- every button's
    /// `pressed` value used to overwrite it unconditionally, so a
    /// right-button press or release interleaved with a held left button
    /// spuriously set or cleared the flag `begin_client_move`/
    /// `begin_client_resize` gate on. Gating the assignment on `BTN_LEFT`
    /// fixes it: a right press/release while the left button stays down
    /// must leave `pointer_pressed` (and so a move grab) untouched.
    #[test]
    fn a_right_click_does_not_disturb_a_held_left_button() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        const BTN_LEFT: u32 = 0x110;
        const BTN_RIGHT: u32 = 0x111;

        wlr::SeatHandler::pointer_button(&mut state, 0.0, 0.0, BTN_LEFT, true, 0);
        assert!(state.pointer_pressed, "a left press must set pointer_pressed");

        wlr::SeatHandler::pointer_button(&mut state, 0.0, 0.0, BTN_RIGHT, true, 0);
        assert!(state.pointer_pressed, "a right press must not clear a held left button");

        wlr::SeatHandler::pointer_button(&mut state, 0.0, 0.0, BTN_RIGHT, false, 0);
        assert!(state.pointer_pressed, "a right release must not clear a held left button");
    }

    /// Counterpart: a lone right-click (no left button ever pressed) must
    /// never set `pointer_pressed`, so a `request_move` that happens to
    /// arrive during it is still ignored.
    #[test]
    fn a_lone_right_click_leaves_pointer_pressed_false() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        let id = state.window_manager.add_window("app", "t", 1, Rectangle { x: 0, y: 0, width: 100, height: 100 });
        const BTN_RIGHT: u32 = 0x111;

        wlr::SeatHandler::pointer_button(&mut state, 0.0, 0.0, BTN_RIGHT, true, 0);
        assert!(!state.pointer_pressed, "a lone right press must not set pointer_pressed");

        state.begin_client_move(id);
        assert!(state.drag.window_id().is_none(), "request_move must be ignored without a held left button");
    }

    // --- Task 13: full server-side decorations ---

    /// A left press on the rightmost `BUTTON_WIDTH` of a title bar is a
    /// close, all the way through the library's own seat entry point --
    /// `hit_test` maps it to `DecorationAction::Close`, which takes
    /// `request_close`'s path like every other close in this file.
    ///
    /// Deliberately *not* bound to a toplevel: `Wayland::close` reports
    /// `true` for a bound window (wait for the client's destroy) and `false`
    /// for an unbacked one (remove the row now), and only the second is
    /// observable without a live client, so this asserts the row is gone.
    #[test]
    fn a_titlebar_close_click_routes_to_request_close() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        state.create_output(0, Rectangle { x: 0, y: 0, width: 800, height: 600 });
        let id = state.window_manager.add_window(
            "plain.app",
            "t",
            1,
            Rectangle { x: 100, y: 100, width: 300, height: 200 },
        );
        let frame = state.window_manager.get(id).expect("w").geometry;
        let bar = crate::decoration::title_bar_rect(frame);
        // The close button is the rightmost of the three (`button_rects`
        // orders them minimize, maximize, close).
        let click = (bar.x + bar.width - 5, bar.y + 5);

        wlr::SeatHandler::pointer_button(&mut state, click.0 as f64, click.1 as f64, 0x110, true, 1);

        assert!(state.window_manager.get(id).is_none(), "close button must close");
        assert!(
            rx.try_iter().any(|e| matches!(e.event, icedtea_contract::Event::WindowClosed { .. })),
            "the close must have been announced"
        );
    }

    /// The middle button is maximize and the leftmost is minimize -- the
    /// same three model command paths D-Bus drives, reached by pointer.
    #[test]
    fn titlebar_buttons_hit_the_models_own_command_paths() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        state.create_output(0, Rectangle { x: 0, y: 0, width: 800, height: 600 });
        let id = state.window_manager.add_window(
            "plain.app",
            "t",
            1,
            Rectangle { x: 100, y: 100, width: 300, height: 200 },
        );
        let frame = state.window_manager.get(id).expect("w").geometry;
        let rects = crate::decoration::button_rects(frame);

        let maximize = (rects[1].x + 5, rects[1].y + 5);
        wlr::SeatHandler::pointer_button(&mut state, maximize.0 as f64, maximize.1 as f64, 0x110, true, 1);
        assert!(state.window_manager.get(id).expect("w").maximized, "middle button maximizes");

        let minimize = (rects[0].x + 5, rects[0].y + 5);
        // The window is maximized now, so its frame moved; re-derive.
        let frame = state.window_manager.get(id).expect("w").geometry;
        let minimize = if crate::decoration::button_rects(frame)[0].contains(minimize.0, minimize.1) {
            minimize
        } else {
            let r = crate::decoration::button_rects(frame)[0];
            (r.x + 5, r.y + 5)
        };
        wlr::SeatHandler::pointer_button(&mut state, minimize.0 as f64, minimize.1 as f64, 0x110, true, 1);
        assert!(state.window_manager.get(id).expect("w").minimized, "left button minimizes");
    }

    /// The negotiation's model half: a client asking to draw its own
    /// decorations is recorded as such, which is what `has_ssd` then reads.
    #[test]
    fn decoration_mode_request_updates_the_model_preference() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        let id = state.window_manager.add_window("app", "t", 1, Rectangle { x: 0, y: 0, width: 100, height: 100 });
        let key = crate::wayland::ToplevelKey::for_test(1);
        state.wayland.bind(id, key);

        wlr::ToplevelHandler::request_decoration_mode(
            &mut state,
            key.0,
            Some(wlr::DecorationMode::ClientSide),
        );
        assert_eq!(state.window_manager.get(id).expect("w").client_decorations_requested, Some(true));

        wlr::ToplevelHandler::request_decoration_mode(
            &mut state,
            key.0,
            Some(wlr::DecorationMode::ServerSide),
        );
        assert_eq!(state.window_manager.get(id).expect("w").client_decorations_requested, Some(false));

        wlr::ToplevelHandler::request_decoration_mode(&mut state, key.0, None);
        assert_eq!(state.window_manager.get(id).expect("w").client_decorations_requested, None);
    }

    /// The ordering that actually happens on the wire: the client states its
    /// preference before its initial commit, so there is no model window to
    /// record it against yet. It must not be lost -- `new_toplevel` collects
    /// it when the window is finally mapped.
    #[test]
    fn a_preference_stated_before_mapping_survives_until_the_window_exists() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        let key = crate::wayland::ToplevelKey::for_test(9);

        wlr::ToplevelHandler::request_decoration_mode(
            &mut state,
            key.0,
            Some(wlr::DecorationMode::ClientSide),
        );
        assert!(state.window_manager.windows().next().is_none(), "nothing is mapped yet");

        state.new_toplevel(key, "plain.app", "t", 1);
        let id = state.wayland.window_for(key).expect("bound at map");
        assert_eq!(
            state.window_manager.get(id).expect("w").client_decorations_requested,
            Some(true),
            "the pre-map preference must reach the model"
        );
        assert!(
            !crate::decoration::has_ssd("plain.app", Some(true), false),
            "and must suppress the band"
        );
    }

    /// A toplevel that dies before mapping takes its parked preference with
    /// it, rather than leaving an entry nothing will ever collect.
    #[test]
    fn a_preference_parked_for_a_toplevel_that_never_maps_is_dropped() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        let key = crate::wayland::ToplevelKey::for_test(11);

        wlr::ToplevelHandler::request_decoration_mode(
            &mut state,
            key.0,
            Some(wlr::DecorationMode::ClientSide),
        );
        assert_eq!(state.pending_decorations.len(), 1);
        state.forget_toplevel(key);
        assert!(state.pending_decorations.is_empty(), "an unmapped toplevel's preference must not leak");
    }

    /// The title raster is memoized on `(title, width, resolved color)`: the
    /// same window synced twice shapes once and keeps one generation, and a
    /// retitle both re-shapes and advances the generation (which is what
    /// makes the seam re-upload).
    #[test]
    fn title_rasters_are_memoized_and_invalidated_by_a_retitle() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        let id = state.window_manager.add_window(
            "plain.app",
            "first",
            1,
            Rectangle { x: 0, y: 0, width: 400, height: 300 },
        );
        let bar = crate::decoration::title_bar_rect(
            state.window_manager.get(id).expect("w").geometry,
        );
        let entry = |state: &State| {
            let e = state.title_rasters.get(&id).expect("cached");
            (e.generation, e.pixels.clone())
        };

        state.ensure_title_raster(id, "first".into(), bar, true);
        assert_eq!(state.title_rasters.len(), 1, "the raster must be cached");
        let first = entry(&state);

        state.ensure_title_raster(id, "first".into(), bar, true);
        assert_eq!(entry(&state), first, "an unchanged title must not re-shape or re-generation");

        state.ensure_title_raster(id, "second".into(), bar, true);
        let renamed = entry(&state);
        assert_eq!(state.title_rasters.len(), 1, "one entry per window, replaced not appended");
        assert_ne!(renamed.1, first.1, "a retitle must produce different pixels");
        assert!(renamed.0 > first.0, "a retitle must advance the generation");

        // Losing focus changes the resolved color, so it invalidates too.
        state.ensure_title_raster(id, "second".into(), bar, false);
        let unfocused = entry(&state);
        assert!(unfocused.0 > renamed.0, "a focus change must re-shape");
        assert_ne!(unfocused.1, renamed.1, "unfocused text is dimmer");

        state.forget_window(id);
        assert!(state.title_rasters.is_empty(), "a closed window's raster must not outlive it");
    }

    /// M3: the cache key carries the resolved foreground color, so a palette
    /// change invalidates a raster even for an otherwise identical title on
    /// an otherwise identical window.
    #[test]
    fn a_palette_change_invalidates_a_cached_title_raster() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        let id = state.window_manager.add_window(
            "plain.app",
            "same",
            1,
            Rectangle { x: 0, y: 0, width: 400, height: 300 },
        );
        let bar = crate::decoration::title_bar_rect(
            state.window_manager.get(id).expect("w").geometry,
        );

        state.ensure_title_raster(id, "same".into(), bar, true);
        let before = state.title_rasters.get(&id).expect("cached").pixels.clone();

        // Repaint the palette directly (rather than through `apply_config`,
        // which now preserves windows and would need a full appearance
        // resync to reach here) to isolate the raster cache's color key.
        state.config.appearance.palette.foreground = "#ff0000".into();
        state.ensure_title_raster(id, "same".into(), bar, true);
        let after = state.title_rasters.get(&id).expect("cached").pixels.clone();

        assert_ne!(before, after, "a repainted palette must not serve stale-colored text");
    }

    /// M3 preserve contract: a reload keeps every window row, so it must
    /// also keep those windows' cached title rasters -- purging them would
    /// force a needless re-shape of text that has not changed. (Palette
    /// changes are still invalidated by the raster's own color-keyed cache;
    /// see `a_palette_change_invalidates_a_cached_title_raster`.)
    #[test]
    fn a_config_reload_preserves_cached_rasters_for_surviving_windows() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        let id = state.window_manager.add_window(
            "plain.app",
            "survivor",
            1,
            Rectangle { x: 0, y: 0, width: 400, height: 300 },
        );
        let bar = crate::decoration::title_bar_rect(
            state.window_manager.get(id).expect("w").geometry,
        );
        state.ensure_title_raster(id, "survivor".into(), bar, true);
        assert_eq!(state.title_rasters.len(), 1);

        let _ = state.apply_config(default_config());
        assert!(state.window_manager.get(id).is_some(), "the window survives the reload");
        assert!(state.title_rasters.contains_key(&id), "its cached title raster survives too");
    }

    /// The title node is inset by the three buttons, so a long title runs
    /// out of room before it runs underneath the close button.
    #[test]
    fn the_title_raster_is_narrower_than_the_band_by_the_button_span() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        let id = state.window_manager.add_window(
            "plain.app",
            "a very long window title that would otherwise run under the buttons",
            1,
            Rectangle { x: 0, y: 0, width: 400, height: 300 },
        );
        let bar = crate::decoration::title_bar_rect(
            state.window_manager.get(id).expect("w").geometry,
        );
        let title = state.window_manager.get(id).expect("w").title.clone();

        state.ensure_title_raster(id, title, bar, true);
        let (w, h, px) = state
            .title_rasters
            .get(&id)
            .expect("cached")
            .pixels
            .clone()
            .expect("pixels");
        assert_eq!(w, bar.width - 3 * crate::decoration::BUTTON_WIDTH);
        assert_eq!(h, bar.height);
        assert_eq!(px.len(), (w * h * 4) as usize);
    }

    /// Button colors are premultiplied (every channel no greater than the
    /// alpha), which is what the wlroots scene graph composites.
    #[test]
    fn button_colors_are_premultiplied() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let state = State::new(default_config(), tx);
        for color in state.button_colors() {
            for channel in &color[..3] {
                assert!(*channel <= color[3] + f32::EPSILON, "{color:?} is not premultiplied");
            }
        }
    }

    // --- Task 14: snap preview rect, model-level unmapped, reload gap-close ---

    /// Without a runtime attached, `sync_snap_preview` is a seam no-op both
    /// ways -- but the `Option` field must still mirror `snap_preview`
    /// exactly (no stale id kept). The runtime-backed version of this test
    /// lives in `tests/headless_boot.rs`.
    #[test]
    fn snap_preview_rect_bookkeeping_follows_the_preview() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        state.snap_preview = Some(Rectangle { x: 0, y: 0, width: 400, height: 600 });
        state.sync_snap_preview();
        state.snap_preview = None;
        state.sync_snap_preview();
        assert!(state.snap_preview_rect.is_none());
    }

    /// Task 3 (wlr-port M3): the snap-preview rect is model-only overlay
    /// chrome, never a hit target. `window_at_point` consults only
    /// `window_manager`'s windows, so a click inside both an active preview
    /// and the window it overlaps must still resolve to the window -- this
    /// pins the model contract the `Band::Overlay` move (see
    /// `sync_snap_preview`) is meant to preserve at the wlr layer too.
    #[test]
    fn a_click_under_an_active_snap_preview_hits_the_window() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        state.create_output(0, Rectangle { x: 0, y: 0, width: 800, height: 600 });
        let id = state.window_manager.add_window(
            "app",
            "t",
            1,
            Rectangle { x: 100, y: 100, width: 300, height: 200 },
        );
        state.snap_preview = Some(Rectangle { x: 0, y: 0, width: 400, height: 600 });
        state.sync_snap_preview();
        // The window at a point inside both the preview and the window must
        // still resolve to the window -- the preview rect is not a hit
        // target.
        assert_eq!(state.window_at_point((150, 150)), Some(id));
    }

    /// Task 14 Step 2: `ToplevelHandler::unmapped` now moves focus to the
    /// next mapped candidate on the same workspace when it unmaps the
    /// focused window, the same shape `set_minimized_and_reconcile` already
    /// gives minimizing the focused window.
    #[test]
    fn unmapping_the_focused_window_moves_focus_to_the_next_candidate() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        let a = state.window_manager.add_window("a", "a", 1, DEFAULT_GEO);
        let b = state.window_manager.add_window("b", "b", 1, DEFAULT_GEO);
        let key_b = crate::wayland::ToplevelKey::for_test(2);
        state.wayland.bind(b, key_b);
        assert_eq!(state.window_manager.focused_window().map(|w| w.id), Some(b));

        wlr::ToplevelHandler::unmapped(&mut state, key_b.0);

        assert_eq!(state.window_manager.get(b).map(|w| w.mapped), Some(false));
        assert_eq!(
            state.window_manager.focused_window().map(|w| w.id),
            Some(a),
            "focus must fall to the next mapped candidate"
        );
    }

    /// Step 2's focus-refusal decision, exercised through the D-Bus surface:
    /// `Focus` on an unmapped window's id must refuse, not remap-safely
    /// no-op.
    #[test]
    fn handle_command_focus_refuses_an_unmapped_window() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        let a = state.window_manager.add_window("a", "a", 1, DEFAULT_GEO);
        state.window_manager.set_mapped(a, false).unwrap();
        assert_eq!(
            state.handle_command(crate::dbus::DbCommand::Focus(a)),
            None,
            "an unmapped window must never be focused via D-Bus either"
        );
    }

    /// Baseline DBus surface audit (Step 3): `GetState` had no direct
    /// `handle_command` test anywhere in the workspace -- the round trip is
    /// via `reply_tx`, not the model, so nothing else in this file happened
    /// to cover it.
    #[test]
    fn handle_command_get_state_replies_with_a_snapshot() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        state.window_manager.add_window("app", "t", 1, DEFAULT_GEO);
        let (reply_tx, reply_rx) = crossbeam_channel::bounded(1);
        assert_eq!(state.handle_command(crate::dbus::DbCommand::GetState(reply_tx)), Some(()));
        let snapshot = reply_rx.try_recv().expect("GetState must reply synchronously");
        assert_eq!(snapshot.windows.len(), 1);
    }

    /// Baseline DBus surface audit (Step 3): `Quit` likewise had no direct
    /// `handle_command` test -- every existing coverage went through
    /// `apply_action("quit")` or sent the command on a channel a live loop
    /// drains, never `handle_command` itself.
    #[test]
    fn handle_command_quit_sets_the_quitting_flag() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        assert_eq!(state.handle_command(crate::dbus::DbCommand::Quit), Some(()));
        assert!(state.quitting);
    }

    /// Reload audit (Step 3): a config with a new background color lands
    /// through the reload path and `state.config` reflects it. This baseline
    /// already worked before this task (`apply_config` swaps `self.config`
    /// wholesale) -- documented here rather than left unasserted.
    #[test]
    fn reload_applies_appearance_to_live_state() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        let mut cfg = default_config();
        cfg.appearance.palette.background = "#ff0000".into();
        state.apply_reloaded_config(cfg);
        assert_eq!(state.config.appearance.palette.background, "#ff0000");
    }

    /// Reload audit (Step 3): a reloaded config with an unparseable key
    /// takes the same warn-and-skip path load-time validation uses
    /// (`input::warn_about_keybindings`, already called from `apply_config`)
    /// rather than panicking or poisoning the rest of the map.
    #[test]
    fn reload_revalidates_keybindings() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        let mut cfg = default_config();
        cfg.keybindings.insert(
            "broken".into(),
            icedtea_config::KeyCombo { modifiers: vec![], key: "NoSuchKeysym".into() },
        );
        state.apply_reloaded_config(cfg);
        // Surviving proof: the compositor did not panic and a real binding
        // still resolves to a real keysym.
        let combo = &state.config.keybindings["quit"];
        assert!(crate::input::key_name_to_keysym(&combo.key) != 0);
    }

    /// Reload gap-close (Step 3): an `appearance.wallpaper` path change
    /// clears the decoded image so no stale pixels survive under the new
    /// path -- the fresh decode is spawned but this test doesn't wait on it
    /// (the point under test is that the *old* image is gone immediately,
    /// not that the new one has landed yet).
    #[test]
    fn reload_swaps_the_wallpaper_when_the_path_changed() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        state.wallpaper.set_decoded(Some(image::RgbaImage::from_pixel(1, 1, image::Rgba([0, 0, 0, 255]))));
        let mut cfg = default_config();
        cfg.appearance.wallpaper = Some("/nonexistent/new.png".into());
        state.apply_reloaded_config(cfg);
        assert!(state.wallpaper.decoded().is_none(), "stale wallpaper must not survive a path change");
    }

    /// The wallpaper path staying the same across a reload must not tear
    /// down and respawn the decode worker for no reason.
    #[test]
    fn reload_leaves_the_wallpaper_alone_when_the_path_is_unchanged() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(default_config(), tx);
        let image = image::RgbaImage::from_pixel(1, 1, image::Rgba([9, 9, 9, 255]));
        state.wallpaper.set_decoded(Some(image.clone()));
        let cfg = default_config(); // same (absent) wallpaper as boot
        state.apply_reloaded_config(cfg);
        assert_eq!(state.wallpaper.decoded(), Some(&image), "an unrelated reload must not clear the wallpaper");
    }

    // --- Task 17: multi-output placement and hotplug migration ---

    #[test]
    fn placement_targets_the_output_under_the_pointer() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(icedtea_config::default_config(), tx);
        state.create_output(0, Rectangle { x: 0, y: 0, width: 800, height: 600 });
        state.create_output(1, Rectangle { x: 800, y: 0, width: 800, height: 600 });
        // Without a runtime, pointer_position is unavailable: output_for_pointer
        // must fall back to the lowest index deterministically.
        assert_eq!(state.output_for_pointer(), Some(0));
    }

    #[test]
    fn windows_migrate_off_a_removed_output() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(icedtea_config::default_config(), tx);
        state.create_output(0, Rectangle { x: 0, y: 0, width: 800, height: 600 });
        state.create_output(1, Rectangle { x: 800, y: 0, width: 800, height: 600 });
        let id = state.window_manager.add_window("app", "t", 1, Rectangle { x: 900, y: 50, width: 300, height: 200 });
        let dead = state.outputs.remove(&1).expect("output 1").geometry;
        state.migrate_windows_from(dead);
        let w = state.window_manager.get(id).expect("window survives");
        let survivor = state.outputs[&0].geometry;
        assert!(
            w.geometry.x >= survivor.x && w.geometry.x + w.geometry.width <= survivor.x + survivor.width,
            "window must land inside the surviving output, got {:?}",
            w.geometry
        );
    }

    /// Task 8, F: a window too wide for the survivor used to pin flush to
    /// the survivor's origin on that axis (`new_x = survivor.x`), bleeding
    /// all of the overflow off the right/bottom edge. Centering spreads it
    /// symmetrically instead -- negative on both edges rather than zero on
    /// one and everything on the other.
    #[test]
    fn an_oversized_migrated_window_is_centered_not_corner_pinned() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(icedtea_config::default_config(), tx);
        state.create_output(0, Rectangle { x: 0, y: 0, width: 800, height: 600 });
        let id = state.window_manager.add_window("app", "t", 1, Rectangle { x: 900, y: 50, width: 1000, height: 800 });
        let dead = state.outputs.remove(&1).map(|o| o.geometry).unwrap_or(Rectangle { x: 800, y: 0, width: 800, height: 600 });
        state.migrate_windows_from(dead);
        let w = state.window_manager.get(id).expect("window");
        // centered: x = 0 + (800 - 1000)/2 = -100 (symmetric overflow), not pinned to 0.
        assert_eq!(w.geometry.x, (800 - 1000) / 2);
    }

    #[test]
    fn migration_with_no_surviving_output_keeps_geometry() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(icedtea_config::default_config(), tx);
        state.create_output(0, Rectangle { x: 0, y: 0, width: 800, height: 600 });
        let id = state.window_manager.add_window("app", "t", 1, Rectangle { x: 10, y: 10, width: 300, height: 200 });
        let dead = state.outputs.remove(&0).expect("output 0").geometry;
        state.migrate_windows_from(dead); // zero outputs left: must not panic, must not move
        assert_eq!(state.window_manager.get(id).expect("w").geometry.x, 10);
    }

    /// Review fix: `new_toplevel`'s cascade placement used to read
    /// `outputs.values().next()` -- arbitrary `HashMap` iteration order, not
    /// even deterministic -- instead of routing through `output_for_pointer`
    /// like the other three placement consumers. With no runtime attached
    /// (the fallback path), `output_for_pointer` always resolves to the
    /// lowest index, so a two-output state must place a new window inside
    /// output 0's box every time, and the cascade origin itself must come
    /// from that box's own `(x, y)` -- not `(0, 0)` -- so this also pins
    /// down that the placeholder-1920x1080 fallback rect isn't what's
    /// actually feeding `cascade_point_in` here.
    #[test]
    fn new_toplevel_cascades_inside_the_lowest_index_output_without_a_runtime() {
        use crate::wayland::ToplevelKey;

        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(icedtea_config::default_config(), tx);
        state.create_output(0, Rectangle { x: 1000, y: 2000, width: 800, height: 600 });
        state.create_output(1, Rectangle { x: 1800, y: 2000, width: 800, height: 600 });

        let key = ToplevelKey::for_test(1);
        state.new_toplevel(key, "app", "t", 1);
        let id = state.wayland.window_for(key).expect("bound");
        let geo = state.window_manager.get(id).expect("model row").geometry;

        let output0 = state.outputs[&0].geometry;
        assert!(
            geo.x >= output0.x
                && geo.x + geo.width <= output0.x + output0.width
                && geo.y >= output0.y
                && geo.y + geo.height <= output0.y + output0.height,
            "the new window must cascade inside output 0's box, got {geo:?}, output 0 is {output0:?}"
        );
        // The cascade origin for the very first window in a workspace is the
        // output box's own top-left corner (`cascade_point_in` with no
        // occupied rects yet) -- pinning that it's `output0`'s `(x, y)`
        // (1000, 2000), not `(0, 0)`, is what actually distinguishes "used
        // the real box" from "used the (0,0)-origin fallback rect."
        assert_eq!((geo.x, geo.y), (output0.x, output0.y), "cascade origin must be the output box's own (x, y)");
    }

    // --- Task 20: layer-shell arrangement and exclusive zones ---

    #[test]
    fn an_exclusive_top_panel_shrinks_the_usable_area() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(icedtea_config::default_config(), tx);
        state.create_output(0, Rectangle { x: 0, y: 0, width: 800, height: 600 });
        state.layers.insert(
            wlr::LayerSurfaceId::dangling_for_test(),
            LayerEntry {
                output: 0,
                sequence: 0,
                layer: wlr::Layer::Top,
                anchor: wlr::Anchor { top: true, left: true, right: true, bottom: false },
                exclusive: 30,
                size: (800, 30),
                interactive: false,
                mapped: true,
                last_configured: None,
                margin: (0, 0, 0, 0),
            },
        );
        state.arrange_layers();
        let usable = state.outputs[&0].usable;
        assert_eq!(usable, Rectangle { x: 0, y: 30, width: 800, height: 570 });
    }

    /// C1/I5's maximize helper (`maximize_applies_output_geometry_and_restores_it`)
    /// proves maximize tracks `geometry` when there is no panel; this proves
    /// it tracks `usable` once one exists, and that fullscreen -- which
    /// covers panels by definition -- keeps ignoring it.
    #[test]
    fn maximize_respects_the_usable_area_but_fullscreen_ignores_it() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(icedtea_config::default_config(), tx);
        // Snapping/maximize geometry math (`layout::maximized_geometry`)
        // also insets by `appearance.snap_gap`; zero it here so the
        // expected numbers below are the panel's carve alone, not a mix of
        // the carve and an unrelated gap constant.
        state.config.appearance.snap_gap = 0;
        state.create_output(0, Rectangle { x: 0, y: 0, width: 800, height: 600 });
        state.layers.insert(
            wlr::LayerSurfaceId::dangling_for_test(),
            LayerEntry {
                output: 0,
                sequence: 0,
                layer: wlr::Layer::Top,
                anchor: wlr::Anchor { top: true, left: true, right: true, bottom: false },
                exclusive: 30,
                size: (800, 30),
                interactive: false,
                mapped: true,
                last_configured: None,
                margin: (0, 0, 0, 0),
            },
        );
        state.arrange_layers();
        assert_eq!(state.outputs[&0].usable, Rectangle { x: 0, y: 30, width: 800, height: 570 });

        let id = state.window_manager.add_window("app", "t", 1, Rectangle { x: 40, y: 50, width: 300, height: 200 });

        // Maximize via the D-Bus command path (the established pattern):
        // its geometry must equal `usable`, the panel's zone excluded.
        state.handle_command(crate::dbus::DbCommand::Maximize(id, true)).unwrap();
        assert_eq!(
            state.window_manager.get(id).unwrap().geometry,
            state.outputs[&0].usable,
            "maximize must exclude the panel's exclusive zone"
        );

        // Unmaximize, then fullscreen: fullscreen covers panels by
        // definition, so its geometry must equal the *full* output
        // geometry, not `usable`.
        state.handle_command(crate::dbus::DbCommand::Maximize(id, false)).unwrap();
        state.handle_command(crate::dbus::DbCommand::Fullscreen(id, true)).unwrap();
        assert_eq!(
            state.window_manager.get(id).unwrap().geometry,
            state.outputs[&0].geometry,
            "fullscreen must ignore the panel's exclusive zone"
        );
    }

    // --- Task 20 review: J1, J2, J3, M4, M5 ---

    fn top_panel_entry(exclusive: i32, mapped: bool) -> LayerEntry {
        LayerEntry {
            output: 0,
            sequence: 0,
            layer: wlr::Layer::Top,
            anchor: wlr::Anchor { top: true, left: true, right: true, bottom: false },
            exclusive,
            size: (800, exclusive as u32),
            interactive: false,
            mapped,
            last_configured: None,
            margin: (0, 0, 0, 0),
        }
    }

    /// J2: an unmapped entry must not reserve; mapping starts reserving;
    /// unmapping gives the space back. Previously `arrange_layers`' fold had
    /// no mapped check at all, so an unmapped-but-not-yet-destroyed panel
    /// left a permanent hole in the workspace.
    #[test]
    fn an_unmapped_layer_entry_does_not_reserve_until_mapped() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(icedtea_config::default_config(), tx);
        state.create_output(0, Rectangle { x: 0, y: 0, width: 800, height: 600 });
        let id = wlr::LayerSurfaceId::dangling_for_test();
        state.layers.insert(id, top_panel_entry(30, false));

        state.arrange_layers();
        assert_eq!(
            state.outputs[&0].usable,
            Rectangle { x: 0, y: 0, width: 800, height: 600 },
            "an unmapped entry must not reserve"
        );

        state.layers.get_mut(&id).unwrap().mapped = true;
        state.arrange_layers();
        assert_eq!(
            state.outputs[&0].usable,
            Rectangle { x: 0, y: 30, width: 800, height: 570 },
            "mapping must start reserving"
        );

        state.layers.get_mut(&id).unwrap().mapped = false;
        state.arrange_layers();
        assert_eq!(
            state.outputs[&0].usable,
            Rectangle { x: 0, y: 0, width: 800, height: 600 },
            "unmapping must give the space back"
        );
    }

    /// M4: a second `arrange_layers` pass with nothing changed must not
    /// re-emit for a maximized window -- `WindowManager::set_geometry`
    /// emits `WindowUpdated` unconditionally, so without the "does the
    /// target actually differ" guard, a panel redrawing at its own frame
    /// rate produced one signal (and one client configure) per maximized
    /// window, per panel frame, with byte-identical geometry.
    #[test]
    fn arrange_layers_does_not_resync_a_maximized_window_when_the_target_is_unchanged() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut state = State::new(icedtea_config::default_config(), tx);
        state.config.appearance.snap_gap = 0;
        state.create_output(0, Rectangle { x: 0, y: 0, width: 800, height: 600 });
        let panel_id = wlr::LayerSurfaceId::dangling_for_test();
        state.layers.insert(panel_id, top_panel_entry(30, true));

        let id = state.window_manager.add_window("app", "t", 1, Rectangle { x: 0, y: 0, width: 300, height: 200 });
        // Maximized before the panel's zone is folded into `usable` (that
        // only happens inside `arrange_layers`), so the first arrange
        // below genuinely changes the target and must emit.
        state.handle_command(crate::dbus::DbCommand::Maximize(id, true)).unwrap();
        while rx.try_recv().is_ok() {}

        state.arrange_layers();
        assert!(rx.try_recv().is_ok(), "the first arrange changes the target and must emit");
        while rx.try_recv().is_ok() {}

        state.arrange_layers();
        assert!(rx.try_recv().is_err(), "an unchanged maximize target must not re-emit");
    }

    /// J1: `arrange_layers` re-homes each maximized window through its own
    /// frame center, not a single pointer-derived output -- otherwise an
    /// unrelated panel commit on output 1 could teleport a maximized window
    /// that is actually on output 1 onto output 0's `usable` rect merely
    /// because the pointer (with no attached runtime, `output_for_pointer`'s
    /// own fallback: the lowest index) resolved to output 0.
    #[test]
    fn arrange_layers_resyncs_a_maximized_window_against_its_own_output_not_the_pointers() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(icedtea_config::default_config(), tx);
        state.config.appearance.snap_gap = 0;
        state.create_output(0, Rectangle { x: 0, y: 0, width: 800, height: 600 });
        state.create_output(1, Rectangle { x: 800, y: 0, width: 800, height: 600 });

        // Maximized on output 1.
        let id = state.window_manager.add_window("app", "t", 1, Rectangle { x: 810, y: 10, width: 300, height: 200 });
        state.window_manager.set_geometry(id, Rectangle { x: 800, y: 0, width: 800, height: 600 }).unwrap();
        state.window_manager.set_maximized(id, true).unwrap();

        // A panel maps on output 1 -- with no runtime attached,
        // `output_for_pointer`'s fallback is the *lowest* index, output 0,
        // which is exactly the wrong output for this window.
        let panel_id = wlr::LayerSurfaceId::dangling_for_test();
        state.layers.insert(
            panel_id,
            LayerEntry {
                output: 1,
                sequence: 0,
                layer: wlr::Layer::Top,
                anchor: wlr::Anchor { top: true, left: true, right: true, bottom: false },
                exclusive: 30,
                size: (800, 30),
                interactive: false,
                mapped: true,
                last_configured: None,
                margin: (0, 0, 0, 0),
            },
        );
        state.arrange_layers();

        let geo = state.window_manager.get(id).unwrap().geometry;
        assert_eq!(geo, state.outputs[&1].usable, "must track output 1's usable rect, not output 0's");
        assert!(geo.x >= 800, "must not have teleported onto output 0, got {geo:?}");
    }

    /// M5: a layer surface with no output at all when the sweep runs is a
    /// no-op, correctly (left parked for the next call, not dropped); once
    /// an output exists, the sweep re-homes it.
    #[test]
    fn resolve_orphaned_layers_configures_a_layer_surface_parked_with_no_output() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(icedtea_config::default_config(), tx);
        let id = wlr::LayerSurfaceId::dangling_for_test();
        state.layers.insert(id, LayerEntry { output: NO_OUTPUT, ..top_panel_entry(30, false) });

        state.resolve_orphaned_layers();
        assert_eq!(state.layers[&id].output, NO_OUTPUT, "no output exists yet: must stay parked, not vanish");

        state.create_output(0, Rectangle { x: 0, y: 0, width: 800, height: 600 });
        state.resolve_orphaned_layers();
        assert_eq!(state.layers[&id].output, 0, "must be re-homed the moment an output exists");
    }

    /// M5a: an entry orphaned by its output being removed is re-homed onto
    /// a surviving output -- the layer analogue of `migrate_windows_from`.
    #[test]
    fn resolve_orphaned_layers_rehomes_onto_a_surviving_output() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(icedtea_config::default_config(), tx);
        state.create_output(0, Rectangle { x: 0, y: 0, width: 800, height: 600 });
        state.create_output(1, Rectangle { x: 800, y: 0, width: 800, height: 600 });
        let id = wlr::LayerSurfaceId::dangling_for_test();
        state.layers.insert(id, LayerEntry { output: 1, ..top_panel_entry(30, true) });

        // The caller (`OutputHandler::destroyed`) removes the dead output
        // from `self.outputs` before this runs.
        state.outputs.remove(&1);
        state.resolve_orphaned_layers();
        assert_eq!(state.layers[&id].output, 0, "must re-home onto the surviving output");
    }

    /// Task 8, M5: each orphaned entry re-homes to whichever surviving
    /// output its own last-known placement's frame center actually sits
    /// over, not a single survivor picked once for the whole batch (the
    /// old `output_for_pointer` heuristic this replaces) -- a panel
    /// already configured onto the far side of a two-output layout must
    /// land on the output whose box it geometrically overlaps, even when
    /// the lowest surviving index is the *other* one.
    #[test]
    fn resolve_orphaned_layers_rehomes_each_entry_by_its_own_frame_center() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(icedtea_config::default_config(), tx);
        state.create_output(0, Rectangle { x: 0, y: 0, width: 800, height: 600 });
        state.create_output(1, Rectangle { x: 800, y: 0, width: 800, height: 600 });
        // Orphaned by a third, now-dead output; its own last-configured
        // placement sits squarely inside output 1's box, not output 0's
        // (the lowest index, and what a uniform pointer-derived pick would
        // have chosen instead).
        let id = wlr::LayerSurfaceId::dangling_for_test();
        state.layers.insert(
            id,
            LayerEntry { output: 2, last_configured: Some((200, 30, 850, 0)), ..top_panel_entry(30, true) },
        );

        state.resolve_orphaned_layers();
        assert_eq!(state.layers[&id].output, 1, "must land on the output its own frame center overlaps");
    }

    /// M5: with no surviving output at all, the sweep is a deliberate
    /// no-op -- there is nowhere to re-home to, and the entry is left
    /// exactly where it was for the next call (the next `new_output`) to
    /// try again, rather than panicking or being dropped.
    #[test]
    fn resolve_orphaned_layers_is_a_no_op_with_no_surviving_output() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(icedtea_config::default_config(), tx);
        state.create_output(0, Rectangle { x: 0, y: 0, width: 800, height: 600 });
        let id = wlr::LayerSurfaceId::dangling_for_test();
        state.layers.insert(id, top_panel_entry(30, true));

        state.outputs.remove(&0);
        state.resolve_orphaned_layers();
        assert_eq!(state.layers[&id].output, 0, "left exactly where it was; no survivor to re-home onto");
    }

    /// J3's guard, exercised directly: `true` only while `layer_focus`
    /// names an entry that is still mapped. `None`, a dangling id, and an
    /// unmapped entry must all fall through to the model's own focus.
    #[test]
    fn layer_holds_keyboard_focus_only_while_the_entry_is_mapped() {
        let mut layers: HashMap<wlr::LayerSurfaceId, LayerEntry> = HashMap::new();
        let id = wlr::LayerSurfaceId::dangling_for_test();
        assert!(!layer_holds_keyboard_focus(None, &layers), "no layer focus at all");
        assert!(!layer_holds_keyboard_focus(Some(id), &layers), "layer_focus names an entry that does not exist");

        layers.insert(id, top_panel_entry(30, false));
        assert!(!layer_holds_keyboard_focus(Some(id), &layers), "the entry exists but is unmapped");

        layers.get_mut(&id).unwrap().mapped = true;
        assert!(layer_holds_keyboard_focus(Some(id), &layers), "a mapped entry must block the model's own focus");
    }

    /// Finding 6: `resize_edges_from_wlr`'s field-by-field mapping, for
    /// every single edge and every corner (two edges at once) --
    /// `begin_client_resize`'s own success path has no headless way to
    /// exercise (see that method's doc), so this is the pure part of it
    /// that stays directly testable.
    #[test]
    fn resize_edges_from_wlr_maps_every_edge_and_corner() {
        let none = wlr::Edges::default();
        assert_eq!(resize_edges_from_wlr(none), input::ResizeEdges { top: false, bottom: false, left: false, right: false });

        let top = wlr::Edges { top: true, ..none };
        assert_eq!(resize_edges_from_wlr(top), input::ResizeEdges { top: true, bottom: false, left: false, right: false });

        let bottom = wlr::Edges { bottom: true, ..none };
        assert_eq!(resize_edges_from_wlr(bottom), input::ResizeEdges { top: false, bottom: true, left: false, right: false });

        let left = wlr::Edges { left: true, ..none };
        assert_eq!(resize_edges_from_wlr(left), input::ResizeEdges { top: false, bottom: false, left: true, right: false });

        let right = wlr::Edges { right: true, ..none };
        assert_eq!(resize_edges_from_wlr(right), input::ResizeEdges { top: false, bottom: false, left: false, right: true });

        let top_left = wlr::Edges { top: true, left: true, ..none };
        assert_eq!(resize_edges_from_wlr(top_left), input::ResizeEdges { top: true, bottom: false, left: true, right: false });

        let top_right = wlr::Edges { top: true, right: true, ..none };
        assert_eq!(resize_edges_from_wlr(top_right), input::ResizeEdges { top: true, bottom: false, left: false, right: true });

        let bottom_left = wlr::Edges { bottom: true, left: true, ..none };
        assert_eq!(resize_edges_from_wlr(bottom_left), input::ResizeEdges { top: false, bottom: true, left: true, right: false });

        let bottom_right = wlr::Edges { bottom: true, right: true, ..none };
        assert_eq!(resize_edges_from_wlr(bottom_right), input::ResizeEdges { top: false, bottom: true, left: false, right: true });
    }

    /// J3's wiring: with an interactive panel holding `layer_focus` and a
    /// maximized window present (so `arrange_layers` has something to
    /// re-sync -- the exact path that used to reassert toplevel focus over
    /// the panel's), `arrange_layers` must leave `layer_focus` alone;
    /// unmapping the panel is the one path that legitimately clears it.
    ///
    /// This cannot observe the seat's *real* keyboard target end-to-end --
    /// there is no attached `wlr::Runtime` in a unit test, and the wlr crate
    /// exposes no way to create a virtual keyboard device for a headless
    /// test harness to bind `wl_seat.get_keyboard` against (confirmed: the
    /// harness's headless backend advertises no keyboard capability at all,
    /// so a real client's `get_keyboard` is a protocol error). This is
    /// `layer_focus`'s bookkeeping wired through the real handler methods,
    /// paired with `layer_holds_keyboard_focus`'s own direct proof of the
    /// guard's logic above.
    #[test]
    fn arrange_layers_leaves_layer_focus_alone_and_unmap_clears_it() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(icedtea_config::default_config(), tx);
        state.create_output(0, Rectangle { x: 0, y: 0, width: 800, height: 600 });
        let win_id = state.window_manager.add_window("app", "t", 1, Rectangle { x: 0, y: 0, width: 300, height: 200 });
        state.handle_command(crate::dbus::DbCommand::Maximize(win_id, true)).unwrap();

        let panel_id = wlr::LayerSurfaceId::dangling_for_test();
        state.layers.insert(panel_id, LayerEntry { interactive: true, ..top_panel_entry(30, true) });
        // What `layer_surface_mapped` would have set with a live runtime
        // attached (see that handler's own doc for why it cannot here).
        state.layer_focus = Some(panel_id);

        state.arrange_layers();
        assert_eq!(state.layer_focus, Some(panel_id), "arrange_layers must not clear layer_focus");

        wlr::ToplevelHandler::layer_surface_unmapped(&mut state, panel_id);
        assert_eq!(state.layer_focus, None, "unmapping must clear layer_focus");
        assert!(!state.layers[&panel_id].mapped, "unmapping must stop the entry from reserving");
    }

    /// Task 8, N9: a layer surface that becomes keyboard-interactive
    /// *after* it already mapped -- an auto-hide launcher's menu opening,
    /// say -- must still take keyboard focus, even though
    /// `layer_surface_mapped`'s own take-focus branch already ran once (at
    /// map, while the surface was still non-interactive) and will not run
    /// again. Driven via `sync_layer_interactive_focus` directly, the
    /// internal bookkeeping `layer_surface_commit` calls after updating
    /// `entry.interactive` -- `layer_surface_commit` itself takes a live
    /// `&wlr::LayerSurface`, which nothing in this harness can fabricate
    /// (same limitation `arrange_layers_leaves_layer_focus_alone_and_unmap_clears_it`
    /// documents on its own).
    #[test]
    fn a_layer_surface_that_becomes_interactive_after_map_takes_focus() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(icedtea_config::default_config(), tx);
        state.create_output(0, Rectangle { x: 0, y: 0, width: 800, height: 600 });
        let id = wlr::LayerSurfaceId::dangling_for_test();
        // Mapped, not (yet) interactive -- what `layer_surface_mapped` left
        // behind for a surface that mapped before ever asking for the
        // keyboard.
        state.layers.insert(id, top_panel_entry(30, true));
        assert_eq!(state.layer_focus, None, "must not hold focus before the flip");

        // The commit that flips `keyboard_interactive()` to `true`.
        state.layers.get_mut(&id).unwrap().interactive = true;
        state.sync_layer_interactive_focus(id, false, true);

        assert_eq!(state.layer_focus, Some(id), "must take layer focus once it becomes interactive post-map");
    }

    /// Task 8, N9: the release half -- a mapped, focused surface that
    /// flips interactive back to `false` (still mapped) must give the
    /// keyboard back up rather than hold a focus it no longer claims.
    #[test]
    fn a_layer_surface_that_stops_being_interactive_releases_focus() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(icedtea_config::default_config(), tx);
        state.create_output(0, Rectangle { x: 0, y: 0, width: 800, height: 600 });
        let id = wlr::LayerSurfaceId::dangling_for_test();
        state.layers.insert(id, LayerEntry { interactive: true, ..top_panel_entry(30, true) });
        state.layer_focus = Some(id);

        state.layers.get_mut(&id).unwrap().interactive = false;
        state.sync_layer_interactive_focus(id, true, false);

        assert_eq!(state.layer_focus, None, "must release layer focus once it stops being interactive");
    }

    /// Task 8, N9: a surface that is not mapped yet must never take focus
    /// through this path, even if the flag flips -- `focus_layer_keyboard`
    /// refuses an unmapped surface for good reason (see that method's own
    /// doc), and this guard is what keeps the bookkeeping in step with it.
    #[test]
    fn an_unmapped_surface_does_not_take_focus_on_becoming_interactive() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(icedtea_config::default_config(), tx);
        state.create_output(0, Rectangle { x: 0, y: 0, width: 800, height: 600 });
        let id = wlr::LayerSurfaceId::dangling_for_test();
        state.layers.insert(id, top_panel_entry(30, false));

        state.layers.get_mut(&id).unwrap().interactive = true;
        state.sync_layer_interactive_focus(id, false, true);

        assert_eq!(state.layer_focus, None, "an unmapped surface must not take layer focus");
    }

    /// Task 8, N9: something else already holding layer focus must not be
    /// stolen from just because an unrelated surface also flips
    /// interactive on.
    #[test]
    fn an_already_focused_layer_is_not_stolen_from_by_another_turning_interactive() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(icedtea_config::default_config(), tx);
        state.create_output(0, Rectangle { x: 0, y: 0, width: 800, height: 600 });
        let held = wlr::LayerSurfaceId::dangling_for_test();
        state.layers.insert(held, LayerEntry { interactive: true, ..top_panel_entry(30, true) });
        state.layer_focus = Some(held);

        let other = wlr::LayerSurfaceId::dangling_for_test();
        state.layers.insert(other, top_panel_entry(30, true));
        state.layers.get_mut(&other).unwrap().interactive = true;
        state.sync_layer_interactive_focus(other, false, true);

        assert_eq!(state.layer_focus, Some(held), "must not steal focus from an already-focused layer surface");
    }

    /// N6: two top-anchored exclusive panels on the same output must stack
    /// rather than both drawing at the box's own top edge -- the earlier
    /// (lower `sequence`) panel's placement base is the raw output box, the
    /// later one's is the box already shrunk by the earlier panel's own
    /// zone, so it is placed *below* the first rather than on top of it.
    #[test]
    fn usable_before_stacks_two_same_edge_panels_instead_of_overlapping() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(icedtea_config::default_config(), tx);
        state.create_output(0, Rectangle { x: 0, y: 0, width: 800, height: 600 });
        let first = wlr::LayerSurfaceId::dangling_for_test();
        state.layers.insert(first, LayerEntry { sequence: 0, ..top_panel_entry(30, true) });

        // `usable_before` for the first panel (nothing precedes it in
        // sequence order): the raw output box, unshrunk.
        assert_eq!(
            state.usable_before(0, 0).unwrap(),
            Rectangle { x: 0, y: 0, width: 800, height: 600 },
            "the first panel's placement base must be the raw box"
        );

        // `usable_before` for a second panel (sequence 1, after the
        // first): shrunk by the first panel's own 30px zone -- this is
        // what makes `configure_layer` place it flush below the first
        // instead of drawing over it.
        assert_eq!(
            state.usable_before(0, 1).unwrap(),
            Rectangle { x: 0, y: 30, width: 800, height: 570 },
            "the second panel's placement base must exclude the first panel's zone"
        );

        // An unmapped earlier entry must not shrink a later panel's base
        // either -- mirrors J2's "unmapped reserves nothing" for the
        // per-surface placement path, not just `arrange_layers`' own fold.
        state.layers.get_mut(&first).unwrap().mapped = false;
        assert_eq!(
            state.usable_before(0, 1).unwrap(),
            Rectangle { x: 0, y: 0, width: 800, height: 600 },
            "an unmapped earlier entry must not shrink a later panel's placement base"
        );
    }

    /// H2: two panels each requesting a pathologically large
    /// `exclusive_zone` (well past the output's own extent -- what a
    /// hostile or buggy client can ask for) must not panic
    /// (`rect.y += exclusive` overflowing `i32` in a debug build) or wrap
    /// into a corrupted rect in release; `usable` must saturate at a sane,
    /// non-negative box instead.
    #[test]
    fn two_large_exclusive_top_panels_do_not_overflow_or_panic() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(icedtea_config::default_config(), tx);
        state.create_output(0, Rectangle { x: 0, y: 0, width: 800, height: 600 });

        let first = wlr::LayerSurfaceId::dangling_for_test();
        state.layers.insert(first, LayerEntry { sequence: 0, exclusive: i32::MAX, ..top_panel_entry(i32::MAX, true) });
        let second = wlr::LayerSurfaceId::dangling_for_test();
        state.layers.insert(second, LayerEntry { sequence: 1, exclusive: i32::MAX, ..top_panel_entry(i32::MAX, true) });

        state.arrange_layers();

        let usable = state.outputs[&0].usable;
        assert!(usable.height >= 0, "height must never go negative, got {usable:?}");
        assert!(usable.width >= 0, "width must never go negative, got {usable:?}");
        assert!(usable.y >= 0, "y must stay a sane, saturated value, got {usable:?}");
    }

    /// H2: the exclusive zone is also clamped where it is captured --
    /// `new_layer_surface`/`layer_surface_commit` -- not only where it is
    /// folded, so `LayerEntry::exclusive` itself never holds a
    /// pathological value in the first place. `clamp_exclusive_zone`'s
    /// contract directly, since a client-supplied `wlr::LayerSurface` has
    /// no test constructor for `exclusive_zone()`.
    #[test]
    fn clamp_exclusive_zone_bounds_a_positive_value_to_the_outputs_extent() {
        let output = OutputSurface::new(Rectangle { x: 0, y: 0, width: 800, height: 600 });
        assert_eq!(clamp_exclusive_zone(i32::MAX, Some(&output)), 800, "clamped to max(width, height)");
        assert_eq!(clamp_exclusive_zone(30, Some(&output)), 30, "a sane value passes through unchanged");
        assert_eq!(clamp_exclusive_zone(-5, Some(&output)), -5, "a non-positive sentinel is never touched");
        assert_eq!(clamp_exclusive_zone(i32::MAX, None), i32::MAX, "no output resolved yet: left to the saturating fold");
    }

    // --- Task 20 re-review: Important-1, Minor-1 ---

    /// Important-1: click-to-focus is an explicit toplevel-focus assertion
    /// and must win over an interactive panel's held `layer_focus`, unlike
    /// the passive resyncs round 1 (correctly) left alone -- see
    /// `release_layer_focus`'s own doc for the two-clause guard this
    /// completes.
    #[test]
    fn clicking_a_window_releases_layer_focus_for_an_interactive_panel() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(icedtea_config::default_config(), tx);
        state.create_output(0, Rectangle { x: 0, y: 0, width: 800, height: 600 });
        let id = state.window_manager.add_window("app", "t", 1, Rectangle { x: 100, y: 100, width: 600, height: 400 });

        let panel_id = wlr::LayerSurfaceId::dangling_for_test();
        state.layers.insert(panel_id, LayerEntry { interactive: true, ..top_panel_entry(30, true) });
        state.layer_focus = Some(panel_id);

        let _ = state.handle_pointer(PointerEvent::Press { id, pointer: (120, 105) });
        assert_eq!(state.layer_focus, None, "clicking a window must release layer_focus");
        assert!(state.window_manager.get(id).unwrap().focused, "the model's own focus must have won");
    }

    /// Important-1: `DbCommand::Focus` is the D-Bus-driven counterpart of a
    /// click and must release `layer_focus` the same way.
    #[test]
    fn db_command_focus_releases_layer_focus_for_an_interactive_panel() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(icedtea_config::default_config(), tx);
        state.create_output(0, Rectangle { x: 0, y: 0, width: 800, height: 600 });
        let a = state.window_manager.add_window("app", "t", 1, Rectangle { x: 0, y: 0, width: 300, height: 200 });
        let _b = state.window_manager.add_window("app2", "t2", 2, Rectangle { x: 0, y: 0, width: 300, height: 200 });

        let panel_id = wlr::LayerSurfaceId::dangling_for_test();
        state.layers.insert(panel_id, LayerEntry { interactive: true, ..top_panel_entry(30, true) });
        state.layer_focus = Some(panel_id);

        state.handle_command(crate::dbus::DbCommand::Focus(a)).unwrap();
        assert_eq!(state.layer_focus, None, "DbCommand::Focus must release layer_focus");
        assert!(state.window_manager.get(a).unwrap().focused);
    }

    /// Round-2 re-review finding Important-2: `release_layer_focus()` ran
    /// *before* the fallible `window_manager.focus(id)?` in both
    /// `DbCommand::Focus` and `handle_pointer_press`, so a focus request
    /// naming a window that cannot actually be focused (unmapped, or --
    /// `DbCommand::Focus` is reachable straight from a D-Bus caller -- a
    /// stale wire id) still cleared `layer_focus` on its way to the early
    /// return, silently defeating an interactive panel's keyboard grab for
    /// a focus assertion that never happened. `release_layer_focus` now
    /// runs after the `?` at both sites.
    #[test]
    fn a_failing_db_command_focus_does_not_release_layer_focus() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(icedtea_config::default_config(), tx);
        state.create_output(0, Rectangle { x: 0, y: 0, width: 800, height: 600 });
        let a = state.window_manager.add_window("app", "t", 1, Rectangle { x: 0, y: 0, width: 300, height: 200 });
        state.window_manager.set_mapped(a, false).unwrap();

        let panel_id = wlr::LayerSurfaceId::dangling_for_test();
        state.layers.insert(panel_id, LayerEntry { interactive: true, ..top_panel_entry(30, true) });
        state.layer_focus = Some(panel_id);

        assert_eq!(
            state.handle_command(crate::dbus::DbCommand::Focus(a)),
            None,
            "an unmapped window must never be focused via D-Bus"
        );
        assert_eq!(state.layer_focus, Some(panel_id), "a failing focus request must not release layer_focus");
    }

    /// Important-1: alt-tab's per-step focus is the third explicit path
    /// named in the finding.
    #[test]
    fn alt_tab_cycling_releases_layer_focus_for_an_interactive_panel() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(icedtea_config::default_config(), tx);
        state.create_output(0, Rectangle { x: 0, y: 0, width: 800, height: 600 });
        let _a = state.window_manager.add_window("app", "t", 1, Rectangle { x: 0, y: 0, width: 300, height: 200 });
        let _b = state.window_manager.add_window("app2", "t2", 2, Rectangle { x: 0, y: 0, width: 300, height: 200 });

        let panel_id = wlr::LayerSurfaceId::dangling_for_test();
        state.layers.insert(panel_id, LayerEntry { interactive: true, ..top_panel_entry(30, true) });
        state.layer_focus = Some(panel_id);

        state.apply_action("cycle:alt_tab");
        assert_eq!(state.layer_focus, None, "alt-tab cycling must release layer_focus");
    }

    /// Minor-1: `arrange_layers` must reconfigure every mapped panel on
    /// every pass, not only the one whose own commit triggered it --
    /// otherwise a panel whose placement input changed for a reason other
    /// than its own commit (another panel's zone growing or shrinking,
    /// unmapping, or -- as reproduced here -- the output itself resizing)
    /// keeps the stale position it was last configured with until its own
    /// next commit happens to come along.
    ///
    /// `wlr::LayerSurfaceId::dangling_for_test()` is the only synthetic id
    /// this crate exposes, so a unit test cannot hold two distinct live
    /// `LayerEntry`s at once the way a real two-panel scenario would --
    /// see `usable_before_stacks_two_same_edge_panels_instead_of_overlapping`,
    /// which works around the identical limitation the same way. This test
    /// instead reproduces the single-panel case of the same mechanism: an
    /// output resize (`create_output` again -- what `OutputHandler::
    /// new_output` does on a real mode change) moves the panel's placement
    /// input with no layer-side event, real or simulated, in between, and
    /// only `arrange_layers`'s own unconditional reconfigure loop notices.
    #[test]
    fn arrange_layers_reconfigures_every_mapped_panel_even_without_its_own_commit() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(icedtea_config::default_config(), tx);
        state.create_output(0, Rectangle { x: 0, y: 0, width: 800, height: 600 });
        let id = wlr::LayerSurfaceId::dangling_for_test();
        state.layers.insert(id, top_panel_entry(30, true));

        state.arrange_layers();
        let first_placement = state.layers[&id].last_configured;
        assert_eq!(first_placement, Some((800, 30, 0, 0)), "the initial arrange must record a placement");

        // Nothing calls `configure_layer` or `layer_surface_commit` for
        // this entry between the two arranges -- an output mode change
        // (`create_output` again, standing in for `OutputHandler::
        // new_output` re-recording a resized output's box) moves this
        // panel's placement input with no layer-side event of any kind,
        // real or simulated, in between.
        state.create_output(0, Rectangle { x: 0, y: 0, width: 640, height: 480 });
        state.arrange_layers();
        let second_placement = state.layers[&id].last_configured;
        assert_ne!(second_placement, first_placement, "arrange_layers must reconfigure a panel with no commit of its own");
        assert_eq!(second_placement, Some((640, 30, 0, 0)));
    }

    /// Minor-1's other half: an `arrange_layers` pass that changes nothing
    /// about a panel's placement must not touch `last_configured` at all
    /// -- this is the M4-storm-class guard `configure_layer` itself now
    /// provides, proven here at the `arrange_layers` call site rather than
    /// `configure_layer` directly.
    #[test]
    fn arrange_layers_does_not_reconfigure_an_unchanged_panel() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(icedtea_config::default_config(), tx);
        state.create_output(0, Rectangle { x: 0, y: 0, width: 800, height: 600 });
        let id = wlr::LayerSurfaceId::dangling_for_test();
        state.layers.insert(id, top_panel_entry(30, true));

        state.arrange_layers();
        let placement = state.layers[&id].last_configured;

        state.arrange_layers();
        assert_eq!(state.layers[&id].last_configured, placement, "an unchanged panel's placement must not be re-recorded");
    }

    // --- Final review C1: wallpaper / background lowering order ---

    /// C1's regression guard. The scene's actual stacking order is not
    /// observable -- `wlr` exposes no z-query, and `BufferId`/`RectId` have
    /// no synthetic constructor -- so what is pinned is the thing that was
    /// wrong: the *order* of the lower calls `sync_wallpaper_nodes` issues.
    /// `lower_*_to_bottom` gives the bottom to whichever node was lowered
    /// last, so the background rect must come last or it sits on top of the
    /// wallpaper and the wallpaper is never seen (which is exactly what
    /// shipped).
    #[test]
    fn the_background_rect_is_lowered_after_every_wallpaper_node() {
        assert_eq!(
            wallpaper_lower_plan(1, true),
            vec![LowerStep::Wallpaper(0), LowerStep::Background],
            "one output: the background must be lowered last"
        );
        assert_eq!(
            wallpaper_lower_plan(3, true),
            vec![
                LowerStep::Wallpaper(0),
                LowerStep::Wallpaper(1),
                LowerStep::Wallpaper(2),
                LowerStep::Background,
            ],
            "multi-output: still exactly one background lower, still last"
        );
        assert_eq!(
            wallpaper_lower_plan(3, true).last(),
            Some(&LowerStep::Background),
            "the contract in one line: last call wins the bottom, and it must be the background"
        );
    }

    /// The two degenerate inputs. Nothing created means nothing to restack
    /// (existing nodes are already correctly ordered, and re-lowering the
    /// background every idempotent re-sync would be pure churn); no
    /// background rect at all -- every model-only build, and the window
    /// between `State::new` and `set_background` -- still lowers the
    /// wallpaper nodes it made.
    #[test]
    fn the_lower_plan_is_empty_when_nothing_was_created() {
        assert_eq!(wallpaper_lower_plan(0, true), Vec::new());
        assert_eq!(wallpaper_lower_plan(0, false), Vec::new());
        assert_eq!(
            wallpaper_lower_plan(2, false),
            vec![LowerStep::Wallpaper(0), LowerStep::Wallpaper(1)],
            "no background rect to lower, but the new nodes still get lowered"
        );
    }

    // --- Final review I3: per-axis layer placement ---

    /// The panel case, unchanged by I3: `TOP | LEFT | RIGHT`, desired
    /// `(800, 30)` -- both horizontal edges anchored so the width spans the
    /// box, one vertical edge anchored so the height is the client's 30 and
    /// it sits flush against the top.
    #[test]
    fn a_top_anchored_panel_still_spans_the_box_at_its_desired_thickness() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(icedtea_config::default_config(), tx);
        state.create_output(0, Rectangle { x: 0, y: 0, width: 800, height: 600 });
        let id = wlr::LayerSurfaceId::dangling_for_test();
        state.layers.insert(id, top_panel_entry(30, true));
        state.configure_layer(id);
        assert_eq!(state.layers[&id].last_configured, Some((800, 30, 0, 0)));
    }

    /// I3, first half: a corner-anchored notification (`TOP | RIGHT`,
    /// 300x100 -- the shape every notification daemon uses) must get *its
    /// own* width, flush into the top-right corner. The shipped rule keyed
    /// the whole placement on `a.top != a.bottom` and stretched it across
    /// the full 800px output width, discarding `desired_size` outright.
    #[test]
    fn a_corner_anchored_layer_surface_keeps_its_desired_size() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(icedtea_config::default_config(), tx);
        state.create_output(0, Rectangle { x: 0, y: 0, width: 800, height: 600 });
        let id = wlr::LayerSurfaceId::dangling_for_test();
        state.layers.insert(
            id,
            LayerEntry {
                output: 0,
                sequence: 0,
                layer: wlr::Layer::Top,
                anchor: wlr::Anchor { top: true, right: true, left: false, bottom: false },
                exclusive: 0,
                size: (300, 100),
                interactive: false,
                mapped: true,
                last_configured: None,
                margin: (0, 0, 0, 0),
            },
        );
        state.configure_layer(id);
        assert_eq!(
            state.layers[&id].last_configured,
            Some((300, 100, 500, 0)),
            "300x100 flush into the top-right corner (x = 800 - 300), not stretched to 800 wide"
        );
    }

    // --- Task 7: N7 (`compute_layer_placement`), N8 (margin plumbing),
    // N11 (skip-hidden maximized re-sync) ---

    /// N7, via the pure function the brief asks for directly: a corner
    /// anchored panel (`TOP | LEFT`, anchored to exactly one edge on each
    /// axis, neither one spanning) keeps its own desired width rather than
    /// spanning the full output -- `compute_layer_placement` answering the
    /// same as `configure_layer` did, just without a live runtime or a
    /// `last_configured` side effect.
    #[test]
    fn a_corner_anchored_panel_keeps_its_desired_width() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(icedtea_config::default_config(), tx);
        state.create_output(0, Rectangle { x: 0, y: 0, width: 800, height: 600 });
        // top+left anchored (a corner), desired 200x30: width must be 200, not 800.
        state.layers.insert(
            wlr::LayerSurfaceId::dangling_for_test(),
            LayerEntry {
                output: 0,
                sequence: 0,
                layer: wlr::Layer::Top,
                anchor: wlr::Anchor { top: true, left: true, right: false, bottom: false },
                exclusive: 0,
                size: (200, 30),
                interactive: false,
                mapped: true,
                last_configured: None,
                margin: (0, 0, 0, 0),
            },
        );
        let placed = state.compute_layer_placement(wlr::LayerSurfaceId::dangling_for_test());
        assert_eq!(placed.map(|(w, _h, _x, _y)| w), Some(200));
    }

    /// N7/exclusive-edge: tightened `fold_exclusive_zone` against wlroots'
    /// own `wlr_layer_surface_v1_get_exclusive_edge` -- a corner-anchored
    /// panel (`TOP | LEFT`, anchored to exactly one edge on *each* axis,
    /// spanning neither) has no exclusive edge at all per that rule, even
    /// with a positive `exclusive_zone`. The old `anchor.top !=
    /// anchor.bottom` check folded it as a full-width top panel regardless
    /// of `left`/`right`, reserving space the surface never actually spans
    /// edge-to-edge.
    #[test]
    fn a_corner_anchored_exclusive_zone_reserves_nothing() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(icedtea_config::default_config(), tx);
        state.create_output(0, Rectangle { x: 0, y: 0, width: 800, height: 600 });
        state.layers.insert(
            wlr::LayerSurfaceId::dangling_for_test(),
            LayerEntry {
                output: 0,
                sequence: 0,
                layer: wlr::Layer::Top,
                anchor: wlr::Anchor { top: true, left: true, right: false, bottom: false },
                exclusive: 30,
                size: (200, 30),
                interactive: false,
                mapped: true,
                last_configured: None,
                margin: (0, 0, 0, 0),
            },
        );
        state.arrange_layers();
        assert_eq!(
            state.outputs[&0].usable,
            Rectangle { x: 0, y: 0, width: 800, height: 600 },
            "a corner anchor spans neither axis, so wlroots' own rule assigns it no exclusive edge at all"
        );
    }

    /// Review fix (post-2f04984, Major finding): a single-edge-only anchor
    /// -- e.g. `ANCHOR_TOP` alone, no `left`/`right` at all -- is a
    /// distinct, spec-legal shape from both the corner case above and the
    /// edge-plus-both-perpendicular case `top_panel_entry` already covers.
    /// The wlr-layer-shell spec (`set_exclusive_zone`) is explicit that a
    /// positive exclusive zone is meaningful for "one edge" *or* "an edge
    /// and both perpendicular edges" -- both must reserve. WLCS's own
    /// conformance test (`is_positioned_to_accommodate_other_surfaces_
    /// exclusive_zone`) anchors `ANCHOR_TOP` alone with `exclusive_zone =
    /// 12` and asserts the reservation happens. The realignment toward
    /// wlroots' `get_exclusive_edge` in this same commit over-corrected:
    /// every arm of the rewritten `fold_exclusive_zone` required *both*
    /// perpendicular edges, so this single-edge shape fell through every
    /// `if`/`else if` and reserved nothing -- a real, silent regression
    /// (the corner test above and every pre-existing exclusive-zone test
    /// use `TOP | LEFT | RIGHT`, so none of them caught it).
    #[test]
    fn a_single_edge_only_anchor_still_reserves_its_exclusive_zone() {
        let (tx, _rx) = crossbeam_channel::unbounded();

        let mut top = State::new(icedtea_config::default_config(), tx.clone());
        top.create_output(0, Rectangle { x: 0, y: 0, width: 800, height: 600 });
        top.layers.insert(
            wlr::LayerSurfaceId::dangling_for_test(),
            LayerEntry {
                output: 0,
                sequence: 0,
                layer: wlr::Layer::Top,
                anchor: wlr::Anchor { top: true, left: false, right: false, bottom: false },
                exclusive: 12,
                size: (0, 12),
                interactive: false,
                mapped: true,
                last_configured: None,
                margin: (0, 0, 0, 0),
            },
        );
        top.arrange_layers();
        assert_eq!(
            top.outputs[&0].usable,
            Rectangle { x: 0, y: 12, width: 800, height: 588 },
            "ANCHOR_TOP alone with a positive exclusive_zone must reserve at the top (WLCS conformance shape)"
        );

        let mut bottom = State::new(icedtea_config::default_config(), tx.clone());
        bottom.create_output(0, Rectangle { x: 0, y: 0, width: 800, height: 600 });
        bottom.layers.insert(
            wlr::LayerSurfaceId::dangling_for_test(),
            LayerEntry {
                output: 0,
                sequence: 0,
                layer: wlr::Layer::Top,
                anchor: wlr::Anchor { top: false, left: false, right: false, bottom: true },
                exclusive: 12,
                size: (0, 12),
                interactive: false,
                mapped: true,
                last_configured: None,
                margin: (0, 0, 0, 0),
            },
        );
        bottom.arrange_layers();
        assert_eq!(
            bottom.outputs[&0].usable,
            Rectangle { x: 0, y: 0, width: 800, height: 588 },
            "ANCHOR_BOTTOM alone with a positive exclusive_zone must reserve at the bottom"
        );

        let mut left = State::new(icedtea_config::default_config(), tx.clone());
        left.create_output(0, Rectangle { x: 0, y: 0, width: 800, height: 600 });
        left.layers.insert(
            wlr::LayerSurfaceId::dangling_for_test(),
            LayerEntry {
                output: 0,
                sequence: 0,
                layer: wlr::Layer::Top,
                anchor: wlr::Anchor { top: false, left: true, right: false, bottom: false },
                exclusive: 12,
                size: (12, 0),
                interactive: false,
                mapped: true,
                last_configured: None,
                margin: (0, 0, 0, 0),
            },
        );
        left.arrange_layers();
        assert_eq!(
            left.outputs[&0].usable,
            Rectangle { x: 12, y: 0, width: 788, height: 600 },
            "ANCHOR_LEFT alone with a positive exclusive_zone must reserve at the left"
        );

        let mut right = State::new(icedtea_config::default_config(), tx);
        right.create_output(0, Rectangle { x: 0, y: 0, width: 800, height: 600 });
        right.layers.insert(
            wlr::LayerSurfaceId::dangling_for_test(),
            LayerEntry {
                output: 0,
                sequence: 0,
                layer: wlr::Layer::Top,
                anchor: wlr::Anchor { top: false, left: false, right: true, bottom: false },
                exclusive: 12,
                size: (12, 0),
                interactive: false,
                mapped: true,
                last_configured: None,
                margin: (0, 0, 0, 0),
            },
        );
        right.arrange_layers();
        assert_eq!(
            right.outputs[&0].usable,
            Rectangle { x: 0, y: 0, width: 788, height: 600 },
            "ANCHOR_RIGHT alone with a positive exclusive_zone must reserve at the right"
        );
    }

    /// N11: `arrange_layers`' maximized re-sync loop must skip a maximized
    /// window that is minimized, or sitting on a workspace that is not the
    /// active one -- neither is actually on screen, so warping its stored
    /// geometry to whatever `usable` happens to be right now (on an output
    /// the user cannot see) is wrong the moment it is shown again with a
    /// panel layout that has since changed underfoot with no configure of
    /// its own.
    #[test]
    fn arrange_layers_skips_hidden_maximized_windows_in_the_resync() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(icedtea_config::default_config(), tx);
        state.config.appearance.snap_gap = 0;
        state.create_output(0, Rectangle { x: 0, y: 0, width: 800, height: 600 });

        // A maximized, minimized window on the active workspace (0).
        let minimized_id = state.window_manager.add_window("app", "t", 1, Rectangle { x: 0, y: 0, width: 300, height: 200 });
        state.window_manager.set_maximized(minimized_id, true).unwrap();
        state.window_manager.set_minimized(minimized_id, true).unwrap();
        let geo_before_min = state.window_manager.get(minimized_id).unwrap().geometry;

        // A maximized window on workspace 1, while workspace 0 stays active.
        let other_ws_id = state.window_manager.add_window("app2", "t2", 2, Rectangle { x: 0, y: 0, width: 300, height: 200 });
        state.window_manager.set_workspace(other_ws_id, 1).unwrap();
        state.window_manager.set_maximized(other_ws_id, true).unwrap();
        let geo_before_ws = state.window_manager.get(other_ws_id).unwrap().geometry;

        // A panel maps, changing `usable` -- the trigger that would have
        // re-synced every maximized window before N11.
        let panel_id = wlr::LayerSurfaceId::dangling_for_test();
        state.layers.insert(panel_id, top_panel_entry(30, true));
        state.arrange_layers();

        assert_eq!(
            state.window_manager.get(minimized_id).unwrap().geometry,
            geo_before_min,
            "a minimized maximized window must not be re-synced while hidden"
        );
        assert_eq!(
            state.window_manager.get(other_ws_id).unwrap().geometry,
            geo_before_ws,
            "a maximized window on an inactive workspace must not be re-synced while hidden"
        );
    }

    /// I3, second half: a surface anchored to all four edges with a `0x0`
    /// desired size -- the protocol's fill-the-output case, what lockers
    /// and fullscreen launchers use -- must get the whole usable box. The
    /// shipped rule fell through to the `else` arm and handed it a centered
    /// 200x200. The offset origin proves it used the real box rather than
    /// a `(0, 0)` fallback.
    #[test]
    fn a_four_edge_anchored_layer_surface_fills_the_usable_box() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(icedtea_config::default_config(), tx);
        state.create_output(0, Rectangle { x: 100, y: 50, width: 800, height: 600 });
        let id = wlr::LayerSurfaceId::dangling_for_test();
        state.layers.insert(
            id,
            LayerEntry {
                output: 0,
                sequence: 0,
                layer: wlr::Layer::Overlay,
                anchor: wlr::Anchor { top: true, bottom: true, left: true, right: true },
                exclusive: 0,
                size: (0, 0),
                interactive: true,
                mapped: true,
                last_configured: None,
                margin: (0, 0, 0, 0),
            },
        );
        state.configure_layer(id);
        assert_eq!(
            state.layers[&id].last_configured,
            Some((800, 600, 100, 50)),
            "all four anchors with a 0x0 desired size fills the box, not a centered 200x200"
        );
    }

    /// The other fill spelling the protocol allows: *no* anchors and a
    /// `0x0` desired size. Same answer, and the same arm of
    /// `layer_axis_placement` (neither edge, no desired -> span).
    #[test]
    fn an_unanchored_zero_sized_layer_surface_also_fills_the_usable_box() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(icedtea_config::default_config(), tx);
        state.create_output(0, Rectangle { x: 0, y: 0, width: 800, height: 600 });
        let id = wlr::LayerSurfaceId::dangling_for_test();
        state.layers.insert(
            id,
            LayerEntry {
                output: 0,
                sequence: 0,
                layer: wlr::Layer::Overlay,
                anchor: wlr::Anchor { top: false, bottom: false, left: false, right: false },
                exclusive: 0,
                size: (0, 0),
                interactive: true,
                mapped: true,
                last_configured: None,
                margin: (0, 0, 0, 0),
            },
        );
        state.configure_layer(id);
        assert_eq!(state.layers[&id].last_configured, Some((800, 600, 0, 0)));
    }

    /// And an unanchored surface that *did* name a size is still centered
    /// at exactly that size -- the one behavior of the old `else` arm that
    /// was already right, kept.
    #[test]
    fn an_unanchored_sized_layer_surface_is_centered() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(icedtea_config::default_config(), tx);
        state.create_output(0, Rectangle { x: 0, y: 0, width: 800, height: 600 });
        let id = wlr::LayerSurfaceId::dangling_for_test();
        state.layers.insert(
            id,
            LayerEntry {
                output: 0,
                sequence: 0,
                layer: wlr::Layer::Overlay,
                anchor: wlr::Anchor { top: false, bottom: false, left: false, right: false },
                exclusive: 0,
                size: (400, 200),
                interactive: true,
                mapped: true,
                last_configured: None,
                margin: (0, 0, 0, 0),
            },
        );
        state.configure_layer(id);
        assert_eq!(state.layers[&id].last_configured, Some((400, 200, 200, 200)));
    }

    /// Finding 7, security: a client-supplied `desired` size that dwarfs
    /// the output (up to `u32::MAX`) must not wrap negative on the `as
    /// i32` cast -- clamped to the output's own span first, `configure_layer`
    /// must never hand wlroots a negative width/height.
    #[test]
    fn a_pathologically_large_desired_size_is_clamped_to_the_output() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(icedtea_config::default_config(), tx);
        state.create_output(0, Rectangle { x: 0, y: 0, width: 800, height: 600 });
        let id = wlr::LayerSurfaceId::dangling_for_test();
        state.layers.insert(
            id,
            LayerEntry {
                output: 0,
                sequence: 0,
                layer: wlr::Layer::Top,
                anchor: wlr::Anchor { top: false, bottom: false, left: false, right: false },
                exclusive: 0,
                size: (u32::MAX, u32::MAX),
                interactive: true,
                mapped: true,
                last_configured: None,
                margin: (0, 0, 0, 0),
            },
        );
        state.configure_layer(id);
        let (w, h, _, _) = state.layers[&id].last_configured.unwrap();
        assert_eq!(w, 800, "width must clamp to the output's own span, not wrap negative");
        assert_eq!(h, 600, "height must clamp to the output's own span, not wrap negative");
    }

    /// `layer_axis_placement` directly, for the same clamp: `desired` past
    /// `span` is bounded to `span` before the cast, whichever of `start`/
    /// `end` is set.
    #[test]
    fn layer_axis_placement_clamps_desired_to_the_span() {
        assert_eq!(layer_axis_placement(false, false, u32::MAX, 0, 800), (800, 0));
        assert_eq!(layer_axis_placement(true, false, u32::MAX, 0, 800), (800, 0), "flush start, still clamped");
        assert_eq!(layer_axis_placement(false, true, u32::MAX, 0, 800), (800, 0), "flush end, still clamped");
    }

    // --- Final review I1: a remapped layer surface must be reconfigured ---

    /// I1 at the model level (the wire-level half lives in
    /// `client_protocol::a_remapped_layer_panel_is_configured_again`):
    /// `layer_surface_unmapped` must forget `last_configured`, or
    /// `configure_layer`'s storm guard suppresses the mandatory configure
    /// the remap needs -- with an identical placement, which is the normal
    /// case for an auto-hide panel, the guard matches and the client hangs
    /// forever.
    #[test]
    fn unmapping_a_layer_surface_forgets_its_placement_so_a_remap_reconfigures() {
        use wlr::ToplevelHandler;

        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(icedtea_config::default_config(), tx);
        state.create_output(0, Rectangle { x: 0, y: 0, width: 800, height: 600 });
        let id = wlr::LayerSurfaceId::dangling_for_test();
        state.layers.insert(id, top_panel_entry(30, true));

        state.configure_layer(id);
        assert_eq!(state.layers[&id].last_configured, Some((800, 30, 0, 0)));

        state.layer_surface_unmapped(id);
        assert_eq!(
            state.layers[&id].last_configured, None,
            "an unmap must forget the placement -- wlroots resets `initialized`, so the remap needs a fresh configure"
        );

        // The remap: same output, same anchors, so the recomputed placement
        // is byte-identical to the one before the unmap. That is precisely
        // the case the storm guard used to swallow.
        if let Some(entry) = state.layers.get_mut(&id) {
            entry.mapped = true;
        }
        state.configure_layer(id);
        assert_eq!(
            state.layers[&id].last_configured,
            Some((800, 30, 0, 0)),
            "the remap must re-send the identical placement, not be suppressed as unchanged"
        );
    }

    // --- Final review I2: unmapping on an inactive workspace ---

    /// I2. A window focused on workspace 2 unmaps while workspace 1 is
    /// active. Before the fix, `unmapped` re-picked only when the unmapping
    /// window was the *active* workspace's focus, so workspace 2 kept a
    /// `focused_window` pointer aimed at an unmapped row; switching back
    /// re-picked only on `is_none()`, so the pointer survived, the seat went
    /// dead (correctly refusing an invisible window), and `close`/
    /// `maximize`/`fullscreen`/`snap` all still resolved to it.
    #[test]
    fn unmapping_on_an_inactive_workspace_does_not_leave_a_stale_focus_pointer() {
        use wlr::ToplevelHandler;

        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(icedtea_config::default_config(), tx);
        state.create_output(0, Rectangle { x: 0, y: 0, width: 800, height: 600 });

        let key = crate::wayland::ToplevelKey::for_test(1);
        state.new_toplevel(key, "a", "A", 1);
        let id = state.wayland.window_for(key).expect("bound");

        // Focus it on workspace 1 (`move_to_workspace` switches there and
        // focuses it), then leave for workspace 0.
        state.move_to_workspace(id, 1).expect("moved to workspace 1");
        assert_eq!(state.window_manager.focused_window().map(|w| w.id), Some(id));
        state.switch_workspace(0).expect("switched away");

        // The unmap happens while workspace 1 is *not* the active one.
        state.unmapped(wlr::ToplevelId::dangling_nth_for_test(1));

        state.switch_workspace(1).expect("switched back");
        assert!(
            state.window_manager.focused_window().is_none(),
            "workspace 1 must have no focused window: its only candidate is unmapped"
        );
        assert!(
            state.window_manager.get(id).is_some_and(|w| !w.focused),
            "the unmapped row must not still claim focus"
        );
        // The consequence that made the stale pointer actionable rather
        // than merely untidy: a focus-targeted command must find nothing.
        assert!(
            state.apply_action("close").is_none(),
            "a close action must not resolve to the unmapped window"
        );
        assert!(
            state.apply_action("maximize").is_none(),
            "a maximize action must not resolve to the unmapped window"
        );
    }

    /// The other half of I2's fix: `switch_workspace`'s guard widened from
    /// `is_none()` to "no focusable focus", which also covers the
    /// pre-existing minimized case -- switching to a workspace whose focus
    /// pointer names a minimized window now hands focus to a real
    /// candidate instead of leaving the seat dead.
    #[test]
    fn switching_to_a_workspace_whose_focus_is_minimized_repicks() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut state = State::new(icedtea_config::default_config(), tx);
        state.create_output(0, Rectangle { x: 0, y: 0, width: 800, height: 600 });

        let a = state.window_manager.add_window("a", "A", 1, Rectangle { x: 0, y: 0, width: 100, height: 100 });
        let b = state.window_manager.add_window("b", "B", 2, Rectangle { x: 0, y: 0, width: 100, height: 100 });
        // `b` was added second so it is focused; minimize it and leave.
        assert_eq!(state.window_manager.focused_window().map(|w| w.id), Some(b));
        state.window_manager.set_minimized(b, true);
        state.switch_workspace(1).expect("switched away");
        state.switch_workspace(0).expect("switched back");

        assert_eq!(
            state.window_manager.focused_window().map(|w| w.id),
            Some(a),
            "a minimized focus pointer must be re-picked on switch-back, not kept"
        );
    }
}
