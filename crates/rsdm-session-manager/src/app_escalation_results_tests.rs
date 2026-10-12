use super::*;

#[tokio::test]
async fn an_unsent_attempt_allows_clean_exit_without_erasing_an_older_uncertain_attempt() {
    let fixture = Fixture::new();
    let deadline = Deadline::default();
    begin(&fixture.runtime, &fixture.app, Some("first"), &deadline).await.unwrap();
    escalating(&fixture.runtime, &fixture.app, Some("first"), &deadline).await.unwrap();
    escalation_finished(&fixture.runtime, &fixture.app, Some("first"), false, &deadline).await.unwrap();
    assert_eq!(finish(&fixture.runtime, &fixture.app, Some("first"), Outcome::Clean, &deadline).await.unwrap(), Outcome::Clean);
    assert!(!fixture.runtime.app(&fixture.app.unit).unwrap().preparation.unwrap().escalated);

    escalating(&fixture.runtime, &fixture.app, Some("first"), &deadline).await.unwrap();
    begin(&fixture.runtime, &fixture.app, Some("retry"), &deadline).await.unwrap();
    escalating(&fixture.runtime, &fixture.app, Some("retry"), &deadline).await.unwrap();
    escalation_finished(&fixture.runtime, &fixture.app, Some("retry"), false, &deadline).await.unwrap();
    assert_eq!(finish(&fixture.runtime, &fixture.app, Some("retry"), Outcome::Clean, &deadline).await.unwrap(), Outcome::Unknown);
}

#[tokio::test]
async fn a_confirmed_signal_survives_retry_and_a_later_unsent_attempt() {
    let fixture = Fixture::new();
    let deadline = Deadline::default();
    begin(&fixture.runtime, &fixture.app, Some("first"), &deadline).await.unwrap();
    escalating(&fixture.runtime, &fixture.app, Some("first"), &deadline).await.unwrap();
    escalation_finished(&fixture.runtime, &fixture.app, Some("first"), true, &deadline).await.unwrap();
    begin(&fixture.runtime, &fixture.app, Some("retry"), &deadline).await.unwrap();
    escalating(&fixture.runtime, &fixture.app, Some("retry"), &deadline).await.unwrap();
    escalation_finished(&fixture.runtime, &fixture.app, Some("retry"), false, &deadline).await.unwrap();
    assert_eq!(finish(&fixture.runtime, &fixture.app, Some("retry"), Outcome::Clean, &deadline).await.unwrap(), Outcome::Forced);
    let app = fixture.runtime.app(&fixture.app.unit).unwrap();
    assert!(app.preparation.as_ref().unwrap().confirmed_escalation());
    assert_eq!(crate::status::preparation(&app).unwrap().result, "forced");
}

#[tokio::test]
async fn a_late_signal_confirmation_updates_the_archive_without_recreating_the_app() {
    let fixture = Fixture::new();
    let deadline = Deadline::default();
    begin(&fixture.runtime, &fixture.app, Some("tx"), &deadline).await.unwrap();
    escalating(&fixture.runtime, &fixture.app, Some("tx"), &deadline).await.unwrap();
    fixture.runtime.remove_app_until(&fixture.app, &deadline).unwrap();
    escalation_finished(&fixture.runtime, &fixture.app, Some("tx"), true, &deadline).await.unwrap();
    assert!(fixture.runtime.app(&fixture.app.unit).is_err());
    assert_eq!(finish(&fixture.runtime, &fixture.app, Some("tx"), Outcome::Clean, &deadline).await.unwrap(), Outcome::Forced);
    assert!(fixture.runtime.archived_app(&fixture.app.unit).unwrap().preparation.unwrap().confirmed_escalation());
}

#[tokio::test]
async fn a_stale_signal_confirmation_cannot_replace_a_new_preparation_or_invocation() {
    let fixture = Fixture::new();
    let deadline = Deadline::default();
    begin(&fixture.runtime, &fixture.app, Some("first"), &deadline).await.unwrap();
    escalating(&fixture.runtime, &fixture.app, Some("first"), &deadline).await.unwrap();
    begin(&fixture.runtime, &fixture.app, Some("next"), &deadline).await.unwrap();
    assert!(escalation_finished(&fixture.runtime, &fixture.app, Some("first"), true, &deadline).await.is_err());
    let mut replacement = fixture.runtime.app(&fixture.app.unit).unwrap();
    assert!(!replacement.preparation.as_ref().unwrap().escalated);
    replacement.invocation_id = vec![2; 16];
    fixture.runtime.save_app(&replacement).unwrap();
    assert!(escalation_finished(&fixture.runtime, &fixture.app, Some("next"), true, &deadline).await.is_err());
    assert_eq!(fixture.runtime.app(&fixture.app.unit).unwrap().invocation_id, vec![2; 16]);
}

#[test]
fn legacy_escalation_flags_are_uncertain_and_never_report_a_confirmed_signal() {
    let fixture = Fixture::new();
    let legacy: Preparation = toml::from_str("transaction_id = 'legacy'\noutcome = 'forced'\nescalated = true\n").unwrap();
    let mut app = fixture.app.clone();
    app.preparation = Some(legacy);
    let status = crate::status::preparation(&app).unwrap();
    assert_eq!(status.result, "unknown");
    assert!(!status.escalated);
    start(&mut app, Some("retry"));
    let preparation = app.preparation.unwrap();
    assert!(!preparation.confirmed_escalation());
    assert!(preparation.uncertain_escalation());
}

#[tokio::test]
async fn overlapping_attempts_preserve_confirmation_in_either_completion_order() {
    for first_sent in [true, false] {
        let fixture = Fixture::new();
        let deadline = Deadline::default();
        begin(&fixture.runtime, &fixture.app, Some("tx"), &deadline).await.unwrap();
        escalating(&fixture.runtime, &fixture.app, Some("tx"), &deadline).await.unwrap();
        escalating(&fixture.runtime, &fixture.app, Some("tx"), &deadline).await.unwrap();
        escalation_finished(&fixture.runtime, &fixture.app, Some("tx"), first_sent, &deadline).await.unwrap();
        escalation_finished(&fixture.runtime, &fixture.app, Some("tx"), !first_sent, &deadline).await.unwrap();
        assert_eq!(finish(&fixture.runtime, &fixture.app, Some("tx"), Outcome::Clean, &deadline).await.unwrap(), Outcome::Forced);
        let preparation = fixture.runtime.app(&fixture.app.unit).unwrap().preparation.unwrap();
        assert!(preparation.confirmed_escalation());
        assert_eq!(preparation.escalation_pending, Some(0));
    }
}

#[tokio::test]
async fn overlapping_unsent_attempts_are_clean_only_after_every_attempt_finishes() {
    let fixture = Fixture::new();
    let deadline = Deadline::default();
    begin(&fixture.runtime, &fixture.app, Some("tx"), &deadline).await.unwrap();
    escalating(&fixture.runtime, &fixture.app, Some("tx"), &deadline).await.unwrap();
    escalating(&fixture.runtime, &fixture.app, Some("tx"), &deadline).await.unwrap();
    escalation_finished(&fixture.runtime, &fixture.app, Some("tx"), false, &deadline).await.unwrap();
    assert_eq!(finish(&fixture.runtime, &fixture.app, Some("tx"), Outcome::Clean, &deadline).await.unwrap(), Outcome::Unknown);
    escalation_finished(&fixture.runtime, &fixture.app, Some("tx"), false, &deadline).await.unwrap();
    assert_eq!(finish(&fixture.runtime, &fixture.app, Some("tx"), Outcome::Clean, &deadline).await.unwrap(), Outcome::Clean);
}
