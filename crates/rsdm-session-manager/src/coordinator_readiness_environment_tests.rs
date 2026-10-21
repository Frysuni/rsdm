//! Readiness discovery preserves pending exports and tolerates temporary bus failures.

use std::{future::Future, task::{Context, Waker}};

use super::*;

async fn starting_fixture(kind: ProviderKind) -> (Fixture, Arc<peer::State>) {
    let mut fixture = Fixture::new().await;
    let state = install(&fixture).await;
    fixture.coordinator.provider.kind = kind;
    fixture.coordinator.record.provider = fixture.coordinator.provider.name().into();
    fixture.coordinator.lifecycle.phase = SessionPhase::Starting;
    fixture.coordinator.ready_once = false;
    fixture.coordinator.ready_busy = true;
    fixture.coordinator.pending_ready = true;
    fixture.coordinator.ready_deadline = Instant::now() + Duration::from_secs(30);
    fixture.coordinator.save().await.unwrap();
    (fixture, state)
}

#[tokio::test]
async fn niri_readiness_preserves_pending_values_and_discovers_other_endpoints() {
    let (mut fixture, state) = starting_fixture(ProviderKind::Niri).await;
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

#[tokio::test]
async fn temporary_environment_failure_defers_readiness_until_the_next_observation() {
    let (mut fixture, state) = starting_fixture(ProviderKind::Niri).await;
    let (reply, mut finalized) = tokio::sync::mpsc::channel(1);
    fixture.coordinator.finalize_replies.push(Reply::for_test(reply));
    state.environment_unavailable.store(true, Ordering::SeqCst);

    let result = fixture.coordinator.complete_readiness(true).await;
    assert!(result.is_ok(), "a temporary environment failure escaped the coordinator: {result:?}");
    assert!(fixture.coordinator.pending_ready && fixture.coordinator.ready_busy);
    assert!(!fixture.coordinator.ready_once);
    assert_eq!(fixture.coordinator.runtime.session().unwrap().phase, SessionPhase::Starting);
    assert_eq!(finalized.try_recv(), Err(tokio::sync::mpsc::error::TryRecvError::Empty));

    state.environment_unavailable.store(false, Ordering::SeqCst);
    state.ready.store(true, Ordering::SeqCst);
    *state.values.lock().unwrap() = vec!["WAYLAND_DISPLAY=wayland-restored".into()];
    fixture.coordinator.start_observation();
    let observed = tokio::time::timeout(Duration::from_secs(1), fixture.coordinator.work.recv()).await.unwrap().unwrap();
    assert!(matches!(observed, Work::Observed(_)));
    fixture.coordinator.completed(observed).await.unwrap();
    assert!(finalized.try_recv().unwrap().is_ok());
    assert_eq!(fixture.coordinator.lifecycle.phase, SessionPhase::Running);
    assert!(!fixture.coordinator.pending_ready && !fixture.coordinator.ready_busy);
    assert!(fixture.coordinator.runtime.session().unwrap().exported_environment.contains(&(
        "WAYLAND_DISPLAY".into(), "wayland-restored".into(),
    )));
}

#[tokio::test]
async fn temporary_target_read_failure_keeps_the_startup_budget_and_finalize_waiter() {
    let (mut fixture, _state) = starting_fixture(ProviderKind::Managed).await;
    fixture.coordinator.pending_ready = false;
    fixture.coordinator.workers = 1;
    let deadline = fixture.coordinator.ready_deadline;
    let (reply, mut finalized) = tokio::sync::mpsc::channel(1);
    fixture.coordinator.finalize_replies.push(Reply::for_test(reply));
    let error = zbus::Error::from(zbus::fdo::Error::NoReply("target state temporarily unavailable".into()));

    fixture.coordinator.completed(Work::TargetsChecked(Err(error.into()))).await.unwrap();
    assert_eq!(fixture.coordinator.ready_deadline, deadline, "a temporary target read expired the startup budget");
    assert_eq!(finalized.try_recv(), Err(tokio::sync::mpsc::error::TryRecvError::Empty));
    assert!(!fixture.coordinator.ready_busy && !fixture.coordinator.pending_ready);
    assert_eq!(fixture.coordinator.lifecycle.phase, SessionPhase::Starting);
    assert_eq!(fixture.coordinator.workers, 0);
}

#[tokio::test]
async fn permanent_environment_errors_are_not_deferred_as_a_reconnection() {
    let (mut fixture, state) = starting_fixture(ProviderKind::Niri).await;
    state.environment_denied.store(true, Ordering::SeqCst);
    let error = fixture.coordinator.complete_readiness(true).await.unwrap_err();
    assert!(error.to_string().contains("environment access denied"), "{error}");
    assert!(!fixture.coordinator.pending_ready && !fixture.coordinator.ready_busy);
}
