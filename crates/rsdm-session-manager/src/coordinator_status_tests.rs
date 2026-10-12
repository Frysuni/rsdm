use std::os::unix::fs::MetadataExt;

use super::*;
use crate::{app_dispatch::{Dispatch, Method, State}, app_result::{Outcome, Preparation}, coordinator::MAX_COORDINATOR_WORKERS};

struct PendingBus { entered: Arc<tokio::sync::Notify>, release: Arc<tokio::sync::Notify> }

#[zbus_macros::interface(name = "org.freedesktop.DBus")]
impl PendingBus {
    async fn name_has_owner(&self, name: &str) -> bool {
        assert_eq!(name, "org.gnome.SessionManager");
        self.entered.notify_one();
        self.release.notified().await;
        false
    }
}

fn prepared(outcome: Outcome, method: Method) -> crate::runtime::AppRecord {
    let mut app = crate::helper_start_fixture::app();
    app.preparation = Some(Preparation { transaction_id: Some("transaction".into()), outcome, escalated: false });
    app.dispatch = Some(Dispatch { transaction_id: Some("transaction".into()), invocation_id: app.invocation_id.clone(), method, state: State::Sent });
    app
}

async fn query(fixture: &mut Fixture) -> crate::SessionStatusDetails {
    let (reply, mut result) = tokio::sync::mpsc::channel(1);
    fixture.coordinator.request_status_details(crate::control::Reply::for_test(reply));
    let work = tokio::time::timeout(Duration::from_secs(3), fixture.coordinator.work.recv()).await.unwrap().unwrap();
    fixture.coordinator.completed(work).await.unwrap();
    result.try_recv().unwrap().unwrap()
}

#[tokio::test]
async fn full_status_retains_actual_dispatch_and_archived_results_without_rewriting_records() {
    let mut fixture = Fixture::new().await;
    let active = prepared(Outcome::Cancelled, Method::WindowClose);
    fixture.coordinator.runtime.save_app(&active).unwrap();
    let mut archive = prepared(Outcome::Clean, Method::Command);
    fixture.coordinator.runtime.archive_app(&archive).unwrap();
    archive.invocation_id = vec![2; 16];
    archive.dispatch.as_mut().unwrap().invocation_id = archive.invocation_id.clone();
    fixture.coordinator.runtime.archive_app(&archive).unwrap();
    let session = fixture.directory.join("session.toml");
    let app = fixture.directory.join("apps").join(format!("{}.toml", active.unit));
    let before = (fs::read(&session).unwrap(), fs::metadata(&session).unwrap().ino(), fs::read(&app).unwrap(), fs::metadata(&app).unwrap().ino());
    let details = query(&mut fixture).await;
    assert_eq!(details.state, "full");
    assert!(details.limitations.is_empty());
    assert_eq!(details.status.apps[0].1, "window-close");
    assert_eq!(details.preparations.len(), 2);
    assert_eq!(details.preparations[0].result, "cancelled");
    assert_eq!(details.preparations[1].result, "clean");
    assert_eq!(details.preparations[1].method, "command");
    assert_eq!(details.preparations[0].transaction_id, "transaction");
    assert!(!details.preparations[0].escalated);
    assert!(details.capabilities.iter().any(|capability| capability.name == "window-close" && capability.state == "unavailable"));
    assert_eq!(before, (fs::read(&session).unwrap(), fs::metadata(&session).unwrap().ino(), fs::read(&app).unwrap(), fs::metadata(&app).unwrap().ino()));
    assert_eq!(fixture.coordinator.workers, 0);
}

#[tokio::test]
async fn partial_status_reports_external_lifecycle_limits() {
    let mut fixture = Fixture::new().await;
    fixture.coordinator.record.provider = "partial".into();
    fixture.coordinator.provider.kind = ProviderKind::Attached;
    let details = query(&mut fixture).await;
    assert_eq!(details.state, "partial");
    assert_eq!(details.status.provider, "partial");
    assert!(details.limitations[0].contains("PAM"));
    assert!(details.limitations[0].contains("prepared"));
    assert!(details.capabilities.iter().any(|capability| capability.name == "compositor-lifecycle" && capability.state == "unavailable"));
}

#[tokio::test]
async fn current_preparation_replaces_the_archive_for_the_same_invocation() {
    let mut fixture = Fixture::new().await;
    fixture.coordinator.runtime.archive_app(&prepared(Outcome::Unknown, Method::Command)).unwrap();
    fixture.coordinator.runtime.save_app(&prepared(Outcome::Cancelled, Method::WindowClose)).unwrap();
    let details = query(&mut fixture).await;
    assert_eq!(details.preparations.len(), 1);
    assert_eq!(details.preparations[0].result, "cancelled");
    assert_eq!(details.preparations[0].method, "window-close");
}

#[tokio::test]
async fn status_probes_respect_the_worker_budget_and_disable_gate() {
    let mut fixture = Fixture::new().await;
    fixture.coordinator.workers = MAX_COORDINATOR_WORKERS;
    let (reply, mut result) = tokio::sync::mpsc::channel(1);
    fixture.coordinator.request_status_details(crate::control::Reply::for_test(reply));
    assert!(result.try_recv().unwrap().unwrap_err().contains("busy"));
    assert_eq!(fixture.coordinator.workers, MAX_COORDINATOR_WORKERS);
    assert!(fixture.coordinator.work.try_recv().is_err());
    fixture.coordinator.workers = 0;
    fixture.coordinator.manager.actions.close();
    let details = query(&mut fixture).await;
    assert!(!details.enabled);
    assert_eq!(details.state, "disabled");
    assert!(details.capabilities.is_empty());
    assert!(details.preparations.is_empty());
}

#[tokio::test]
async fn a_pending_capability_probe_does_not_block_cancellation() {
    let mut fixture = Fixture::new().await;
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    fixture._server.object_server().at("/org/freedesktop/DBus", PendingBus { entered: entered.clone(), release: release.clone() }).await.unwrap();
    fixture.coordinator.record.provider = "gnome".into();
    fixture.coordinator.provider.kind = ProviderKind::Gnome;
    fixture.coordinator.lifecycle.phase = SessionPhase::Preparing;
    let control = Arc::new(crate::app_stop::ShutdownControl::default());
    fixture.coordinator.shutdown = Some(control.clone());
    let (reply, mut status) = tokio::sync::mpsc::channel(1);
    fixture.coordinator.request_status_details(crate::control::Reply::for_test(reply));
    tokio::time::timeout(Duration::from_secs(1), entered.notified()).await.unwrap();
    let (reply, mut cancelled) = tokio::sync::mpsc::channel(1);
    tokio::time::timeout(Duration::from_millis(500), fixture.coordinator.request(crate::control::Request::Cancel {
        generation: GENERATION.into(), reply: crate::control::Reply::for_test(reply),
    })).await.unwrap().unwrap();
    cancelled.try_recv().unwrap().unwrap();
    assert!(control.cancelled());
    assert!(status.try_recv().is_err());
    release.notify_one();
    let work = tokio::time::timeout(Duration::from_secs(1), fixture.coordinator.work.recv()).await.unwrap().unwrap();
    fixture.coordinator.completed(work).await.unwrap();
    let details = status.try_recv().unwrap().unwrap();
    assert_eq!(details.state, "full");
    assert!(details.capabilities.iter().any(|capability| capability.name == "native-logout" && capability.state == "unavailable"));
}
