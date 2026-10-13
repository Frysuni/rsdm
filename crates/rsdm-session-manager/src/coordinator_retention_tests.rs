//! Preparation completion releases coordinator work without ending a retained login.

use super::*;
use crate::{session_process::SessionProcess, status::Capability};

fn process(fixture: &Fixture) -> SessionProcess {
    let provider = Provider { kind: ProviderKind::Managed, native_unit: None, logout_command: Vec::new() };
    let mut process = SessionProcess::launch(&["unused".into()], &[], &fixture.directory,
        &provider, "compositor.service".into()).unwrap();
    process.booting = false;
    process
}

#[tokio::test]
async fn external_native_shutdown_retains_infrastructure_when_control_is_unavailable() {
    for kind in [ProviderKind::Gnome, ProviderKind::Plasma] {
        for state in ["unavailable", "unknown"] {
            let mut fixture = Fixture::new().await;
            let process = process(&fixture);
            let coordinator = &mut fixture.coordinator;
            coordinator.provider.kind = kind;
            coordinator.record.provider = coordinator.provider.name().into();
            let (reply, mut outcome) = tokio::sync::mpsc::channel(1);
            coordinator.stop_replies.push(crate::control::Reply::for_test(reply));
            let deadline = monotonic_usec().unwrap() + 30_000_000;
            coordinator.begin_stop("external-shutdown", Some(deadline)).await.unwrap();
            let capability = Capability::new("compositor-lifecycle", state, "fixture native capability");
            coordinator.lifecycle_checked(process, capability).await.unwrap();
            assert_eq!(outcome.try_recv().unwrap().unwrap().result, "prepared");
            assert_eq!(coordinator.lifecycle.phase, SessionPhase::Closed);
            assert!(coordinator.retain_launcher);
            assert!(coordinator.process.is_some());
            assert_eq!(coordinator.manager.deadline.get(), deadline);
            assert_eq!(coordinator.record.shutdown_deadline_usec, Some(deadline));
            assert_eq!(coordinator.action, "external-shutdown");
            assert!(coordinator.shutdown.as_ref().unwrap().noncancelable.load(Ordering::SeqCst));
            assert_eq!(coordinator.workers, 0);
            assert!(coordinator.work.try_recv().is_err());
        }
    }
}

#[tokio::test]
async fn native_lifecycle_notifications_do_not_authorize_unverified_teardown() {
    for action in ["external-stop", "compositor-exited"] {
        for kind in [ProviderKind::Gnome, ProviderKind::Plasma] {
            let mut fixture = Fixture::new().await;
            let process = process(&fixture);
            let coordinator = &mut fixture.coordinator;
            coordinator.provider.kind = kind;
            coordinator.record.provider = coordinator.provider.name().into();
            let deadline = monotonic_usec().unwrap() + 5_000_000;
            coordinator.begin_stop(action, Some(deadline)).await.unwrap();
            let capability = Capability::new("compositor-lifecycle", "unavailable", "fixture native capability");
            coordinator.lifecycle_checked(process, capability).await.unwrap();
            assert!(coordinator.retain_launcher);
            assert_eq!(coordinator.lifecycle.phase, SessionPhase::Closed);
            assert_eq!(coordinator.record.shutdown_transaction.as_ref().unwrap().result.as_deref(), Some("prepared"));
            assert_eq!(coordinator.manager.deadline.get(), deadline);
            assert_eq!(coordinator.workers, 0);
            assert!(coordinator.work.try_recv().is_err());
        }
    }
}

#[tokio::test]
async fn native_teardown_starts_with_an_ownership_probe_for_each_lifecycle_notification() {
    for action in ["external-shutdown", "external-stop", "compositor-exited"] {
        let mut fixture = Fixture::new().await;
        fixture._server.object_server().at("/org/freedesktop/DBus", super::attached::NoNative).await.unwrap();
        let process = process(&fixture);
        let coordinator = &mut fixture.coordinator;
        coordinator.provider.kind = ProviderKind::Gnome;
        coordinator.record.provider = "gnome".into();
        coordinator.process = Some(process);
        coordinator.begin_stop(action, Some(monotonic_usec().unwrap() + 5_000_000)).await.unwrap();
        coordinator.begin_finish().await.unwrap();
        let work = tokio::time::timeout(Duration::from_secs(3), coordinator.work.recv()).await.unwrap().unwrap();
        assert!(matches!(work, Work::LifecycleChecked(_, _)));
        coordinator.completed(work).await.unwrap();
        assert!(coordinator.retain_launcher);
        assert_eq!(coordinator.lifecycle.phase, SessionPhase::Closed);
        assert_eq!(coordinator.workers, 0);
    }
}

#[tokio::test]
async fn a_power_completion_after_logind_notice_does_not_reset_the_external_budget() {
    let mut fixture = Fixture::new().await;
    let coordinator = &mut fixture.coordinator;
    coordinator.provider.kind = ProviderKind::Gnome;
    coordinator.record.provider = "gnome".into();
    coordinator.begin_stop("poweroff", None).await.unwrap();
    coordinator.lifecycle.phase = SessionPhase::StoppingSession;
    let deadline = monotonic_usec().unwrap() + 30_000_000;
    coordinator.shutdown_notice(ShutdownNotice { preparing: true, deadline_usec: deadline }).await.unwrap();
    let capability = Capability::new("native-logout", "unavailable", "fixture native capability");
    coordinator.power_requested(Ok(()), Some(capability)).await.unwrap();
    assert!(coordinator.retain_launcher);
    assert_eq!(coordinator.lifecycle.phase, SessionPhase::Closed);
    assert_eq!(coordinator.manager.deadline.get(), deadline);
    assert_eq!(coordinator.record.shutdown_deadline_usec, Some(deadline));
    assert_eq!(coordinator.record.shutdown_transaction.as_ref().unwrap().deadline_usec, deadline);
    assert!(coordinator.shutdown.as_ref().unwrap().noncancelable.load(Ordering::SeqCst));
    assert_eq!(coordinator.record.shutdown_transaction.as_ref().unwrap().result.as_deref(), Some("prepared"));
}

#[tokio::test]
async fn retained_native_handoff_waits_for_the_original_child_and_preserves_failures() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    for outcome in ["success", "nonzero", "error"] {
        let mut fixture = Fixture::new().await;
        fixture._server.object_server().at("/org/freedesktop/DBus", super::attached::NoNative).await.unwrap();
        let mut process = process(&fixture);
        let mut child = tokio::process::Command::new("/bin/sh")
            .args(["-c", "printf 'ready\\n'; read line; exit 7"])
            .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).spawn().unwrap();
        let mut input = child.stdin.take().unwrap();
        let mut ready = String::new();
        BufReader::new(child.stdout.take().unwrap()).read_line(&mut ready).await.unwrap();
        assert_eq!(ready, "ready\n");
        process.unit = None;
        process.child = Some(Arc::new(tokio::sync::Mutex::new(child)));
        fixture.coordinator.provider.kind = ProviderKind::Gnome;
        fixture.coordinator.record.provider = "gnome".into();
        fixture.coordinator.process = Some(process.clone());
        fixture.coordinator.retain_session("fixture native capability").await.unwrap();
        let actions = fixture.coordinator.manager.actions.clone();
        let result = match outcome {
            "success" => Ok(0), "nonzero" => Ok(1),
            _ => Err(SessionError::State("completion record could not be saved".into())),
        };
        let mut waiting = Box::pin(crate::coordinator_lifecycle::wait_retained(fixture.coordinator, process, result));
        assert!(tokio::time::timeout(Duration::from_millis(50), &mut waiting).await.is_err());
        assert!(actions.check().is_err());
        input.write_all(b"exit\n").await.unwrap();
        let result = tokio::time::timeout(Duration::from_secs(1), waiting).await.unwrap();
        match outcome {
            "success" => assert_eq!(result.unwrap(), 7),
            "nonzero" => assert_eq!(result.unwrap(), 1),
            _ => assert!(matches!(result, Err(SessionError::State(reason)) if reason == "completion record could not be saved")),
        }
    }
}

#[tokio::test]
async fn a_retained_unsubmitted_command_without_a_child_needs_no_launcher_wait() {
    let mut fixture = Fixture::new().await;
    let process = process(&fixture);
    fixture.coordinator.process = Some(process.clone());
    fixture.coordinator.retain_session("fixture unavailable lifecycle").await.unwrap();
    let error = SessionError::State("managed start was not submitted".into());
    let waiting = crate::coordinator_lifecycle::wait_retained(fixture.coordinator, process, Err(error));
    let result = tokio::time::timeout(Duration::from_millis(100), waiting).await.unwrap();
    assert!(matches!(result, Err(SessionError::State(reason)) if reason == "managed start was not submitted"));
}

#[tokio::test]
async fn a_record_failure_still_requires_retaining_the_launcher() {
    let mut fixture = Fixture::new().await;
    fs::remove_file(fixture.directory.join("session.toml")).unwrap();
    fs::create_dir(fixture.directory.join("session.toml")).unwrap();
    assert!(fixture.coordinator.retain_session("fixture native capability").await.is_err());
    assert!(fixture.coordinator.retain_launcher);
    assert_eq!(fixture.coordinator.lifecycle.phase, SessionPhase::Closed);
}
