//! Framing, plugin discovery and socket authentication never dispatch a close.

use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

use tokio::net::UnixListener;

use super::*;

pub(crate) struct Fixture { directory: PathBuf, pub(crate) path: PathBuf, listener: UnixListener }

impl Fixture {
    pub(crate) fn new() -> Self {
        let directory = std::env::temp_dir().join(format!("rsdm-wayfire-{}", zbus::Guid::generate()));
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        let path = directory.join("ipc.sock");
        let listener = UnixListener::bind(&path).unwrap();
        Self { directory, path, listener }
    }

    pub(crate) async fn accept(&self) -> UnixStream { self.listener.accept().await.unwrap().0 }

    async fn connect(&self) -> (Connection, UnixStream) {
        // SAFETY: geteuid has no preconditions.
        let connection = Connection::connect(&self.path, unsafe { libc::geteuid() }).await.unwrap();
        (connection, self.accept().await)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.directory); }
}

pub(crate) fn frame(response: &Value) -> Vec<u8> {
    let payload = serde_json::to_vec(response).unwrap();
    let mut frame = (payload.len() as u32).to_ne_bytes().to_vec();
    frame.extend_from_slice(&payload);
    frame
}

pub(crate) async fn request(peer: &mut UnixStream) -> Value {
    let mut header = [0_u8; 4];
    peer.read_exact(&mut header).await.unwrap();
    let mut payload = vec![0_u8; u32::from_ne_bytes(header) as usize];
    peer.read_exact(&mut payload).await.unwrap();
    serde_json::from_slice(&payload).unwrap()
}

pub(crate) async fn serve_discovery(peer: &mut UnixStream, views: Value) {
    assert_eq!(request(peer).await, json!({"method":"list-methods", "data":{}}));
    let methods = json!({"methods":[LIST_VIEWS, VIEW_INFO, "window-rules/close-view"]});
    for byte in frame(&methods) { peer.write_all(&[byte]).await.unwrap(); }
    assert_eq!(request(peer).await, json!({"method":LIST_VIEWS, "data":{}}));
    peer.write_all(&frame(&views)).await.unwrap();
    if let Some(view) = views.as_array().unwrap().first() {
        assert_eq!(request(peer).await, json!({"method":VIEW_INFO, "data":{"id":view["id"]}}));
        peer.write_all(&frame(&json!({"result":"ok", "info":view}))).await.unwrap();
    }
}

#[tokio::test]
async fn discovery_uses_only_read_methods_with_fragmented_frames_and_untrusted_view_metadata() {
    for views in [json!([]), json!([{"id":42,"pid":999,"app-id":"untrusted","title":"arbitrary"}])] {
        let fixture = Fixture::new();
        let (mut connection, mut peer) = fixture.connect().await;
        assert_eq!(connection.pid, std::process::id());
        let server = tokio::spawn(async move { serve_discovery(&mut peer, views).await; });
        connection.discover().await.unwrap();
        server.await.unwrap();
    }
}

#[tokio::test]
async fn missing_plugins_protocol_errors_and_invalid_method_lists_are_distinct_failures() {
    for (response, reason) in [
        (json!({"methods":[]}), "ipc-rules is unavailable"),
        (json!({"error":"No such method found!"}), "No such method"),
        (json!({"methods":[42]}), "invalid Wayfire method list"),
    ] {
        let fixture = Fixture::new();
        let (mut connection, mut peer) = fixture.connect().await;
        let server = tokio::spawn(async move {
            request(&mut peer).await;
            peer.write_all(&frame(&response)).await.unwrap();
        });
        assert!(connection.discover().await.unwrap_err().to_string().contains(reason));
        server.await.unwrap();
    }
}

#[tokio::test]
async fn incomplete_empty_oversized_and_invalid_json_frames_are_rejected() {
    let mut incomplete = frame(&json!({"methods":[]}));
    incomplete.pop();
    let mut invalid = 1_u32.to_ne_bytes().to_vec();
    invalid.push(b'x');
    for response in [vec![0,0], 0_u32.to_ne_bytes().to_vec(), (MAX_FRAME_BYTES as u32 + 1).to_ne_bytes().to_vec(), incomplete, invalid] {
        let fixture = Fixture::new();
        let (mut connection, mut peer) = fixture.connect().await;
        let server = tokio::spawn(async move { request(&mut peer).await; peer.write_all(&response).await.unwrap(); });
        assert!(connection.discover().await.is_err());
        server.await.unwrap();
    }
}

#[tokio::test]
async fn malformed_view_lists_and_changed_or_missing_view_info_cannot_validate_discovery() {
    for (views, info) in [
        (json!({}), None), (json!([{"pid":42}]), None), (json!([{"id":u64::MAX}]), None),
        (json!([{"id":42}]), Some(json!({"info":{"id":43}}))),
        (json!([{"id":42}]), Some(json!({"result":"ok"}))),
        (json!([{"id":42}]), Some(json!({"error":"no such view"}))),
    ] {
        let fixture = Fixture::new();
        let (mut connection, mut peer) = fixture.connect().await;
        let server = tokio::spawn(async move {
            assert_eq!(request(&mut peer).await["method"], "list-methods");
            peer.write_all(&frame(&json!({"methods":[LIST_VIEWS, VIEW_INFO, "window-rules/close-view"]}))).await.unwrap();
            assert_eq!(request(&mut peer).await["method"], LIST_VIEWS);
            peer.write_all(&frame(&views)).await.unwrap();
            if let Some(info) = info {
                assert_eq!(request(&mut peer).await["method"], VIEW_INFO);
                peer.write_all(&frame(&info)).await.unwrap();
            }
        });
        assert!(connection.discover().await.is_err());
        server.await.unwrap();
    }
}

#[tokio::test]
async fn stalled_peers_foreign_users_and_relative_paths_are_rejected() {
    let fixture = Fixture::new();
    // SAFETY: geteuid has no preconditions.
    let uid = unsafe { libc::geteuid() };
    assert!(Connection::connect(Path::new("ipc.sock"), uid).await.is_err());
    assert!(Connection::connect(&fixture.path, uid ^ 1).await.is_err());
    let (mut connection, _peer) = fixture.connect().await;
    let before = std::time::Instant::now();
    assert!(connection.discover().await.unwrap_err().to_string().contains("timed out"));
    assert!(before.elapsed() < Duration::from_secs(3));
}
