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
