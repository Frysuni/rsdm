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
}

struct Manager(Arc<State>);

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Manager")]
impl Manager {
    fn get_unit(&self, unit: &str) -> OwnedObjectPath {
        self.0.reads.fetch_add(1, Ordering::SeqCst);
        OwnedObjectPath::try_from(if unit == "niri.service" { "/compositor" } else { "/app" }).unwrap()
    }

    fn get_unit_processes(&self, _unit: &str) -> Vec<(String, u32, String)> {
        if self.0.present.load(Ordering::SeqCst) { vec![("app".into(), std::process::id(), "app".into())] } else { Vec::new() }
    }
}

struct Unit { state: Arc<State>, compositor: bool }

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Unit")]
impl Unit {
    #[zbus(property, name = "InvocationID")]
    fn invocation_id(&self) -> Vec<u8> {
        let invocation = if self.compositor { &self.state.compositor_invocation } else { &self.state.app_invocation };
        vec![invocation.load(Ordering::SeqCst); 16]
    }
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
            group: Mutex::new(group), reads: AtomicUsize::new(0) });
        let shared = state.clone();
        let (server, client) = tokio::net::UnixStream::pair().unwrap();
        let server = tokio::spawn(async move {
            zbus::connection::Builder::unix_stream(server).p2p().server(zbus::Guid::generate()).unwrap()
                .serve_at("/org/freedesktop/systemd1", Manager(shared.clone())).unwrap()
                .serve_at("/compositor", Unit { state: shared.clone(), compositor: true }).unwrap()
                .serve_at("/compositor", Service(shared.clone())).unwrap()
                .serve_at("/app", Unit { state: shared.clone(), compositor: false }).unwrap()
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
