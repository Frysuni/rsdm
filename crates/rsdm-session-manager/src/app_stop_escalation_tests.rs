//! Escalation outcomes distinguish durable intent from an accepted signal.

use super::*;
use crate::app_result::{self, Outcome};

#[tokio::test]
async fn cancellation_during_escalation_discovery_never_records_a_sent_signal() {
    let control = Arc::new(ShutdownControl::default());
    let mut fixture = Fixture::new(control.clone(), Snapshot::Cancel).await;
    let app = fixture.save_app();
    app_result::begin(&fixture.runtime, &app, None, &fixture.manager.deadline).await.unwrap();
    let outcome = crate::app_stop::wait::force_exit(&fixture.manager, &fixture.runtime, &app, &control, &fixture.manager.deadline).await.unwrap();
    assert_eq!(outcome, Outcome::Cancelled);
    app_result::finish(&fixture.runtime, &app, None, outcome, &fixture.manager.deadline).await.unwrap();
    let preparation = fixture.runtime.app(&app.unit).unwrap().preparation.unwrap();
    assert!(!preparation.escalated);
    assert_eq!(preparation.outcome, Outcome::Cancelled);
    assert!(fixture.child.try_wait().unwrap().is_none());
}

#[tokio::test]
async fn force_records_a_signal_accepted_by_the_kernel() {
    let control = Arc::new(ShutdownControl::default());
    control.noncancelable.store(true, Ordering::SeqCst);
    let mut fixture = Fixture::new(control.clone(), Snapshot::Cancel).await;
    let app = fixture.save_app();
    app_result::begin(&fixture.runtime, &app, None, &fixture.manager.deadline).await.unwrap();
    let outcome = crate::app_stop::wait::force_exit(&fixture.manager, &fixture.runtime, &app, &control, &fixture.manager.deadline).await.unwrap();
    assert_eq!(outcome, Outcome::Forced);
    app_result::finish(&fixture.runtime, &app, None, outcome, &fixture.manager.deadline).await.unwrap();
    assert!(fixture.runtime.app(&app.unit).unwrap().preparation.unwrap().escalated);
    let status = fixture.child.wait().unwrap();
    assert_eq!(status.signal(), Some(libc::SIGTERM));
}

#[tokio::test]
async fn cancellation_after_the_saved_intent_restores_a_known_unsent_result() {
    let control = Arc::new(ShutdownControl::default());
    let mut fixture = Fixture::new(control.clone(), Snapshot::CancelDuringSignal).await;
    let app = fixture.save_app();
    app_result::begin(&fixture.runtime, &app, None, &fixture.manager.deadline).await.unwrap();
    let outcome = crate::app_stop::wait::force_exit(&fixture.manager, &fixture.runtime, &app, &control, &fixture.manager.deadline).await.unwrap();
    assert_eq!(outcome, Outcome::Cancelled);
    app_result::finish(&fixture.runtime, &app, None, outcome, &fixture.manager.deadline).await.unwrap();
    let preparation = fixture.runtime.app(&app.unit).unwrap().preparation.unwrap();
    assert!(!preparation.escalated);
    assert_eq!(preparation.escalation_pending, Some(0));
    assert!(fixture.child.try_wait().unwrap().is_none());
}

#[tokio::test]
async fn empty_process_membership_after_intent_does_not_report_forced_exit() {
    let control = Arc::new(ShutdownControl::default());
    let mut fixture = Fixture::new(control.clone(), Snapshot::EmptyDuringSignal).await;
    let app = fixture.save_app();
    app_result::begin(&fixture.runtime, &app, None, &fixture.manager.deadline).await.unwrap();
    let outcome = crate::app_stop::wait::force_exit(&fixture.manager, &fixture.runtime, &app, &control, &fixture.manager.deadline).await.unwrap();
    assert_eq!(outcome, Outcome::Clean);
    assert_eq!(app_result::finish(&fixture.runtime, &app, None, outcome, &fixture.manager.deadline).await.unwrap(), Outcome::Clean);
    let preparation = fixture.runtime.app(&app.unit).unwrap().preparation.unwrap();
    assert!(!preparation.escalated);
    assert_eq!(preparation.escalation_pending, Some(0));
    assert!(fixture.child.try_wait().unwrap().is_none());
}

#[tokio::test]
async fn failed_process_inspection_does_not_invent_a_possible_signal() {
    let control = Arc::new(ShutdownControl::default());
    let mut fixture = Fixture::new(control.clone(), Snapshot::FailDuringSignal).await;
    let app = fixture.save_app();
    app_result::begin(&fixture.runtime, &app, None, &fixture.manager.deadline).await.unwrap();
    assert!(crate::app_stop::wait::force_exit(&fixture.manager, &fixture.runtime, &app, &control, &fixture.manager.deadline).await.is_err());
    let preparation = fixture.runtime.app(&app.unit).unwrap().preparation.unwrap();
    assert_eq!(preparation.outcome, Outcome::Unknown);
    assert!(!preparation.escalated);
    assert_eq!(preparation.escalation_pending, Some(0));
    app_result::begin(&fixture.runtime, &app, Some("recovery"), &fixture.manager.deadline).await.unwrap();
    assert_eq!(app_result::finish(&fixture.runtime, &app, Some("recovery"), Outcome::Clean, &fixture.manager.deadline).await.unwrap(), Outcome::Clean);
    assert!(fixture.child.try_wait().unwrap().is_none());
}

#[tokio::test]
async fn a_confirmed_signal_survives_an_error_checking_the_next_batch() {
    let control = Arc::new(ShutdownControl::default());
    let mut fixture = Fixture::new(control.clone(), Snapshot::FailAfterFirstBatch).await;
    let app = fixture.save_app();
    app_result::begin(&fixture.runtime, &app, None, &fixture.manager.deadline).await.unwrap();
    let error = crate::app_stop::wait::force_exit(&fixture.manager, &fixture.runtime, &app, &control, &fixture.manager.deadline).await.unwrap_err();
    assert!(error.to_string().contains("second batch inspection failed"));
    let preparation = fixture.runtime.app(&app.unit).unwrap().preparation.unwrap();
    assert!(preparation.confirmed_escalation());
    assert_eq!(preparation.escalation_pending, Some(0));
    assert_eq!(app_result::finish(&fixture.runtime, &app, None, Outcome::Clean, &fixture.manager.deadline).await.unwrap(), Outcome::Forced);
    assert_eq!(fixture.child.wait().unwrap().signal(), Some(libc::SIGTERM));
}
