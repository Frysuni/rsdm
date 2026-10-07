mod desktop_entry;
mod desktop_exec;
mod desktop_locale;
mod discoverer;

pub use desktop_entry::{DesktopEntry, DesktopEntryError, parse_desktop_entry};
pub use discoverer::DesktopSessionDiscoverer;
