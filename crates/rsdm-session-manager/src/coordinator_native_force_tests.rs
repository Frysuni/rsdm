//! Logind retains authority when native preflight or dispatch is already pending.

use super::*;

#[tokio::test]
async fn a_logind_notice_during_native_dispatch_cannot_resume_or_clear_the_budget() {
    for callback in ["delegated", "native-error", "power-error"] {
        let mut fixture = Fixture::new().await;
        fixture._server.object_server().at("/org/freedesktop/DBus", super::super::attached::NoNative).await.unwrap();
        let provider = Provider { kind: ProviderKind::Managed, native_unit: None, logout_command: Vec::new() };
        let mut process = SessionProcess::launch(&["unused".into()], &[], &fixture.directory,
            &provider, "compositor.service".into()).unwrap();
        process.booting = false;
        let coordinator = &mut fixture.coordinator;
        coordinator.provider.kind = ProviderKind::Gnome;
        coordinator.record.provider = "gnome".into();
        coordinator.process = Some(process);
        coordinator.begin_stop(if callback == "power-error" { "poweroff" } else { "logout" }, None).await.unwrap();
        coordinator.lifecycle.phase = SessionPhase::StoppingSession;
        let deadline = monotonic_usec().unwrap() + 30_000_000;
        coordinator.shutdown_notice(ShutdownNotice { preparing: true, deadline_usec: deadline }).await.unwrap();
        match callback {
            "delegated" => coordinator.delegated(Ok(())).await.unwrap(),
            "native-error" => coordinator.delegated(Err(SessionError::State("reply lost".into()))).await.unwrap(),
            _ => coordinator.power_requested(Err(SessionError::State("reply lost".into())), None).await.unwrap(),
        }
        assert_ne!(coordinator.lifecycle.phase, SessionPhase::Running, "{callback}");
        assert_eq!(coordinator.manager.deadline.get(), deadline, "{callback}");
        let work = tokio::time::timeout(Duration::from_secs(2), coordinator.work.recv()).await.unwrap().unwrap();
        let work = if matches!(work, Work::Prepared(_, None)) {
            coordinator.completed(work).await.unwrap();
            tokio::time::timeout(Duration::from_secs(2), coordinator.work.recv()).await.unwrap().unwrap()
        } else { work };
        assert!(matches!(work, Work::LifecycleChecked(_, _)), "{callback}");
        coordinator.completed(work).await.unwrap();
        assert!(coordinator.retain_launcher);
        assert_eq!(coordinator.lifecycle.phase, SessionPhase::Closed);
    }
}

#[tokio::test]
async fn logind_during_native_preflight_or_attached_lookup_prepares_apps_left_for_dialogs() {
    for boundary in ["preflight", "lookup"] {
        let mut fixture = Fixture::new().await;
        fixture._server.object_server().at("/org/freedesktop/DBus", super::super::attached::NoNative).await.unwrap();
        fixture._server.object_server().at("/org/freedesktop/systemd1", MissingApplications).await.unwrap();
        fixture._server.object_server().at(crate::control::OBJECT_PATH, NoXsmp).await.unwrap();
        let mut app = crate::helper_start_fixture::app();
        app.invocation_id.clear();
        app.policy.quit_command.clear();
        fixture.coordinator.runtime.save_app(&app).unwrap();
        let coordinator = &mut fixture.coordinator;
        coordinator.provider.kind = ProviderKind::Attached;
        coordinator.record.provider = "partial".into();
        coordinator.begin_stop("logout", None).await.unwrap();
        let native = Capability::new("native-logout", "available", "fixture native peer");
        if boundary == "lookup" { coordinator.prepared(Ok(Vec::new()), Some(native.clone())).await.unwrap(); }
        coordinator.forced_units.push("earlier-explicit.service".into());
        let deadline = monotonic_usec().unwrap() + 30_000_000;
        coordinator.shutdown_notice(ShutdownNotice { preparing: true, deadline_usec: deadline }).await.unwrap();
        if boundary == "preflight" { coordinator.prepared(Ok(Vec::new()), Some(native)).await.unwrap(); }
        tokio::time::timeout(Duration::from_secs(2), async {
            while coordinator.lifecycle.phase != SessionPhase::Closed {
                let work = coordinator.work.recv().await.unwrap();
                coordinator.completed(work).await.unwrap();
            }
        }).await.unwrap();
        assert!(coordinator.runtime.app(&app.unit).unwrap().preparation.is_some());
        assert_eq!(coordinator.manager.deadline.get(), deadline);
        assert_eq!(coordinator.forced_units, vec!["earlier-explicit.service"]);
        assert_eq!(coordinator.workers, 0);
    }
}

#[tokio::test]
async fn logind_cancellation_during_native_dispatch_prevents_new_forced_preparation() {
    for kind in [ProviderKind::Attached, ProviderKind::Gnome] {
        let mut fixture = Fixture::new().await;
        let provider = Provider { kind: ProviderKind::Managed, native_unit: None, logout_command: Vec::new() };
        let mut process = SessionProcess::launch(&["unused".into()], &[], &fixture.directory,
            &provider, "compositor.service".into()).unwrap();
        process.booting = false;
        let coordinator = &mut fixture.coordinator;
        coordinator.provider.kind = kind;
        coordinator.record.provider = coordinator.provider.name().into();
        coordinator.process = Some(process);
        coordinator.begin_stop("logout", None).await.unwrap();
        coordinator.lifecycle.phase = SessionPhase::StoppingSession;
        coordinator.shutdown_notice(ShutdownNotice::new(true, 30_000_000)).await.unwrap();
        coordinator.shutdown_notice(ShutdownNotice::new(false, 30_000_000)).await.unwrap();
        coordinator.delegated(Ok(())).await.unwrap();
        assert_eq!(coordinator.lifecycle.phase, SessionPhase::Running);
        assert_eq!(coordinator.manager.deadline.get(), 0);
        assert!(coordinator.shutdown.is_none());
        assert_eq!(coordinator.workers, 0);
        assert!(coordinator.work.try_recv().is_err());
    }
}
