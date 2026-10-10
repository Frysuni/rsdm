//! Native shutdown methods are submitted once, including when a reply is lost.

use std::{os::unix::net::UnixStream, sync::{Arc, Mutex}, time::Duration};

use super::*;

#[derive(Clone, Copy)]
enum Reply { Success, Lost, Pending }

struct State {
    calls: Mutex<Vec<String>>,
    accepted: tokio::sync::Notify,
    reply: Reply,
}

impl State {
    async fn request(&self, method: &str) -> zbus::fdo::Result<()> {
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
    async fn logout(&self, mode: u32) -> zbus::fdo::Result<()> {
        assert_eq!(mode, 0);
        self.0.request("Logout").await
    }

    async fn reboot(&self) -> zbus::fdo::Result<()> { self.0.request("Reboot").await }

    async fn shutdown(&self) -> zbus::fdo::Result<()> { self.0.request("Shutdown").await }
}

struct Plasma(Arc<State>);

#[zbus_macros::interface(name = "org.kde.LogoutPrompt")]
impl Plasma {
    #[zbus(name = "promptLogout")]
    async fn prompt_logout(&self) -> zbus::fdo::Result<()> { self.0.request("promptLogout").await }

    #[zbus(name = "promptReboot")]
    async fn prompt_reboot(&self) -> zbus::fdo::Result<()> { self.0.request("promptReboot").await }

    #[zbus(name = "promptShutDown")]
    async fn prompt_shut_down(&self) -> zbus::fdo::Result<()> { self.0.request("promptShutDown").await }
}

async fn fixture(reply: Reply) -> (UserManager, zbus::Connection, Arc<State>) {
    let state = Arc::new(State { calls: Mutex::new(Vec::new()), accepted: tokio::sync::Notify::new(), reply });
    let gnome = Gnome(state.clone());
    let plasma = Plasma(state.clone());
    let (server_socket, client_socket) = UnixStream::pair().unwrap();
    let server = tokio::spawn(async move {
        zbus::connection::Builder::unix_stream(server_socket).p2p()
            .server(zbus::Guid::generate()).unwrap()
            .serve_at("/org/gnome/SessionManager", gnome).unwrap()
            .serve_at("/LogoutPrompt", plasma).unwrap().build().await.unwrap()
    });
    let connection = zbus::connection::Builder::unix_stream(client_socket).p2p().build().await.unwrap();
    (UserManager::with_connection(connection), server.await.unwrap(), state)
}

#[tokio::test]
async fn native_methods_are_dispatched_once_with_and_without_a_reply() {
    for reply in [Reply::Success, Reply::Lost] {
        let (manager, _server, state) = fixture(reply).await;
        for kind in [ProviderKind::Gnome, ProviderKind::Plasma] {
            let provider = Provider { kind, native_unit: None, logout_command: Vec::new() };
            for action in ["logout", "reboot", "poweroff"] {
                let result = provider.delegate(&manager, action).await;
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
    let (manager, _server, state) = fixture(Reply::Success).await;
    manager.deadline.set(1);
    let provider = Provider { kind: ProviderKind::Gnome, native_unit: None, logout_command: Vec::new() };
    assert!(provider.delegate(&manager, "logout").await.is_err());
    assert!(state.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn revising_the_deadline_interrupts_a_pending_native_reply() {
    let (manager, _server, state) = fixture(Reply::Pending).await;
    let deadline = manager.deadline.clone();
    let provider = Provider { kind: ProviderKind::Gnome, native_unit: None, logout_command: Vec::new() };
    let worker = tokio::spawn(async move { provider.delegate(&manager, "logout").await });
    tokio::time::timeout(Duration::from_secs(2), state.accepted.notified()).await.unwrap();
    deadline.set(1);
    let result = tokio::time::timeout(Duration::from_secs(1), worker).await.unwrap().unwrap();
    assert!(matches!(result, Err(SessionError::Bus(error)) if crate::bus::retryable_error(&error)));
    assert_eq!(*state.calls.lock().unwrap(), ["Logout"]);
}

#[tokio::test]
async fn unsupported_actions_and_providers_do_not_dispatch() {
    let (manager, _server, state) = fixture(Reply::Success).await;
    let provider = Provider { kind: ProviderKind::Gnome, native_unit: None, logout_command: Vec::new() };
    assert!(provider.delegate(&manager, "suspend").await.is_err());
    let provider = Provider { kind: ProviderKind::Managed, native_unit: None, logout_command: Vec::new() };
    assert!(provider.delegate(&manager, "logout").await.is_err());
    assert!(state.calls.lock().unwrap().is_empty());
}
