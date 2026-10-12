//! Independent systemd and compositor peers for queued window-close tests.

use std::sync::{Arc, Mutex, atomic::{AtomicBool, AtomicU32, AtomicU8, AtomicUsize, Ordering}};

use zbus::{Connection, zvariant::OwnedObjectPath};

use crate::{bus::UserManager, runtime::{AppRecord, SessionRecord}};

pub(super) struct State {
    pub app_invocation: AtomicU8,
    pub compositor_invocation: AtomicU8,
    pub main_pid: AtomicU32,
    pub control_pid: AtomicU32,
    pub present: AtomicBool,
    pub group: Mutex<String>,
    pub reads: AtomicUsize,
    pub scope_invocation: AtomicU8,
    pub scope_present: AtomicBool,
    pub scope_pid: AtomicU32,
    pub scope_foreign_pid: AtomicU32,
    pub scope_group: Mutex<String>,
    pub scope_transient: AtomicBool,
    pub scope_reads: AtomicUsize,
    pub scope_missing_at: AtomicUsize,
}

struct Manager(Arc<State>);

#[derive(Debug, zbus_macros::DBusError)]
#[zbus(prefix = "org.freedesktop.systemd1")]
enum ManagerError { NoSuchUnit(String) }

impl Manager {
    fn check_scope(&self, unit: &str) -> Result<(), ManagerError> {
        if unit.ends_with(".scope") && self.0.scope_reads.fetch_add(1, Ordering::SeqCst) + 1 >= self.0.scope_missing_at.load(Ordering::SeqCst) {
            return Err(ManagerError::NoSuchUnit("scope was collected".into()));
        }
        Ok(())
    }
}

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Manager")]
impl Manager {
    fn get_unit(&self, unit: &str) -> Result<OwnedObjectPath, ManagerError> {
        self.0.reads.fetch_add(1, Ordering::SeqCst);
        self.check_scope(unit)?;
        let path = if unit == "niri.service" { "/compositor" } else if unit.ends_with(".scope") { "/scope" } else { "/app" };
        Ok(OwnedObjectPath::try_from(path).unwrap())
    }

    fn get_unit_processes(&self, unit: &str) -> Result<Vec<(String, u32, String)>, ManagerError> {
        self.check_scope(unit)?;
        let scope = unit.ends_with(".scope");
        let present = if scope { &self.0.scope_present } else { &self.0.present };
        let pid = if scope { self.0.scope_pid.load(Ordering::SeqCst) } else { self.0.main_pid.load(Ordering::SeqCst) };
        let mut pids = if present.load(Ordering::SeqCst) { vec![("app".into(), pid, "app".into())] } else { Vec::new() };
        let foreign = self.0.scope_foreign_pid.load(Ordering::SeqCst);
        if scope && foreign != 0 { pids.push(("foreign".into(), foreign, "foreign".into())); }
        Ok(pids)
    }

    #[zbus(name = "GetUnitByPID")]
    fn get_unit_by_pid(&self, pid: u32) -> OwnedObjectPath {
        assert_eq!(pid, self.0.main_pid.load(Ordering::SeqCst));
        OwnedObjectPath::try_from(if self.0.scope_present.load(Ordering::SeqCst) { "/scope" } else { "/app" }).unwrap()
    }
}

struct Unit { state: Arc<State>, role: &'static str }

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Unit")]
impl Unit {
    #[zbus(property, name = "InvocationID")]
    fn invocation_id(&self) -> Vec<u8> {
        let invocation = match self.role { "compositor" => &self.state.compositor_invocation, "scope" => &self.state.scope_invocation, _ => &self.state.app_invocation };
        vec![invocation.load(Ordering::SeqCst); 16]
    }

    #[zbus(property)]
    fn id(&self) -> String {
        match self.role { "scope" => "app-test.scope".into(), "compositor" => "niri.service".into(), _ => crate::helper_start_fixture::app().unit }
    }

    #[zbus(property)]
    fn transient(&self) -> bool { self.role != "scope" || self.state.scope_transient.load(Ordering::SeqCst) }

    #[zbus(property)]
    fn active_state(&self) -> &str {
        let inactive = self.role == "scope" && !self.state.scope_present.load(Ordering::SeqCst)
            || self.role == "app" && self.state.app_invocation.load(Ordering::SeqCst) == 0;
        if inactive { "inactive" } else { "active" }
    }
}

struct Scope(Arc<State>);

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Scope")]
impl Scope {
    #[zbus(property)]
    fn control_group(&self) -> String { self.0.scope_group.lock().unwrap().clone() }
}

struct Service(Arc<State>);

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Service")]
impl Service {
    #[zbus(property)]
    fn environment(&self) -> Vec<String> { vec![format!("RSDM_SESSION_GENERATION={}", crate::helper_start_fixture::GENERATION)] }
    #[zbus(property, name = "MainPID")]
    fn main_pid(&self) -> u32 { self.0.main_pid.load(Ordering::SeqCst) }
    #[zbus(property, name = "ControlPID")]
    fn control_pid(&self) -> u32 { self.0.control_pid.load(Ordering::SeqCst) }
    #[zbus(property)]
    fn control_group(&self) -> String { self.0.group.lock().unwrap().clone() }
}

pub(super) struct Fixture {
    pub manager: UserManager,
    pub state: Arc<State>,
    pub record: SessionRecord,
    pub app: AppRecord,
    _server: Connection,
}

impl Fixture {
    pub async fn new() -> Self {
        let membership = std::fs::read_to_string("/proc/self/cgroup").unwrap();
        let group = membership.lines().find_map(|line| line.strip_prefix("0::")).unwrap_or("/").to_owned();
        let state = Arc::new(State { app_invocation: AtomicU8::new(1), compositor_invocation: AtomicU8::new(1),
            main_pid: AtomicU32::new(std::process::id()), control_pid: AtomicU32::new(0), present: AtomicBool::new(true),
            group: Mutex::new(group.clone()), reads: AtomicUsize::new(0), scope_invocation: AtomicU8::new(1),
            scope_present: AtomicBool::new(false), scope_foreign_pid: AtomicU32::new(0), scope_group: Mutex::new(group),
            scope_pid: AtomicU32::new(std::process::id()),
            scope_transient: AtomicBool::new(true), scope_reads: AtomicUsize::new(0), scope_missing_at: AtomicUsize::new(usize::MAX) });
        let shared = state.clone();
        let (server, client) = tokio::net::UnixStream::pair().unwrap();
        let server = tokio::spawn(async move {
            zbus::connection::Builder::unix_stream(server).p2p().server(zbus::Guid::generate()).unwrap()
                .serve_at("/org/freedesktop/systemd1", Manager(shared.clone())).unwrap()
                .serve_at("/compositor", Unit { state: shared.clone(), role: "compositor" }).unwrap()
                .serve_at("/compositor", Service(shared.clone())).unwrap()
                .serve_at("/app", Unit { state: shared.clone(), role: "app" }).unwrap()
                .serve_at("/scope", Unit { state: shared.clone(), role: "scope" }).unwrap()
                .serve_at("/scope", Scope(shared.clone())).unwrap()
                .serve_at("/app", Service(shared)).unwrap().build().await.unwrap()
        });
        let connection = zbus::connection::Builder::unix_stream(client).p2p().build().await.unwrap();
        let mut record = crate::helper_start_fixture::record();
        record.compositor_unit = Some("niri.service".into());
        record.compositor_invocation = vec![1; 16];
        Self { manager: UserManager::with_connection(connection), state, record,
            app: crate::helper_start_fixture::app(), _server: server.await.unwrap() }
    }
}
