//! Keep the endpoint and its channel alive across a user-bus restart.

use std::time::{Duration, Instant};

use futures_lite::future;

use super::*;

pub(in crate::session_manager) struct Server {
    connection: Connection,
    endpoint: Endpoint,
    checked: Instant,
}

impl Server {
    pub fn start(endpoint: Endpoint) -> Result<Self, SessionError> {
        let connection = serve(endpoint.clone())?;
        Ok(Self::with_connection(connection, endpoint))
    }

    pub fn with_connection(connection: Connection, endpoint: Endpoint) -> Self {
        Self { connection, endpoint, checked: Instant::now() }
    }

    pub fn maintain(&mut self) {
        if !self.connection.is_bus() || self.checked.elapsed() < Duration::from_secs(1) { return; }
        self.checked = Instant::now();
        let health = async_io::block_on(async {
            let bus = zbus::Proxy::new(&self.connection, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus").await?;
            bus.call::<_, _, String>("GetId", &()).await
        });
        if health.is_ok() { return; }

        match serve(self.endpoint.clone()) {
            Ok(connection) => {
                self.connection = connection;
                tracing::info!("restored session control endpoint");
            }
            Err(error) => tracing::warn!(%error, "deferring session control reconnection"),
        }
    }
}

fn serve(endpoint: Endpoint) -> Result<Connection, SessionError> {
    async_io::block_on(future::or(async {
        Ok(zbus::connection::Builder::session()?
            .method_timeout(Duration::from_secs(5))
            .allow_name_replacements(false).replace_existing_names(false)
            .serve_at(OBJECT_PATH, endpoint)?.name(BUS_NAME)?.build().await?)
    }, async {
        async_io::Timer::after(Duration::from_secs(5)).await;
        Err(zbus::Error::from(zbus::fdo::Error::TimedOut("connecting session control timed out".into())).into())
    }))
}
