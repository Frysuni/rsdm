//! Read-only discovery of compositor APIs and their ownership limitations.

use std::path::Path;

use super::{SessionError, bus::UserManager, runtime::SessionRecord, status::Capability, window_close};

pub(super) async fn capability(manager: &UserManager, record: &SessionRecord) -> Capability {
    if record.disabled {
        return Capability::new("window-close", "unavailable", "Session Manager is disabled.");
    }
    if matches!(record.provider.as_str(), "gnome" | "plasma") {
        return Capability::new("window-close", "not-applicable", "The native desktop owns application shutdown and its dialogs.");
    }
    if record.compositor_unit.is_none() || record.compositor_invocation.is_empty() {
        return Capability::new("window-close", "unavailable", "No verified compositor invocation is recorded; TERM fallback remains subject to app policy.");
    }
    match tokio::time::timeout(std::time::Duration::from_secs(2), probe(manager, record)).await {
        Ok(capability) => capability,
        Err(_) => Capability::new("window-close", "unavailable", "The compositor capability probe timed out."),
    }
}

async fn probe(manager: &UserManager, record: &SessionRecord) -> Capability {
    let mut reason = "No supported compositor IPC endpoint is recorded.".to_string();
    for (name, value) in &record.exported_environment {
        if name == "WAYFIRE_SOCKET" {
            reason = match discover_wayfire(manager, record, Path::new(value)).await {
                Ok(()) => "WAYFIRE_SOCKET: discovery available on the verified compositor; view IDs lack an atomic identity guard and reported PIDs do not prove ownership. TERM fallback remains subject to app policy.".into(),
                Err(error) => format!("WAYFIRE_SOCKET: {error}"),
            };
            continue;
        }
        if !matches!(name.as_str(), "NIRI_SOCKET" | "SWAYSOCK" | "HYPRLAND_INSTANCE_SIGNATURE") { continue; }
        let path = if name == "HYPRLAND_INSTANCE_SIGNATURE" {
            let Some(path) = super::hyprland::socket_directory(&record.exported_environment, value) else {
                reason = "The recorded Hyprland endpoint is invalid.".into();
                continue;
            };
            path
        } else { Path::new(value).to_path_buf() };
        match window_close::connect_endpoint(manager, record, name, &path).await {
            Ok(Some(_)) => return Capability::new("window-close", "available", &format!("{name}: authenticated command/event transport and window API; every app still requires ownership checks.")),
            Ok(None) => reason = format!("{name}: IPC peer does not match the recorded compositor invocation."),
            Err(error) => reason = format!("{name}: {error}"),
        }
    }
    if record.exported_environment.iter().any(|(name, _)| name == "WAYLAND_DISPLAY") {
        let discovery = match discover_wayland(manager, record).await {
            Ok(interfaces) if interfaces.is_empty() => "WAYLAND_DISPLAY: no foreign-toplevel or Plasma window-management protocol is advertised.".into(),
            Ok(interfaces) if interfaces.iter().any(|interface| interface.starts_with("org_kde_plasma_window_management ")) => {
                let path = super::wayland_registry::socket_path(&record.exported_environment).expect("discovered Wayland endpoint");
                match window_close::connect_endpoint(manager, record, "WAYLAND_DISPLAY", &path).await {
                    Ok(Some(_)) => return Capability::new("window-close", "available", "WAYLAND_DISPLAY: authenticated KWin version and live Plasma window handles; every app still requires PID/cgroup checks."),
                    Ok(None) => "WAYLAND_DISPLAY: KWin peer does not match the compositor invocation.".into(),
                    Err(error) => format!("WAYLAND_DISPLAY: {error}"),
                }
            }
            Ok(interfaces) => format!("WAYLAND_DISPLAY: advertised {}; registry discovery alone provides no verified window PID/cgroup or close handle. TERM fallback remains subject to app policy.", interfaces.join(", ")),
            Err(error) => format!("WAYLAND_DISPLAY: {error}"),
        };
        reason.push(' ');
        reason.push_str(&discovery);
    }
    Capability::new("window-close", "unavailable", &reason)
}

async fn discover_wayland(manager: &UserManager, record: &SessionRecord) -> Result<Vec<String>, SessionError> {
    let path = super::wayland_registry::socket_path(&record.exported_environment)
        .ok_or_else(|| SessionError::State("the recorded Wayland endpoint is invalid".into()))?;
    let mut connection = manager.request(async { Ok(super::wayland_registry::Connection::connect(&path, record.identity.uid).await) }).await??;
    if !window_close::owns_compositor(manager, record, connection.pid).await? {
        return Err(SessionError::State("Wayland peer does not match the recorded compositor invocation".into()));
    }
    let interfaces = manager.request(async { Ok(connection.discover().await) }).await??;
    if !window_close::owns_compositor(manager, record, connection.pid).await? {
        return Err(SessionError::State("compositor invocation changed during Wayland discovery".into()));
    }
    Ok(interfaces)
}

async fn discover_wayfire(manager: &UserManager, record: &SessionRecord, path: &Path) -> Result<(), SessionError> {
    let mut connection = manager.request(async { Ok(super::wayfire::Connection::connect(path, record.identity.uid).await) }).await??;
    if !window_close::owns_compositor(manager, record, connection.pid).await? {
        return Err(SessionError::State("IPC peer does not match the recorded compositor invocation".into()));
    }
    manager.request(async { Ok(connection.discover().await) }).await??;
    if !window_close::owns_compositor(manager, record, connection.pid).await? {
        return Err(SessionError::State("compositor invocation changed during discovery".into()));
    }
    Ok(())
}

#[cfg(test)]
#[path = "window_capability_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "window_wayland_capability_tests.rs"]
mod wayland_tests;
