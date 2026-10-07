//! References are restored only for the same owned transient invocation.

use std::{os::unix::net::UnixStream, sync::atomic::{AtomicUsize, Ordering}, thread};

use super::*;

struct UnitState {
    refs: AtomicUsize,
    unrefs: AtomicUsize,
    generation: &'static str,
    invocation: Vec<u8>,
    transient: bool,
    replaced: bool,
    pause: Option<Pause>,
}

struct Pause {
    entered: std::sync::mpsc::Sender<()>,
    resume: async_channel::Receiver<()>,
}

struct Manager(Arc<UnitState>);

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Manager")]
impl Manager {
    #[zbus(property)]
    fn version(&self) -> &str { "260" }
    fn subscribe(&self) {}
    fn get_unit(&self, _name: &str) -> OwnedObjectPath { OwnedObjectPath::try_from("/unit").unwrap() }
    async fn ref_unit(&self, _name: &str) {
        self.0.refs.fetch_add(1, Ordering::SeqCst);
        if let Some(pause) = &self.0.pause {
            let _ = pause.entered.send(());
            let _ = pause.resume.recv().await;
        }
    }
    fn unref_unit(&self, _name: &str) { self.0.unrefs.fetch_add(1, Ordering::SeqCst); }
}

struct Unit(Arc<UnitState>);

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Unit")]
impl Unit {
    #[zbus(property)]
    fn id(&self) -> &str { "example.service" }
    #[zbus(property)]
    fn transient(&self) -> bool { self.0.transient }
    #[zbus(property, name = "InvocationID")]
    fn invocation_id(&self) -> Vec<u8> {
        if self.0.replaced && self.0.refs.load(Ordering::SeqCst) > 0 { return vec![2; 16]; }
        self.0.invocation.clone()
    }
}

struct Service(Arc<UnitState>);

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Service")]
impl Service {
    #[zbus(property)]
    fn environment(&self) -> Vec<String> { vec![format!("RSDM_SESSION_GENERATION={}", self.0.generation)] }
}

fn fixture(state: UnitState) -> (Connection, Connection, Arc<UnitState>) {
    let state = Arc::new(state);
    let shared = state.clone();
    let (server_socket, client_socket) = UnixStream::pair().unwrap();
    let server = thread::spawn(move || async_io::block_on(async {
        zbus::connection::Builder::unix_stream(server_socket).p2p().server(zbus::Guid::generate()).unwrap()
            .serve_at(MANAGER_PATH, Manager(shared.clone())).unwrap()
            .serve_at("/unit", Unit(shared.clone())).unwrap()
            .serve_at("/unit", Service(shared)).unwrap().build().await.unwrap()
    }));
    let client = async_io::block_on(async {
        zbus::connection::Builder::unix_stream(client_socket).p2p().build().await.unwrap()
    });
    (client, server.join().unwrap(), state)
}

fn state() -> UnitState {
    UnitState { refs: AtomicUsize::new(0), unrefs: AtomicUsize::new(0), generation: "ours",
        invocation: vec![1; 16], transient: true, replaced: false, pause: None }
}

#[test]
fn reconnect_restores_a_reference_to_the_original_owned_invocation() {
    let (connection, _server, state) = fixture(state());
    let reference = Reference { generation: "ours".into(), invocation: Some(vec![1; 16]) };
    async_io::block_on(restore_reference(&connection, "example.service", &reference)).unwrap();
    assert_eq!(state.refs.load(Ordering::SeqCst), 1);
    assert_eq!(state.unrefs.load(Ordering::SeqCst), 0);
}

#[test]
fn foreign_replaced_unstarted_and_non_transient_units_are_never_referenced() {
    for case in 0..4 {
        let mut initial = state();
        match case {
            0 => initial.generation = "foreign",
            1 => initial.invocation = vec![2; 16],
            2 => initial.invocation = vec![0; 16],
            _ => initial.transient = false,
        }
        let (connection, _server, state) = fixture(initial);
        let reference = Reference { generation: "ours".into(), invocation: Some(vec![1; 16]) };
        async_io::block_on(restore_reference(&connection, "example.service", &reference)).unwrap();
        assert_eq!(state.refs.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn an_invocation_replaced_during_ref_unit_is_immediately_unreferenced() {
    let mut initial = state();
    initial.replaced = true;
    let (connection, _server, state) = fixture(initial);
    let reference = Reference { generation: "ours".into(), invocation: Some(vec![1; 16]) };
    async_io::block_on(restore_reference(&connection, "example.service", &reference)).unwrap();
    assert_eq!(state.refs.load(Ordering::SeqCst), 1);
    assert_eq!(state.unrefs.load(Ordering::SeqCst), 1);
}

#[test]
fn cloned_workers_keep_the_original_reference_identity_and_share_release() {
    let (connection, _server, _) = fixture(state());
    let owner = Transport::new(connection);
    let worker = owner.clone();
    owner.track("example.service", "ours".into());
    worker.remember("example.service", &[1; 16]);
    owner.remember("example.service", &[2; 16]);
    assert_eq!(worker.0.lock().unwrap().references["example.service"].invocation, Some(vec![1; 16]));
    worker.forget("example.service");
    assert!(owner.0.lock().unwrap().references.is_empty());
}

#[test]
fn manager_reexec_does_not_require_replacing_a_live_bus_connection() {
    assert!(!disconnected(&zbus::fdo::Error::NoReply("manager reexec".into()).into()));
    assert!(disconnected(&zbus::Error::from(std::io::Error::from(std::io::ErrorKind::BrokenPipe))));
}

#[test]
fn slow_restoration_does_not_lock_state_or_publish_a_forgotten_reference() {
    let (original, _original_server, _) = fixture(state());
    let owner = Transport::new(original);
    owner.track("example.service", "ours".into());
    owner.remember("example.service", &[1; 16]);
    let (entered, ready) = std::sync::mpsc::channel();
    let (resume, resumed) = async_channel::bounded(1);
    let mut initial = state();
    initial.pause = Some(Pause { entered, resume: resumed });
    let (candidate, _server, _) = fixture(initial);
    let restorer = owner.clone();
    let worker = thread::spawn(move || async_io::block_on(restorer.restore_candidate(&candidate, 0)));
    ready.recv_timeout(Duration::from_secs(2)).unwrap();

    let reader = owner.clone();
    let (accessed, access) = std::sync::mpsc::channel();
    let reader = thread::spawn(move || {
        let _ = reader.connection();
        reader.forget("example.service");
        let _ = accessed.send(reader.epoch());
    });
    let responsive = access.recv_timeout(Duration::from_secs(1));
    resume.try_send(()).unwrap();
    let published = worker.join().unwrap().unwrap();
    reader.join().unwrap();
    assert_eq!(responsive.unwrap(), 0);
    assert!(!published);
    assert_eq!(owner.epoch(), 0);
    assert!(owner.0.lock().unwrap().references.is_empty());
}

#[test]
fn maintenance_guard_is_shared_bounded_and_released_on_drop() {
    let (connection, _server, _) = fixture(state());
    let owner = Transport::new(connection);
    let worker = owner.clone();
    let guard = async_io::block_on(owner.acquire_connection_guard(Instant::now() + Duration::from_secs(1))).unwrap();
    let blocked = async_io::block_on(worker.acquire_connection_guard(Instant::now() + Duration::from_millis(20)));
    assert!(matches!(blocked, Err(error) if retryable_error(&error)));
    assert_eq!(worker.epoch(), 0);
    drop(guard);
    assert!(async_io::block_on(worker.acquire_connection_guard(Instant::now() + Duration::from_secs(1))).is_ok());
}
