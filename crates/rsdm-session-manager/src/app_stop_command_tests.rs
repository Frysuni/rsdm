//! Quit command cancellation is distinct from a possibly delivered start.

use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf, sync::{Arc, atomic::{AtomicBool, AtomicUsize}}};

use zbus::{Connection, zvariant::{OwnedObjectPath, OwnedValue}};

use super::*;
use crate::app_dispatch::State as DispatchState;

#[derive(Clone, Copy, PartialEq, Eq)]
enum CancelAt { Environment, Directory, DirectoryFailure, Preflight, Accepted, Never }

struct State {
    control: Arc<ShutdownControl>,
    cancel_at: CancelAt,
    unit: String,
    starts: AtomicUsize,
    stops: AtomicUsize,
    unrefs: AtomicUsize,
    block_preflight: AtomicBool,
    preflight_entered: tokio::sync::Notify,
    preflight_reply: tokio::sync::Semaphore,
}

impl State {
    fn cancel(&self, point: CancelAt) {
        if self.cancel_at == point { self.control.cancelled.store(true, Ordering::SeqCst); }
    }
}

#[derive(Debug, zbus_macros::DBusError)]
#[zbus(prefix = "org.freedesktop")]
enum ManagerError {
    #[zbus(name = "systemd1.NoSuchUnit")]
    NoSuchUnit(String),
    #[zbus(name = "DBus.Error.NoReply")]
    NoReply(String),
    #[zbus(name = "DBus.Error.AccessDenied")]
    AccessDenied(String),
    #[zbus(error)]
    Bus(zbus::Error),
}

struct Manager(Arc<State>);

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Manager")]
impl Manager {
    async fn get_unit(&self, unit: &str) -> Result<OwnedObjectPath, ManagerError> {
        if unit.starts_with("app-rsdm-") { return Ok(OwnedObjectPath::try_from("/app").unwrap()); }
        assert_eq!(unit, self.0.unit);
        if self.0.block_preflight.load(Ordering::SeqCst) && self.0.starts.load(Ordering::SeqCst) == 0 {
            self.0.preflight_entered.notify_one();
            self.0.preflight_reply.acquire().await.unwrap().forget();
        }
        self.0.cancel(CancelAt::Preflight);
        if self.0.starts.load(Ordering::SeqCst) == 0 { return Err(ManagerError::NoSuchUnit("not loaded".into())); }
        if self.0.cancel_at == CancelAt::Accepted { return Err(ManagerError::AccessDenied("cannot confirm accepted start".into())); }
        Ok(OwnedObjectPath::try_from("/helper").unwrap())
    }

    fn start_transient_unit(
        &self, unit: &str, _mode: &str, _properties: Vec<(String, OwnedValue)>,
        _auxiliary: Vec<(String, Vec<(String, OwnedValue)>)>,
    ) -> Result<OwnedObjectPath, ManagerError> {
        assert_eq!(unit, self.0.unit);
        self.0.starts.fetch_add(1, Ordering::SeqCst);
        self.0.cancel(CancelAt::Accepted);
        if self.0.cancel_at == CancelAt::Accepted { return Err(ManagerError::NoReply("reply lost after accepting start".into())); }
        Ok(OwnedObjectPath::try_from("/job/42").unwrap())
    }

    fn stop_unit(&self, _unit: &str, _mode: &str) -> zbus::fdo::Result<OwnedObjectPath> {
        self.0.stops.fetch_add(1, Ordering::SeqCst);
        Err(zbus::fdo::Error::AccessDenied("recorded cleanup".into()))
    }

    fn unref_unit(&self, _unit: &str) { self.0.unrefs.fetch_add(1, Ordering::SeqCst); }
}

struct Unit(Arc<State>);

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Unit")]
impl Unit {
    #[zbus(property)]
    fn id(&self) -> &str { &self.0.unit }
    #[zbus(property)]
    fn job(&self) -> (u32, OwnedObjectPath) { (0, OwnedObjectPath::try_from("/").unwrap()) }
    #[zbus(property)]
    fn transient(&self) -> bool { true }
    #[zbus(property)]
    fn active_state(&self) -> &str { "active" }
}

struct Service(Arc<State>);

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Service")]
impl Service {
    #[zbus(property)]
    fn environment(&self) -> Vec<String> {
        self.0.cancel(CancelAt::Environment);
        vec![format!("RSDM_SESSION_GENERATION={}", crate::helper_start_fixture::GENERATION)]
    }

    #[zbus(property)]
    fn working_directory(&self) -> zbus::fdo::Result<&str> {
        self.0.cancel(CancelAt::Directory);
        self.0.cancel(CancelAt::DirectoryFailure);
        if self.0.cancel_at == CancelAt::DirectoryFailure { return Err(zbus::fdo::Error::AccessDenied("working directory unavailable".into())); }
        Ok("/")
    }
}

struct Fixture { directory: PathBuf, runtime: Runtime, manager: UserManager, _server: Connection, state: Arc<State> }

impl Fixture {
    async fn new(control: Arc<ShutdownControl>, cancel_at: CancelAt) -> Self {
        let app = crate::helper_start_fixture::app();
        let state = Arc::new(State { control, cancel_at, unit: app.unit.replacen("app-rsdm-", "rsdm-quit-", 1),
            starts: AtomicUsize::new(0), stops: AtomicUsize::new(0), unrefs: AtomicUsize::new(0),
            block_preflight: AtomicBool::new(false), preflight_entered: tokio::sync::Notify::new(),
            preflight_reply: tokio::sync::Semaphore::new(0) });
        let directory = std::env::temp_dir().join(format!("rsdm-command-cancel-{}", zbus::Guid::generate()));
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        let runtime = Runtime::create_at(&directory, crate::helper_start_fixture::GENERATION).unwrap();
        let mut record = crate::helper_start_fixture::record();
        // SAFETY: geteuid has no preconditions.
        record.identity.uid = unsafe { libc::geteuid() };
        runtime.save_session(&record).unwrap();
        runtime.save_app(&app).unwrap();
        let (server, client) = tokio::net::UnixStream::pair().unwrap();
        let shared = state.clone();
        let server = tokio::spawn(async move {
            zbus::connection::Builder::unix_stream(server).p2p().server(zbus::Guid::generate()).unwrap()
                .serve_at("/org/freedesktop/systemd1", Manager(shared.clone())).unwrap()
                .serve_at("/app", Service(shared.clone())).unwrap()
                .serve_at("/helper", Unit(shared.clone())).unwrap()
                .serve_at("/helper", Service(shared)).unwrap().build().await.unwrap()
        });
        let client = zbus::connection::Builder::unix_stream(client).p2p().build().await.unwrap();
        Self { directory, runtime, manager: UserManager::with_connection(client), _server: server.await.unwrap(), state }
    }

    fn saved_dispatch(&self) -> Option<crate::app_dispatch::Dispatch> {
        self.runtime.app(&crate::helper_start_fixture::app().unit).unwrap().dispatch
    }
}

impl Drop for Fixture {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.directory); }
}

#[tokio::test]
async fn cancellation_during_command_preparation_or_start_preflight_never_sends_start() {
    for point in [CancelAt::Environment, CancelAt::Directory, CancelAt::DirectoryFailure, CancelAt::Preflight] {
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
    let fixture = Fixture::new(control.clone(), CancelAt::Never).await;
    let app = crate::helper_start_fixture::app();
    assert!(request_quit(&fixture.manager, &fixture.runtime, &app, &control).await.unwrap().is_none());
    assert!(fixture.saved_dispatch().is_none());
    assert_eq!(fixture.state.starts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_forced_command_can_start_despite_cancellation_during_preflight() {
    let control = Arc::new(ShutdownControl::default());
    control.force(0);
    let fixture = Fixture::new(control.clone(), CancelAt::Preflight).await;
    let app = crate::helper_start_fixture::app();
    assert!(request_quit(&fixture.manager, &fixture.runtime, &app, &control).await.unwrap().is_some());
    assert!(!control.cancelled());
    assert_eq!(fixture.saved_dispatch().unwrap().state, DispatchState::Sent);
    assert_eq!(fixture.state.starts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn cancellation_and_lost_reply_after_acceptance_preserve_intent_and_never_repeat_start() {
    let control = Arc::new(ShutdownControl::default());
    let fixture = Fixture::new(control.clone(), CancelAt::Accepted).await;
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
    let fixture = Fixture::new(control.clone(), CancelAt::Never).await;
    fixture.state.block_preflight.store(true, Ordering::SeqCst);
    let app = crate::helper_start_fixture::app();
    let mut request = Box::pin(request_quit(&fixture.manager, &fixture.runtime, &app, &control));
    tokio::select! {
        result = tokio::time::timeout(Duration::from_secs(2), fixture.state.preflight_entered.notified()) => result.unwrap(),
        result = &mut request => panic!("start completed before its preflight reply: {result:?}"),
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
        result = &mut request => panic!("queued start completed with all request slots held: {result:?}"),
    }
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
    let fixture = Fixture::new(control.clone(), CancelAt::Never).await;
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
