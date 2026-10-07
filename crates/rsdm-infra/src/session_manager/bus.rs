//! Typed user-manager operations and completion of the exact systemd job.

use std::time::Duration;

use futures_lite::{StreamExt, future};
use zbus::{Connection, Proxy, zvariant::{OwnedObjectPath, OwnedValue, Value}};

use super::SessionError;

const DESTINATION: &str = "org.freedesktop.systemd1";
const MANAGER_PATH: &str = "/org/freedesktop/systemd1";
const MANAGER_INTERFACE: &str = "org.freedesktop.systemd1.Manager";

pub(super) type UnitProperties = Vec<(&'static str, Value<'static>)>;

#[derive(Clone)]
pub(super) struct UserManager {
    pub connection: Connection,
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
        let manager = Self { connection };
        let proxy = manager.proxy().await?;
        let version: String = proxy.get_property("Version").await?;
        let version_number = version.split(|ch: char| !ch.is_ascii_digit())
            .next().and_then(|number| number.parse::<u32>().ok());
        if !version_number.is_some_and(|number| number >= 250) {
            return Err(SessionError::State(format!("systemd 250 or newer is required (found {version})")));
        }
        proxy.call::<_, _, ()>("Subscribe", &()).await?;
        Ok(manager)
    }

    async fn proxy(&self) -> Result<Proxy<'_>, zbus::Error> {
        Proxy::new(&self.connection, DESTINATION, MANAGER_PATH, MANAGER_INTERFACE).await
    }

    pub fn start_service(&self, unit: &str, properties: &UnitProperties) -> Result<(), SessionError> {
        async_io::block_on(async {
            let proxy = self.proxy().await?;
            let mut signals = proxy.receive_signal_with_args("JobRemoved", &[(2, unit)]).await?;
            let auxiliary: Vec<(&str, UnitProperties)> = Vec::new();
            let job: OwnedObjectPath = proxy.call("StartTransientUnit", &(unit, "fail", properties, auxiliary)).await?;
            wait_job(&mut signals, &job, Duration::from_secs(10)).await
        })
    }

    pub fn stop(&self, unit: &str, timeout: Duration) -> Result<(), SessionError> {
        async_io::block_on(async {
            let proxy = self.proxy().await?;
            let mut signals = proxy.receive_signal_with_args("JobRemoved", &[(2, unit)]).await?;
            let job = match proxy.call::<_, _, OwnedObjectPath>("StopUnit", &(unit, "replace")).await {
                Ok(job) => job,
                Err(error) if missing_unit(&error) => return Ok(()),
                Err(error) => return Err(error.into()),
            };
            wait_job(&mut signals, &job, timeout).await
        })
    }

    pub fn unref(&self, unit: &str) -> Result<(), SessionError> {
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
        async_io::block_on(async {
            let path: OwnedObjectPath = self.proxy().await?.call("GetUnit", &(unit,)).await?;
            let proxy = Proxy::new(&self.connection, DESTINATION, path, interface).await?;
            Ok(proxy.get_property(name).await?)
        })
    }

    pub fn invocation_id(&self, unit: &str) -> Result<Vec<u8>, SessionError> {
        self.unit_property(unit, "org.freedesktop.systemd1.Unit", "InvocationID")
    }

    #[cfg(feature = "xsmp")]
    pub fn unit_for_pid(&self, pid: u32) -> Result<String, SessionError> {
        async_io::block_on(async {
            let path: OwnedObjectPath = self.proxy().await?.call("GetUnitByPID", &(pid,)).await?;
            let unit = Proxy::new(&self.connection, DESTINATION, path, "org.freedesktop.systemd1.Unit").await?;
            Ok(unit.get_property("Id").await?)
        })
    }

    pub fn processes(&self, unit: &str) -> Result<Vec<(String, u32, String)>, SessionError> {
        async_io::block_on(async {
            match self.proxy().await?.call("GetUnitProcesses", &(unit,)).await {
                Ok(processes) => Ok(processes),
                Err(error) if missing_unit(&error) => Ok(Vec::new()),
                Err(error) => Err(error.into()),
            }
        })
    }

    pub fn environment(&self) -> Result<Vec<String>, SessionError> {
        async_io::block_on(async { Ok(self.proxy().await?.get_property("Environment").await?) })
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

async fn wait_job(
    signals: &mut zbus::proxy::SignalStream<'_>,
    job: &OwnedObjectPath,
    timeout: Duration,
) -> Result<(), SessionError> {
    future::or(async {
        while let Some(message) = signals.next().await {
            let (_, path, unit, result): (u32, OwnedObjectPath, String, String) = message.body().deserialize()?;
            if path == *job {
                return if result == "done" {
                    Ok(())
                } else {
                    Err(SessionError::State(format!("{unit} job completed with {result}")))
                };
            }
        }
        Err(SessionError::State("systemd job signal stream ended".into()))
    }, async {
        async_io::Timer::after(timeout).await;
        Err(SessionError::State(format!("timed out waiting for systemd job {job}")))
    }).await
}

pub(super) fn missing_unit(error: &zbus::Error) -> bool {
    matches!(error, zbus::Error::MethodError(name, _, _)
        if name.as_str() == "org.freedesktop.systemd1.NoSuchUnit")
}

#[cfg(test)]
#[path = "bus_tests.rs"]
mod tests;
