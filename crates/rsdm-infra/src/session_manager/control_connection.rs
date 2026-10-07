//! Keep the endpoint and its channel alive across a user-bus restart.

use std::{thread, time::{Duration, Instant}};

use futures_lite::future;

use super::*;

type Maintenance = Result<Option<Connection>, SessionError>;

pub(in crate::session_manager) struct Server {
    connection: Connection,
    endpoint: Endpoint,
    checked: Instant,
    pending: Option<async_channel::Receiver<Maintenance>>,
}

impl Server {
    pub fn start(endpoint: Endpoint) -> Result<Self, SessionError> {
        let connection = async_io::block_on(serve(endpoint.clone()))?;
        Ok(Self::with_connection(connection, endpoint))
    }

    pub fn with_connection(connection: Connection, endpoint: Endpoint) -> Self {
        Self { connection, endpoint, checked: Instant::now(), pending: None }
    }

    pub fn maintain(&mut self) {
        if let Some(pending) = &self.pending {
            let result = match pending.try_recv() {
                Ok(result) => result,
                Err(async_channel::TryRecvError::Empty) => return,
                Err(async_channel::TryRecvError::Closed) => Err(SessionError::State("control maintenance worker stopped".into())),
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
        let (completed, result) = async_channel::bounded(1);
        let spawned = thread::Builder::new().name("rsdm-control-bus".into()).spawn(move || {
            async_io::block_on(future::or(async {
                let result = maintain_connection(connection, endpoint).await;
                let _ = completed.try_send(result);
            }, async {
                // Dropping the server must cancel a pending registration, not
                // create a replacement endpoint after the coordinator exits.
                completed.closed().await;
            }));
        });
        match spawned {
            Ok(_) => self.pending = Some(result),
            Err(error) => tracing::warn!(%error, "could not start control maintenance worker"),
        }
    }
}

async fn maintain_connection(connection: Connection, endpoint: Endpoint) -> Maintenance {
    let health = future::or(async {
        let bus = zbus::Proxy::new(&connection, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus").await?;
        bus.call::<_, _, String>("GetId", &()).await.map(|_| ())
    }, async {
        async_io::Timer::after(Duration::from_secs(5)).await;
        Err(zbus::Error::from(zbus::fdo::Error::TimedOut("control health check timed out".into())))
    }).await;
    if health.is_ok() { return Ok(None); }
    serve(endpoint).await.map(Some)
}

async fn serve(endpoint: Endpoint) -> Result<Connection, SessionError> {
    future::or(async {
        Ok(zbus::connection::Builder::session()?
            .method_timeout(Duration::from_secs(5))
            .allow_name_replacements(false).replace_existing_names(false)
            .serve_at(OBJECT_PATH, endpoint)?.name(BUS_NAME)?.build().await?)
    }, async {
        async_io::Timer::after(Duration::from_secs(5)).await;
        Err(zbus::Error::from(zbus::fdo::Error::TimedOut("connecting session control timed out".into())).into())
    }).await
}

#[cfg(test)]
#[path = "control_connection_tests.rs"]
mod tests;
