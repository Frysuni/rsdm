//! Revalidate window ownership while the final IPC request slot is held.

use zbus::{Connection, Proxy};

use super::{app_ownership::{self, unit_proxy, valid_invocation}, processes::ProcessHandle, runtime::{AppRecord, SessionRecord}};

pub(super) async fn current(connection: &Connection, record: &SessionRecord, app: &AppRecord,
    compositor_pid: u32, pid: u32, handle: &ProcessHandle) -> zbus::Result<bool> {
    let Some(compositor) = &record.compositor_unit else { return Ok(false); };
    if !valid_invocation(&record.compositor_invocation) || !valid_invocation(&app.invocation_id) || !handle.alive()? { return Ok(false); }
    let manager = Proxy::new(connection, "org.freedesktop.systemd1", "/org/freedesktop/systemd1", "org.freedesktop.systemd1.Manager").await?;
    let compositor_unit = unit_proxy(&manager, compositor, "Unit").await?;
    let service = unit_proxy(&manager, compositor, "Service").await?;
    if compositor_unit.get_property::<Vec<u8>>("InvocationID").await? != record.compositor_invocation
        || service.get_property::<u32>("MainPID").await? != compositor_pid
    { return Ok(false); }
    if !app_ownership::current(&manager, app, pid, handle).await? { return Ok(false); }
    Ok(service.get_property::<u32>("MainPID").await? == compositor_pid
        && compositor_unit.get_property::<Vec<u8>>("InvocationID").await? == record.compositor_invocation
        && app_ownership::current(&manager, app, pid, handle).await?
        && handle.alive()?)
}
