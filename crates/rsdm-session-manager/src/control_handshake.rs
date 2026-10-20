//! Shared user units follow logind's primary graphical login.

use std::time::Duration;

use rsdm_core::domain::SessionPhase;
use zbus::{Connection, message::Header, zvariant::OwnedObjectPath};

use super::{SessionError, control_caller::{Caller, denied}, identity::SessionIdentity, runtime::{Runtime, SessionRecord}};

#[derive(Clone)]
pub(super) struct Verifier {
    pub(super) identity: SessionIdentity,
    #[cfg(test)]
    pub(super) context: Option<std::sync::Arc<super::control_test_authority::Context>>,
}

impl Verifier {
    pub(super) fn new(identity: SessionIdentity) -> Self {
        Self { identity, #[cfg(test)] context: None }
    }

    pub(super) async fn verify(&self, connection: &Connection, header: &Header<'_>) -> Result<String, SessionError> {
        tokio::time::timeout(Duration::from_secs(5), async {
            let caller = Caller::discover(connection, header, self.identity.uid).await?;
            let (runtime, record) = self.session().await?;
            let logind = self.logind().await?;
            if !caller.belongs_to_login(&logind, &self.identity).await? {
                if Caller::belongs_to_display(&logind, &self.identity).await? {
                    user_unit(connection, &caller).await?;
                } else {
                    registered_process(connection, &caller, &runtime, &record).await?;
                }
            }
            caller.check_alive()?;
            self.session().await?;
            Ok(self.identity.generation.clone())
        }).await.map_err(|_| SessionError::Bus(zbus::fdo::Error::TimedOut("verifying session membership timed out".into()).into()))?
    }

    async fn session(&self) -> Result<(Runtime, SessionRecord), SessionError> {
        let identity = self.identity.clone();
        #[cfg(test)]
        let context = self.context.clone();
        super::runtime::blocking(move || {
            #[cfg(test)]
            let runtime = match context {
                Some(context) => context.runtime.clone(),
                None => Runtime::open(&identity.generation)?.ok_or_else(|| denied("session is not published"))?,
            };
            #[cfg(not(test))]
            let runtime = Runtime::open(&identity.generation)?.ok_or_else(|| denied("session is not published"))?;
            let record = runtime.session()?;
            if record.identity != identity || record.disabled || record.phase == SessionPhase::Closed {
                return Err(denied("session is stale or disabled"));
            }
            Ok((runtime, record))
        }).await
    }

    async fn logind(&self) -> Result<Connection, SessionError> {
        #[cfg(test)]
        if let Some(context) = &self.context { return Ok(context.logind.clone()); }
        Ok(zbus::connection::Builder::system()?.method_timeout(Duration::from_secs(5)).build().await?)
    }
}

async fn registered_process(connection: &Connection, caller: &Caller, runtime: &Runtime, record: &SessionRecord) -> Result<(), SessionError> {
    let (name, invocation) = user_unit(connection, caller).await?;
    let expected = registered_invocation(runtime, record, &name).await?;
    if invocation != expected { return Err(denied("caller unit invocation has changed")); }
    Ok(())
}

async fn user_unit(connection: &Connection, caller: &Caller) -> Result<(String, Vec<u8>), SessionError> {
    let manager = zbus::Proxy::new(connection, "org.freedesktop.systemd1", "/org/freedesktop/systemd1", "org.freedesktop.systemd1.Manager").await?;
    let path: OwnedObjectPath = manager.call("GetUnitByPID", &(caller.pid,)).await?;
    let unit = zbus::proxy::Builder::<zbus::Proxy<'_>>::new(connection)
        .destination("org.freedesktop.systemd1")?.path(path)?
        .interface("org.freedesktop.systemd1.Unit")?.cache_properties(zbus::proxy::CacheProperties::No).build().await?;
    let name: String = unit.get_property("Id").await?;
    let invocation: Vec<u8> = unit.get_property("InvocationID").await?;
    if invocation.is_empty() || invocation.iter().all(|byte| *byte == 0) { return Err(denied("caller unit has no live invocation")); }
    let interface = if name.ends_with(".scope") { "org.freedesktop.systemd1.Scope" }
        else if name.ends_with(".service") { "org.freedesktop.systemd1.Service" }
        else { return Err(denied("caller is not in a user service or scope")); };
    let service = zbus::Proxy::new(connection, "org.freedesktop.systemd1", unit.path(), interface).await?;
    let group: String = service.get_property("ControlGroup").await?;
    let pid = caller.pid;
    let belongs = super::runtime::blocking(move || {
        let membership = std::fs::read_to_string(format!("/proc/{pid}/cgroup"))?;
        Ok(group.starts_with('/') && group != "/" && super::processes::belongs_to_cgroup(&membership, &group))
    }).await?;
    if !belongs || unit.get_property::<Vec<u8>>("InvocationID").await? != invocation {
        return Err(denied("caller is not in the registered unit invocation"));
    }
    caller.check_alive()?;
    Ok((name, invocation))
}

async fn registered_invocation(runtime: &Runtime, record: &SessionRecord, unit: &str) -> Result<Vec<u8>, SessionError> {
    let invocation = if record.compositor_unit.as_deref() == Some(unit) {
        compositor_invocation(runtime, record).await?
    } else if let Some((coordinator, invocation)) = &record.coordinator_unit
        && coordinator == unit
    {
        invocation.clone()
    } else {
        let runtime = runtime.clone();
        let unit = unit.to_string();
        super::runtime::blocking(move || Ok(runtime.app(&unit)?.invocation_id)).await?
    };
    if invocation.is_empty() || invocation.iter().all(|byte| *byte == 0) { return Err(denied("caller unit has no registered invocation")); }
    Ok(invocation)
}

async fn compositor_invocation(runtime: &Runtime, expected: &SessionRecord) -> Result<Vec<u8>, SessionError> {
    let mut record = expected.clone();
    loop {
        if record.identity != expected.identity || record.compositor_unit != expected.compositor_unit
            || record.disabled || record.phase == SessionPhase::Closed
        { return Err(denied("compositor registration has changed or ended")); }
        if !record.compositor_invocation.is_empty() || record.phase != SessionPhase::Starting {
            return Ok(record.compositor_invocation);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
        let runtime = runtime.clone();
        record = super::runtime::blocking(move || runtime.session()).await?;
    }
}

#[cfg(test)]
#[path = "control_handshake_tests.rs"]
mod tests;
