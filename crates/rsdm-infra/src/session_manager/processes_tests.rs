use std::{os::unix::net::UnixStream, thread};

use zbus::{Connection, zvariant::OwnedObjectPath};

use super::*;

struct Manager { processes: Vec<u32> }

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Manager")]
impl Manager {
    fn get_unit(&self, unit: &str) -> OwnedObjectPath {
        assert_eq!(unit, "native.service");
        OwnedObjectPath::try_from("/org/freedesktop/systemd1/unit/native").unwrap()
    }

    fn get_unit_processes(&self, _unit: &str) -> Vec<(String, u32, String)> {
        self.processes.iter().map(|pid| ("/test".into(), *pid, "native".into())).collect()
    }
}

struct Unit { invocation: Vec<u8>, state: &'static str }

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Unit")]
impl Unit {
    #[zbus(property, name = "InvocationID")]
    fn invocation_id(&self) -> &[u8] { &self.invocation }

    #[zbus(property)]
    fn active_state(&self) -> &str { self.state }
}

fn manager(invocation: Vec<u8>, state: &'static str, processes: Vec<u32>) -> (UserManager, Connection) {
    let (server_socket, client_socket) = UnixStream::pair().unwrap();
    let worker = thread::spawn(move || async_io::block_on(async {
        zbus::connection::Builder::unix_stream(server_socket).p2p()
            .server(zbus::Guid::generate()).unwrap()
            .serve_at("/org/freedesktop/systemd1", Manager { processes }).unwrap()
            .serve_at("/org/freedesktop/systemd1/unit/native", Unit { invocation, state }).unwrap()
            .build().await.unwrap()
    }));
    let connection = async_io::block_on(async {
        zbus::connection::Builder::unix_stream(client_socket).p2p().build().await.unwrap()
    });
    (UserManager::with_connection(connection), worker.join().unwrap())
}

#[test]
fn recovery_accepts_an_empty_native_unit_after_systemd_cleared_its_invocation() {
    let (manager, _server) = manager(vec![0; 16], "inactive", Vec::new());
    stop_invocation(&manager, "native.service", &[1; 16]).unwrap();
}

#[test]
fn a_cleared_invocation_cannot_hide_remaining_processes() {
    let (manager, _server) = manager(vec![0; 16], "inactive", vec![42]);
    assert!(stop_invocation(&manager, "native.service", &[1; 16]).is_err());
}

#[test]
fn a_cleared_invocation_cannot_hide_an_active_unit() {
    let (manager, _server) = manager(vec![0; 16], "active", Vec::new());
    assert!(stop_invocation(&manager, "native.service", &[1; 16]).is_err());
}

#[test]
fn recovery_does_not_stop_a_replacement_native_invocation() {
    let (manager, _server) = manager(vec![2; 16], "inactive", Vec::new());
    assert!(stop_invocation(&manager, "native.service", &[1; 16]).is_err());
}
