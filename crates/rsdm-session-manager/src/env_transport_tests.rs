use std::{collections::HashMap, sync::{Arc, Mutex}};

use super::*;
use crate::bus::UserManager;

#[derive(Debug, PartialEq, Eq)]
enum Effect {
    Activation(Vec<(String, String)>),
    Unset(Vec<String>),
}

type Effects = Arc<Mutex<Vec<Effect>>>;

struct Manager(Effects);

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Manager")]
impl Manager {
    #[zbus(property)]
    fn environment(&self) -> Vec<String> {
        vec!["WAYLAND_DISPLAY=wayland-new".into(), "DISPLAY=:old".into(), "PATH=/bin".into()]
    }

    fn unset_environment(&self, names: Vec<String>) {
        self.0.lock().unwrap().push(Effect::Unset(names));
    }
}

struct Activation {
    effects: Effects,
    reject: bool,
}

#[zbus_macros::interface(name = "org.freedesktop.DBus")]
impl Activation {
    fn update_activation_environment(&self, values: HashMap<String, String>) -> zbus::fdo::Result<()> {
        if self.reject {
            return Err(zbus::fdo::Error::Failed("activation update rejected".into()));
        }
        let mut values = values.into_iter().collect::<Vec<_>>();
        values.sort();
        self.effects.lock().unwrap().push(Effect::Activation(values));
        Ok(())
    }
}

async fn connect(reject: bool) -> (UserManager, zbus::Connection, Effects) {
    let (server_socket, client_socket) = tokio::net::UnixStream::pair().unwrap();
    let effects = Effects::default();
    let manager = Manager(effects.clone());
    let activation = Activation { effects: effects.clone(), reject };
    let server = tokio::spawn(async move {
        zbus::connection::Builder::unix_stream(server_socket.into_std().unwrap())
            .p2p().server(zbus::Guid::generate())
            .serve_at("/org/freedesktop/systemd1", manager)
            .serve_at("/org/freedesktop/DBus", activation)
            .build().await.unwrap()
    });
    let connection = zbus::connection::Builder::unix_stream(client_socket.into_std().unwrap()).p2p().build().await.unwrap();
    (UserManager::with_connection(connection), server.await.unwrap(), effects)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cleanup_clears_activation_before_unsetting_owned_manager_values() {
    let (manager, _server, effects) = connect(false).await;
    let previous = vec![
        ("WAYLAND_DISPLAY".into(), "wayland-old".into()),
        ("DISPLAY".into(), ":old".into()),
        ("PATH".into(), "/bin".into()),
    ];
    clear_owned(&manager, &previous).await.unwrap();
    assert_eq!(*effects.lock().unwrap(), [
        Effect::Activation(vec![("DISPLAY".into(), String::new())]),
        Effect::Unset(vec!["DISPLAY".into()]),
    ]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_activation_update_preserves_manager_ownership_for_recovery() {
    let (manager, _server, effects) = connect(true).await;
    assert!(clear_names(&manager, &["DISPLAY".into()]).await.is_err());
    assert!(effects.lock().unwrap().is_empty());
}
