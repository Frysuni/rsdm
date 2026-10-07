//! Private user-manager fixture for job and start-ownership regressions.

use std::{os::unix::net::UnixStream, sync::{Arc, atomic::{AtomicBool, AtomicUsize, Ordering}}, thread};

use super::*;

#[derive(Debug, zbus_macros::DBusError)]
#[zbus(prefix = "org.freedesktop")]
enum ManagerError {
    #[zbus(name = "systemd1.NoSuchUnit")]
    NoSuchUnit(String),
    #[zbus(name = "systemd1.UnitExists")]
    UnitExists(String),
    #[zbus(name = "DBus.Error.NoReply")]
    NoReply(String),
    #[zbus(name = "DBus.Error.InvalidArgs")]
    InvalidArgs(String),
    #[zbus(name = "DBus.Error.AccessDenied")]
    AccessDenied(String),
    #[zbus(name = "DBus.Error.UnknownProperty")]
    UnknownProperty(String),
    #[zbus(name = "DBus.Error.PropertyReadOnly")]
    PropertyReadOnly(String),
    #[zbus(name = "DBus.Error.NotSupported")]
    NotSupported(String),
    #[zbus(error)]
    Bus(zbus::Error),
}

#[derive(Clone, Copy)]
pub(super) enum Rejection {
    UnitExists,
    InvalidArgs,
    AccessDenied,
    UnknownProperty,
    PropertyReadOnly,
    NotSupported,
}

impl Rejection {
    fn error(self) -> ManagerError {
        match self {
            Self::UnitExists => ManagerError::UnitExists("already exists".into()),
            Self::InvalidArgs => ManagerError::InvalidArgs("invalid properties".into()),
            Self::AccessDenied => ManagerError::AccessDenied("start denied".into()),
            Self::UnknownProperty => ManagerError::UnknownProperty("unknown property".into()),
            Self::PropertyReadOnly => ManagerError::PropertyReadOnly("read-only property".into()),
            Self::NotSupported => ManagerError::NotSupported("unsupported unit".into()),
        }
    }
}

pub(super) struct State {
    pub(super) exists: AtomicBool,
    pub(super) calls: AtomicUsize,
    pub(super) lost_reply: bool,
    pub(super) active: bool,
    pub(super) status: i32,
    pub(super) generation: &'static str,
    pub(super) preflight_delay: Duration,
    pub(super) reply_delay: Duration,
    pub(super) inspection_delay: Duration,
    pub(super) rejection: Option<Rejection>,
    pub(super) created_on_rejection: bool,
    pub(super) denied_inspection: bool,
    pub(super) denied_preflight: bool,
    pub(super) queries: AtomicUsize,
}

struct Manager(Arc<State>);

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Manager")]
impl Manager {
    #[zbus(property)]
    fn version(&self) -> &str { "260" }

    fn subscribe(&self) {}

    async fn get_unit(&self, _name: &str) -> Result<OwnedObjectPath, ManagerError> {
        self.0.queries.fetch_add(1, Ordering::SeqCst);
        if self.0.denied_preflight || (self.0.denied_inspection && self.0.calls.load(Ordering::SeqCst) > 0) {
            return Err(Rejection::AccessDenied.error());
        }
        let delay = if self.0.calls.load(Ordering::SeqCst) == 0 {
            self.0.preflight_delay
        } else {
            self.0.inspection_delay
        };
        if !delay.is_zero() { async_io::Timer::after(delay).await; }
        if !self.0.exists.load(Ordering::SeqCst) {
            return Err(ManagerError::NoSuchUnit("not loaded".into()));
        }
        Ok(OwnedObjectPath::try_from("/unit").unwrap())
    }

    async fn start_transient_unit(
        &self, _unit: &str, _mode: &str,
        _properties: Vec<(String, OwnedValue)>, _auxiliary: Vec<(String, Vec<(String, OwnedValue)>)>,
    ) -> Result<OwnedObjectPath, ManagerError> {
        self.0.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(rejection) = self.0.rejection {
            self.0.exists.store(self.0.created_on_rejection, Ordering::SeqCst);
            return Err(rejection.error());
        }
        self.0.exists.store(true, Ordering::SeqCst);
        if !self.0.reply_delay.is_zero() { async_io::Timer::after(self.0.reply_delay).await; }
        self.reply()
    }

    async fn stop_unit(&self, _unit: &str, _mode: &str) -> Result<OwnedObjectPath, ManagerError> {
        self.0.calls.fetch_add(1, Ordering::SeqCst);
        if !self.0.reply_delay.is_zero() { async_io::Timer::after(self.0.reply_delay).await; }
        self.reply()
    }

    fn get_unit_processes(&self, _unit: &str) -> Vec<(String, u32, String)> {
        if self.0.active { vec![("/unit".into(), 123, "app".into())] } else { Vec::new() }
    }

    #[zbus(signal)]
    async fn job_removed(
        emitter: &zbus::object_server::SignalEmitter<'_>, id: u32,
        job: &OwnedObjectPath, unit: &str, result: &str,
    ) -> zbus::Result<()>;
}

impl Manager {
    fn reply(&self) -> Result<OwnedObjectPath, ManagerError> {
        if self.0.lost_reply { return Err(ManagerError::NoReply("reply lost after accepting request".into())); }
        Ok(OwnedObjectPath::try_from("/job/42").unwrap())
    }
}

struct Unit(Arc<State>);

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Unit")]
impl Unit {
    #[zbus(property)]
    fn id(&self) -> &str { "example.service" }
    #[zbus(property)]
    fn job(&self) -> (u32, OwnedObjectPath) { (0, OwnedObjectPath::try_from("/").unwrap()) }
    #[zbus(property)]
    fn transient(&self) -> bool { true }
    #[zbus(property)]
    fn active_state(&self) -> &str { if self.0.active { "active" } else { "inactive" } }
}

struct Service(Arc<State>);

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Service")]
impl Service {
    #[zbus(property)]
    fn environment(&self) -> Vec<String> { vec![format!("RSDM_SESSION_GENERATION={}", self.0.generation)] }
    #[zbus(property)]
    fn result(&self) -> &str { "success" }
    #[zbus(property)]
    fn exec_main_code(&self) -> i32 { libc::CLD_EXITED }
    #[zbus(property)]
    fn exec_main_status(&self) -> i32 { self.0.status }
}

pub(super) fn fixture(state: State) -> (UserManager, Connection, Arc<State>) {
    let state = Arc::new(state);
    let shared = state.clone();
    let (server_socket, client_socket) = UnixStream::pair().unwrap();
    let server = thread::spawn(move || async_io::block_on(async {
        zbus::connection::Builder::unix_stream(server_socket).p2p().server(zbus::Guid::generate()).unwrap()
            .serve_at(MANAGER_PATH, Manager(shared.clone())).unwrap()
            .serve_at("/unit", Unit(shared.clone())).unwrap()
            .serve_at("/unit", Service(shared)).unwrap().build().await.unwrap()
    }));
    let connection = async_io::block_on(async {
        zbus::connection::Builder::unix_stream(client_socket).p2p().build().await.unwrap()
    });
    let server = server.join().unwrap();
    let manager = async_io::block_on(UserManager::from_connection(connection)).unwrap();
    (manager, server, state)
}

pub(super) fn state(lost_reply: bool, active: bool, status: i32) -> State {
    State { exists: AtomicBool::new(false), calls: AtomicUsize::new(0), lost_reply, active, status,
        generation: "ours", preflight_delay: Duration::ZERO, reply_delay: Duration::ZERO,
        inspection_delay: Duration::ZERO, rejection: None, created_on_rejection: false,
        denied_inspection: false, denied_preflight: false, queries: AtomicUsize::new(0) }
}
