//! Replaced compositor units must not authorize teardown of shared targets.

use super::*;
use std::sync::Mutex;
use zbus::zvariant::OwnedObjectPath;

struct Manager { stops: Arc<Mutex<Vec<String>>> }

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Manager")]
impl Manager {
    fn get_unit(&self, _unit: &str) -> OwnedObjectPath {
        OwnedObjectPath::try_from("/compositor").unwrap()
    }

    fn stop_unit(&self, unit: &str, _mode: &str) -> zbus::fdo::Result<OwnedObjectPath> {
        self.stops.lock().unwrap().push(unit.into());
        Err(zbus::fdo::Error::Failed("unexpected stop of retained infrastructure".into()))
    }

    fn unref_unit(&self, _unit: &str) {}

    #[zbus(property)]
    fn environment(&self) -> Vec<String> { vec![format!("RSDM_SESSION_GENERATION={GENERATION}")] }
}

struct Unit { invocation: Vec<u8> }

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Unit")]
impl Unit {
    #[zbus(property, name = "InvocationID")]
    fn invocation_id(&self) -> &[u8] { &self.invocation }

    #[zbus(property)]
    fn active_state(&self) -> &str { "active" }
}

async fn fixture(captured: Vec<u8>, current: Vec<u8>) -> (Fixture, Arc<Mutex<Vec<String>>>) {
    let mut fixture = Fixture::new().await;
    let stops = Arc::new(Mutex::new(Vec::new()));
    fixture._server.object_server().at("/org/freedesktop/systemd1", Manager { stops: stops.clone() }).await.unwrap();
    fixture._server.object_server().at("/compositor", Unit { invocation: current }).await.unwrap();
    fixture.coordinator.provider.kind = ProviderKind::Managed;
    fixture.coordinator.record.provider = "managed".into();
    fixture.coordinator.record.compositor_unit = Some("compositor.service".into());
    fixture.coordinator.record.compositor_invocation = captured;
    fixture.coordinator.record.owns_targets = true;
    fixture.coordinator.runtime.save_session(&fixture.coordinator.record).unwrap();
    (fixture, stops)
}

#[tokio::test]
async fn logout_checks_compositor_ownership_before_stopping_shared_infrastructure() {
    for (captured, current) in [(vec![1; 16], vec![2; 16]), (vec![0; 16], vec![0; 16]), (Vec::new(), vec![1; 16])] {
        let (fixture, stops) = fixture(captured, current).await;
        let coordinator = &fixture.coordinator;
        let mut process = SessionProcess::launch(&["unused".into()], &[], &fixture.directory,
            &coordinator.provider, "compositor.service".into()).unwrap();
        process.invocation = coordinator.record.compositor_invocation.clone();
        process.booting = false;
        let result = tokio::time::timeout(Duration::from_secs(2), finish(&coordinator.manager,
            &coordinator.runtime, &coordinator.record, &coordinator.provider, &mut process, fixture.directory.clone())).await.unwrap();
        assert!(result.is_err());
        let stops = stops.lock().unwrap();
        assert!(stops.is_empty(), "unexpected stops: {stops:?}");
    }
}

#[tokio::test]
async fn recovery_checks_compositor_ownership_before_stopping_shared_infrastructure() {
    let (fixture, stops) = fixture(vec![1; 16], vec![2; 16]).await;
    let result = tokio::time::timeout(Duration::from_secs(2), crate::cleanup::recover(
        &fixture.coordinator.manager, &fixture.coordinator.runtime)).await.unwrap();
    assert!(result.is_err());
    let stops = stops.lock().unwrap();
    assert!(stops.is_empty(), "unexpected stops: {stops:?}");
}
