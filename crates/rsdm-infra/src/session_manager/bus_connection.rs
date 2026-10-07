//! Clones share reconnections and the references owned by this bus peer.

use std::{collections::BTreeMap, sync::{Arc, Mutex}};

use super::*;

#[derive(Clone)]
pub(super) struct Transport(Arc<Mutex<State>>);

struct State {
    connection: Connection,
    epoch: u64,
    references: BTreeMap<String, Reference>,
    activation_environment: BTreeMap<String, String>,
}

#[derive(Clone)]
struct Reference {
    generation: String,
    invocation: Option<Vec<u8>>,
}

impl Transport {
    pub fn new(connection: Connection) -> Self {
        Self(Arc::new(Mutex::new(State {
            connection, epoch: 0, references: BTreeMap::new(), activation_environment: BTreeMap::new(),
        })))
    }

    pub fn connection(&self) -> Connection {
        self.0.lock().unwrap_or_else(|error| error.into_inner()).connection.clone()
    }

    pub fn epoch(&self) -> u64 {
        self.0.lock().unwrap_or_else(|error| error.into_inner()).epoch
    }

    pub fn track(&self, unit: &str, generation: String) {
        self.0.lock().unwrap_or_else(|error| error.into_inner()).references.insert(
            unit.into(), Reference { generation, invocation: None },
        );
    }

    pub fn remember(&self, unit: &str, invocation: &[u8]) {
        if let Some(reference) = self.0.lock().unwrap_or_else(|error| error.into_inner()).references.get_mut(unit) {
            if reference.invocation.is_none() { reference.invocation = Some(invocation.to_vec()); }
        }
    }

    pub fn forget(&self, unit: &str) {
        self.0.lock().unwrap_or_else(|error| error.into_inner()).references.remove(unit);
    }

    pub fn remember_activation(&self, pairs: &[(String, String)]) {
        self.0.lock().unwrap_or_else(|error| error.into_inner()).activation_environment.extend(pairs.iter().cloned());
    }

    pub async fn reconnect(&self, observed_epoch: u64) -> zbus::Result<()> {
        let mut state = self.0.lock().unwrap_or_else(|error| error.into_inner());
        if state.epoch != observed_epoch { return Ok(()); }
        let references = state.references.clone();
        let activation_environment = state.activation_environment.clone();
        let connection = future::or(async {
            let connection = zbus::connection::Builder::session()?
                .method_timeout(Duration::from_secs(5)).build().await?;
            let result = async {
                validate_connection(&connection).await?;
                for (unit, reference) in references {
                    restore_reference(&connection, &unit, &reference).await?;
                }
                restore_activation(&connection, activation_environment).await?;
                Ok::<(), zbus::Error>(())
            }.await;
            if let Err(error) = result {
                // RefUnit is counted per peer. Closing a failed candidate
                // prevents a later attempt from accumulating references.
                let _ = connection.close().await;
                return Err(error);
            }
            Ok(connection)
        }, async {
            async_io::Timer::after(Duration::from_secs(5)).await;
            Err(zbus::fdo::Error::TimedOut("reconnecting the user bus timed out".into()).into())
        }).await?;
        state.connection = connection;
        state.epoch += 1;
        tracing::info!("restored user manager connection");
        Ok(())
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
