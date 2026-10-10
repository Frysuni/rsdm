//! Distinguish rejected mutations from ambiguous replies and later read errors.

use std::sync::atomic::Ordering;

use super::{*, fixture::{fixture, state, Rejection}};

fn properties() -> UnitProperties {
    vec![("AddRef", Value::Bool(true)),
         ("Environment", Value::new(vec!["RSDM_SESSION_GENERATION=ours".to_string()]))]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicit_rejections_without_a_created_unit_roll_back_the_reference() {
    for rejection in [Rejection::InvalidArgs, Rejection::AccessDenied, Rejection::UnknownProperty,
                      Rejection::PropertyReadOnly, Rejection::NotSupported] {
        let mut initial = state(false, false, 0);
        initial.rejection = Some(rejection);
        let (manager, _server, state) = fixture(initial).await;
        assert!(matches!(manager.start_service("example.service", &properties()).await, Err(SessionError::StartRejected { reference_retained: false, .. })));
        assert!(!manager.transport.has_reference("example.service"));
        assert_eq!(state.calls.load(Ordering::SeqCst), 1);
        assert_eq!(state.queries.load(Ordering::SeqCst), 2);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unit_exists_rejection_never_claims_or_probes_the_colliding_unit() {
    let mut initial = state(false, true, 0);
    initial.rejection = Some(Rejection::UnitExists);
    initial.created_on_rejection = true;
    initial.generation = "foreign";
    let (manager, _server, state) = fixture(initial).await;
    assert!(matches!(manager.start_service("example.service", &properties()).await, Err(SessionError::StartRejected { reference_retained: false, .. })));
    assert!(!manager.transport.has_reference("example.service"));
    assert_eq!(state.queries.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejected_calls_that_created_a_partial_unit_keep_ownership_for_cleanup() {
    for rejection in [Rejection::InvalidArgs, Rejection::AccessDenied] {
        let mut initial = state(false, false, 0);
        initial.rejection = Some(rejection);
        initial.created_on_rejection = true;
        let (manager, _server, _) = fixture(initial).await;
        assert!(matches!(manager.start_service("example.service", &properties()).await, Err(SessionError::Bus(_))));
        assert!(manager.transport.has_reference("example.service"));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inspection_denial_after_an_accepted_start_is_not_a_rejected_mutation() {
    let mut initial = state(false, true, 0);
    initial.denied_inspection = true;
    let (manager, _server, state) = fixture(initial).await;
    assert!(matches!(manager.start_service("example.service", &properties()).await, Err(SessionError::Bus(_))));
    assert!(manager.transport.has_reference("example.service"));
    assert!(state.exists.load(Ordering::SeqCst));
    assert_eq!(state.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lost_accepted_reply_and_a_local_timeout_preserve_reference_intent() {
    let (manager, _server, _) = fixture(state(true, true, 0)).await;
    manager.start_service("example.service", &properties()).await.unwrap();
    assert!(manager.transport.has_reference("example.service"));

    let mut initial = state(false, true, 0);
    initial.reply_delay = Duration::from_secs(2);
    let (manager, _server, state) = fixture(initial).await;
    let submitted = AtomicBool::new(false);
    assert!(start(&manager, "example.service", &properties(), Duration::from_millis(50), &submitted).await.is_err());
    assert!(submitted.load(Ordering::Relaxed));
    assert!(manager.transport.has_reference("example.service"));
    assert_eq!(state.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn preflight_denial_bad_generation_and_expired_deadline_never_stage_a_reference() {
    let mut initial = state(false, false, 0);
    initial.denied_preflight = true;
    let (manager, _server, state) = fixture(initial).await;
    assert!(matches!(manager.start_service("example.service", &properties()).await, Err(SessionError::StartRejected { reference_retained: false, .. })));
    assert!(!manager.transport.has_reference("example.service"));
    assert_eq!(state.calls.load(Ordering::SeqCst), 0);

    let (manager, _server, state) = fixture(super::fixture::state(false, false, 0)).await;
    let malformed = vec![("AddRef", Value::Bool(true)), ("Environment", Value::Bool(true))];
    assert!(matches!(manager.start_service("example.service", &malformed).await, Err(SessionError::StartRejected { reference_retained: false, .. })));
    manager.deadline.set(1);
    assert!(matches!(manager.start_service("example.service", &properties()).await, Err(SessionError::StartRejected { reference_retained: false, .. })));
    assert!(!manager.transport.has_reference("example.service"));
    assert_eq!(state.calls.load(Ordering::SeqCst), 0);
}
