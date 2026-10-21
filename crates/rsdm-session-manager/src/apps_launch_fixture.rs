//! Launch persistence checks use private records and a private manager peer.

use std::{
    fs, os::unix::fs::PermissionsExt, path::PathBuf,
    sync::{Arc, Mutex, atomic::{AtomicBool, AtomicUsize, Ordering}},
};

use zbus::{Connection, OwnedObjectPath, OwnedVariant};

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
    pub(super) dependencies: Mutex<Vec<String>>,
    pub(super) compositor_exists: AtomicBool,
    pub(super) compositor_invocation: Mutex<Vec<u8>>,
    pub(super) compositor_active: Mutex<String>,
    pub(super) compositor_query_failed: AtomicBool,
}

struct Manager(Arc<State>);

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Manager")]
impl Manager {
    fn get_unit(&self, unit: &str) -> Result<OwnedObjectPath, ManagerError> {
        if unit == "compositor.service" {
            return if self.0.compositor_exists.load(Ordering::SeqCst) { Ok(OwnedObjectPath::try_from("/compositor").unwrap()) }
                else { Err(ManagerError::NoSuchUnit("compositor is absent".into())) };
        }
        if self.0.mode == Mode::InspectionDenied && self.0.starts.load(Ordering::SeqCst) > 0 {
            return Err(ManagerError::AccessDenied("inspection denied after acceptance".into()));
        }
        if unit != self.0.unit || !self.0.exists.load(Ordering::SeqCst) {
            return Err(ManagerError::NoSuchUnit("not loaded".into()));
        }
        Ok(OwnedObjectPath::try_from("/unit").unwrap())
    }

    fn start_transient_unit(
        &self, _unit: &str, _mode: &str, properties: Vec<(String, OwnedVariant)>,
        _auxiliary: Vec<(String, Vec<(String, OwnedVariant)>)>,
    ) -> Result<OwnedObjectPath, ManagerError> {
        self.0.starts.fetch_add(1, Ordering::SeqCst);
        *self.0.dependencies.lock().unwrap() = properties.into_iter().map(|(name, _)| name)
            .filter(|name| matches!(name.as_str(), "After" | "PartOf" | "Requisite")).collect();
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

struct Compositor(Arc<State>);

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Unit")]
impl Compositor {
    #[zbus(property, name = "InvocationID")]
    fn invocation_id(&self) -> zbus::fdo::Result<Vec<u8>> {
        if self.0.compositor_query_failed.load(Ordering::SeqCst) { return Err(zbus::fdo::Error::Failed("compositor query failed".into())); }
        Ok(self.0.compositor_invocation.lock().unwrap().clone())
    }

    #[zbus(property)]
    fn active_state(&self) -> String { self.0.compositor_active.lock().unwrap().clone() }
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
    pub(super) async fn new(mode: Mode) -> Self {
        let directory = std::env::temp_dir().join(format!("rsdm-app-launch-{}-{}",
            std::process::id(), NEXT_ID.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        fs::create_dir(directory.join("apps")).unwrap();
        fs::set_permissions(directory.join("apps"), fs::Permissions::from_mode(0o700)).unwrap();
        let runtime = Runtime { path: directory.clone(), generation: GENERATION.into() };
        runtime.save_session(&crate::runtime::SessionRecord {
            identity: crate::identity::SessionIdentity { generation: GENERATION.into(), login_session_id: "test".into(),
                desktop_entry_id: None, uid: unsafe { libc::geteuid() } },
            anchor_unit: format!("rsdm-session-{GENERATION}.service"), compositor_unit: None,
            compositor_invocation: Vec::new(), coordinator_unit: None, provider: "managed".into(), owns_targets: false,
            phase: rsdm_core::domain::SessionPhase::Running, disabled: false, config_path: None,
            exported_environment: Vec::new(), previous_environment: Vec::new(), shutdown_deadline_usec: None, shutdown_transaction: None,
        }).unwrap();
        let request = LaunchRequest {
            generation: GENERATION.into(), argv: vec![std::env::current_exe().unwrap().to_string_lossy().into_owned()],
            environment: Vec::new(), directory: directory.to_string_lossy().into_owned(),
            timeout_secs: 10, on_timeout: "force".into(), method: "auto".into(), quit_command: Vec::new(),
        };
        let provider = Provider { kind: crate::provider::ProviderKind::Managed,
            native_unit: None, logout_command: Vec::new() };
        let app = register(&runtime, &provider, &request).await.unwrap();
        let state = Arc::new(State { unit: app.unit.clone(), mode, exists: AtomicBool::new(false),
            starts: AtomicUsize::new(0), invocation_reads: AtomicUsize::new(0), unrefs: AtomicUsize::new(0),
            dependencies: Mutex::new(Vec::new()), compositor_exists: AtomicBool::new(true),
            compositor_invocation: Mutex::new(vec![1; 16]), compositor_active: Mutex::new("active".into()),
            compositor_query_failed: AtomicBool::new(false) });
        let shared = Arc::clone(&state);
        let (server_socket, client_socket) = tokio::net::UnixStream::pair().unwrap();
        let server = tokio::spawn(async move {
            zbus::connection::Builder::unix_stream(server_socket.into_std().unwrap()).p2p().server(zbus::Guid::generate())
                .serve_at("/org/freedesktop/systemd1", Manager(Arc::clone(&shared)))
                .serve_at("/compositor", Compositor(Arc::clone(&shared)))
                .serve_at("/unit", Unit(Arc::clone(&shared)))
                .serve_at("/unit", Service(shared)).build().await.unwrap()
        });
        let connection = zbus::connection::Builder::unix_stream(client_socket.into_std().unwrap()).p2p().build().await.unwrap();
        Self { manager: UserManager::with_connection(connection), runtime, request, app, state,
            _server: server.await.unwrap(), directory }
    }

    pub(super) async fn launch(&self) -> Result<String, SessionError> {
        launch(&self.manager, &self.runtime, &format!("rsdm-session-{GENERATION}.service"), &self.request, self.app.clone(), None).await
    }

    pub(super) fn missing_command(&mut self) {
        self.request.argv = vec![self.directory.join("missing").to_string_lossy().into_owned()];
    }
}

impl Drop for Fixture {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.directory); }
}
