//! Private systemd peer for quit-command identity and cancellation races.

use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf, sync::{Arc, atomic::{AtomicBool, AtomicU8, AtomicUsize}}};

use zbus::{Connection, OwnedObjectPath, OwnedVariant};

use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Stage { Environment, Directory, DirectoryFailure, Preflight, FinalIdentity, Accepted, Never }

pub(super) struct State {
    pub(super) invocation: AtomicU8,
    pub(super) replacement: std::sync::Mutex<Option<Stage>>,
    preflight_complete: AtomicBool,
    pub(super) expire_on_final_identity: AtomicBool,
    control: Arc<ShutdownControl>,
    cancel_at: Stage,
    unit: String,
    pub(super) starts: AtomicUsize,
    pub(super) lifetime: std::sync::Mutex<Vec<(String, Vec<String>)>>,
    pub(super) stops: AtomicUsize,
    pub(super) unrefs: AtomicUsize,
    pub(super) block_preflight: AtomicBool,
    pub(super) preflight_entered: tokio::sync::Notify,
    pub(super) preflight_reply: tokio::sync::Semaphore,
}

impl State {
    fn reach(&self, point: Stage) {
        if *self.replacement.lock().unwrap() == Some(point) { self.invocation.store(2, Ordering::SeqCst); }
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
        if unit == crate::attach::UNIT { return Err(ManagerError::NoSuchUnit("coordinator was collected".into())); }
        assert_eq!(unit, self.0.unit);
        if self.0.block_preflight.load(Ordering::SeqCst) && self.0.starts.load(Ordering::SeqCst) == 0 {
            self.0.preflight_entered.notify_one();
            self.0.preflight_reply.acquire().await.unwrap().forget();
        }
        self.0.reach(Stage::Preflight);
        self.0.preflight_complete.store(true, Ordering::SeqCst);
        if self.0.starts.load(Ordering::SeqCst) == 0 { return Err(ManagerError::NoSuchUnit("not loaded".into())); }
        if self.0.cancel_at == Stage::Accepted { return Err(ManagerError::AccessDenied("cannot confirm accepted start".into())); }
        Ok(OwnedObjectPath::try_from("/helper").unwrap())
    }

    fn start_transient_unit(
        &self, unit: &str, _mode: &str, properties: Vec<(String, OwnedVariant)>,
        _auxiliary: Vec<(String, Vec<(String, OwnedVariant)>)>,
    ) -> Result<OwnedObjectPath, ManagerError> {
        assert_eq!(unit, self.0.unit);
        for (name, value) in properties {
            if matches!(name.as_str(), "PartOf" | "Requisite") {
                self.0.lifetime.lock().unwrap().push((name, value.try_into().unwrap()));
            }
        }
        self.0.starts.fetch_add(1, Ordering::SeqCst);
        self.0.reach(Stage::Accepted);
        if self.0.cancel_at == Stage::Accepted { return Err(ManagerError::NoReply("reply lost after accepting start".into())); }
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

struct ApplicationUnit(Arc<State>);

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Unit")]
impl ApplicationUnit {
    #[zbus(property, name = "InvocationID")]
    fn invocation_id(&self) -> Vec<u8> {
        if self.0.preflight_complete.load(Ordering::SeqCst) {
            self.0.reach(Stage::FinalIdentity);
            if self.0.expire_on_final_identity.load(Ordering::SeqCst) { self.0.control.hard_deadline.set(1); }
        }
        vec![self.0.invocation.load(Ordering::SeqCst); 16]
    }
    #[zbus(property)]
    fn active_state(&self) -> &str { "active" }
}

struct Service(Arc<State>);

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Service")]
impl Service {
    #[zbus(property)]
    fn environment(&self) -> Vec<String> {
        self.0.reach(Stage::Environment);
        vec![format!("RSDM_SESSION_GENERATION={}", crate::helper_start_fixture::GENERATION)]
    }

    #[zbus(property)]
    fn working_directory(&self) -> zbus::fdo::Result<&str> {
        self.0.reach(Stage::Directory);
        self.0.reach(Stage::DirectoryFailure);
        if self.0.cancel_at == Stage::DirectoryFailure { return Err(zbus::fdo::Error::AccessDenied("working directory unavailable".into())); }
        Ok("/")
    }
}

pub(super) struct Fixture {
    directory: PathBuf,
    pub(super) runtime: Runtime,
    pub(super) manager: UserManager,
    _server: Connection,
    pub(super) state: Arc<State>,
}

impl Fixture {
    pub(super) async fn new(control: Arc<ShutdownControl>, cancel_at: Stage) -> Self {
        let app = crate::helper_start_fixture::app();
        let state = Arc::new(State { control, cancel_at, invocation: AtomicU8::new(1), replacement: std::sync::Mutex::new(None), unit: app.unit.replacen("app-rsdm-", "rsdm-quit-", 1),
            preflight_complete: AtomicBool::new(false), expire_on_final_identity: AtomicBool::new(false),
            starts: AtomicUsize::new(0), lifetime: std::sync::Mutex::new(Vec::new()), stops: AtomicUsize::new(0), unrefs: AtomicUsize::new(0),
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
            zbus::connection::Builder::unix_stream(server.into_std().unwrap()).p2p().server(zbus::Guid::generate())
                .serve_at("/org/freedesktop/systemd1", Manager(shared.clone()))
                .serve_at("/app", Service(shared.clone()))
                .serve_at("/app", ApplicationUnit(shared.clone()))
                .serve_at("/helper", Unit(shared.clone()))
                .serve_at("/helper", Service(shared)).build().await.unwrap()
        });
        let client = zbus::connection::Builder::unix_stream(client.into_std().unwrap()).p2p().build().await.unwrap();

        Self { directory, runtime, manager: UserManager::with_connection(client), _server: server.await.unwrap(), state }
    }

    pub(super) fn saved_dispatch(&self) -> Option<crate::app_dispatch::Dispatch> {
        self.runtime.app(&crate::helper_start_fixture::app().unit).unwrap().dispatch
    }
}

impl Drop for Fixture {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.directory); }
}
