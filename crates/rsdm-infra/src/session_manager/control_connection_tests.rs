//! Suspend the health reply on a private peer, without touching the user bus.

use std::{os::unix::net::UnixStream, sync::mpsc};

use super::*;

struct Bus {
    entered: mpsc::Sender<()>,
    resume: async_channel::Receiver<()>,
}

#[zbus_macros::interface(name = "org.freedesktop.DBus")]
impl Bus {
    async fn get_id(&self) -> &str {
        let _ = self.entered.send(());
        let _ = self.resume.recv().await;
        "private-peer"
    }
}

fn fixture() -> (Server, Connection, mpsc::Receiver<()>, async_channel::Sender<()>) {
    let (entered, ready) = mpsc::channel();
    let (resume, resumed) = async_channel::bounded(1);
    let (server_socket, client_socket) = UnixStream::pair().unwrap();
    let peer = thread::spawn(move || async_io::block_on(async {
        zbus::connection::Builder::unix_stream(server_socket).p2p()
            .server(zbus::Guid::generate()).unwrap()
            .serve_at("/org/freedesktop/DBus", Bus { entered, resume: resumed }).unwrap()
            .build().await.unwrap()
    }));
    let connection = async_io::block_on(async {
        zbus::connection::Builder::unix_stream(client_socket).p2p().build().await.unwrap()
    });
    let (endpoint, _received) = Endpoint::channel(crate::lock_control::current_uid());
    (Server::with_connection(connection, endpoint), peer.join().unwrap(), ready, resume)
}

#[test]
fn pending_health_checks_leave_the_caller_responsive() {
    let (mut server, _peer, ready, resume) = fixture();
    server.start_maintenance();
    ready.recv_timeout(Duration::from_secs(2)).unwrap();
    let before = Instant::now();
    server.maintain();
    assert!(before.elapsed() < Duration::from_millis(100));
    assert!(server.pending.is_some());

    resume.try_send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while server.pending.is_some() && Instant::now() < deadline {
        server.maintain();
        thread::sleep(Duration::from_millis(5));
    }
    assert!(server.pending.is_none());
}

#[test]
fn closing_the_result_channel_cancels_its_pending_worker() {
    let (mut server, _peer, ready, _resume) = fixture();
    server.start_maintenance();
    ready.recv_timeout(Duration::from_secs(2)).unwrap();
    let pending = server.pending.take().unwrap();
    pending.close();
    let deadline = Instant::now() + Duration::from_secs(2);
    while pending.sender_count() != 0 && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(pending.sender_count(), 0);
}
