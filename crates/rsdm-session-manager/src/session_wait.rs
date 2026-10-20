//! Passive launcher lifetime after session management has detached.

use std::{os::unix::process::ExitStatusExt, sync::Arc, time::Duration};

use zbus::{Connection, OwnedObjectPath, OwnedVariant};

use super::{SessionError, processes::ProcessHandle, provider::Provider, runtime::SessionRecord, session_process::SessionProcess};

const DESTINATION: &str = "org.freedesktop.systemd1";
const UNIT: &str = "org.freedesktop.systemd1.Unit";
const SERVICE: &str = "org.freedesktop.systemd1.Service";

pub(super) async fn wait(process: SessionProcess, record: &SessionRecord, provider: &Provider, bus: Connection) -> Result<i32, SessionError> {
    let mut pinned = process.compositor.lock().unwrap().clone();
    let mut connection = Some(bus);
    let mut record = record.clone();
    loop {
        if let Some(handle) = &pinned {
            if !handle.alive()? { return finish(&process, &record, connection.as_ref()).await; }
        } else {
            if connection.is_none() {
                connection = connect().await.ok();
            }
            if let Some(bus) = &connection {
                let observed = tokio::time::timeout(Duration::from_secs(5), observe(bus, &process, &record, provider)).await;
                match observed {
                    Ok(Ok(Observed::Running(handle, id))) => {
                        pinned = Some(handle);
                        if !id.is_empty() { record.compositor_invocation = id; }
                    }
                    Ok(Ok(Observed::Exited)) => return finish(&process, &record, connection.as_ref()).await,
                    Ok(Ok(Observed::Pending)) => {}
                    _ => connection = None,
                }
            }
            if process.unit.is_none() && !provider.native_desktop() { return finish_child(&process).await; }
            if process.unit.is_none() && pinned.is_none() {
                let exited = match &process.child {
                    Some(child) => child.lock().await.try_wait()?.is_some(),
                    None => false,
                };
                if exited { return finish_child(&process).await; }
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn finish(process: &SessionProcess, record: &SessionRecord, connection: Option<&Connection>) -> Result<i32, SessionError> {
    if process.child.is_some() { return finish_child(process).await; }
    let unit = process.unit.as_ref().ok_or_else(|| SessionError::State("launcher has no process or compositor unit".into()))?;
    let connection = connection.ok_or_else(|| SessionError::State("compositor exited but its status cannot be read".into()))?;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let reply = connection.call_method(Some(DESTINATION), "/org/freedesktop/systemd1",
                Some("org.freedesktop.systemd1.Manager"), "GetUnit", &(unit,)).await?;
            let path: OwnedObjectPath = reply.body().deserialize()?;
            let id: Vec<u8> = property(connection, &path, UNIT, "InvocationID").await?;
            if id.iter().any(|byte| *byte != 0) && id != record.compositor_invocation {
                return Err(SessionError::State("compositor exit status belongs to another invocation".into()));
            }
            let state: String = property(connection, &path, UNIT, "ActiveState").await?;
            if matches!(state.as_str(), "inactive" | "failed") {
                let code: i32 = property(connection, &path, SERVICE, "ExecMainCode").await?;
                let status: i32 = property(connection, &path, SERVICE, "ExecMainStatus").await?;
                if property::<Vec<u8>>(connection, &path, UNIT, "InvocationID").await? != id {
                    return Err(SessionError::State("compositor invocation changed while its exit status was read".into()));
                }
                return Ok(if code == libc::CLD_EXITED { status } else { 128 + status });
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }).await.map_err(|_| SessionError::State("compositor exited but its status remains unconfirmed".into()))?
}

async fn finish_child(process: &SessionProcess) -> Result<i32, SessionError> {
    let Some(child) = &process.child else { return Ok(0); };
    let status = child.lock().await.wait().await?;
    Ok(status.code().or_else(|| status.signal().map(|signal| 128 + signal)).unwrap_or(1))
}

enum Observed { Running(Arc<ProcessHandle>, Vec<u8>), Pending, Exited }

async fn observe(connection: &Connection, process: &SessionProcess, record: &SessionRecord, provider: &Provider) -> Result<Observed, SessionError> {
    if let Some(unit) = &process.unit {
        return observe_unit(connection, unit, record).await;
    }
    if provider.native_desktop() {
        return Ok(super::native_compositor::pin(connection, provider).await?
            .map_or(Observed::Pending, |handle| Observed::Running(handle, Vec::new())));
    }
    Ok(Observed::Exited)
}

async fn observe_unit(connection: &Connection, unit: &str, record: &SessionRecord) -> Result<Observed, SessionError> {
    let reply = connection.call_method(Some(DESTINATION), "/org/freedesktop/systemd1",
        Some("org.freedesktop.systemd1.Manager"), "GetUnit", &(unit,)).await?;
    let path: OwnedObjectPath = reply.body().deserialize()?;
    let id: Vec<u8> = property(connection, &path, UNIT, "InvocationID").await?;
    if id.iter().any(|byte| *byte != 0) && !record.compositor_invocation.is_empty() && record.compositor_invocation != id {
        return Ok(Observed::Pending);
    }
    let environment: Vec<String> = property(connection, &path, SERVICE, "Environment").await?;
    if !environment.contains(&format!("RSDM_SESSION_GENERATION={}", record.identity.generation)) { return Ok(Observed::Pending); }
    let pid: u32 = property(connection, &path, SERVICE, "MainPID").await?;
    let handle = if pid == 0 { None } else { ProcessHandle::open(pid)? };
    if property::<Vec<u8>>(connection, &path, UNIT, "InvocationID").await? != id { return Ok(Observed::Pending); }
    if let Some(handle) = handle { return Ok(Observed::Running(Arc::new(handle), id)); }
    let state: String = property(connection, &path, UNIT, "ActiveState").await?;
    let job: (u32, OwnedObjectPath) = property(connection, &path, UNIT, "Job").await?;
    if job.0 == 0 && matches!(state.as_str(), "inactive" | "failed") { return Ok(Observed::Exited); }
    Ok(Observed::Pending)
}

async fn property<T>(connection: &Connection, path: &OwnedObjectPath, interface: &str, name: &str) -> Result<T, SessionError>
where T: TryFrom<OwnedVariant>, T::Error: Into<zbus::Error> {
    let reply = connection.call_method(Some(DESTINATION), path, Some("org.freedesktop.DBus.Properties"), "Get", &(interface, name)).await?;
    let value: OwnedVariant = reply.body().deserialize()?;
    value.try_into().map_err(|error: T::Error| SessionError::from(error.into()))
}

async fn connect() -> Result<Connection, SessionError> {
    tokio::time::timeout(Duration::from_secs(5), async {
        Ok(zbus::connection::Builder::session().method_timeout(Duration::from_secs(5)).build().await?)
    }).await.map_err(|_| SessionError::State("launcher observation bus connection timed out".into()))?
}

#[cfg(test)]
#[path = "session_wait_tests.rs"]
mod tests;
