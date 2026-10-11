//! Exercise the one-request-per-connection wire and stable-ID close guard.

use std::{fs, os::unix::fs::PermissionsExt, sync::Arc};

use serde_json::{Value, json};
use tokio::net::UnixListener;

use super::*;

pub(super) struct Fixture {
    pub(super) directory: PathBuf,
    pub(super) command: Arc<UnixListener>,
    pub(super) events: UnixListener,
}

impl Fixture {
    pub(super) fn new() -> Self {
        let directory = std::env::temp_dir().join(format!("rsdm-hypr-{}", zbus::Guid::generate()));
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        let command = Arc::new(UnixListener::bind(directory.join(".socket.sock")).unwrap());
        let events = UnixListener::bind(directory.join(".socket2.sock")).unwrap();
        Self { directory, command, events }
    }

    pub(super) async fn connect(&self) -> Connection {
        // SAFETY: geteuid has no preconditions.
        Connection::connect(&self.directory, unsafe { libc::geteuid() }).await.unwrap()
    }

    pub(super) fn serve(&self, responses: Vec<String>) -> tokio::task::JoinHandle<Vec<String>> {
        let listener = self.command.clone();
        tokio::spawn(async move {
            let mut requests = Vec::new();
            for response in responses {
                loop {
                    let (mut stream, _) = listener.accept().await.unwrap();
                    let mut request = String::new();
                    stream.read_to_string(&mut request).await.unwrap();
                    if request.is_empty() { continue; }
                    requests.push(request);
                    stream.write_all(response.as_bytes()).await.unwrap();
                    break;
                }
            }
            requests
        })
    }
}

impl Drop for Fixture {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.directory); }
}

pub(super) fn client(id: &str, address: &str, pid: i32) -> Value {
    json!({"stableId":id,"address":address,"pid":pid,"mapped":true,"xwayland":false})
}

#[tokio::test]
async fn close_checks_stable_id_pid_and_address_in_one_compositor_call() {
    let fixture = Fixture::new();
    let server = fixture.serve(vec!["ok".into(), json!([client("18000001","0x123",123)]).to_string(), "ok".into()]);
    let mut connection = fixture.connect().await;
    assert_eq!(connection.pid, std::process::id());
    assert_eq!(connection.windows().await.unwrap(), [Window { id:0x18000001, pid:Some(123) }]);
    assert!(connection.close(0x18000001).await.unwrap());
    let requests = server.await.unwrap();
    assert_eq!(requests[0], PROBE);
    assert_eq!(requests[1], "j/clients");
    assert!(requests[2].contains("math.type(w.stable_id)=='integer' and w.stable_id==0x18000001"));
    assert!(requests[2].contains("or w.stable_id=='18000001') and w.address=='0x123' and w.pid==123"));
    assert!(requests[2].contains("w.mapped and not w.xwayland"));
    assert!(requests[2].contains("hl.dispatch(hl.dsp.window.close({window='address:0x123'}))"));
    assert!(requests[2].ends_with("error('rsdm: window-close target changed',0)"));
}

#[tokio::test]
async fn address_only_unmapped_xwayland_missing_or_invalid_pids_never_become_targets() {
    let fixture = Fixture::new();
    let mut xwayland = client("2","0x2",123);
    xwayland["xwayland"] = json!(true);
    let mut unmapped = client("3","0x3",123);
    unmapped["mapped"] = json!(false);
    let mut legacy = client("4","0x4",123);
    legacy["stableId"] = Value::Null;
    let mut missing_pid = client("9","0x9",123);
    missing_pid.as_object_mut().unwrap().remove("pid");
    let server = fixture.serve(vec!["ok".into(), json!([xwayland,unmapped,legacy,missing_pid,client("0","0x5",123),
        client("6","0x6",0),client("7","0x7",-1),client("8","0x8'; os.exit()",123)]).to_string()]);
    let mut connection = fixture.connect().await;
    assert!(connection.windows().await.unwrap().is_empty());
    assert!(!connection.close(4).await.unwrap());
    assert_eq!(server.await.unwrap().len(), 2);
}

#[tokio::test]
async fn a_changed_target_is_unsent_but_ambiguous_or_lost_close_replies_are_not_retryable() {
    for response in [format!("error: {TARGET_CHANGED}"), "".into(), "error: dispatch failed".into()] {
        let rejected = response == format!("error: {TARGET_CHANGED}");
        let fixture = Fixture::new();
        let server = fixture.serve(vec!["ok".into(), json!([client("1","0x123",123)]).to_string(), response]);
        let mut connection = fixture.connect().await;
        connection.windows().await.unwrap();
        let result = connection.close(1).await;
        if rejected { assert!(!result.unwrap()); } else { assert!(result.is_err()); }
        server.await.unwrap();
    }
}

#[tokio::test]
async fn unavailable_atomic_api_invalid_json_or_oversized_replies_cannot_dispatch() {
    for responses in [vec!["eval is only supported with the lua config manager".into()],
        vec!["ok".into(), "{".into()], vec!["x".repeat(MAX_FRAME_BYTES as usize + 1)]]
    {
        let fixture = Fixture::new();
        let server = fixture.serve(responses);
        let mut connection = fixture.connect().await;
        assert!(connection.windows().await.is_err());
        server.await.unwrap();
    }
}

#[test]
fn socket_paths_require_the_recorded_runtime_directory_and_a_single_signature_component() {
    let environment = vec![("XDG_RUNTIME_DIR".into(), "/run/user/1000".into())];
    assert_eq!(socket_directory(&environment, "commit_123_456").unwrap(), Path::new("/run/user/1000/hypr/commit_123_456"));
    for signature in ["", "..", "../foreign", "/foreign", "a/b", "a\0b"] {
        assert!(socket_directory(&environment, signature).is_none());
    }
    assert!(socket_directory(&[], "signature").is_none());
    assert!(socket_directory(&[("XDG_RUNTIME_DIR".into(), "relative".into())], "signature").is_none());
}
