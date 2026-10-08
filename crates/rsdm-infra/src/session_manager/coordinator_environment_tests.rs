use super::*;
use crate::session_manager::control::{Reply, Request};

#[path = "coordinator_environment_fixture.rs"]
mod peer;
use peer::{install, pause};

fn finalize(fixture: &mut Fixture, value: &str) -> async_channel::Receiver<Result<(), String>> {
    let (reply, response) = async_channel::bounded(1);
    fixture.coordinator.request(Request::Finalize {
        generation: GENERATION.into(), environment: vec![("WAYLAND_DISPLAY".into(), value.into())],
        reply: Reply::for_test(reply),
    }).unwrap();
    response
}

fn completed_publication(fixture: &mut Fixture) {
    let work = fixture.coordinator.work.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(matches!(work, Work::EnvironmentPublished(_)));
    fixture.coordinator.completed(work).unwrap();
}

#[test]
fn repeated_finalize_updates_environment_without_reactivating_a_running_session() {
    let mut fixture = Fixture::new();
    let state = install(&fixture);
    for value in ["wayland-1", "wayland-2"] {
        let response = finalize(&mut fixture, value);
        completed_publication(&mut fixture);
        assert!(response.try_recv().unwrap().is_ok());
        assert_eq!(fixture.coordinator.lifecycle.phase, SessionPhase::Running);
        assert_eq!(fixture.coordinator.workers, 0);
        assert!(!fixture.coordinator.ready_busy);
        assert!(fixture.coordinator.finalize_replies.is_empty());
        assert!(fixture.coordinator.runtime.session().unwrap().exported_environment.contains(&(
            "WAYLAND_DISPLAY".into(), value.into(),
        )));
    }
    assert_eq!(*state.writes.lock().unwrap(), [vec!["WAYLAND_DISPLAY=wayland-1"], vec!["WAYLAND_DISPLAY=wayland-2"]]);
    assert_eq!(state.activations.load(Ordering::SeqCst), 2);
    assert_eq!(state.reads.load(Ordering::SeqCst), 0);
}

#[test]
fn a_paused_publication_does_not_block_status_or_cancellation() {
    let mut fixture = Fixture::new();
    let state = install(&fixture);
    let (entered, resume) = pause(&state);
    let before = Instant::now();
    let response = finalize(&mut fixture, "wayland-new");
    assert!(before.elapsed() < Duration::from_secs(1));
    entered.recv_timeout(Duration::from_secs(1)).unwrap();
    assert_eq!(fixture.coordinator.workers, 1);
    assert!(fixture.coordinator.runtime.session().unwrap().exported_environment.contains(&(
        "WAYLAND_DISPLAY".into(), "wayland-new".into(),
    )));

    let (reply, status) = async_channel::bounded(1);
    fixture.coordinator.request(Request::Status(Reply::for_test(reply))).unwrap();
    assert_eq!(status.try_recv().unwrap().unwrap().phase, "running");
    fixture.coordinator.begin_stop("logout", None).unwrap();
    let (reply, cancelled) = async_channel::bounded(1);
    fixture.coordinator.request(Request::Cancel { generation: GENERATION.into(), reply: Reply::for_test(reply) }).unwrap();
    assert!(cancelled.try_recv().unwrap().is_ok());
    fixture.coordinator.advance().unwrap();
    assert!(!fixture.coordinator.preparing, "teardown must drain the accepted publication first");

    resume.try_send(()).unwrap();
    completed_publication(&mut fixture);
    assert!(response.try_recv().unwrap().is_ok());
    assert_eq!(fixture.coordinator.runtime.session().unwrap().phase, SessionPhase::Running);
    assert_eq!(fixture.coordinator.lifecycle.phase, SessionPhase::Running);
    assert_eq!(fixture.coordinator.workers, 0);
}

#[test]
fn queued_publications_run_serially_without_overwriting_recorded_intent() {
    let mut fixture = Fixture::new();
    let state = install(&fixture);
    let (entered, resume) = pause(&state);
    let first = finalize(&mut fixture, "wayland-first");
    entered.recv_timeout(Duration::from_secs(1)).unwrap();
    let second = finalize(&mut fixture, "wayland-second");
    assert_eq!(fixture.coordinator.workers, 1);
    assert_eq!(fixture.coordinator.pending_environment.len(), 1);
    assert_eq!(*state.writes.lock().unwrap(), [vec!["WAYLAND_DISPLAY=wayland-first"]]);
    assert!(fixture.coordinator.runtime.session().unwrap().exported_environment.contains(&(
        "WAYLAND_DISPLAY".into(), "wayland-first".into(),
    )));
    resume.try_send(()).unwrap();
    completed_publication(&mut fixture);
    assert!(first.try_recv().unwrap().is_ok());
    assert_eq!(fixture.coordinator.workers, 1);
    completed_publication(&mut fixture);
    assert!(second.try_recv().unwrap().is_ok());
    assert_eq!(*state.writes.lock().unwrap(), [vec!["WAYLAND_DISPLAY=wayland-first"], vec!["WAYLAND_DISPLAY=wayland-second"]]);
    assert_eq!(fixture.coordinator.workers, 0);
    assert!(fixture.coordinator.pending_environment.is_empty());
    assert!(fixture.coordinator.runtime.session().unwrap().exported_environment.contains(&(
        "WAYLAND_DISPLAY".into(), "wayland-second".into(),
    )));
}

#[test]
fn shutdown_rejects_queued_environment_without_publishing_it() {
    let mut fixture = Fixture::new();
    let state = install(&fixture);
    let (entered, resume) = pause(&state);
    let first = finalize(&mut fixture, "wayland-first");
    entered.recv_timeout(Duration::from_secs(1)).unwrap();
    let second = finalize(&mut fixture, "wayland-second");
    fixture.coordinator.begin_stop("logout", None).unwrap();
    assert!(second.try_recv().unwrap().unwrap_err().contains("before environment publication"));
    assert!(fixture.coordinator.pending_environment.is_empty());
    resume.try_send(()).unwrap();
    completed_publication(&mut fixture);
    assert!(first.try_recv().unwrap().is_err());
    assert_eq!(*state.writes.lock().unwrap(), [vec!["WAYLAND_DISPLAY=wayland-first"]]);
    assert_eq!(fixture.coordinator.workers, 0);
}

#[test]
fn partial_publication_failure_preserves_environment_ownership_for_recovery() {
    let mut fixture = Fixture::new();
    let state = install(&fixture);
    state.fail_activation.store(true, Ordering::SeqCst);
    let failed = finalize(&mut fixture, "wayland-failed");
    completed_publication(&mut fixture);
    assert!(failed.try_recv().unwrap().unwrap_err().contains("activation publication rejected"));
    assert_eq!(*state.values.lock().unwrap(), ["WAYLAND_DISPLAY=wayland-failed"]);
    assert!(fixture.coordinator.runtime.session().unwrap().exported_environment.contains(&(
        "WAYLAND_DISPLAY".into(), "wayland-failed".into(),
    )));
    state.fail_activation.store(false, Ordering::SeqCst);
    let next = finalize(&mut fixture, "wayland-next");
    completed_publication(&mut fixture);
    assert!(next.try_recv().unwrap().is_ok());
    assert_eq!(fixture.coordinator.workers, 0);
}

#[test]
fn recovery_interrupts_pending_publication_and_never_publishes_queued_values() {
    let mut fixture = Fixture::new();
    let state = install(&fixture);
    let (entered, resume) = pause(&state);
    let first = finalize(&mut fixture, "wayland-first");
    entered.recv_timeout(Duration::from_secs(1)).unwrap();
    let second = finalize(&mut fixture, "wayland-second");
    fixture.coordinator.manager.deadline.set(monotonic_usec().unwrap() + 50_000);
    let before = Instant::now();
    fixture.coordinator.recover();
    assert!(before.elapsed() < Duration::from_secs(1));
    assert!(first.try_recv().unwrap().is_err());
    assert!(second.try_recv().unwrap().is_err());
    assert!(fixture.coordinator.pending_environment.is_empty());
    assert!(fixture.coordinator.environment_reply.is_none());
    let record = fixture.coordinator.runtime.session().unwrap();
    assert_eq!(record.phase, SessionPhase::Preparing);
    assert!(record.exported_environment.contains(&("WAYLAND_DISPLAY".into(), "wayland-first".into())));
    assert!(!record.exported_environment.contains(&("WAYLAND_DISPLAY".into(), "wayland-second".into())));
    assert_eq!(*state.writes.lock().unwrap(), [vec!["WAYLAND_DISPLAY=wayland-first"]]);
    resume.try_send(()).unwrap();
}

#[test]
fn cancelling_logout_preserves_an_accepted_startup_finalize_until_readiness() {
    let mut fixture = Fixture::new();
    let state = install(&fixture);
    fixture.coordinator.lifecycle.phase = SessionPhase::Starting;
    fixture.coordinator.ready_once = false;
    fixture.coordinator.ready_busy = true;
    fixture.coordinator.workers = 1;
    let (entered, resume) = pause(&state);
    let response = finalize(&mut fixture, "wayland-startup");
    entered.recv_timeout(Duration::from_secs(1)).unwrap();
    fixture.coordinator.begin_stop("logout", None).unwrap();
    let (reply, cancelled) = async_channel::bounded(1);
    fixture.coordinator.request(Request::Cancel { generation: GENERATION.into(), reply: Reply::for_test(reply) }).unwrap();
    assert!(cancelled.try_recv().unwrap().is_ok());
    resume.try_send(()).unwrap();
    completed_publication(&mut fixture);
    assert!(response.try_recv().is_err());
    assert_eq!(fixture.coordinator.lifecycle.phase, SessionPhase::Starting);
    assert_eq!(fixture.coordinator.finalize_replies.len(), 1);
    state.ready.store(true, Ordering::SeqCst);
    fixture.coordinator.completed(Work::Ready(Ok(false))).unwrap();
    fixture.coordinator.start_observation();
    let work = fixture.coordinator.work.recv_timeout(Duration::from_secs(2)).unwrap();
    fixture.coordinator.completed(work).unwrap();
    assert!(response.try_recv().unwrap().is_ok());
    assert_eq!(fixture.coordinator.lifecycle.phase, SessionPhase::Running);
    assert_eq!(fixture.coordinator.workers, 0);
}

#[test]
fn failed_record_publication_prevents_all_environment_side_effects() {
    let mut fixture = Fixture::new();
    let state = install(&fixture);
    let record = fixture.coordinator.runtime.path.join("session.toml");
    std::fs::remove_file(&record).unwrap();
    std::fs::create_dir(&record).unwrap();
    let response = finalize(&mut fixture, "wayland-never-published");
    assert!(response.try_recv().unwrap().is_err());
    assert_eq!(fixture.coordinator.workers, 0);
    assert!(fixture.coordinator.environment_reply.is_none());
    assert!(state.writes.lock().unwrap().is_empty());
    assert_eq!(state.activations.load(Ordering::SeqCst), 0);
}
