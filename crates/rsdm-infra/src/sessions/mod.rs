mod desktop_entry;
mod discoverer;

pub use desktop_entry::{DesktopEntry, DesktopEntryError, parse_desktop_entry};
pub use discoverer::DesktopSessionDiscoverer;
