//! Backend selection.
//!
//! `wlr`'s `Backend::autocreate` covers nested-under-Wayland, X11, headless
//! and DRM from one call, driven entirely by `WLR_BACKENDS` and friends in
//! the environment. This module is therefore just the `--nested` alias
//! ([`apply_backend_choice`]) plus the shutdown self-pipe
//! ([`shutdown_source`]) that lets the run loop in `lib.rs` stop cleanly.

use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;

/// Which backend the compositor was asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendChoice {
    /// Nested inside an existing Wayland session.
    Nested,
    /// Whatever the environment offers -- DRM on a TTY, X11 or Wayland under a
    /// session, headless in CI.
    Auto,
}

impl BackendChoice {
    /// `--nested` selects [`BackendChoice::Nested`]; anything else is
    /// [`BackendChoice::Auto`].
    pub fn from_args(args: impl IntoIterator<Item = String>) -> Self {
        if args.into_iter().any(|a| a == "--nested") {
            BackendChoice::Nested
        } else {
            BackendChoice::Auto
        }
    }
}

/// Apply a backend choice to the environment, before the backend is created.
///
/// `--nested` is an alias for `WLR_BACKENDS=wayland`, which is the whole of
/// what "nested" means now: the compositor library's `autocreate` reads that
/// variable and gives back a Wayland-backed output inside the host session.
/// The smithay build needed a bespoke winit path for this and a separate DRM
/// path for a TTY; one call now covers nested-under-Wayland, X11, headless
/// and DRM, and the flag survives only because it is in muscle memory and in
/// the README.
///
/// An explicit `WLR_BACKENDS` already in the environment wins: someone who
/// set it meant it, and silently overriding it would make `--nested` a
/// mystery rather than an alias.
pub fn apply_backend_choice(choice: BackendChoice) {
    if choice != BackendChoice::Nested {
        return;
    }
    if std::env::var_os("WLR_BACKENDS").is_some() {
        tracing::info!("WLR_BACKENDS is already set; --nested leaves it alone");
        return;
    }
    // SAFETY (icedtea unsafe exception (c)): this runs during single-threaded
    // startup, before the D-Bus thread, the wallpaper worker, or the event
    // loop exist, so no other thread can observe a torn environment read.
    // Same window and same argument as the `WAYLAND_DISPLAY` set below.
    unsafe {
        std::env::set_var("WLR_BACKENDS", "wayland");
    }
}

/// Register a self-pipe that SIGINT and SIGTERM write to, as an event source.
///
/// This is the slice's live consumer of the fd-source API, and it is a
/// self-pipe rather than a `signalfd` for one reason: `signalfd(2)` and
/// `sigprocmask(2)` would put raw `unsafe` in this crate, which the project's
/// unsafe policy does not permit outside its two named exceptions.
/// `signal_hook::low_level::pipe::register` is a safe API that does the
/// async-signal-safe write for us, and the read end is an ordinary
/// descriptor.
///
/// The write half is deliberately leaked into the signal handler's keeping
/// (`std::mem::forget`): it must stay open for the life of the process, and
/// there is no later point at which closing it would be correct — a closed
/// write half would give the read end a permanent hangup and spin the loop.
pub fn shutdown_source(runtime: &wlr::Runtime) -> std::io::Result<wlr::SourceId> {
    let (read, write) = UnixStream::pair()?;
    signal_hook::low_level::pipe::register(signal_hook::consts::SIGINT, write.try_clone()?)?;
    signal_hook::low_level::pipe::register(signal_hook::consts::SIGTERM, write.try_clone()?)?;
    std::mem::forget(write);

    let fd = OwnedFd::from(read);
    Ok(runtime.add_fd(fd, wlr::Interest::READABLE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_is_selected_only_by_its_own_flag() {
        assert_eq!(
            BackendChoice::from_args(["icedtea-compositor".into(), "--nested".into()]),
            BackendChoice::Nested
        );
        assert_eq!(BackendChoice::from_args(["icedtea-compositor".into()]), BackendChoice::Auto);
        assert_eq!(
            BackendChoice::from_args(["icedtea-compositor".into(), "--nestedish".into()]),
            BackendChoice::Auto
        );
    }
}

#[cfg(test)]
mod shutdown_tests {
    use super::*;

    /// Registration must succeed and hand back an id the runtime knows,
    /// independently of any backend — this is what boot's ordering relies on.
    #[test]
    fn a_shutdown_source_registers_against_a_bare_runtime() {
        let runtime = wlr::Runtime::new().expect("runtime");
        let a = shutdown_source(&runtime).expect("first");
        let b = shutdown_source(&runtime).expect("second");
        assert_ne!(a, b, "each registration gets its own id");
    }
}
