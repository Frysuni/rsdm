//! Logind power authorization and delay inhibitors, without privilege escalation.

use std::{future::Future, os::fd::OwnedFd, time::Duration};

use thiserror::Error;
use zbus::{Connection, Proxy};

pub use monitor::{ShutdownMonitor, ShutdownNotice};
mod monitor;

#[derive(Debug, Error)]
pub enum LogindError {
    #[error("logind I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("logind bus error: {0}")]
    Bus(#[from] zbus::Error),
    #[error("{0}")]
    State(String),
}

const DESTINATION: &str = "org.freedesktop.login1";
const PATH: &str = "/org/freedesktop/login1";
const INTERFACE: &str = "org.freedesktop.login1.Manager";

async fn connection(timeout: Duration) -> Result<Connection, LogindError> {
    Ok(zbus::connection::Builder::system().method_timeout(timeout).build().await?)
}

async fn proxy(connection: &Connection) -> Result<Proxy<'_>, zbus::Error> {
    Proxy::new(connection, DESTINATION, PATH, INTERFACE).await
}

fn method(action: &str) -> Result<&'static str, LogindError> {
    match action {
        "reboot" => Ok("Reboot"), "poweroff" => Ok("PowerOff"),
        "suspend" => Ok("Suspend"), "hibernate" => Ok("Hibernate"),
        _ => Err(LogindError::State("unsupported power action".into())),
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PowerCapabilities {
    pub hibernate: bool,
    pub suspend: bool,
}

/// Query the same logind authority used for executing sleep requests.
pub async fn capabilities() -> Result<PowerCapabilities, LogindError> {
    let timeout = Duration::from_secs(5);
    query_capabilities(connection(timeout), timeout).await
}

async fn query_capabilities(
    connect: impl Future<Output = Result<Connection, LogindError>>, timeout: Duration,
) -> Result<PowerCapabilities, LogindError> {
    tokio::time::timeout(timeout, async {
        let connection = connect.await?;
        let proxy = proxy(&connection).await?;
        let (hibernate, suspend) = tokio::join!(
            available(&proxy, "CanHibernate"), available(&proxy, "CanSuspend"),
        );
        Ok(PowerCapabilities { hibernate, suspend })
    }).await.map_err(|_| LogindError::State("logind power capability query timed out".into()))?
}

async fn available(proxy: &Proxy<'_>, method: &str) -> bool {
    match proxy.call::<_, _, String>(method, &()).await {
        Ok(result) => matches!(result.as_str(), "yes" | "challenge"),
        Err(error) => {
            tracing::warn!(%method, %error, "logind power capability unavailable");
            false
        }
    }
}

pub async fn check(action: &str) -> Result<(), LogindError> {
    let timeout = Duration::from_secs(5);
    check_with(connection(timeout), action, timeout).await
}

async fn check_with(
    connect: impl Future<Output = Result<Connection, LogindError>>, action: &str, timeout: Duration,
) -> Result<(), LogindError> {
    let method = format!("Can{}", method(action)?);
    within_timeout(async {
        let connection = connect.await?;
        let result: String = proxy(&connection).await?.call(method.as_str(), &()).await?;
        if matches!(result.as_str(), "yes" | "challenge") { return Ok(()); }
        Err(LogindError::State(format!("logind does not allow {action}: {result}")))
    }, timeout).await
}

pub async fn request_direct(action: &str) -> Result<(), LogindError> {
    let timeout = Duration::from_secs(60);
    request_with(connection(timeout), action, timeout).await
}

async fn request_with(
    connect: impl Future<Output = Result<Connection, LogindError>>, action: &str, timeout: Duration,
) -> Result<(), LogindError> {
    let method = method(action)?;
    within_timeout(async {
        let connection = connect.await?;
        proxy(&connection).await?.call::<_, _, ()>(method, &(true,)).await?;
        Ok(())
    }, timeout).await
}

async fn within_timeout<T>(
    operation: impl Future<Output = Result<T, LogindError>>, timeout: Duration,
) -> Result<T, LogindError> {
    tokio::time::timeout(timeout, operation).await.map_err(|_| {
        LogindError::Bus(zbus::fdo::Error::TimedOut("logind request timed out".into()).into())
    })?
}

async fn inhibit(connection: &Connection, reason: &str) -> Result<OwnedFd, LogindError> {
    let fd: zbus::OwnedFd = proxy(connection).await?
        .call("Inhibit", &("shutdown", "RSDM", reason, "delay")).await?;
    Ok(fd.into())
}

/// Hold a delay inhibitor until the caller has completed its shutdown work.
pub async fn delay_inhibitor(reason: &str) -> Result<OwnedFd, LogindError> {
    let timeout = Duration::from_secs(5);
    within_timeout(async {
        let connection = connection(timeout).await?;
        inhibit(&connection, reason).await
    }, timeout).await
}

#[cfg(test)]
#[path = "power_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "power_capability_tests.rs"]
mod capability_tests;

#[cfg(test)]
#[path = "power_request_tests.rs"]
mod request_tests;
