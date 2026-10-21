//! Cancelled logout keeps accepted activation work able to finish or retry.

use super::*;

#[tokio::test]
async fn cancelled_logout_preserves_activation_completion_and_a_safe_retry() {
    let mut failures = Vec::new();
    for success in [false, true] {
        let mut fixture = Fixture::new().await;
        install(&mut fixture).await;
        fixture.coordinator.ready_busy = true;
        fixture.coordinator.ready_deadline = Instant::now() + Duration::from_secs(30);
        let deadline = fixture.coordinator.ready_deadline;
        let (reply, mut finalized) = tokio::sync::mpsc::channel(1);
        fixture.coordinator.finalize_replies.push(Reply::for_test(reply));
        fixture.coordinator.begin_stop("logout", None).await.unwrap();
        fixture.coordinator.shutdown.as_ref().unwrap().cancelled.store(true, Ordering::SeqCst);
        let result = if success { Ok(Some(true)) } else {
            Err(SessionError::StartRejected {
                error: Box::new(zbus::Error::from(zbus::fdo::Error::NoReply("preflight temporarily unavailable".into())).into()),
                reference_retained: false,
            })
        };

        fixture.coordinator.completed(Work::Ready(result)).await.unwrap();
        let actual = (fixture.coordinator.lifecycle.phase, fixture.coordinator.runtime.session().unwrap().phase,
            fixture.coordinator.ready_deadline, finalized.try_recv());
        let expected = (SessionPhase::Starting, SessionPhase::Starting, deadline,
            Err(tokio::sync::mpsc::error::TryRecvError::Empty));
        if actual != expected { failures.push((success, actual)); }
    }
    assert!(failures.is_empty(), "activation completion lost cancellation: {failures:?}");
}

#[tokio::test]
async fn a_skipped_activation_is_retried_after_logout_cancellation() {
    let mut fixture = Fixture::new().await;
    let (state, entered, resume) = install(&mut fixture).await;
    fixture.coordinator.ready_deadline = Instant::now() + Duration::from_secs(30);
    let (reply, mut finalized) = tokio::sync::mpsc::channel(1);
    fixture.coordinator.finalize_replies.push(Reply::for_test(reply));
    fixture.coordinator.activate();
    tokio::time::timeout(Duration::from_secs(2), entered.notified()).await.unwrap();
    resume.notify_one();
    let checked = tokio::time::timeout(Duration::from_secs(2), fixture.coordinator.work.recv()).await.unwrap().unwrap();
    let entered = Arc::new(tokio::sync::Notify::new());
    let resume = Arc::new(tokio::sync::Notify::new());
    *state.pause.lock().unwrap() = Some((entered.clone(), resume.clone()));
    fixture.coordinator.completed(checked).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), entered.notified()).await.unwrap();
    fixture.coordinator.begin_stop("logout", None).await.unwrap();
    fixture.coordinator.shutdown.as_ref().unwrap().cancelled.store(true, Ordering::SeqCst);
    resume.notify_one();
    let skipped = tokio::time::timeout(Duration::from_secs(2), fixture.coordinator.work.recv()).await.unwrap().unwrap();
    assert_eq!(state.starts.load(Ordering::SeqCst), 0);
    assert!(fixture.coordinator.resume_if_cancelled().await.unwrap());
    fixture.coordinator.completed(skipped).await.unwrap();
    assert!(!fixture.coordinator.ready_busy && !fixture.coordinator.pending_ready);
    assert_eq!(finalized.try_recv(), Err(tokio::sync::mpsc::error::TryRecvError::Empty));

    fixture.coordinator.activate();
    complete_check(&mut fixture).await;
    let attempted = tokio::time::timeout(Duration::from_secs(2), fixture.coordinator.work.recv()).await.unwrap().unwrap();
    fixture.coordinator.completed(attempted).await.unwrap();
    assert!(finalized.try_recv().unwrap().unwrap_err().contains("anchor activation rejected"));
    assert_eq!(state.starts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn an_uncancelled_logout_rejects_a_safe_activation_retry() {
    let mut fixture = Fixture::new().await;
    install(&mut fixture).await;
    fixture.coordinator.ready_busy = true;
    let (reply, mut finalized) = tokio::sync::mpsc::channel(1);
    fixture.coordinator.finalize_replies.push(Reply::for_test(reply));
    fixture.coordinator.begin_stop("logout", None).await.unwrap();
    let error = SessionError::StartRejected {
        error: Box::new(zbus::Error::from(zbus::fdo::Error::NoReply("preflight temporarily unavailable".into())).into()),
        reference_retained: false,
    };

    fixture.coordinator.completed(Work::Ready(Err(error))).await.unwrap();
    assert_eq!(fixture.coordinator.lifecycle.phase, SessionPhase::Preparing);
    assert!(finalized.try_recv().unwrap().is_err());
    assert!(!fixture.coordinator.ready_busy && !fixture.coordinator.pending_ready);
}
