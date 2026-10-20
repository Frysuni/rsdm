//! Suspend the health reply on a private peer, without touching the user bus.

use std::{sync::Arc};

use tokio::sync::{Notify, mpsc};

use super::*;

struct Bus {
    entered: mpsc::UnboundedSender<()>,
    resume: Arc<Notify>,
}

#[zbus_macros::interface(name = "org.freedesktop.DBus")]
impl Bus {
    async fn get_id(&self) -> &str {
        let _ = self.entered.send(());
        self.resume.notified().await;
        "private-peer"
    }
}

async fn fixture() -> (Server, Connection, mpsc::UnboundedReceiver<()>, Arc<Notify>) {
    let (entered, ready) = mpsc::unbounded_channel();
    let resume = Arc::new(Notify::new());
    let bus = Bus { entered, resume: resume.clone() };
    let (server_socket, client_socket) = tokio::net::UnixStream::pair().unwrap();
    let peer = tokio::spawn(async move {
        zbus::connection::Builder::unix_stream(server_socket.into_std().unwrap()).p2p()
            .server(zbus::Guid::generate())
            .serve_at("/org/freedesktop/DBus", bus)
            .build().await.unwrap()
    });
    let connection = zbus::connection::Builder::unix_stream(client_socket.into_std().unwrap()).p2p().build().await.unwrap();
    // SAFETY: geteuid has no preconditions.
    let (endpoint, _received) = Endpoint::channel(unsafe { libc::geteuid() });
    (Server::with_connection(connection, endpoint), peer.await.unwrap(), ready, resume)
}

#[tokio::test]
async fn pending_health_checks_leave_the_caller_responsive() {
    let (mut server, _peer, mut ready, resume) = fixture().await;
    server.start_maintenance();
    tokio::time::timeout(Duration::from_secs(2), ready.recv()).await.unwrap().unwrap();
    let before = Instant::now();
    server.maintain();
    assert!(before.elapsed() < Duration::from_millis(100));
    assert!(server.pending.is_some());

    resume.notify_one();
    tokio::time::timeout(Duration::from_secs(2), async {
        while server.pending.is_some() {
            server.maintain();
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }).await.unwrap();
}

#[tokio::test]
async fn dropping_the_server_cancels_its_pending_worker() {
    let (mut server, _peer, mut ready, _resume) = fixture().await;
    server.start_maintenance();
    tokio::time::timeout(Duration::from_secs(2), ready.recv()).await.unwrap().unwrap();
    let worker = server.pending.as_ref().unwrap().worker.abort_handle();
    drop(server);
    tokio::time::timeout(Duration::from_secs(2), async {
        while !worker.is_finished() {
            tokio::task::yield_now().await;
        }
    }).await.unwrap();
}
