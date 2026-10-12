//! Die claims survive cancellation and recovery without replaying a close.

use super::*;

fn transaction(fixture: &Fixture, id: &str) {
    let mut record = crate::helper_start_fixture::record();
    record.identity.generation = fixture.runtime.generation.clone();
    // SAFETY: geteuid has no preconditions.
    record.identity.uid = unsafe { libc::geteuid() };
    record.shutdown_transaction = Some(crate::shutdown_transaction::ShutdownTransaction {
        id: id.into(), action: "logout".into(), started_usec: 1, deadline_usec: i64::MAX as u64, result: None,
    });
    fixture.runtime.save_session(&record).unwrap();
}

async fn saved() -> Fixture {
    let fixture = Fixture::new();
    transaction(&fixture, "first");
    assert!(claim(&fixture.runtime, &fixture.app, Some("first"), Method::Xsmp, &Deadline::default()).await.unwrap());
    finish(&fixture.runtime, &fixture.app, Some("first"), Method::Xsmp, State::Sent, &Deadline::default()).await.unwrap();
    fixture
}

#[tokio::test]
async fn competing_die_claims_persist_one_intent_and_block_every_later_graceful_method() {
    let fixture = saved().await;
    let deadline = Deadline::default();
    let (first, second) = tokio::join!(claim_xsmp_die(&fixture.runtime, &fixture.app, Some("first"), &deadline),
        claim_xsmp_die(&fixture.runtime, &fixture.app, Some("first"), &deadline));
    assert_ne!(first.unwrap().is_some(), second.unwrap().is_some());
    assert_eq!(fixture.runtime.app(&fixture.app.unit).unwrap().dispatch.unwrap().xsmp_die_state, Some(State::Intent));

    crate::apps::reset_preparation(&fixture.runtime).await.unwrap();
    transaction(&fixture, "next");
    for method in [Method::Xsmp, Method::WindowClose, Method::Command, Method::Term] {
        assert!(!claim(&fixture.runtime, &fixture.app, Some("next"), method, &deadline).await.unwrap());
    }
    assert!(claim_xsmp_die(&fixture.runtime, &fixture.app, Some("next"), &deadline).await.unwrap().is_none());
}

#[tokio::test]
async fn a_sent_die_remains_claimed_after_cancellation_and_record_reload() {
    let fixture = saved().await;
    assert!(claim_xsmp_die(&fixture.runtime, &fixture.app, Some("first"), &Deadline::default()).await.unwrap().is_some());
    finish_xsmp_die(&fixture.runtime, &fixture.app, Some("first"), State::Sent, &Deadline::default()).await.unwrap();
    crate::apps::reset_preparation(&fixture.runtime).await.unwrap();

    let reloaded: AppRecord = toml::from_str(&toml::to_string(&fixture.runtime.app(&fixture.app.unit).unwrap()).unwrap()).unwrap();
    let dispatch = reloaded.dispatch.unwrap();
    assert_eq!(dispatch.state, State::Sent);
    assert_eq!(dispatch.xsmp_die_state, Some(State::Sent));
    transaction(&fixture, "next");
    assert!(!claim(&fixture.runtime, &fixture.app, Some("next"), Method::Xsmp, &Deadline::default()).await.unwrap());
}

#[tokio::test]
async fn a_proven_unsent_die_allows_a_new_save_after_cancellation() {
    let fixture = saved().await;
    assert!(claim_xsmp_die(&fixture.runtime, &fixture.app, Some("first"), &Deadline::default()).await.unwrap().is_some());
    finish_xsmp_die(&fixture.runtime, &fixture.app, Some("first"), State::NotSent, &Deadline::default()).await.unwrap();
    crate::apps::reset_preparation(&fixture.runtime).await.unwrap();
    assert_eq!(fixture.runtime.app(&fixture.app.unit).unwrap().dispatch.unwrap().state, State::Cancelled);
    transaction(&fixture, "next");
    assert!(claim(&fixture.runtime, &fixture.app, Some("next"), Method::Xsmp, &Deadline::default()).await.unwrap());
}

#[tokio::test]
async fn legacy_save_records_cannot_prove_that_die_was_never_sent() {
    for state in [State::Sent, State::Cancelled] {
        let fixture = saved().await;
        let mut app = fixture.runtime.app(&fixture.app.unit).unwrap();
        app.dispatch.as_mut().unwrap().state = state;
        let encoded = toml::to_string(&app).unwrap().replace("xsmp_die_state = \"not_sent\"\n", "");
        let legacy: AppRecord = toml::from_str(&encoded).unwrap();
        assert_eq!(legacy.dispatch.as_ref().unwrap().xsmp_die_state, None);
        fixture.runtime.save_app(&legacy).unwrap();
        crate::apps::reset_preparation(&fixture.runtime).await.unwrap();
        assert_eq!(fixture.runtime.app(&fixture.app.unit).unwrap().dispatch.unwrap().state, state);
        assert!(claim_xsmp_die(&fixture.runtime, &fixture.app, Some("first"), &Deadline::default()).await.unwrap().is_none());
        transaction(&fixture, "next");
        assert!(!claim(&fixture.runtime, &fixture.app, Some("next"), Method::Xsmp, &Deadline::default()).await.unwrap());
    }
}

#[tokio::test]
async fn stale_transactions_and_replaced_invocations_do_not_acquire_or_overwrite_a_die_claim() {
    let fixture = saved().await;
    assert!(claim_xsmp_die(&fixture.runtime, &fixture.app, Some("stale"), &Deadline::default()).await.unwrap().is_none());
    let mut replaced = fixture.runtime.app(&fixture.app.unit).unwrap();
    replaced.invocation_id = vec![2; 16];
    fixture.runtime.save_app(&replaced).unwrap();
    assert!(claim_xsmp_die(&fixture.runtime, &fixture.app, Some("first"), &Deadline::default()).await.unwrap().is_none());
    assert!(finish_xsmp_die(&fixture.runtime, &fixture.app, Some("first"), State::Sent, &Deadline::default()).await.is_err());
    assert_eq!(fixture.runtime.app(&fixture.app.unit).unwrap().invocation_id, replaced.invocation_id);
    assert_eq!(fixture.runtime.app(&fixture.app.unit).unwrap().dispatch.unwrap().xsmp_die_state, Some(State::NotSent));
}

#[tokio::test]
async fn die_cannot_follow_an_unconfirmed_or_cancelled_save() {
    for state in [State::Intent, State::NotSent, State::Cancelled] {
        let fixture = saved().await;
        let mut app = fixture.runtime.app(&fixture.app.unit).unwrap();
        app.dispatch.as_mut().unwrap().state = state;
        fixture.runtime.save_app(&app).unwrap();
        assert!(claim_xsmp_die(&fixture.runtime, &fixture.app, Some("first"), &Deadline::default()).await.unwrap().is_none());
        assert_eq!(fixture.runtime.app(&fixture.app.unit).unwrap().dispatch.unwrap().xsmp_die_state, Some(State::NotSent));
    }
}

#[tokio::test]
async fn a_die_claim_waiting_for_a_lease_rechecks_the_transaction() {
    let fixture = saved().await;
    let deadline = Deadline::default();
    let lease = fixture.runtime.app_lease(&fixture.app.unit).unwrap();
    let mut pending = Box::pin(claim_xsmp_die(&fixture.runtime, &fixture.app, Some("first"), &deadline));
    tokio::select! {
        _ = tokio::time::sleep(std::time::Duration::from_millis(20)) => {},
        result = &mut pending => panic!("claim completed with its lease held: {result:?}"),
    }
    transaction(&fixture, "next");
    drop(lease);
    assert!(pending.await.unwrap().is_none());
    assert_eq!(fixture.runtime.app(&fixture.app.unit).unwrap().dispatch.unwrap().xsmp_die_state, Some(State::NotSent));
}
