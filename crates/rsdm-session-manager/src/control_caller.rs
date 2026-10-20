//! Bind a control handshake to the caller's authenticated process identity.

use std::{os::fd::AsFd, sync::Arc};

use zbus::{Connection, fdo::ConnectionCredentials, message::Header, zvariant::OwnedObjectPath};

use super::{SessionError, identity::SessionIdentity, processes::ProcessHandle};

pub(super) struct Caller {
    pub(super) pid: u32,
    handle: ProcessHandle,
}

impl Caller {
    pub(super) async fn discover(connection: &Connection, header: &Header<'_>, uid: u32) -> Result<Self, SessionError> {
        let credentials = if connection.is_bus() {
            let sender = header.sender().ok_or_else(|| denied("caller has no bus identity"))?;
            let reply = connection.call_method(Some("org.freedesktop.DBus"), "/org/freedesktop/DBus",
                Some("org.freedesktop.DBus"), "GetConnectionCredentials", &(sender,)).await?;
            Arc::new(reply.body().deserialize::<ConnectionCredentials>()?)
        } else {
            connection.peer_creds().await?.clone()
        };
        Self::from_credentials(&credentials, uid)
    }

    pub(super) fn from_credentials(credentials: &ConnectionCredentials, uid: u32) -> Result<Self, SessionError> {
        if credentials.unix_user_id() != Some(uid) { return Err(denied("session belongs to another user")); }
        let pid = credentials.process_id().filter(|pid| *pid != 0)
            .ok_or_else(|| denied("caller has no process identity"))?;
        let handle = match credentials.process_fd() {
            Some(descriptor) => ProcessHandle::from_pidfd(descriptor.as_fd().try_clone_to_owned()?),
            None => ProcessHandle::open(pid)?.ok_or_else(|| denied("caller has exited"))?,
        };
        let caller = Self { pid, handle };
        caller.check_alive()?;
        Ok(caller)
    }

    pub(super) fn check_alive(&self) -> Result<(), SessionError> {
        if !self.handle.alive()? { return Err(denied("caller has exited")); }
        Ok(())
    }

    pub(super) async fn belongs_to_login(&self, connection: &Connection, identity: &SessionIdentity) -> Result<bool, SessionError> {
        let manager = zbus::Proxy::new(connection, "org.freedesktop.login1", "/org/freedesktop/login1", "org.freedesktop.login1.Manager").await?;
        match super::identity::login_session_for_process(&manager, self.pid, identity.uid).await {
            Ok(id) if id == identity.login_session_id => Ok(true),
            Ok(_) => Err(denied("caller belongs to another login session")),
            Err(SessionError::Bus(zbus::Error::MethodError(name, _, _))) if name.as_str() == "org.freedesktop.login1.NoSessionForPID" => Ok(false),
            Err(error) => Err(error),
        }
    }

    pub(super) async fn belongs_to_display(connection: &Connection, identity: &SessionIdentity) -> Result<bool, SessionError> {
        let manager = zbus::Proxy::new(connection, "org.freedesktop.login1", "/org/freedesktop/login1", "org.freedesktop.login1.Manager").await?;
        let path: OwnedObjectPath = manager.call("GetUser", &(identity.uid,)).await?;
        let user = zbus::proxy::Builder::<zbus::Proxy<'_>>::new(connection)
            .destination("org.freedesktop.login1")?.path(path)?
            .interface("org.freedesktop.login1.User")?.cache_properties(zbus::proxy::CacheProperties::No).build().await?;
        let (display, _): (String, OwnedObjectPath) = user.get_property("Display").await?;
        Ok(!display.is_empty() && display == identity.login_session_id)
    }
}

pub(super) fn denied(message: &str) -> SessionError {
    SessionError::Bus(zbus::fdo::Error::AccessDenied(message.into()).into())
}

#[cfg(test)]
#[path = "control_caller_tests.rs"]
mod tests;
