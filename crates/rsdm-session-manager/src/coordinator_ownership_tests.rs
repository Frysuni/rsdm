//! Replaced compositor units must not authorize teardown of shared targets.

use super::*;
use std::sync::{Mutex, atomic::AtomicBool};
use zbus::OwnedObjectPath;

#[path = "coordinator_recovery_ownership_tests.rs"]
mod recovery;

struct State {
    stops: Mutex<Vec<String>>,
    invocation: Mutex<Vec<u8>>,
    replace_on: Mutex<Option<String>>,
    mutations: Mutex<Vec<String>>,
    active: Mutex<String>,
    missing: AtomicBool,
    failed_query: AtomicBool,
    pending_query: AtomicBool,
    queries: AtomicUsize,
}

#[derive(Debug, zbus_macros::DBusError)]
#[zbus(prefix = "org.freedesktop.systemd1")]
enum UnitError { NoSuchUnit(String) }

struct Manager { state: Arc<State> }

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Manager")]
impl Manager {
    fn get_unit(&self, unit: &str) -> Result<OwnedObjectPath, UnitError> {
        if self.state.missing.load(Ordering::SeqCst) { return Err(UnitError::NoSuchUnit("compositor is absent".into())); }
        if self.state.replace_on.lock().unwrap().as_deref() == Some(unit) {
            *self.state.invocation.lock().unwrap() = vec![2; 16];
        }
        Ok(OwnedObjectPath::try_from("/compositor").unwrap())
    }

    fn stop_unit(&self, unit: &str, _mode: &str) -> zbus::fdo::Result<OwnedObjectPath> {
        self.state.stops.lock().unwrap().push(unit.into());
        Err(zbus::fdo::Error::Failed("unexpected stop of retained infrastructure".into()))
    }

    fn unref_unit(&self, unit: &str) { self.state.mutations.lock().unwrap().push(format!("unref {unit}")); }

    fn set_environment(&self, _values: Vec<String>) { self.state.mutations.lock().unwrap().push("set environment".into()); }

    fn unset_environment(&self, _names: Vec<String>) { self.state.mutations.lock().unwrap().push("unset environment".into()); }

    #[zbus(property)]
    fn environment(&self) -> Vec<String> { vec![format!("RSDM_SESSION_GENERATION={GENERATION}")] }
}

struct Unit { state: Arc<State> }

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Unit")]
impl Unit {
    #[zbus(property, name = "InvocationID")]
    async fn invocation_id(&self) -> zbus::fdo::Result<Vec<u8>> {
        self.state.queries.fetch_add(1, Ordering::SeqCst);
        if self.state.pending_query.load(Ordering::SeqCst) { std::future::pending::<()>().await; }
        if self.state.failed_query.load(Ordering::SeqCst) { return Err(zbus::fdo::Error::Failed("ownership query failed".into())); }
        Ok(self.state.invocation.lock().unwrap().clone())
    }

    #[zbus(property)]
    fn active_state(&self) -> String { self.state.active.lock().unwrap().clone() }
}

async fn fixture(captured: Vec<u8>, current: Vec<u8>) -> (Fixture, Arc<State>) {
    let mut fixture = Fixture::new().await;
    let state = Arc::new(State { stops: Mutex::new(Vec::new()), invocation: Mutex::new(current), replace_on: Mutex::new(None), mutations: Mutex::new(Vec::new()),
        active: Mutex::new("active".into()), missing: AtomicBool::new(false), failed_query: AtomicBool::new(false),
        pending_query: AtomicBool::new(false), queries: AtomicUsize::new(0) });
    fixture._server.object_server().at("/org/freedesktop/systemd1", Manager { state: state.clone() }).await.unwrap();
    fixture._server.object_server().at("/compositor", Unit { state: state.clone() }).await.unwrap();
    fixture.coordinator.provider.kind = ProviderKind::Managed;
    fixture.coordinator.record.provider = "managed".into();
    fixture.coordinator.record.compositor_unit = Some("compositor.service".into());
    fixture.coordinator.record.compositor_invocation = captured;
    fixture.coordinator.record.owns_targets = true;
    fixture.coordinator.record.exported_environment = vec![("RSDM_SESSION_GENERATION".into(), GENERATION.into())];
    fixture.coordinator.runtime.save_session(&fixture.coordinator.record).unwrap();
    (fixture, state)
}

#[tokio::test]
async fn logout_checks_compositor_ownership_before_stopping_shared_infrastructure() {
    for (captured, current) in [(vec![1; 16], vec![2; 16]), (vec![0; 16], vec![0; 16]), (Vec::new(), vec![1; 16])] {
        let (fixture, stops) = fixture(captured, current).await;
        let coordinator = &fixture.coordinator;
        let mut process = SessionProcess::launch(&["unused".into()], &[], &fixture.directory,
            &coordinator.provider, "compositor.service".into()).unwrap();
        process.invocation = coordinator.record.compositor_invocation.clone();
        process.booting = false;
        let result = tokio::time::timeout(Duration::from_secs(2), finish(&coordinator.manager,
            &coordinator.runtime, &coordinator.record, &coordinator.provider, &mut process, fixture.directory.clone())).await.unwrap();
        assert!(result.is_err());
        let stops = stops.stops.lock().unwrap();
        assert!(stops.is_empty(), "unexpected stops: {stops:?}");
    }
}

#[tokio::test]
async fn recovery_checks_compositor_ownership_before_stopping_shared_infrastructure() {
    let (fixture, stops) = fixture(vec![1; 16], vec![2; 16]).await;
    let result = tokio::time::timeout(Duration::from_secs(2), crate::cleanup::recover(
        &fixture.coordinator.manager, &fixture.coordinator.runtime)).await.unwrap();
    assert!(result.is_err());
    let stops = stops.stops.lock().unwrap();
    assert!(stops.is_empty(), "unexpected stops: {stops:?}");
}

#[tokio::test]
async fn logout_rechecks_compositor_ownership_before_queued_stop() {
    let (fixture, state) = fixture(vec![1; 16], vec![1; 16]).await;
    *state.replace_on.lock().unwrap() = Some(units::AUTOSTART_TARGET.into());
    let coordinator = &fixture.coordinator;
    let mut process = SessionProcess::launch(&["unused".into()], &[], &fixture.directory,
        &coordinator.provider, "compositor.service".into()).unwrap();
    process.invocation = coordinator.record.compositor_invocation.clone();
    process.booting = false;
    let result = tokio::time::timeout(Duration::from_secs(2), finish(&coordinator.manager,
        &coordinator.runtime, &coordinator.record, &coordinator.provider, &mut process, fixture.directory.clone())).await.unwrap();
    assert!(result.is_err());
    let stops = state.stops.lock().unwrap();
    assert!(stops.is_empty(), "unexpected stops: {stops:?}");
    assert!(state.mutations.lock().unwrap().is_empty());
}

#[tokio::test]
async fn ownership_is_rechecked_after_waiting_for_a_request_slot() {
    let (fixture, state) = fixture(vec![1; 16], vec![1; 16]).await;
    let manager = &fixture.coordinator.manager;
    let release = Arc::new(tokio::sync::Notify::new());
    let (ready, mut entered) = tokio::sync::mpsc::unbounded_channel();
    let mut workers = tokio::task::JoinSet::new();
    for _ in 0..4 {
        let manager = manager.clone();
        let release = release.clone();
        let ready = ready.clone();
        workers.spawn(async move { manager.request(async {
            let wait = release.notified();
            tokio::pin!(wait);
            wait.as_mut().enable();
            ready.send(()).unwrap();
            wait.await;
            Ok(())
        }).await });
    }
    for _ in 0..4 { tokio::time::timeout(Duration::from_secs(2), entered.recv()).await.unwrap().unwrap(); }
    let owned = manager.with_owner("compositor.service", &[1; 16]);
    let mut mutation = Box::pin(owned.set_environment(&[]));
    assert!(tokio::time::timeout(Duration::from_millis(50), &mut mutation).await.is_err());
    *state.invocation.lock().unwrap() = vec![2; 16];
    release.notify_waiters();
    assert!(tokio::time::timeout(Duration::from_secs(2), mutation).await.unwrap().is_err());
    while let Some(result) = workers.join_next().await { result.unwrap().unwrap(); }
    assert!(state.mutations.lock().unwrap().is_empty());
}

#[tokio::test]
async fn recovery_rechecks_compositor_ownership_before_queued_stop() {
    let (fixture, state) = fixture(vec![1; 16], vec![1; 16]).await;
    *state.replace_on.lock().unwrap() = Some(units::AUTOSTART_TARGET.into());
    let result = tokio::time::timeout(Duration::from_secs(2), crate::cleanup::recover(
        &fixture.coordinator.manager, &fixture.coordinator.runtime)).await.unwrap();
    assert!(result.is_err());
    let stops = state.stops.lock().unwrap();
    assert!(stops.is_empty(), "unexpected stops: {stops:?}");
    assert!(state.mutations.lock().unwrap().is_empty());
}

#[tokio::test]
async fn activation_environment_update_rejects_a_replaced_invocation() {
    let (fixture, state) = fixture(vec![1; 16], vec![2; 16]).await;
    let manager = fixture.coordinator.manager.with_owner("compositor.service", &[1; 16]);
    let result = tokio::time::timeout(Duration::from_secs(2),
        manager.update_activation(&[("DISPLAY".into(), String::new())])).await.unwrap();
    assert!(result.is_err());
    assert!(state.mutations.lock().unwrap().is_empty());
}

#[tokio::test]
async fn lost_compositor_ownership_returns_prepared_and_keeps_the_coordinator_running() {
    for (captured, current) in [(vec![1; 16], vec![2; 16]), (vec![0; 16], vec![0; 16]), (Vec::new(), vec![1; 16])] {
        let (fixture, state) = fixture(captured, current).await;
        assert_prepared(fixture, state).await;
    }
}

#[tokio::test]
async fn unavailable_or_unknown_lifecycle_retains_infrastructure() {
    for case in ["missing", "inactive", "failed", "unrecorded"] {
        let (mut fixture, state) = fixture(vec![1; 16], vec![1; 16]).await;
        match case {
            "missing" => state.missing.store(true, Ordering::SeqCst),
            "inactive" => *state.active.lock().unwrap() = "inactive".into(),
            "failed" => state.failed_query.store(true, Ordering::SeqCst),
            "unrecorded" => fixture.coordinator.record.compositor_unit = None,
            _ => unreachable!(),
        }
        assert_prepared(fixture, state).await;
    }
}

async fn assert_prepared(mut fixture: Fixture, state: Arc<State>) {
    let coordinator = &mut fixture.coordinator;
    let mut process = SessionProcess::launch(&["unused".into()], &[], &fixture.directory,
        &coordinator.provider, "compositor.service".into()).unwrap();
    process.invocation = coordinator.record.compositor_invocation.clone();
    process.booting = false;
    coordinator.process = Some(process);
    let (reply, mut result) = tokio::sync::mpsc::channel(1);
    coordinator.stop_replies.push(crate::control::Reply::for_test(reply));
    coordinator.begin_stop("logout", None).await.unwrap();
    coordinator.prepared(Ok(Vec::new()), None).await.unwrap();
    let work = tokio::time::timeout(Duration::from_secs(2), coordinator.work.recv()).await.unwrap().unwrap();
    coordinator.completed(work).await.unwrap();
    let outcome = result.try_recv().unwrap().unwrap();
    assert_eq!(outcome.result, "prepared");
    assert!(outcome.message.contains("session infrastructure retained"));
    assert_eq!(coordinator.lifecycle.phase, SessionPhase::Running);
    assert!(coordinator.shutdown.is_none());
    assert!(coordinator.process.is_some());
    assert_eq!(coordinator.record.provider, "managed");
    assert!(state.stops.lock().unwrap().is_empty());
    assert!(state.mutations.lock().unwrap().is_empty());
}

#[tokio::test]
async fn cancellation_remains_available_while_lifecycle_ownership_is_queried() {
    let (mut fixture, state) = fixture(vec![1; 16], vec![1; 16]).await;
    state.pending_query.store(true, Ordering::SeqCst);
    let coordinator = &mut fixture.coordinator;
    let mut process = SessionProcess::launch(&["unused".into()], &[], &fixture.directory,
        &coordinator.provider, "compositor.service".into()).unwrap();
    process.invocation = coordinator.record.compositor_invocation.clone();
    process.booting = false;
    coordinator.process = Some(process);
    coordinator.begin_stop("logout", None).await.unwrap();
    coordinator.prepared(Ok(Vec::new()), None).await.unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        while state.queries.load(Ordering::SeqCst) == 0 { tokio::task::yield_now().await; }
    }).await.unwrap();
    let (reply, mut result) = tokio::sync::mpsc::channel(1);
    coordinator.request(crate::control::Request::Cancel {
        generation: GENERATION.into(), reply: crate::control::Reply::for_test(reply),
    }).await.unwrap();
    result.try_recv().unwrap().unwrap();
    let work = tokio::time::timeout(Duration::from_secs(3), coordinator.work.recv()).await.unwrap().unwrap();
    coordinator.completed(work).await.unwrap();
    assert_eq!(coordinator.lifecycle.phase, SessionPhase::Running);
    assert_eq!(coordinator.record.shutdown_transaction.as_ref().unwrap().result.as_deref(), Some("cancelled"));
    assert!(coordinator.process.is_some());
    assert!(state.stops.lock().unwrap().is_empty());
    assert!(state.mutations.lock().unwrap().is_empty());
}
