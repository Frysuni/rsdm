//! Ownership revoked in the request queue cannot authorize a compositor write.

use std::{sync::{Arc, atomic::Ordering}, time::Duration};

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use super::*;

#[path = "window_dispatch_fixture.rs"]
mod fixture;
use fixture::Fixture;

async fn prepared() -> (PreparedClose, BufReader<tokio::net::UnixStream>, tokio::net::UnixStream, crate::wayfire::tests::Fixture) {
    let socket = crate::wayfire::tests::Fixture::new();
    let uid = unsafe { libc::geteuid() };
    let connection = Connection::connect("NIRI_SOCKET", &socket.path, uid).await.unwrap();
    let peer = BufReader::new(socket.accept().await);
    let subscriber = Connection::connect("NIRI_SOCKET", &socket.path, uid).await.unwrap();
    let mut events = BufReader::new(socket.accept().await);
    let server = tokio::spawn(async move {
        let mut request = String::new();
        events.read_line(&mut request).await.unwrap();
        assert_eq!(serde_json::from_str::<Value>(&request).unwrap(), json!("EventStream"));
        let snapshot = json!({"WindowsChanged":{"windows":[{"id":42,"pid":std::process::id()}]}});
        events.get_mut().write_all(format!("{{\"Ok\":\"Handled\"}}\n{snapshot}\n").as_bytes()).await.unwrap();
        events.into_inner()
    });
    let mut connection = connection;
    let (events, mut windows) = connection.subscribe(subscriber).await.unwrap();
    let process = ProcessHandle::open(std::process::id()).unwrap().unwrap();
    (PreparedClose { connection, events, window: windows.remove(0), process }, peer, server.await.unwrap(), socket)
}

async fn occupy(manager: &UserManager) -> (tokio::task::JoinSet<()>, Arc<tokio::sync::Semaphore>) {
    let (entered, mut occupied) = tokio::sync::mpsc::unbounded_channel();
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let mut workers = tokio::task::JoinSet::new();
    for _ in 0..4 {
        let manager = manager.clone();
        let entered = entered.clone();
        let release = release.clone();
        workers.spawn(async move { manager.request(async {
            entered.send(()).unwrap();
            release.acquire().await.unwrap().forget();
            Ok(())
        }).await.unwrap(); });
    }
    for _ in 0..4 { tokio::time::timeout(Duration::from_secs(1), occupied.recv()).await.unwrap().unwrap(); }
    (workers, release)
}

#[tokio::test]
async fn invocation_process_and_cgroup_changes_while_queued_prevent_window_close() {
    for revoked in ["app", "compositor", "main-pid", "control-pid", "processes", "cgroup"] {
        let fixture = Fixture::new().await;
        let (mut prepared, mut peer, _events, _socket) = prepared().await;
        let control = ShutdownControl::default();
        let (mut workers, release) = occupy(&fixture.manager).await;
        let request = close_prepared(&fixture.manager, &fixture.record, &fixture.app, &mut prepared, &control);
        tokio::pin!(request);
        assert!(tokio::time::timeout(Duration::from_millis(25), &mut request).await.is_err());
        assert_eq!(fixture.state.reads.load(Ordering::SeqCst), 0);
        match revoked {
            "app" => fixture.state.app_invocation.store(2, Ordering::SeqCst),
            "compositor" => fixture.state.compositor_invocation.store(2, Ordering::SeqCst),
            "main-pid" => fixture.state.main_pid.store(0, Ordering::SeqCst),
            "control-pid" => fixture.state.control_pid.store(std::process::id(), Ordering::SeqCst),
            "processes" => fixture.state.present.store(false, Ordering::SeqCst),
            "cgroup" => *fixture.state.group.lock().unwrap() = "/rsdm-foreign".into(),
            _ => unreachable!(),
        }
        release.add_permits(4);
        while let Some(result) = workers.join_next().await { result.unwrap(); }
        assert!(!tokio::time::timeout(Duration::from_secs(1), &mut request).await.unwrap().unwrap(), "{revoked}");
        assert!(fixture.state.reads.load(Ordering::SeqCst) > 0);
        let mut command = String::new();
        assert!(tokio::time::timeout(Duration::from_millis(25), peer.read_line(&mut command)).await.is_err(), "{revoked}: {command}");
    }
}

#[tokio::test]
async fn current_dedicated_cgroup_authorizes_one_close_without_nested_request_slots() {
    let fixture = Fixture::new().await;
    let (mut prepared, mut peer, _events, _socket) = prepared().await;
    let dedicated = *fixture.state.group.lock().unwrap() != "/";
    let server = tokio::spawn(async move {
        let mut command = String::new();
        if dedicated {
            peer.read_line(&mut command).await.unwrap();
            assert_eq!(serde_json::from_str::<Value>(&command).unwrap(), json!({"Action":{"CloseWindow":{"id":42}}}));
            peer.get_mut().write_all(b"{\"Ok\":\"Handled\"}\n").await.unwrap();
        } else {
            assert!(tokio::time::timeout(Duration::from_millis(100), peer.read_line(&mut command)).await.is_err());
        }
    });
    let control = ShutdownControl::default();
    let (mut workers, release) = occupy(&fixture.manager).await;
    let request = close_prepared(&fixture.manager, &fixture.record, &fixture.app, &mut prepared, &control);
    tokio::pin!(request);
    assert!(tokio::time::timeout(Duration::from_millis(25), &mut request).await.is_err());
    release.add_permits(1);
    assert_eq!(tokio::time::timeout(Duration::from_secs(1), &mut request).await.unwrap().unwrap(), dedicated);
    release.add_permits(3);
    while let Some(result) = workers.join_next().await { result.unwrap(); }
    server.await.unwrap();
}

#[tokio::test]
async fn migrated_scope_ownership_is_rechecked_after_waiting_for_the_dispatch_slot() {
    for revoked in ["none", "escaped-main", "service-ended", "scope", "app"] {
        let mut fixture = Fixture::new().await;
        let handle = ProcessHandle::open(std::process::id()).unwrap().unwrap();
        let Some(identity) = handle.id().unwrap() else { return; };
        fixture.app.original_process = Some(crate::app_origin::OriginalProcess { pid: std::process::id(), pidfd_id: identity });
        fixture.app.migrated_scope = Some(crate::app_scope::MigratedScope { unit: "app-test.scope".into(), invocation_id: vec![1; 16] });
        fixture.state.present.store(false, Ordering::SeqCst);
        fixture.state.scope_present.store(true, Ordering::SeqCst);
        if revoked == "escaped-main" {
            fixture.state.present.store(true, Ordering::SeqCst);
            *fixture.state.group.lock().unwrap() = "/old-service".into();
        }
        let expected = matches!(revoked, "none" | "escaped-main" | "service-ended") && *fixture.state.scope_group.lock().unwrap() != "/";
        let (mut prepared, mut peer, _events, _socket) = prepared().await;
        let server = tokio::spawn(async move {
            let mut command = String::new();
            if expected {
                peer.read_line(&mut command).await.unwrap();
                assert_eq!(serde_json::from_str::<Value>(&command).unwrap(), json!({"Action":{"CloseWindow":{"id":42}}}));
                peer.get_mut().write_all(b"{\"Ok\":\"Handled\"}\n").await.unwrap();
            } else { assert!(tokio::time::timeout(Duration::from_millis(100), peer.read_line(&mut command)).await.is_err()); }
        });
        let control = ShutdownControl::default();
        let (mut workers, release) = occupy(&fixture.manager).await;
        let request = close_prepared(&fixture.manager, &fixture.record, &fixture.app, &mut prepared, &control);
        tokio::pin!(request);
        assert!(tokio::time::timeout(Duration::from_millis(25), &mut request).await.is_err());
        if revoked == "scope" { fixture.state.scope_invocation.store(2, Ordering::SeqCst); }
        if revoked == "app" { fixture.state.app_invocation.store(2, Ordering::SeqCst); }
        if revoked == "service-ended" { fixture.state.app_invocation.store(0, Ordering::SeqCst); }
        release.add_permits(1);
        assert_eq!(tokio::time::timeout(Duration::from_secs(1), &mut request).await.unwrap().unwrap(), expected, "{revoked}");
        release.add_permits(3);
        while let Some(result) = workers.join_next().await { result.unwrap(); }
        server.await.unwrap();
    }
}
