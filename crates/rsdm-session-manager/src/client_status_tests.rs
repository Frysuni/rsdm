//! Wire compatibility and service activation boundaries on a private bus.

use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};

use super::*;
use zbus::{message::Header, object_server::Interface};

#[derive(Default)]
struct Calls { owners: AtomicUsize, details: AtomicUsize, legacy: AtomicUsize, activations: AtomicUsize }

struct Bus { calls: Arc<Calls>, present: bool }

#[zbus_macros::interface(name = "org.freedesktop.DBus")]
impl Bus {
    fn get_name_owner(&self, name: &str) -> zbus::fdo::Result<String> {
        assert_eq!(name, control::BUS_NAME);
        self.calls.owners.fetch_add(1, Ordering::SeqCst);
        if self.present { Ok(":1.42".into()) } else { Err(zbus::fdo::Error::NameHasNoOwner("absent".into())) }
    }

    fn start_service_by_name(&self, _name: &str, _flags: u32) -> u32 {
        self.calls.activations.fetch_add(1, Ordering::SeqCst);
        1
    }
}

struct Legacy(Arc<Calls>);

#[zbus_macros::interface(name = "org.rsdm.Session1")]
impl Legacy {
    fn status(&self, #[zbus(header)] header: Header<'_>) -> SessionStatus {
        assert_eq!(header.destination().unwrap().as_str(), ":1.42");
        self.0.legacy.fetch_add(1, Ordering::SeqCst);
        old_status()
    }
}

struct Current { calls: Arc<Calls>, denied: bool }

#[zbus_macros::interface(name = "org.rsdm.Session1")]
impl Current {
    fn status_details(&self, #[zbus(header)] header: Header<'_>) -> zbus::fdo::Result<SessionStatusDetails> {
        assert_eq!(header.destination().unwrap().as_str(), ":1.42");
        self.calls.details.fetch_add(1, Ordering::SeqCst);
        if self.denied { return Err(zbus::fdo::Error::AccessDenied("another login".into())); }
        let mut details = SessionStatusDetails::legacy(old_status());
        details.capabilities.push(crate::status::Capability::new("xsmp", "unavailable", "not built"));
        Ok(details)
    }

    fn status(&self) -> SessionStatus { self.calls.legacy.fetch_add(1, Ordering::SeqCst); old_status() }
}

struct WrongWire(Arc<Calls>);

#[zbus_macros::interface(name = "org.rsdm.Session1")]
impl WrongWire {
    fn status_details(&self) -> bool { self.0.details.fetch_add(1, Ordering::SeqCst); true }
    fn status(&self) -> SessionStatus { self.0.legacy.fetch_add(1, Ordering::SeqCst); old_status() }
}

fn old_status() -> SessionStatus {
    SessionStatus { generation: "generation".into(), login_session_id: "login".into(), desktop_entry_id: "desktop".into(),
        provider: "partial".into(), phase: "running".into(), xsmp_available: false,
        apps: vec![("app.service".into(), "command".into(), 5, "registered".into())] }
}

async fn connect<T: Interface>(calls: Arc<Calls>, present: bool, interface: T) -> (Connection, Connection) {
    let (server, client) = tokio::net::UnixStream::pair().unwrap();
    let server = tokio::spawn(async move {
        zbus::connection::Builder::unix_stream(server).p2p().server(zbus::Guid::generate()).unwrap()
            .serve_at("/org/freedesktop/DBus", Bus { calls, present }).unwrap()
            .serve_at(control::OBJECT_PATH, interface).unwrap().build().await.unwrap()
    });
    let client = zbus::connection::Builder::unix_stream(client).p2p().build().await.unwrap();
    (server.await.unwrap(), client)
}

fn assert_calls(calls: &Calls, details: usize, legacy: usize) {
    assert_eq!(calls.owners.load(Ordering::SeqCst), 1);
    assert_eq!(calls.details.load(Ordering::SeqCst), details);
    assert_eq!(calls.legacy.load(Ordering::SeqCst), legacy);
    assert_eq!(calls.activations.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn details_are_requested_from_the_existing_unique_owner() {
    let calls = Arc::new(Calls::default());
    let (_server, client) = connect(calls.clone(), true, Current { calls: calls.clone(), denied: false }).await;
    let details = query_details(&client).await.unwrap();
    assert_eq!(details.state, "partial");
    assert_eq!(details.status.apps, old_status().apps);
    assert_eq!(details.capabilities[0].reason, "not built");
    assert_calls(&calls, 1, 0);
}

#[tokio::test]
async fn only_unknown_method_falls_back_to_legacy_status_on_the_same_owner() {
    let calls = Arc::new(Calls::default());
    let (_server, client) = connect(calls.clone(), true, Legacy(calls.clone())).await;
    let details = query_details(&client).await.unwrap();
    assert_eq!(details.state, "partial");
    assert_eq!(details.status.apps, old_status().apps);
    assert!(details.capabilities.is_empty());
    assert!(details.limitations[0].contains("does not report"));
    assert_calls(&calls, 0, 1);
}

#[tokio::test]
async fn denied_or_malformed_details_do_not_fall_back() {
    let calls = Arc::new(Calls::default());
    let (_server, client) = connect(calls.clone(), true, Current { calls: calls.clone(), denied: true }).await;
    assert!(query_details(&client).await.unwrap_err().to_string().contains("AccessDenied"));
    assert_calls(&calls, 1, 0);
    let calls = Arc::new(Calls::default());
    let (_server, client) = connect(calls.clone(), true, WrongWire(calls.clone())).await;
    assert!(query_details(&client).await.is_err());
    assert_calls(&calls, 1, 0);
}

#[tokio::test]
async fn absent_owner_is_inactive_without_any_coordinator_call_or_activation() {
    let calls = Arc::new(Calls::default());
    let (_server, client) = connect(calls.clone(), false, Legacy(calls.clone())).await;
    let details = query_details(&client).await.unwrap();
    assert!(details.enabled);
    assert_eq!(details.state, "inactive");
    assert!(details.status.generation.is_empty());
    assert_calls(&calls, 0, 0);
}

#[test]
fn legacy_wire_signatures_are_unchanged() {
    use zbus::zvariant::Type;
    assert_eq!(SessionStatus::SIGNATURE.to_string(), "(sssssba(ssts))");
    assert_eq!(crate::StopOutcome::SIGNATURE.to_string(), "(sass)");
}
