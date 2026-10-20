use std::{sync::{Arc, Mutex}, time::Duration};

use super::*;

struct Bus {
    pid: u32,
    calls: Arc<Mutex<Vec<&'static str>>>,
}

#[zbus_macros::interface(name = "org.freedesktop.DBus")]
impl Bus {
    #[zbus(name = "GetConnectionUnixProcessID")]
    fn get_connection_unix_process_id(&self, _name: &str) -> u32 { self.pid }

    fn update_activation_environment(&self, _values: BTreeMap<String, String>) {
        self.calls.lock().unwrap().push("activation");
    }
}

struct Manager(Arc<Mutex<Vec<&'static str>>>);

struct LegacyBus(Arc<Mutex<Vec<&'static str>>>);

#[zbus_macros::interface(name = "org.freedesktop.DBus")]
impl LegacyBus {
    fn update_activation_environment(&self, _values: BTreeMap<String, String>) {
        self.0.lock().unwrap().push("activation");
    }
}

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Manager")]
impl Manager {
    fn set_environment(&self, values: Vec<String>) {
        assert_eq!(values, ["RSDM_SESSION_GENERATION="]);
        self.0.lock().unwrap().push("set");
    }

    fn unset_environment(&self, names: Vec<String>) {
        assert_eq!(names, ["RSDM_SESSION_GENERATION"]);
        self.0.lock().unwrap().push("unset");
    }
}

async fn fixture(pid: u32) -> (crate::bus::UserManager, Connection, Arc<Mutex<Vec<&'static str>>>) {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let bus = Bus { pid, calls: calls.clone() };
    let manager = Manager(calls.clone());
    let (server_socket, client_socket) = tokio::net::UnixStream::pair().unwrap();
    let server = tokio::spawn(async move {
        zbus::connection::Builder::unix_stream(server_socket.into_std().unwrap()).p2p().server(zbus::Guid::generate())
            .serve_at("/org/freedesktop/DBus", bus)
            .serve_at("/org/freedesktop/systemd1", manager).build().await.unwrap()
    });
    let connection = zbus::connection::Builder::unix_stream(client_socket.into_std().unwrap()).p2p().build().await.unwrap();
    (crate::bus::UserManager::with_connection(connection), server.await.unwrap(), calls)
}

struct BrokerProcess {
    child: tokio::process::Child,
    directory: std::path::PathBuf,
}

impl BrokerProcess {
    fn start() -> Self {
        let directory = std::env::temp_dir().join(format!("rsdm-bus-{}", zbus::Guid::generate()));
        std::fs::create_dir(&directory).unwrap();
        let shell = std::env::split_paths(&std::env::var_os("PATH").unwrap())
            .map(|directory| directory.join("bash")).find(|path| path.is_file()).unwrap();
        let executable = directory.join("dbus-broker");
        std::fs::copy(shell, &executable).unwrap();
        let child = tokio::process::Command::new(executable).args(["-c", "read -r -t 30 value"])
            .stdin(std::process::Stdio::piped()).kill_on_drop(true).spawn().unwrap();
        Self { child, directory }
    }
}

impl Drop for BrokerProcess {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.directory); }
}

#[tokio::test]
async fn broker_cleanup_awaits_the_manager_instead_of_scheduling_a_late_activation_write() {
    let mut broker = BrokerProcess::start();
    let (manager, _server, calls) = fixture(broker.child.id().unwrap()).await;
    crate::env::clear_names(&manager, &["RSDM_SESSION_GENERATION".into()]).await.unwrap();
    assert_eq!(*calls.lock().unwrap(), ["set", "unset"]);
    broker.child.kill().await.unwrap();
    broker.child.wait().await.unwrap();
}

#[tokio::test]
async fn other_bus_implementations_keep_the_standard_activation_update() {
    let (manager, _server, calls) = fixture(std::process::id()).await;
    crate::env::clear_names(&manager, &["RSDM_SESSION_GENERATION".into()]).await.unwrap();
    assert_eq!(*calls.lock().unwrap(), ["activation", "unset"]);
}

#[tokio::test]
async fn a_bus_without_pid_lookup_keeps_the_standard_activation_update() {
    let (manager, server, calls) = fixture(std::process::id()).await;
    server.object_server().remove::<Bus, _>("/org/freedesktop/DBus").await.unwrap();
    server.object_server().at("/org/freedesktop/DBus", LegacyBus(calls.clone())).await.unwrap();
    crate::env::clear_names(&manager, &["RSDM_SESSION_GENERATION".into()]).await.unwrap();
    assert_eq!(*calls.lock().unwrap(), ["activation", "unset"]);
}

#[tokio::test]
async fn expired_cleanup_does_not_submit_an_activation_or_manager_write() {
    let (manager, _server, calls) = fixture(std::process::id()).await;
    manager.deadline.set(1);
    assert!(crate::env::clear_names(&manager, &["RSDM_SESSION_GENERATION".into()]).await.is_err());
    assert!(calls.lock().unwrap().is_empty());
    assert!(manager.deadline.remaining(Duration::MAX).is_err());
}
