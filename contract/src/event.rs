use crate::{AltTabState, Appearance, WindowId, WindowInfo, WindowUpdate, WorkspaceInfo};

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    WindowOpened(WindowInfo),
    WindowClosed(WindowId),
    WindowUpdated { id: WindowId, update: WindowUpdate },
    WorkspaceSet { id: u32, active: bool },
    WorkspaceList(Vec<WorkspaceInfo>),
    AltTabState(AltTabState),
    ConfigReloaded(Appearance),
}

/// An [`Event`] tagged with the `seq` its producing mutation advanced the
/// model's sequence counter to.
///
/// Review finding I2: the design doc's client-connection model says "the
/// snapshot envelope carries a monotonically increasing sequence number so a
/// client can detect missed events and re-sync", but only [`crate::Snapshot`]
/// carried one -- signals didn't, so a client that called `GetState()` at
/// seq N and then subscribed could neither tell which signals were already
/// folded into that snapshot (they may have been queued but not yet emitted
/// when the snapshot was answered) nor detect a gap. Every event now travels
/// with the `seq` value it produced: a client discards any signal whose
/// `seq <= snapshot.seq`, and treats `seq > last_seen + 1` as a missed
/// event requiring a fresh `GetState()`.
///
/// `seq` is the *first* argument of every D-Bus signal (see
/// `compositor/src/dbus.rs`).
#[derive(Debug, Clone, PartialEq)]
pub struct SeqEvent {
    pub seq: u64,
    pub event: Event,
}
