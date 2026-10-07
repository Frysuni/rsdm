//! Clones share reconnections and the references owned by this bus peer.

use std::{collections::BTreeMap, sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}}};

use super::*;

#[derive(Clone)]
pub(super) struct Transport(Arc<Mutex<State>>, Arc<AtomicBool>);

#[derive(Clone)]
struct State {
    connection: Connection,
    epoch: u64,
    revision: u64,
    references: BTreeMap<String, Reference>,
    activation_environment: BTreeMap<String, String>,
}

struct ConnectionGuard(Arc<AtomicBool>);

impl Drop for ConnectionGuard {
    fn drop(&mut self) { self.0.store(false, Ordering::Release); }
}

#[derive(Clone)]
struct Reference {
    generation: String,
    invocation: Option<Vec<u8>>,
}

impl Transport {
    pub fn new(connection: Connection) -> Self {
        Self(Arc::new(Mutex::new(State {
            connection, epoch: 0, revision: 0, references: BTreeMap::new(), activation_environment: BTreeMap::new(),
        })), Arc::new(AtomicBool::new(false)))
    }

    pub fn connection(&self) -> Connection {
        self.0.lock().unwrap_or_else(|error| error.into_inner()).connection.clone()
    }

    pub fn epoch(&self) -> u64 {
        self.0.lock().unwrap_or_else(|error| error.into_inner()).epoch
    }

    pub fn track(&self, unit: &str, generation: String) {
        let mut state = self.0.lock().unwrap_or_else(|error| error.into_inner());
        state.references.insert(
            unit.into(), Reference { generation, invocation: None },
        );
        state.revision += 1;
    }

    pub fn remember(&self, unit: &str, invocation: &[u8]) {
        let mut state = self.0.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(reference) = state.references.get_mut(unit) {
            if reference.invocation.is_none() {
                reference.invocation = Some(invocation.to_vec());
                state.revision += 1;
            }
        }
    }

    pub fn forget(&self, unit: &str) {
        let mut state = self.0.lock().unwrap_or_else(|error| error.into_inner());
        if state.references.remove(unit).is_some() { state.revision += 1; }
    }

    pub async fn update_activation(&self, pairs: &[(String, String)]) -> zbus::Result<()> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let _guard = self.acquire_connection_guard(deadline).await?;
        {
            let mut state = self.0.lock().unwrap_or_else(|error| error.into_inner());
            state.activation_environment.extend(pairs.iter().cloned());
            state.revision += 1;
        }
        let connection = self.connection();
        let values: BTreeMap<&str, &str> = pairs.iter().map(|(name, value)| (name.as_str(), value.as_str())).collect();
        future::or(async {
            let bus = Proxy::new(&connection, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus").await?;
            bus.call::<_, _, ()>("UpdateActivationEnvironment", &(values,)).await
        }, async {
            async_io::Timer::at(deadline).await;
            Err(zbus::fdo::Error::TimedOut("activation environment update timed out".into()).into())
        }).await
    }

    pub async fn reconnect(&self, observed_epoch: u64) -> zbus::Result<()> {
        if self.epoch() != observed_epoch { return Ok(()); }
        let deadline = Instant::now() + Duration::from_secs(5);
        let _guard = self.acquire_connection_guard(deadline).await?;
        while self.epoch() == observed_epoch {
            let connection = future::or(async {
                zbus::connection::Builder::session()?.method_timeout(Duration::from_secs(5)).build().await
            }, async {
                async_io::Timer::at(deadline).await;
                Err(zbus::fdo::Error::TimedOut("connecting the user bus timed out".into()).into())
            }).await?;
            let restored = future::or(self.restore_candidate(&connection, observed_epoch), async {
                async_io::Timer::at(deadline).await;
                Err(zbus::fdo::Error::TimedOut("restoring the user bus timed out".into()).into())
            }).await;
            if matches!(restored, Ok(true)) {
                tracing::info!("restored user manager connection");
                return Ok(());
            }
            // A failed or outdated candidate may hold per-peer RefUnit counts.
            // Close it before retrying instead of publishing stale ownership.
            let _ = connection.close().await;
            restored?;
        }
        Ok(())
    }

    async fn acquire_connection_guard(&self, deadline: Instant) -> zbus::Result<ConnectionGuard> {
        loop {
            if Instant::now() >= deadline {
                return Err(zbus::fdo::Error::TimedOut("waiting for user bus maintenance timed out".into()).into());
            }
            if self.1.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_ok() {
                return Ok(ConnectionGuard(self.1.clone()));
            }
            async_io::Timer::at((Instant::now() + Duration::from_millis(10)).min(deadline)).await;
        }
    }

    async fn restore_candidate(&self, connection: &Connection, observed_epoch: u64) -> zbus::Result<bool> {
        let snapshot = self.0.lock().unwrap_or_else(|error| error.into_inner()).clone();
        if snapshot.epoch != observed_epoch { return Ok(false); }
        validate_connection(connection).await?;
        for (unit, reference) in &snapshot.references {
            restore_reference(connection, unit, reference).await?;
        }
        restore_activation(connection, snapshot.activation_environment).await?;

        let mut state = self.0.lock().unwrap_or_else(|error| error.into_inner());
        if state.epoch != observed_epoch || state.revision != snapshot.revision { return Ok(false); }
        state.connection = connection.clone();
        state.epoch += 1;
        Ok(true)
    }
}

async fn restore_activation(connection: &Connection, pairs: BTreeMap<String, String>) -> zbus::Result<()> {
    if pairs.is_empty() { return Ok(()); }
    let manager = Proxy::new(connection, DESTINATION, MANAGER_PATH, MANAGER_INTERFACE).await?;
    let current: Vec<String> = manager.get_property("Environment").await?;
    let owned: BTreeMap<String, String> = pairs.into_iter()
        .filter(|(name, value)| current.contains(&format!("{name}={value}"))).collect();
    if owned.is_empty() { return Ok(()); }
    let bus = Proxy::new(connection, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus").await?;
    bus.call::<_, _, ()>("UpdateActivationEnvironment", &(owned,)).await
}

async fn restore_reference(connection: &Connection, unit: &str, reference: &Reference) -> zbus::Result<()> {
    let manager = Proxy::new(connection, DESTINATION, MANAGER_PATH, MANAGER_INTERFACE).await?;
    let path: OwnedObjectPath = match manager.call("GetUnit", &(unit,)).await {
        Ok(path) => path,
        Err(error) if missing_unit(&error) => return Ok(()),
        Err(error) => return Err(error),
    };
    let proxy = zbus::proxy::Builder::<Proxy<'_>>::new(connection).destination(DESTINATION)?.path(path.clone())?
        .interface("org.freedesktop.systemd1.Unit")?.cache_properties(zbus::proxy::CacheProperties::No).build().await?;
    let id: Vec<u8> = proxy.get_property("InvocationID").await?;
    if id.iter().all(|byte| *byte == 0) || reference.invocation.as_ref().is_some_and(|expected| *expected != id)
        || proxy.get_property::<String>("Id").await? != unit || !proxy.get_property::<bool>("Transient").await?
    { return Ok(()); }
    let service = zbus::proxy::Builder::<Proxy<'_>>::new(connection).destination(DESTINATION)?.path(path)?
        .interface("org.freedesktop.systemd1.Service")?.cache_properties(zbus::proxy::CacheProperties::No).build().await?;
    let environment: Vec<String> = service.get_property("Environment").await?;
    if !environment.contains(&format!("RSDM_SESSION_GENERATION={}", reference.generation)) { return Ok(()); }

    manager.call::<_, _, ()>("RefUnit", &(unit,)).await?;
    if proxy.get_property::<Vec<u8>>("InvocationID").await? != id {
        manager.call::<_, _, ()>("UnrefUnit", &(unit,)).await?;
    }
    Ok(())
}

pub(super) fn disconnected(error: &zbus::Error) -> bool {
    match error {
        zbus::Error::InputOutput(error) => matches!(error.kind(),
            std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::ConnectionAborted | std::io::ErrorKind::NotConnected
            | std::io::ErrorKind::UnexpectedEof),
        zbus::Error::FDO(error) => matches!(**error, zbus::fdo::Error::Disconnected(_)),
        zbus::Error::MethodError(name, _, _) => name.as_str() == "org.freedesktop.DBus.Error.Disconnected",
        _ => false,
    }
}

#[cfg(test)]
#[path = "bus_connection_tests.rs"]
mod tests;
