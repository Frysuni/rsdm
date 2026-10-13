//! Invocation checks run inside the same request slot as the guarded operation.

use super::*;

#[derive(Clone)]
pub(super) struct UnitOwner {
    unit: String,
    invocation: Vec<u8>,
}

impl UserManager {
    pub(crate) fn with_owner(&self, unit: &str, invocation: &[u8]) -> Self {
        let mut manager = self.clone();
        manager.owner = Some(UnitOwner { unit: unit.into(), invocation: invocation.into() });
        manager
    }

    pub(crate) async fn check_owner(&self) -> zbus::Result<()> {
        if let Some(owner) = &self.owner {
            verify(&self.connection(), &owner.unit, &owner.invocation).await?;
        }
        self.actions.check().map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;
        self.deadline.remaining(Duration::MAX)?;
        Ok(())
    }

    pub(crate) async fn verify_owner(&self, unit: &str, invocation: &[u8]) -> Result<bool, SessionError> {
        let running = self.read(|| async { verify(&self.connection(), unit, invocation).await }).await?;
        if running { self.transport.remember(unit, invocation); }
        Ok(running)
    }
}

async fn verify(connection: &Connection, unit: &str, expected: &[u8]) -> zbus::Result<bool> {
    let manager = Proxy::new(connection, DESTINATION, MANAGER_PATH, MANAGER_INTERFACE).await?;
    let path: OwnedObjectPath = match manager.call("GetUnit", &(unit,)).await {
        Ok(path) => path,
        Err(error) if missing_unit(&error) => return Ok(false),
        Err(error) => return Err(error),
    };
    let proxy = zbus::proxy::Builder::<Proxy<'_>>::new(connection).destination(DESTINATION)?.path(path)?
        .interface("org.freedesktop.systemd1.Unit")?.cache_properties(zbus::proxy::CacheProperties::No).build().await?;
    let current: Vec<u8> = proxy.get_property("InvocationID").await?;
    if expected.iter().any(|byte| *byte != 0) && current == expected { return Ok(true); }
    if current.iter().any(|byte| *byte != 0) { return Err(ownership_error(unit)); }
    let state: String = proxy.get_property("ActiveState").await?;
    if matches!(state.as_str(), "active" | "reloading") { return Err(ownership_error(unit)); }
    let processes: Vec<(String, u32, String)> = manager.call("GetUnitProcesses", &(unit,)).await?;
    if !processes.is_empty() { return Err(ownership_error(unit)); }
    let latest: Vec<u8> = proxy.get_property("InvocationID").await?;
    if latest.iter().all(|byte| *byte == 0) { return Ok(false); }
    Err(ownership_error(unit))
}

fn ownership_error(unit: &str) -> zbus::Error {
    zbus::fdo::Error::Failed(format!("{unit} invocation changed or was never verified")).into()
}
