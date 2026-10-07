//! Completion recovery against a private user-manager substitute.

use std::sync::atomic::{AtomicBool, AtomicUsize};

use zbus::zvariant::OwnedObjectPath;

use super::*;

#[derive(Default)]
struct ManagerState {
    unavailable: AtomicBool,
    active: AtomicBool,
    reads: AtomicUsize,
    starts: AtomicUsize,
}

struct FakeManager(Arc<ManagerState>);

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Manager")]
impl FakeManager {
    fn get_unit(&self, _unit: &str) -> zbus::fdo::Result<OwnedObjectPath> {
        self.0.reads.fetch_add(1, Ordering::SeqCst);
        if self.0.unavailable.load(Ordering::SeqCst) {
            return Err(zbus::fdo::Error::NoReply("user manager is reexecuting".into()));
        }
        Ok(OwnedObjectPath::try_from("/org/freedesktop/systemd1/unit/anchor").unwrap())
    }

    fn start_transient_unit(
        &self, _unit: &str, _mode: &str,
        _properties: Vec<(String, zbus::zvariant::OwnedValue)>,
        _auxiliary: Vec<(String, Vec<(String, zbus::zvariant::OwnedValue)>)>,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        self.0.starts.fetch_add(1, Ordering::SeqCst);
        Err(zbus::fdo::Error::Failed("activation must not be repeated".into()))
    }
}

struct FakeUnit(Arc<ManagerState>);

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Unit")]
impl FakeUnit {
    #[zbus(property)]
    fn active_state(&self) -> &str {
        if self.0.active.load(Ordering::SeqCst) { "active" } else { "inactive" }
    }

    #[zbus(property, name = "InvocationID")]
    fn invocation_id(&self) -> Vec<u8> { vec![1; 16] }
}

struct FakeService;

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Service")]
impl FakeService {
    #[zbus(property)]
    fn environment(&self) -> Vec<String> { vec![format!("RSDM_SESSION_GENERATION={GENERATION}")] }
}

fn install_manager(fixture: &Fixture) -> Arc<ManagerState> {
    let state = Arc::new(ManagerState::default());
    state.unavailable.store(true, Ordering::SeqCst);
    state.active.store(true, Ordering::SeqCst);
    async_io::block_on(async {
        fixture._server.object_server().at("/org/freedesktop/systemd1", FakeManager(state.clone())).await.unwrap();
        fixture._server.object_server().at("/org/freedesktop/systemd1/unit/anchor", FakeUnit(state.clone())).await.unwrap();
        fixture._server.object_server().at("/org/freedesktop/systemd1/unit/anchor", FakeService).await.unwrap();
    });
    state
}

#[test]
fn a_lost_invocation_read_after_start_does_not_stop_or_restart_the_compositor() {
    let mut fixture = Fixture::new();
    let state = install_manager(&fixture);
    fixture.coordinator.lifecycle.phase = SessionPhase::Starting;
    fixture.coordinator.ready_once = false;
    let provider = super::super::super::provider::Provider {
        kind: ProviderKind::Managed, native_unit: None, logout_command: Vec::new(),
    };
    fixture.coordinator.process = Some(crate::session_manager::session_process::SessionProcess::launch(
        &["true".into()], &[], &fixture.directory, &provider, "example.service".into(),
    ).unwrap());
    fixture.coordinator.boot_completed(Err(zbus::Error::from(zbus::fdo::Error::NoReply("reexec".into())).into())).unwrap();
    assert!(fixture.coordinator.pending_boot);
    assert!(fixture.coordinator.booting());
    assert_eq!(fixture.coordinator.exit_code, 0);
    fixture.coordinator.verify_boot().unwrap();
    assert!(fixture.coordinator.pending_boot);
    assert_eq!(fixture.coordinator.lifecycle.phase, SessionPhase::Starting);

    state.unavailable.store(false, Ordering::SeqCst);
    fixture.coordinator.verify_boot().unwrap();
    assert!(!fixture.coordinator.pending_boot);
    assert!(!fixture.coordinator.booting());
    assert_eq!(fixture.coordinator.record.compositor_invocation, vec![1; 16]);
    assert_eq!(state.starts.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.coordinator.workers, 0);
}

#[test]
fn readiness_completion_waits_for_reads_without_repeating_activation() {
    let mut fixture = Fixture::new();
    let state = install_manager(&fixture);
    fixture.coordinator.lifecycle.phase = SessionPhase::Starting;
    fixture.coordinator.ready_once = false;
    fixture.coordinator.ready_busy = true;
    let (reply, received) = async_channel::bounded(1);
    fixture.coordinator.finalize_replies.push(reply);

    fixture.coordinator.ready_completed(Ok(true)).unwrap();
    assert!(fixture.coordinator.pending_ready);
    assert!(fixture.coordinator.ready_busy);
    assert!(fixture.coordinator.record.owns_targets);
    assert_eq!(fixture.coordinator.lifecycle.phase, SessionPhase::Starting);
    assert!(received.try_recv().is_err());
    fixture.coordinator.observe_readiness().unwrap();
    assert_eq!(state.starts.load(Ordering::SeqCst), 0);

    state.unavailable.store(false, Ordering::SeqCst);
    fixture.coordinator.verify_readiness().unwrap();
    assert!(!fixture.coordinator.pending_ready);
    assert!(!fixture.coordinator.ready_busy);
    assert_eq!(fixture.coordinator.lifecycle.phase, SessionPhase::Running);
    assert!(received.try_recv().unwrap().is_ok());
    assert_eq!(fixture.coordinator.workers, 0);
    assert_eq!(state.starts.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.coordinator.runtime.session().unwrap().phase, SessionPhase::Running);
}

#[test]
fn shutdown_releases_pending_readiness_even_during_an_outage() {
    let mut fixture = Fixture::new();
    let state = install_manager(&fixture);
    fixture.coordinator.lifecycle.phase = SessionPhase::Starting;
    fixture.coordinator.ready_busy = true;
    fixture.coordinator.pending_ready = true;
    let (reply, received) = async_channel::bounded(1);
    fixture.coordinator.finalize_replies.push(reply);

    fixture.coordinator.begin_stop("logout", None).unwrap();
    fixture.coordinator.verify_readiness().unwrap();
    assert!(received.try_recv().unwrap().is_err());
    assert!(!fixture.coordinator.ready_busy);
    assert!(!fixture.coordinator.pending_ready);
    assert_eq!(fixture.coordinator.lifecycle.phase, SessionPhase::Preparing);
    assert_eq!(fixture.coordinator.workers, 0);
    assert_eq!(state.reads.load(Ordering::SeqCst), 0);
}

#[test]
fn preparation_failure_and_native_delegation_resume_without_manager_reads() {
    for delegated in [false, true] {
        for ready_once in [false, true] {
            let mut fixture = Fixture::new();
            let state = install_manager(&fixture);
            fixture.coordinator.provider.kind = ProviderKind::Gnome;
            fixture.coordinator.ready_once = ready_once;
            fixture.coordinator.begin_stop("logout", None).unwrap();
            fixture.coordinator.preparing = true;
            let (reply, received) = async_channel::bounded(1);
            fixture.coordinator.stop_replies.push(reply);

            if delegated {
                fixture.coordinator.lifecycle.phase = SessionPhase::StoppingSession;
                fixture.coordinator.delegated(Ok(())).unwrap();
            } else {
                fixture.coordinator.prepared(Err(SessionError::State("preparation failed".into()))).unwrap();
            }
            assert_eq!(received.try_recv().unwrap().unwrap().result, if delegated { "delegated" } else { "failed" });
            assert_eq!(fixture.coordinator.lifecycle.phase, if ready_once { SessionPhase::Running } else { SessionPhase::Starting });
            assert!(!fixture.coordinator.stopping.load(Ordering::SeqCst));
            assert!(!fixture.coordinator.preparing);
            assert!(fixture.coordinator.shutdown.is_none());
            assert_eq!(state.reads.load(Ordering::SeqCst), 0);
        }
    }
}

#[test]
fn observation_still_detects_a_real_stop_after_resuming() {
    let mut fixture = Fixture::new();
    let state = install_manager(&fixture);
    fixture.coordinator.begin_stop("logout", None).unwrap();
    fixture.coordinator.preparing = true;
    fixture.coordinator.prepared(Err(SessionError::State("preparation failed".into()))).unwrap();
    assert_eq!(fixture.coordinator.lifecycle.phase, SessionPhase::Running);

    state.active.store(false, Ordering::SeqCst);
    state.unavailable.store(false, Ordering::SeqCst);
    fixture.coordinator.observe().unwrap();
    assert_eq!(fixture.coordinator.lifecycle.phase, SessionPhase::Preparing);
    assert_eq!(fixture.coordinator.action, "external-stop");
    assert!(fixture.coordinator.stopping.load(Ordering::SeqCst));
}
