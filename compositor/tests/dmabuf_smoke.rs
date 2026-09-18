//! Dmabuf regression smoke tests: the Tauri v2 / WebKitGTK contract a
//! compositor must honor, proven with a real `wayland-client` speaking real
//! `zwp_linux_dmabuf_v1` to a real headless compositor over a real socket.
//!
//! The failure modes this guards are the ones Tauri documents on other DEs
//! (blank windows, resize flicker/crash, `Error 71` kills,
//! `AcceleratedSurfaceDMABuf was unable to construct a complete
//! framebuffer`): a client that cannot negotiate a usable format/modifier
//! from our feedback, or that gets killed for a missing acquire point, has
//! nowhere to go. Three tiers:
//!
//! * Tier 1 (always runs): `zwp_linux_dmabuf_v1` is advertised at version ≥
//!   4 and both the default feedback and the per-surface feedback deliver a
//!   format table with ≥ 1 tranche offering ARGB8888 — the format the probe
//!   watched a real Tauri app negotiate. The surface half matters: WebKitGTK
//!   binds the per-surface path, whose tranches the compositor may tailor,
//!   so the default offer alone does not prove what the surface gets.
//! * Tier 2 (GPU-gated, visible SKIP without a render node): a real GBM
//!   buffer imports through `create_params` → `created`, attaches to a
//!   mapped toplevel, and completes its first frame — the Tauri first-frame
//!   scenario end to end. Reject ≠ skip: a compositor `Failed` panics (a
//!   rejecting compositor is a contract violation), as do transport errors;
//!   ONLY environment absence (no render node, GBM/BO/fd failure, no ARGB
//!   tranche) returns `None`, so the test below stays `let Some else return`.
//!   A dedicated rejection-shape test would need a second compositor
//!   configuration that rejects on purpose; the structural split (reject≠
//!   skip) plus this tier IS the coverage.
//! * Negative pin: `linux_drm_syncobj_v1` is NOT advertised, locking in the
//!   leniency decision (no explicit-sync kill path, ever).
//!
//! Documented-uncovered arms (no deficient-compositor stubs, on purpose):
//! `wait_feedback` returning `false` (spawn always requests the default
//! feedback and a working server always answers it in practice — faking a
//! deficient compositor to watch it fail would prove nothing about the
//! client), and `DmabufClient::spawn`'s panic branches (each `expect` names
//! a global a working compositor always advertises; the strings are reviewed
//! in code).

use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use icedtea_harness::{Compositor, DmabufClient, advertised_globals, has_render_node};
use wayland_client::protocol::{wl_buffer, wl_registry};
use wayland_client::{
    Connection, Dispatch, EventQueue, QueueHandle, delegate_noop, event_created_child,
};
use wayland_protocols::wp::linux_dmabuf::zv1::client::{
    zwp_linux_buffer_params_v1, zwp_linux_dmabuf_v1,
};

#[test]
fn dmabuf_feedback_v4_with_usable_tranches() {
    let comp = Compositor::spawn();
    let mut client = DmabufClient::spawn(&comp.socket);
    assert!(
        client.version() >= 4,
        "linux_dmabuf advertised below v4: no per-surface feedback for WebKitGTK"
    );
    assert!(
        client.wait_feedback(),
        "default dmabuf feedback never delivered format table + tranches + done"
    );
    assert!(
        !client.tranches().is_empty(),
        "dmabuf feedback delivered no tranches to choose from"
    );
    assert!(
        client.has_argb8888(),
        "dmabuf feedback offers no ARGB8888 tranche: the Tauri format is missing"
    );
    // The surface half of the same contract: the path WebKitGTK binds. Full
    // replace-vs-union proof (that the surface collection drops the default
    // offer rather than appending to it) lives in the harness unit test
    // `dmabuf_feedback_clear_drops_prior_rows`; here we only assert the
    // surface feedback itself delivered a usable set. Deliberately no
    // row-count parity with the default offer: a compositor may tailor
    // per-surface tranches (see this file's header), so equal counts are
    // neither required nor a contract.
    assert!(
        client.dmabuf_feedback_surface(),
        "per-surface dmabuf feedback never delivered format table + tranches + done \
         (or the transport died before it could)"
    );
    let surface_rows = client.tranches();
    assert!(
        !surface_rows.is_empty(),
        "surface dmabuf feedback delivered no tranches to choose from"
    );
    assert!(
        client.has_argb8888(),
        "surface dmabuf feedback offers no ARGB8888 tranche: the Tauri format is missing"
    );
}

#[test]
fn no_explicit_sync_manager_advertised() {
    let comp = Compositor::spawn();
    let globals = advertised_globals(&comp.socket);
    assert!(
        globals.iter().any(|g| g == "zwp_linux_dmabuf_v1"),
        "linux_dmabuf itself went missing"
    );
    assert!(
        !globals.iter().any(|g| g.contains("syncobj")),
        "a syncobj manager is advertised: clients may engage explicit sync \
         and die on missing acquire points (Error 71 class)"
    );
}

#[test]
fn dmabuf_import_completes_a_first_frame() {
    let comp = Compositor::spawn();
    let mut client = DmabufClient::spawn(&comp.socket);
    assert!(
        client.wait_feedback(),
        "no dmabuf feedback to choose an import format from"
    );
    let Some(()) = client.import_and_present() else {
        eprintln!("SKIP: no DRM render node for GBM allocation — import path unproven here");
        return;
    };
    assert!(
        client.wait_first_frame(),
        "dmabuf buffer attached and committed but the first frame never completed"
    );
}

/// A client that never presents must time out `false` from
/// `wait_first_frame` — the negative arm of the Tier 2 wait. Costs the full
/// ~5s TIMEOUT by design (there is no early signal for "no frame is
/// coming"); that is why this is its own test rather than folded into the
/// import test above.
#[test]
fn first_frame_without_present_times_out_false() {
    let comp = Compositor::spawn();
    let mut client = DmabufClient::spawn(&comp.socket);
    assert!(
        !client.wait_first_frame(),
        "wait_first_frame returned true with no present ever committed"
    );
}

/// Minimal registry state for the garbage-modifier test: just the dmabuf
/// global plus the `Failed`/`Created` outcome of one raw `create_params`.
#[derive(Default)]
struct GarbageState {
    dmabuf: Option<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1>,
    failed: bool,
    created: bool,
}

impl Dispatch<wl_registry::WlRegistry, ()> for GarbageState {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
            && interface == "zwp_linux_dmabuf_v1"
        {
            state.dmabuf = Some(registry.bind(name, version.min(5), qh, ()));
        }
    }
}

impl Dispatch<zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1, ()> for GarbageState {
    event_created_child!(GarbageState, zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1, [
        zwp_linux_buffer_params_v1::EVT_CREATED_OPCODE => (wl_buffer::WlBuffer, ()),
    ]);

    fn event(
        state: &mut Self,
        _: &zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1,
        event: zwp_linux_buffer_params_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwp_linux_buffer_params_v1::Event::Created { .. } => {
                state.created = true;
            }
            zwp_linux_buffer_params_v1::Event::Failed => {
                state.failed = true;
            }
            _ => {}
        }
    }
}

delegate_noop!(GarbageState: ignore wl_buffer::WlBuffer);
delegate_noop!(GarbageState: ignore zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1);

/// Garbage-modifier rejection shape: a raw `create_params` naming a
/// modifier no driver offers must end in
/// `zwp_linux_buffer_params_v1.failed`, never `created`.
///
/// Spoken directly on a fresh `wayland-client` connection — no harness
/// changes; the harness only ever imports *valid* GBM buffers — mirroring
/// the registry dance the harness's own connect performs. This pins both
/// halves at once: the compositor contract (garbage in → `Failed`, not a
/// kill and not a buffer) and the harness's own failed-event decode path
/// (the arm that records `dmabuf_failed` fires on exactly this event).
///
/// The client-side panic arm itself (`import_and_present` panicking on
/// `Failed`) is deliberately NOT executed here: that would need a
/// compositor that rejects a *valid* import — a deliberately deficient
/// server, i.e. stub-compositor infrastructure out of scope. It is covered
/// by inspection plus the Tier 2 pass above: a live `Failed` on the real
/// import would panic, so a green Tier 2 proves the reject≠skip split holds.
#[test]
fn dmabuf_garbage_modifier_is_rejected_with_failed() {
    use std::os::fd::AsFd as _;

    let comp = Compositor::spawn();
    let stream =
        UnixStream::connect(comp.socket_path()).expect("connect to the harness compositor");
    let conn = Connection::from_socket(stream).expect("wayland connection");
    let mut queue: EventQueue<GarbageState> = conn.new_event_queue();
    let qh = queue.handle();
    let mut state = GarbageState::default();
    let _registry = conn.display().get_registry(&qh, ());
    queue.roundtrip(&mut state).expect("registry roundtrip");
    queue.roundtrip(&mut state).expect("bind roundtrip");
    let dmabuf = state
        .dmabuf
        .clone()
        .expect("compositor did not advertise zwp_linux_dmabuf_v1");

    // The fd must back a plausible allocation: wlroots stats it and kills
    // the client with a protocol error when `offset + stride * height`
    // overruns its size, so /dev/null (size 0) can never reach the modifier
    // check. A sized temp file sails through the bounds checks and dies
    // exactly where it should: 0xDEADBEEF_CAFEF00D is offered by no driver
    // on any planet, so the import fails with `Failed`.
    //
    // `NamedTempFile`, not a `temp_dir()`-joined pid name: the name is
    // random and created with `O_EXCL` (no symlink-clobber), and RAII
    // unlinks it on both the normal and panic paths. It must outlive the
    // import (the fd has to stay valid until the outcome arrives), so it is
    // held to the end of the test.
    let backing = tempfile::NamedTempFile::new().expect("create dmabuf backing file");
    backing
        .as_file()
        .set_len(65536)
        .expect("size dmabuf backing file");
    let garbage: u64 = 0xDEAD_BEEF_CAFE_F00D;
    let params = dmabuf.create_params(&qh, ());
    params.add(
        backing.as_file().as_fd(),
        0,
        0,
        256,
        (garbage >> 32) as u32,
        garbage as u32,
    );
    params.create(
        64,
        64,
        0x3432_5241, // DRM_FORMAT_ARGB8888
        zwp_linux_buffer_params_v1::Flags::empty(),
    );
    conn.flush().expect("flush garbage create_params");

    let deadline = Instant::now() + Duration::from_secs(5);
    while !state.failed && !state.created {
        assert!(
            Instant::now() < deadline,
            "garbage-modifier create_params neither failed nor created within 5s"
        );
        if queue.roundtrip(&mut state).is_err() {
            break;
        }
    }
    assert!(
        !state.created,
        "compositor CREATED a buffer for garbage modifier 0x{garbage:016x}: expected Failed"
    );
    assert!(
        state.failed,
        "compositor neither failed nor created garbage-modifier import 0x{garbage:016x} within 5s \
         (a protocol-error kill would also land here, as a roundtrip error)"
    );
    // `backing` (a `NamedTempFile`) unlinks itself on drop, here and on the
    // panic path; nothing to clean up by hand.
}

/// Extract the string message from a `catch_unwind` payload.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic payload>".to_string()
    }
}

/// Transport death mid-import panics, never a silent SKIP: with the
/// compositor gone the import's transport error is a panic. GPU-gated —
/// `import_and_present` returns `None` at its render-node/GBM guards long
/// before any flush, so on a host with no render node the path is simply
/// unprovable and the test SKIPs visibly rather than failing.
///
/// `drop` joins the compositor thread, so the socket is fully closed before
/// the import starts — no kill-timing race. Depending on how the FIN is
/// handled the panic can land at the `create_params` flush (the socket is
/// already tearing down) or at the roundtrip that follows (a graceful FIN
/// lets the first flush succeed), so the assertion matches the shared
/// `"dmabuf create_params"` prefix, not one exact site.
#[test]
fn dmabuf_import_panics_when_transport_dies() {
    if !has_render_node() {
        eprintln!("SKIP: no renderD node — transport-death import path unprovable here");
        return;
    }
    let comp = Compositor::spawn();
    let mut client = DmabufClient::spawn(&comp.socket);
    assert!(
        client.wait_feedback(),
        "no dmabuf feedback to choose an import format from"
    );
    drop(comp);
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = client.import_and_present();
    }));
    match outcome {
        Ok(_) => panic!(
            "import_and_present returned on a dead transport instead of panicking; \
             the render-node/GBM guards must not have been the stop"
        ),
        Err(payload) => {
            let message = panic_message(&*payload);
            assert!(
                message.contains("dmabuf create_params"),
                "transport death must panic at the create_params flush or the following \
                 roundtrip, got: {message}"
            );
        }
    }
}

/// Surface feedback against a dead connection returns `false` — the flush /
/// `pump_until` roundtrip-error path — never panics. A `false` there names
/// either cause (`dmabuf_feedback_surface`'s doc): feedback never completed,
/// or the transport is gone; here it is deterministically the latter. `drop`
/// joins the compositor thread, so the socket is fully closed before the
/// request goes out; no timing dependence.
#[test]
fn dmabuf_surface_feedback_false_on_dead_connection() {
    let comp = Compositor::spawn();
    let mut client = DmabufClient::spawn(&comp.socket);
    drop(comp);
    assert!(
        !client.dmabuf_feedback_surface(),
        "surface feedback against a dead compositor returned true"
    );
}

/// A dead connection fails `wait_first_frame` with `false`, and the stop is
/// a recorded transport error, not a silent full-`TIMEOUT` spin: the
/// roundtrip errors immediately and `pump_until` latches the error kind.
/// Asserting the latched kind is deterministic — unlike a wall-clock bound,
/// which a loaded CI runner can flake.
#[test]
fn dmabuf_wait_first_frame_false_when_transport_dead() {
    let comp = Compositor::spawn();
    let mut client = DmabufClient::spawn(&comp.socket);
    drop(comp);
    let completed = client.wait_first_frame();
    assert!(
        !completed,
        "wait_first_frame against a dead compositor returned true"
    );
    assert!(
        client.last_pump_errored(),
        "the wait must stop on a recorded transport error; an error-free false would mean \
         the roundtrip never failed and the wait merely spun to TIMEOUT"
    );
}
