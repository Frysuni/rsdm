use std::{os::unix::net::UnixStream, sync::{Arc, atomic::{AtomicUsize, Ordering}}, thread};

use zbus::{Connection, zvariant::OwnedObjectPath};

use super::*;

struct Manager { processes: Vec<u32>, queries: Arc<AtomicUsize> }

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Manager")]
impl Manager {
    fn get_unit(&self, unit: &str) -> OwnedObjectPath {
        assert_eq!(unit, "native.service");
        OwnedObjectPath::try_from("/org/freedesktop/systemd1/unit/native").unwrap()
    }

    fn get_unit_processes(&self, _unit: &str) -> Vec<(String, u32, String)> {
        self.queries.fetch_add(1, Ordering::SeqCst);
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

    #[zbus(property)]
    fn control_group(&self) -> &str { "/rsdm-test-app" }
}

struct Service;

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Service")]
impl Service {
    #[zbus(property, name = "ControlPID")]
    fn control_pid(&self) -> u32 { 0 }
}

fn manager(invocation: Vec<u8>, state: &'static str, processes: Vec<u32>) -> (UserManager, Connection, Arc<AtomicUsize>) {
    let queries = Arc::new(AtomicUsize::new(0));
    let shared = queries.clone();
    let (server_socket, client_socket) = UnixStream::pair().unwrap();
    let worker = thread::spawn(move || async_io::block_on(async {
        zbus::connection::Builder::unix_stream(server_socket).p2p()
            .server(zbus::Guid::generate()).unwrap()
            .serve_at("/org/freedesktop/systemd1", Manager { processes, queries: shared }).unwrap()
            .serve_at("/org/freedesktop/systemd1/unit/native", Unit { invocation, state }).unwrap()
            .serve_at("/org/freedesktop/systemd1/unit/native", Service).unwrap()
            .build().await.unwrap()
    }));
    let connection = async_io::block_on(async {
        zbus::connection::Builder::unix_stream(client_socket).p2p().build().await.unwrap()
    });
    (UserManager::with_connection(connection), worker.join().unwrap(), queries)
}

#[test]
fn recovery_accepts_an_empty_native_unit_after_systemd_cleared_its_invocation() {
    let (manager, _server, _) = manager(vec![0; 16], "inactive", Vec::new());
    stop_invocation(&manager, "native.service", &[1; 16]).unwrap();
}

#[test]
fn a_cleared_invocation_cannot_hide_remaining_processes() {
    let (manager, _server, _) = manager(vec![0; 16], "inactive", vec![42]);
    assert!(stop_invocation(&manager, "native.service", &[1; 16]).is_err());
}

#[test]
fn a_cleared_invocation_cannot_hide_an_active_unit() {
    let (manager, _server, _) = manager(vec![0; 16], "active", Vec::new());
    assert!(stop_invocation(&manager, "native.service", &[1; 16]).is_err());
}

#[test]
fn recovery_does_not_stop_a_replacement_native_invocation() {
    let (manager, _server, _) = manager(vec![2; 16], "inactive", Vec::new());
    assert!(stop_invocation(&manager, "native.service", &[1; 16]).is_err());
}

#[test]
fn process_signaling_does_not_repeat_full_cgroup_snapshots() {
    let (manager, _server, queries) = manager(vec![1; 16], "active", vec![std::process::id(); 128]);
    let app = AppRecord { unit: "native.service".into(), invocation_id: vec![1; 16],
        policy: rsdm_core::domain::ShutdownPolicy::default(), deadline_usec: None, quit_started: false };
    signal_app(&manager, &app, 0).unwrap();
    assert_eq!(queries.load(Ordering::SeqCst), 1);
}

#[test]
fn process_signaling_rejects_a_replacement_before_taking_a_snapshot() {
    let (manager, _server, queries) = manager(vec![2; 16], "active", vec![std::process::id()]);
    let app = AppRecord { unit: "native.service".into(), invocation_id: vec![1; 16],
        policy: rsdm_core::domain::ShutdownPolicy::default(), deadline_usec: None, quit_started: false };
    assert!(signal_app(&manager, &app, 0).is_err());
    assert_eq!(queries.load(Ordering::SeqCst), 0);
}

#[test]
fn membership_matches_only_the_systemd_hierarchy_and_exact_group_or_descendants() {
    let group = "/user.slice/app.service";
    for membership in [
        "0::/user.slice/app.service", "0::/user.slice/app.service/child",
        "1:name=systemd:/user.slice/app.service",
    ] {
        assert!(belongs_to_cgroup(membership, group));
    }
    for membership in [
        "0::/user.slice/app.service-other", "0::/other/app.service",
        "2:memory:/user.slice/app.service", "malformed",
    ] {
        assert!(!belongs_to_cgroup(membership, group));
    }
}
