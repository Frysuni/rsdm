//! Real shutdown messages are checked with isolated libSM client and server processes.

use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf, process::Stdio, sync::atomic::{AtomicU32, AtomicUsize, Ordering}};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use zbus::zvariant::OwnedObjectPath;

use super::*;
use crate::{app_stop::ShutdownControl, app_dispatch::State as DispatchState};

#[path = "transaction_client.rs"]
mod client;

const TEST: &str = "xsmp::handle::transaction_tests::cancellation_reaches_worker_before_shutdown_messages";
const ROLE: &str = "RSDM_XSMP_TRANSACTION_TEST";

struct State { control: Arc<ShutdownControl>, pid: AtomicU32, checks: AtomicUsize, cancel_at: AtomicUsize }
struct Manager(Arc<State>);

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Manager")]
impl Manager {
    fn get_unit(&self, _unit: &str) -> OwnedObjectPath { OwnedObjectPath::try_from("/app").unwrap() }

    #[zbus(name = "GetUnitByPID")]
    fn get_unit_by_pid(&self, pid: u32) -> OwnedObjectPath {
        assert_eq!(pid, self.0.pid.load(Ordering::SeqCst));
        self.get_unit("")
    }

    fn get_unit_processes(&self, _unit: &str) -> Vec<(String, u32, String)> {
        let check = self.0.checks.fetch_add(1, Ordering::SeqCst) + 1;
        if check == self.0.cancel_at.load(Ordering::SeqCst) { self.0.control.cancelled.store(true, Ordering::SeqCst); }
        vec![("/app".into(), self.0.pid.load(Ordering::SeqCst), "xsmp-client".into())]
    }
}

struct Unit;

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Unit")]
impl Unit {
    #[zbus(property)]
    fn id(&self) -> String { crate::helper_start_fixture::app().unit }
    #[zbus(property, name = "InvocationID")]
    fn invocation_id(&self) -> Vec<u8> { vec![1; 16] }
}

struct Service;

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Service")]
impl Service {
    #[zbus(property, name = "ControlPID")]
    fn control_pid(&self) -> u32 { 0 }
}

struct Fixture { directory: PathBuf, runtime: Runtime, manager: UserManager, state: Arc<State>, _server: zbus::Connection }

impl Fixture {
    async fn new() -> Self {
        let directory = std::env::temp_dir().join(format!("rsdm-xsmp-cancel-{}", zbus::Guid::generate()));
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        let runtime = Runtime::create_at(&directory, crate::helper_start_fixture::GENERATION).unwrap();
        let mut record = crate::helper_start_fixture::record();
        // SAFETY: geteuid has no preconditions.
        record.identity.uid = unsafe { libc::geteuid() };
        runtime.save_session(&record).unwrap();
        let mut app = crate::helper_start_fixture::app();
        app.policy.quit_command.clear();
        app.policy.method = rsdm_core::domain::ShutdownMethod::Xsmp;
        runtime.save_app(&app).unwrap();
        let state = Arc::new(State { control: Arc::default(), pid: AtomicU32::new(0),
            checks: AtomicUsize::new(0), cancel_at: AtomicUsize::new(usize::MAX) });
        let shared = state.clone();
        let (server, connection) = tokio::net::UnixStream::pair().unwrap();
        let server = tokio::spawn(async move {
            zbus::connection::Builder::unix_stream(server).p2p().server(zbus::Guid::generate()).unwrap()
                .serve_at("/org/freedesktop/systemd1", Manager(shared)).unwrap()
                .serve_at("/app", Unit).unwrap().serve_at("/app", Service).unwrap().build().await.unwrap()
        });
        let connection = zbus::connection::Builder::unix_stream(connection).p2p().build().await.unwrap();
        Self { directory, runtime, manager: UserManager::with_connection(connection), state, _server: server.await.unwrap() }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.directory); }
}

#[tokio::test]
async fn cancellation_reaches_worker_before_shutdown_messages() {
    let mode = std::env::var(ROLE).unwrap_or_default();
    if mode == "client" {
        client::run(&std::env::var("RSDM_XSMP_CLIENT_MODE").unwrap());
        return;
    }
    if !mode.is_empty() { worker(&mode).await; return; }
    let mut failed = Vec::new();
    for mode in ["already-cancelled", "ownership-check", "forced", "after-send", "phase2", "interact"] {
        let mut child = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST, "--nocapture"]).env(ROLE, mode).kill_on_drop(true).spawn().unwrap();
        if !tokio::time::timeout(Duration::from_secs(10), child.wait()).await.unwrap().unwrap().success() { failed.push(mode); }
    }
    assert!(failed.is_empty(), "failed cases: {failed:?}");
}

async fn worker(mode: &str) {
    let fixture = Fixture::new().await;
    let provider = Provider { kind: ProviderKind::Managed, native_unit: None, logout_command: Vec::new() };
    let mut handle = Handle::start(&fixture.manager, &fixture.runtime, &provider).await.unwrap();
    let (mut child, mut output) = start_client(&fixture, &handle, mode).await;
    fixture.state.checks.store(0, Ordering::SeqCst);
    if mode == "already-cancelled" { fixture.state.control.cancelled.store(true, Ordering::SeqCst); }
    else if mode == "ownership-check" || mode == "forced" { fixture.state.cancel_at.store(3, Ordering::SeqCst); }
    if mode == "forced" { fixture.state.control.force(0); }

    let unit = crate::helper_start_fixture::app().unit;
    let (reply, mut received) = mpsc::channel(1);
    handle.prepare(vec![unit.clone()], String::new(), mode != "forced", Some(fixture.state.control.clone()), Reply::for_test(reply));
    received.recv().await.unwrap().unwrap();
    if matches!(mode, "after-send" | "phase2" | "interact") {
        wait_marker(&mut output, "xsmp-client-saving").await;
        fixture.state.control.cancelled.store(true, Ordering::SeqCst);
        child.stdin.take().unwrap().write_all(&[1]).await.unwrap();
        wait_marker(&mut output, "xsmp-client-cancelled").await;
    }
    if mode != "forced" { handle.shutdown(&Deadline::default()).await.unwrap(); }
    assert!(tokio::time::timeout(Duration::from_secs(3), child.wait()).await.unwrap().unwrap().success(), "libSM client observed an unexpected shutdown message");
    handle.shutdown(&Deadline::default()).await.unwrap();
    let dispatch = fixture.runtime.app(&unit).unwrap().dispatch;
    if mode == "already-cancelled" { assert!(dispatch.is_none()); }
    else {
        assert_eq!(fixture.state.checks.load(Ordering::SeqCst), 3);
        assert!(fixture.state.control.cancelled.load(Ordering::SeqCst));
        assert_eq!(dispatch.unwrap().state, if mode == "ownership-check" { DispatchState::NotSent } else { DispatchState::Sent });
    }
}

async fn start_client(fixture: &Fixture, handle: &Handle, mode: &str) -> (tokio::process::Child, tokio::io::Lines<BufReader<tokio::process::ChildStdout>>) {
    let mut child = tokio::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", TEST, "--nocapture"]).env(ROLE, "client")
        .env("RSDM_XSMP_CLIENT_MODE", mode).envs(handle.environment.clone())
        .stdin(Stdio::piped()).stdout(Stdio::piped()).kill_on_drop(true).spawn().unwrap();
    fixture.state.pid.store(child.id().unwrap(), Ordering::SeqCst);
    let mut output = BufReader::new(child.stdout.take().unwrap()).lines();
    wait_marker(&mut output, "xsmp-client-ready").await;
    (child, output)
}

async fn wait_marker(output: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>, marker: &str) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while let Some(line) = output.next_line().await.unwrap() {
            if line == marker { return; }
        }
        panic!("libSM client exited before {marker}");
    }).await.unwrap();
}
