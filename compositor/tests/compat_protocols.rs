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
/// wlroots' scene documents this as automatic (`wlr_scene_surface_create`'s
/// own behavior -- see the `wlr::Runtime::notify_fractional_scale` doc in
/// the `wlr` crate this compositor is built on): every scene surface reports
/// its preferred fractional scale without the compositor calling anything.
/// This test is what actually proves that reaches a real client rather than
/// leaving it an assumption -- and it does: no `runtime.notify_fractional_scale`
/// call was needed in `compositor/src/state.rs` to make this pass. See the
/// task-7-9 report for the record of that finding.
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
