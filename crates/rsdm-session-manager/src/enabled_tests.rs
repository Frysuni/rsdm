//! Disabled public entry points do not inspect identities, records or buses.

use std::time::Duration;

use super::*;

const CHILD: &str = "RSDM_DISABLED_ENTRY_TEST_CHILD";

#[tokio::test]
async fn disabled_entries_run_in_a_fresh_process_without_access_to_a_bus() {
    if std::env::var_os(CHILD).is_some() {
        let cfg = SessionManagerConfig { enabled: false, ..SessionManagerConfig::default() };
        let status = status(&cfg).await.unwrap();
        assert_eq!(status.phase, "disabled");
        assert!(status.generation.is_empty() && status.apps.is_empty() && !status.xsmp_available);
        assert_eq!(stop("invalid action", &cfg).await.unwrap().result, "disabled");
        assert_eq!(cancel(&cfg).await.unwrap().result, "disabled");
        assert!(generation(&cfg).await.unwrap_err().to_string().contains("disabled"));
        finalize(&cfg, &["INVALID NAME".into()]).await.unwrap();
        stop_hook("../invalid", "../invalid", &cfg).await.unwrap();
        cleanup("../invalid", &cfg).await.unwrap();

        let argv = vec!["/bin/sh".into(), "-c".into(), "exit 7".into()];
        let policy = ShutdownPolicy { timeout_secs: 0, quit_command: vec![String::new()], ..ShutdownPolicy::default() };
        assert_eq!(run_app_with_policy(&argv, &policy, &cfg).await.unwrap(), 7);
        assert_eq!(start(&argv, &cfg).await.unwrap(), 7);
        let signalled = vec!["/bin/sh".into(), "-c".into(), "kill -TERM $$".into()];
        assert_eq!(run_app(&signalled, &cfg).await.unwrap(), 128 + libc::SIGTERM);
        let missing = vec!["/nonexistent-rsdm-disabled-command".into()];
        assert!(matches!(run_app(&missing, &cfg).await, Err(SessionError::Io(error))
            if error.kind() == std::io::ErrorKind::NotFound));
        return;
    }

    let mut child = tokio::process::Command::new(std::env::current_exe().unwrap());
    child.args(["--exact", "enabled_tests::disabled_entries_run_in_a_fresh_process_without_access_to_a_bus"])
        .env(CHILD, "1").env("RSDM_SESSION_GENERATION", "invalid generation")
        .env("XDG_RUNTIME_DIR", "/nonexistent-rsdm-disabled-runtime")
        .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent-rsdm-disabled-session-bus")
        .env("DBUS_SYSTEM_BUS_ADDRESS", "unix:path=/nonexistent-rsdm-disabled-system-bus")
        .kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(5), child.output()).await.unwrap().unwrap();
    assert!(output.status.success(), "{output:?}");
}
