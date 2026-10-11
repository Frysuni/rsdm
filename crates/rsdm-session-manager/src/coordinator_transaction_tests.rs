use super::*;

#[tokio::test]
async fn ordinary_logout_persists_one_transaction_and_a_bounded_default_budget() {
    let mut fixture = Fixture::new().await;
    let before = monotonic_usec().unwrap();
    fixture.coordinator.begin_stop("logout", None).await.unwrap();
    let after = monotonic_usec().unwrap();
    let first = fixture.coordinator.runtime.session().unwrap();
    let transaction = first.shutdown_transaction.unwrap();
    assert!((before..=after).contains(&transaction.started_usec));
    assert_eq!(transaction.deadline_usec, transaction.started_usec + 120_000_000);
    assert_eq!(first.shutdown_deadline_usec, Some(transaction.deadline_usec));
    assert_eq!(fixture.deadline(), transaction.deadline_usec);
    fixture.coordinator.begin_stop("logout", None).await.unwrap();
    let repeated = fixture.coordinator.runtime.session().unwrap().shutdown_transaction.unwrap();
    assert_eq!(repeated.id, transaction.id);
    assert_eq!(repeated.started_usec, transaction.started_usec);
    assert_eq!(repeated.deadline_usec, transaction.deadline_usec);
}

#[tokio::test]
async fn the_budget_uses_the_largest_registered_app_timeout_and_preserves_it_for_recovery() {
    let mut fixture = Fixture::new().await;
    for (index, timeout_secs) in [5, 140, 30].into_iter().enumerate() {
        let app = crate::runtime::AppRecord {
            unit: format!("app-rsdm-budget@{GENERATION}-{index}.service"), invocation_id: vec![1; 16],
            policy: rsdm_core::domain::ShutdownPolicy { timeout_secs, ..Default::default() },
            deadline_usec: None, quit_started: false,
        };
        fixture.coordinator.runtime.save_app(&app).unwrap();
    }
    fixture.coordinator.begin_stop("logout", None).await.unwrap();
    let transaction = fixture.coordinator.runtime.session().unwrap().shutdown_transaction.unwrap();
    assert_eq!(transaction.deadline_usec - transaction.started_usec, 230_000_000);
    assert_eq!(crate::cleanup::recovery_deadline(Some(transaction.deadline_usec), 0).unwrap(), transaction.deadline_usec);
    fixture.notify(true, 1_000_000).await;
    let shortened = fixture.coordinator.runtime.session().unwrap().shutdown_transaction.unwrap();
    assert_eq!(shortened.id, transaction.id);
    assert_eq!(shortened.started_usec, transaction.started_usec);
    assert!(shortened.deadline_usec < transaction.deadline_usec);
    assert_eq!(shortened.deadline_usec, fixture.deadline());
}
