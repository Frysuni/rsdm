//! Target ownership reads cannot hold the actor or start a stopping session.

use std::sync::{Mutex, atomic::AtomicBool};

use zbus::{OwnedObjectPath, OwnedVariant};

use super::*;
use crate::control::{Reply, Request};

struct State {
    runtime: Runtime,
    starts: AtomicUsize,
    unavailable: AtomicBool,
    pause: Mutex<Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>>,
}

struct Manager(Arc<State>);

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Manager")]
impl Manager {
    async fn get_unit(&self, unit: &str) -> zbus::fdo::Result<OwnedObjectPath> {
        if self.0.unavailable.load(Ordering::SeqCst) {
            return Err(zbus::fdo::Error::NoReply("activation preflight temporarily unavailable".into()));
        }
        if unit == units::SESSION_TARGET {
            let pause = self.0.pause.lock().unwrap().take();
            if let Some((entered, resume)) = pause {
                entered.notify_one();
                resume.notified().await;
            }
        }
        Ok(OwnedObjectPath::try_from("/unit").unwrap())
    }

    fn start_transient_unit(
        &self, _unit: &str, _mode: &str, _properties: Vec<(String, OwnedVariant)>,
        _auxiliary: Vec<(String, Vec<(String, OwnedVariant)>)>,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        assert!(self.0.runtime.session().unwrap().owns_targets, "ownership must precede activation");
        self.0.starts.fetch_add(1, Ordering::SeqCst);
        Err(zbus::fdo::Error::Failed("anchor activation rejected".into()))
    }
}

struct Unit;

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Unit")]
impl Unit {
    #[zbus(property)]
    fn active_state(&self) -> &str { "inactive" }
}

async fn install(fixture: &mut Fixture) -> (Arc<State>, Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>) {
    fixture.coordinator.provider.kind = ProviderKind::Managed;
    fixture.coordinator.lifecycle.phase = SessionPhase::Starting;
    fixture.coordinator.ready_once = false;
    fixture.coordinator.environment = vec![("PATH".into(), std::env::var("PATH").unwrap())];
    let entered = Arc::new(tokio::sync::Notify::new());
    let resume = Arc::new(tokio::sync::Notify::new());
    let state = Arc::new(State {
        runtime: fixture.coordinator.runtime.clone(), starts: AtomicUsize::new(0), unavailable: AtomicBool::new(false),
        pause: Mutex::new(Some((entered.clone(), resume.clone()))),
    });
    fixture._server.object_server().at("/org/freedesktop/systemd1", Manager(state.clone())).await.unwrap();
    fixture._server.object_server().at("/unit", Unit).await.unwrap();
    (state, entered, resume)
}

async fn complete_check(fixture: &mut Fixture) {
    let work = tokio::time::timeout(Duration::from_secs(2), fixture.coordinator.work.recv()).await.unwrap().unwrap();
    assert!(matches!(work, Work::TargetsChecked(_)));
    fixture.coordinator.completed(work).await.unwrap();
}

#[tokio::test]
async fn a_pending_target_read_allows_status_and_cancellation_without_rejecting_finalize() {
    let mut fixture = Fixture::new().await;
    let (state, entered, resume) = install(&mut fixture).await;
    let (reply, mut finalized) = tokio::sync::mpsc::channel(1);
    fixture.coordinator.finalize_replies.push(Reply::for_test(reply));
    fixture.coordinator.activate();
    tokio::time::timeout(Duration::from_secs(2), entered.notified()).await.unwrap();
    assert_eq!(state.starts.load(Ordering::SeqCst), 0);

    let (reply, mut status) = tokio::sync::mpsc::channel(1);
    tokio::time::timeout(Duration::from_secs(1), fixture.coordinator.request(Request::Status(Reply::for_test(reply)))).await.unwrap().unwrap();
    assert_eq!(status.try_recv().unwrap().unwrap().phase, "starting");
    fixture.coordinator.begin_stop("logout", None).await.unwrap();
    let (reply, mut cancelled) = tokio::sync::mpsc::channel(1);
    fixture.coordinator.request(Request::Cancel { generation: GENERATION.into(), reply: Reply::for_test(reply) }).await.unwrap();
    assert!(cancelled.try_recv().unwrap().is_ok());
    assert!(finalized.try_recv().is_err());

    resume.notify_one();
    complete_check(&mut fixture).await;
    assert_eq!(fixture.coordinator.lifecycle.phase, SessionPhase::Starting);
    assert!(fixture.coordinator.runtime.session().unwrap().owns_targets);
    let work = tokio::time::timeout(Duration::from_secs(2), fixture.coordinator.work.recv()).await.unwrap().unwrap();
    assert!(matches!(work, Work::Ready(Err(_))));
    fixture.coordinator.completed(work).await.unwrap();
    assert!(finalized.try_recv().unwrap().unwrap_err().contains("anchor activation rejected"));
    assert_eq!(state.starts.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.coordinator.workers, 0);
}

#[tokio::test]
async fn a_transient_activation_preflight_failure_retries_without_resubmitting_a_start() {
    let mut fixture = Fixture::new().await;
    let (state, entered, resume) = install(&mut fixture).await;
    fixture.coordinator.ready_deadline = Instant::now() + Duration::from_secs(30);
    let deadline = fixture.coordinator.ready_deadline;
    let (reply, mut finalized) = tokio::sync::mpsc::channel(1);
    fixture.coordinator.finalize_replies.push(Reply::for_test(reply));
    fixture.coordinator.activate();
    tokio::time::timeout(Duration::from_secs(2), entered.notified()).await.unwrap();
    state.unavailable.store(true, Ordering::SeqCst);
    resume.notify_one();
    complete_check(&mut fixture).await;
    let failed = tokio::time::timeout(Duration::from_secs(7), fixture.coordinator.work.recv()).await.unwrap().unwrap();
    assert!(matches!(failed, Work::Ready(Err(_))));
    assert_eq!(state.starts.load(Ordering::SeqCst), 0);
    fixture.coordinator.completed(failed).await.unwrap();
    assert_eq!(fixture.coordinator.ready_deadline, deadline, "a read failure expired the startup budget");
    assert_eq!(finalized.try_recv(), Err(tokio::sync::mpsc::error::TryRecvError::Empty));
    assert!(!fixture.coordinator.ready_busy);

    state.unavailable.store(false, Ordering::SeqCst);
    fixture.coordinator.activate();
    complete_check(&mut fixture).await;
    let attempted = tokio::time::timeout(Duration::from_secs(2), fixture.coordinator.work.recv()).await.unwrap().unwrap();
    fixture.coordinator.completed(attempted).await.unwrap();
    assert!(finalized.try_recv().unwrap().unwrap_err().contains("anchor activation rejected"));
    assert_eq!(state.starts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn unconfirmed_or_retained_starts_are_not_retried_as_read_failures() {
    for retained in [false, true] {
        let mut fixture = Fixture::new().await;
        fixture.coordinator.ready_busy = true;
        fixture.coordinator.ready_deadline = Instant::now() + Duration::from_secs(30);
        let deadline = fixture.coordinator.ready_deadline;
        let (reply, mut finalized) = tokio::sync::mpsc::channel(1);
        fixture.coordinator.finalize_replies.push(Reply::for_test(reply));
        let error = SessionError::from(zbus::Error::from(zbus::fdo::Error::NoReply("start reply was lost".into())));
        let error = if retained { SessionError::StartRejected { error: Box::new(error), reference_retained: true } } else { error };

        fixture.coordinator.ready_completed(Err(error)).unwrap();
        assert!(finalized.try_recv().unwrap().is_err());
        assert!(fixture.coordinator.ready_deadline < deadline);
        assert!(!fixture.coordinator.ready_busy && !fixture.coordinator.pending_ready);
        assert!(fixture.coordinator.work.try_recv().is_err());
    }
}

#[tokio::test]
async fn stopping_while_ownership_is_pending_neither_claims_targets_nor_starts_the_anchor() {
    let mut fixture = Fixture::new().await;
    let (state, entered, resume) = install(&mut fixture).await;
    let (reply, mut finalized) = tokio::sync::mpsc::channel(1);
    fixture.coordinator.finalize_replies.push(Reply::for_test(reply));
    fixture.coordinator.activate();
    tokio::time::timeout(Duration::from_secs(2), entered.notified()).await.unwrap();
    fixture.coordinator.begin_stop("logout", None).await.unwrap();
    resume.notify_one();
    complete_check(&mut fixture).await;
    assert!(finalized.try_recv().unwrap().unwrap_err().contains("shutdown interrupted activation"));
    assert_eq!(state.starts.load(Ordering::SeqCst), 0);
    assert!(!fixture.coordinator.runtime.session().unwrap().owns_targets);
    assert!(!fixture.coordinator.ready_busy);
    assert_eq!(fixture.coordinator.workers, 0);
}

#[tokio::test]
async fn failed_ownership_persistence_prevents_anchor_activation() {
    let mut fixture = Fixture::new().await;
    let (state, entered, resume) = install(&mut fixture).await;
    fixture.coordinator.activate();
    tokio::time::timeout(Duration::from_secs(2), entered.notified()).await.unwrap();
    resume.notify_one();
    let work = tokio::time::timeout(Duration::from_secs(2), fixture.coordinator.work.recv()).await.unwrap().unwrap();
    assert!(matches!(work, Work::TargetsChecked(Ok(true))));
    let record = fixture.coordinator.runtime.path.join("session.toml");
    fs::remove_file(&record).unwrap();
    fs::create_dir(&record).unwrap();
    assert!(fixture.coordinator.completed(work).await.is_err());
    assert_eq!(state.starts.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.coordinator.workers, 0);
    assert!(fixture.coordinator.work.try_recv().is_err());
}

#[tokio::test]
async fn recovery_drains_target_observation_without_activating_or_claiming_targets() {
    let mut fixture = Fixture::new().await;
    let (state, entered, resume) = install(&mut fixture).await;
    fixture.coordinator.activate();
    tokio::time::timeout(Duration::from_secs(2), entered.notified()).await.unwrap();
    let stopping = fixture.coordinator.stopping.clone();
    let released = tokio::spawn(async move {
        while !stopping.load(Ordering::SeqCst) { tokio::task::yield_now().await; }
        resume.notify_one();
    });
    tokio::time::timeout(Duration::from_secs(2), fixture.coordinator.recover()).await.unwrap();
    released.await.unwrap();
    assert_eq!(state.starts.load(Ordering::SeqCst), 0);
    assert!(!fixture.coordinator.runtime.session().unwrap().owns_targets);
    assert_eq!(fixture.coordinator.workers, 0);
}

#[tokio::test]
async fn logind_can_interrupt_a_pending_target_read_without_activation() {
    let mut fixture = Fixture::new().await;
    let (state, entered, resume) = install(&mut fixture).await;
    fixture.coordinator.activate();
    tokio::time::timeout(Duration::from_secs(2), entered.notified()).await.unwrap();
    fixture.notify(true, 100_000).await;
    let deadline = fixture.deadline();
    complete_check(&mut fixture).await;
    assert_eq!(state.starts.load(Ordering::SeqCst), 0);
    assert!(!fixture.coordinator.runtime.session().unwrap().owns_targets);
    assert_eq!(fixture.coordinator.runtime.session().unwrap().shutdown_deadline_usec, Some(deadline));
    assert_eq!(fixture.coordinator.workers, 0);
    assert!(!fixture.coordinator.ready_busy);
    assert!(fixture.coordinator.manager.deadline.remaining(Duration::MAX).is_err());
    resume.notify_one();
}
