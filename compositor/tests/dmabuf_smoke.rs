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

use icedtea_harness::{Compositor, DmabufClient, advertised_globals};

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
    // The surface half of the same contract: the path WebKitGTK binds.
    assert!(
        client.dmabuf_feedback_surface(),
        "per-surface dmabuf feedback never delivered format table + tranches + done"
    );
    assert!(
        !client.tranches().is_empty(),
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
