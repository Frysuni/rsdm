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
    let Some(mut prepared) = prepare(manager, &record, app).await else {
        tracing::debug!(unit = app.unit, "no verified window is available for close");
        return Ok(false);
    };
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
    if let Some(path) = super::wayland_registry::socket_path(&record.exported_environment) {
        match prepare_endpoint(manager, record, app, "WAYLAND_DISPLAY", &path).await {
            Ok(Some(prepared)) => return Some(prepared),
            Ok(None) => {},
            Err(error) => { tracing::debug!(%error, unit = app.unit, "KWin window close is unavailable"); }
        }
    }
    None
}

async fn prepare_endpoint(manager: &UserManager, record: &SessionRecord, app: &AppRecord, name: &str, path: &Path) -> Result<Option<PreparedClose>, SessionError> {
    let Some((connection, events, mut windows)) = connect_endpoint(manager, record, name, path).await? else { return Ok(None); };
    let pids = processes::app_processes(manager, app).await?;
    tracing::debug!(unit = app.unit, endpoint = name, window_count = windows.len(), process_count = pids.len(), "checking application windows");
    connection.order_windows(&mut windows);
    for window in windows {
        let Some(pid) = window.pid.and_then(|pid| u32::try_from(pid).ok()).filter(|pid| pids.contains(pid)) else { continue; };
        // Pin one candidate, rather than every browser cgroup process. The
        // second window snapshot in dispatch is checked while it stays pinned.
        let Some(handle) = ProcessHandle::open(pid)? else { continue; };
        if owns_process(manager, app, pid, &handle).await? {
            tracing::debug!(unit = app.unit, pid, window = window.id, "prepared owned window close");
            return Ok(Some(PreparedClose { connection, events, window, process: handle }));
        }
        tracing::debug!(unit = app.unit, pid, window = window.id, "window process ownership could not be verified");
    }
    Ok(None)
}

pub(super) async fn connect_endpoint(manager: &UserManager, record: &SessionRecord, name: &str, path: &Path) -> Result<Option<(Connection, Events, Vec<Window>)>, SessionError> {
    let mut connection = manager.request(async { Ok(Connection::connect(name, path, record.identity.uid).await) }).await??;
    if !owns_compositor(manager, record, connection.pid()).await? {
        tracing::debug!(endpoint = name, pid = connection.pid(), "compositor ownership could not be verified");
        return Ok(None);
    }
    if let Connection::KWin(kwin) = &mut connection {
        kwin.authenticate(manager).await?;
        let windows = manager.request(async { Ok(connection.windows().await) }).await??;
        if !owns_compositor(manager, record, connection.pid()).await? { return Ok(None); }
        return Ok(Some((connection, Events::KWin, windows)));
    }
    let subscriber = manager.request(async { Ok(Connection::connect(name, path, record.identity.uid).await) }).await??;
    if subscriber.pid() != connection.pid() || !owns_compositor(manager, record, subscriber.pid()).await? { return Ok(None); }
    let (events, windows) = manager.request(async { Ok(connection.subscribe(subscriber).await) }).await??;
    Ok(Some((connection, events, windows)))
}

async fn dispatch(manager: &UserManager, record: &SessionRecord, app: &AppRecord, prepared: &mut PreparedClose, control: &ShutdownControl) -> Result<bool, SessionError> {
    let PreparedClose { connection, window, process: handle, .. } = prepared;
    let pid = window.pid.and_then(|pid| u32::try_from(pid).ok()).unwrap_or(0);
    if !owns_compositor(manager, record, connection.pid()).await? || !owns_process(manager, app, pid, handle).await? { return Ok(false); }
    let windows = manager.request(async { Ok(connection.windows().await) }).await??;
    if !windows.contains(window) || !owns_process(manager, app, pid, handle).await? { return Ok(false); }
    close_prepared(manager, record, app, prepared, control).await
}

async fn close_prepared(manager: &UserManager, record: &SessionRecord, app: &AppRecord, prepared: &mut PreparedClose, control: &ShutdownControl) -> Result<bool, SessionError> {
    let PreparedClose { connection, events, window, process: handle } = prepared;
    let pid = window.pid.and_then(|pid| u32::try_from(pid).ok()).unwrap_or(0);
    let bus = manager.connection();
    manager.request(async {
        if !super::window_ownership::current(&bus, record, app, connection.pid(), pid, handle).await? {
            tracing::debug!(unit = app.unit, pid, window = window.id, "final ownership proof rejected window close");
            return Ok(Ok(false));
        }
        manager.actions.check_generation_async().await.map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;
        manager.deadline.remaining(std::time::Duration::MAX)?;
        if control.cancelled() || !handle.alive()? { return Ok(Ok(false)); }
        manager.actions.check().map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;
        match events.unchanged(connection, window) {
            Ok(true) => {}
            Ok(false) => {
                tracing::debug!(unit = app.unit, window = window.id, "window changed before close");
                return Ok(Ok(false));
            }
            Err(error) => { tracing::debug!(%error, "window events cannot authorize close"); return Ok(Ok(false)); }
        }
        Ok(connection.close(window.id).await)
    }).await?
}

pub(super) async fn owns_compositor(manager: &UserManager, record: &SessionRecord, pid: u32) -> Result<bool, SessionError> {
    let Some(unit) = &record.compositor_unit else { return Ok(false); };
    if record.compositor_invocation.is_empty() || record.compositor_invocation.iter().all(|byte| *byte == 0) { return Ok(false); }
    Ok(manager.invocation_id(unit).await? == record.compositor_invocation
        && manager.unit_property::<u32>(unit, SERVICE, "MainPID").await? == pid
        && manager.invocation_id(unit).await? == record.compositor_invocation)
}

async fn owns_process(manager: &UserManager, app: &AppRecord, pid: u32, handle: &ProcessHandle) -> Result<bool, SessionError> {
    if !handle.alive()? || !processes::app_processes(manager, app).await?.contains(&pid) { return Ok(false); }
    let groups = processes::process_groups(manager, app).await?;
    let membership = runtime::blocking(move || Ok(std::fs::read_to_string(format!("/proc/{pid}/cgroup"))?)).await?;
    Ok(groups.iter().any(|group| processes::belongs_to_cgroup(&membership, group)) && handle.alive()?)
}

#[cfg(test)]
#[path = "window_close_tests.rs"]
pub(crate) mod tests;

#[cfg(test)]
#[path = "window_dispatch_tests.rs"]
mod dispatch_tests;
