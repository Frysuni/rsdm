//! Private records and a peer bus keep shutdown tests off the live session.

use std::{
    fs::{self, OpenOptions}, os::{fd::AsRawFd, unix::fs::PermissionsExt},
    sync::atomic::AtomicUsize, time::Instant,
};

use super::*;

#[cfg(feature = "xsmp")]
#[path = "app_stop_batch_tests.rs"]
mod batch;
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
    async fn new() -> Self {
        let path = std::env::temp_dir().join(format!("rsdm-app-stop-test-{}-{}",
            std::process::id(), NEXT_ID.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        fs::create_dir(path.join("apps")).unwrap();
        let runtime = Runtime { path, generation: GENERATION.into() };
        let app = AppRecord {
            unit: format!("app-rsdm-example@{GENERATION}-1.service"), invocation_id: vec![1; 16],
            policy: ShutdownPolicy::default(), deadline_usec: None, quit_started: false, dispatch: None, window_close_sent: false, original_process: None, migrated_scope: None, lifetime_process: None, preparation: None,
        };
        runtime.save_app(&app).unwrap();
        let queries = Arc::new(AtomicUsize::new(0));
        let shared = queries.clone();
        let (server_socket, client_socket) = tokio::net::UnixStream::pair().unwrap();
        let server = tokio::spawn(async move {
            zbus::connection::Builder::unix_stream(server_socket).p2p()
                .server(zbus::Guid::generate()).unwrap()
                .serve_at("/org/freedesktop/systemd1", Manager(shared)).unwrap()
                .serve_at("/unit", Unit).unwrap().serve_at("/unit", Service).unwrap()
                .build().await.unwrap()
        });
        let connection = zbus::connection::Builder::unix_stream(client_socket).p2p().build().await.unwrap();
        Self { runtime, app, queries, manager: UserManager::with_connection(connection), _server: server.await.unwrap() }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.runtime.path); }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn waiting_for_an_app_does_not_hold_its_record_lease() {
    let fixture = Fixture::new().await;
    let runtime = fixture.runtime.clone();
    let manager = fixture.manager.clone();
    let unit = fixture.app.unit.clone();
    let control = Arc::new(ShutdownControl::default());
    *control.xsmp_units.lock().unwrap() = Some(vec![unit.clone()]);
    let worker_control = control.clone();
    let worker = tokio::spawn(async move {
        prepare_app(&manager, &runtime, &unit, &worker_control).await
    });
    let deadline = Instant::now() + Duration::from_secs(2);
    while fixture.queries.load(Ordering::SeqCst) < 2 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let lease = OpenOptions::new().read(true).write(true)
        .open(fixture.runtime.path.join("apps").join(format!("{}.lock", fixture.app.unit))).unwrap();
    // SAFETY: the descriptor is owned and this lock attempt cannot block.
    let acquired = unsafe { libc::flock(lease.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
    drop(lease);
    control.cancelled.store(true, Ordering::SeqCst);
    let outcome = tokio::time::timeout(Duration::from_secs(2), worker).await.unwrap().unwrap().unwrap();
    assert!(fixture.queries.load(Ordering::SeqCst) >= 2);
    assert!(acquired, "application wait retained the record lease");
    assert_eq!(outcome, AppOutcome::Cancelled);
    let saved = fixture.runtime.app(&fixture.app.unit).unwrap();
    assert!(saved.quit_started);
    assert_eq!(saved.dispatch.unwrap().state, crate::app_dispatch::State::Intent);
    assert_eq!(saved.deadline_usec, None);
}

#[tokio::test]
async fn resuming_a_deadline_without_dispatch_intent_keeps_the_budget_and_claims_quit() {
    let fixture = Fixture::new().await;
    let control = ShutdownControl::default();
    *control.xsmp_units.lock().unwrap() = Some(vec![fixture.app.unit.clone()]);
    let (prepared, claimed) = claim_preparation(
        &fixture.runtime, &fixture.app.unit, &control, &Deadline::default(),
    ).await.unwrap();
    assert!(claimed);
    assert!(!prepared.quit_started);
    assert!(prepared.dispatch.is_none());

    let resumed = begin_preparation(&fixture.manager, &fixture.runtime, &fixture.app.unit, &control).await.unwrap();
    assert!(matches!(resumed, Preparation::Waiting(_, None)));
    let saved = fixture.runtime.app(&fixture.app.unit).unwrap();
    assert_eq!(saved.deadline_usec, prepared.deadline_usec);
    assert!(saved.quit_started);
    assert_eq!(saved.dispatch.unwrap().method, crate::app_dispatch::Method::Xsmp);
}

#[tokio::test]
async fn competing_preparations_claim_quit_once_and_preserve_the_deadline() {
    let fixture = Fixture::new().await;
    let control = ShutdownControl::default();
    let (first, second) = tokio::join!(
        claim_preparation(&fixture.runtime, &fixture.app.unit, &control, &control.hard_deadline),
        claim_preparation(&fixture.runtime, &fixture.app.unit, &control, &control.hard_deadline),
    );
    let (first, first_claimed) = first.unwrap();
    let (second, second_claimed) = second.unwrap();
    assert_ne!(first_claimed, second_claimed);
    assert_eq!(first.deadline_usec, second.deadline_usec);
}

#[tokio::test]
async fn later_applications_are_prepared_while_the_first_four_still_run() {
    let fixture = Fixture::new().await;
    let mut apps = Vec::new();
    for index in 0..8 {
        let mut app = fixture.app.clone();
        app.unit = format!("app-rsdm-example@{GENERATION}-{index}.service");
        fixture.runtime.save_app(&app).unwrap();
        apps.push(app);
    }
    let control = Arc::new(ShutdownControl::default());
    control.recovery.store(true, Ordering::SeqCst);
    *control.xsmp_units.lock().unwrap() = Some(apps.iter().map(|app| app.unit.clone()).collect());
    let manager = fixture.manager.clone();
    let runtime = fixture.runtime.clone();
    let worker_control = control.clone();
    let worker = tokio::spawn(async move { prepare_apps(&manager, &runtime, apps, worker_control).await });

    let prepared = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let runtime = fixture.runtime.clone();
            let prepared = blocking(move || runtime.apps()).await.unwrap();
            if prepared.iter().filter(|app| app.quit_started).count() == 8 { break; }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }).await.is_ok();
    control.cancelled.store(true, Ordering::SeqCst);
    let forced = tokio::time::timeout(Duration::from_secs(2), worker).await.unwrap().unwrap().unwrap();
    assert!(prepared, "later applications waited for an earlier application's exit");
    assert!(forced.is_empty());
}

#[tokio::test]
async fn a_contended_preparation_lease_preserves_the_teardown_budget() {
    let fixture = Fixture::new().await;
    let _lease = fixture.runtime.app_lease(&fixture.app.unit).unwrap();
    let control = ShutdownControl::for_manager(&fixture.manager);
    control.force(monotonic_usec().unwrap() + 200_000);
    let manager = preparation_manager(&fixture.manager, &control).unwrap();
    assert!(claim_preparation(&fixture.runtime, &fixture.app.unit, &control, &manager.deadline).await.is_err());
    assert!(control.hard_deadline.remaining(Duration::MAX).is_ok());
    let saved = fixture.runtime.app(&fixture.app.unit).unwrap();
    assert!(!saved.quit_started);
    assert_eq!(saved.deadline_usec, None);
}

#[tokio::test]
async fn a_contended_record_claim_keeps_the_callers_runtime_responsive() {
    let fixture = Fixture::new().await;
    let lease = fixture.runtime.app_lease(&fixture.app.unit).unwrap();
    let control = ShutdownControl::default();
    let mut claim = Box::pin(claim_preparation(
        &fixture.runtime, &fixture.app.unit, &control, &control.hard_deadline,
    ));
    tokio::select! {
        biased;
        _ = tokio::time::sleep(Duration::from_millis(10)) => {},
        result = &mut claim => panic!("contended claim completed before the timer: {result:?}"),
    }
    drop(lease);
    let (app, claimed) = claim.await.unwrap();
    assert!(claimed);
    let saved = fixture.runtime.app(&app.unit).unwrap();
    assert!(!saved.quit_started);
    assert!(saved.deadline_usec.is_some());
}

#[tokio::test]
async fn cancellation_while_waiting_for_a_record_lease_does_not_dispatch_quit() {
    let fixture = Fixture::new().await;
    let lease = fixture.runtime.app_lease(&fixture.app.unit).unwrap();
    let control = ShutdownControl::default();
    let mut preparation = Box::pin(begin_preparation(
        &fixture.manager, &fixture.runtime, &fixture.app.unit, &control,
    ));
    let queried = async {
        while fixture.queries.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    };
    tokio::select! {
        result = tokio::time::timeout(Duration::from_secs(2), queried) => result.unwrap(),
        result = &mut preparation => panic!("contended preparation completed: {}", result.is_ok()),
    }
    tokio::select! {
        biased;
        _ = tokio::time::sleep(Duration::from_millis(20)) => {},
        result = &mut preparation => panic!("contended preparation completed: {}", result.is_ok()),
    }
    assert_eq!(fixture.queries.load(Ordering::SeqCst), 1);
    control.cancelled.store(true, Ordering::SeqCst);
    drop(lease);
    assert!(matches!(preparation.await.unwrap(), Preparation::Finished(_, AppOutcome::Cancelled)));
    let saved = fixture.runtime.app(&fixture.app.unit).unwrap();
    assert!(!saved.quit_started);
    assert_eq!(saved.deadline_usec, None);
}

#[tokio::test]
async fn cancellation_does_not_reset_a_replacement_invocation() {
    let fixture = Fixture::new().await;
    let (prepared, _) = claim_preparation(&fixture.runtime, &fixture.app.unit, &ShutdownControl::default(), &Deadline::default()).await.unwrap();
    let mut replacement = prepared.clone();
    replacement.invocation_id = vec![2; 16];
    replacement.quit_started = true;
    fixture.runtime.save_app(&replacement).unwrap();
    reset_preparation(&fixture.runtime, &prepared, &Deadline::default()).await.unwrap();
    let saved = fixture.runtime.app(&fixture.app.unit).unwrap();
    assert_eq!(saved.invocation_id, replacement.invocation_id);
    assert_eq!(saved.deadline_usec, replacement.deadline_usec);
    assert!(saved.quit_started);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rejected_quit_helper_never_stops_or_unrefs_a_conflicting_unit() {
    use crate::helper_start_fixture::{self, Mode};
    let app = helper_start_fixture::app();
    for mode in [Mode::Collision, Mode::Denied] {
        let fixture = helper_start_fixture::Fixture::new(app.unit.replacen("app-rsdm-", "rsdm-quit-", 1), mode).await;
        assert!(matches!(launch_quit(&fixture.manager, &app, &helper_start_fixture::record().anchor_unit, &ShutdownControl::default()).await, Err(SessionError::StartRejected { .. })));
        assert_eq!(fixture.state.starts.load(Ordering::SeqCst), 1);
        assert!(fixture.state.lifetime_bound.load(Ordering::SeqCst));
        assert_eq!(fixture.state.stops.load(Ordering::SeqCst), 0);
        assert_eq!(fixture.state.unrefs.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_accepted_quit_helper_with_an_inspection_error_still_attempts_cleanup() {
    use crate::helper_start_fixture::{self, Mode};
    let app = helper_start_fixture::app();
    let fixture = helper_start_fixture::Fixture::new(app.unit.replacen("app-rsdm-", "rsdm-quit-", 1), Mode::InspectionDenied).await;
    assert!(launch_quit(&fixture.manager, &app, &helper_start_fixture::record().anchor_unit, &ShutdownControl::default()).await.is_err());
    assert_eq!(fixture.state.starts.load(Ordering::SeqCst), 1);
    assert!(fixture.state.lifetime_bound.load(Ordering::SeqCst));
    assert_eq!(fixture.state.unrefs.load(Ordering::SeqCst), 1);
}
