//! Bind read-only queries to the existing unique owner, without service activation.

use std::time::Duration;

use rsdm_core::domain::SessionManagerConfig;
use zbus::{Connection, names::OwnedUniqueName};

use crate::{SessionError, control::{self, SessionStatus}, status::SessionStatusDetails};

pub async fn status(cfg: &SessionManagerConfig) -> Result<SessionStatus, SessionError> {
    if !cfg.enabled { return Ok(SessionStatus::disabled()); }
    let Some(connection) = connection().await? else { return Ok(SessionStatusDetails::inactive().status); };
    let Some(owner) = coordinator_owner(&connection).await? else { return Ok(SessionStatusDetails::inactive().status); };
    let proxy = zbus::Proxy::new(&connection, owner.as_str(), control::OBJECT_PATH, control::BUS_NAME).await?;
    Ok(proxy.call("Status", &()).await?)
}

pub async fn status_details(cfg: &SessionManagerConfig) -> Result<SessionStatusDetails, SessionError> {
    if !cfg.enabled { return Ok(SessionStatusDetails::disabled()); }
    let Some(connection) = connection().await? else { return Ok(SessionStatusDetails::inactive()); };
    query_details(&connection).await
}

async fn connection() -> Result<Option<Connection>, SessionError> {
    if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none() && std::env::var_os("XDG_RUNTIME_DIR").is_none() { return Ok(None); }
    let connected = tokio::time::timeout(Duration::from_secs(5),
        zbus::connection::Builder::session()?.method_timeout(Duration::from_secs(5)).build()).await
        .map_err(|_| SessionError::State("session status connection timed out".into()))?;
    match connected {
        Ok(connection) => Ok(Some(connection)),
        Err(zbus::Error::InputOutput(error)) if matches!(error.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

async fn query_details(connection: &Connection) -> Result<SessionStatusDetails, SessionError> {
    let Some(owner) = coordinator_owner(connection).await? else { return Ok(SessionStatusDetails::inactive()); };
    let proxy = zbus::Proxy::new(connection, owner.as_str(), control::OBJECT_PATH, control::BUS_NAME).await?;
    match proxy.call::<_, _, SessionStatusDetails>("StatusDetails", &()).await {
        Ok(details) => Ok(details),
        Err(zbus::Error::MethodError(name, _, _)) if name.as_str() == "org.freedesktop.DBus.Error.UnknownMethod" => {
            Ok(SessionStatusDetails::legacy(proxy.call("Status", &()).await?))
        }
        Err(error) => Err(error.into()),
    }
}

async fn coordinator_owner(connection: &Connection) -> Result<Option<OwnedUniqueName>, SessionError> {
    let reply = connection.call_method(Some("org.freedesktop.DBus"), "/org/freedesktop/DBus", Some("org.freedesktop.DBus"), "GetNameOwner", &(control::BUS_NAME,)).await;
    match reply {
        Ok(reply) => Ok(Some(reply.body().deserialize()?)),
        Err(zbus::Error::MethodError(name, _, _)) if name.as_str() == "org.freedesktop.DBus.Error.NameHasNoOwner" => Ok(None),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
#[path = "client_status_tests.rs"]
mod tests;
