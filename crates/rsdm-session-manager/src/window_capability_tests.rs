//! Discovery is read-only and belongs to the current compositor invocation.

use std::sync::atomic::Ordering;

use serde_json::json;
use tokio::io::AsyncWriteExt;

use super::*;
use crate::{wayfire::tests::{Fixture, frame, request, serve_discovery}, window_close::tests::Fixture as Manager};

fn record(socket: &Fixture) -> SessionRecord {
    let mut record = crate::helper_start_fixture::record();
    // SAFETY: geteuid has no preconditions; the socket is owned by this test process.
    record.identity.uid = unsafe { libc::geteuid() };
    record.compositor_unit = Some("niri.service".into());
    record.compositor_invocation = vec![1; 16];
    record.exported_environment = vec![("WAYFIRE_SOCKET".into(), socket.path.to_str().unwrap().into())];
    record
}

#[tokio::test]
async fn verified_wayfire_discovery_reports_its_ownership_limit_without_authorizing_close() {
    let manager = Manager::new().await;
    let socket = Fixture::new();
    let record = record(&socket);
    let server = tokio::spawn(async move {
        let mut peer = socket.accept().await;
        serve_discovery(&mut peer, json!([{"id":42,"pid":std::process::id()}])).await;
    });
    let capability = capability(&manager.manager, &record).await;
    assert_eq!(capability.state, "unavailable");
    assert!(capability.reason.contains("discovery available"), "{}", capability.reason);
    assert!(capability.reason.contains("PIDs do not prove ownership"));
    server.await.unwrap();
}

#[tokio::test]
async fn foreign_or_replaced_compositor_peers_receive_no_discovery_queries() {
    for replaced in [false, true] {
        let manager = Manager::new().await;
        if replaced { manager.invocation.store(2, Ordering::SeqCst); } else { manager.pid.store(0, Ordering::SeqCst); }
        let socket = Fixture::new();
        let record = record(&socket);
        let server = tokio::spawn(async move {
            let mut peer = socket.accept().await;
            let mut byte = [0_u8; 1];
            use tokio::io::AsyncReadExt;
            assert_eq!(peer.read(&mut byte).await.unwrap(), 0);
        });
        let capability = capability(&manager.manager, &record).await;
        assert_eq!(capability.state, "unavailable");
        assert!(capability.reason.contains("does not match"));
        server.await.unwrap();
    }
}

#[tokio::test]
async fn missing_plugins_and_invocation_changes_are_reported_separately() {
    for replaced in [false, true] {
        let manager = Manager::new().await;
        let socket = Fixture::new();
        let record = record(&socket);
        let invocation = manager.invocation.clone();
        let server = tokio::spawn(async move {
            let mut peer = socket.accept().await;
            if replaced {
                assert_eq!(request(&mut peer).await["method"], "list-methods");
                peer.write_all(&frame(&json!({"methods":["window-rules/list-views", "window-rules/view-info", "window-rules/close-view"]}))).await.unwrap();
                assert_eq!(request(&mut peer).await["method"], "window-rules/list-views");
                invocation.store(2, Ordering::SeqCst);
                peer.write_all(&frame(&json!([]))).await.unwrap();
            } else {
                request(&mut peer).await;
                peer.write_all(&frame(&json!({"methods":[]}))).await.unwrap();
            }
        });
        let capability = capability(&manager.manager, &record).await;
        assert_eq!(capability.state, "unavailable");
        let reason = if replaced { "changed during discovery" } else { "ipc-rules is unavailable" };
        assert!(capability.reason.contains(reason), "{}", capability.reason);
        server.await.unwrap();
    }
}

#[tokio::test]
async fn disabled_native_and_unowned_records_do_not_connect_to_wayfire() {
    let manager = Manager::new().await;
    let socket = Fixture::new();
    for mode in ["disabled", "gnome", "plasma", "unowned"] {
        let mut record = record(&socket);
        match mode {
            "disabled" => record.disabled = true,
            "unowned" => record.compositor_unit = None,
            provider => record.provider = provider.into(),
        }
        let capability = capability(&manager.manager, &record).await;
        assert_ne!(capability.state, "available");
        assert!(tokio::time::timeout(std::time::Duration::from_millis(20), socket.accept()).await.is_err());
    }
}
