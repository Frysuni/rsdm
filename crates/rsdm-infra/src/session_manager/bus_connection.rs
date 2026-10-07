//! Clones share reconnections and the references owned by this bus peer.

use std::{collections::BTreeMap, future::Future, pin::Pin, sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}}};

use futures_lite::future;

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
    pending: usize,
    retained: bool,
    claim: Arc<()>,
}

impl Reference {
    fn new(generation: String) -> Self {
        Self { generation, invocation: None, pending: 0, retained: false, claim: Arc::new(()) }
    }
}

pub(super) struct ReferenceIntent {
    transport: Transport,
    unit: String,
    claim: Arc<()>,
    epoch: u64,
    submitted: bool,
    finished: bool,
}

impl ReferenceIntent {
    pub fn submitted(&mut self) { self.submitted = true; }

    pub fn reject(mut self) -> bool {
        let rejected = self.transport.settle_reference(&self.unit, &self.claim, self.epoch, false);
        self.finished = true;
        rejected
    }
}

impl Drop for ReferenceIntent {
    fn drop(&mut self) {
        if !self.finished {
            self.transport.settle_reference(&self.unit, &self.claim, self.epoch, self.submitted);
        }
    }
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

    #[cfg(test)]
    pub fn has_reference(&self, unit: &str) -> bool {
        self.0.lock().unwrap().references.contains_key(unit)
    }

    pub fn stage_reference(&self, unit: &str, generation: String) -> ReferenceIntent {
        let mut state = self.0.lock().unwrap_or_else(|error| error.into_inner());
        let epoch = state.epoch;
        let reference = state.references.entry(unit.into()).or_insert_with(|| Reference::new(generation));
        reference.pending += 1;
        let claim = Arc::clone(&reference.claim);
        state.revision += 1;
        ReferenceIntent { transport: self.clone(), unit: unit.into(), claim, epoch, submitted: false, finished: false }
    }

    fn settle_reference(&self, unit: &str, claim: &Arc<()>, epoch: u64, submitted: bool) -> bool {
        let mut state = self.0.lock().unwrap_or_else(|error| error.into_inner());
        // A published replacement peer may already hold a restored RefUnit.
        // Preserve its ownership until ordinary cleanup rather than forgetting it.
        let rejected = !submitted && state.epoch == epoch;
        let Some(reference) = state.references.get_mut(unit).filter(|reference| Arc::ptr_eq(&reference.claim, claim)) else {
            return rejected;
        };
        reference.pending -= 1;
        reference.retained |= !rejected;
        if reference.pending == 0 && !reference.retained && reference.invocation.is_none() {
            state.references.remove(unit);
        }
        state.revision += 1;
        rejected
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
        for batch in snapshot.references.iter().collect::<Vec<_>>().chunks(REFERENCE_RESTORE_BATCH) {
            restore_reference_batch(connection, batch).await?;
        }
        restore_activation(connection, snapshot.activation_environment).await?;

        let mut state = self.0.lock().unwrap_or_else(|error| error.into_inner());
        if state.epoch != observed_epoch || state.revision != snapshot.revision { return Ok(false); }
        state.connection = connection.clone();
        state.epoch += 1;
        Ok(true)
    }
}

const REFERENCE_RESTORE_BATCH: usize = 8;

async fn restore_reference_batch(
    connection: &Connection,
    references: &[(&String, &Reference)],
) -> zbus::Result<()> {
    let mut combined: Pin<Box<dyn Future<Output = zbus::Result<()>> + Send + '_>> =
        Box::pin(async { Ok(()) });
    for (unit, reference) in references {
        let connection = connection.clone();
        let unit = (*unit).clone();
        let reference = (*reference).clone();
        combined = Box::pin(async move {
            let ((), ()) = future::try_zip(combined, restore_reference(&connection, &unit, &reference)).await?;
            Ok(())
        });
    }
    combined.await
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
