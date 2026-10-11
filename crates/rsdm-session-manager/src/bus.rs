//! Typed user-manager operations and completion of the exact systemd job.

use std::{future::Future, sync::atomic::{AtomicBool, Ordering}, time::{Duration, Instant}};

use zbus::{Connection, Proxy, zvariant::{OwnedObjectPath, OwnedValue, Value}};

use super::{SessionError, deadline::Deadline};

#[path = "bus_jobs.rs"]
mod jobs;

#[path = "bus_connection.rs"]
mod transport;

#[path = "bus_activation.rs"]
mod activation;

const DESTINATION: &str = "org.freedesktop.systemd1";
const MANAGER_PATH: &str = "/org/freedesktop/systemd1";
const MANAGER_INTERFACE: &str = "org.freedesktop.systemd1.Manager";

pub(super) type UnitProperties = Vec<(&'static str, Value<'static>)>;

#[derive(Clone)]
pub(super) struct UserManager {
    transport: transport::Transport,
    pub(super) deadline: Deadline,
    pub(super) actions: super::action_gate::ActionGate,
}

impl UserManager {
    pub async fn connect_until(deadline: Deadline) -> Result<Self, SessionError> {
        Self::connect_with(async {
            zbus::connection::Builder::session()?.method_timeout(Duration::from_secs(5)).build().await
        }, deadline).await
    }

    pub(super) async fn connect_guarded(deadline: Deadline, actions: super::action_gate::ActionGate) -> Result<Self, SessionError> {
        let mut manager = actions.guard(Self::connect_until(deadline)).await??;
        actions.check()?;
        manager.actions = actions;
        Ok(manager)
    }

    async fn connect_with(
        connection: impl Future<Output = zbus::Result<Connection>>, deadline: Deadline,
    ) -> Result<Self, SessionError> {
        let setup = async {
            let connection = connection.await?;
            validate_connection(&connection).await?;
            Ok(Self { transport: transport::Transport::new(connection), deadline: deadline.clone(), actions: Default::default() })
        };
        deadline.bound(async {
            tokio::select! {
                biased;
                result = setup => result,
                _ = tokio::time::sleep(Duration::from_secs(5)) => {
                    Err(zbus::Error::from(zbus::fdo::Error::TimedOut("connecting to the user manager timed out".into())).into())
                },
            }
        }).await
    }

    #[cfg(test)]
    async fn from_connection(connection: Connection) -> Result<Self, SessionError> {
        Self::connect_with(async { Ok(connection) }, Deadline::default()).await
    }

    #[cfg(test)]
    pub(super) fn with_connection(connection: Connection) -> Self {
        Self { transport: transport::Transport::new(connection), deadline: Deadline::default(), actions: Default::default() }
    }

    pub(super) fn connection(&self) -> Connection {
        self.transport.connection()
    }

    async fn proxy(&self) -> Result<Proxy<'static>, zbus::Error> {
        Proxy::new_owned(self.connection(), DESTINATION.to_string(), MANAGER_PATH.to_string(), MANAGER_INTERFACE.to_string()).await
    }

    pub(super) async fn request<T>(&self, operation: impl Future<Output = zbus::Result<T>>) -> zbus::Result<T> {
        // Job and recovery futures compose these requests; keep their state
        // off the caller's stack instead of growing every enclosing future.
        self.actions.guard(self.deadline.bound(Box::pin(self.transport.request(async {
            self.actions.check_generation_async().await.map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;
            // The deadline can change while this request waits for a slot.
            self.deadline.remaining(Duration::MAX)?;
            operation.await
        })))).await.map_err(|error| zbus::Error::from(zbus::fdo::Error::Failed(error.to_string())))?
    }

    pub async fn start_service(&self, unit: &str, properties: &UnitProperties) -> Result<(), SessionError> {
        let submitted = AtomicBool::new(false);
        self.start_service_tracked(unit, properties, &submitted).await
    }

    pub(super) async fn start_service_tracked(&self, unit: &str, properties: &UnitProperties, submitted: &AtomicBool) -> Result<(), SessionError> {
        let result = self.deadline.bound(jobs::start(self, unit, properties, Duration::from_secs(10), submitted)).await;
        match result {
            Err(error) if !submitted.load(Ordering::Relaxed) => Err(SessionError::StartRejected { error: Box::new(error), reference_retained: false }),
            other => other,
        }
    }

    pub async fn stop(&self, unit: &str, timeout: Duration) -> Result<(), SessionError> {
        self.deadline.bound(jobs::stop(self, unit, timeout)).await
    }

    pub async fn unref(&self, unit: &str) -> Result<(), SessionError> {
        self.request(async {
            self.transport.forget(unit);
            match self.proxy().await?.call::<_, _, ()>("UnrefUnit", &(unit,)).await {
                Ok(()) => Ok(()),
                Err(error) if missing_unit(&error) => Ok(()),
                Err(error) => Err(error),
            }
        }).await.map_err(Into::into)
    }

    pub async fn active(&self, unit: &str) -> Result<bool, SessionError> {
        match self.unit_property::<String>(unit, "org.freedesktop.systemd1.Unit", "ActiveState").await {
            Ok(state) => Ok(matches!(state.as_str(), "active" | "reloading")),
            Err(SessionError::Bus(error)) if missing_unit(&error) => Ok(false),
            Err(error) => Err(error),
        }
    }

    pub async fn unit_property<T>(&self, unit: &str, interface: &'static str, name: &str) -> Result<T, SessionError>
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
        }).await
    }

    pub async fn invocation_id(&self, unit: &str) -> Result<Vec<u8>, SessionError> {
        let id: Vec<u8> = self.unit_property(unit, "org.freedesktop.systemd1.Unit", "InvocationID").await?;
        self.transport.remember(unit, &id);
        Ok(id)
    }

    #[cfg(feature = "xsmp")]
    pub async fn unit_for_pid(&self, pid: u32) -> Result<String, SessionError> {
        self.read(|| async {
            let connection = self.connection();
            let path: OwnedObjectPath = self.proxy().await?.call("GetUnitByPID", &(pid,)).await?;
            let unit = Proxy::new(&connection, DESTINATION, path, "org.freedesktop.systemd1.Unit").await?;
            unit.get_property("Id").await
        }).await
    }

    pub async fn processes(&self, unit: &str) -> Result<Vec<(String, u32, String)>, SessionError> {
        self.read(|| async {
            match self.proxy().await?.call("GetUnitProcesses", &(unit,)).await {
                Ok(processes) => Ok(processes),
                Err(error) if missing_unit(&error) => Ok(Vec::new()),
                Err(error) => Err(error),
            }
        }).await
    }

    pub async fn environment(&self) -> Result<Vec<String>, SessionError> {
        self.read(|| async { self.proxy().await?.get_property("Environment").await }).await
    }

    pub(super) async fn update_activation(&self, pairs: &[(String, String)]) -> Result<(), SessionError> {
        self.actions.guard(self.deadline.bound(self.transport.update_activation(pairs))).await?.map_err(Into::into)
    }

    pub(super) async fn read<T, F, Fut>(&self, query: F) -> Result<T, SessionError>
    where
        F: Fn() -> Fut,
        Fut: Future<Output = zbus::Result<T>>,
    {
        self.read_async(query, Duration::from_secs(5)).await.map_err(Into::into)
    }

    async fn read_async<T, F, Fut>(&self, query: F, timeout: Duration) -> zbus::Result<T>
    where
        F: Fn() -> Fut,
        Fut: Future<Output = zbus::Result<T>>,
    {
        self.deadline.bound(retry_read(|| async {
            let epoch = self.transport.epoch();
            let result = self.request(query()).await;
            if result.as_ref().is_err_and(transport::disconnected) {
                self.actions.guard(self.transport.reconnect(epoch)).await
                    .map_err(|error| zbus::fdo::Error::Failed(error.to_string()))??;
            }
            result
        }, timeout)).await
    }

    pub async fn set_environment(&self, values: &[String]) -> Result<(), SessionError> {
        self.request(async {
            self.proxy().await?.call::<_, _, ()>("SetEnvironment", &(values,)).await?;
            Ok(())
        }).await.map_err(Into::into)
    }

    pub async fn unset_environment(&self, names: &[String]) -> Result<(), SessionError> {
        self.request(async {
            self.proxy().await?.call::<_, _, ()>("UnsetEnvironment", &(names,)).await?;
            Ok(())
        }).await.map_err(Into::into)
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
        let result = tokio::select! {
            biased;
            result = query() => result,
            _ = tokio::time::sleep_until(deadline.into()) => {
                Err(zbus::fdo::Error::TimedOut("user manager read timed out".into()).into())
            },
        };
        match result {
            Err(error) if retryable_error(&error) && Instant::now() < deadline => {
                if !interrupted {
                    tracing::warn!(%error, "user manager temporarily unavailable; retrying read");
                    interrupted = true;
                }
            }
            other => return other,
        }
        tokio::time::sleep_until((Instant::now() + Duration::from_millis(50)).min(deadline).into()).await;
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

#[cfg(test)]
#[path = "bus_setup_tests.rs"]
mod setup_tests;

#[cfg(test)]
#[path = "bus_concurrency_tests.rs"]
mod concurrency_tests;
