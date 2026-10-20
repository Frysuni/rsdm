//! Native shutdown methods are submitted once, including when a reply is lost.

use std::{sync::{Arc, Mutex}, time::Duration};

use super::*;

#[path = "provider_login_fixture.rs"]
mod login_fixture;
use login_fixture::{delegate, available};

struct Broker {
    uid: Option<u32>,
    pid: Option<u32>,
    replaced: bool,
    missing: bool,
    owner_queries: std::sync::atomic::AtomicUsize,
    descriptor: Option<std::os::fd::OwnedFd>,
    activation: Option<Arc<Activation>>,
    disable: Option<crate::action_gate::ActionGate>,
    activatable: bool,
    replaced_desktop: bool,
    desktop_queries: std::sync::atomic::AtomicUsize,
    gnome_missing: bool,
}

struct Activation { calls: std::sync::atomic::AtomicUsize, lost: bool, replace_desktop: bool }

impl Broker {
    fn current() -> Self {
        // SAFETY: geteuid has no preconditions.
        Self { uid: Some(unsafe { libc::geteuid() }), pid: Some(std::process::id()), replaced: false,
            missing: false, owner_queries: Default::default(), descriptor: None, activation: None, disable: None,
            activatable: false, replaced_desktop: false, desktop_queries: Default::default(), gnome_missing: false }
    }

    fn pinned() -> Self {
        use std::os::fd::FromRawFd;
        let mut broker = Self::current();
        // SAFETY: pidfd_open pins the still-running test process.
        let descriptor = unsafe { libc::syscall(libc::SYS_pidfd_open, std::process::id(), 0) };
        assert!(descriptor >= 0);
        // SAFETY: the successful syscall returned an owned descriptor.
        broker.descriptor = Some(unsafe { std::os::fd::OwnedFd::from_raw_fd(descriptor as i32) });
        broker
    }

}

#[zbus_macros::interface(name = "org.freedesktop.DBus")]
impl Broker {
    fn get_name_owner(&self, name: &str) -> zbus::fdo::Result<String> {
        assert!(matches!(name, "org.gnome.SessionManager" | "org.kde.LogoutPrompt" | "org.kde.ksmserver" | "org.kde.KWin"));
        if self.gnome_missing && name == "org.gnome.SessionManager" { return Err(zbus::fdo::Error::NameHasNoOwner("GNOME is absent".into())); }
        let activated = self.activation.as_ref().is_some_and(|state| state.calls.load(std::sync::atomic::Ordering::SeqCst) != 0);
        if self.missing && !(name == "org.kde.ksmserver" && self.activation.is_some()) && !activated {
            return Err(zbus::fdo::Error::NameHasNoOwner("desktop is absent".into()));
        }
        if name == "org.kde.ksmserver" {
            let query = self.desktop_queries.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let replaced = (activated && self.activation.as_ref().is_some_and(|state| state.replace_desktop))
                || (self.replaced_desktop && query != 0);
            return Ok(if replaced { ":1.10" } else { ":1.9" }.into());
        }
        let query = self.owner_queries.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if query != 0 && let Some(actions) = &self.disable { actions.close(); }
        Ok(if self.replaced && query != 0 { ":1.2" } else { ":1.1" }.into())
    }

    fn get_connection_credentials(&self, name: &str) -> zbus::fdo::ConnectionCredentials {
        use std::os::fd::AsFd;
        assert!(matches!(name, ":1.1" | ":1.9"));
        let mut credentials = zbus::fdo::ConnectionCredentials::default();
        if let Some(uid) = self.uid { credentials = credentials.set_unix_user_id(uid); }
        if let Some(pid) = self.pid { credentials = credentials.set_process_id(pid); }
        if let Some(descriptor) = &self.descriptor {
            credentials = credentials.set_process_fd(descriptor.as_fd().try_clone_to_owned().unwrap().into());
        }
        credentials
    }

    fn start_service_by_name(&self, name: &str, flags: u32) -> zbus::fdo::Result<u32> {
        assert_eq!(name, "org.kde.LogoutPrompt");
        assert_eq!(flags, 0);
        let activation = self.activation.as_ref().expect("unexpected native service activation");
        activation.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if activation.lost { return Err(zbus::fdo::Error::NoReply("activation reply lost".into())); }
        Ok(1)
    }

    fn list_activatable_names(&self) -> Vec<String> {
        if self.activatable { vec!["org.kde.LogoutPrompt".into()] } else { Vec::new() }
    }
}

#[derive(Clone, Copy)]
enum Reply { Success, Lost, Pending }

struct State {
    calls: Mutex<Vec<String>>,
    accepted: tokio::sync::Notify,
    reply: Reply,
}

impl State {
    async fn request(&self, method: &str, header: zbus::message::Header<'_>) -> zbus::fdo::Result<()> {
        assert_eq!(header.destination().unwrap().as_str(), ":1.1");
        assert!(header.primary().flags().contains(zbus::message::Flags::NO_AUTO_START));
        self.calls.lock().unwrap().push(method.to_string());
        self.accepted.notify_one();
        match self.reply {
            Reply::Success => Ok(()),
            Reply::Lost => Err(zbus::fdo::Error::NoReply("accepted request; lost reply".into())),
            Reply::Pending => std::future::pending().await,
        }
    }
}

struct Gnome(Arc<State>);

#[zbus_macros::interface(name = "org.gnome.SessionManager")]
impl Gnome {
    async fn logout(&self, mode: u32, #[zbus(header)] header: zbus::message::Header<'_>) -> zbus::fdo::Result<()> {
        assert_eq!(mode, 0);
        self.0.request("Logout", header).await
    }

    async fn reboot(&self, #[zbus(header)] header: zbus::message::Header<'_>) -> zbus::fdo::Result<()> { self.0.request("Reboot", header).await }

    async fn shutdown(&self, #[zbus(header)] header: zbus::message::Header<'_>) -> zbus::fdo::Result<()> { self.0.request("Shutdown", header).await }
}

struct Plasma(Arc<State>);

#[zbus_macros::interface(name = "org.kde.LogoutPrompt")]
impl Plasma {
    #[zbus(name = "promptLogout")]
    async fn prompt_logout(&self, #[zbus(header)] header: zbus::message::Header<'_>) -> zbus::fdo::Result<()> { self.0.request("promptLogout", header).await }

    #[zbus(name = "promptReboot")]
    async fn prompt_reboot(&self, #[zbus(header)] header: zbus::message::Header<'_>) -> zbus::fdo::Result<()> { self.0.request("promptReboot", header).await }

    #[zbus(name = "promptShutDown")]
    async fn prompt_shut_down(&self, #[zbus(header)] header: zbus::message::Header<'_>) -> zbus::fdo::Result<()> { self.0.request("promptShutDown", header).await }
}

async fn fixture(reply: Reply, broker: Broker) -> (UserManager, zbus::Connection, Arc<State>) {
    let state = Arc::new(State { calls: Mutex::new(Vec::new()), accepted: tokio::sync::Notify::new(), reply });
    let gnome = Gnome(state.clone());
    let plasma = Plasma(state.clone());
    let (server_socket, client_socket) = tokio::net::UnixStream::pair().unwrap();
    let server = tokio::spawn(async move {
        zbus::connection::Builder::unix_stream(server_socket.into_std().unwrap()).p2p()
            .server(zbus::Guid::generate())
            .serve_at("/org/freedesktop/DBus", broker)
            .serve_at("/org/gnome/SessionManager", gnome)
            .serve_at("/LogoutPrompt", plasma).build().await.unwrap()
    });
    let connection = zbus::connection::Builder::unix_stream(client_socket.into_std().unwrap()).p2p().build().await.unwrap();
    (UserManager::with_connection(connection), server.await.unwrap(), state)
}

#[tokio::test]
async fn native_methods_are_dispatched_once_with_and_without_a_reply() {
    for reply in [Reply::Success, Reply::Lost] {
        let (manager, _server, state) = fixture(reply, Broker::pinned()).await;
        for kind in [ProviderKind::Gnome, ProviderKind::Plasma] {
            let provider = Provider { kind, native_unit: None, logout_command: Vec::new() };
            for action in ["logout", "reboot", "poweroff"] {
                let result = delegate(&provider, &manager, action).await;
                match reply {
                    Reply::Success => result.unwrap(),
                    Reply::Lost => assert!(matches!(result, Err(SessionError::Bus(error)) if crate::bus::retryable_error(&error))),
                    Reply::Pending => unreachable!(),
                }
            }
        }
        assert_eq!(*state.calls.lock().unwrap(), ["Logout", "Reboot", "Shutdown", "promptLogout", "promptReboot", "promptShutDown"]);
    }
}

#[tokio::test]
async fn an_expired_shared_deadline_prevents_native_dispatch() {
    let (manager, _server, state) = fixture(Reply::Success, Broker::pinned()).await;
    manager.deadline.set(1);
    let provider = Provider { kind: ProviderKind::Gnome, native_unit: None, logout_command: Vec::new() };
    assert!(delegate(&provider, &manager, "logout").await.is_err());
    assert!(state.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn revising_the_deadline_interrupts_a_pending_native_reply() {
    let (manager, _server, state) = fixture(Reply::Pending, Broker::pinned()).await;
    let deadline = manager.deadline.clone();
    let provider = Provider { kind: ProviderKind::Gnome, native_unit: None, logout_command: Vec::new() };
    let worker = tokio::spawn(async move { delegate(&provider, &manager, "logout").await });
    tokio::time::timeout(Duration::from_secs(2), state.accepted.notified()).await.unwrap();
    deadline.set(1);
    let result = tokio::time::timeout(Duration::from_secs(1), worker).await.unwrap().unwrap();
    assert!(matches!(result, Err(SessionError::Bus(error)) if crate::bus::retryable_error(&error)));
    assert_eq!(*state.calls.lock().unwrap(), ["Logout"]);
}

#[tokio::test]
async fn unsupported_actions_and_providers_do_not_dispatch() {
    let (manager, _server, state) = fixture(Reply::Success, Broker::pinned()).await;
    let provider = Provider { kind: ProviderKind::Gnome, native_unit: None, logout_command: Vec::new() };
    assert!(delegate(&provider, &manager, "suspend").await.is_err());
    let provider = Provider { kind: ProviderKind::Managed, native_unit: None, logout_command: Vec::new() };
    assert!(delegate(&provider, &manager, "logout").await.is_err());
    assert!(state.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn missing_changed_foreign_or_unidentified_owners_cannot_receive_native_shutdown() {
    for kind in [ProviderKind::Gnome, ProviderKind::Plasma] {
        for invalid in ["missing", "changed", "foreign", "no-uid", "no-pid", "zero-pid"] {
            let mut broker = Broker::pinned();
            match invalid {
                "missing" => broker.missing = true,
                "changed" => broker.replaced = true,
                "foreign" => broker.uid = broker.uid.map(|uid| uid ^ 1),
                "no-uid" => broker.uid = None,
                "no-pid" => broker.pid = None,
                "zero-pid" => broker.pid = Some(0),
                _ => unreachable!(),
            }
            let (manager, _server, state) = fixture(Reply::Success, broker).await;
            let provider = Provider { kind, native_unit: None, logout_command: Vec::new() };
            assert!(delegate(&provider, &manager, "logout").await.is_err(), "{invalid}");
            assert!(state.calls.lock().unwrap().is_empty(), "{invalid}");
        }
    }
}

#[tokio::test]
async fn an_exited_broker_pidfd_cannot_be_replaced_by_a_live_numeric_pid() {
    use std::os::fd::{FromRawFd, OwnedFd};
    let mut child = tokio::process::Command::new("sleep").arg("30").kill_on_drop(true).spawn().unwrap();
    // SAFETY: pidfd_open creates an owned descriptor for this unreaped child.
    let raw = unsafe { libc::syscall(libc::SYS_pidfd_open, child.id().unwrap(), 0) };
    assert!(raw >= 0);
    let descriptor = unsafe { OwnedFd::from_raw_fd(raw as i32) };
    child.kill().await.unwrap();
    child.wait().await.unwrap();
    let mut broker = Broker::pinned();
    broker.descriptor = Some(descriptor);
    let (manager, _server, state) = fixture(Reply::Success, broker).await;
    let provider = Provider { kind: ProviderKind::Gnome, native_unit: None, logout_command: Vec::new() };
    assert!(available(&provider, &manager).await.is_err());
    assert!(delegate(&provider, &manager, "logout").await.is_err());
    assert!(state.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn disabling_during_the_final_owner_query_prevents_native_dispatch() {
    let actions = crate::action_gate::ActionGate::default();
    let mut broker = Broker::pinned();
    broker.disable = Some(actions.clone());
    let (mut manager, _server, state) = fixture(Reply::Success, broker).await;
    manager.actions = actions;
    let provider = Provider { kind: ProviderKind::Gnome, native_unit: None, logout_command: Vec::new() };
    assert!(delegate(&provider, &manager, "logout").await.is_err());
    assert!(state.calls.lock().unwrap().is_empty());
}

#[path = "provider_activation_tests.rs"]
mod activation_tests;

#[path = "provider_capability_tests.rs"]
mod capability_tests;

#[path = "native_attached_tests.rs"]
mod attached_tests;

#[path = "native_full_scope_tests.rs"]
mod scope_tests;

#[path = "kwin_auth_tests.rs"]
mod kwin_tests;
