use std::sync::atomic::Ordering;

use super::*;

#[path = "apps_launch_fixture.rs"]
mod fixture;

use fixture::{Fixture, Mode};

#[tokio::test]
async fn attached_apps_keep_private_xsmp_and_have_no_graphical_lifetime_dependencies() {
    let mut fixture = Fixture::new(Mode::Accepted).await;
    let mut record = fixture.runtime.session().unwrap();
    record.provider = "partial".into();
    record.exported_environment = vec![("SESSION_MANAGER".into(), "private-xsmp".into()), ("VALUE".into(), "default".into())];
    fixture.runtime.save_session(&record).unwrap();
    fixture.request.environment = vec![("SESSION_MANAGER".into(), "foreign-xsmp".into()), ("VALUE".into(), "caller".into())];
    let properties = launch_properties(&fixture.runtime, "foreign-anchor.service", &fixture.request, &fixture.app, None).await.unwrap();
    assert!(properties.iter().all(|(name, _)| !matches!(*name, "After" | "PartOf" | "Requisite")));
    let value = properties.into_iter().find(|(name, _)| *name == "Environment").unwrap().1;
    let environment: Vec<String> = zbus::zvariant::OwnedValue::try_from(value).unwrap().try_into().unwrap();
    assert!(environment.contains(&"SESSION_MANAGER=private-xsmp".into()));
    assert!(environment.contains(&"VALUE=caller".into()));
    assert!(!environment.contains(&"SESSION_MANAGER=foreign-xsmp".into()));
}

#[tokio::test]
async fn stop_hooks_preserve_the_coordinators_config_path_as_one_argument() {
    let fixture = Fixture::new(Mode::Accepted).await;
    let path = Path::new("/run/custom config/rsdm.toml");
    let properties = launch_properties(&fixture.runtime, "anchor.service", &fixture.request, &fixture.app, Some(path)).await.unwrap();
    let value = properties.into_iter().find(|(name, _)| *name == "ExecStopEx").unwrap().1;
    let commands: Vec<(String, Vec<String>, Vec<String>)> = zbus::zvariant::OwnedValue::try_from(value).unwrap().try_into().unwrap();
    let (executable, argv, flags) = &commands[0];
    assert_eq!(argv, &vec![executable.clone(), "--config".into(), path.to_str().unwrap().into(),
        "session".into(), "app-stop".into(), "--generation".into(), fixture.runtime.generation.clone(),
        "--unit".into(), fixture.app.unit.clone()]);
    assert_eq!(flags, &["no-env-expand"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_missing_executable_rolls_back_registration_without_contacting_systemd() {
    let mut fixture = Fixture::new(Mode::Accepted).await;
    fixture.missing_command();
    let error = fixture.launch().await.unwrap_err();
    assert!(error.to_string().contains("was not found in the launch environment"));
    assert!(fixture.runtime.apps().unwrap().is_empty());
    assert_eq!(fixture.state.starts.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.state.invocation_reads.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rejected_start_removes_its_record_without_claiming_another_invocation() {
    for mode in [Mode::Denied, Mode::Collision] {
        let fixture = Fixture::new(mode).await;
        assert!(matches!(fixture.launch().await, Err(SessionError::StartRejected { .. })));
        assert!(fixture.runtime.apps().unwrap().is_empty());
        assert_eq!(fixture.state.starts.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.state.invocation_reads.load(Ordering::SeqCst), 0);
        assert_eq!(fixture.state.unrefs.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn accepted_and_reconciled_lost_reply_launches_pin_the_owned_invocation() {
    for mode in [Mode::Accepted, Mode::LostReply] {
        let fixture = Fixture::new(mode).await;
        assert_eq!(fixture.launch().await.unwrap(), fixture.app.unit);
        assert_eq!(fixture.runtime.app(&fixture.app.unit).unwrap().invocation_id, vec![1; 16]);
        assert_eq!(fixture.state.starts.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.state.invocation_reads.load(Ordering::SeqCst), 2);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn accepted_read_failures_keep_the_record_without_replaying_the_start() {
    for mode in [Mode::InvocationDenied, Mode::InspectionDenied] {
        let fixture = Fixture::new(mode).await;
        assert!(fixture.launch().await.is_err());
        assert!(fixture.runtime.app(&fixture.app.unit).unwrap().invocation_id.is_empty());
        assert_eq!(fixture.state.starts.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.state.unrefs.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_foreign_generation_after_start_cannot_be_pinned_for_later_cleanup() {
    let fixture = Fixture::new(Mode::Foreign).await;
    assert!(fixture.launch().await.is_err());
    assert!(fixture.runtime.app(&fixture.app.unit).unwrap().invocation_id.is_empty());
    assert_eq!(fixture.state.starts.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.state.unrefs.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_partially_created_unit_remains_registered_until_ordinary_cleanup() {
    let fixture = Fixture::new(Mode::PartialDenied).await;
    assert!(fixture.launch().await.is_err());
    assert_eq!(fixture.runtime.app(&fixture.app.unit).unwrap().invocation_id, vec![0; 16]);
    assert_eq!(fixture.state.starts.load(Ordering::SeqCst), 1);
    release_closed(&fixture.manager, &fixture.runtime).await.unwrap();
    assert!(fixture.runtime.apps().unwrap().is_empty());
    assert_eq!(fixture.state.unrefs.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rollback_never_removes_a_registration_claimed_by_another_invocation() {
    let mut fixture = Fixture::new(Mode::Accepted).await;
    let mut pinned = fixture.app.clone();
    pinned.invocation_id = vec![1; 16];
    fixture.runtime.save_app(&pinned).unwrap();
    fixture.missing_command();
    let error = fixture.launch().await.unwrap_err();
    assert!(error.to_string().contains("registration changed before rollback"));
    assert_eq!(fixture.runtime.app(&fixture.app.unit).unwrap().invocation_id, vec![1; 16]);
    assert_eq!(fixture.state.starts.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn contended_rollback_reports_both_errors_and_can_be_retried_after_release() {
    let mut fixture = Fixture::new(Mode::Accepted).await;
    fixture.missing_command();
    let lease = fixture.runtime.app_lease(&fixture.app.unit).unwrap();
    let error = fixture.launch().await.unwrap_err().to_string();
    assert!(error.contains("was not found in the launch environment"));
    assert!(error.contains("could not roll back application registration"));
    assert!(fixture.runtime.app(&fixture.app.unit).unwrap().invocation_id.is_empty());
    drop(lease);
    assert!(fixture.launch().await.is_err());
    assert!(fixture.runtime.apps().unwrap().is_empty());
    assert_eq!(fixture.state.starts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_restored_peer_reference_keeps_its_registration_for_recovery() {
    let fixture = Fixture::new(Mode::Accepted).await;
    let error = SessionError::StartRejected {
        error: Box::new(SessionError::State("rejected after peer replacement".into())),
        reference_retained: true,
    };
    let error = rollback_launch(&fixture.manager, &fixture.runtime, &fixture.app, error).await;
    assert!(matches!(error, SessionError::StartRejected { reference_retained: true, .. }));
    assert!(fixture.runtime.app(&fixture.app.unit).unwrap().invocation_id.is_empty());
    assert_eq!(fixture.state.starts.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.state.invocation_reads.load(Ordering::SeqCst), 0);
}
