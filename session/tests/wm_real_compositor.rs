//! The production [`ZbusWmClient`] driven end to end against the project's
//! **real** headless compositor — the `icedtea-harness::Compositor` that runs
//! the production `Display`/`Runtime`/`Backend`/`State` wiring on a real
//! Wayland socket — rather than the hand-written stub `wm_bus.rs` uses.
//!
//! Two hermetic pieces are joined here:
//!
//! - a private `dbus-daemon` this test owns (never the developer's session
//!   bus), and
//! - the production `org.icedtea.Compositor` service
//!   ([`icedtea_compositor::dbus::spawn_service`]) wired to the harness
//!   compositor's own event stream and command queue, so `IsLocked`/`Quit`
//!   reach the live compositor and `SessionLockChanged` is the signal the real
//!   `State::session_lock_changed` emitted when a real `SessionLockClient`
//!   took/released the `ext_session_lock_manager_v1` lock.
//!
//! `spawn_service` hard-codes the *session* bus, so this test points
//! `DBUS_SESSION_BUS_ADDRESS` at its private daemon through a `Once` guard.
//! The client connects through the *production* `ZbusWmClient::new()` (not
//! the `with_connection` seam), so the env var is what routes it.
//!
//! Skips visibly (`LEXSKIP:`) only when no `dbus-daemon` exists, or the
//! harness compositor cannot create the headless wlroots backend at all; any
//! other boot failure is a real regression and fails the test. **CI must
//! provide `dbus-daemon` and a working headless wlroots.**

mod support;

use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Once};
use std::time::{Duration, Instant};

use icedtea_harness::{Compositor, SessionLockClient};
use icedtea_session::wm_client::{LockState, WmClient, ZbusWmClient};

/// Point every `zbus` `Connection::session()` in this process at `address`.
///
/// # Safety
///
/// `std::env::set_var` is unsound if another thread may read the environment
/// concurrently. `Once::call_once` makes it structurally true that exactly one
/// thread ever runs the write and every other caller blocks until it returns,
/// so no thread can observe or cause a torn read. Mirrors the harness's own
/// guarded env writes under the same project exception.
fn point_session_bus_at(address: &str) {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // SAFETY (icedtea unsafe exception (c)): `Once::call_once` above
        // guarantees this closure runs on exactly one thread and that every
        // other caller of `point_session_bus_at` blocks until it has
        // returned. This file is its own integration-test binary and holds
        // exactly one `#[test]`, so no other test thread exists to race it.
        unsafe {
            std::env::set_var("DBUS_SESSION_BUS_ADDRESS", address);
        }
    });
}

/// Boot the harness compositor, or report the headless backend unavailable and
/// return `None`.
///
/// [`Compositor::spawn`] panics on any boot failure, and its boot thread
/// panics *before* the handshake, so the panic that reaches this thread is
/// only `"compositor thread never completed its boot handshake"` — the
/// `expect` that actually failed is lost with the thread. A scoped panic hook
/// records the real payload (this file is its own single-`#[test]` process, so
/// the global hook cannot race another test) so the failure can be classified
/// instead of assumed: only a boot that could not select the headless wlroots
/// backend is a visible skip, and every other boot panic is re-raised as the
/// regression it is.
fn spawn_or_skip(test: &str) -> Option<Compositor> {
    let recorded = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = Arc::clone(&recorded);
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned());
        if let Some(message) = message {
            sink.lock().unwrap_or_else(|e| e.into_inner()).push(message);
        }
    }));
    let booted = catch_unwind(AssertUnwindSafe(Compositor::spawn));
    std::panic::set_hook(previous);
    match booted {
        Ok(comp) => Some(comp),
        Err(payload) => {
            // `Backend::autocreate`'s `.expect("backend")` is the one boot
            // step that varies with how libwlroots was built; a wlroots
            // without the headless backend panics there and only there.
            let causes = recorded.lock().unwrap_or_else(|e| e.into_inner());
            if causes.iter().any(|cause| cause.starts_with("backend")) {
                eprintln!(
                    "{} {test} — harness compositor unavailable (no headless backend)",
                    support::SKIP_MARKER
                );
                None
            } else {
                // The boot thread's real cause would otherwise be lost behind
                // `spawn`'s generic handshake panic; surface it before
                // re-raising.
                for cause in causes.iter() {
                    eprintln!("harness compositor boot failed: {cause}");
                }
                resume_unwind(payload)
            }
        }
    }
}

/// Whether `seen` has recorded `expected`, polling up to `timeout`.
fn saw(seen: &Arc<Mutex<Vec<bool>>>, expected: bool, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if seen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&expected)
        {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn zbus_wm_client_tracks_the_real_compositor_lock_state_and_signal() {
    const TEST: &str = "zbus_wm_client_tracks_the_real_compositor_lock_state_and_signal";

    let Some(bus) = support::spawn() else {
        eprintln!("{} {TEST} — no dbus-daemon", support::SKIP_MARKER);
        return;
    };
    point_session_bus_at(&bus.address);

    let Some(comp) = spawn_or_skip(TEST) else {
        return;
    };

    // Serve the production interface on this private bus, fed by the *real*
    // compositor's event stream and command queue. It is handed a clone of the
    // loop's own registered wake writer, so every `IsLocked`/`Quit` it serves
    // nudges the real loop directly -- no companion thread needed. Keep the
    // returned connection alive for the whole test so the name stays owned.
    let quit = Arc::new(AtomicBool::new(false));
    let (_service, emitter) = icedtea_compositor::dbus::spawn_service(
        comp.events.clone(),
        comp.commands.clone(),
        Arc::clone(&quit),
        comp.wake_handle(),
    );

    let wm = ZbusWmClient::new().expect("production client opens the private session bus");

    // Unlocked polarity, through the production client.
    assert_eq!(
        wm.lock_state(),
        LockState::Unlocked,
        "the real compositor starts unlocked"
    );
    assert!(!wm.is_locked());

    // Subscribe before the transition. The subscription's async `AddMatch`
    // lands on the same ordered connection; a blocking call on it (plus a
    // short settle) pushes the `AddMatch` out ahead of the first emission.
    // The bounded retry is belt-and-braces for a heavily loaded host: each
    // missed cycle is repeated rather than silently passing, and the final
    // attempt's `false` is asserted against a cleared sink so it cannot be
    // satisfied by an earlier unlock.
    let seen = Arc::new(Mutex::new(Vec::<bool>::new()));
    let sink = Arc::clone(&seen);
    let _subscription = wm
        .subscribe_lock_changed(Box::new(move |locked| {
            sink.lock().unwrap_or_else(|e| e.into_inner()).push(locked);
        }))
        .expect("the production client supports subscriptions");
    let _ = wm.lock_state();
    std::thread::sleep(Duration::from_millis(300));

    let mut observed = false;
    for _ in 0..5 {
        // A real client takes the real compositor's session lock.
        let mut locker = SessionLockClient::spawn(&comp.socket);
        locker.lock();
        assert!(
            locker.wait_locked(),
            "the real compositor never reported the lock"
        );
        assert!(
            comp.session_locked(),
            "harness oracle disagrees with the client"
        );
        if !saw(&seen, true, Duration::from_millis(800)) {
            locker.unlock();
            continue;
        }

        // Locked polarity, through the production client.
        assert_eq!(
            wm.lock_state(),
            LockState::Locked,
            "IsLocked must follow the real lock"
        );
        assert!(wm.is_locked());

        // Clear first so the release assertion can only see this cycle's
        // `SessionLockChanged(false)`.
        seen.lock().unwrap_or_else(|e| e.into_inner()).clear();
        locker.unlock();
        assert!(
            saw(&seen, false, Duration::from_secs(2)),
            "no real SessionLockChanged(false) reached the client callback; saw {:?}",
            seen.lock().unwrap_or_else(|e| e.into_inner())
        );
        assert_eq!(
            wm.lock_state(),
            LockState::Unlocked,
            "IsLocked must follow the real unlock"
        );
        assert!(!wm.is_locked());

        observed = true;
        break;
    }
    assert!(
        observed,
        "no real SessionLockChanged(true) reached the client callback after 5 lock cycles; saw {:?}",
        seen.lock().unwrap_or_else(|e| e.into_inner())
    );

    // `quit()` last, when the harness is torn down with the rest of the test:
    // it reports the call reached the bus, and the harness's own Drop sends a
    // final `Quit` that actually stops the loop.
    assert!(wm.quit(), "Quit must be issued to the real compositor");

    quit.store(true, Ordering::SeqCst);
    drop(comp);
    emitter.join().expect("emitter thread");
}
