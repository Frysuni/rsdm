//! Native capability selects preparation before any desktop shutdown dispatch.

use super::*;
use crate::{app_result::Outcome, coordinator_preparation::prepare_applications,
    status::Capability};

#[derive(Debug, zbus_macros::DBusError)]
#[zbus(prefix = "org.freedesktop.systemd1")]
enum ManagerError { NoSuchUnit(String) }

struct MissingApplications;

#[zbus_macros::interface(name = "org.freedesktop.systemd1.Manager")]
impl MissingApplications {
    fn get_unit(&self, _unit: &str) -> Result<zbus::zvariant::OwnedObjectPath, ManagerError> {
        Err(ManagerError::NoSuchUnit("application is absent".into()))
    }
}

struct NoXsmp;

#[zbus_macros::interface(name = "org.rsdm.Session1")]
impl NoXsmp {
    fn xsmp_prepare(&self, generation: &str, _transaction: &str, _units: Vec<String>, _cancelable: bool) -> Vec<String> {
        assert_eq!(generation, GENERATION);
        Vec::new()
    }
}

#[tokio::test]
async fn unavailable_native_logout_prepares_auto_apps_instead_of_skipping_them() {
    for state in ["available", "unavailable", "unknown"] {
        let fixture = Fixture::new().await;
        fixture._server.object_server().at("/org/freedesktop/systemd1", MissingApplications).await.unwrap();
        fixture._server.object_server().at(crate::control::OBJECT_PATH, NoXsmp).await.unwrap();
        let mut app = crate::helper_start_fixture::app();
        app.invocation_id.clear();
        app.policy.quit_command.clear();
        fixture.coordinator.runtime.save_app(&app).unwrap();
        let mut explicit = app.clone();
        explicit.unit = explicit.unit.replace("example", "explicit");
        explicit.policy.method = rsdm_core::domain::ShutdownMethod::Term;
        fixture.coordinator.runtime.save_app(&explicit).unwrap();
        let capability = Capability::new("native-logout", state, "fixture native capability");
        let control = Arc::new(ShutdownControl::default());
        prepare_applications(&fixture.coordinator.manager, &fixture.coordinator.runtime,
            control, "logout", Some(&capability)).await.unwrap();
        let saved = fixture.coordinator.runtime.app(&app.unit).unwrap();
        assert_eq!(saved.preparation.is_some(), state != "available");
        let saved = fixture.coordinator.runtime.app(&explicit.unit).unwrap();
        assert_eq!(saved.preparation.unwrap().outcome, Outcome::Unknown);
    }
}

#[tokio::test]
async fn recovered_native_control_cannot_bypass_a_registered_auto_cancel_policy() {
    for state in ["available", "unavailable", "unknown"] {
        let fixture = Fixture::new().await;
        fixture._server.object_server().at("/org/freedesktop/systemd1", MissingApplications).await.unwrap();
        fixture._server.object_server().at(crate::control::OBJECT_PATH, NoXsmp).await.unwrap();
        let mut app = crate::helper_start_fixture::app();
        app.invocation_id.clear();
        app.policy.quit_command.clear();
        app.policy.on_timeout = rsdm_core::domain::TimeoutAction::Cancel;
        fixture.coordinator.runtime.save_app(&app).unwrap();
        let mut explicit = app.clone();
        explicit.unit = explicit.unit.replace("example", "explicit");
        explicit.policy.method = rsdm_core::domain::ShutdownMethod::Term;
        fixture.coordinator.runtime.save_app(&explicit).unwrap();
        let capability = Capability::new("native-logout", state, "fixture native capability");
        let result = prepare_applications(&fixture.coordinator.manager, &fixture.coordinator.runtime,
            Arc::new(ShutdownControl::default()), "logout", Some(&capability)).await;
        assert_eq!(result.is_err(), state == "available");
        for unit in [&app.unit, &explicit.unit] {
            let saved = fixture.coordinator.runtime.app(unit).unwrap();
            assert_eq!(saved.preparation.is_some(), state != "available");
            assert!(!saved.quit_started);
        }
    }
}

#[tokio::test]
async fn unavailable_native_logout_returns_prepared_without_infrastructure_work() {
    for kind in [ProviderKind::Gnome, ProviderKind::Plasma] {
        for state in ["unavailable", "unknown"] {
            let mut fixture = Fixture::new().await;
            let coordinator = &mut fixture.coordinator;
            coordinator.provider.kind = kind;
            coordinator.record.provider = coordinator.provider.name().into();
            let (reply, mut outcome) = tokio::sync::mpsc::channel(1);
            coordinator.stop_replies.push(crate::control::Reply::for_test(reply));
            coordinator.begin_stop("logout", None).await.unwrap();
            let capability = Capability::new("native-logout", state, "fixture native capability");
            coordinator.prepared(Ok(Vec::new()), Some(capability)).await.unwrap();
            assert_eq!(outcome.try_recv().unwrap().unwrap().result, "prepared");
            assert_eq!(coordinator.lifecycle.phase, SessionPhase::Running);
            assert!(coordinator.shutdown.is_none());
            assert_eq!(coordinator.workers, 0);
            assert!(coordinator.work.try_recv().is_err());
            assert_eq!(coordinator.record.provider, coordinator.provider.name());
        }
    }
}

#[tokio::test]
async fn logind_power_acceptance_retains_a_native_session_without_lifecycle_control() {
    for action in ["reboot", "poweroff"] {
        let mut fixture = Fixture::new().await;
        let coordinator = &mut fixture.coordinator;
        coordinator.provider.kind = ProviderKind::Gnome;
        coordinator.record.provider = "gnome".into();
        let (reply, mut outcome) = tokio::sync::mpsc::channel(1);
        coordinator.stop_replies.push(crate::control::Reply::for_test(reply));
        coordinator.begin_stop(action, None).await.unwrap();
        let capability = Capability::new("native-logout", "unavailable", "fixture native capability");
        coordinator.power_requested(Ok(()), Some(capability)).await.unwrap();
        let outcome = outcome.try_recv().unwrap().unwrap();
        assert_eq!(outcome.result, "prepared");
        assert!(outcome.message.contains("logind accepted"));
        assert_eq!(coordinator.lifecycle.phase, SessionPhase::Closed);
        assert!(coordinator.retain_launcher);
        assert_eq!(coordinator.workers, 0);
    }
}

#[tokio::test]
async fn a_failed_native_dispatch_has_no_generic_shutdown_fallback() {
    let mut fixture = Fixture::new().await;
    let coordinator = &mut fixture.coordinator;
    coordinator.provider.kind = ProviderKind::Gnome;
    coordinator.record.provider = "gnome".into();
    let (reply, mut outcome) = tokio::sync::mpsc::channel(1);
    coordinator.stop_replies.push(crate::control::Reply::for_test(reply));
    coordinator.begin_stop("logout", None).await.unwrap();
    coordinator.lifecycle.phase = SessionPhase::StoppingSession;
    coordinator.delegated(Err(SessionError::State("native reply was lost".into()))).await.unwrap();
    assert_eq!(outcome.try_recv().unwrap().unwrap().result, "failed");
    assert_eq!(coordinator.lifecycle.phase, SessionPhase::Running);
    assert_eq!(coordinator.workers, 0);
    assert!(coordinator.work.try_recv().is_err());
}
