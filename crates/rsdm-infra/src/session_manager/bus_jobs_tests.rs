//! Lost replies and signals must not cause a second systemd mutation.

use std::{os::unix::net::UnixStream, sync::{Arc, atomic::{AtomicBool, AtomicUsize, Ordering}}, thread};

use super::*;

#[derive(Debug, zbus_macros::DBusError)]
#[zbus(prefix = "org.freedesktop.systemd1")]
enum ManagerError {
    NoSuchUnit(String),
    #[zbus(error)]
    Bus(zbus::Error),
}

struct State {
    exists: AtomicBool,
    calls: AtomicUsize,
    lost_reply: bool,
    active: bool,
    status: i32,
    generation: &'static str,
}

struct Manager(Arc<State>);

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Manager")]
impl Manager {
    #[zbus(property)]
    fn version(&self) -> &str { "260" }

    fn subscribe(&self) {}

    fn get_unit(&self, _name: &str) -> Result<OwnedObjectPath, ManagerError> {
        if !self.0.exists.load(Ordering::SeqCst) {
            return Err(ManagerError::NoSuchUnit("not loaded".into()));
        }
        Ok(OwnedObjectPath::try_from("/unit").unwrap())
    }

    fn start_transient_unit(
        &self, _unit: &str, _mode: &str,
        _properties: Vec<(String, OwnedValue)>, _auxiliary: Vec<(String, Vec<(String, OwnedValue)>)>,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        self.0.calls.fetch_add(1, Ordering::SeqCst);
        self.0.exists.store(true, Ordering::SeqCst);
        self.reply()
    }

    fn stop_unit(&self, _unit: &str, _mode: &str) -> zbus::fdo::Result<OwnedObjectPath> {
        self.0.calls.fetch_add(1, Ordering::SeqCst);
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
    fn reply(&self) -> zbus::fdo::Result<OwnedObjectPath> {
        if self.0.lost_reply { return Err(zbus::fdo::Error::NoReply("reply lost after accepting request".into())); }
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

fn fixture(state: State) -> (UserManager, Connection, Arc<State>) {
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

fn state(lost_reply: bool, active: bool, status: i32) -> State {
    State { exists: AtomicBool::new(false), calls: AtomicUsize::new(0), lost_reply, active, status, generation: "ours" }
}

#[test]
fn accepted_start_survives_lost_reply_or_job_signal_without_replay() {
    for lost_reply in [true, false] {
        let (manager, _server, state) = fixture(state(lost_reply, true, 0));
        let properties = vec![("Environment", Value::new(vec!["RSDM_SESSION_GENERATION=ours".to_string()]))];
        async_io::block_on(start(&manager, "example.service", &properties, Duration::from_millis(20))).unwrap();
        assert_eq!(state.calls.load(Ordering::SeqCst), 1);
    }
}

#[test]
fn lost_reply_accepts_only_a_successfully_exited_service() {
    for status in [0, 1] {
        let (manager, _server, state) = fixture(state(true, false, status));
        let result = async_io::block_on(start(&manager, "example.service", &Vec::new(), Duration::from_millis(50)));
        assert_eq!(result.is_ok(), status == 0);
        assert_eq!(state.calls.load(Ordering::SeqCst), 1);
    }
}

#[test]
fn an_existing_or_foreign_unit_cannot_confirm_a_lost_start_reply() {
    for existing in [true, false] {
        let mut initial = state(true, true, 0);
        initial.exists.store(existing, Ordering::SeqCst);
        if !existing { initial.generation = "foreign"; }
        let (manager, _server, state) = fixture(initial);
        let properties = vec![("Environment", Value::new(vec!["RSDM_SESSION_GENERATION=ours".to_string()]))];
        assert!(async_io::block_on(start(&manager, "example.service", &properties, Duration::from_millis(50))).is_err());
        assert_eq!(state.calls.load(Ordering::SeqCst), 1);
    }
}

#[test]
fn a_lost_stop_reply_requires_an_empty_stopped_unit() {
    for active in [false, true] {
        let initial = state(true, active, 0);
        initial.exists.store(true, Ordering::SeqCst);
        let (manager, _server, state) = fixture(initial);
        let result = async_io::block_on(stop(&manager, "example.service", Duration::from_millis(50)));
        assert_eq!(result.is_ok(), !active);
        assert_eq!(state.calls.load(Ordering::SeqCst), 1);
    }
}
