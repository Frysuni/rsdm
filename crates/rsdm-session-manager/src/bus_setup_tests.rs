use std::{sync::{Arc, atomic::{AtomicUsize, Ordering}}};

use super::*;
use crate::processes::monotonic_usec;

struct Manager {
    version_delay: Duration,
    subscribe_delay: Duration,
    subscriptions: Arc<AtomicUsize>,
}

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Manager")]
impl Manager {
    #[zbus(property)]
    async fn version(&self) -> &str {
        if !self.version_delay.is_zero() { tokio::time::sleep(self.version_delay).await; }
        "260"
    }

    async fn subscribe(&self) {
        self.subscriptions.fetch_add(1, Ordering::SeqCst);
        if !self.subscribe_delay.is_zero() { tokio::time::sleep(self.subscribe_delay).await; }
    }
}

async fn fixture(version_delay: Duration, subscribe_delay: Duration) -> (Connection, Connection, Arc<AtomicUsize>) {
    let (server_socket, client_socket) = tokio::net::UnixStream::pair().unwrap();
    let subscriptions = Arc::new(AtomicUsize::new(0));
    let state = subscriptions.clone();
    let server = tokio::spawn(async move {
        zbus::connection::Builder::unix_stream(server_socket.into_std().unwrap()).p2p().server(zbus::Guid::generate())
            .serve_at(MANAGER_PATH, Manager { version_delay, subscribe_delay, subscriptions: state })
            .build().await.unwrap()
    });
    let connection = zbus::connection::Builder::unix_stream(client_socket.into_std().unwrap()).p2p().build().await.unwrap();
    (connection, server.await.unwrap(), subscriptions)
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

#[tokio::test]
async fn expired_setup_does_not_even_poll_the_connection_builder() {
    let deadline = Deadline::default();
    deadline.set(1);
    let calls = AtomicUsize::new(0);
    let before = Instant::now();
    let result = UserManager::connect_with(async {
        calls.fetch_add(1, Ordering::SeqCst);
        std::future::pending().await
    }, deadline).await;
    assert_timed_out(result, before);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn the_recovery_budget_includes_a_hung_bus_handshake() {
    let (_silent_peer, client_socket) = tokio::net::UnixStream::pair().unwrap();
    let before = Instant::now();
    let result = UserManager::connect_with(
        zbus::connection::Builder::unix_stream(client_socket.into_std().unwrap()).p2p().build(), short_deadline(),
    ).await;
    assert_timed_out(result, before);
}

#[tokio::test]
async fn the_recovery_budget_includes_version_validation_and_subscription() {
    for (version, subscribe, expected) in [(2, 0, 0), (0, 2, 1)] {
        let (connection, _server, subscriptions) = fixture(Duration::from_secs(version), Duration::from_secs(subscribe)).await;
        let before = Instant::now();
        let result = UserManager::connect_with(async { Ok(connection) }, short_deadline()).await;
        assert_timed_out(result, before);
        assert_eq!(subscriptions.load(Ordering::SeqCst), expected);
    }
}

#[tokio::test]
async fn a_connected_manager_keeps_the_deadline_source_used_during_setup() {
    let (connection, _server, _) = fixture(Duration::ZERO, Duration::ZERO).await;
    let deadline = Deadline::default();
    let manager = UserManager::connect_with(async { Ok(connection) }, deadline.clone()).await.unwrap();
    deadline.set(1);
    assert_eq!(manager.deadline.get(), 1);
    assert!(manager.deadline.remaining(Duration::from_secs(1)).is_err());
}
