use std::{os::unix::net::UnixStream, sync::{Arc, atomic::{AtomicUsize, Ordering}}, thread};

use super::*;
use crate::session_manager::processes::monotonic_usec;

struct Manager {
    version_delay: Duration,
    subscribe_delay: Duration,
    subscriptions: Arc<AtomicUsize>,
}

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Manager")]
impl Manager {
    #[zbus(property)]
    async fn version(&self) -> &str {
        if !self.version_delay.is_zero() { async_io::Timer::after(self.version_delay).await; }
        "260"
    }

    async fn subscribe(&self) {
        self.subscriptions.fetch_add(1, Ordering::SeqCst);
        if !self.subscribe_delay.is_zero() { async_io::Timer::after(self.subscribe_delay).await; }
    }
}

fn fixture(version_delay: Duration, subscribe_delay: Duration) -> (Connection, Connection, Arc<AtomicUsize>) {
    let (server_socket, client_socket) = UnixStream::pair().unwrap();
    let subscriptions = Arc::new(AtomicUsize::new(0));
    let state = subscriptions.clone();
    let server = thread::spawn(move || async_io::block_on(async {
        zbus::connection::Builder::unix_stream(server_socket).p2p().server(zbus::Guid::generate()).unwrap()
            .serve_at(MANAGER_PATH, Manager { version_delay, subscribe_delay, subscriptions: state }).unwrap()
            .build().await.unwrap()
    }));
    let connection = async_io::block_on(zbus::connection::Builder::unix_stream(client_socket).p2p().build()).unwrap();
    (connection, server.join().unwrap(), subscriptions)
}

fn short_deadline() -> Deadline {
    let deadline = Deadline::default();
    deadline.set(monotonic_usec().unwrap() + 50_000);
    deadline
}

fn assert_timed_out(result: Result<UserManager, SessionError>, before: Instant) {
    assert!(matches!(result, Err(SessionError::Bus(error)) if retryable_error(&error)));
    assert!(before.elapsed() < Duration::from_secs(1));
}

#[test]
fn expired_setup_does_not_even_poll_the_connection_builder() {
    let deadline = Deadline::default();
    deadline.set(1);
    let calls = AtomicUsize::new(0);
    let before = Instant::now();
    let result = async_io::block_on(UserManager::connect_with(async {
        calls.fetch_add(1, Ordering::SeqCst);
        std::future::pending().await
    }, deadline));
    assert_timed_out(result, before);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn the_recovery_budget_includes_a_hung_bus_handshake() {
    let (_silent_peer, client_socket) = UnixStream::pair().unwrap();
    let before = Instant::now();
    let result = async_io::block_on(UserManager::connect_with(
        zbus::connection::Builder::unix_stream(client_socket).p2p().build(), short_deadline(),
    ));
    assert_timed_out(result, before);
}

#[test]
fn the_recovery_budget_includes_version_validation_and_subscription() {
    for (version, subscribe, expected) in [(2, 0, 0), (0, 2, 1)] {
        let (connection, _server, subscriptions) = fixture(Duration::from_secs(version), Duration::from_secs(subscribe));
        let before = Instant::now();
        let result = async_io::block_on(UserManager::connect_with(async { Ok(connection) }, short_deadline()));
        assert_timed_out(result, before);
        assert_eq!(subscriptions.load(Ordering::SeqCst), expected);
    }
}

#[test]
fn a_connected_manager_keeps_the_deadline_source_used_during_setup() {
    let (connection, _server, _) = fixture(Duration::ZERO, Duration::ZERO);
    let deadline = Deadline::default();
    let manager = async_io::block_on(UserManager::connect_with(async { Ok(connection) }, deadline.clone())).unwrap();
    deadline.set(1);
    assert_eq!(manager.deadline.get(), 1);
    assert!(manager.deadline.remaining(Duration::from_secs(1)).is_err());
}
