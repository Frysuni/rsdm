use super::*;
use crate::session_manager::control::{Reply, Request};

#[test]
fn recovery_worker_drain_cannot_extend_an_active_shutdown_deadline() {
    let mut fixture = Fixture::new();
    let deadline = monotonic_usec().unwrap() + 50_000;
    fixture.coordinator.begin_stop("external-shutdown", Some(deadline)).unwrap();
    fixture.coordinator.workers = 1;
    let before = Instant::now();
    fixture.coordinator.recover();
    assert!(before.elapsed() < Duration::from_secs(1));
    assert_eq!(fixture.coordinator.workers, 1);
    assert_eq!(fixture.coordinator.manager.deadline.get(), deadline);
    assert_eq!(fixture.deadline(), deadline);
    let record = fixture.coordinator.runtime.session().unwrap();
    assert_eq!(record.shutdown_deadline_usec, Some(deadline));
    assert_eq!(record.phase, SessionPhase::Preparing);
}

#[test]
fn a_saved_budget_bounds_worker_drain_before_shutdown_control_exists() {
    let mut fixture = Fixture::new();
    let deadline = monotonic_usec().unwrap() + 50_000;
    fixture.coordinator.record.shutdown_deadline_usec = Some(deadline);
    fixture.coordinator.workers = 1;
    let before = Instant::now();
    fixture.coordinator.recover();
    assert!(before.elapsed() < Duration::from_secs(1));
    assert_eq!(fixture.coordinator.manager.deadline.get(), deadline);
    assert!(fixture.coordinator.shutdown.is_none());
    assert_eq!(fixture.coordinator.runtime.session().unwrap().shutdown_deadline_usec, Some(deadline));
}

#[test]
fn expired_recovery_skips_worker_waits_and_preserves_owned_state() {
    let mut fixture = Fixture::new();
    fixture.coordinator.record.compositor_invocation = vec![7; 16];
    fixture.coordinator.record.owns_targets = true;
    fixture.coordinator.manager.deadline.set(1);
    fixture.coordinator.workers = 1;
    let before = Instant::now();
    fixture.coordinator.recover();
    assert!(before.elapsed() < Duration::from_secs(1));
    let record = fixture.coordinator.runtime.session().unwrap();
    assert_eq!(record.shutdown_deadline_usec, Some(1));
    assert_eq!(record.compositor_invocation, vec![7; 16]);
    assert!(record.owns_targets);
    assert_eq!(fixture.coordinator.workers, 1);
}

#[test]
fn recovery_consumes_completed_work_and_rejects_pending_requests_inside_its_budget() {
    let mut fixture = Fixture::new();
    let (sender, received) = std::sync::mpsc::sync_channel(1);
    fixture.coordinator.requests = received;
    let (reply, response) = async_channel::bounded(1);
    sender.send(Request::Status(Reply::for_test(reply))).unwrap();
    fixture.coordinator.events.send(Work::Boot(Ok(vec![3; 16]))).unwrap();
    fixture.coordinator.workers = 1;
    fixture.coordinator.recover();
    assert_eq!(fixture.coordinator.workers, 0);
    assert_eq!(fixture.coordinator.runtime.session().unwrap().compositor_invocation, vec![3; 16]);
    assert_eq!(response.try_recv().unwrap().unwrap_err(), "session coordinator failed");
}
