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
//! `DBUS_SESSION_BUS_ADDRESS` at its private daemon. That is safe **only**
//! because this file holds a single test: an integration-test file is its own
//! process, and with one test there is no parallel thread that could observe
//! the rewritten variable. The client connects through the *production*
//! `ZbusWmClient::new()` (not the `with_connection` seam), so the env var is
//! what routes it.
//!
//! Skips visibly (`LEXSKIP:`) when no `dbus-daemon` exists or the harness
//! compositor cannot boot. **CI must provide `dbus-daemon` and a working
//! headless wlroots.**

mod support;

use std::os::unix::net::UnixStream;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use icedtea_harness::{Compositor, SessionLockClient};
use icedtea_session::wm_client::{LockState, WmClient, ZbusWmClient};

/// Point every `zbus` `Connection::session()` in this process at `address`.
///
/// # Safety
///
/// `std::env::set_var` is unsound if another thread may read the environment
/// concurrently. This file contains exactly one `#[test]`, so no other test
/// thread exists; the harness compositor is spawned only after this call, and
/// the only readers of `DBUS_SESSION_BUS_ADDRESS` are the zbus connections
/// this test itself opens. Mirrors the harness's own guarded env writes.
fn point_session_bus_at(address: &str) {
    unsafe {
        std::env::set_var("DBUS_SESSION_BUS_ADDRESS", address);
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
    let Some(bus) = support::spawn() else {
        eprintln!(
            "{} zbus_wm_client_tracks_the_real_compositor_lock_state_and_signal — no dbus-daemon",
            support::SKIP_MARKER
        );
        return;
    };
    point_session_bus_at(&bus.address);

    let comp = match catch_unwind(AssertUnwindSafe(Compositor::spawn)) {
        Ok(comp) => comp,
        Err(_) => {
            eprintln!(
                "{} zbus_wm_client_tracks_the_real_compositor_lock_state_and_signal — harness \
                 compositor unavailable (no headless backend)",
                support::SKIP_MARKER
            );
            return;
        }
    };

    // Serve the production interface on this private bus, fed by the *real*
    // compositor's event stream and command queue. Keep the returned
    // connection alive for the whole test so the name stays owned.
    let (_wake_read, wake_write) = UnixStream::pair().expect("wake pair");
    let quit = Arc::new(AtomicBool::new(false));
    let (_service, emitter) = icedtea_compositor::dbus::spawn_service(
        comp.events.clone(),
        comp.commands.clone(),
        Arc::clone(&quit),
        wake_write,
    );

    let wm = ZbusWmClient::new().expect("production client opens the private session bus");

    // The harness compositor only drains its command channel when the loop
    // wakes; the D-Bus interface has no handle on the loop's registered wake
    // fd, so a companion thread nudges it with a real `SessionLocked`
    // round-trip while the assertions below make their own D-Bus calls. Not a
    // synchronization nicety: without it a blocked `IsLocked` would time out
    // to `Unreachable`.
    let stop = Arc::new(AtomicBool::new(false));
    std::thread::scope(|scope| {
        let nudge_stop = Arc::clone(&stop);
        let comp = &comp;
        scope.spawn(move || {
            while !nudge_stop.load(Ordering::SeqCst) {
                let _ = comp.session_locked();
                std::thread::sleep(Duration::from_millis(10));
            }
        });

        // Unlocked polarity, through the production client.
        assert_eq!(
            wm.lock_state(),
            LockState::Unlocked,
            "the real compositor starts unlocked"
        );
        assert!(!wm.is_locked());

        // Subscribe before the transition. The subscription's async
        // `AddMatch` lands on the same ordered connection; a blocking call on
        // it (plus a short settle) pushes the `AddMatch` out ahead of the
        // first emission. The bounded retry is belt-and-braces for a heavily
        // loaded host: each missed cycle is repeated rather than silently
        // passing, and the final attempt's `false` is asserted against a
        // cleared sink so it cannot be satisfied by an earlier unlock.
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

        stop.store(true, Ordering::SeqCst);
    });

    // `quit()` last, when the harness is torn down with the rest of the test:
    // it reports the call reached the bus, and the harness's own Drop sends a
    // final `Quit` that actually stops the loop.
    assert!(wm.quit(), "Quit must be issued to the real compositor");

    quit.store(true, Ordering::SeqCst);
    drop(comp);
    emitter.join().expect("emitter thread");
}
