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
//!   4 and its default feedback delivers a format table with ≥ 1 tranche
//!   offering ARGB8888 — the format the probe watched a real Tauri app
//!   negotiate.
//! * Tier 2 (GPU-gated, visible SKIP without a render node): a real GBM
//!   buffer imports through `create_params` → `created`, attaches to a
//!   mapped toplevel, and completes its first frame — the Tauri first-frame
//!   scenario end to end.
//! * Negative pin: `linux_drm_syncobj_v1` is NOT advertised, locking in the
//!   leniency decision (no explicit-sync kill path, ever).

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
