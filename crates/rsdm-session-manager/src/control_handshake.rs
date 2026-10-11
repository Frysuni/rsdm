//! Generation discovery requires a login or a registered unit invocation.

use std::time::Duration;

use rsdm_core::domain::SessionPhase;
use zbus::{Connection, message::Header, zvariant::OwnedObjectPath};

use super::{SessionError, control_caller::{Caller, denied}, identity::SessionIdentity, runtime::{Runtime, SessionRecord}};

pub(super) async fn verify(connection: &Connection, header: &Header<'_>, identity: &SessionIdentity) -> Result<String, SessionError> {
    tokio::time::timeout(Duration::from_secs(5), async {
        let caller = Caller::discover(connection, header, identity.uid).await?;
        let (runtime, record) = session(identity).await?;
        if !caller.belongs_to_login(identity).await? {
            registered_process(connection, &caller, &runtime, &record).await?;
        }
        caller.check_alive()?;
        session(identity).await?;
        Ok(identity.generation.clone())
    }).await.map_err(|_| SessionError::Bus(zbus::fdo::Error::TimedOut("session handshake timed out".into()).into()))?
}

async fn session(identity: &SessionIdentity) -> Result<(Runtime, SessionRecord), SessionError> {
    let identity = identity.clone();
    super::runtime::blocking(move || {
        let runtime = Runtime::open(&identity.generation)?.ok_or_else(|| denied("session is not published"))?;
        let record = runtime.session()?;
        if record.identity != identity || record.disabled || record.phase == SessionPhase::Closed {
            return Err(denied("session is stale or disabled"));
        }
        Ok((runtime, record))
    }).await
}

async fn registered_process(connection: &Connection, caller: &Caller, runtime: &Runtime, record: &SessionRecord) -> Result<(), SessionError> {
    let manager = zbus::Proxy::new(connection, "org.freedesktop.systemd1", "/org/freedesktop/systemd1", "org.freedesktop.systemd1.Manager").await?;
    let path: OwnedObjectPath = manager.call("GetUnitByPID", &(caller.pid,)).await?;
    let unit = zbus::proxy::Builder::<zbus::Proxy<'_>>::new(connection)
        .destination("org.freedesktop.systemd1")?.path(path)?
        .interface("org.freedesktop.systemd1.Unit")?.cache_properties(zbus::proxy::CacheProperties::No).build().await?;
    let name: String = unit.get_property("Id").await?;
    let expected = registered_invocation(runtime, record, &name).await?;
    let invocation: Vec<u8> = unit.get_property("InvocationID").await?;
    if invocation != expected { return Err(denied("caller unit invocation has changed")); }
    let service = zbus::Proxy::new(connection, "org.freedesktop.systemd1", unit.path(), "org.freedesktop.systemd1.Service").await?;
    let group: String = service.get_property("ControlGroup").await?;
    let pid = caller.pid;
    let belongs = super::runtime::blocking(move || {
        let membership = std::fs::read_to_string(format!("/proc/{pid}/cgroup"))?;
        Ok(group.starts_with('/') && group != "/" && super::processes::belongs_to_cgroup(&membership, &group))
    }).await?;
    if !belongs || unit.get_property::<Vec<u8>>("InvocationID").await? != invocation {
        return Err(denied("caller is not in the registered unit invocation"));
    }
    Ok(())
}

async fn registered_invocation(runtime: &Runtime, record: &SessionRecord, unit: &str) -> Result<Vec<u8>, SessionError> {
    let invocation = if record.compositor_unit.as_deref() == Some(unit) {
        record.compositor_invocation.clone()
    } else {
        let runtime = runtime.clone();
        let unit = unit.to_string();
        super::runtime::blocking(move || Ok(runtime.app(&unit)?.invocation_id)).await?
    };
    if invocation.is_empty() || invocation.iter().all(|byte| *byte == 0) { return Err(denied("caller unit has no registered invocation")); }
    Ok(invocation)
}

#[cfg(test)]
#[path = "control_handshake_tests.rs"]
mod tests;
