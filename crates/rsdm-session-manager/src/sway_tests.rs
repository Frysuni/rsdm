//! Verify the actual binary framing, tree traversal and ambiguous dispatch.

use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

use serde_json::json;
use tokio::net::UnixListener;

use super::*;

pub(super) struct Fixture { directory: PathBuf, path: PathBuf, listener: UnixListener }

impl Fixture {
    pub(super) fn new() -> Self {
        let directory = std::env::temp_dir().join(format!("rsdm-sway-{}", zbus::Guid::generate()));
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        let path = directory.join("ipc.sock");
        let listener = UnixListener::bind(&path).unwrap();
        Self { directory, path, listener }
    }

    pub(super) async fn connect(&self) -> (Connection, UnixStream) {
        // SAFETY: geteuid has no preconditions.
        let connection = Connection::connect(&self.path, unsafe { libc::geteuid() }).await.unwrap();
        let (peer, _) = self.listener.accept().await.unwrap();
        (connection, peer)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.directory); }
}

pub(super) fn frame(kind: u32, response: &Value) -> Vec<u8> {
    let payload = serde_json::to_vec(response).unwrap();
    let mut frame = MAGIC.to_vec();
    frame.extend_from_slice(&(payload.len() as u32).to_ne_bytes());
    frame.extend_from_slice(&kind.to_ne_bytes());
    frame.extend_from_slice(&payload);
    frame
}

pub(super) async fn request(peer: &mut UnixStream) -> (u32, Vec<u8>) {
    let mut header = [0_u8; HEADER_BYTES];
    peer.read_exact(&mut header).await.unwrap();
    let (length, kind) = decode_header(&header).unwrap();
    let mut payload = vec![0_u8; length];
    peer.read_exact(&mut payload).await.unwrap();
    (kind, payload)
}

#[tokio::test]
async fn tree_traversal_finds_tiled_and_floating_native_windows_and_excludes_untrusted_pids() {
    let fixture = Fixture::new();
    let (mut connection, mut peer) = fixture.connect().await;
    assert_eq!(connection.pid, std::process::id());
    let server = tokio::spawn(async move {
        assert_eq!(request(&mut peer).await, (4, Vec::new()));
        let tree = json!({"id":1,"nodes":[{"id":2,"nodes":[{"id":42,"pid":123,"shell":"xdg_shell"}]}],
            "floating_nodes":[{"id":7,"pid":456,"shell":"xdg_shell"},{"id":8,"pid":123,"shell":"xwayland"},
            {"id":9,"pid":-1,"shell":"xdg_shell"},{"id":10,"shell":"xdg_shell"}]});
        peer.write_all(&frame(4, &tree)).await.unwrap();
        assert_eq!(request(&mut peer).await, (0, b"[con_id=42] kill".to_vec()));
        peer.write_all(&frame(0, &json!([{"success":true}]))).await.unwrap();
    });
    let mut windows = connection.windows().await.unwrap();
    windows.sort_by_key(|window| window.id);
    assert_eq!(windows, [Window { id:7, pid:Some(456) }, Window { id:42, pid:Some(123) }]);
    assert!(connection.close(42).await.unwrap());
    server.await.unwrap();
}

#[tokio::test]
async fn rejected_close_is_distinct_from_a_lost_or_malformed_reply() {
    for response in [Some(json!([{"success":false}])), Some(json!([])), None] {
        let fixture = Fixture::new();
        let (mut connection, mut peer) = fixture.connect().await;
        let rejected = response == Some(json!([{"success":false}]));
        let server = tokio::spawn(async move {
            request(&mut peer).await;
            if let Some(response) = response { peer.write_all(&frame(0, &response)).await.unwrap(); }
        });
        let result = connection.close(42).await;
        if rejected { assert!(!result.unwrap()); } else { assert!(result.is_err()); }
        server.await.unwrap();
    }
}

#[tokio::test]
async fn invalid_magic_type_size_json_and_incomplete_frames_are_rejected() {
    let mut magic = frame(4, &json!({"id":1}));
    magic[0] = b'x';
    let mut oversized = MAGIC.to_vec();
    oversized.extend_from_slice(&(MAX_FRAME_BYTES as u32 + 1).to_ne_bytes());
    oversized.extend_from_slice(&4_u32.to_ne_bytes());
    let mut incomplete = frame(4, &json!({"id":1}));
    incomplete.pop();
    let mut invalid = frame(4, &json!({"id":1}));
    invalid[HEADER_BYTES] = b'x';
    for response in [magic, oversized, incomplete, invalid, frame(0, &json!({"id":1}))] {
        let fixture = Fixture::new();
        let (mut connection, mut peer) = fixture.connect().await;
        let server = tokio::spawn(async move { request(&mut peer).await; peer.write_all(&response).await.unwrap(); });
        assert!(connection.windows().await.is_err());
        server.await.unwrap();
    }
}

#[tokio::test]
async fn stalled_transport_credentials_and_relative_paths_cannot_dispatch() {
    let fixture = Fixture::new();
    // SAFETY: geteuid has no preconditions.
    let uid = unsafe { libc::geteuid() };
    assert!(Connection::connect(Path::new("ipc.sock"), uid).await.is_err());
    assert!(Connection::connect(&fixture.path, uid ^ 1).await.is_err());
    let (mut connection, _peer) = fixture.connect().await;
    let before = std::time::Instant::now();
    assert!(connection.windows().await.is_err());
    assert!(before.elapsed() < Duration::from_secs(3));
}
