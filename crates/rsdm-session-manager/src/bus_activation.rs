//! dbus-broker activates services through the user manager's environment.

use std::collections::BTreeMap;

use zbus::{Connection, Proxy};

pub(super) async fn update(connection: &Connection, values: BTreeMap<&str, &str>) -> zbus::Result<()> {
    let bus = Proxy::new(connection, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus").await?;
    if uses_user_manager(&bus).await? {
        // The broker forwards UpdateActivationEnvironment asynchronously. Its
        // reply is not a barrier before UnsetEnvironment during logout.
        let manager = Proxy::new(connection, super::DESTINATION, super::MANAGER_PATH, super::MANAGER_INTERFACE).await?;
        let environment: Vec<String> = values.iter().map(|(name, value)| format!("{name}={value}")).collect();
        manager.call("SetEnvironment", &(environment,)).await
    } else {
        bus.call("UpdateActivationEnvironment", &(values,)).await
    }
}

async fn uses_user_manager(bus: &Proxy<'_>) -> zbus::Result<bool> {
    let pid: u32 = match bus.call("GetConnectionUnixProcessID", &("org.freedesktop.DBus",)).await {
        Ok(pid) => pid,
        // Peers without PID lookup retain the standard activation interface.
        Err(zbus::Error::FDO(error)) if matches!(*error, zbus::fdo::Error::UnknownMethod(_)) => return Ok(false),
        Err(error) => return Err(error),
    };
    tokio::task::spawn_blocking(move || {
        let executable = std::fs::read_link(format!("/proc/{pid}/exe"))?;
        let name = executable.file_name().and_then(|name| name.to_str()).unwrap_or_default();
        let name = name.strip_suffix(" (deleted)").unwrap_or(name);
        Ok(matches!(name, "dbus-broker" | "dbus-broker-launch"))
    }).await.map_err(|error| zbus::Error::Failure(format!("reading the bus executable failed: {error}")))?
}

#[cfg(test)]
#[path = "bus_activation_tests.rs"]
mod tests;
