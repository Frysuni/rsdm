use std::sync::atomic::Ordering;

use super::*;

#[path = "apps_launch_fixture.rs"]
mod fixture;

use fixture::{Fixture, Mode};

#[test]
fn a_missing_executable_rolls_back_registration_without_contacting_systemd() {
    let mut fixture = Fixture::new(Mode::Accepted);
    fixture.missing_command();
    let error = fixture.launch().unwrap_err();
    assert!(error.to_string().contains("was not found in the launch environment"));
    assert!(fixture.runtime.apps().unwrap().is_empty());
    assert_eq!(fixture.state.starts.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.state.invocation_reads.load(Ordering::SeqCst), 0);
}

#[test]
fn a_rejected_start_removes_its_record_without_claiming_another_invocation() {
    for mode in [Mode::Denied, Mode::Collision] {
        let fixture = Fixture::new(mode);
        assert!(matches!(fixture.launch(), Err(SessionError::StartRejected { .. })));
        assert!(fixture.runtime.apps().unwrap().is_empty());
        assert_eq!(fixture.state.starts.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.state.invocation_reads.load(Ordering::SeqCst), 0);
        assert_eq!(fixture.state.unrefs.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn accepted_and_reconciled_lost_reply_launches_pin_the_owned_invocation() {
    for mode in [Mode::Accepted, Mode::LostReply] {
        let fixture = Fixture::new(mode);
        assert_eq!(fixture.launch().unwrap(), fixture.app.unit);
        assert_eq!(fixture.runtime.app(&fixture.app.unit).unwrap().invocation_id, vec![1; 16]);
        assert_eq!(fixture.state.starts.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.state.invocation_reads.load(Ordering::SeqCst), 2);
    }
}

#[test]
fn accepted_read_failures_keep_the_record_without_replaying_the_start() {
    for mode in [Mode::InvocationDenied, Mode::InspectionDenied] {
        let fixture = Fixture::new(mode);
        assert!(fixture.launch().is_err());
        assert!(fixture.runtime.app(&fixture.app.unit).unwrap().invocation_id.is_empty());
        assert_eq!(fixture.state.starts.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.state.unrefs.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn a_foreign_generation_after_start_cannot_be_pinned_for_later_cleanup() {
    let fixture = Fixture::new(Mode::Foreign);
    assert!(fixture.launch().is_err());
    assert!(fixture.runtime.app(&fixture.app.unit).unwrap().invocation_id.is_empty());
    assert_eq!(fixture.state.starts.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.state.unrefs.load(Ordering::SeqCst), 0);
}

#[test]
fn a_partially_created_unit_remains_registered_until_ordinary_cleanup() {
    let fixture = Fixture::new(Mode::PartialDenied);
    assert!(fixture.launch().is_err());
    assert_eq!(fixture.runtime.app(&fixture.app.unit).unwrap().invocation_id, vec![0; 16]);
    assert_eq!(fixture.state.starts.load(Ordering::SeqCst), 1);
    release_closed(&fixture.manager, &fixture.runtime).unwrap();
    assert!(fixture.runtime.apps().unwrap().is_empty());
    assert_eq!(fixture.state.unrefs.load(Ordering::SeqCst), 1);
}

#[test]
fn rollback_never_removes_a_registration_claimed_by_another_invocation() {
    let mut fixture = Fixture::new(Mode::Accepted);
    let mut pinned = fixture.app.clone();
    pinned.invocation_id = vec![1; 16];
    fixture.runtime.save_app(&pinned).unwrap();
    fixture.missing_command();
    let error = fixture.launch().unwrap_err();
    assert!(error.to_string().contains("registration changed before rollback"));
    assert_eq!(fixture.runtime.app(&fixture.app.unit).unwrap().invocation_id, vec![1; 16]);
    assert_eq!(fixture.state.starts.load(Ordering::SeqCst), 0);
}

#[test]
fn contended_rollback_reports_both_errors_and_can_be_retried_after_release() {
    let mut fixture = Fixture::new(Mode::Accepted);
    fixture.missing_command();
    let lease = fixture.runtime.app_lease(&fixture.app.unit).unwrap();
    let error = fixture.launch().unwrap_err().to_string();
    assert!(error.contains("was not found in the launch environment"));
    assert!(error.contains("could not roll back application registration"));
    assert!(fixture.runtime.app(&fixture.app.unit).unwrap().invocation_id.is_empty());
    drop(lease);
    assert!(fixture.launch().is_err());
    assert!(fixture.runtime.apps().unwrap().is_empty());
    assert_eq!(fixture.state.starts.load(Ordering::SeqCst), 0);
}

#[test]
fn a_restored_peer_reference_keeps_its_registration_for_recovery() {
    let fixture = Fixture::new(Mode::Accepted);
    let error = SessionError::StartRejected {
        error: Box::new(SessionError::State("rejected after peer replacement".into())),
        reference_retained: true,
    };
    let error = rollback_launch(&fixture.manager, &fixture.runtime, &fixture.app, error);
    assert!(matches!(error, SessionError::StartRejected { reference_retained: true, .. }));
    assert!(fixture.runtime.app(&fixture.app.unit).unwrap().invocation_id.is_empty());
    assert_eq!(fixture.state.starts.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.state.invocation_reads.load(Ordering::SeqCst), 0);
}
