//! Private bootstrap channel inherited by one transient coordinator service.

use std::{os::fd::{FromRawFd, OwnedFd}, path::PathBuf, time::Duration};

use serde::{Deserialize, Serialize};
use zbus::Connection;

use super::{SessionError, control_caller::Caller, identity::{SessionIdentity, valid_generation}};

const PATH: &str = "/org/rsdm/SessionBootstrap";
const INTERFACE: &str = "org.rsdm.SessionBootstrap";

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Snapshot {
    pub(super) identity: SessionIdentity,
    pub(super) environment: Vec<(String, String)>,
    pub(super) directory: PathBuf,
}

struct Endpoint(String);

#[zbus_macros::interface(name = "org.rsdm.SessionBootstrap")]
impl Endpoint {
    fn snapshot(&self) -> &str { &self.0 }
}

pub(super) fn channel(snapshot: Snapshot) -> Result<(tokio::task::JoinHandle<zbus::Result<Connection>>, OwnedFd), SessionError> {
    let encoded = toml::to_string(&snapshot).map_err(|error| SessionError::State(error.to_string()))?;
    if encoded.len() > 262_144 { return Err(SessionError::State("attach environment is too large".into())); }
    let (server, service) = tokio::net::UnixStream::pair()?;
    let server = tokio::spawn(async move {
        zbus::connection::Builder::unix_stream(server).p2p().server(zbus::Guid::generate())?
            .method_timeout(Duration::from_secs(5)).serve_at(PATH, Endpoint(encoded))?.build().await
    });
    // The service side must reach systemd before the peer authentication can finish.
    let descriptor: OwnedFd = service.into_std()?.into();
    Ok((server, descriptor))
}

pub(super) async fn receive() -> Result<Snapshot, SessionError> {
    tokio::time::timeout(Duration::from_secs(5), async {
        // SAFETY: F_DUPFD_CLOEXEC creates an owned copy without consuming stdin.
        let raw = unsafe { libc::fcntl(0, libc::F_DUPFD_CLOEXEC, 3) };
        if raw < 0 { return Err(std::io::Error::last_os_error().into()); }
        let descriptor = unsafe { OwnedFd::from_raw_fd(raw) };
        let socket = std::os::unix::net::UnixStream::from(descriptor);
        socket.set_nonblocking(true)?;
        let connection = zbus::connection::Builder::unix_stream(tokio::net::UnixStream::from_std(socket)?)
            .p2p().method_timeout(Duration::from_secs(5)).build().await?;
        let proxy = zbus::Proxy::new(&connection, INTERFACE, PATH, INTERFACE).await?;
        let encoded: String = proxy.call("Snapshot", &()).await?;
        if encoded.len() > 262_144 { return Err(SessionError::State("attach snapshot is too large".into())); }
        let snapshot: Snapshot = toml::from_str(&encoded).map_err(|error| SessionError::State(error.to_string()))?;
        if !valid_generation(&snapshot.identity.generation) || !snapshot.directory.is_absolute() {
            return Err(SessionError::State("invalid attach snapshot".into()));
        }
        super::control::validate_finalize(&snapshot.identity.generation, &snapshot.environment)
            .map_err(SessionError::State)?;
        let caller = Caller::from_credentials(connection.peer_creds().await?, unsafe { libc::geteuid() })?;
        verify_login(&caller, &snapshot.identity).await?;
        Ok(snapshot)
    }).await.map_err(|_| SessionError::State("attach bootstrap verification timed out".into()))?
}

async fn verify_login(caller: &Caller, identity: &SessionIdentity) -> Result<(), SessionError> {
    let system = zbus::connection::Builder::system()?.method_timeout(Duration::from_secs(5)).build().await?;
    let manager = zbus::Proxy::new(&system, "org.freedesktop.login1", "/org/freedesktop/login1", "org.freedesktop.login1.Manager").await?;
    let path = super::identity::login_or_display_path(&manager, caller.pid, identity.uid).await?;
    let session = zbus::Proxy::new(&system, "org.freedesktop.login1", &path, "org.freedesktop.login1.Session").await?;
    let (uid, _): (u32, zbus::zvariant::OwnedObjectPath) = session.get_property("User").await?;
    let id: String = session.get_property("Id").await?;
    let kind: String = session.get_property("Type").await?;
    if uid != identity.uid || id != identity.login_session_id || !matches!(kind.as_str(), "wayland" | "x11") {
        return Err(SessionError::State("attach requester has no matching graphical logind session".into()));
    }
    if !caller.belongs_to_login(&system, identity).await? {
        let user = zbus::connection::Builder::session()?.method_timeout(Duration::from_secs(5)).build().await?;
        super::control_handshake::user_unit(&user, caller).await?;
    }
    if super::identity::login_or_display_path(&manager, caller.pid, identity.uid).await? != path {
        return Err(SessionError::State("attach requester's logind session changed during verification".into()));
    }
    caller.check_alive()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_private_socket_preserves_its_creator_credentials_and_snapshot() {
        let snapshot = Snapshot {
            identity: SessionIdentity { generation: "0123456789abcdef0123456789abcdef".into(), uid: unsafe { libc::geteuid() },
                login_session_id: "test".into(), desktop_entry_id: None },
            environment: vec![("VALUE".into(), "two words".into())], directory: "/".into(),
        };
        let (server, descriptor) = channel(snapshot.clone()).unwrap();
        let socket = std::os::unix::net::UnixStream::from(descriptor);
        let client = zbus::connection::Builder::unix_stream(tokio::net::UnixStream::from_std(socket).unwrap())
            .p2p().build().await.unwrap();
        let _server = server.await.unwrap().unwrap();
        let credentials = client.peer_creds().await.unwrap();
        assert_eq!(credentials.process_id(), Some(std::process::id()));
        assert_eq!(credentials.unix_user_id(), Some(snapshot.identity.uid));
        let encoded: String = zbus::Proxy::new(&client, INTERFACE, PATH, INTERFACE).await.unwrap().call("Snapshot", &()).await.unwrap();
        let received: Snapshot = toml::from_str(&encoded).unwrap();
        assert_eq!(received.identity, snapshot.identity);
        assert_eq!(received.environment, snapshot.environment);
    }
}
