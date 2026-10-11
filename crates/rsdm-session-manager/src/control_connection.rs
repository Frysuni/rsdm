//! Keep the endpoint and its channel alive across a user-bus restart.

use std::time::{Duration, Instant};

use tokio::{sync::oneshot, task::JoinHandle};

use super::*;

type MaintenanceResult = Result<Option<Connection>, SessionError>;

struct Maintenance {
    result: oneshot::Receiver<MaintenanceResult>,
    worker: JoinHandle<()>,
}

impl Drop for Maintenance {
    fn drop(&mut self) {
        // A pending registration must not publish a replacement endpoint after
        // the coordinator has dropped its control server.
        self.worker.abort();
    }
}

pub(crate) struct Server {
    connection: Connection,
    endpoint: Endpoint,
    checked: Instant,
    pending: Option<Maintenance>,
}

impl Server {
    pub async fn start(endpoint: Endpoint) -> Result<Self, SessionError> {
        let connection = serve(endpoint.clone()).await?;
        Ok(Self::with_connection(connection, endpoint))
    }

    pub fn with_connection(connection: Connection, endpoint: Endpoint) -> Self {
        Self { connection, endpoint, checked: Instant::now(), pending: None }
    }

    pub fn close_admission(&mut self) {
        self.endpoint.admission.close();
        self.pending = None;
    }

    pub fn maintain(&mut self) {
        if let Some(pending) = &mut self.pending {
            let result = match pending.result.try_recv() {
                Ok(result) => result,
                Err(oneshot::error::TryRecvError::Empty) => return,
                Err(oneshot::error::TryRecvError::Closed) => Err(SessionError::State("control maintenance worker stopped".into())),
            };
            self.pending = None;
            self.checked = Instant::now();
            match result {
                Ok(Some(connection)) => {
                    self.connection = connection;
                    tracing::info!("restored session control endpoint");
                }
                Ok(None) => {},
                Err(error) => tracing::warn!(%error, "deferring session control reconnection"),
            }
        }
        if !self.connection.is_bus() || self.checked.elapsed() < Duration::from_secs(1) { return; }
        self.start_maintenance();
    }

    fn start_maintenance(&mut self) {
        self.checked = Instant::now();
        let connection = self.connection.clone();
        let endpoint = self.endpoint.clone();
        let (completed, result) = oneshot::channel();
        let worker = tokio::spawn(async move {
            let result = maintain_connection(connection, endpoint).await;
            let _ = completed.send(result);
        });
        self.pending = Some(Maintenance { result, worker });
    }
}

async fn maintain_connection(connection: Connection, endpoint: Endpoint) -> MaintenanceResult {
    let health = tokio::time::timeout(Duration::from_secs(5), async {
        let bus = zbus::Proxy::new(&connection, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus").await?;
        bus.call::<_, _, String>("GetId", &()).await.map(|_| ())
    }).await;
    if matches!(health, Ok(Ok(()))) { return Ok(None); }
    serve(endpoint).await.map(Some)
}

async fn serve(endpoint: Endpoint) -> Result<Connection, SessionError> {
    tokio::time::timeout(Duration::from_secs(5), async {
        Ok(zbus::connection::Builder::session()?
            .method_timeout(Duration::from_secs(5))
            .allow_name_replacements(false).replace_existing_names(false)
            .serve_at(OBJECT_PATH, endpoint)?.name(BUS_NAME)?.build().await?)
    }).await.map_err(|_| zbus::Error::from(zbus::fdo::Error::TimedOut("connecting session control timed out".into())))?
}

#[cfg(test)]
#[path = "control_connection_tests.rs"]
mod tests;
