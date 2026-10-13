//! Cancellation and logind retain authority over a pending native lookup.

use super::*;

pub(super) struct NoNative;

#[zbus_macros::interface(name = "org.freedesktop.DBus")]
impl NoNative {
    fn get_name_owner(&self, _name: &str) -> zbus::fdo::Result<String> {
        Err(zbus::fdo::Error::NameHasNoOwner("native desktop is absent".into()))
    }
}

#[tokio::test]
async fn an_attached_logout_prepares_apps_and_keeps_the_external_session_running() {
    let mut fixture = Fixture::new().await;
    fixture._server.object_server().at("/org/freedesktop/DBus", NoNative).await.unwrap();
    fixture.coordinator.provider.kind = ProviderKind::Attached;
    fixture.coordinator.record.provider = "partial".into();
    fixture.coordinator.ready_once = false;
    fixture.coordinator.begin_stop("logout", None).await.unwrap();
    fixture.coordinator.prepared(Ok(Vec::new()), None).await.unwrap();
    let work = tokio::time::timeout(Duration::from_secs(2), fixture.coordinator.work.recv()).await.unwrap().unwrap();
    fixture.coordinator.completed(work).await.unwrap();
    assert_eq!(fixture.coordinator.lifecycle.phase, SessionPhase::Running);
    assert!(fixture.coordinator.shutdown.is_none());
    assert_eq!(fixture.coordinator.workers, 0);
    assert!(!fixture.coordinator.record.owns_targets);
}

#[tokio::test]
async fn attached_power_completion_replies_without_stopping_foreign_infrastructure() {
    let mut fixture = Fixture::new().await;
    fixture.coordinator.provider.kind = ProviderKind::Attached;
    fixture.coordinator.record.provider = "partial".into();
    fixture.coordinator.ready_once = false;
    let (reply, mut result) = tokio::sync::mpsc::channel(1);
    fixture.coordinator.stop_replies.push(crate::control::Reply::for_test(reply));
    fixture.coordinator.begin_stop("reboot", None).await.unwrap();
    fixture.coordinator.power_requested(Ok(()), None).await.unwrap();
    assert_eq!(result.try_recv().unwrap().unwrap().result, "prepared");
    assert_eq!(fixture.coordinator.lifecycle.phase, SessionPhase::Closed);
    assert_eq!(fixture.coordinator.workers, 0);
}

#[tokio::test]
async fn cancelling_after_app_preparation_prevents_native_delegation() {
    let mut fixture = Fixture::new().await;
    fixture._server.object_server().at("/org/freedesktop/DBus", NoNative).await.unwrap();
    fixture.coordinator.provider.kind = ProviderKind::Attached;
    fixture.coordinator.record.provider = "partial".into();
    fixture.coordinator.begin_stop("logout", None).await.unwrap();
    let native = crate::status::Capability::new("native-logout", "available", "fixture native peer");
    fixture.coordinator.prepared(Ok(Vec::new()), Some(native)).await.unwrap();
    assert_eq!(fixture.coordinator.lifecycle.phase, SessionPhase::Preparing);
    assert!(fixture.coordinator.preparing);
    let (reply, mut cancelled) = tokio::sync::mpsc::channel(1);
    fixture.coordinator.request(crate::control::Request::Cancel {
        generation: GENERATION.into(), reply: crate::control::Reply::for_test(reply),
    }).await.unwrap();
    cancelled.try_recv().unwrap().unwrap();
    let work = tokio::time::timeout(Duration::from_secs(1), fixture.coordinator.work.recv()).await.unwrap().unwrap();
    fixture.coordinator.completed(work).await.unwrap();
    assert_eq!(fixture.coordinator.lifecycle.phase, SessionPhase::Running);
    assert_eq!(fixture.coordinator.record.shutdown_transaction.as_ref().unwrap().result.as_deref(), Some("cancelled"));
    assert!(fixture.coordinator.shutdown.is_none());
    assert_eq!(fixture.coordinator.workers, 0);
}

#[tokio::test]
async fn attached_logout_probes_native_ownership_before_selecting_preparation() {
    let mut fixture = Fixture::new().await;
    fixture._server.object_server().at("/org/freedesktop/DBus", NoNative).await.unwrap();
    fixture.coordinator.provider.kind = ProviderKind::Attached;
    fixture.coordinator.record.provider = "partial".into();
    fixture.coordinator.begin_stop("logout", None).await.unwrap();
    fixture.coordinator.advance().unwrap();
    let work = tokio::time::timeout(Duration::from_secs(2), fixture.coordinator.work.recv()).await.unwrap().unwrap();
    let Work::Prepared(result, Some(native)) = work else { panic!("native probe did not precede preparation"); };
    result.unwrap();
    assert_eq!(native.state, "unavailable");
}

#[tokio::test]
async fn a_lost_attached_native_peer_cannot_claim_auto_applications_were_prepared() {
    for state in ["available", "unavailable", "unknown"] {
        for lookup_failed in [false, true] {
            let mut fixture = Fixture::new().await;
            let coordinator = &mut fixture.coordinator;
            coordinator.provider.kind = ProviderKind::Attached;
            coordinator.record.provider = "partial".into();
            let (reply, mut outcome) = tokio::sync::mpsc::channel(1);
            coordinator.stop_replies.push(crate::control::Reply::for_test(reply));
            coordinator.begin_stop("logout", None).await.unwrap();
            let native = crate::status::Capability::new("native-logout", state, "fixture native peer");
            let lookup = if lookup_failed { Err(SessionError::State("peer changed".into())) } else { Ok(None) };
            coordinator.attached_ready(lookup, Some(native)).await.unwrap();
            let expected = if state == "available" { "failed" } else { "prepared" };
            assert_eq!(outcome.try_recv().unwrap().unwrap().result, expected);
            assert_eq!(coordinator.lifecycle.phase, SessionPhase::Running);
            assert!(coordinator.shutdown.is_none());
            assert_eq!(coordinator.workers, 0);
            assert!(coordinator.work.try_recv().is_err());
            assert!(!coordinator.record.owns_targets);
        }
    }
}

#[tokio::test]
async fn an_external_shutdown_supersedes_attached_native_lookup_without_stopping_foreign_infrastructure() {
    let mut fixture = Fixture::new().await;
    fixture._server.object_server().at("/org/freedesktop/DBus", NoNative).await.unwrap();
    fixture.coordinator.provider.kind = ProviderKind::Attached;
    fixture.coordinator.record.provider = "partial".into();
    fixture.coordinator.begin_stop("logout", None).await.unwrap();
    fixture.coordinator.prepared(Ok(Vec::new()), None).await.unwrap();
    fixture.coordinator.shutdown_notice(ShutdownNotice { preparing: true, deadline_usec: monotonic_usec().unwrap() + 30_000_000 }).await.unwrap();
    let work = tokio::time::timeout(Duration::from_secs(1), fixture.coordinator.work.recv()).await.unwrap().unwrap();
    fixture.coordinator.completed(work).await.unwrap();
    assert_eq!(fixture.coordinator.lifecycle.phase, SessionPhase::Closed);
    assert_eq!(fixture.coordinator.record.shutdown_transaction.as_ref().unwrap().result.as_deref(), Some("prepared"));
    assert!(!fixture.coordinator.record.owns_targets);
    assert_eq!(fixture.coordinator.workers, 0);
}
