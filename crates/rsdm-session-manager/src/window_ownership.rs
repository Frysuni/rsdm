//! Revalidate window ownership while the final IPC request slot is held.

use zbus::{Connection, Proxy, proxy::CacheProperties, zvariant::OwnedObjectPath};

use super::{processes::{self, ProcessHandle}, runtime::{self, AppRecord, SessionRecord}};

pub(super) async fn current(connection: &Connection, record: &SessionRecord, app: &AppRecord,
    compositor_pid: u32, pid: u32, handle: &ProcessHandle) -> zbus::Result<bool> {
    let Some(compositor) = &record.compositor_unit else { return Ok(false); };
    if !valid_invocation(&record.compositor_invocation) || !valid_invocation(&app.invocation_id) || !handle.alive()? { return Ok(false); }
    let manager = Proxy::new(connection, "org.freedesktop.systemd1", "/org/freedesktop/systemd1", "org.freedesktop.systemd1.Manager").await?;
    let compositor_unit = unit_proxy(&manager, compositor, "Unit").await?;
    let app_unit = unit_proxy(&manager, &app.unit, "Unit").await?;
    let service = unit_proxy(&manager, compositor, "Service").await?;
    if compositor_unit.get_property::<Vec<u8>>("InvocationID").await? != record.compositor_invocation
        || app_unit.get_property::<Vec<u8>>("InvocationID").await? != app.invocation_id
        || service.get_property::<u32>("MainPID").await? != compositor_pid
    { return Ok(false); }
    if !current_process(&manager, app, pid, handle).await? { return Ok(false); }
    Ok(service.get_property::<u32>("MainPID").await? == compositor_pid
        && compositor_unit.get_property::<Vec<u8>>("InvocationID").await? == record.compositor_invocation
        && app_unit.get_property::<Vec<u8>>("InvocationID").await? == app.invocation_id
        && handle.alive()?)
}

async fn current_process(manager: &Proxy<'_>, app: &AppRecord, pid: u32, handle: &ProcessHandle) -> zbus::Result<bool> {
    let service = unit_proxy(manager, &app.unit, "Service").await?;
    if service.get_property::<u32>("ControlPID").await? == pid { return Ok(false); }
    let processes: Vec<(String, u32, String)> = manager.call("GetUnitProcesses", &(&app.unit,)).await?;
    if !processes.iter().any(|(_, candidate, _)| *candidate == pid) { return Ok(false); }
    let group: String = service.get_property("ControlGroup").await?;
    if !group.starts_with('/') || group == "/" { return Ok(false); }
    let membership = runtime::blocking(move || Ok(std::fs::read_to_string(format!("/proc/{pid}/cgroup"))?)).await
        .map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;
    Ok(processes::belongs_to_cgroup(&membership, &group) && handle.alive()?)
}

async fn unit_proxy<'a>(manager: &Proxy<'a>, unit: &str, interface: &str) -> zbus::Result<Proxy<'a>> {
    let path: OwnedObjectPath = manager.call("GetUnit", &(unit,)).await?;
    // These reads share the caller's held slot and original bus connection.
    zbus::proxy::Builder::<Proxy<'_>>::new(manager.connection()).destination("org.freedesktop.systemd1")?.path(path)?
        .interface(format!("org.freedesktop.systemd1.{interface}"))?.cache_properties(CacheProperties::No).build().await
}

fn valid_invocation(invocation: &[u8]) -> bool {
    !invocation.is_empty() && invocation.iter().any(|byte| *byte != 0)
}
