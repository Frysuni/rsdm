//! Compositor IPC cannot act through a foreign or replaced invocation.

use std::sync::{Arc, atomic::{AtomicU32, AtomicU8, Ordering}};

use zbus::{Connection, zvariant::OwnedObjectPath};

use super::*;

struct Manager;

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Manager")]
impl Manager {
    fn get_unit(&self, unit: &str) -> OwnedObjectPath {
        assert_eq!(unit, "niri.service");
        OwnedObjectPath::try_from("/compositor").unwrap()
    }
}

struct Unit(Arc<AtomicU8>);

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Unit")]
impl Unit {
    #[zbus(property, name = "InvocationID")]
    fn invocation_id(&self) -> Vec<u8> { vec![self.0.load(Ordering::SeqCst); 16] }
}

struct Service(Arc<AtomicU32>);

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Service")]
impl Service {
    #[zbus(property, name = "MainPID")]
    fn main_pid(&self) -> u32 { self.0.load(Ordering::SeqCst) }
}

pub(crate) struct Fixture {
    pub(crate) manager: UserManager,
    pub(crate) invocation: Arc<AtomicU8>,
    pub(crate) pid: Arc<AtomicU32>,
    _server: Connection,
}

impl Fixture {
    pub(crate) async fn new() -> Self {
        let invocation = Arc::new(AtomicU8::new(1));
        let pid = Arc::new(AtomicU32::new(std::process::id()));
        let unit = Unit(invocation.clone());
        let service = Service(pid.clone());
        let (server, client) = tokio::net::UnixStream::pair().unwrap();
        let server = tokio::spawn(async move {
            zbus::connection::Builder::unix_stream(server).p2p().server(zbus::Guid::generate()).unwrap()
                .serve_at("/org/freedesktop/systemd1", Manager).unwrap()
                .serve_at("/compositor", unit).unwrap().serve_at("/compositor", service).unwrap()
                .build().await.unwrap()
        });
        let connection = zbus::connection::Builder::unix_stream(client).p2p().build().await.unwrap();
        Self { manager: UserManager::with_connection(connection), invocation, pid, _server: server.await.unwrap() }
    }
}

#[tokio::test]
async fn compositor_socket_credentials_must_match_the_persisted_invocation_and_main_process() {
    let fixture = Fixture::new().await;
    let mut record = crate::helper_start_fixture::record();
    record.compositor_unit = Some("niri.service".into());
    record.compositor_invocation = vec![1; 16];
    assert!(owns_compositor(&fixture.manager, &record, std::process::id()).await.unwrap());
    fixture.pid.store(0, Ordering::SeqCst);
    assert!(!owns_compositor(&fixture.manager, &record, std::process::id()).await.unwrap());
    fixture.pid.store(std::process::id(), Ordering::SeqCst);
    fixture.invocation.store(2, Ordering::SeqCst);
    assert!(!owns_compositor(&fixture.manager, &record, std::process::id()).await.unwrap());
}

#[tokio::test]
async fn unowned_partial_or_unpinned_compositors_cannot_authorize_window_close() {
    let fixture = Fixture::new().await;
    let mut record = crate::helper_start_fixture::record();
    assert!(!owns_compositor(&fixture.manager, &record, std::process::id()).await.unwrap());
    record.compositor_unit = Some("niri.service".into());
    for invocation in [Vec::new(), vec![0; 16]] {
        record.compositor_invocation = invocation;
        assert!(!owns_compositor(&fixture.manager, &record, std::process::id()).await.unwrap());
    }
}
