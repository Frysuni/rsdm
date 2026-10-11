//! Cleanup shares the result writer's lease and retains pinned ownership.

use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};

use zbus::{Connection, zvariant::OwnedObjectPath};

use super::*;
use crate::bus::UserManager;

#[derive(Clone, Copy)]
enum Invocation { Pinned, ClearedAfterPinning }

#[derive(Default)]
struct Calls { unrefs: AtomicUsize, stops: AtomicUsize }

struct Manager(Arc<Calls>);

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Manager")]
impl Manager {
    fn get_unit(&self, _unit: &str) -> OwnedObjectPath { OwnedObjectPath::try_from("/unit").unwrap() }
    fn get_unit_processes(&self, _unit: &str) -> Vec<(String, u32, String)> { Vec::new() }
    fn unref_unit(&self, _unit: &str) { self.0.unrefs.fetch_add(1, Ordering::SeqCst); }
    fn stop_unit(&self, _unit: &str, _mode: &str) -> zbus::fdo::Result<OwnedObjectPath> {
        self.0.stops.fetch_add(1, Ordering::SeqCst);
        Err(zbus::fdo::Error::Failed("unexpected stop".into()))
    }
}

struct Unit { invocation: Invocation, reads: AtomicUsize }

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Unit")]
impl Unit {
    #[zbus(property, name = "InvocationID")]
    fn invocation_id(&self) -> Vec<u8> {
        let read = self.reads.fetch_add(1, Ordering::SeqCst);
        let cleared = matches!(self.invocation, Invocation::ClearedAfterPinning) && read >= 2;
        vec![if cleared { 0 } else { 1 }; 16]
    }
    #[zbus(property)]
    fn active_state(&self) -> &str { "inactive" }
}

struct Service;

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Service")]
impl Service {
    #[zbus(property, name = "ControlPID")]
    fn control_pid(&self) -> u32 { 0 }
    #[zbus(property)]
    fn environment(&self) -> Vec<String> { vec![format!("RSDM_SESSION_GENERATION={}", crate::helper_start_fixture::GENERATION)] }
}

async fn manager(invocation: Invocation) -> (UserManager, Connection, Arc<Calls>) {
    let calls = Arc::new(Calls::default());
    let shared = calls.clone();
    let (server_socket, client_socket) = tokio::net::UnixStream::pair().unwrap();
    let server = tokio::spawn(async move {
        zbus::connection::Builder::unix_stream(server_socket).p2p().server(zbus::Guid::generate()).unwrap()
            .serve_at("/org/freedesktop/systemd1", Manager(shared)).unwrap()
            .serve_at("/unit", Unit { invocation, reads: AtomicUsize::new(0) }).unwrap()
            .serve_at("/unit", Service).unwrap().build().await.unwrap()
    });
    let connection = zbus::connection::Builder::unix_stream(client_socket).p2p().build().await.unwrap();
    (UserManager::with_connection(connection), server.await.unwrap(), calls)
}

#[tokio::test]
async fn reaping_waits_for_the_result_writer_before_archiving() {
    let fixture = Fixture::new();
    begin(&fixture.runtime, &fixture.app, Some("tx"), &Deadline::default()).await.unwrap();
    let (manager, _server, calls) = manager(Invocation::Pinned).await;
    let lease = fixture.runtime.app_lease(&fixture.app.unit).unwrap();
    let mut reaping = Box::pin(crate::apps::release_closed(&manager, &fixture.runtime));
    let unref = async {
        while calls.unrefs.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
    };
    tokio::select! {
        result = tokio::time::timeout(std::time::Duration::from_secs(2), unref) => result.unwrap(),
        result = &mut reaping => panic!("reaping overtook its result writer: {result:?}"),
    }
    tokio::select! {
        _ = tokio::time::sleep(std::time::Duration::from_millis(20)) => {},
        result = &mut reaping => panic!("reaping overtook its result writer: {result:?}"),
    }
    drop(lease);
    reaping.await.unwrap();
    assert_eq!(fixture.runtime.archived_app(&fixture.app.unit).unwrap().preparation.unwrap().outcome, Outcome::Unknown);
}

#[tokio::test]
async fn cleanup_retires_the_invocation_it_pinned_from_a_pending_record() {
    let fixture = Fixture::new();
    let mut app = fixture.app.clone();
    app.invocation_id.clear();
    fixture.runtime.save_app(&app).unwrap();
    let (manager, _server, calls) = manager(Invocation::ClearedAfterPinning).await;
    crate::apps::finish(&manager, &fixture.runtime, &app).await.unwrap();
    assert!(fixture.runtime.app(&app.unit).is_err());
    assert_eq!(calls.stops.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn cleanup_rejects_a_replacement_record_before_stopping_the_unit() {
    let fixture = Fixture::new();
    let mut replacement = fixture.app.clone();
    replacement.invocation_id = vec![2; 16];
    fixture.runtime.save_app(&replacement).unwrap();
    let (manager, _server, calls) = manager(Invocation::Pinned).await;
    assert!(matches!(crate::apps::finish(&manager, &fixture.runtime, &fixture.app).await, Err(crate::SessionError::State(_))));
    assert_eq!(calls.stops.load(Ordering::SeqCst), 0);
    assert_eq!(calls.unrefs.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.runtime.app(&fixture.app.unit).unwrap().invocation_id, replacement.invocation_id);
}
