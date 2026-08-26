//! A2 batch-1 passive-protocol tests: the six globals `d69ff5f` wired up at
//! boot (viewporter, fractional-scale, single-pixel-buffer, content-type,
//! presentation-time, xdg-output), proven with real `wayland-client`s
//! against a real headless compositor -- the same style
//! `compositor/tests/client_protocol.rs` already uses for every other
//! interop protocol in this crate.
//!
//! Three tiers of proof, one per task:
//!
//! * Task 7 -- the cheapest possible claim: the six globals are advertised
//!   at all.
//! * Task 8 -- load-bearing: a client that actually binds `wp_fractional_scale_v1`
//!   receives the right `preferred_scale`, and a client that actually sets a
//!   `wp_viewport` crop/scale sees the scene honor it in a real capture. Both
//!   go well past "the global exists" -- see each test's own doc.
//! * Task 9 -- a client that requests `wp_presentation` feedback on a real
//!   commit receives a terminal event.
//!
//! A2 batch-2 (task 10) continues in the same three tiers, for the three
//! globals `2901de0` wired up (cursor-shape, xdg-activation, gamma-control):
//! the advertisement baseline, then load-bearing proof for each -- a client
//! that owns the pointer repaints the seat cursor and loses it again on
//! leave, both branches of the xdg-activation focus-steal policy, and a real
//! `zwlr_gamma_control_v1` claim answered rather than left hanging.

use icedtea_harness::{Compositor, PresentationOutcome, TestClient};

/// The six A2 batch-1 globals must all be advertised -- the daemon-free
/// baseline every other test in this file assumes.
#[test]
fn a2_batch1_globals_are_advertised() {
    let comp = Compositor::spawn();
    let globals = icedtea_harness::advertised_globals(&comp.socket);
    for iface in [
        "wp_viewporter",
        "wp_fractional_scale_manager_v1",
        "wp_single_pixel_buffer_manager_v1",
        "wp_content_type_manager_v1",
        "wp_presentation",
        "zxdg_output_manager_v1",
    ] {
        assert!(
            globals.iter().any(|g| g == iface),
            "{iface} global missing; saw {globals:?}"
        );
    }
}

/// Task 8 (load-bearing, part 1): with the output at a fractional scale
/// (`1.5`, legal per `State::guarded_scale`), a client that binds
/// `wp_fractional_scale_v1` for its surface must receive a `preferred_scale`
/// event equal to `round(1.5 * 120) = 180` -- the protocol encodes the scale
/// as the numerator of a fraction over a denominator of 120.
///
/// wlroots' scene sends this automatically (`wlr_scene_surface_create`'s own
/// behavior): every scene surface reports its preferred fractional scale from
/// the *live* `wlr_output.scale`, with no explicit compositor call. The
/// compositor therefore does not wire up `wlr::Runtime::notify_fractional_scale`
/// at all -- the one thing it must do is push a scale change onto the real
/// `wlr::Output` so the scene reads the new value: `set_output_scale_for_test`
/// does exactly that via `State`'s `pending_test_output_scale` deferral (the
/// scale is applied to the live output in the next `OutputHandler::frame`, not
/// just mirrored in the model). This test proves the auto-sent `preferred_scale`
/// actually reaches a real client at the right value rather than leaving it an
/// assumption.
#[test]
fn fractional_scale_preferred_scale_matches_output_scale() {
    let comp = Compositor::spawn();
    comp.set_output_scale_for_test(1.5);

    let mut client = TestClient::map_toplevel(&comp.socket, "fractional.app", "fractional");
    assert!(client.wait_until(|c| c.last_configure().is_some()));

    let _fractional_scale = client.get_fractional_scale();
    assert!(
        client.wait_until(|c| c.preferred_scale().is_some()),
        "no preferred_scale event arrived for a client that bound wp_fractional_scale_v1 \
         after its surface already entered the output at scale 1.5"
    );
    assert_eq!(
        client.preferred_scale(),
        Some(180),
        "expected round(1.5 * 120) = 180"
    );
}

/// Task 8 (load-bearing, part 2): a client sets a `wp_viewport` source-crop
/// and destination-size on its surface; the scene must honor both -- render
/// only the cropped sub-rectangle of the buffer, scaled to the destination
/// size -- not merely accept the requests as no-ops.
///
/// Proof shape: paint a buffer at 2x the surface's already-configured
/// content resolution with two large, mutually exclusive solid colors (KEEP
/// on the left half, DISCARD on the right), crop the viewport's source
/// rectangle to select *only* the KEEP half, and scale the destination back
/// down to the surface's original content size. Capture the composited
/// output and assert:
///
/// 1. DISCARD never appears anywhere in the capture -- the strong claim: the
///    cropped-out half of the buffer was genuinely never rendered, not just
///    that the request didn't error.
/// 2. KEEP's on-screen footprint is close to the destination size in pixel
///    count -- the scale was genuinely applied, not left at the buffer's
///    native (2x) resolution.
///
/// Both KEEP and DISCARD are chosen far (per-channel) from the default
/// wallpaper (`#1e1e2e`), the default palette foreground (`#cdd6f4`), and
/// the harness's own plain shm grey (`0x80,0x80,0x80`), so a false positive
/// from an unrelated part of the scene matching by coincidence is not
/// plausible.
#[test]
fn viewporter_crop_and_scale_render_only_the_cropped_region() {
    let comp = Compositor::spawn();
    let mut client = TestClient::map_toplevel(&comp.socket, "viewport.app", "viewport");
    assert!(client.wait_until(|c| c.last_configure().is_some()));

    let (content_w, content_h) = client
        .last_configure()
        .filter(|&(w, h)| w > 0 && h > 0)
        .unwrap_or((200, 100));

    // 0x00RRGGBB, chosen far from wallpaper/decoration/grey on every channel.
    const KEEP: u32 = 0x00E020C0; // R=0xE0 G=0x20 B=0xC0 -- magenta-ish
    const DISCARD: u32 = 0x0020E040; // R=0x20 G=0xE0 B=0x40 -- green-ish
    let buf_w = content_w * 2;
    let buf_h = content_h * 2;
    let half = buf_w / 2;
    client.attach_pattern_buffer(buf_w, buf_h, move |x, _y| if x < half { KEEP } else { DISCARD });

    let viewport = client.get_viewport();
    // Source crop, in buffer coordinates: only the KEEP half.
    viewport.set_source(0.0, 0.0, half as f64, buf_h as f64);
    // Destination: the surface's original (pre-pattern-buffer) content size,
    // so the window's on-screen frame is unchanged and only the crop+scale
    // mechanism is under test -- not a resize.
    viewport.set_destination(content_w, content_h);
    client.commit();

    // Give the compositor a beat to actually composite the new content
    // before capturing -- `wait_until` isn't applicable here (nothing on
    // the client side signals "the compositor rendered your last commit"),
    // so this polls the capture itself until it stops looking like the
    // pre-viewport grey frame.
    let mut sc = icedtea_harness::ScreencopyClient::spawn(&comp.socket);
    let mut frame = sc.capture();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !frame_contains_color(&frame, KEEP) && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(20));
        frame = sc.capture();
    }

    let bpp = match frame.format {
        wayland_client::protocol::wl_shm::Format::Xrgb8888
        | wayland_client::protocol::wl_shm::Format::Argb8888 => 4usize,
        wayland_client::protocol::wl_shm::Format::Bgr888 => 3usize,
        other => panic!("unexpected screencopy shm format {other:?}"),
    };
    // Byte order per `screencopy_of_empty_output_is_the_wallpaper_color`'s
    // own doc in `client_protocol.rs`: Xrgb/Argb are B,G,R,X in memory;
    // Bgr888 is R,G,B despite the name.
    let byte_order: [usize; 3] = match frame.format {
        wayland_client::protocol::wl_shm::Format::Xrgb8888
        | wayland_client::protocol::wl_shm::Format::Argb8888 => [2, 1, 0],
        wayland_client::protocol::wl_shm::Format::Bgr888 => [0, 1, 2],
        _ => unreachable!("matched above"),
    };
    let close = |a: u8, b: u8| (a as i32 - b as i32).abs() <= 8;
    let matches_color = |px: (u8, u8, u8), color: u32| {
        let (r, g, b) = ((color >> 16) as u8, (color >> 8) as u8, color as u8);
        close(px.0, r) && close(px.1, g) && close(px.2, b)
    };

    let mut discard_pixels = 0u32;
    let mut keep_pixels = 0u32;
    for y in 0..frame.height as usize {
        for x in 0..frame.width as usize {
            let o = y * frame.stride as usize + x * bpp;
            let px = (
                frame.bytes[o + byte_order[0]],
                frame.bytes[o + byte_order[1]],
                frame.bytes[o + byte_order[2]],
            );
            if matches_color(px, DISCARD) {
                discard_pixels += 1;
            }
            if matches_color(px, KEEP) {
                keep_pixels += 1;
            }
        }
    }

    assert_eq!(
        discard_pixels, 0,
        "the cropped-out (DISCARD) half of the buffer rendered anyway -- the viewport \
         source-crop was not honored"
    );
    assert!(keep_pixels > 0, "the cropped-in (KEEP) half never rendered at all");

    // The scaled destination footprint should be close to content_w *
    // content_h pixels -- not the buffer's native 2x resolution
    // (content_w*2 * content_h*2), and not some unrelated size. Generous
    // tolerance (40%) absorbs output-scale rounding and any decoration
    // pixels that happen to land inside the tolerance window without
    // weakening the core claim: an unscaled render would be ~4x too big,
    // comfortably outside this band.
    let expected = (content_w as u32) * (content_h as u32);
    let low = expected * 6 / 10;
    let high = expected * 14 / 10;
    assert!(
        keep_pixels >= low && keep_pixels <= high,
        "KEEP footprint {keep_pixels}px not close to the destination size {content_w}x{content_h} \
         ({expected}px, expected in [{low}, {high}]) -- the viewport scale was not honored"
    );
}

fn frame_contains_color(frame: &icedtea_harness::CapturedFrame, color: u32) -> bool {
    let bpp = match frame.format {
        wayland_client::protocol::wl_shm::Format::Xrgb8888
        | wayland_client::protocol::wl_shm::Format::Argb8888 => 4usize,
        wayland_client::protocol::wl_shm::Format::Bgr888 => 3usize,
        _ => return false,
    };
    let byte_order: [usize; 3] = match frame.format {
        wayland_client::protocol::wl_shm::Format::Xrgb8888
        | wayland_client::protocol::wl_shm::Format::Argb8888 => [2, 1, 0],
        wayland_client::protocol::wl_shm::Format::Bgr888 => [0, 1, 2],
        _ => return false,
    };
    let (r, g, b) = ((color >> 16) as u8, (color >> 8) as u8, color as u8);
    let close = |a: u8, b: u8| (a as i32 - b as i32).abs() <= 8;
    for y in 0..frame.height as usize {
        for x in 0..frame.width as usize {
            let o = y * frame.stride as usize + x * bpp;
            if o + 2 >= frame.bytes.len() {
                continue;
            }
            let px = (
                frame.bytes[o + byte_order[0]],
                frame.bytes[o + byte_order[1]],
                frame.bytes[o + byte_order[2]],
            );
            if close(px.0, r) && close(px.1, g) && close(px.2, b) {
                return true;
            }
        }
    }
    false
}

/// Task 9: a client commits a frame with a `wp_presentation` feedback
/// request and receives a terminal feedback event after the output actually
/// commits.
///
/// The headless backend has no real display hardware, so it never emits a
/// page-flip/vblank "present" event for wlroots' presentation-time tracking
/// to time a `presented` event against -- there is no clock to wait on. Per
/// the task brief, this test instead forces the OTHER terminal event the
/// protocol defines: `discarded`, sent when "the associated content update
/// was replaced by a newer one before it was ever displayed" (the XML's own
/// wording). A second, genuinely new content submission on the same surface
/// -- committed before the compositor could ever have displayed the
/// first -- is exactly that supersession, and it is deterministic on any
/// backend, real or headless. The assertion still accepts EITHER terminal
/// event (`matches!` below), so a future/real-hardware backend that manages
/// to race a real `presented` in first passes too -- what this test rules
/// out is neither ever arriving, i.e. a client's feedback object silently
/// dropped, which is the real regression `set_scene_presentation` (Task 6)
/// guards against.
#[test]
fn presentation_feedback_arrives_on_commit() {
    let comp = Compositor::spawn();
    let mut client = TestClient::map_toplevel(&comp.socket, "presentation.app", "presentation");
    assert!(client.wait_until(|c| c.last_configure().is_some()));
    let (w, h) = client.last_configure().filter(|&(w, h)| w > 0 && h > 0).unwrap_or((200, 100));

    // Ties the feedback object to whatever is the surface's current content
    // submission (the map's own initial buffer) -- the plain `commit()`
    // reaffirms it as a fresh double-buffered submission the request can
    // answer against, the same contract `wp_viewport.set_source`/
    // `set_destination` follow.
    let _feedback = client.request_presentation_feedback();
    client.commit();
    // Immediately supersede that submission with a genuinely new one, before
    // the compositor could ever have displayed the first -- see this test's
    // own doc for why that deterministically forces `discarded`.
    client.attach_pattern_buffer(w, h, |_, _| 0x0040_4040);
    client.commit();

    assert!(
        client.wait_until(|c| c.presentation_outcome().is_some()),
        "no wp_presentation_feedback terminal event (presented or discarded) arrived \
         after superseding the tracked content submission"
    );
    let outcome = client.presentation_outcome().unwrap();
    assert!(
        matches!(outcome, PresentationOutcome::Presented | PresentationOutcome::Discarded),
        "unreachable: presentation_outcome() only ever stores one of these two variants"
    );
}

/// The three A2 batch-2 globals must all be advertised -- the baseline the
/// three load-bearing batch-2 tests below assume, and the batch-2 twin of
/// [`a2_batch1_globals_are_advertised`].
#[test]
fn a2_batch2_globals_are_advertised() {
    let comp = Compositor::spawn();
    let globals = icedtea_harness::advertised_globals(&comp.socket);
    for iface in ["wp_cursor_shape_manager_v1", "xdg_activation_v1", "zwlr_gamma_control_manager_v1"] {
        assert!(
            globals.iter().any(|g| g == iface),
            "{iface} global missing; saw {globals:?}"
        );
    }
}

/// Poll [`Compositor::cursor_shape`] until it reads `want`, bounded. Returns
/// the last value seen, so a failed assertion can report what it actually
/// was. A poll rather than a sleep because the request travels client ->
/// compositor thread -> `State`, and nothing on the test's side of the
/// socket signals when that has landed.
fn wait_for_cursor_shape(comp: &Compositor, want: &str) -> String {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let got = comp.cursor_shape();
        if got == want || std::time::Instant::now() >= deadline {
            return got;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// Task 10 (load-bearing): a client that owns the pointer names a cursor
/// image through `cursor-shape-v1` and the seat's cursor actually becomes
/// it -- then reverts to the platform default once the pointer leaves that
/// client's surface.
///
/// Both halves are real claims about `State`, not about the protocol object
/// existing:
///
/// 1. `SeatHandler::request_set_shape` -> `State::apply_cursor_shape` ->
///    `wlr::Runtime::set_cursor_shape`. Deleting that call leaves the
///    request accepted and silently ignored, and this assertion is what
///    notices.
/// 2. The revert-on-leave in `pointer_motion` (see `pointer_over_window`'s
///    field doc): the request carries no seat-client identity, so without
///    it a *background* client's `set_shape` would keep repainting the
///    shared cursor forever. Moving the pointer to genuinely empty desktop
///    -- asserted to be outside every mapped window, not assumed -- is the
///    leave this compositor can observe.
/// 3. The re-assertion in `State::reassert_cursor_shape_after_wlroots_stomp`:
///    wlroots resets the cursor image to `left_ptr` on its own, before
///    every pointer callback reaches this compositor, so a named shape has
///    to be put back on each one or it survives exactly one pointer event.
///    Deleting the re-assert makes the within-window assertion below read
///    `Default`.
///
/// The serial passed to `set_shape` is a real one the seat issued to this
/// client (its `wl_pointer.enter`), read back off the client, not invented.
#[test]
fn cursor_shape_set_by_the_pointer_owner_applies_and_reverts_on_leave() {
    let comp = Compositor::spawn();
    let mut vp = icedtea_harness::VirtualPointerClient::spawn(&comp.socket);

    let mut client = TestClient::map_toplevel(&comp.socket, "cursor.app", "cursor");
    assert!(client.wait_until(|c| c.last_configure().is_some()), "client never configured");
    let info = comp.snapshot().windows.into_iter().next().expect("the mapped window must be in the model");
    // Finding F11: the output's real geometry, straight off the model --
    // not the snap-gap-inset rect a maximized window lands on.
    let (ow, oh) = comp.output_size();

    let geo = comp
        .snapshot()
        .windows
        .iter()
        .find(|w| w.id == info.id)
        .expect("still mapped")
        .geometry;

    // Over the window: the client receives `wl_pointer.enter`, whose serial
    // is what `set_shape` must carry.
    vp.motion_absolute((geo.x + geo.width / 2) as f64, (geo.y + geo.height / 2) as f64, ow as u32, oh as u32);
    vp.frame();
    assert!(client.wait_until(|c| c.last_pointer_serial().is_some()), "client never got a pointer serial");
    let enter_serial = client.last_pointer_serial().expect("just asserted this is Some");

    client.set_cursor_shape(enter_serial, icedtea_harness::CursorShape::Text);
    assert_eq!(
        wait_for_cursor_shape(&comp, "Text"),
        "Text",
        "the seat cursor never became the shape the pointer's own client named"
    );

    // Third claim, and the one a model-mirror-only test would miss: a shape
    // must SURVIVE further pointer motion inside the same window. wlroots
    // resets the cursor image itself -- the `wlr` crate's
    // `Runtime::ensure_cursor_image` calls `wlr_cursor_set_xcursor(cursor,
    // xcursor, "left_ptr")` unconditionally, from the motion,
    // absolute-motion and button callbacks in `backend.rs`, *before* the
    // event reaches the compositor -- so without
    // `State::reassert_cursor_shape_after_wlroots_stomp` the client's shape
    // lasts exactly until the pointer twitches. A few pixels, deliberately
    // still well inside the same window, so this is not the revert-on-leave
    // branch below wearing a disguise.
    let (inner_x, inner_y) = (geo.x + geo.width / 2 + 3, geo.y + geo.height / 2 + 3);
    assert!(
        inner_x >= geo.x && inner_x < geo.x + geo.width && inner_y >= geo.y && inner_y < geo.y + geo.height,
        "({inner_x}, {inner_y}) must still be inside {geo:?} for this to test within-window motion"
    );
    vp.motion_absolute(inner_x as f64, inner_y as f64, ow as u32, oh as u32);
    vp.frame();
    // `settle()` then a SINGLE reading, deliberately not `wait_for_cursor_shape`
    // (see `Compositor::settle`'s own doc): the shape is already "Text" going
    // into this motion, so a poll-until-it-reads-"Text" helper returns on its
    // very first sample and would pass vacuously whenever the command channel
    // beats the compositor's dispatch of the virtual-pointer motion -- with
    // the bug fully present. The motion arrives over the wayland socket while
    // `cursor_shape()` arrives over the command channel, and nothing orders
    // the two, so the reading has to be taken *after* the loop has been given
    // real dispatch cycles.
    comp.settle();
    assert_eq!(
        comp.cursor_shape(),
        "Text",
        "a pointer motion inside the very same window dropped the client's named cursor shape \
         (wlroots reset it to left_ptr and nothing put it back)"
    );

    // Empty desktop: proven empty against the live model rather than
    // assumed, so a future default layout that fills the output cannot turn
    // this half of the test vacuous (it would fail loudly here instead).
    //
    // Finding F11, second half: the point checked here is the point the
    // pointer actually LANDS on, read back from the compositor after the
    // motion, not the one requested. `motion_absolute` maps a coordinate
    // through the output extent and the compositor clamps it to the output,
    // so a requested point and a landed point are not the same thing -- and
    // it is the landed one that has to be off every window for this half of
    // the test to mean anything.
    let (empty_x, empty_y) = (ow - 5, oh - 5);
    vp.motion_absolute(empty_x as f64, empty_y as f64, ow as u32, oh as u32);
    vp.frame();
    comp.settle();
    let (landed_x, landed_y) = comp.cursor_position();
    let (landed_x, landed_y) = (landed_x as i32, landed_y as i32);
    for w in comp.snapshot().windows {
        let g = w.geometry;
        assert!(
            landed_x < g.x
                || landed_x >= g.x + g.width
                || landed_y < g.y
                || landed_y >= g.y + g.height,
            "the pointer landed at ({landed_x}, {landed_y}), which is not empty desktop -- \
             window {:?} covers it",
            w.id
        );
    }
    assert_eq!(
        wait_for_cursor_shape(&comp, "Default"),
        "Default",
        "the seat cursor kept the shape a client named after the pointer left that client's surface"
    );

    client.detach();
}

/// Task 10 (load-bearing): the *refused* half of the xdg-activation policy.
///
/// B mints a token that carries no evidence of a user interaction at all --
/// no `set_serial` (so `ActivationToken::has_seat` is false) and no
/// `set_surface` (so `requesting_toplevel` is `None`) -- and redeems it
/// against its own surface while A holds the keyboard. Per
/// `activation_may_steal_focus` this must not move focus; instead B gets the
/// model's `attention` bit, which the shell surfaces without yanking the
/// keyboard out from under the user.
///
/// Non-vacuous in both directions: A is asserted to still be focused *and*
/// B's attention bit is asserted to have flipped, both through the real
/// `org.icedtea.Compositor` surface (the `WindowUpdated` event first, then
/// the snapshot). A policy that simply dropped the request would pass the
/// focus half and fail the attention half; one that honored everything
/// fails the focus half.
#[test]
fn xdg_activation_without_a_seat_serial_flags_attention_instead_of_stealing_focus() {
    let comp = Compositor::spawn();

    let mut a = TestClient::map_toplevel(&comp.socket, "activation-a.app", "A");
    assert!(a.wait_until(|c| c.last_configure().is_some()), "A never configured");
    let mut b = TestClient::map_toplevel(&comp.socket, "activation-b.app", "B");
    assert!(b.wait_until(|c| c.last_configure().is_some()), "B never configured");

    let (a_id, b_id) = window_ids(&comp);

    // B mapped last, so it holds focus; hand it to A explicitly so the
    // refusal has something to protect.
    comp.send(icedtea_compositor::dbus::DbCommand::Focus(a_id));
    comp.settle();
    assert!(focused_id(&comp) == Some(a_id), "A must hold focus before the activation request");

    let token = b.create_activation_token(None, false);
    b.activate_self(&token);

    comp.wait_event(|e| {
        matches!(e, icedtea_contract::Event::WindowUpdated { id, update }
            if *id == b_id && update.attention == Some(true))
    });

    comp.settle();
    let snapshot = comp.snapshot();
    let a_info = snapshot.windows.iter().find(|w| w.id == a_id).expect("A still mapped");
    let b_info = snapshot.windows.iter().find(|w| w.id == b_id).expect("B still mapped");
    assert!(a_info.focused, "a seat-less activation moved the keyboard off A anyway");
    assert!(!b_info.focused, "a seat-less activation stole focus for B");
    assert!(b_info.attention, "the refused activation left no attention hint on B");

    // Focusing B afterwards clears the hint -- the other half of the
    // contract `set_attention` establishes (the flag is a "look at me until
    // the user does", not a sticky property).
    comp.send(icedtea_compositor::dbus::DbCommand::Focus(b_id));
    comp.wait_event(|e| {
        matches!(e, icedtea_contract::Event::WindowUpdated { id, update }
            if *id == b_id && update.attention == Some(false))
    });
    comp.settle();
    assert!(
        !comp.snapshot().windows.iter().find(|w| w.id == b_id).expect("B still mapped").attention,
        "focusing B did not clear its attention hint"
    );

    a.detach();
    b.detach();
}

/// Task 10 (load-bearing), review finding (low): the refusal isolated to
/// `has_seat`'s *sibling* condition.
///
/// [`xdg_activation_without_a_seat_serial_flags_attention_instead_of_stealing_focus`]
/// refuses a token that fails `activation_may_steal_focus` on **both**
/// `has_seat` and `requester == focused`, so it cannot say which one did the
/// work -- a policy that only checked `has_seat` would pass it unchanged.
/// This test supplies the missing isolation: B mints a *fully seat-backed*
/// token (`set_serial` with a serial the seat genuinely issued to B, plus
/// `set_surface(B)`) while **A** holds the keyboard, so `has_seat` is true
/// and `requester == focused` is the only failing condition. Focus must
/// still not move, and B must still be flagged.
///
/// Together the two tests pin both halves of the conjunction from the
/// outside, through the real protocol, rather than only in the table test.
#[test]
fn a_seat_backed_activation_from_a_non_focused_window_is_still_refused() {
    let comp = Compositor::spawn();
    // A keyboard on the seat, so `wl_keyboard.enter` (and its serial) exists
    // at all -- see the honored test's own note.
    let _vk = icedtea_harness::VirtualKeyboardClient::spawn(&comp.socket);

    let mut a = TestClient::map_toplevel(&comp.socket, "activation-a.app", "A");
    assert!(a.wait_until(|c| c.last_configure().is_some()), "A never configured");
    let mut b = TestClient::map_toplevel(&comp.socket, "activation-b.app", "B");
    assert!(b.wait_until(|c| c.last_configure().is_some()), "B never configured");

    let (a_id, b_id) = window_ids(&comp);

    // B mapped last, so it holds focus and is handed a real seat serial. The
    // token is minted here, while B is still the focused window, so it is
    // every bit as well-formed as the one the honored test mints -- wlroots
    // validates `set_serial`'s serial against the seat client that was given
    // it, and a token minted from a client the seat has since left is not a
    // token this test could rely on being accepted at all.
    assert!(b.wait_until(|c| c.has_input_serial()), "B never received wl_keyboard.enter");
    let b_serial = b.last_input_serial().expect("just asserted this is Some");
    let token = b.create_activation_token(Some(b_serial), true);

    // Now hand the keyboard to A. The token stays perfectly valid; what
    // changes is the only condition this test means to isolate --
    // `requester == focused` is now false while `has_seat` is still true.
    // Redeeming a token after focus moved on is also exactly the real
    // scenario the policy exists for.
    comp.send(icedtea_compositor::dbus::DbCommand::Focus(a_id));
    assert!(wait_for_focus(&comp, a_id), "A must hold focus before the refusal has anything to protect");

    b.activate_self(&token);

    comp.wait_event(|e| {
        matches!(e, icedtea_contract::Event::WindowUpdated { id, update }
            if *id == b_id && update.attention == Some(true))
    });

    comp.settle();
    let snapshot = comp.snapshot();
    let a_info = snapshot.windows.iter().find(|w| w.id == a_id).expect("A still mapped");
    let b_info = snapshot.windows.iter().find(|w| w.id == b_id).expect("B still mapped");
    assert!(
        a_info.focused,
        "a seat-backed activation from a NON-focused client moved the keyboard off A -- \
         the policy is checking has_seat alone, not requester == focused"
    );
    assert!(!b_info.focused, "a seat-backed activation from a non-focused client stole focus for B");
    assert!(b_info.attention, "the refused activation left no attention hint on B");

    a.detach();
    b.detach();
}

/// Task 10 (load-bearing): the *honored* half of the xdg-activation policy.
///
/// A holds the keyboard and mints a token the proper way -- `set_serial`
/// with a serial the seat genuinely issued to A (its `wl_keyboard.enter`,
/// which is what makes wlroots record a seat on the token) plus
/// `set_surface(A)` -- then hands the opaque token string to B, which
/// redeems it against its own surface. That is exactly
/// `activation_may_steal_focus`'s intended case (the app you are using
/// handing you off to another window), so focus must actually move.
///
/// The token string crosses between the two clients by value because that is
/// the only way it *can*: a `wl_surface` is a per-connection object, so no
/// client can name another's, and the protocol is designed around the
/// requester minting and the target redeeming.
///
/// A `VirtualKeyboardClient` exists only to give the headless seat a
/// keyboard capability at all -- without it no client is ever sent
/// `wl_keyboard.enter` and there is no seat-issued serial to pass.
#[test]
fn xdg_activation_from_the_focused_window_moves_focus() {
    let comp = Compositor::spawn();
    // Spawned before either client connects so the seat already advertises
    // the keyboard capability when they bind it -- a client only calls
    // `get_keyboard` on the capability it saw.
    let _vk = icedtea_harness::VirtualKeyboardClient::spawn(&comp.socket);

    let mut a = TestClient::map_toplevel(&comp.socket, "activation-a.app", "A");
    assert!(a.wait_until(|c| c.last_configure().is_some()), "A never configured");
    let mut b = TestClient::map_toplevel(&comp.socket, "activation-b.app", "B");
    assert!(b.wait_until(|c| c.last_configure().is_some()), "B never configured");

    let (a_id, b_id) = window_ids(&comp);

    // Cleared first (review finding, low): B mapped last and took focus, so
    // A already holds a serial from its own map-time `wl_keyboard.enter`.
    // Waiting on `has_input_serial()` without clearing passes instantly on
    // that stale serial and asserts nothing about the `Focus(a_id)` below --
    // the token would then be minted with a serial from before A was
    // re-focused. This makes it a real bounded wait for the enter that the
    // refocus actually produces.
    a.clear_input_serial();
    comp.send(icedtea_compositor::dbus::DbCommand::Focus(a_id));
    assert!(a.wait_until(|c| c.has_input_serial()), "A never received wl_keyboard.enter after refocus");
    assert!(focused_id(&comp) == Some(a_id), "A must hold focus before minting the token");
    let serial = a.last_input_serial().expect("just asserted this is Some");

    let token = a.create_activation_token(Some(serial), true);
    b.activate_self(&token);

    // A bounded poll of the live model rather than `wait_event`: every
    // `WindowUpdated { focused: Some(true) }` this test could match on has
    // already been emitted once by B's own map and again by the explicit
    // `Focus(a_id)` above, so an event-stream match would pass on a stale
    // one and say nothing about the activation.
    assert!(
        wait_for_focus(&comp, b_id),
        "an activation from the focused window did not move focus to its target"
    );

    comp.settle();
    let snapshot = comp.snapshot();
    let a_info = snapshot.windows.iter().find(|w| w.id == a_id).expect("A still mapped");
    let b_info = snapshot.windows.iter().find(|w| w.id == b_id).expect("B still mapped");
    assert!(b_info.focused, "an activation from the focused window did not move focus to its target");
    assert!(!a_info.focused, "focus must have left A");
    assert!(
        !b_info.attention,
        "an honored activation must move focus, not fall back to the attention hint"
    );

    a.detach();
    b.detach();
}

/// The `(A, B)` window ids of the two-client activation tests, resolved by
/// `app_id` off the live model rather than by index (the snapshot's order is
/// the stacking/MRU order, which the very thing under test changes).
fn window_ids(comp: &Compositor) -> (icedtea_contract::WindowId, icedtea_contract::WindowId) {
    let snapshot = comp.snapshot();
    let find = |app_id: &str| {
        snapshot
            .windows
            .iter()
            .find(|w| w.app_id == app_id)
            .unwrap_or_else(|| panic!("{app_id} must be in the model once mapped"))
            .id
    };
    (find("activation-a.app"), find("activation-b.app"))
}

/// Poll the live model until `id` holds focus, bounded. Returns whether it
/// ever did.
fn wait_for_focus(comp: &Compositor, id: icedtea_contract::WindowId) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if focused_id(comp) == Some(id) {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// The first window in the snapshot carrying `focused`, if any.
///
/// Deliberately workspace-blind: `WindowInfo.focused` is per-*workspace*
/// (each workspace keeps its own focus pointer), so with windows on more
/// than one workspace this can return a window that does not hold the
/// keyboard. Every caller in this file keeps all its windows on the single
/// default workspace, where "focused" and "holds the keyboard" coincide.
/// A test that spreads windows across workspaces must compare against
/// `Snapshot::active_workspace` itself rather than reuse this.
fn focused_id(comp: &Compositor) -> Option<icedtea_contract::WindowId> {
    comp.snapshot().windows.iter().find(|w| w.focused).map(|w| w.id)
}

/// Task 10: a `zwlr_gamma_control_manager_v1` client claims gamma control of
/// the compositor's output and gets a protocol-conformant answer -- not a
/// hang, and not silence.
///
/// The manager is scene-integrated
/// (`wlr_scene_set_gamma_control_manager_v1`), so the compositor itself
/// never answers this request: wlroots does, off the output's own gamma LUT
/// size. The headless backend has no CRTC and therefore no LUT, so its
/// `gamma_size` is 0 and wlroots' `zwlr_gamma_control_v1` sends `failed`
/// instead of a size -- the documented behavior for an output it cannot
/// hand out gamma control for.
///
/// The assertion accepts either protocol-legal outcome and pins the shape of
/// each, rather than only the one this backend happens to take: `failed`, or
/// a real `gamma_size` followed by an accepted identity ramp of exactly that
/// size that does *not* then fail. What it rules out is the regression that
/// matters -- neither event ever arriving, i.e. the manager never created
/// and the client left waiting forever.
#[test]
fn gamma_control_answers_a_claim_on_the_headless_output() {
    let comp = Compositor::spawn();
    let mut gamma = icedtea_harness::GammaControlClient::spawn(&comp.socket);

    assert!(
        gamma.wait_until(|g| g.failed() || g.gamma_size().is_some()),
        "zwlr_gamma_control_v1 sent neither gamma_size nor failed -- the client would hang"
    );

    match gamma.gamma_size() {
        None => assert!(gamma.failed(), "unreachable: the wait above requires one or the other"),
        Some(0) => panic!(
            "gamma_size 0 is not a legal answer -- wlroots sends `failed` for an output with no LUT"
        ),
        // Unreachable on the headless backend, and kept deliberately: this
        // arm is what makes the assertion a claim about the *protocol*
        // rather than about this backend's lack of a CRTC, and it is the arm
        // that runs the moment anyone points this test at a DRM backend. No
        // `#[allow]` is needed -- a `match` arm that happens not to be taken
        // at runtime is not dead code to the compiler.
        Some(size) => {
            // Not this backend's path (see the doc above), but a real one on
            // a DRM backend: a control that reported a size must accept a
            // ramp of exactly that size and stay alive afterwards.
            gamma.set_identity_gamma(size);
            // Past the output commit that would apply the ramp: `failed` is
            // what a rejection looks like, and it would be on the wire by
            // the time these round trips are done.
            comp.settle();
            gamma.pump();
            assert!(
                !gamma.failed(),
                "the compositor rejected an identity gamma ramp of the size it asked for ({size})"
            );
        }
    }
}
