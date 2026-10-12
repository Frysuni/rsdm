//! One owned window close precedes TERM, with durable intent before IPC.

use std::path::Path;

use super::{SessionError, app_dispatch::{self, Method, State}, app_stop::ShutdownControl, bus::UserManager, window_backend::{Connection, Events, Window},
    processes::{self, ProcessHandle}, runtime::{self, AppRecord, Runtime, SessionRecord}};

const SERVICE: &str = "org.freedesktop.systemd1.Service";

struct PreparedClose {
    connection: Connection,
    events: Events,
    window: Window,
    process: ProcessHandle,
}

pub(super) async fn request(manager: &UserManager, runtime: &Runtime, app: &AppRecord, control: &ShutdownControl) -> Result<bool, SessionError> {
    let records = runtime.clone();
    let record = runtime::blocking(move || records.session()).await?;
    if record.disabled { return Err(SessionError::Disabled); }
    let Some(mut prepared) = prepare(manager, &record, app).await else { return Ok(false); };
    if control.cancelled() { return Ok(true); }
    let transaction = control.transaction_id.as_deref();
    if !app_dispatch::claim(runtime, app, transaction, Method::WindowClose, &manager.deadline).await? { return Ok(true); }
    // All fallible ownership checks precede the write. After it begins, an
    // interrupted operation is potentially delivered and cannot fall back.
    let dispatched = dispatch(manager, &record, app, &mut prepared, control).await;
    let state = match dispatched {
        Ok(true) => State::Sent,
        Ok(false) => State::NotSent,
        Err(error) => { tracing::warn!(%error, unit = app.unit, "window close is unconfirmed; observing the application"); State::Intent }
    };
    app_dispatch::finish(runtime, app, transaction, Method::WindowClose, state, &manager.deadline).await?;
    Ok(state != State::NotSent)
}

async fn prepare(manager: &UserManager, record: &SessionRecord, app: &AppRecord) -> Option<PreparedClose> {
    for (name, path) in &record.exported_environment {
        if !matches!(name.as_str(), "NIRI_SOCKET" | "SWAYSOCK" | "HYPRLAND_INSTANCE_SIGNATURE") { continue; }
        let path = if name == "HYPRLAND_INSTANCE_SIGNATURE" {
            let Some(directory) = super::hyprland::socket_directory(&record.exported_environment, path) else { continue; };
            directory
        } else { Path::new(path).to_path_buf() };
        match prepare_endpoint(manager, record, app, name, &path).await {
            Ok(Some(prepared)) => return Some(prepared),
            Ok(None) => {}
            Err(error) => { tracing::debug!(%error, endpoint = name, unit = app.unit, "window close is unavailable"); }
        }
    }
    None
}

async fn prepare_endpoint(manager: &UserManager, record: &SessionRecord, app: &AppRecord, name: &str, path: &Path) -> Result<Option<PreparedClose>, SessionError> {
    let Some((connection, events, mut windows)) = connect_endpoint(manager, record, name, path).await? else { return Ok(None); };
    let pids = processes::app_processes(manager, app).await?;
    windows.sort_by_key(|window| window.id);
    for window in windows {
        let Some(pid) = window.pid.and_then(|pid| u32::try_from(pid).ok()).filter(|pid| pids.contains(pid)) else { continue; };
        // Pin one candidate, rather than every browser cgroup process. The
        // second window snapshot in dispatch is checked while it stays pinned.
        let Some(handle) = ProcessHandle::open(pid)? else { continue; };
        if owns_process(manager, app, pid, &handle).await? {
            return Ok(Some(PreparedClose { connection, events, window, process: handle }));
        }
    }
    Ok(None)
}

async fn connect_endpoint(manager: &UserManager, record: &SessionRecord, name: &str, path: &Path) -> Result<Option<(Connection, Events, Vec<Window>)>, SessionError> {
    let mut connection = manager.request(async { Ok(Connection::connect(name, path, record.identity.uid).await) }).await??;
    if !owns_compositor(manager, record, connection.pid()).await? { return Ok(None); }
    let subscriber = manager.request(async { Ok(Connection::connect(name, path, record.identity.uid).await) }).await??;
    if subscriber.pid() != connection.pid() || !owns_compositor(manager, record, subscriber.pid()).await? { return Ok(None); }
    let (events, windows) = manager.request(async { Ok(connection.subscribe(subscriber).await) }).await??;
    Ok(Some((connection, events, windows)))
}

pub(super) async fn capability(manager: &UserManager, record: &SessionRecord) -> super::status::Capability {
    use super::status::Capability;
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

async fn probe(manager: &UserManager, record: &SessionRecord) -> super::status::Capability {
    use super::status::Capability;
    let mut reason = "No supported compositor IPC endpoint is recorded.".to_string();
    for (name, value) in &record.exported_environment {
        if !matches!(name.as_str(), "NIRI_SOCKET" | "SWAYSOCK" | "HYPRLAND_INSTANCE_SIGNATURE") { continue; }
        let path = if name == "HYPRLAND_INSTANCE_SIGNATURE" {
            let Some(path) = super::hyprland::socket_directory(&record.exported_environment, value) else {
                reason = "The recorded Hyprland endpoint is invalid.".into();
                continue;
            };
            path
        } else { Path::new(value).to_path_buf() };
        match connect_endpoint(manager, record, name, &path).await {
            Ok(Some(_)) => return Capability::new("window-close", "available", &format!("{name}: authenticated command/event transport and window API; every app still requires ownership checks.")),
            Ok(None) => reason = format!("{name}: IPC peer does not match the recorded compositor invocation."),
            Err(error) => reason = format!("{name}: {error}"),
        }
    }
    Capability::new("window-close", "unavailable", &reason)
}

async fn dispatch(manager: &UserManager, record: &SessionRecord, app: &AppRecord, prepared: &mut PreparedClose, control: &ShutdownControl) -> Result<bool, SessionError> {
    let PreparedClose { connection, events, window, process: handle } = prepared;
    let pid = window.pid.and_then(|pid| u32::try_from(pid).ok()).unwrap_or(0);
    if !owns_compositor(manager, record, connection.pid()).await? || !owns_process(manager, app, pid, handle).await? { return Ok(false); }
    let windows = manager.request(async { Ok(connection.windows().await) }).await??;
    if !windows.contains(window) || !owns_process(manager, app, pid, handle).await? { return Ok(false); }
    manager.request(async {
        manager.actions.check_generation_async().await.map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;
        if control.cancelled() || !handle.alive()? { return Ok(Ok(false)); }
        match events.unchanged(window) {
            Ok(true) => {}
            Ok(false) => return Ok(Ok(false)),
            Err(error) => { tracing::debug!(%error, "window events cannot authorize close"); return Ok(Ok(false)); }
        }
        Ok(connection.close(window.id).await)
    }).await?
}

async fn owns_compositor(manager: &UserManager, record: &SessionRecord, pid: u32) -> Result<bool, SessionError> {
    let Some(unit) = &record.compositor_unit else { return Ok(false); };
    if record.compositor_invocation.is_empty() || record.compositor_invocation.iter().all(|byte| *byte == 0) { return Ok(false); }
    Ok(manager.invocation_id(unit).await? == record.compositor_invocation
        && manager.unit_property::<u32>(unit, SERVICE, "MainPID").await? == pid
        && manager.invocation_id(unit).await? == record.compositor_invocation)
}

async fn owns_process(manager: &UserManager, app: &AppRecord, pid: u32, handle: &ProcessHandle) -> Result<bool, SessionError> {
    if !handle.alive()? || !processes::app_processes(manager, app).await?.contains(&pid) { return Ok(false); }
    let group: String = manager.unit_property(&app.unit, SERVICE, "ControlGroup").await?;
    if !group.starts_with('/') || group == "/" { return Ok(false); }
    let membership = runtime::blocking(move || Ok(std::fs::read_to_string(format!("/proc/{pid}/cgroup"))?)).await?;
    Ok(processes::belongs_to_cgroup(&membership, &group) && handle.alive()? && processes::verify_invocation(manager, app).await?)
}

#[cfg(test)]
#[path = "window_close_tests.rs"]
mod tests;
