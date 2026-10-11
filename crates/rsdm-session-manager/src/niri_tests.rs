//! Exercise the actual JSON wire transport, including ambiguous dispatch.

use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

use tokio::net::UnixListener;

use super::*;

pub(super) struct Fixture { directory: PathBuf, path: PathBuf, listener: UnixListener }

impl Fixture {
    pub(super) fn new() -> Self {
        let directory = std::env::temp_dir().join(format!("rsdm-niri-{}", zbus::Guid::generate()));
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        let path = directory.join("ipc.sock");
        let listener = UnixListener::bind(&path).unwrap();
        Self { directory, path, listener }
    }

    pub(super) async fn connect(&self) -> (Connection, BufReader<UnixStream>) {
        // SAFETY: geteuid has no preconditions.
        let connection = Connection::connect(&self.path, unsafe { libc::geteuid() }).await.unwrap();
        let (peer, _) = self.listener.accept().await.unwrap();
        (connection, BufReader::new(peer))
    }
}

impl Drop for Fixture {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.directory); }
}

pub(super) async fn read_request(peer: &mut BufReader<UnixStream>) -> Value {
    let mut frame = Vec::new();
    peer.read_until(b'\n', &mut frame).await.unwrap();
    serde_json::from_slice(&frame).unwrap()
}

#[tokio::test]
async fn windows_and_addressed_close_use_one_authenticated_connection() {
    let fixture = Fixture::new();
    let (mut connection, mut peer) = fixture.connect().await;
    assert_eq!(connection.pid, std::process::id());
    let server = tokio::spawn(async move {
        assert_eq!(read_request(&mut peer).await, json!("Windows"));
        peer.get_mut().write_all(b"{\"Ok\":{\"Windows\":[{\"id\":42,\"pid\":123,\"new_field\":true},{\"id\":7,\"pid\":null}]}}\n").await.unwrap();
        assert_eq!(read_request(&mut peer).await, json!({"Action":{"CloseWindow":{"id":42}}}));
        peer.get_mut().write_all(b"{\"Ok\":\"Handled\"}\n").await.unwrap();
    });
    assert_eq!(connection.windows().await.unwrap(), [Window { id: 42, pid: Some(123) }, Window { id: 7, pid: None }]);
    assert!(connection.close(42).await.unwrap());
    server.await.unwrap();
}

#[tokio::test]
async fn rejected_close_is_distinct_from_a_lost_response() {
    let fixture = Fixture::new();
    let (mut connection, mut peer) = fixture.connect().await;
    let server = tokio::spawn(async move {
        read_request(&mut peer).await;
        peer.get_mut().write_all(b"{\"Err\":\"unsupported action\"}\n").await.unwrap();
        read_request(&mut peer).await;
        // The action was received, but no reply reaches the client.
    });
    assert!(!connection.close(1).await.unwrap());
    assert!(connection.close(1).await.is_err());
    server.await.unwrap();
}

#[tokio::test]
async fn incomplete_invalid_and_oversized_frames_are_not_success() {
    for response in [b"{\"Ok\":\"Handled\"}".to_vec(), b"invalid\n".to_vec(), vec![b'x'; MAX_FRAME_BYTES as usize + 1]] {
        let fixture = Fixture::new();
        let (mut connection, mut peer) = fixture.connect().await;
        let server = tokio::spawn(async move {
            read_request(&mut peer).await;
            let _ = peer.get_mut().write_all(&response).await;
        });
        assert!(connection.close(1).await.is_err());
        server.await.unwrap();
    }
}

#[tokio::test]
async fn a_hung_peer_has_a_bounded_request_timeout() {
    let fixture = Fixture::new();
    let (mut connection, _peer) = fixture.connect().await;
    let before = std::time::Instant::now();
    assert!(connection.windows().await.is_err());
    assert!(before.elapsed() < Duration::from_secs(3));
}

#[tokio::test]
async fn foreign_credentials_and_relative_paths_are_rejected_before_protocol_requests() {
    let fixture = Fixture::new();
    assert!(Connection::connect(Path::new("ipc.sock"), unsafe { libc::geteuid() }).await.is_err());
    assert!(Connection::connect(&fixture.path, unsafe { libc::geteuid() } ^ 1).await.is_err());
}
