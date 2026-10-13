//! Recovery cannot turn a partial native desktop into owned infrastructure.

use super::*;
use std::sync::Mutex;
use zbus::zvariant::OwnedObjectPath;

struct Manager { mutations: Arc<Mutex<Vec<String>>> }

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Manager")]
impl Manager {
    fn stop_unit(&self, unit: &str, _mode: &str) -> zbus::fdo::Result<OwnedObjectPath> {
        self.mutations.lock().unwrap().push(format!("stop {unit}"));
        Err(zbus::fdo::Error::Failed("unexpected native infrastructure stop".into()))
    }

    fn unref_unit(&self, unit: &str) { self.mutations.lock().unwrap().push(format!("unref {unit}")); }

    fn unset_environment(&self, _names: Vec<String>) {
        self.mutations.lock().unwrap().push("unset environment".into());
    }

    #[zbus(property)]
    fn environment(&self) -> Vec<String> { vec![format!("RSDM_SESSION_GENERATION={GENERATION}")] }
}

async fn fixture(kind: ProviderKind) -> (Fixture, Arc<Mutex<Vec<String>>>) {
    let mut fixture = Fixture::new().await;
    let mutations = Arc::new(Mutex::new(Vec::new()));
    fixture._server.object_server().at("/org/freedesktop/systemd1", Manager { mutations: mutations.clone() }).await.unwrap();
    fixture._server.object_server().at("/org/freedesktop/DBus", super::attached::NoNative).await.unwrap();
    let coordinator = &mut fixture.coordinator;
    coordinator.provider.kind = kind;
    coordinator.record.provider = coordinator.provider.name().into();
    coordinator.record.owns_targets = true;
    coordinator.record.exported_environment = vec![("RSDM_SESSION_GENERATION".into(), GENERATION.into())];
    coordinator.runtime.save_session(&coordinator.record).unwrap();
    (fixture, mutations)
}

#[tokio::test]
async fn standalone_native_recovery_preserves_unverified_infrastructure() {
    for kind in [ProviderKind::Gnome, ProviderKind::Plasma] {
        let (fixture, mutations) = fixture(kind).await;
        let coordinator = &fixture.coordinator;
        let _ = crate::cleanup::recover(&coordinator.manager, &coordinator.runtime).await;
        assert!(mutations.lock().unwrap().is_empty());
        let record = coordinator.runtime.session().unwrap();
        assert!(record.owns_targets);
        assert_eq!(record.exported_environment, coordinator.record.exported_environment);
    }
}

#[tokio::test]
async fn failed_native_coordinator_retains_its_launcher_when_control_is_unverified() {
    for kind in [ProviderKind::Gnome, ProviderKind::Plasma] {
        let (mut fixture, mutations) = fixture(kind).await;
        let provider = Provider { kind: ProviderKind::Managed, native_unit: None, logout_command: Vec::new() };
        let mut process = SessionProcess::launch(&["unused".into()], &[], &fixture.directory,
            &provider, "unused.service".into()).unwrap();
        process.booting = false;
        fixture.coordinator.process = Some(process);
        fixture.coordinator.recover().await;
        assert!(fixture.coordinator.retain_launcher);
        assert!(fixture.coordinator.process.is_some());
        assert!(mutations.lock().unwrap().is_empty());
    }
}
