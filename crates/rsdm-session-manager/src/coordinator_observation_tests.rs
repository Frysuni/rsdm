//! Regression checks for observation worker ownership and lifecycle ordering.

use super::*;
use crate::{control::Reply, session_process::SessionProcess};

async fn pending_readiness() -> (Fixture, tokio::sync::mpsc::Receiver<Result<(), String>>) {
    let mut fixture = Fixture::new().await;
    let state = completion::install_manager(&fixture).await;
    state.unavailable.store(false, Ordering::SeqCst);
    fixture.coordinator.provider.kind = ProviderKind::Managed;
    fixture.coordinator.lifecycle.phase = SessionPhase::Starting;
    fixture.coordinator.ready_once = false;
    fixture.coordinator.ready_busy = true;
    fixture.coordinator.pending_ready = true;
    let (reply, response) = tokio::sync::mpsc::channel(1);
    fixture.coordinator.finalize_replies.push(Reply::for_test(reply));
    (fixture, response)
}

async fn cancel_logout(fixture: &mut Fixture) {
    let (reply, mut response) = tokio::sync::mpsc::channel(1);
    fixture.coordinator.request(crate::control::Request::Cancel {
        generation: GENERATION.into(), reply: Reply::for_test(reply),
    }).await.unwrap();
    assert!(response.try_recv().unwrap().is_ok());
}

#[tokio::test]
async fn cancelling_logout_keeps_an_inflight_readiness_confirmation() {
    let (mut fixture, mut finalized) = pending_readiness().await;
    fixture.coordinator.start_observation();
    let observed = tokio::time::timeout(Duration::from_secs(2), fixture.coordinator.work.recv()).await.unwrap().unwrap();
    fixture.coordinator.begin_stop("logout", None).await.unwrap();
    cancel_logout(&mut fixture).await;

    fixture.coordinator.completed(observed).await.unwrap();
    assert!(finalized.try_recv().unwrap().is_ok());
    assert_eq!(fixture.coordinator.lifecycle.phase, SessionPhase::Running);
    assert_eq!(fixture.coordinator.runtime.session().unwrap().phase, SessionPhase::Running);
    assert!(fixture.coordinator.ready_once);
    assert!(!fixture.coordinator.ready_busy && !fixture.coordinator.pending_ready);
    assert_eq!(fixture.coordinator.workers, 0);
}

#[tokio::test]
async fn cancelling_logout_retries_readiness_sampled_during_preparation() {
    let mut failures = Vec::new();
    for already_resumed in [false, true] {
        let (mut fixture, mut finalized) = pending_readiness().await;
        fixture.coordinator.begin_stop("logout", None).await.unwrap();
        fixture.coordinator.start_observation();
        let observed = tokio::time::timeout(Duration::from_secs(2), fixture.coordinator.work.recv()).await.unwrap().unwrap();
        cancel_logout(&mut fixture).await;
        if already_resumed { assert!(fixture.coordinator.resume_if_cancelled().await.unwrap()); }

        fixture.coordinator.completed(observed).await.unwrap();
        let actual = (fixture.coordinator.lifecycle.phase, fixture.coordinator.ready_busy,
            fixture.coordinator.pending_ready, finalized.try_recv());
        let expected = (SessionPhase::Starting, true, true, Err(tokio::sync::mpsc::error::TryRecvError::Empty));
        if actual != expected { failures.push((already_resumed, actual)); continue; }

        fixture.coordinator.start_observation();
        let observed = tokio::time::timeout(Duration::from_secs(2), fixture.coordinator.work.recv()).await.unwrap().unwrap();
        fixture.coordinator.completed(observed).await.unwrap();
        assert!(finalized.try_recv().unwrap().is_ok());
        assert_eq!(fixture.coordinator.runtime.session().unwrap().phase, SessionPhase::Running);
        assert_eq!(fixture.coordinator.workers, 0);
    }
    assert!(failures.is_empty(), "a shutdown snapshot rejected accepted readiness: {failures:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn observation_preserves_the_process_until_the_start_worker_completes() {
    let mut fixture = Fixture::new().await;
    fixture.coordinator.provider.kind = ProviderKind::Managed;
    fixture.coordinator.lifecycle.phase = SessionPhase::Starting;
    fixture.coordinator.ready_once = false;
    fixture.coordinator.process = Some(SessionProcess::launch(
        &["true".into()], &[], &fixture.directory, &fixture.coordinator.provider,
        "example.service".into(),
    ).unwrap());
    fixture.coordinator.workers = 1;

    fixture.coordinator.start_observation();
    assert!(!fixture.coordinator.observation_busy);
    assert!(fixture.coordinator.process.is_some());
    fixture.coordinator.completed(Work::Boot(Ok(vec![3; 16]))).await.unwrap();
    assert!(!fixture.coordinator.booting());
    assert_eq!(fixture.coordinator.process.as_ref().unwrap().invocation, vec![3; 16]);
    assert_eq!(fixture.coordinator.record.compositor_invocation, vec![3; 16]);
    let saved = fixture.coordinator.runtime.session().unwrap();
    assert_eq!(saved.compositor_invocation, vec![3; 16]);
    assert_eq!(saved.phase, SessionPhase::Starting);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stopping_does_not_start_observations_that_starve_preparation() {
    let mut fixture = Fixture::new().await;
    fixture.coordinator.begin_stop("logout", None).await.unwrap();

    fixture.coordinator.start_observation();
    fixture.coordinator.advance().unwrap();
    assert!(!fixture.coordinator.observation_busy);
    assert!(fixture.coordinator.preparing);
    let work = tokio::time::timeout(Duration::from_secs(8), fixture.coordinator.work.recv()).await.unwrap().unwrap();
    assert!(matches!(work, Work::Prepared(Ok(_), None)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stale_readiness_cannot_acknowledge_or_restart_a_stopping_session() {
    let mut fixture = Fixture::new().await;
    fixture.coordinator.ready_once = false;
    fixture.coordinator.ready_busy = true;
    fixture.coordinator.pending_ready = true;
    let (reply, mut response) = tokio::sync::mpsc::channel(1);
    fixture.coordinator.finalize_replies.push(Reply::for_test(reply));
    fixture.coordinator.begin_stop("logout", None).await.unwrap();

    fixture.coordinator.complete_readiness(true).await.unwrap();
    fixture.coordinator.activate();
    assert!(response.try_recv().unwrap().is_err());
    assert!(!fixture.coordinator.ready_once);
    assert!(!fixture.coordinator.ready_busy);
    assert_eq!(fixture.coordinator.lifecycle.phase, SessionPhase::Preparing);
    assert_eq!(fixture.coordinator.workers, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_inflight_observation_returns_ownership_before_shutdown_advances() {
    let mut fixture = Fixture::new().await;
    completion::install_manager(&fixture).await;
    fixture.coordinator.start_observation();
    assert!(fixture.coordinator.observation_busy);
    fixture.coordinator.begin_stop("logout", None).await.unwrap();
    fixture.coordinator.advance().unwrap();
    assert!(!fixture.coordinator.preparing);

    let work = tokio::time::timeout(Duration::from_secs(8), fixture.coordinator.work.recv()).await.unwrap().unwrap();
    fixture.coordinator.completed(work).await.unwrap();
    fixture.coordinator.start_observation();
    fixture.coordinator.advance().unwrap();
    assert!(!fixture.coordinator.observation_busy);
    assert!(fixture.coordinator.preparing);
    let work = tokio::time::timeout(Duration::from_secs(8), fixture.coordinator.work.recv()).await.unwrap().unwrap();
    assert!(matches!(work, Work::Prepared(Ok(_), None)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recovery_reclaims_the_launcher_from_a_completed_observation() {
    let mut fixture = Fixture::new().await;
    completion::install_manager(&fixture).await;
    fixture.coordinator.process = Some(SessionProcess::launch(
        &["true".into()], &[], &fixture.directory, &fixture.coordinator.provider,
        "unused.service".into(),
    ).unwrap());
    fixture.coordinator.start_observation();
    assert!(fixture.coordinator.process.is_none());
    let work = tokio::time::timeout(Duration::from_secs(8), fixture.coordinator.work.recv()).await.unwrap().unwrap();
    assert!(matches!(work, Work::Observed(_)));
    fixture.coordinator.events.send(work).unwrap();
    fixture.coordinator.manager.deadline.set(monotonic_usec().unwrap() + 100_000);

    fixture.coordinator.recover().await;
    assert!(fixture.coordinator.process.is_some());
    assert!(!fixture.coordinator.observation_busy);
    assert_eq!(fixture.coordinator.workers, 0);
}
