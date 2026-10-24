//! The compositor invocation, rather than its wrapper, owns the session result.

use std::sync::{Mutex, atomic::{AtomicBool, AtomicUsize, Ordering}};

use super::*;

const GENERATION: &str = "0123456789abcdef0123456789abcdef";

struct State {
    active: AtomicBool,
    invocation: Mutex<Vec<u8>>,
    result: Mutex<String>,
    code: i32,
    status: i32,
    replace_on_status: AtomicBool,
    references: AtomicUsize,
}

struct Manager(Arc<State>);

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Manager")]
impl Manager {
    fn get_unit(&self, _unit: &str) -> zbus::OwnedObjectPath {
        zbus::OwnedObjectPath::try_from("/compositor").unwrap()
    }

    fn ref_unit(&self, _unit: &str) { self.0.references.fetch_add(1, Ordering::SeqCst); }
}

struct Unit(Arc<State>);

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Unit")]
impl Unit {
    #[zbus(property)]
    fn active_state(&self) -> &str { if self.0.active.load(Ordering::SeqCst) { "active" } else { "inactive" } }

    #[zbus(property, name = "InvocationID")]
    fn invocation_id(&self) -> Vec<u8> { self.0.invocation.lock().unwrap().clone() }

    #[zbus(property)]
    fn job(&self) -> (u32, zbus::OwnedObjectPath) { (0, zbus::OwnedObjectPath::try_from("/").unwrap()) }
}

struct Service(Arc<State>);

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Service")]
impl Service {
    #[zbus(property, name = "MainPID")]
    fn main_pid(&self) -> u32 { 0 }

    #[zbus(property)]
    fn environment(&self) -> Vec<String> { vec![format!("RSDM_SESSION_GENERATION={GENERATION}")] }

    #[zbus(property)]
    fn result(&self) -> String { self.0.result.lock().unwrap().clone() }

    #[zbus(property)]
    fn exec_main_code(&self) -> i32 { self.0.code }

    #[zbus(property)]
    fn exec_main_status(&self) -> i32 {
        if self.0.replace_on_status.load(Ordering::SeqCst) { *self.0.invocation.lock().unwrap() = vec![2; 16]; }
        self.0.status
    }
}

struct Fixture {
    state: Arc<State>,
    manager: UserManager,
    _server: zbus::Connection,
}

impl Fixture {
    async fn new(result: &str, code: i32, status: i32) -> Self {
        let state = Arc::new(State { active: AtomicBool::new(true), invocation: Mutex::new(vec![1; 16]),
            result: Mutex::new(result.into()), code, status, replace_on_status: AtomicBool::new(false), references: AtomicUsize::new(0) });
        let (client, server) = tokio::net::UnixStream::pair().unwrap();
        let guid = zbus::Guid::generate();
        let (client, server) = tokio::join!(
            zbus::connection::Builder::unix_stream(client.into_std().unwrap()).p2p().build(),
            zbus::connection::Builder::unix_stream(server.into_std().unwrap()).server(guid).p2p()
                .serve_at("/org/freedesktop/systemd1", Manager(state.clone()))
                .serve_at("/compositor", Unit(state.clone()))
                .serve_at("/compositor", Service(state.clone())).build(),
        );
        Self { state, manager: UserManager::with_connection(client.unwrap()), _server: server.unwrap() }
    }

    async fn process(&self, kind: ProviderKind) -> SessionProcess {
        let provider = Provider { kind, native_unit: Some("compositor.service".into()), logout_command: Vec::new() };
        let mut process = SessionProcess::launch(&["/bin/sh".into(), "-c".into(), "exit 0".into()],
            &[("RSDM_SESSION_GENERATION".into(), GENERATION.into())], Path::new("/"), &provider, "compositor.service".into()).unwrap();
        process.booting = false;
        if let Some(child) = &process.child { child.lock().await.wait().await.unwrap(); }
        else { process.invocation = vec![1; 16]; }
        assert_eq!(process.exited(&self.manager).await.unwrap(), None);
        self.state.active.store(false, Ordering::SeqCst);
        process
    }
}

#[tokio::test]
async fn native_compositor_crash_survives_a_successful_launcher() {
    let fixture = Fixture::new("core-dump", libc::CLD_DUMPED, libc::SIGABRT).await;
    let mut process = fixture.process(ProviderKind::Niri).await;
    assert_eq!(process.exited(&fixture.manager).await.unwrap(), Some(134));
}

#[tokio::test]
async fn service_failure_cannot_be_hidden_by_a_zero_main_status() {
    let fixture = Fixture::new("timeout", libc::CLD_EXITED, 0).await;
    let mut process = fixture.process(ProviderKind::Managed).await;
    assert_eq!(process.exited(&fixture.manager).await.unwrap(), Some(1));
}

#[tokio::test]
async fn a_replaced_invocation_cannot_supply_the_compositor_exit_status() {
    let fixture = Fixture::new("success", libc::CLD_EXITED, 0).await;
    let mut process = fixture.process(ProviderKind::Managed).await;
    *fixture.state.invocation.lock().unwrap() = vec![2; 16];
    assert!(process.exited(&fixture.manager).await.is_err());
}

#[tokio::test]
async fn invocation_is_rechecked_after_reading_the_service_result() {
    let fixture = Fixture::new("success", libc::CLD_EXITED, 0).await;
    let mut process = fixture.process(ProviderKind::Managed).await;
    fixture.state.replace_on_status.store(true, Ordering::SeqCst);
    assert!(process.exited(&fixture.manager).await.is_err());
}

#[tokio::test]
async fn clean_native_exit_remains_successful() {
    let fixture = Fixture::new("success", libc::CLD_EXITED, 0).await;
    let mut process = fixture.process(ProviderKind::Niri).await;
    assert_eq!(process.exited(&fixture.manager).await.unwrap(), Some(0));
}

#[tokio::test]
async fn native_compositor_is_referenced_only_once_while_its_status_is_observed() {
    let fixture = Fixture::new("success", libc::CLD_EXITED, 0).await;
    let mut process = fixture.process(ProviderKind::Niri).await;
    fixture.state.active.store(true, Ordering::SeqCst);
    assert_eq!(process.exited(&fixture.manager).await.unwrap(), None);
    assert_eq!(fixture.state.references.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn confirmed_compositor_crash_is_not_overwritten_by_a_later_invocation() {
    let fixture = Fixture::new("core-dump", libc::CLD_DUMPED, libc::SIGABRT).await;
    let mut process = fixture.process(ProviderKind::Niri).await;
    assert_eq!(process.exited(&fixture.manager).await.unwrap(), Some(134));
    *fixture.state.invocation.lock().unwrap() = vec![2; 16];
    assert_eq!(process.exited(&fixture.manager).await.unwrap(), Some(134));
}

#[tokio::test]
async fn detached_native_observation_preserves_the_compositor_crash() {
    let fixture = Fixture::new("core-dump", libc::CLD_DUMPED, libc::SIGABRT).await;
    let process = fixture.process(ProviderKind::Niri).await;
    let mut record = crate::helper_start_fixture::record();
    record.compositor_invocation = vec![1; 16];
    let provider = Provider { kind: ProviderKind::Niri, native_unit: Some("compositor.service".into()), logout_command: Vec::new() };
    let code = tokio::time::timeout(Duration::from_secs(1),
        crate::session_wait::wait(process, &record, &provider, fixture.manager.connection())).await.unwrap().unwrap();
    assert_eq!(code, 134);
}
