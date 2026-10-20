//! Read-only identification of the native desktop's compositor process.

use std::{os::fd::AsFd, sync::Arc};

use zbus::{Connection, fdo::ConnectionCredentials};

use super::{processes::ProcessHandle, provider::{Provider, ProviderKind}};

pub(super) async fn pin(connection: &Connection, provider: &Provider) -> zbus::Result<Option<Arc<ProcessHandle>>> {
    let name = match provider.kind {
        ProviderKind::Gnome => "org.gnome.Shell",
        ProviderKind::Plasma => "org.kde.KWin",
        _ => return Ok(None),
    };
    let Some(unique_owner) = owner(connection, name).await? else { return Ok(None); };
    let reply = connection.call_method(Some("org.freedesktop.DBus"), "/org/freedesktop/DBus",
        Some("org.freedesktop.DBus"), "GetConnectionCredentials", &(&unique_owner,)).await?;
    let credentials: ConnectionCredentials = reply.body().deserialize()?;
    let pid = credentials.process_id();
    let uid = credentials.unix_user_id();
    if uid != Some(unsafe { libc::geteuid() })
        || (credentials.process_fd().is_none() && pid.is_none_or(|pid| pid == 0))
    {
        return Err(zbus::fdo::Error::AccessDenied("native compositor credentials do not identify a process of this user".into()).into());
    }
    let handle = match credentials.process_fd() {
        Some(descriptor) => Some(ProcessHandle::from_pidfd(descriptor.as_fd().try_clone_to_owned()?)),
        None => ProcessHandle::open(pid.unwrap())?,
    };
    if owner(connection, name).await?.as_ref() != Some(&unique_owner) { return Ok(None); }
    Ok(handle.map(Arc::new))
}

async fn owner(connection: &Connection, name: &str) -> zbus::Result<Option<String>> {
    match connection.call_method(Some("org.freedesktop.DBus"), "/org/freedesktop/DBus",
        Some("org.freedesktop.DBus"), "GetNameOwner", &(name,)).await
    {
        Ok(reply) => Ok(Some(reply.body().deserialize()?)),
        Err(zbus::Error::FDO(error)) if matches!(*error, zbus::fdo::Error::NameHasNoOwner(_)) => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
#[path = "native_compositor_tests.rs"]
mod tests;
