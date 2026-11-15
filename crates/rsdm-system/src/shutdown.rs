//! Read-only shutdown diagnostics; observing never acquires an inhibitor.

use std::{future::Future, time::Duration};

use zbus::{Connection, OwnedObjectPath, Proxy, proxy::{CacheProperties, MethodFlags}};

#[derive(Debug, Clone)]
pub struct Job {
    pub id: u32,
    pub unit: String,
    pub kind: String,
    pub state: String,
}

#[derive(Debug, Clone)]
pub struct Inhibitor {
    pub who: String,
    pub why: String,
    pub mode: String,
    pub pid: u32,
}

#[derive(Debug, Clone)]
pub struct LogindStatus {
    pub preparing: bool,
    pub inhibitors: Vec<Inhibitor>,
}

pub async fn connect() -> zbus::Result<Connection> {
    bounded(zbus::connection::Builder::system().method_timeout(Duration::from_secs(2)).build()).await
}

pub async fn jobs(connection: &Connection) -> zbus::Result<Vec<Job>> {
    bounded(async {
        let proxy = diagnostic_proxy(connection, "org.freedesktop.systemd1", "/org/freedesktop/systemd1", "org.freedesktop.systemd1.Manager").await?;
        let jobs: Vec<(u32, String, String, String, OwnedObjectPath, OwnedObjectPath)> = proxy
            .call_with_flags("ListJobs", MethodFlags::NO_AUTO_START, &()).await?.ok_or(zbus::Error::InvalidReply)?;
        Ok(jobs.into_iter().map(|(id, unit, kind, state, _, _)| Job { id, unit, kind, state }).collect())
    }).await
}

pub async fn logind_status(connection: &Connection) -> zbus::Result<LogindStatus> {
    bounded(async {
        let interface = "org.freedesktop.login1.Manager";
        let proxy = diagnostic_proxy(connection, "org.freedesktop.login1", "/org/freedesktop/login1", interface).await?;
        let inhibitors: Vec<(String, String, String, String, u32, u32)> = proxy
            .call_with_flags("ListInhibitors", MethodFlags::NO_AUTO_START, &()).await?.ok_or(zbus::Error::InvalidReply)?;
        let properties = diagnostic_proxy(connection, "org.freedesktop.login1", "/org/freedesktop/login1", "org.freedesktop.DBus.Properties").await?;
        let value: zbus::OwnedVariant = properties.call_with_flags("Get", MethodFlags::NO_AUTO_START,
            &(interface, "PreparingForShutdown")).await?.ok_or(zbus::Error::InvalidReply)?;
        let preparing = bool::try_from(value)?;
        let inhibitors = inhibitors.into_iter().filter(|(what, ..)| what.split(':').any(|part| part == "shutdown"))
            .map(|(_, who, why, mode, _, pid)| Inhibitor { who, why, mode, pid }).collect();
        Ok(LogindStatus { preparing, inhibitors })
    }).await
}

async fn diagnostic_proxy<'a>(connection: &'a Connection, destination: &'a str, path: &'a str, interface: &'a str) -> zbus::Result<Proxy<'a>> {
    zbus::proxy::Builder::new(connection).destination(destination).path(path).interface(interface)
        .cache_properties(CacheProperties::No).build().await
}

async fn bounded<T>(operation: impl Future<Output = zbus::Result<T>>) -> zbus::Result<T> {
    tokio::time::timeout(Duration::from_secs(2), operation).await
        .map_err(|_| zbus::Error::from(zbus::fdo::Error::TimedOut("shutdown diagnostics timed out".into())))?
}

#[cfg(test)]
#[path = "shutdown_tests.rs"]
mod tests;
