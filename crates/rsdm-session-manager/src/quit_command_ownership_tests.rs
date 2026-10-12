//! Reused units cannot supply properties or receive a quit helper dispatch.

use super::*;

fn assert_unsent(fixture: &Fixture) {
    assert_eq!(fixture.saved_dispatch().unwrap().state, DispatchState::NotSent);
    assert_eq!(fixture.state.starts.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.state.stops.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.state.unrefs.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn empty_or_zero_invocation_records_never_authorize_a_quit_helper() {
    for invocation in [Vec::new(), vec![0; 16]] {
        let control = Arc::new(ShutdownControl::default());
        let fixture = Fixture::new(control.clone(), Stage::Never).await;
        let mut app = crate::helper_start_fixture::app();
        app.invocation_id = invocation;
        fixture.runtime.save_app(&app).unwrap();
        fixture.state.invocation.store(0, Ordering::SeqCst);
        assert!(request_quit(&fixture.manager, &fixture.runtime, &app, &control).await.is_err());
        if app.invocation_id.is_empty() {
            assert!(fixture.saved_dispatch().is_none());
            assert_eq!(fixture.state.starts.load(Ordering::SeqCst), 0);
            assert_eq!(fixture.state.stops.load(Ordering::SeqCst), 0);
            assert_eq!(fixture.state.unrefs.load(Ordering::SeqCst), 0);
        } else { assert_unsent(&fixture); }
    }
}

#[tokio::test]
async fn changed_invocations_before_properties_during_reads_and_after_helper_preflight_never_start() {
    for forced in [false, true] {
        for replacement in [None, Some(Stage::Environment), Some(Stage::Directory), Some(Stage::Preflight), Some(Stage::FinalIdentity)] {
            let control = Arc::new(ShutdownControl::default());
            if forced { control.force(0); }
            let fixture = Fixture::new(control.clone(), Stage::Never).await;
            if replacement.is_none() { fixture.state.invocation.store(2, Ordering::SeqCst); }
            *fixture.state.replacement.lock().unwrap() = replacement;
            let app = crate::helper_start_fixture::app();
            assert!(matches!(request_quit(&fixture.manager, &fixture.runtime, &app, &control).await,
                Err(SessionError::StartRejected { reference_retained: false, .. })));
            assert_unsent(&fixture);
        }
    }
}

#[tokio::test]
async fn cancellation_during_final_identity_confirmation_does_not_send_start() {
    let control = Arc::new(ShutdownControl::default());
    let fixture = Fixture::new(control.clone(), Stage::FinalIdentity).await;
    let app = crate::helper_start_fixture::app();
    assert!(request_quit(&fixture.manager, &fixture.runtime, &app, &control).await.unwrap().is_none());
    assert!(control.cancelled());
    assert_unsent(&fixture);
}

#[tokio::test]
async fn a_deadline_shortened_during_final_identity_confirmation_does_not_send_start() {
    let control = Arc::new(ShutdownControl::default());
    let mut fixture = Fixture::new(control.clone(), Stage::Never).await;
    fixture.manager.deadline = control.hard_deadline.clone();
    fixture.state.expire_on_final_identity.store(true, Ordering::SeqCst);
    let app = crate::helper_start_fixture::app();
    assert!(request_quit(&fixture.manager, &fixture.runtime, &app, &control).await.is_err());
    assert_eq!(fixture.state.starts.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.state.stops.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.state.unrefs.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn an_invocation_change_while_waiting_for_the_dispatch_lease_leaves_the_command_unsent() {
    let control = Arc::new(ShutdownControl::default());
    let fixture = Fixture::new(control.clone(), Stage::Never).await;
    let app = crate::helper_start_fixture::app();
    let lease = fixture.runtime.app_lease(&app.unit).unwrap();
    let mut request = Box::pin(request_quit(&fixture.manager, &fixture.runtime, &app, &control));
    tokio::select! {
        biased;
        _ = tokio::time::sleep(Duration::from_millis(20)) => {},
        result = &mut request => panic!("contended command claim completed: {result:?}"),
    }
    fixture.state.invocation.store(2, Ordering::SeqCst);
    drop(lease);
    assert!(tokio::time::timeout(Duration::from_secs(2), request).await.unwrap().is_err());
    assert_unsent(&fixture);
}

#[tokio::test]
async fn an_invocation_change_while_start_waits_for_request_slots_never_sends_start() {
    let control = Arc::new(ShutdownControl::default());
    let fixture = Fixture::new(control.clone(), Stage::Never).await;
    fixture.state.block_preflight.store(true, Ordering::SeqCst);
    let app = crate::helper_start_fixture::app();
    let mut request = Box::pin(request_quit(&fixture.manager, &fixture.runtime, &app, &control));
    let (mut workers, replies) = occupy_slots_before_start(&fixture, &mut request).await;
    fixture.state.invocation.store(2, Ordering::SeqCst);
    replies.add_permits(4);
    while let Some(result) = workers.join_next().await { result.unwrap(); }

    assert!(tokio::time::timeout(Duration::from_secs(2), request).await.unwrap().is_err());
    assert_unsent(&fixture);
}
