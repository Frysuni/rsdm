//! Private records and a peer bus keep shutdown tests off the live session.

use std::{
    fs::{self, OpenOptions}, os::{fd::AsRawFd, unix::{fs::PermissionsExt, net::UnixStream}},
    sync::atomic::AtomicUsize, time::Instant,
};

use super::*;
use rsdm_core::domain::ShutdownPolicy;
use zbus::zvariant::OwnedObjectPath;

const GENERATION: &str = "0123456789abcdef0123456789abcdef";
static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

struct Manager(Arc<AtomicUsize>);

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Manager")]
impl Manager {
    fn get_unit(&self, _unit: &str) -> OwnedObjectPath {
        OwnedObjectPath::try_from("/unit").unwrap()
    }

    fn get_unit_processes(&self, _unit: &str) -> Vec<(String, u32, String)> {
        self.0.fetch_add(1, Ordering::SeqCst);
        vec![("/test".into(), 42, "app".into())]
    }
}

struct Unit;

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Unit")]
impl Unit {
    #[zbus(property, name = "InvocationID")]
    fn invocation_id(&self) -> Vec<u8> { vec![1; 16] }
}

struct Service;

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Service")]
impl Service {
    #[zbus(property, name = "ControlPID")]
    fn control_pid(&self) -> u32 { 0 }
}

struct Fixture {
    runtime: Runtime,
    app: AppRecord,
    manager: UserManager,
    queries: Arc<AtomicUsize>,
    _server: zbus::Connection,
}

impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("rsdm-app-stop-test-{}-{}",
            std::process::id(), NEXT_ID.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        fs::create_dir(path.join("apps")).unwrap();
        let runtime = Runtime { path, generation: GENERATION.into() };
        let app = AppRecord {
            unit: format!("app-rsdm-example@{GENERATION}-1.service"), invocation_id: vec![1; 16],
            policy: ShutdownPolicy::default(), deadline_usec: None, quit_started: false,
        };
        runtime.save_app(&app).unwrap();
        let queries = Arc::new(AtomicUsize::new(0));
        let shared = queries.clone();
        let (server_socket, client_socket) = UnixStream::pair().unwrap();
        let server = thread::spawn(move || async_io::block_on(async {
            zbus::connection::Builder::unix_stream(server_socket).p2p()
                .server(zbus::Guid::generate()).unwrap()
                .serve_at("/org/freedesktop/systemd1", Manager(shared)).unwrap()
                .serve_at("/unit", Unit).unwrap().serve_at("/unit", Service).unwrap()
                .build().await.unwrap()
        }));
        let connection = async_io::block_on(async {
            zbus::connection::Builder::unix_stream(client_socket).p2p().build().await.unwrap()
        });
        Self { runtime, app, queries, manager: UserManager::with_connection(connection), _server: server.join().unwrap() }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.runtime.path); }
}

#[test]
fn waiting_for_an_app_does_not_hold_its_record_lease() {
    let fixture = Fixture::new();
    let runtime = fixture.runtime.clone();
    let manager = fixture.manager.clone();
    let unit = fixture.app.unit.clone();
    let control = Arc::new(ShutdownControl::default());
    control.xsmp_units.set(vec![unit.clone()]).unwrap();
    let worker_control = control.clone();
    let (sender, result) = std::sync::mpsc::channel();
    let worker = thread::spawn(move || {
        let _ = sender.send(prepare_app(&manager, &runtime, &unit, &worker_control));
    });
    let deadline = Instant::now() + Duration::from_secs(2);
    while fixture.queries.load(Ordering::SeqCst) < 2 && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    let lease = OpenOptions::new().read(true).write(true)
        .open(fixture.runtime.path.join("apps").join(format!("{}.lock", fixture.app.unit))).unwrap();
    // SAFETY: the descriptor is owned and this lock attempt cannot block.
    let acquired = unsafe { libc::flock(lease.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
    drop(lease);
    control.cancelled.store(true, Ordering::SeqCst);
    let outcome = result.recv_timeout(Duration::from_secs(2)).unwrap().unwrap();
    worker.join().unwrap();
    assert!(fixture.queries.load(Ordering::SeqCst) >= 2);
    assert!(acquired, "application wait retained the record lease");
    assert_eq!(outcome, AppOutcome::Cancelled);
    let saved = fixture.runtime.app(&fixture.app.unit).unwrap();
    assert!(!saved.quit_started);
    assert_eq!(saved.deadline_usec, None);
}

#[test]
fn competing_preparations_claim_quit_once_and_preserve_the_deadline() {
    let fixture = Fixture::new();
    let control = ShutdownControl::default();
    let (first, claimed) = claim_preparation(&fixture.runtime, &fixture.app.unit, &control, &control.hard_deadline).unwrap();
    assert!(claimed);
    let (second, claimed) = claim_preparation(&fixture.runtime, &fixture.app.unit, &control, &control.hard_deadline).unwrap();
    assert!(!claimed);
    assert_eq!(first.deadline_usec, second.deadline_usec);
}

#[test]
fn a_contended_preparation_lease_preserves_the_teardown_budget() {
    let fixture = Fixture::new();
    let _lease = fixture.runtime.app_lease(&fixture.app.unit).unwrap();
    let control = ShutdownControl::for_manager(&fixture.manager);
    control.force(monotonic_usec().unwrap() + 200_000);
    let manager = preparation_manager(&fixture.manager, &control).unwrap();
    assert!(claim_preparation(&fixture.runtime, &fixture.app.unit, &control, &manager.deadline).is_err());
    assert!(control.hard_deadline.remaining(Duration::MAX).is_ok());
    let saved = fixture.runtime.app(&fixture.app.unit).unwrap();
    assert!(!saved.quit_started);
    assert_eq!(saved.deadline_usec, None);
}

#[test]
fn cancellation_does_not_reset_a_replacement_invocation() {
    let fixture = Fixture::new();
    let (prepared, _) = claim_preparation(&fixture.runtime, &fixture.app.unit, &ShutdownControl::default(), &Deadline::default()).unwrap();
    let mut replacement = prepared.clone();
    replacement.invocation_id = vec![2; 16];
    fixture.runtime.save_app(&replacement).unwrap();
    reset_preparation(&fixture.runtime, &prepared, &Deadline::default()).unwrap();
    let saved = fixture.runtime.app(&fixture.app.unit).unwrap();
    assert_eq!(saved.invocation_id, replacement.invocation_id);
    assert_eq!(saved.deadline_usec, replacement.deadline_usec);
    assert!(saved.quit_started);
}

#[test]
fn a_rejected_quit_helper_never_stops_or_unrefs_a_conflicting_unit() {
    use crate::session_manager::helper_start_fixture::{self, Mode};
    let app = helper_start_fixture::app();
    for mode in [Mode::Collision, Mode::Denied] {
        let fixture = helper_start_fixture::Fixture::new(app.unit.replacen("app-rsdm-", "rsdm-quit-", 1), mode);
        assert!(matches!(launch_quit(&fixture.manager, &app, helper_start_fixture::GENERATION), Err(SessionError::StartRejected { .. })));
        assert_eq!(fixture.state.starts.load(Ordering::SeqCst), 1);
        assert!(fixture.state.lifetime_bound.load(Ordering::SeqCst));
        assert_eq!(fixture.state.stops.load(Ordering::SeqCst), 0);
        assert_eq!(fixture.state.unrefs.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn an_accepted_quit_helper_with_an_inspection_error_still_attempts_cleanup() {
    use crate::session_manager::helper_start_fixture::{self, Mode};
    let app = helper_start_fixture::app();
    let fixture = helper_start_fixture::Fixture::new(app.unit.replacen("app-rsdm-", "rsdm-quit-", 1), Mode::InspectionDenied);
    assert!(launch_quit(&fixture.manager, &app, helper_start_fixture::GENERATION).is_err());
    assert_eq!(fixture.state.starts.load(Ordering::SeqCst), 1);
    assert!(fixture.state.lifetime_bound.load(Ordering::SeqCst));
    assert_eq!(fixture.state.unrefs.load(Ordering::SeqCst), 1);
}
