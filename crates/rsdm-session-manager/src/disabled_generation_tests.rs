//! Re-enabling configuration must not revive a disabled generation's hooks.

use std::{fs, os::unix::fs::PermissionsExt, sync::atomic::Ordering, time::Duration};

use super::{*, helper_start_fixture::{self as fixture, GENERATION}, runtime::{blocking, Runtime}};

const CHILD: &str = "RSDM_DISABLED_GENERATION_TEST_CHILD";

#[tokio::test]
async fn disabled_records_survive_reenabled_configuration_without_recovery_actions() {
    if std::env::var_os(CHILD).is_some() {
        check_disabled_hooks().await;
        return;
    }

    let directory = blocking(|| {
        let path = std::env::temp_dir().join(format!("rsdm-disabled-generation-{}", std::process::id()));
        fs::create_dir(&path)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
        Ok(path)
    }).await.unwrap();
    let mut child = tokio::process::Command::new(std::env::current_exe().unwrap());
    child.args(["--exact", "disabled_generation_tests::disabled_records_survive_reenabled_configuration_without_recovery_actions"])
        .env(CHILD, "1").env("XDG_RUNTIME_DIR", &directory)
        .env("RSDM_SESSION_GENERATION", GENERATION)
        .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent-rsdm-disabled-generation-session-bus")
        .env("DBUS_SYSTEM_BUS_ADDRESS", "unix:path=/nonexistent-rsdm-disabled-generation-system-bus")
        .kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(5), child.output()).await.unwrap().unwrap();
    blocking(move || Ok(fs::remove_dir_all(directory)?)).await.unwrap();
    assert!(output.status.success(), "{output:?}");
}

async fn check_disabled_hooks() {
    let (runtime, session, app, lease) = blocking(|| {
        let runtime = Runtime::create(GENERATION)?;
        let mut record = fixture::record();
        // SAFETY: geteuid has no preconditions.
        record.identity.uid = unsafe { libc::geteuid() };
        let active = record.clone();
        record.disabled = true;
        runtime.save_session(&record)?;
        assert!(runtime.save_session(&active).is_err());
        runtime.save_app(&fixture::app())?;
        // Disabled hooks must not enumerate or validate unrelated app records.
        fs::write(runtime.path.join("apps/invalid.toml"), "invalid TOML")?;
        let session = fs::read(runtime.path.join("session.toml"))?;
        let app = fs::read(runtime.path.join("apps").join(format!("{}.toml", fixture::app().unit)))?;
        let lease = super::session_lease::SessionLease::acquire()?;
        Ok((runtime, session, app, lease))
    }).await.unwrap();
    let cfg = SessionManagerConfig::default();
    assert!(cfg.enabled);
    stop_hook(GENERATION, "../invalid", &cfg).await.unwrap();
    stop_hook(GENERATION, &fixture::app().unit, &cfg).await.unwrap();
    cleanup(GENERATION, &cfg).await.unwrap();

    let peer = fixture::Fixture::new(format!("rsdm-session-{GENERATION}.service"), fixture::Mode::Collision).await;
    super::cleanup::recover(&peer.manager, &runtime).await.unwrap();
    assert_eq!(peer.state.starts.load(Ordering::SeqCst), 0);
    assert_eq!(peer.state.stops.load(Ordering::SeqCst), 0);
    assert_eq!(peer.state.unrefs.load(Ordering::SeqCst), 0);
    check_damaged_records(&runtime, &cfg).await;

    blocking(move || {
        assert_eq!(fs::read(runtime.path.join("session.toml"))?, session);
        assert_eq!(fs::read(runtime.path.join("apps").join(format!("{}.toml", fixture::app().unit)))?, app);
        assert!(runtime.path.join("apps/invalid.toml").exists());
        assert_eq!(fs::read_dir(runtime.path.join("apps"))?.count(), 2);
        drop(lease);
        Ok(())
    }).await.unwrap();
}

async fn check_damaged_records(runtime: &Runtime, cfg: &SessionManagerConfig) {
    let records = runtime.clone();
    let saved = blocking(move || Ok(fs::read(records.path.join("session.toml"))?)).await.unwrap();
    for content in [Some("invalid TOML"), None] {
        let records = runtime.clone();
        blocking(move || {
            let path = records.path.join("session.toml");
            match content { Some(content) => fs::write(path, content)?, None => fs::remove_file(path)? }
            Ok(())
        }).await.unwrap();
        stop_hook(GENERATION, "../invalid", cfg).await.unwrap();
        cleanup(GENERATION, cfg).await.unwrap();
    }
    let records = runtime.clone();
    blocking(move || Ok(fs::write(records.path.join("session.toml"), saved)?)).await.unwrap();
}

#[test]
fn legacy_session_records_default_to_enabled() {
    let record = fixture::record();
    let encoded = toml::to_string(&record).unwrap();
    let legacy = encoded.replace("disabled = false\n", "");
    assert_ne!(legacy, encoded);
    let restored: super::runtime::SessionRecord = toml::from_str(&legacy).unwrap();
    assert!(!restored.disabled);
    assert_eq!(restored.identity.generation, record.identity.generation);
    assert_eq!(restored.compositor_invocation, record.compositor_invocation);
}
