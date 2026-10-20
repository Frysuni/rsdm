use std::{os::fd::{AsFd, FromRawFd, OwnedFd}, sync::atomic::{AtomicUsize, Ordering}};

use super::*;

struct Broker {
    uid: Option<u32>,
    pid: Option<u32>,
    replaced: bool,
    descriptor: Option<OwnedFd>,
    owner_queries: AtomicUsize,
}

struct AbsentBroker;

#[zbus_macros::interface(name = "org.freedesktop.DBus")]
impl AbsentBroker {
    fn get_name_owner(&self, _name: &str) -> zbus::fdo::Result<String> {
        Err(zbus::fdo::Error::NameHasNoOwner("no native compositor".into()))
    }
}

#[zbus_macros::interface(name = "org.freedesktop.DBus")]
impl Broker {
    fn get_name_owner(&self, name: &str) -> String {
        assert_eq!(name, "org.gnome.Shell");
        let query = self.owner_queries.fetch_add(1, Ordering::SeqCst);
        if self.replaced && query != 0 { ":1.2" } else { ":1.1" }.into()
    }

    fn get_connection_credentials(&self, name: &str) -> ConnectionCredentials {
        assert_eq!(name, ":1.1");
        let mut values = ConnectionCredentials::default();
        if let Some(uid) = self.uid { values = values.set_unix_user_id(uid); }
        if let Some(pid) = self.pid { values = values.set_process_id(pid); }
        if let Some(descriptor) = &self.descriptor {
            values = values.set_process_fd(descriptor.as_fd().try_clone_to_owned().unwrap().into());
        }
        values
    }
}

async fn fixture(uid: Option<u32>, pid: Option<u32>, replaced: bool, descriptor: Option<OwnedFd>) -> (Connection, Connection) {
    let (server, client) = tokio::net::UnixStream::pair().unwrap();
    let broker = Broker { uid, pid, replaced, descriptor, owner_queries: AtomicUsize::new(0) };
    let server = tokio::spawn(async move {
        zbus::connection::Builder::unix_stream(server.into_std().unwrap()).p2p().server(zbus::Guid::generate())
            .serve_at("/org/freedesktop/DBus", broker).build().await.unwrap()
    });
    let client = zbus::connection::Builder::unix_stream(client.into_std().unwrap()).p2p().build().await.unwrap();
    (client, server.await.unwrap())
}

fn provider() -> Provider {
    Provider { kind: ProviderKind::Gnome, native_unit: None, logout_command: Vec::new() }
}

#[tokio::test]
async fn an_absent_native_compositor_is_not_an_authorization_failure() {
    let (client, server) = fixture(None, None, false, None).await;
    server.object_server().remove::<Broker, _>("/org/freedesktop/DBus").await.unwrap();
    server.object_server().at("/org/freedesktop/DBus", AbsentBroker).await.unwrap();
    assert!(pin(&client, &provider()).await.unwrap().is_none());
}

#[tokio::test]
async fn pin_survives_the_identified_process_exiting() {
    let mut child = tokio::process::Command::new("sleep").arg("30").kill_on_drop(true).spawn().unwrap();
    let (client, _server) = fixture(Some(unsafe { libc::geteuid() }), child.id(), false, None).await;
    let handle = pin(&client, &provider()).await.unwrap().unwrap();
    assert!(handle.alive().unwrap());
    child.kill().await.unwrap();
    child.wait().await.unwrap();
    assert!(!handle.alive().unwrap());
}

#[tokio::test]
async fn a_changed_unique_owner_is_not_pinned() {
    let (client, _server) = fixture(Some(unsafe { libc::geteuid() }), Some(std::process::id()), true, None).await;
    assert!(pin(&client, &provider()).await.unwrap().is_none());
}

#[tokio::test]
async fn a_foreign_user_or_missing_process_credentials_are_rejected() {
    let uid = unsafe { libc::geteuid() };
    for (uid, pid) in [(Some(uid + 1), Some(std::process::id())), (None, Some(std::process::id())), (Some(uid), None), (Some(uid), Some(0))] {
        let (client, _server) = fixture(uid, pid, false, None).await;
        assert!(pin(&client, &provider()).await.is_err());
    }
}

#[tokio::test]
async fn broker_pidfd_takes_precedence_over_a_recycled_numeric_pid() {
    let mut child = tokio::process::Command::new("sleep").arg("30").kill_on_drop(true).spawn().unwrap();
    // SAFETY: the syscall returns a new descriptor for the live child.
    let raw = unsafe { libc::syscall(libc::SYS_pidfd_open, child.id().unwrap(), 0) };
    assert!(raw >= 0);
    let descriptor = unsafe { OwnedFd::from_raw_fd(raw as i32) };
    child.kill().await.unwrap();
    child.wait().await.unwrap();
    for pid in [Some(std::process::id()), None] {
        let (client, _server) = fixture(Some(unsafe { libc::geteuid() }), pid, false, Some(descriptor.as_fd().try_clone_to_owned().unwrap())).await;
        assert!(!pin(&client, &provider()).await.unwrap().unwrap().alive().unwrap());
    }
}
