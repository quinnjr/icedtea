//! `icedtea-notifications` — the `org.freedesktop.Notifications` /
//! `org.icedtea.Notifications` D-Bus daemon. See
//! `docs/superpowers/specs/2026-08-20-icedtea-notifications-daemon-design.md`
//! for the design; this is milestone N1 (model + contract types only — the
//! D-Bus service, expiry worker, and `main` wiring land in N2).

// N1 only wires the pure store and its tests; N2 wires `service`/`expiry`
// consumers of this API onto the D-Bus layer, so most of it is unused today.
#[allow(dead_code)]
mod store;

fn main() {
    unimplemented!("D-Bus service wiring lands in milestone N2");
}
