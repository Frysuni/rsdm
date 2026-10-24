//! Read the terminal result of the compositor invocation that was observed.

use zbus::{Connection, OwnedObjectPath, Proxy};

pub(super) async fn read(connection: &Connection, unit: &str, expected: &[u8]) -> zbus::Result<Option<i32>> {
    let manager = Proxy::new(connection, "org.freedesktop.systemd1", "/org/freedesktop/systemd1",
        "org.freedesktop.systemd1.Manager").await?;
    let path: OwnedObjectPath = manager.call("GetUnit", &(unit,)).await?;
    let unit = zbus::proxy::Builder::<Proxy<'_>>::new(connection).destination("org.freedesktop.systemd1").path(path.clone())
        .interface("org.freedesktop.systemd1.Unit").cache_properties(zbus::proxy::CacheProperties::No).build().await?;
    let id: Vec<u8> = unit.get_property("InvocationID").await?;
    if !id.iter().any(|byte| *byte != 0) || id != expected {
        return Err(zbus::fdo::Error::Failed("compositor exit status belongs to an unconfirmed invocation".into()).into());
    }
    let state: String = unit.get_property("ActiveState").await?;
    if !matches!(state.as_str(), "inactive" | "failed") { return Ok(None); }

    let service = zbus::proxy::Builder::<Proxy<'_>>::new(connection).destination("org.freedesktop.systemd1").path(path)
        .interface("org.freedesktop.systemd1.Service").cache_properties(zbus::proxy::CacheProperties::No).build().await?;
    let result: String = service.get_property("Result").await?;
    let code: i32 = service.get_property("ExecMainCode").await?;
    let status: i32 = service.get_property("ExecMainStatus").await?;
    if unit.get_property::<Vec<u8>>("InvocationID").await? != id {
        return Err(zbus::fdo::Error::Failed("compositor invocation changed while its exit status was read".into()).into());
    }
    let exit = match code {
        libc::CLD_EXITED if (0..=255).contains(&status) => status,
        libc::CLD_KILLED | libc::CLD_DUMPED if (1..128).contains(&status) => 128 + status,
        0 if result != "success" => 1,
        _ => return Err(zbus::fdo::Error::Failed("compositor exited but its main process status is unconfirmed".into()).into()),
    };
    Ok(Some(if exit == 0 && result != "success" { 1 } else { exit }))
}
