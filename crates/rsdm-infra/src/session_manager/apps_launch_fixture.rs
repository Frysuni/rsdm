//! Launch persistence checks use private records and a private manager peer.

use std::{
    fs, os::unix::{fs::PermissionsExt, net::UnixStream}, path::PathBuf,
    sync::{Arc, atomic::{AtomicBool, AtomicUsize, Ordering}}, thread,
};

use zbus::{Connection, zvariant::{OwnedObjectPath, OwnedValue}};

use super::*;

const GENERATION: &str = "0123456789abcdef0123456789abcdef";
static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Mode {
    Accepted,
    Denied,
    Collision,
    LostReply,
    PartialDenied,
    InvocationDenied,
    InspectionDenied,
    Foreign,
}

#[derive(Debug, zbus_macros::DBusError)]
#[zbus(prefix = "org.freedesktop")]
enum ManagerError {
    #[zbus(name = "systemd1.NoSuchUnit")]
    NoSuchUnit(String),
    #[zbus(name = "systemd1.UnitExists")]
    UnitExists(String),
    #[zbus(name = "DBus.Error.AccessDenied")]
    AccessDenied(String),
    #[zbus(name = "DBus.Error.NoReply")]
    NoReply(String),
    #[zbus(error)]
    Bus(zbus::Error),
}

pub(super) struct State {
    unit: String,
    mode: Mode,
    exists: AtomicBool,
    pub(super) starts: AtomicUsize,
    pub(super) invocation_reads: AtomicUsize,
    pub(super) unrefs: AtomicUsize,
}

struct Manager(Arc<State>);

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Manager")]
impl Manager {
    fn get_unit(&self, unit: &str) -> Result<OwnedObjectPath, ManagerError> {
        if self.0.mode == Mode::InspectionDenied && self.0.starts.load(Ordering::SeqCst) > 0 {
            return Err(ManagerError::AccessDenied("inspection denied after acceptance".into()));
        }
        if unit != self.0.unit || !self.0.exists.load(Ordering::SeqCst) {
            return Err(ManagerError::NoSuchUnit("not loaded".into()));
        }
        Ok(OwnedObjectPath::try_from("/unit").unwrap())
    }

    fn start_transient_unit(
        &self, _unit: &str, _mode: &str, _properties: Vec<(String, OwnedValue)>,
        _auxiliary: Vec<(String, Vec<(String, OwnedValue)>)>,
    ) -> Result<OwnedObjectPath, ManagerError> {
        self.0.starts.fetch_add(1, Ordering::SeqCst);
        self.0.exists.store(self.0.mode != Mode::Denied, Ordering::SeqCst);
        match self.0.mode {
            Mode::Denied | Mode::PartialDenied => Err(ManagerError::AccessDenied("start denied".into())),
            Mode::Collision => Err(ManagerError::UnitExists("another unit won the name".into())),
            Mode::LostReply => Err(ManagerError::NoReply("accepted before the reply was lost".into())),
            _ => Ok(OwnedObjectPath::try_from("/job/42").unwrap()),
        }
    }

    fn get_unit_processes(&self, _unit: &str) -> Vec<(String, u32, String)> { Vec::new() }
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
    fn active_state(&self) -> &str { if self.0.mode == Mode::PartialDenied { "inactive" } else { "active" } }
    #[zbus(property, name = "InvocationID")]
    fn invocation_id(&self) -> zbus::fdo::Result<Vec<u8>> {
        self.0.invocation_reads.fetch_add(1, Ordering::SeqCst);
        if self.0.mode == Mode::InvocationDenied { return Err(zbus::fdo::Error::AccessDenied("invocation denied".into())); }
        Ok(vec![if self.0.mode == Mode::PartialDenied { 0 } else { 1 }; 16])
    }
}

struct Service(Arc<State>);

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Service")]
impl Service {
    #[zbus(property)]
    fn environment(&self) -> Vec<String> {
        let generation = if matches!(self.0.mode, Mode::Foreign | Mode::Collision) { "foreign" } else { GENERATION };
        vec![format!("RSDM_SESSION_GENERATION={generation}")]
    }
}

pub(super) struct Fixture {
    pub(super) manager: UserManager,
    pub(super) runtime: Runtime,
    pub(super) request: LaunchRequest,
    pub(super) app: AppRecord,
    pub(super) state: Arc<State>,
    _server: Connection,
    directory: PathBuf,
}

impl Fixture {
    pub(super) fn new(mode: Mode) -> Self {
        let directory = std::env::temp_dir().join(format!("rsdm-app-launch-{}-{}",
            std::process::id(), NEXT_ID.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        fs::create_dir(directory.join("apps")).unwrap();
        fs::set_permissions(directory.join("apps"), fs::Permissions::from_mode(0o700)).unwrap();
        let runtime = Runtime { path: directory.clone(), generation: GENERATION.into() };
        let request = LaunchRequest {
            generation: GENERATION.into(), argv: vec![std::env::current_exe().unwrap().to_string_lossy().into_owned()],
            environment: Vec::new(), directory: directory.to_string_lossy().into_owned(),
            timeout_secs: 10, on_timeout: "force".into(), method: "auto".into(), quit_command: Vec::new(),
        };
        let provider = Provider { kind: crate::session_manager::provider::ProviderKind::Managed,
            native_unit: None, logout_command: Vec::new() };
        let app = register(&runtime, &provider, &request).unwrap();
        let state = Arc::new(State { unit: app.unit.clone(), mode, exists: AtomicBool::new(false),
            starts: AtomicUsize::new(0), invocation_reads: AtomicUsize::new(0), unrefs: AtomicUsize::new(0) });
        let shared = Arc::clone(&state);
        let (server_socket, client_socket) = UnixStream::pair().unwrap();
        let server = thread::spawn(move || async_io::block_on(async {
            zbus::connection::Builder::unix_stream(server_socket).p2p().server(zbus::Guid::generate()).unwrap()
                .serve_at("/org/freedesktop/systemd1", Manager(Arc::clone(&shared))).unwrap()
                .serve_at("/unit", Unit(Arc::clone(&shared))).unwrap()
                .serve_at("/unit", Service(shared)).unwrap().build().await.unwrap()
        }));
        let connection = async_io::block_on(async {
            zbus::connection::Builder::unix_stream(client_socket).p2p().build().await.unwrap()
        });
        Self { manager: UserManager::with_connection(connection), runtime, request, app, state,
            _server: server.join().unwrap(), directory }
    }

    pub(super) fn launch(&self) -> Result<String, SessionError> {
        launch(&self.manager, &self.runtime, &format!("rsdm-session-{GENERATION}.service"), &self.request, self.app.clone())
    }

    pub(super) fn missing_command(&mut self) {
        self.request.argv = vec![self.directory.join("missing").to_string_lossy().into_owned()];
    }
}

impl Drop for Fixture {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.directory); }
}
