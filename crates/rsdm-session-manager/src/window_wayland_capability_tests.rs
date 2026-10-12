//! Wayland registry discovery requires the recorded compositor invocation.

use std::sync::atomic::Ordering;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::*;
use crate::{wayfire::tests::Fixture, wayland_registry::tests::{advertisement, event, read_requests}, window_close::tests::Fixture as Manager};

fn record(socket: &Fixture) -> SessionRecord {
    let mut record = crate::helper_start_fixture::record();
    // SAFETY: geteuid has no preconditions; the socket belongs to this test process.
    record.identity.uid = unsafe { libc::geteuid() };
    record.compositor_unit = Some("niri.service".into());
    record.compositor_invocation = vec![1; 16];
    record.exported_environment = vec![("WAYLAND_DISPLAY".into(), socket.path.to_str().unwrap().into())];
    record
}

#[tokio::test]
async fn verified_advertisements_and_absent_protocols_explain_the_close_limitation() {
    for advertised in [false, true] {
        let manager = Manager::new().await;
        let socket = Fixture::new();
        let record = record(&socket);
        let server = tokio::spawn(async move {
            let mut peer = socket.accept().await;
            read_requests(&mut peer).await;
            if advertised { peer.write_all(&advertisement(1, "zwlr_foreign_toplevel_manager_v1", 3)).await.unwrap(); }
            peer.write_all(&event(3, 0, &0_u32.to_ne_bytes())).await.unwrap();
            let mut byte = [0_u8; 1];
            assert_eq!(peer.read(&mut byte).await.unwrap(), 0);
        });
        let capability = capability(&manager.manager, &record).await;
        assert_eq!(capability.state, "unavailable");
        let reason = if advertised { "zwlr_foreign_toplevel_manager_v1 v3" } else { "no foreign-toplevel" };
        assert!(capability.reason.contains(reason), "{}", capability.reason);
        server.await.unwrap();
    }
}

#[tokio::test]
async fn foreign_and_replaced_invocations_receive_no_registry_requests() {
    for replaced in [false, true] {
        let manager = Manager::new().await;
        if replaced { manager.invocation.store(2, Ordering::SeqCst); } else { manager.pid.store(0, Ordering::SeqCst); }
        let socket = Fixture::new();
        let record = record(&socket);
        let server = tokio::spawn(async move {
            let mut peer = socket.accept().await;
            let mut byte = [0_u8; 1];
            assert_eq!(peer.read(&mut byte).await.unwrap(), 0);
        });
        let capability = capability(&manager.manager, &record).await;
        assert!(capability.reason.contains("does not match"), "{}", capability.reason);
        server.await.unwrap();
    }
}

#[tokio::test]
async fn an_invocation_change_during_discovery_discards_the_advertisement() {
    let manager = Manager::new().await;
    let invocation = manager.invocation.clone();
    let socket = Fixture::new();
    let record = record(&socket);
    let server = tokio::spawn(async move {
        let mut peer = socket.accept().await;
        read_requests(&mut peer).await;
        invocation.store(2, Ordering::SeqCst);
        peer.write_all(&event(3, 0, &0_u32.to_ne_bytes())).await.unwrap();
    });
    let capability = capability(&manager.manager, &record).await;
    assert_eq!(capability.state, "unavailable");
    assert!(capability.reason.contains("changed during Wayland discovery"), "{}", capability.reason);
    server.await.unwrap();
}
