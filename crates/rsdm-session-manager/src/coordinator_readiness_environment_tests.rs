//! Readiness reads cannot replace an accepted environment publication's ownership.

use std::{future::Future, task::{Context, Waker}};

use super::*;

#[tokio::test]
async fn niri_readiness_preserves_pending_values_and_discovers_other_endpoints() {
    let mut fixture = Fixture::new().await;
    let state = install(&fixture).await;
    fixture.coordinator.provider.kind = ProviderKind::Niri;
    fixture.coordinator.record.provider = "niri".into();
    fixture.coordinator.lifecycle.phase = SessionPhase::Starting;
    fixture.coordinator.ready_once = false;
    fixture.coordinator.ready_busy = true;
    fixture.coordinator.pending_ready = true;
    fixture.coordinator.record.exported_environment = vec![("WAYLAND_DISPLAY".into(), "wayland-before".into())];
    *state.values.lock().unwrap() = vec!["WAYLAND_DISPLAY=wayland-before".into(), "NIRI_SOCKET=/run/niri-current.sock".into()];
    let (resume, mut workers) = peer::hold_requests(&fixture.coordinator.manager).await;
    let mut response = finalize(&mut fixture, "wayland-after").await;

    // Queue the readiness read before the publication worker can acquire a slot.
    let mut readiness = Box::pin(fixture.coordinator.complete_readiness(true));
    assert!(readiness.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending());
    resume.add_permits(1);
    tokio::time::timeout(Duration::from_secs(1), readiness).await.unwrap().unwrap();
    let ready_record = fixture.coordinator.runtime.session().unwrap();
    completed_publication(&mut fixture).await;
    resume.add_permits(3);
    while let Some(result) = workers.join_next().await { result.unwrap(); }

    assert!(response.try_recv().unwrap().is_ok());
    let proposed = ("WAYLAND_DISPLAY".into(), "wayland-after".into());
    assert!(ready_record.exported_environment.contains(&proposed), "readiness lost the pending display: {:?}", ready_record.exported_environment);
    let record = fixture.coordinator.runtime.session().unwrap();
    assert!(record.exported_environment.contains(&proposed));
    assert!(record.exported_environment.contains(&("NIRI_SOCKET".into(), "/run/niri-current.sock".into())));
    crate::env::clear_owned(&fixture.coordinator.manager, &record.cleanup_environment()).await.unwrap();
    assert!(state.values.lock().unwrap().is_empty(), "the published display survived cleanup");
}
