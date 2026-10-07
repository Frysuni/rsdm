//! Typed user-manager operations and completion of the exact systemd job.

use std::{future::Future, time::{Duration, Instant}};

use futures_lite::future;
use zbus::{Connection, Proxy, zvariant::{OwnedObjectPath, OwnedValue, Value}};

use super::SessionError;

#[path = "bus_jobs.rs"]
mod jobs;

#[path = "bus_connection.rs"]
mod transport;

#[cfg(test)]
use jobs::wait_job;

const DESTINATION: &str = "org.freedesktop.systemd1";
const MANAGER_PATH: &str = "/org/freedesktop/systemd1";
const MANAGER_INTERFACE: &str = "org.freedesktop.systemd1.Manager";

pub(super) type UnitProperties = Vec<(&'static str, Value<'static>)>;

#[derive(Clone)]
pub(super) struct UserManager {
    transport: transport::Transport,
}

impl UserManager {
    pub fn connect() -> Result<Self, SessionError> {
        async_io::block_on(async {
            let connection = zbus::connection::Builder::session()?
                .method_timeout(Duration::from_secs(5))
                .build().await?;
            Self::from_connection(connection).await
        })
    }

    async fn from_connection(connection: Connection) -> Result<Self, SessionError> {
        validate_connection(&connection).await?;
        Ok(Self::with_connection(connection))
    }

    pub(super) fn with_connection(connection: Connection) -> Self {
        Self { transport: transport::Transport::new(connection) }
    }

    pub(super) fn connection(&self) -> Connection {
        self.transport.connection()
    }

    async fn proxy(&self) -> Result<Proxy<'static>, zbus::Error> {
        Proxy::new_owned(self.connection(), DESTINATION.to_string(), MANAGER_PATH.to_string(), MANAGER_INTERFACE.to_string()).await
    }

    pub fn start_service(&self, unit: &str, properties: &UnitProperties) -> Result<(), SessionError> {
        async_io::block_on(jobs::start(self, unit, properties, Duration::from_secs(10)))
    }

    pub fn stop(&self, unit: &str, timeout: Duration) -> Result<(), SessionError> {
        async_io::block_on(jobs::stop(self, unit, timeout))
    }

    pub fn unref(&self, unit: &str) -> Result<(), SessionError> {
        self.transport.forget(unit);
        async_io::block_on(async {
            match self.proxy().await?.call::<_, _, ()>("UnrefUnit", &(unit,)).await {
                Ok(()) => Ok(()),
                Err(error) if missing_unit(&error) => Ok(()),
                Err(error) => Err(error.into()),
            }
        })
    }

    pub fn active(&self, unit: &str) -> Result<bool, SessionError> {
        match self.unit_property::<String>(unit, "org.freedesktop.systemd1.Unit", "ActiveState") {
            Ok(state) => Ok(matches!(state.as_str(), "active" | "reloading")),
            Err(SessionError::Bus(error)) if missing_unit(&error) => Ok(false),
            Err(error) => Err(error),
        }
    }

    pub fn unit_property<T>(&self, unit: &str, interface: &'static str, name: &str) -> Result<T, SessionError>
    where
        T: TryFrom<OwnedValue>,
        T::Error: Into<zbus::Error>,
    {
        self.read(|| async {
            let connection = self.connection();
            let manager = Proxy::new(&connection, DESTINATION, MANAGER_PATH, MANAGER_INTERFACE).await?;
            let path: OwnedObjectPath = manager.call("GetUnit", &(unit,)).await?;
            let proxy = zbus::proxy::Builder::<Proxy<'_>>::new(&connection).destination(DESTINATION)?.path(path)?
                .interface(interface)?.cache_properties(zbus::proxy::CacheProperties::No).build().await?;
            proxy.get_property(name).await
        })
    }

    pub fn invocation_id(&self, unit: &str) -> Result<Vec<u8>, SessionError> {
        let id: Vec<u8> = self.unit_property(unit, "org.freedesktop.systemd1.Unit", "InvocationID")?;
        self.transport.remember(unit, &id);
        Ok(id)
    }

    #[cfg(feature = "xsmp")]
    pub fn unit_for_pid(&self, pid: u32) -> Result<String, SessionError> {
        self.read(|| async {
            let connection = self.connection();
            let path: OwnedObjectPath = self.proxy().await?.call("GetUnitByPID", &(pid,)).await?;
            let unit = Proxy::new(&connection, DESTINATION, path, "org.freedesktop.systemd1.Unit").await?;
            unit.get_property("Id").await
        })
    }

    pub fn processes(&self, unit: &str) -> Result<Vec<(String, u32, String)>, SessionError> {
        self.read(|| async {
            match self.proxy().await?.call("GetUnitProcesses", &(unit,)).await {
                Ok(processes) => Ok(processes),
                Err(error) if missing_unit(&error) => Ok(Vec::new()),
                Err(error) => Err(error),
            }
        })
    }

    pub fn environment(&self) -> Result<Vec<String>, SessionError> {
        self.read(|| async { self.proxy().await?.get_property("Environment").await })
    }

    pub(super) fn remember_activation(&self, pairs: &[(String, String)]) {
        self.transport.remember_activation(pairs);
    }

    pub(super) fn read<T, F, Fut>(&self, query: F) -> Result<T, SessionError>
    where
        F: Fn() -> Fut,
        Fut: Future<Output = zbus::Result<T>>,
    {
        async_io::block_on(self.read_async(query, Duration::from_secs(5))).map_err(Into::into)
    }

    async fn read_async<T, F, Fut>(&self, query: F, timeout: Duration) -> zbus::Result<T>
    where
        F: Fn() -> Fut,
        Fut: Future<Output = zbus::Result<T>>,
    {
        retry_read(|| async {
            let epoch = self.transport.epoch();
            let result = query().await;
            if result.as_ref().is_err_and(transport::disconnected) {
                self.transport.reconnect(epoch).await?;
            }
            result
        }, timeout).await
    }

    pub fn set_environment(&self, values: &[String]) -> Result<(), SessionError> {
        async_io::block_on(async {
            self.proxy().await?.call::<_, _, ()>("SetEnvironment", &(values,)).await?;
            Ok(())
        })
    }

    pub fn unset_environment(&self, names: &[String]) -> Result<(), SessionError> {
        async_io::block_on(async {
            self.proxy().await?.call::<_, _, ()>("UnsetEnvironment", &(names,)).await?;
            Ok(())
        })
    }
}

async fn validate_connection(connection: &Connection) -> zbus::Result<()> {
    let proxy = Proxy::new(connection, DESTINATION, MANAGER_PATH, MANAGER_INTERFACE).await?;
    let version: String = proxy.get_property("Version").await?;
    let version_number = version.split(|ch: char| !ch.is_ascii_digit())
        .next().and_then(|number| number.parse::<u32>().ok());
    if !version_number.is_some_and(|number| number >= 250) {
        return Err(zbus::Error::Failure(format!("systemd 250 or newer is required (found {version})")));
    }
    proxy.call::<_, _, ()>("Subscribe", &()).await
}

// Only read operations may be replayed: a lost method reply does not undo a
// start, stop, power request or reference-count change.
async fn retry_read<T, F, Fut>(query: F, timeout: Duration) -> zbus::Result<T>
where
    F: Fn() -> Fut,
    Fut: Future<Output = zbus::Result<T>>,
{
    let deadline = Instant::now() + timeout;
    let mut interrupted = false;
    loop {
        let result = future::or(query(), async {
            async_io::Timer::at(deadline).await;
            Err(zbus::fdo::Error::TimedOut("user manager read timed out".into()).into())
        }).await;
        match result {
            Err(error) if retryable_error(&error) && Instant::now() < deadline => {
                if !interrupted {
                    tracing::warn!(%error, "user manager temporarily unavailable; retrying read");
                    interrupted = true;
                }
            }
            other => return other,
        }
        async_io::Timer::at((Instant::now() + Duration::from_millis(50)).min(deadline)).await;
    }
}

pub(super) fn retryable_error(error: &zbus::Error) -> bool {
    match error {
        zbus::Error::MethodError(name, _, _) => matches!(name.as_str(),
            "org.freedesktop.DBus.Error.NoReply" | "org.freedesktop.DBus.Error.Disconnected"
            | "org.freedesktop.DBus.Error.ServiceUnknown" | "org.freedesktop.DBus.Error.NameHasNoOwner"
            | "org.freedesktop.DBus.Error.Timeout" | "org.freedesktop.DBus.Error.TimedOut"
            | "org.freedesktop.DBus.Error.UnknownObject"),
        zbus::Error::FDO(error) => matches!(**error, zbus::fdo::Error::NoReply(_)
            | zbus::fdo::Error::Disconnected(_) | zbus::fdo::Error::ServiceUnknown(_)
            | zbus::fdo::Error::NameHasNoOwner(_) | zbus::fdo::Error::Timeout(_)
            | zbus::fdo::Error::TimedOut(_) | zbus::fdo::Error::UnknownObject(_)),
        zbus::Error::InputOutput(error) => matches!(error.kind(),
            std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::ConnectionAborted | std::io::ErrorKind::NotConnected
            | std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::TimedOut
            | std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound),
        _ => false,
    }
}

pub(super) fn missing_unit(error: &zbus::Error) -> bool {
    matches!(error, zbus::Error::MethodError(name, _, _)
        if name.as_str() == "org.freedesktop.systemd1.NoSuchUnit")
}

#[cfg(test)]
#[path = "bus_tests.rs"]
mod tests;
