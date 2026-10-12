//! Cancellation during process discovery must prevent the claimed TERM.

use std::{fs, os::unix::{fs::PermissionsExt, process::ExitStatusExt}, path::PathBuf,
    process::{Child, Command}, sync::{Arc, atomic::{AtomicU32, AtomicUsize}}};

use zbus::{Connection, zvariant::OwnedObjectPath};

use super::*;

#[path = "app_stop_escalation_tests.rs"]
mod escalation;

#[derive(Clone, Copy)]
enum Snapshot { Observe, Cancel, CancelDuringSignal, EmptyDuringSignal, FailDuringSignal, FailAfterFirstBatch }

struct Manager {
    pid: u32, process: crate::processes::ProcessHandle, control: Arc<ShutdownControl>,
    snapshot: Snapshot, reads: AtomicUsize,
}

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Manager")]
impl Manager {
    fn get_unit(&self, _unit: &str) -> OwnedObjectPath { OwnedObjectPath::try_from("/app").unwrap() }

    fn get_unit_processes(&self, _unit: &str) -> zbus::fdo::Result<Vec<(String, u32, String)>> {
        let read = self.reads.fetch_add(1, Ordering::SeqCst);
        if !self.process.alive().unwrap() { return Ok(Vec::new()); }
        match self.snapshot {
            Snapshot::Cancel => self.control.cancelled.store(true, Ordering::SeqCst),
            Snapshot::CancelDuringSignal if read > 0 => self.control.cancelled.store(true, Ordering::SeqCst),
            Snapshot::EmptyDuringSignal if read > 0 => return Ok(Vec::new()),
            Snapshot::FailDuringSignal if read > 0 => return Err(zbus::fdo::Error::Failed("process inspection failed".into())),
            _ => {},
        }
        let process = ("/test".into(), self.pid, "application".into());
        if matches!(self.snapshot, Snapshot::FailAfterFirstBatch) { Ok(vec![process; 33]) } else { Ok(vec![process]) }
    }
}

struct Unit { snapshot: Snapshot, reads: AtomicUsize }

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Unit")]
impl Unit {
    #[zbus(property, name = "InvocationID")]
    fn invocation_id(&self) -> zbus::fdo::Result<Vec<u8>> {
        let read = self.reads.fetch_add(1, Ordering::SeqCst);
        if matches!(self.snapshot, Snapshot::FailAfterFirstBatch) && read >= 3 {
            return Err(zbus::fdo::Error::Failed("second batch inspection failed".into()));
        }
        Ok(vec![1; 16])
    }
}

struct Service { main: Arc<AtomicU32>, group: String }

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Service")]
impl Service {
    #[zbus(property, name = "MainPID")]
    fn main_pid(&self) -> u32 { self.main.load(Ordering::SeqCst) }

    #[zbus(property, name = "ControlPID")]
    fn control_pid(&self) -> u32 { 0 }

    #[zbus(property)]
    fn control_group(&self) -> &str { &self.group }
}

struct Fixture {
    directory: PathBuf,
    runtime: Runtime,
    manager: UserManager,
    _server: Connection,
    child: Child,
    main: Arc<AtomicU32>,
}

impl Fixture {
    async fn new(control: Arc<ShutdownControl>, snapshot: Snapshot) -> Self {
        let child = Command::new("sleep").arg("60").spawn().unwrap();
        let pid = child.id();
        let process = crate::processes::ProcessHandle::open(pid).unwrap().unwrap();
        let main = Arc::new(AtomicU32::new(pid));
        let membership = fs::read_to_string(format!("/proc/{pid}/cgroup")).unwrap();
        let group = membership.lines().find_map(|line| line.strip_prefix("0::")).unwrap().to_string();
        let directory = std::env::temp_dir().join(format!("rsdm-term-cancel-{}", zbus::Guid::generate()));
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        let runtime = Runtime::create_at(&directory, crate::helper_start_fixture::GENERATION).unwrap();
        let (server, client) = tokio::net::UnixStream::pair().unwrap();
        let service = Service { main: main.clone(), group };
        let server = tokio::spawn(async move {
            zbus::connection::Builder::unix_stream(server).p2p().server(zbus::Guid::generate()).unwrap()
                .serve_at("/org/freedesktop/systemd1", Manager { pid, process, control, snapshot, reads: AtomicUsize::new(0) }).unwrap()
                .serve_at("/app", Unit { snapshot, reads: AtomicUsize::new(0) }).unwrap()
                .serve_at("/app", service).unwrap().build().await.unwrap()
        });
        let client = zbus::connection::Builder::unix_stream(client).p2p().build().await.unwrap();
        Self { directory, runtime, manager: UserManager::with_connection(client), _server: server.await.unwrap(), child, main }
    }

    fn save_app(&self) -> AppRecord {
        let mut app = crate::helper_start_fixture::app();
        app.policy.quit_command.clear();
        app.policy.method = ShutdownMethod::Term;
        self.runtime.save_app(&app).unwrap();
        app
    }

    fn dispatch_state(&self, app: &AppRecord) -> crate::app_dispatch::State {
        self.runtime.app(&app.unit).unwrap().dispatch.unwrap().state
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.directory);
    }
}

#[tokio::test]
async fn lifetime_observation_excludes_only_the_recorded_kernel_process() {
    let control = Arc::new(ShutdownControl::default());
    let mut fixture = Fixture::new(control.clone(), Snapshot::Observe).await;
    let handle = crate::processes::ProcessHandle::open(fixture.child.id()).unwrap().unwrap();
    let Some(identity) = handle.id().unwrap() else { return; };
    let mut app = fixture.save_app();
    app.lifetime_process = Some(crate::app_lifetime::LifetimeProcess { pid: fixture.child.id(), pidfd_id: identity });
    assert!(crate::processes::app_processes(&fixture.manager, &app).await.unwrap().is_empty());
    assert!(!crate::processes::signal_app(&fixture.manager, &app, libc::SIGTERM, &control).await.unwrap());
    assert!(fixture.child.try_wait().unwrap().is_none());

    app.lifetime_process.as_mut().unwrap().pidfd_id = identity.wrapping_add(1);
    assert_eq!(crate::processes::app_processes(&fixture.manager, &app).await.unwrap(), vec![fixture.child.id()]);
}

#[tokio::test]
async fn cancellation_during_main_pid_verification_leaves_term_unsent() {
    let control = Arc::new(ShutdownControl::default());
    let mut fixture = Fixture::new(control.clone(), Snapshot::Cancel).await;
    let app = fixture.save_app();
    request_quit(&fixture.manager, &fixture.runtime, &app, &control).await.unwrap();
    assert!(control.cancelled());
    assert_eq!(fixture.dispatch_state(&app), crate::app_dispatch::State::NotSent);
    assert!(fixture.child.try_wait().unwrap().is_none());
}

#[tokio::test]
async fn cancellation_during_cgroup_snapshot_leaves_term_fallback_unsent() {
    let control = Arc::new(ShutdownControl::default());
    let mut fixture = Fixture::new(control.clone(), Snapshot::Cancel).await;
    fixture.main.store(0, Ordering::SeqCst);
    let app = fixture.save_app();
    request_quit(&fixture.manager, &fixture.runtime, &app, &control).await.unwrap();
    assert!(control.cancelled());
    assert_eq!(fixture.dispatch_state(&app), crate::app_dispatch::State::NotSent);
    assert!(fixture.child.try_wait().unwrap().is_none());
}

#[tokio::test]
async fn force_can_signal_after_cancellation_during_pid_verification() {
    let control = Arc::new(ShutdownControl::default());
    control.noncancelable.store(true, Ordering::SeqCst);
    let mut fixture = Fixture::new(control.clone(), Snapshot::Cancel).await;
    let app = fixture.save_app();
    request_quit(&fixture.manager, &fixture.runtime, &app, &control).await.unwrap();
    assert!(control.cancelled.load(Ordering::SeqCst));
    assert!(!control.cancelled());
    assert_eq!(fixture.dispatch_state(&app), crate::app_dispatch::State::Sent);
    let status = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Some(status) = fixture.child.try_wait().unwrap() { return status; }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.unwrap();
    assert_eq!(status.signal(), Some(libc::SIGTERM));
}
