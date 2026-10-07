//! Private manager peer records cleanup attempts after helper start errors.

use std::{os::unix::net::UnixStream, sync::{Arc, atomic::{AtomicBool, AtomicUsize, Ordering}}, thread};

use rsdm_core::domain::{SessionPhase, ShutdownPolicy};
use zbus::{Connection, zvariant::{OwnedObjectPath, OwnedValue}};

use super::{bus::UserManager, identity::SessionIdentity, runtime::{AppRecord, SessionRecord}};

pub(super) const GENERATION: &str = "0123456789abcdef0123456789abcdef";

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Mode { Collision, Denied, InspectionDenied }

#[derive(Debug, zbus_macros::DBusError)]
#[zbus(prefix = "org.freedesktop")]
enum ManagerError {
    #[zbus(name = "systemd1.NoSuchUnit")]
    NoSuchUnit(String),
    #[zbus(name = "systemd1.UnitExists")]
    UnitExists(String),
    #[zbus(name = "DBus.Error.AccessDenied")]
    AccessDenied(String),
    #[zbus(error)]
    Bus(zbus::Error),
}

pub(super) struct State {
    mode: Mode,
    unit: String,
    exists: AtomicBool,
    pub(super) starts: AtomicUsize,
    pub(super) stops: AtomicUsize,
    pub(super) unrefs: AtomicUsize,
    pub(super) lifetime_bound: AtomicBool,
}

struct Manager(Arc<State>);

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Manager")]
impl Manager {
    #[zbus(property)]
    fn environment(&self) -> Vec<String> { vec![format!("RSDM_SESSION_GENERATION={GENERATION}")] }

    fn get_unit(&self, unit: &str) -> Result<OwnedObjectPath, ManagerError> {
        if unit.starts_with("app-rsdm-") { return Ok(OwnedObjectPath::try_from("/app").unwrap()); }
        if self.0.mode == Mode::InspectionDenied && self.0.starts.load(Ordering::SeqCst) > 0 {
            return Err(ManagerError::AccessDenied("accepted helper cannot be inspected".into()));
        }
        if unit != self.0.unit || !self.0.exists.load(Ordering::SeqCst) {
            return Err(ManagerError::NoSuchUnit("not loaded".into()));
        }
        Ok(OwnedObjectPath::try_from("/helper").unwrap())
    }

    fn start_transient_unit(
        &self, _unit: &str, _mode: &str, properties: Vec<(String, OwnedValue)>,
        _auxiliary: Vec<(String, Vec<(String, OwnedValue)>)>,
    ) -> Result<OwnedObjectPath, ManagerError> {
        self.0.starts.fetch_add(1, Ordering::SeqCst);
        self.0.lifetime_bound.store(properties.iter().any(|(name, _)| *name == "PartOf")
            && properties.iter().any(|(name, _)| *name == "Requisite"), Ordering::SeqCst);
        self.0.exists.store(self.0.mode != Mode::Denied, Ordering::SeqCst);
        match self.0.mode {
            Mode::Collision => Err(ManagerError::UnitExists("another unit owns this name".into())),
            Mode::Denied => Err(ManagerError::AccessDenied("start rejected".into())),
            Mode::InspectionDenied => Ok(OwnedObjectPath::try_from("/job/42").unwrap()),
        }
    }

    fn stop_unit(&self, _unit: &str, _mode: &str) -> zbus::fdo::Result<OwnedObjectPath> {
        self.0.stops.fetch_add(1, Ordering::SeqCst);
        Err(zbus::fdo::Error::AccessDenied("recorded stop attempt".into()))
    }

    fn unref_unit(&self, _unit: &str) { self.0.unrefs.fetch_add(1, Ordering::SeqCst); }
}

struct Unit(Arc<State>);

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Unit")]
impl Unit {
    #[zbus(property)]
    fn id(&self) -> &str { &self.0.unit }
    #[zbus(property)]
    fn transient(&self) -> bool { true }
    #[zbus(property)]
    fn job(&self) -> (u32, OwnedObjectPath) { (0, OwnedObjectPath::try_from("/").unwrap()) }
    #[zbus(property)]
    fn active_state(&self) -> &str { "active" }
}

struct Service;

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Service")]
impl Service {
    #[zbus(property)]
    fn environment(&self) -> Vec<String> { vec![format!("RSDM_SESSION_GENERATION={GENERATION}")] }
    #[zbus(property)]
    fn working_directory(&self) -> &str { "/" }
}

pub(super) struct Fixture {
    pub(super) manager: UserManager,
    pub(super) state: Arc<State>,
    _server: Connection,
}

impl Fixture {
    pub(super) fn new(unit: String, mode: Mode) -> Self {
        let state = Arc::new(State { mode, unit, exists: AtomicBool::new(false),
            starts: AtomicUsize::new(0), stops: AtomicUsize::new(0), unrefs: AtomicUsize::new(0),
            lifetime_bound: AtomicBool::new(false) });
        let shared = Arc::clone(&state);
        let (server_socket, client_socket) = UnixStream::pair().unwrap();
        let server = thread::spawn(move || async_io::block_on(async {
            zbus::connection::Builder::unix_stream(server_socket).p2p().server(zbus::Guid::generate()).unwrap()
                .serve_at("/org/freedesktop/systemd1", Manager(Arc::clone(&shared))).unwrap()
                .serve_at("/app", Service).unwrap().serve_at("/helper", Unit(shared)).unwrap()
                .build().await.unwrap()
        }));
        let connection = async_io::block_on(async {
            zbus::connection::Builder::unix_stream(client_socket).p2p().build().await.unwrap()
        });
        Self { manager: UserManager::with_connection(connection), state, _server: server.join().unwrap() }
    }
}

pub(super) fn command() -> Vec<String> {
    vec![std::env::current_exe().unwrap().to_string_lossy().into_owned()]
}

pub(super) fn app() -> AppRecord {
    AppRecord { unit: format!("app-rsdm-example@{GENERATION}-1.service"), invocation_id: vec![1; 16],
        policy: ShutdownPolicy { quit_command: command(), ..ShutdownPolicy::default() },
        deadline_usec: None, quit_started: false }
}

pub(super) fn record() -> SessionRecord {
    SessionRecord { identity: SessionIdentity { generation: GENERATION.into(), login_session_id: "test".into(),
            desktop_entry_id: None, uid: 0 },
        anchor_unit: format!("rsdm-session-{GENERATION}.service"), compositor_unit: None,
        compositor_invocation: Vec::new(), provider: "external".into(), owns_targets: false,
        phase: SessionPhase::Running, exported_environment: Vec::new(), shutdown_deadline_usec: None }
}
