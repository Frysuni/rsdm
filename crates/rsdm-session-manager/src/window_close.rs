//! One owned window close precedes TERM, with durable intent before IPC.

use std::path::Path;

use super::{SessionError, app_dispatch::{self, Method, State}, app_stop::ShutdownControl, bus::UserManager, niri,
    processes::{self, ProcessHandle}, runtime::{self, AppRecord, Runtime, SessionRecord}};

const SERVICE: &str = "org.freedesktop.systemd1.Service";

pub(super) async fn request(manager: &UserManager, runtime: &Runtime, app: &AppRecord, control: &ShutdownControl) -> Result<bool, SessionError> {
    let records = runtime.clone();
    let record = runtime::blocking(move || records.session()).await?;
    if record.disabled { return Err(SessionError::Disabled); }
    let Some((_, path)) = record.exported_environment.iter().find(|(name, _)| name == "NIRI_SOCKET") else { return Ok(false); };
    let prepared = prepare(manager, &record, app, path).await;
    let (mut connection, window, process) = match prepared {
        Ok(Some(prepared)) => prepared,
        Ok(None) => return Ok(false),
        Err(error) => { tracing::debug!(%error, unit = app.unit, "Niri window close is unavailable"); return Ok(false); }
    };
    if control.cancelled() { return Ok(true); }
    let transaction = control.transaction_id.as_deref();
    if !app_dispatch::claim(runtime, app, transaction, Method::WindowClose, &manager.deadline).await? { return Ok(true); }
    // All fallible ownership checks precede the write. After it begins, an
    // interrupted operation is potentially delivered and cannot fall back.
    let dispatched = dispatch(manager, &record, app, &mut connection, &window, &process, control).await;
    let state = match dispatched {
        Ok(true) => State::Sent,
        Ok(false) => State::NotSent,
        Err(error) => { tracing::warn!(%error, unit = app.unit, "Niri window close is unconfirmed; observing the application"); State::Intent }
    };
    app_dispatch::finish(runtime, app, transaction, Method::WindowClose, state, &manager.deadline).await?;
    Ok(state != State::NotSent)
}

async fn prepare(manager: &UserManager, record: &SessionRecord, app: &AppRecord, path: &str) -> Result<Option<(niri::Connection, niri::Window, ProcessHandle)>, SessionError> {
    let mut connection = manager.request(async { Ok(niri::Connection::connect(Path::new(path), record.identity.uid).await) }).await??;
    if !owns_compositor(manager, record, connection.pid).await? { return Ok(None); }
    let pids = processes::app_processes(manager, app).await?;
    let mut windows = manager.request(async { Ok(connection.windows().await) }).await??;
    windows.sort_by_key(|window| window.id);
    for window in windows {
        let Some(pid) = window.pid.and_then(|pid| u32::try_from(pid).ok()).filter(|pid| pids.contains(pid)) else { continue; };
        // Pin one candidate, rather than every browser cgroup process. The
        // second window snapshot in dispatch is checked while it stays pinned.
        let Some(handle) = ProcessHandle::open(pid)? else { continue; };
        if owns_process(manager, app, pid, &handle).await? { return Ok(Some((connection, window, handle))); }
    }
    Ok(None)
}

async fn dispatch(manager: &UserManager, record: &SessionRecord, app: &AppRecord, connection: &mut niri::Connection, window: &niri::Window, handle: &ProcessHandle, control: &ShutdownControl) -> Result<bool, SessionError> {
    let pid = window.pid.and_then(|pid| u32::try_from(pid).ok()).unwrap_or(0);
    if !owns_compositor(manager, record, connection.pid).await? || !owns_process(manager, app, pid, handle).await? { return Ok(false); }
    let windows = manager.request(async { Ok(connection.windows().await) }).await??;
    if !windows.contains(window) || !owns_process(manager, app, pid, handle).await? { return Ok(false); }
    manager.request(async {
        manager.actions.check_generation_async().await.map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;
        if control.cancelled() || !handle.alive()? { return Ok(Ok(false)); }
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
