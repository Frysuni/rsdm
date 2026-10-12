//! Authenticate an app process inside the caller's held IPC request slot.

use zbus::{Proxy, proxy::CacheProperties, zvariant::OwnedObjectPath};

use super::{processes::{self, ProcessHandle}, runtime::{self, AppRecord}};

pub(super) async fn current(manager: &Proxy<'_>, app: &AppRecord, pid: u32, handle: &ProcessHandle) -> zbus::Result<bool> {
    if !valid_invocation(&app.invocation_id) || !handle.alive()?
        || super::app_lifetime::owns_process(app, pid, handle)? { return Ok(false); }
    let unit = unit_proxy(manager, &app.unit, "Unit").await?;
    if !current_application(manager, &unit, app).await? { return Ok(false); }
    Ok(current_process(manager, app, pid, handle).await?
        && current_application(manager, &unit, app).await? && handle.alive()?)
}

async fn current_process(manager: &Proxy<'_>, app: &AppRecord, pid: u32, handle: &ProcessHandle) -> zbus::Result<bool> {
    let service = unit_proxy(manager, &app.unit, "Service").await?;
    if service.get_property::<u32>("ControlPID").await? == pid { return Ok(false); }
    let processes: Vec<(String, u32, String)> = manager.call("GetUnitProcesses", &(&app.unit,)).await?;
    if processes.iter().any(|(_, candidate, _)| *candidate == pid) {
        let group: String = service.get_property("ControlGroup").await?;
        // GetUnitProcesses also includes a service's escaped MainPID.
        if group_contains(group, pid, handle).await? { return Ok(true); }
    }
    let Some(scope) = super::app_scope::registered_scope(app)
        .map_err(|error| zbus::fdo::Error::Failed(error.to_string()))? else { return Ok(false); };
    if !scope.unit.ends_with(".scope") || !valid_invocation(&scope.invocation_id) { return Ok(false); }
    let unit = unit_proxy(manager, &scope.unit, "Unit").await?;
    if unit.get_property::<Vec<u8>>("InvocationID").await? != scope.invocation_id { return Ok(false); }
    let processes: Vec<(String, u32, String)> = manager.call("GetUnitProcesses", &(&scope.unit,)).await?;
    if !processes.iter().any(|(_, candidate, _)| *candidate == pid) { return Ok(false); }
    let scope_service = unit_proxy(manager, &scope.unit, "Scope").await?;
    let group: String = scope_service.get_property("ControlGroup").await?;
    Ok(group_contains(group, pid, handle).await?
        && super::app_scope::owns_process(scope, pid, handle).await.map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?
        && unit.get_property::<Vec<u8>>("InvocationID").await? == scope.invocation_id)
}

async fn group_contains(group: String, pid: u32, handle: &ProcessHandle) -> zbus::Result<bool> {
    if !group.starts_with('/') || group == "/" { return Ok(false); }
    let membership = runtime::blocking(move || Ok(std::fs::read_to_string(format!("/proc/{pid}/cgroup"))?)).await
        .map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;
    Ok(processes::belongs_to_cgroup(&membership, &group) && handle.alive()?)
}

async fn current_application(manager: &Proxy<'_>, unit: &Proxy<'_>, app: &AppRecord) -> zbus::Result<bool> {
    let invocation: Vec<u8> = unit.get_property("InvocationID").await?;
    if invocation == app.invocation_id { return Ok(true); }
    if app.migrated_scope.is_none() || invocation.iter().any(|byte| *byte != 0) { return Ok(false); }
    let state: String = unit.get_property("ActiveState").await?;
    if !matches!(state.as_str(), "inactive" | "failed") { return Ok(false); }
    let processes: Vec<(String, u32, String)> = manager.call("GetUnitProcesses", &(&app.unit,)).await?;
    let mut pids = processes.into_iter().map(|(_, pid, _)| pid).collect();
    super::app_lifetime::exclude_process(app, &mut pids)
        .map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;
    Ok(pids.is_empty())
}

pub(super) async fn unit_proxy<'a>(manager: &Proxy<'a>, unit: &str, interface: &str) -> zbus::Result<Proxy<'a>> {
    let path: OwnedObjectPath = manager.call("GetUnit", &(unit,)).await?;
    // These reads share the caller's held slot and original bus connection.
    zbus::proxy::Builder::<Proxy<'_>>::new(manager.connection()).destination("org.freedesktop.systemd1")?.path(path)?
        .interface(format!("org.freedesktop.systemd1.{interface}"))?.cache_properties(CacheProperties::No).build().await
}

pub(super) fn valid_invocation(invocation: &[u8]) -> bool {
    !invocation.is_empty() && invocation.iter().any(|byte| *byte != 0)
}
