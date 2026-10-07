//! Shutdown decisions on private records and an isolated peer-to-peer bus.

use std::{fs, os::unix::{fs::PermissionsExt, net::UnixStream}, sync::atomic::AtomicUsize};

use super::*;
use crate::{power::{ShutdownMonitor, ShutdownNotice}, session_manager::{
    identity::SessionIdentity, lifecycle::Lifecycle, provider::ProviderKind, xsmp,
}};

const GENERATION: &str = "0123456789abcdef0123456789abcdef";
static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

struct Fixture {
    coordinator: Coordinator,
    notices: std::sync::mpsc::Sender<ShutdownNotice>,
    _server: zbus::Connection,
    directory: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!("rsdm-shutdown-test-{}-{}",
            std::process::id(), NEXT_ID.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        fs::create_dir(directory.join("apps")).unwrap();
        fs::set_permissions(directory.join("apps"), fs::Permissions::from_mode(0o700)).unwrap();
        let runtime = Runtime { path: directory.clone(), generation: GENERATION.into() };
        let (server_socket, client_socket) = UnixStream::pair().unwrap();
        let server = thread::spawn(move || async_io::block_on(async {
            zbus::connection::Builder::unix_stream(server_socket).p2p()
                .server(zbus::Guid::generate()).unwrap().build().await.unwrap()
        }));
        let connection = async_io::block_on(async {
            zbus::connection::Builder::unix_stream(client_socket).p2p().build().await.unwrap()
        });
        let manager = UserManager { connection: connection.clone() };
        let provider = Provider { kind: ProviderKind::External, native_unit: None, logout_command: Vec::new() };
        let xsmp = xsmp::Handle::start(&manager, &runtime, &provider).unwrap();
        // SAFETY: geteuid has no preconditions.
        let uid = unsafe { libc::geteuid() };
        let record = SessionRecord {
            identity: SessionIdentity { generation: GENERATION.into(), login_session_id: "test".into(),
                desktop_entry_id: None, uid },
            anchor_unit: format!("rsdm-session-{GENERATION}.service"), compositor_unit: None,
            compositor_invocation: Vec::new(), provider: "external".into(), owns_targets: false,
            phase: SessionPhase::Running, exported_environment: Vec::new(), shutdown_deadline_usec: None,
        };
        runtime.save_session(&record).unwrap();
        let (_, requests) = std::sync::mpsc::channel();
        let (events, work) = std::sync::mpsc::channel();
        let (notices, received) = std::sync::mpsc::channel();
        let mut lifecycle = Lifecycle::new();
        lifecycle.ready();
        let coordinator = Coordinator {
            manager, runtime, record, provider, lifecycle, process: None, environment: Vec::new(),
            directory: directory.clone(), requests, work, events, queued: Default::default(),
            finalize_replies: Vec::new(), stop_replies: Vec::new(), stopping: Arc::default(),
            shutdown: None, action: String::new(), ready_busy: false, ready_once: true,
            pending_ready: false,
            preparing: false, forced_units: Vec::new(), exit_code: 0, replies_pending: 0, workers: 0,
            xsmp, _control_bus: connection, ready_deadline: Instant::now(), display_since: None,
            _power_monitor: ShutdownMonitor::idle_for_test(), notices: received, last_reap: Instant::now(),
            _lease: crate::session_manager::session_lease::SessionLease::acquire_in(&directory).unwrap(),
        };
        Self { coordinator, notices, _server: server.join().unwrap(), directory }
    }

    fn notify(&mut self, preparing: bool, budget_usec: u64) {
        self.notices.send(ShutdownNotice { preparing, budget_usec }).unwrap();
        self.coordinator.notifications().unwrap();
    }

    fn deadline(&self) -> u64 {
        self.coordinator.shutdown.as_ref().unwrap().hard_deadline.load(Ordering::SeqCst)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.directory); }
}

use crate::session_manager::processes::monotonic_usec;

#[test]
fn logind_shutdown_uses_the_full_delay_budget() {
    let mut fixture = Fixture::new();
    let before = monotonic_usec().unwrap();
    fixture.notify(true, 30_000_000);
    let after = monotonic_usec().unwrap();
    assert!((before + 30_000_000..=after + 30_000_000).contains(&fixture.deadline()));
    assert_eq!(fixture.coordinator.runtime.session().unwrap().shutdown_deadline_usec, Some(fixture.deadline()));
    assert!(fixture.coordinator.shutdown.as_ref().unwrap().noncancelable.load(Ordering::SeqCst));
    assert_eq!(fixture.coordinator.lifecycle.phase, SessionPhase::Preparing);
}

#[test]
fn repeated_notices_never_extend_the_original_deadline() {
    let mut fixture = Fixture::new();
    fixture.notify(true, 30_000_000);
    let original = fixture.deadline();
    fixture.notify(true, 60_000_000);
    assert_eq!(fixture.deadline(), original);
    let before = monotonic_usec().unwrap();
    fixture.notify(true, 1_000_000);
    let after = monotonic_usec().unwrap();
    assert!((before + 1_000_000..=after + 1_000_000).contains(&fixture.deadline()));
    assert!(fixture.deadline() < original);
}

#[test]
fn wm_exit_does_not_replace_loginds_budget_or_prevent_its_cancellation() {
    let mut fixture = Fixture::new();
    fixture.notify(true, 30_000_000);
    let original = fixture.deadline();
    for action in ["compositor-exited", "external-stop"] {
        fixture.coordinator.begin_stop(action, Some(monotonic_usec().unwrap() + 5_000_000)).unwrap();
        assert_eq!(fixture.deadline(), original);
        assert_eq!(fixture.coordinator.action, "external-shutdown");
    }
    fixture.notify(false, 30_000_000);
    let control = fixture.coordinator.shutdown.as_ref().unwrap();
    assert!(control.cancelled.load(Ordering::SeqCst));
    assert!(!control.noncancelable.load(Ordering::SeqCst));
    assert_eq!(fixture.deadline(), 0);
    assert_eq!(fixture.coordinator.runtime.session().unwrap().shutdown_deadline_usec, None);
    fixture.notify(true, 30_000_000);
    assert!(!fixture.coordinator.shutdown.as_ref().unwrap().cancelled.load(Ordering::SeqCst));
    assert!(fixture.deadline() >= original);
}

#[test]
fn compositor_cleanup_retains_its_explicit_five_second_deadline() {
    let mut fixture = Fixture::new();
    let deadline = monotonic_usec().unwrap() + 5_000_000;
    fixture.coordinator.begin_stop("compositor-exited", Some(deadline)).unwrap();
    assert_eq!(fixture.deadline(), deadline);
    assert_eq!(fixture.coordinator.record.shutdown_deadline_usec, None);
    fixture.notify(true, 30_000_000);
    assert_eq!(fixture.deadline(), deadline);
    assert_eq!(fixture.coordinator.action, "external-shutdown");
}

#[test]
fn notices_during_infrastructure_stop_record_the_budget_without_reopening_preparation() {
    let mut fixture = Fixture::new();
    fixture.coordinator.begin_stop("reboot", None).unwrap();
    fixture.coordinator.lifecycle.phase = SessionPhase::StoppingSession;
    let before = monotonic_usec().unwrap();
    fixture.notify(true, 30_000_000);
    let after = monotonic_usec().unwrap();
    assert!((before + 30_000_000..=after + 30_000_000).contains(&fixture.deadline()));
    assert_eq!(fixture.coordinator.lifecycle.phase, SessionPhase::StoppingSession);
    assert_eq!(fixture.coordinator.action, "reboot");
    fixture.notify(false, 30_000_000);
    assert!(fixture.coordinator.shutdown.as_ref().unwrap().noncancelable.load(Ordering::SeqCst));
}

#[path = "coordinator_completion_tests.rs"]
mod completion;
