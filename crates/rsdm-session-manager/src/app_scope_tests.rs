use std::{fs, os::unix::fs::PermissionsExt, sync::atomic::Ordering};

use super::*;

#[path = "window_dispatch_fixture.rs"]
mod fixture;

#[path = "app_scope_process_tests.rs"]
mod children;

async fn migrated() -> (fixture::Fixture, AppRecord, bool) {
    let fixture = fixture::Fixture::new().await;
    fixture.state.present.store(false, Ordering::SeqCst);
    fixture.state.scope_present.store(true, Ordering::SeqCst);
    let handle = ProcessHandle::open(std::process::id()).unwrap().unwrap();
    let identity = handle.id().unwrap();
    let supported = identity.is_some() && *fixture.state.scope_group.lock().unwrap() != "/";
    let mut app = fixture.app.clone();
    app.original_process = Some(crate::app_origin::OriginalProcess { pid: std::process::id(), pidfd_id: identity.unwrap_or(0) });
    (fixture, app, supported)
}

#[tokio::test]
async fn only_a_dedicated_scope_of_the_original_process_can_be_adopted() {
    let (fixture, app, supported) = migrated().await;
    assert_eq!(discover(&fixture.manager, &app).await.unwrap().is_some(), supported);
    for invalid in ["original", "main", "invocation", "zero", "foreign", "group", "persistent"] {
        let (fixture, mut app, _) = migrated().await;
        match invalid {
            "original" => app.original_process.as_mut().unwrap().pidfd_id = 0,
            "main" => fixture.state.main_pid.store(0, Ordering::SeqCst),
            "invocation" => fixture.state.app_invocation.store(2, Ordering::SeqCst),
            "zero" => fixture.state.scope_invocation.store(0, Ordering::SeqCst),
            "foreign" => fixture.state.scope_foreign_pid.store(1, Ordering::SeqCst),
            "group" => *fixture.state.scope_group.lock().unwrap() = "/foreign.scope".into(),
            "persistent" => fixture.state.scope_transient.store(false, Ordering::SeqCst),
            _ => unreachable!(),
        }
        let result = discover(&fixture.manager, &app).await;
        assert!(result.is_err() || result.unwrap().is_none(), "{invalid}");
    }
}

#[tokio::test]
async fn captured_scopes_remain_observed_after_the_service_cgroup_becomes_empty() {
    let (fixture, mut app, supported) = migrated().await;
    if !supported { return; }
    app.migrated_scope = discover(&fixture.manager, &app).await.unwrap();
    fixture.state.app_invocation.store(0, Ordering::SeqCst);
    assert_eq!(processes::app_processes(&fixture.manager, &app).await.unwrap(), [std::process::id()]);
    assert!(!closed(&fixture.manager, &app).await.unwrap());
    fixture.state.scope_invocation.store(2, Ordering::SeqCst);
    assert!(processes::app_processes(&fixture.manager, &app).await.is_err());
    assert!(closed(&fixture.manager, &app).await.is_err());
    fixture.state.scope_invocation.store(0, Ordering::SeqCst);
    fixture.state.scope_present.store(false, Ordering::SeqCst);
    assert!(closed(&fixture.manager, &app).await.unwrap());
}

#[tokio::test]
async fn an_uncaptured_live_original_process_cannot_be_reported_as_a_clean_exit() {
    let (fixture, app, supported) = migrated().await;
    if !supported { return; }
    fixture.state.app_invocation.store(0, Ordering::SeqCst);
    assert!(processes::app_processes(&fixture.manager, &app).await.is_err());
}

#[test]
fn a_saved_scope_requires_an_original_process_and_nonzero_invocation() {
    let mut app = crate::helper_start_fixture::app();
    app.migrated_scope = Some(MigratedScope { unit: "app-test.scope".into(), invocation_id: vec![1; 16], lifetime_group: None });
    assert!(registered_scope(&app).is_err());
    app.original_process = Some(crate::app_origin::OriginalProcess { pid: 1, pidfd_id: 2 });
    assert!(registered_scope(&app).unwrap().is_some());
    app.migrated_scope.as_mut().unwrap().invocation_id.fill(0);
    assert!(registered_scope(&app).is_err());
    app.migrated_scope.as_mut().unwrap().invocation_id = vec![1; 16];
    app.migrated_scope.as_mut().unwrap().unit = "app-test.service".into();
    assert!(registered_scope(&app).is_err());
}

#[tokio::test]
async fn a_scope_collected_between_reads_is_closed_but_replacement_is_an_error() {
    for missing_at in 1..=6 {
        let (fixture, mut app, supported) = migrated().await;
        if !supported { return; }
        app.migrated_scope = discover(&fixture.manager, &app).await.unwrap();
        let scope = app.migrated_scope.as_ref().unwrap();
        fixture.state.scope_reads.store(0, Ordering::SeqCst);
        fixture.state.scope_missing_at.store(missing_at, Ordering::SeqCst);
        assert!(processes(&fixture.manager, scope).await.unwrap().is_empty(), "read {missing_at}");
        assert!(closed(&fixture.manager, &app).await.unwrap());
    }
    let (fixture, mut app, supported) = migrated().await;
    if !supported { return; }
    app.migrated_scope = discover(&fixture.manager, &app).await.unwrap();
    fixture.state.scope_invocation.store(2, Ordering::SeqCst);
    assert!(closed(&fixture.manager, &app).await.is_err());
}

#[tokio::test]
async fn scope_adoption_is_durable_and_never_writes_a_retired_generation() {
    for disabled in [false, true] {
        let (fixture, app, supported) = migrated().await;
        if !supported { return; }
        let path = std::env::temp_dir().join(format!("rsdm-scope-{}", zbus::Guid::generate()));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        let runtime = Runtime::create_at(&path, crate::helper_start_fixture::GENERATION).unwrap();
        let mut session = fixture.record.clone();
        // SAFETY: geteuid has no preconditions.
        session.identity.uid = unsafe { libc::geteuid() };
        session.disabled = disabled;
        runtime.save_session(&session).unwrap();
        runtime.save_app(&app).unwrap();
        assert_eq!(refresh(&fixture.manager, &runtime, app.clone()).await.is_ok(), !disabled);
        let saved = runtime.app(&app.unit).unwrap();
        assert_eq!(saved.migrated_scope.is_some(), !disabled);
        if !disabled {
            let scope = saved.migrated_scope.clone();
            assert_eq!(refresh(&fixture.manager, &runtime, saved).await.unwrap().migrated_scope, scope);
        }
        fs::remove_dir_all(path).unwrap();
    }
}

#[cfg(feature = "xsmp")]
#[tokio::test]
async fn an_ice_peer_scope_maps_to_its_authenticated_registered_service() {
    let (fixture, app, supported) = migrated().await;
    if !supported { return; }
    let path = std::env::temp_dir().join(format!("rsdm-scope-peer-{}", zbus::Guid::generate()));
    fs::create_dir(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    let runtime = Runtime::create_at(&path, crate::helper_start_fixture::GENERATION).unwrap();
    let mut session = fixture.record.clone();
    session.identity.uid = unsafe { libc::geteuid() };
    runtime.save_session(&session).unwrap();
    runtime.save_app(&app).unwrap();
    let pid = std::process::id();
    let process = ProcessHandle::open(pid).unwrap().unwrap();
    assert!(peer_app(&fixture.manager, &runtime, "foreign.scope", pid, &process).await.is_err());
    let saved = peer_app(&fixture.manager, &runtime, "app-test.scope", pid, &process).await.unwrap();
    assert_eq!(saved.unit, app.unit);
    assert!(saved.migrated_scope.is_some());
    fixture.state.app_invocation.store(0, Ordering::SeqCst);
    assert_eq!(peer_app(&fixture.manager, &runtime, "app-test.scope", pid, &process).await.unwrap().unit, app.unit);
    fs::remove_dir_all(path).unwrap();
}
