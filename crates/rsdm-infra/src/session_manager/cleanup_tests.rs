use std::{os::unix::net::UnixStream, sync::{Arc, atomic::{AtomicUsize, Ordering}}, thread};

use super::*;

struct Bus(Arc<AtomicUsize>);

#[zbus_macros::interface(name = "org.freedesktop.DBus")]
impl Bus {
    fn hello(&self) -> &str { ":1.42" }
    fn add_match(&self, _rule: &str) {}
    async fn request_name(&self, _name: &str, _flags: u32) -> u32 {
        self.0.fetch_add(1, Ordering::SeqCst);
        std::future::pending().await
    }
}

fn bus_fixture() -> (zbus::Connection, zbus::Connection, Arc<AtomicUsize>) {
    let (server_socket, client_socket) = UnixStream::pair().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let state = calls.clone();
    let server = thread::spawn(move || async_io::block_on(async {
        zbus::connection::Builder::unix_stream(server_socket).p2p().server(zbus::Guid::generate()).unwrap()
            .serve_at("/org/freedesktop/DBus", Bus(state)).unwrap().build().await.unwrap()
    }));
    let connection = async_io::block_on(zbus::connection::Builder::unix_stream(client_socket).build()).unwrap();
    (connection, server.join().unwrap(), calls)
}

#[test]
fn recovery_never_extends_a_preexisting_deadline() {
    let now = super::super::processes::monotonic_usec().unwrap();
    assert_eq!(recovery_deadline(Some(now + 1_000_000), 0).unwrap(), now + 1_000_000);
    assert_eq!(recovery_deadline(None, now + 1_000_000).unwrap(), now + 1_000_000);
    assert_eq!(recovery_deadline(Some(now + 2_000_000), now + 1_000_000).unwrap(), now + 1_000_000);
    assert_eq!(recovery_deadline(Some(1), 0).unwrap(), 1);
}

#[test]
fn a_new_recovery_attempt_has_one_five_second_budget() {
    let before = super::super::processes::monotonic_usec().unwrap();
    let deadline = recovery_deadline(None, 0).unwrap();
    let after = super::super::processes::monotonic_usec().unwrap();
    assert!((before + 5_000_000..=after + 5_000_000).contains(&deadline));
    assert!(recovery_deadline(Some(after + 30_000_000), after + 60_000_000).unwrap() <= after + 5_100_000);
}

#[test]
fn acquiring_a_bus_name_observes_the_recovery_deadline() {
    let (connection, _server, calls) = bus_fixture();
    let manager = UserManager::with_connection(connection);
    manager.deadline.set(super::super::processes::monotonic_usec().unwrap() + 50_000);
    let before = std::time::Instant::now();
    let result = acquire_lease(&manager);
    assert!(matches!(result, Err(SessionError::Bus(error)) if super::super::bus::retryable_error(&error)));
    assert!(before.elapsed() < Duration::from_secs(1));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
