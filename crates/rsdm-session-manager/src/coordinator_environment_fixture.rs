use std::sync::{Arc, Mutex, atomic::{AtomicUsize, Ordering}};

use super::super::*;

#[derive(Default)]
pub(super) struct State {
    pub(super) values: Mutex<Vec<String>>,
    pub(super) writes: Mutex<Vec<Vec<String>>>,
    pub(super) activations: AtomicUsize,
    pub(super) reads: AtomicUsize,
    pub(super) fail_activation: std::sync::atomic::AtomicBool,
    pub(super) fail_manager: std::sync::atomic::AtomicBool,
    pub(super) ready: std::sync::atomic::AtomicBool,
    pause: Mutex<Option<Pause>>,
}

struct Pause {
    entered: Arc<tokio::sync::Notify>,
    resume: Arc<tokio::sync::Notify>,
}

pub(super) fn pause(state: &State) -> (Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>) {
    let entered = Arc::new(tokio::sync::Notify::new());
    let resume = Arc::new(tokio::sync::Notify::new());
    *state.pause.lock().unwrap() = Some(Pause { entered: entered.clone(), resume: resume.clone() });
    (entered, resume)
}

struct Manager(Arc<State>);

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Manager")]
impl Manager {
    #[zbus(property)]
    fn environment(&self) -> Vec<String> { self.0.values.lock().unwrap().clone() }

    async fn set_environment(&self, values: Vec<String>) -> zbus::fdo::Result<()> {
        self.0.writes.lock().unwrap().push(values.clone());
        let pause = self.0.pause.lock().unwrap().take();
        if let Some(pause) = pause {
            pause.entered.notify_one();
            pause.resume.notified().await;
        }
        if self.0.fail_manager.load(Ordering::SeqCst) {
            return Err(zbus::fdo::Error::Failed("manager publication rejected".into()));
        }
        let mut current = self.0.values.lock().unwrap();
        for value in values {
            let name = value.split_once('=').unwrap().0;
            current.retain(|entry| entry.split_once('=').unwrap().0 != name);
            current.push(value);
        }
        Ok(())
    }

    fn unset_environment(&self, names: Vec<String>) {
        self.0.values.lock().unwrap().retain(|entry| {
            !names.iter().any(|name| entry.split_once('=').unwrap().0 == name)
        });
    }

    fn get_unit(&self, _unit: &str) -> zbus::fdo::Result<zbus::OwnedObjectPath> {
        self.0.reads.fetch_add(1, Ordering::SeqCst);
        if self.0.ready.load(Ordering::SeqCst) { return Ok(zbus::OwnedObjectPath::try_from("/anchor").unwrap()); }
        Err(zbus::fdo::Error::Failed("unexpected readiness query".into()))
    }
}

struct Unit;

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Unit")]
impl Unit {
    #[zbus(property)]
    fn active_state(&self) -> &str { "active" }
}

struct Bus(Arc<State>);

#[zbus_macros::interface(name = "org.freedesktop.DBus")]
impl Bus {
    fn update_activation_environment(&self, _values: std::collections::BTreeMap<String, String>) -> zbus::fdo::Result<()> {
        self.0.activations.fetch_add(1, Ordering::SeqCst);
        if self.0.fail_activation.load(Ordering::SeqCst) {
            return Err(zbus::fdo::Error::Failed("activation publication rejected".into()));
        }
        Ok(())
    }
}

pub(super) async fn install(fixture: &Fixture) -> Arc<State> {
    let state = Arc::new(State::default());
    fixture._server.object_server().at("/org/freedesktop/systemd1", Manager(state.clone())).await.unwrap();
    fixture._server.object_server().at("/org/freedesktop/DBus", Bus(state.clone())).await.unwrap();
    fixture._server.object_server().at("/anchor", Unit).await.unwrap();
    state
}
