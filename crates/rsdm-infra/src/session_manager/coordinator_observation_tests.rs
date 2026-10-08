//! Regression checks for observation worker ownership and lifecycle ordering.

use super::*;
use crate::session_manager::{control::Reply, session_process::SessionProcess};

#[test]
fn observation_preserves_the_process_until_the_start_worker_completes() {
    let mut fixture = Fixture::new();
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
    fixture.coordinator.completed(Work::Boot(Ok(vec![3; 16]))).unwrap();
    assert!(!fixture.coordinator.booting());
    assert_eq!(fixture.coordinator.process.as_ref().unwrap().invocation, vec![3; 16]);
    assert_eq!(fixture.coordinator.record.compositor_invocation, vec![3; 16]);
}

#[test]
fn stopping_does_not_start_observations_that_starve_preparation() {
    let mut fixture = Fixture::new();
    fixture.coordinator.begin_stop("logout", None).unwrap();

    fixture.coordinator.start_observation();
    fixture.coordinator.advance().unwrap();
    assert!(!fixture.coordinator.observation_busy);
    assert!(fixture.coordinator.preparing);
    let work = fixture.coordinator.work.recv_timeout(Duration::from_secs(8)).unwrap();
    assert!(matches!(work, Work::Prepared(Ok(_))));
}

#[test]
fn stale_readiness_cannot_acknowledge_or_restart_a_stopping_session() {
    let mut fixture = Fixture::new();
    fixture.coordinator.ready_once = false;
    fixture.coordinator.ready_busy = true;
    fixture.coordinator.pending_ready = true;
    let (reply, response) = async_channel::bounded(1);
    fixture.coordinator.finalize_replies.push(Reply::for_test(reply));
    fixture.coordinator.begin_stop("logout", None).unwrap();

    fixture.coordinator.complete_readiness(true).unwrap();
    fixture.coordinator.activate();
    assert!(response.try_recv().unwrap().is_err());
    assert!(!fixture.coordinator.ready_once);
    assert!(!fixture.coordinator.ready_busy);
    assert_eq!(fixture.coordinator.lifecycle.phase, SessionPhase::Preparing);
    assert_eq!(fixture.coordinator.workers, 0);
}

#[test]
fn an_inflight_observation_returns_ownership_before_shutdown_advances() {
    let mut fixture = Fixture::new();
    completion::install_manager(&fixture);
    fixture.coordinator.start_observation();
    assert!(fixture.coordinator.observation_busy);
    fixture.coordinator.begin_stop("logout", None).unwrap();
    fixture.coordinator.advance().unwrap();
    assert!(!fixture.coordinator.preparing);

    let work = fixture.coordinator.work.recv_timeout(Duration::from_secs(8)).unwrap();
    fixture.coordinator.completed(work).unwrap();
    fixture.coordinator.start_observation();
    fixture.coordinator.advance().unwrap();
    assert!(!fixture.coordinator.observation_busy);
    assert!(fixture.coordinator.preparing);
    let work = fixture.coordinator.work.recv_timeout(Duration::from_secs(8)).unwrap();
    assert!(matches!(work, Work::Prepared(Ok(_))));
}

#[test]
fn recovery_reclaims_the_launcher_from_a_completed_observation() {
    let mut fixture = Fixture::new();
    completion::install_manager(&fixture);
    fixture.coordinator.process = Some(SessionProcess::launch(
        &["true".into()], &[], &fixture.directory, &fixture.coordinator.provider,
        "unused.service".into(),
    ).unwrap());
    fixture.coordinator.start_observation();
    assert!(fixture.coordinator.process.is_none());
    let work = fixture.coordinator.work.recv_timeout(Duration::from_secs(8)).unwrap();
    assert!(matches!(work, Work::Observed(_)));
    fixture.coordinator.events.send(work).unwrap();
    fixture.coordinator.manager.deadline.set(monotonic_usec().unwrap() + 100_000);

    fixture.coordinator.recover();
    assert!(fixture.coordinator.process.is_some());
    assert!(!fixture.coordinator.observation_busy);
    assert_eq!(fixture.coordinator.workers, 0);
}
