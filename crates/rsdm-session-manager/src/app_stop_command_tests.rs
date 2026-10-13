//! Quit command cancellation is distinct from a possibly delivered start.

use std::sync::Arc;

use super::*;
use crate::app_dispatch::State as DispatchState;

#[path = "quit_command_fixture.rs"]
mod fixture;
use fixture::{Fixture, Stage};

#[path = "quit_command_ownership_tests.rs"]
mod ownership;

#[tokio::test]
async fn partial_quit_commands_depend_on_the_registered_app_instead_of_the_session_anchor() {
    for provider in ["managed", "gnome", "plasma", "partial"] {
        let control = Arc::new(ShutdownControl::default());
        let fixture = Fixture::new(control.clone(), Stage::Never).await;
        let mut record = fixture.runtime.session().unwrap();
        record.provider = provider.into();
        if provider == "partial" { record.coordinator_unit = Some((crate::attach::UNIT.into(), vec![1; 16])); }
        fixture.runtime.save_session(&record).unwrap();
        let app = crate::helper_start_fixture::app();
        assert!(request_quit(&fixture.manager, &fixture.runtime, &app, &control).await.unwrap().is_some());
        assert_eq!(*fixture.state.lifetime.lock().unwrap(), vec![
            ("PartOf".into(), vec![app.unit.clone()]), ("Requisite".into(), vec![app.unit.clone()]),
        ]);
        assert_eq!(fixture.state.starts.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.saved_dispatch().unwrap().state, DispatchState::Sent);
    }
}

#[tokio::test]
async fn cancellation_during_command_preparation_or_start_preflight_never_sends_start() {
    for point in [Stage::Environment, Stage::Directory, Stage::DirectoryFailure, Stage::Preflight] {
        let control = Arc::new(ShutdownControl::default());
        let fixture = Fixture::new(control.clone(), point).await;
        let app = crate::helper_start_fixture::app();
        assert!(request_quit(&fixture.manager, &fixture.runtime, &app, &control).await.unwrap().is_none());
        assert!(control.cancelled());
        assert_eq!(fixture.saved_dispatch().unwrap().state, DispatchState::NotSent);
        assert_eq!(fixture.state.starts.load(Ordering::SeqCst), 0);
        assert_eq!(fixture.state.stops.load(Ordering::SeqCst), 0);
        assert_eq!(fixture.state.unrefs.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn an_already_cancelled_request_does_not_claim_a_command() {
    let control = Arc::new(ShutdownControl::default());
    control.cancelled.store(true, Ordering::SeqCst);
    let fixture = Fixture::new(control.clone(), Stage::Never).await;
    let app = crate::helper_start_fixture::app();
    assert!(request_quit(&fixture.manager, &fixture.runtime, &app, &control).await.unwrap().is_none());
    assert!(fixture.saved_dispatch().is_none());
    assert_eq!(fixture.state.starts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_forced_command_can_start_despite_cancellation_during_preflight() {
    let control = Arc::new(ShutdownControl::default());
    control.force(0);
    let fixture = Fixture::new(control.clone(), Stage::Preflight).await;
    let app = crate::helper_start_fixture::app();
    assert!(request_quit(&fixture.manager, &fixture.runtime, &app, &control).await.unwrap().is_some());
    assert!(!control.cancelled());
    assert_eq!(fixture.saved_dispatch().unwrap().state, DispatchState::Sent);
    assert_eq!(fixture.state.starts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn cancellation_and_lost_reply_after_acceptance_preserve_intent_and_never_repeat_start() {
    let control = Arc::new(ShutdownControl::default());
    let fixture = Fixture::new(control.clone(), Stage::Accepted).await;
    let app = crate::helper_start_fixture::app();
    assert!(request_quit(&fixture.manager, &fixture.runtime, &app, &control).await.unwrap().is_none());
    assert_eq!(fixture.saved_dispatch().unwrap().state, DispatchState::Intent);
    control.cancelled.store(false, Ordering::SeqCst);
    assert!(request_quit(&fixture.manager, &fixture.runtime, &app, &control).await.unwrap().is_none());
    assert_eq!(fixture.state.starts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn cancellation_while_start_waits_for_request_slots_never_sends_start() {
    let control = Arc::new(ShutdownControl::default());
    let fixture = Fixture::new(control.clone(), Stage::Never).await;
    fixture.state.block_preflight.store(true, Ordering::SeqCst);
    let app = crate::helper_start_fixture::app();
    let mut request = Box::pin(request_quit(&fixture.manager, &fixture.runtime, &app, &control));
    let (mut workers, replies) = occupy_slots_before_start(&fixture, &mut request).await;
    control.cancelled.store(true, Ordering::SeqCst);
    replies.add_permits(4);
    while let Some(result) = workers.join_next().await { result.unwrap(); }

    assert!(tokio::time::timeout(Duration::from_secs(2), request).await.unwrap().unwrap().is_none());
    assert_eq!(fixture.saved_dispatch().unwrap().state, DispatchState::NotSent);
    assert_eq!(fixture.state.starts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn cancellation_while_waiting_for_a_command_claim_leaves_it_unsent() {
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
    control.cancelled.store(true, Ordering::SeqCst);
    drop(lease);
    assert!(tokio::time::timeout(Duration::from_secs(2), request).await.unwrap().unwrap().is_none());
    assert_eq!(fixture.saved_dispatch().unwrap().state, DispatchState::NotSent);
    assert_eq!(fixture.state.starts.load(Ordering::SeqCst), 0);
}

async fn occupy_slots_before_start(
    fixture: &Fixture, request: &mut (impl std::future::Future<Output = Result<Option<String>, SessionError>> + Unpin),
) -> (tokio::task::JoinSet<()>, Arc<tokio::sync::Semaphore>) {
    tokio::select! {
        result = tokio::time::timeout(Duration::from_secs(2), fixture.state.preflight_entered.notified()) => result.unwrap(),
        result = &mut *request => panic!("start completed before its preflight reply: {result:?}"),
    }

    let replies = Arc::new(tokio::sync::Semaphore::new(0));
    let (queued, mut queue) = tokio::sync::mpsc::unbounded_channel();
    let (entered, mut slots) = tokio::sync::mpsc::unbounded_channel();
    let mut workers = tokio::task::JoinSet::new();
    for _ in 0..4 {
        let manager = fixture.manager.clone();
        let queued = queued.clone();
        let entered = entered.clone();
        let replies = replies.clone();
        workers.spawn(async move {
            queued.send(()).unwrap();
            manager.request(async {
                entered.send(()).unwrap();
                replies.acquire().await.unwrap().forget();
                Ok(())
            }).await.unwrap();
        });
    }
    for _ in 0..4 { tokio::time::timeout(Duration::from_secs(2), queue.recv()).await.unwrap().unwrap(); }
    for _ in 0..3 { tokio::time::timeout(Duration::from_secs(2), slots.recv()).await.unwrap().unwrap(); }
    fixture.state.preflight_reply.add_permits(1);
    tokio::select! {
        result = tokio::time::timeout(Duration::from_secs(2), slots.recv()) => { result.unwrap().unwrap(); },
        result = &mut *request => panic!("queued start completed with all request slots held: {result:?}"),
    }
    (workers, replies)
}
