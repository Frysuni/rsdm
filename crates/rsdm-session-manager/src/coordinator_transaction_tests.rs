use super::*;

#[cfg(feature = "xsmp")]
#[tokio::test]
async fn cancelling_an_active_transaction_does_not_enqueue_a_late_xsmp_prepare() {
    let mut fixture = Fixture::new().await;
    let (sender, mut messages) = tokio::sync::mpsc::unbounded_channel();
    fixture.coordinator.xsmp = xsmp::Handle::with_sender_for_test(sender);
    fixture.coordinator.begin_stop("logout", None).await.unwrap();
    let transaction_id = fixture.coordinator.record.pending_transaction_id().unwrap().to_string();
    let (reply, mut cancelled) = tokio::sync::mpsc::channel(1);
    fixture.coordinator.request(crate::control::Request::Cancel {
        generation: GENERATION.into(), reply: crate::control::Reply::for_test(reply),
    }).await.unwrap();
    cancelled.recv().await.unwrap().unwrap();
    assert!(matches!(messages.try_recv().unwrap(), xsmp::Message::Cancel));

    let (reply, mut received) = tokio::sync::mpsc::channel(1);
    fixture.coordinator.request(crate::control::Request::XsmpPrepare {
        generation: GENERATION.into(), transaction_id, units: Vec::new(), cancellable: true,
        reply: crate::control::Reply::for_test(reply),
    }).await.unwrap();
    assert!(matches!(messages.try_recv(), Err(tokio::sync::mpsc::error::TryRecvError::Empty)));
    assert!(received.recv().await.unwrap().unwrap().is_empty());
}

#[cfg(feature = "xsmp")]
#[tokio::test]
async fn a_forced_xsmp_prepare_uses_the_same_control_despite_a_late_cancellation_flag() {
    let mut fixture = Fixture::new().await;
    let (sender, mut messages) = tokio::sync::mpsc::unbounded_channel();
    fixture.coordinator.xsmp = xsmp::Handle::with_sender_for_test(sender);
    fixture.coordinator.begin_stop("logout", None).await.unwrap();
    let control = fixture.coordinator.shutdown.as_ref().unwrap().clone();
    control.force(0);
    control.cancelled.store(true, Ordering::SeqCst);
    let transaction_id = fixture.coordinator.record.pending_transaction_id().unwrap().to_string();
    let (reply, _received) = tokio::sync::mpsc::channel(1);
    fixture.coordinator.request(crate::control::Request::XsmpPrepare {
        generation: GENERATION.into(), transaction_id, units: Vec::new(), cancellable: true,
        reply: crate::control::Reply::for_test(reply),
    }).await.unwrap();
    let xsmp::Message::Prepare { control: Some(shared), cancellable, .. } = messages.try_recv().unwrap() else { panic!("forced prepare was not queued"); };
    assert!(Arc::ptr_eq(&control, &shared));
    assert!(!cancellable);
    assert!(!shared.cancelled());
}

#[tokio::test]
async fn an_independent_stop_hook_after_cancellation_does_not_reuse_the_finished_transaction() {
    let mut fixture = Fixture::new().await;
    fixture.coordinator.begin_stop("logout", None).await.unwrap();
    fixture.coordinator.shutdown.as_ref().unwrap().cancelled.store(true, Ordering::SeqCst);
    fixture.coordinator.resume_if_cancelled().await.unwrap();
    assert_eq!(fixture.coordinator.record.shutdown_transaction.as_ref().unwrap().result.as_deref(), Some("cancelled"));
    let (reply, mut received) = tokio::sync::mpsc::channel(1);
    fixture.coordinator.request(crate::control::Request::XsmpPrepare {
        generation: GENERATION.into(), transaction_id: String::new(), units: Vec::new(), cancellable: false,
        reply: crate::control::Reply::for_test(reply),
    }).await.unwrap();
    assert!(received.recv().await.unwrap().unwrap().is_empty());
    fixture.coordinator.begin_stop("logout", None).await.unwrap();
    let (reply, mut received) = tokio::sync::mpsc::channel(1);
    fixture.coordinator.request(crate::control::Request::XsmpPrepare {
        generation: GENERATION.into(), transaction_id: String::new(), units: Vec::new(), cancellable: false,
        reply: crate::control::Reply::for_test(reply),
    }).await.unwrap();
    assert_eq!(received.recv().await.unwrap().unwrap_err(), "shutdown transaction is stale");
}

#[tokio::test]
async fn cancelled_and_replaced_transactions_reject_late_xsmp_requests() {
    let mut fixture = Fixture::new().await;
    fixture.coordinator.begin_stop("logout", None).await.unwrap();
    let old = fixture.coordinator.record.shutdown_transaction.as_ref().unwrap().id.clone();
    fixture.coordinator.shutdown.as_ref().unwrap().cancelled.store(true, Ordering::SeqCst);
    assert!(fixture.coordinator.resume_if_cancelled().await.unwrap());
    for replaced in [false, true] {
        if replaced { fixture.coordinator.begin_stop("logout", None).await.unwrap(); }
        let (reply, mut received) = tokio::sync::mpsc::channel(1);
        fixture.coordinator.request(crate::control::Request::XsmpPrepare {
            generation: GENERATION.into(), transaction_id: old.clone(), units: Vec::new(), cancellable: true,
            reply: crate::control::Reply::for_test(reply),
        }).await.unwrap();
        let result = tokio::time::timeout(Duration::from_secs(1), received.recv()).await.unwrap().unwrap();
        assert_eq!(result.unwrap_err(), "shutdown transaction is stale");
    }
}

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
            deadline_usec: None, quit_started: false, dispatch: None, window_close_sent: false, original_process: None, preparation: None,
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
