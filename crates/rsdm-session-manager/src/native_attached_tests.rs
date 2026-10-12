//! Scoped native logout uses independent user-bus and logind protocol peers.

use super::*;
use crate::{app_stop::ShutdownControl, control_test_authority::Context, identity::SessionIdentity,
    native_attached::{self, Scoped}};
use rsdm_core::domain::SessionPhase;
use std::sync::atomic::Ordering;
use std::os::fd::{FromRawFd, OwnedFd};

struct Attached {
    manager: UserManager, server: zbus::Connection, state: Arc<State>, login: Arc<Context>,
    target: Scoped, identity: SessionIdentity, control: ShutdownControl,
}

impl Attached {
    async fn create(kind: ProviderKind, reply: Reply) -> Self {
        let mut broker = Broker::current();
        broker.gnome_missing = kind == ProviderKind::Plasma;
        // SAFETY: pidfd_open pins this unreaped, still-running test process.
        let descriptor = unsafe { libc::syscall(libc::SYS_pidfd_open, std::process::id(), 0) };
        assert!(descriptor >= 0);
        broker.descriptor = Some(unsafe { OwnedFd::from_raw_fd(descriptor as i32) });
        let (manager, server, state) = fixture(reply, broker).await;
        let mut identity = crate::helper_start_fixture::record().identity;
        identity.uid = unsafe { libc::geteuid() };
        let login = Context::create(identity.clone()).await;
        let targets = manager.read(|| async { native_attached::read_targets(manager.connection()).await }).await.unwrap();
        let target = native_attached::scope_targets(&manager, &identity, login.logind.clone(), targets).await.unwrap().unwrap();

        let transaction = crate::shutdown_transaction::ShutdownTransaction::start(&login.runtime, "logout", None).await.unwrap();
        manager.deadline.set(transaction.deadline_usec);
        let mut control = ShutdownControl::for_manager(&manager);
        control.transaction_id = Some(transaction.id.clone());
        let mut record = login.runtime.session().unwrap();
        record.provider = "partial".into();
        record.owns_targets = false;
        record.phase = SessionPhase::StoppingSession;
        record.shutdown_transaction = Some(transaction);
        login.runtime.save_session(&record).unwrap();
        Self { manager, server, state, login, target, identity, control }
    }

    async fn logout(&self) -> Result<(), SessionError> {
        self.target.logout(&self.manager, &self.login.runtime, &self.identity, &self.control).await
    }
}

#[tokio::test]
async fn matched_peers_receive_one_native_logout_even_when_the_reply_is_lost() {
    for kind in [ProviderKind::Gnome, ProviderKind::Plasma] {
        for reply in [Reply::Success, Reply::Lost] {
            let attached = Attached::create(kind, reply).await;
            let result = attached.logout().await;
            assert_eq!(result.is_ok(), matches!(reply, Reply::Success));
            let method = if kind == ProviderKind::Gnome { "Logout" } else { "promptLogout" };
            assert_eq!(*attached.state.calls.lock().unwrap(), [method]);
        }
    }
}

#[tokio::test]
async fn a_changed_login_uid_or_bus_owner_cannot_receive_logout() {
    for invalid in ["login", "uid", "unscoped", "owner"] {
        let attached = Attached::create(ProviderKind::Gnome, Reply::Success).await;
        match invalid {
            "login" | "uid" => {
                let mut identity = attached.identity.clone();
                if invalid == "login" { identity.login_session_id.push_str("-other"); } else { identity.uid ^= 1; }
                attached.login.set_login(identity).await;
            }
            "unscoped" => { attached.login.missing_login.store(true, Ordering::SeqCst); }
            "owner" => {
                let interface = attached.server.object_server().interface::<_, Broker>("/org/freedesktop/DBus").await.unwrap();
                interface.get_mut().await.replaced = true;
            }
            _ => unreachable!(),
        }
        assert!(attached.logout().await.is_err(), "{invalid}");
        assert!(attached.state.calls.lock().unwrap().is_empty(), "{invalid}");
    }
}

#[tokio::test]
async fn revoked_preparation_and_stale_records_never_send_native_logout() {
    for invalid in ["cancel", "deadline", "gate", "recovery", "transaction", "phase", "disabled"] {
        let mut attached = Attached::create(ProviderKind::Gnome, Reply::Success).await;
        match invalid {
            "cancel" => attached.control.cancelled.store(true, Ordering::SeqCst),
            "deadline" => attached.manager.deadline.set(1),
            "gate" => attached.manager.actions.close(),
            "recovery" => attached.control.recovery.store(true, Ordering::SeqCst),
            "transaction" => attached.control.transaction_id = Some("stale".into()),
            "phase" | "disabled" => {
                let mut record = attached.login.runtime.session().unwrap();
                if invalid == "phase" { record.phase = SessionPhase::Running; } else { record.disabled = true; }
                attached.login.runtime.save_session(&record).unwrap();
            }
            _ => unreachable!(),
        }
        assert!(attached.logout().await.is_err(), "{invalid}");
        assert!(attached.state.calls.lock().unwrap().is_empty(), "{invalid}");
    }
}

#[tokio::test]
async fn an_expiring_native_reply_is_not_replayed() {
    let attached = Attached::create(ProviderKind::Gnome, Reply::Pending).await;
    let state = attached.state.clone();
    let deadline = attached.manager.deadline.clone();
    let worker = tokio::spawn(async move { attached.logout().await });
    tokio::time::timeout(Duration::from_secs(2), state.accepted.notified()).await.unwrap();
    deadline.set(1);
    assert!(tokio::time::timeout(Duration::from_secs(1), worker).await.unwrap().unwrap().is_err());
    assert_eq!(*state.calls.lock().unwrap(), ["Logout"]);
}

#[tokio::test]
async fn numeric_pid_credentials_cannot_authorize_an_attached_native_peer() {
    let (manager, _server, state) = fixture(Reply::Success, Broker::current()).await;
    let mut identity = crate::helper_start_fixture::record().identity;
    identity.uid = unsafe { libc::geteuid() };
    let login = Context::create(identity.clone()).await;
    let targets = manager.read(|| async { native_attached::read_targets(manager.connection()).await }).await.unwrap();
    assert!(native_attached::scope_targets(&manager, &identity, login.logind.clone(), targets).await.is_err());
    assert!(state.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn cancellation_interrupts_native_lookup_while_all_request_slots_are_occupied() {
    let (manager, _server, state) = fixture(Reply::Success, Broker::current()).await;
    let (entered, mut occupied) = tokio::sync::mpsc::unbounded_channel();
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let mut workers = tokio::task::JoinSet::new();
    for _ in 0..4 {
        let manager = manager.clone();
        let entered = entered.clone();
        let release = release.clone();
        workers.spawn(async move { manager.request(async {
            entered.send(()).unwrap();
            release.acquire().await.unwrap().forget();
            Ok(())
        }).await.unwrap(); });
    }
    for _ in 0..4 { tokio::time::timeout(Duration::from_secs(1), occupied.recv()).await.unwrap().unwrap(); }
    let control = Arc::new(ShutdownControl::default());
    let cancelled = control.clone();
    let identity = crate::helper_start_fixture::record().identity;
    let lookup = tokio::spawn(async move { native_attached::prepare(&manager, &identity, &control).await });
    tokio::time::sleep(Duration::from_millis(35)).await;
    assert!(!lookup.is_finished());
    cancelled.cancelled.store(true, Ordering::SeqCst);
    assert!(tokio::time::timeout(Duration::from_millis(500), lookup).await.unwrap().unwrap().is_err());
    assert!(state.calls.lock().unwrap().is_empty());
    release.add_permits(4);
    while let Some(result) = workers.join_next().await { result.unwrap(); }
}
