//! Cancellation during process discovery must prevent the claimed TERM.

use std::{fs, os::unix::{fs::PermissionsExt, process::ExitStatusExt}, path::PathBuf,
    process::{Child, Command}, sync::{Arc, atomic::AtomicU32}};

use zbus::{Connection, zvariant::OwnedObjectPath};

use super::*;

struct Manager { pid: u32, control: Arc<ShutdownControl> }

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Manager")]
impl Manager {
    fn get_unit(&self, _unit: &str) -> OwnedObjectPath { OwnedObjectPath::try_from("/app").unwrap() }

    fn get_unit_processes(&self, _unit: &str) -> Vec<(String, u32, String)> {
        self.control.cancelled.store(true, Ordering::SeqCst);
        vec![("/test".into(), self.pid, "application".into())]
    }
}

struct Unit;

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Unit")]
impl Unit {
    #[zbus(property, name = "InvocationID")]
    fn invocation_id(&self) -> Vec<u8> { vec![1; 16] }
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
    async fn new(control: Arc<ShutdownControl>) -> Self {
        let child = Command::new("sleep").arg("60").spawn().unwrap();
        let pid = child.id();
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
                .serve_at("/org/freedesktop/systemd1", Manager { pid, control }).unwrap()
                .serve_at("/app", Unit).unwrap().serve_at("/app", service).unwrap().build().await.unwrap()
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
async fn cancellation_during_main_pid_verification_leaves_term_unsent() {
    let control = Arc::new(ShutdownControl::default());
    let mut fixture = Fixture::new(control.clone()).await;
    let app = fixture.save_app();
    request_quit(&fixture.manager, &fixture.runtime, &app, &control).await.unwrap();
    assert!(control.cancelled());
    assert_eq!(fixture.dispatch_state(&app), crate::app_dispatch::State::NotSent);
    assert!(fixture.child.try_wait().unwrap().is_none());
}

#[tokio::test]
async fn cancellation_during_cgroup_snapshot_leaves_term_fallback_unsent() {
    let control = Arc::new(ShutdownControl::default());
    let mut fixture = Fixture::new(control.clone()).await;
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
    let mut fixture = Fixture::new(control.clone()).await;
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
