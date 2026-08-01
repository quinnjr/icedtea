pub mod event;
pub mod types;

pub const WM_BUS_NAME: &str = "org.icedtea.WM";
pub const WM_PATH: &str = "/org/icedtea/WM";

pub use event::Event;
pub use types::*;
