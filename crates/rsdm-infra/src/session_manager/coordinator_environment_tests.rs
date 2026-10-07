use std::sync::{Arc, Mutex, atomic::{AtomicUsize, Ordering}};

use super::*;
use crate::session_manager::control::{Reply, Request};

#[derive(Default)]
struct State {
    values: Mutex<Vec<String>>,
    writes: Mutex<Vec<Vec<String>>>,
    activations: AtomicUsize,
    reads: AtomicUsize,
}

struct Manager(Arc<State>);

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Manager")]
impl Manager {
    #[zbus(property)]
    fn environment(&self) -> Vec<String> { self.0.values.lock().unwrap().clone() }

    fn set_environment(&self, values: Vec<String>) {
        self.0.writes.lock().unwrap().push(values.clone());
        let mut current = self.0.values.lock().unwrap();
        for value in values {
            let name = value.split_once('=').unwrap().0;
            current.retain(|entry| entry.split_once('=').unwrap().0 != name);
            current.push(value);
        }
    }

    fn get_unit(&self, _unit: &str) -> zbus::fdo::Result<zbus::zvariant::OwnedObjectPath> {
        self.0.reads.fetch_add(1, Ordering::SeqCst);
        Err(zbus::fdo::Error::Failed("unexpected readiness query".into()))
    }
}

struct Bus(Arc<State>);

#[zbus_macros::interface(name = "org.freedesktop.DBus")]
impl Bus {
    fn update_activation_environment(&self, _values: std::collections::BTreeMap<String, String>) {
        self.0.activations.fetch_add(1, Ordering::SeqCst);
    }
}

fn install(fixture: &Fixture) -> Arc<State> {
    let state = Arc::new(State::default());
    async_io::block_on(async {
        fixture._server.object_server().at("/org/freedesktop/systemd1", Manager(state.clone())).await.unwrap();
        fixture._server.object_server().at("/org/freedesktop/DBus", Bus(state.clone())).await.unwrap();
    });
    state
}

#[test]
fn repeated_finalize_updates_environment_without_reactivating_a_running_session() {
    let mut fixture = Fixture::new();
    let state = install(&fixture);
    for value in ["wayland-1", "wayland-2"] {
        let (reply, response) = async_channel::bounded(1);
        fixture.coordinator.request(Request::Finalize {
            generation: GENERATION.into(), environment: vec![("WAYLAND_DISPLAY".into(), value.into())],
            reply: Reply::for_test(reply),
        }).unwrap();
        assert!(response.try_recv().unwrap().is_ok());
        assert_eq!(fixture.coordinator.lifecycle.phase, SessionPhase::Running);
        assert_eq!(fixture.coordinator.workers, 0);
        assert!(!fixture.coordinator.ready_busy);
        assert!(fixture.coordinator.finalize_replies.is_empty());
        assert!(fixture.coordinator.runtime.session().unwrap().exported_environment.contains(&(
            "WAYLAND_DISPLAY".into(), value.into(),
        )));
    }
    assert_eq!(*state.writes.lock().unwrap(), [vec!["WAYLAND_DISPLAY=wayland-1"], vec!["WAYLAND_DISPLAY=wayland-2"]]);
    assert_eq!(state.activations.load(Ordering::SeqCst), 2);
    assert_eq!(state.reads.load(Ordering::SeqCst), 0);
}
