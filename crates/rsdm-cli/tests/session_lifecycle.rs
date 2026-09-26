use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicUsize, Ordering},
};

static SEQUENCE: AtomicUsize = AtomicUsize::new(0);

struct SessionTools(PathBuf);

impl SessionTools {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "rsdm-session-test-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        fs::write(root.join("config.toml"), "").unwrap();
        let tools = Self(root);
        tools.script(
            "systemctl",
            r#"
printf 'systemctl %s\n' "$*" >> "$RSDM_TEST_LOG"
case "$*" in
  *is-active*) test "$RSDM_TEST_ACTIVE" = 1 ;;
  *) exit 0 ;;
esac
"#,
        );
        tools.script(
            "systemd-run",
            r#"
printf 'systemd-run %s\n' "$*" >> "$RSDM_TEST_LOG"
exit 77
"#,
        );
        tools.script(
            "dbus-update-activation-environment",
            r#"
printf 'dbus-update %s\n' "$*" >> "$RSDM_TEST_LOG"
exit 0
"#,
        );
        tools
    }

    fn script(&self, name: &str, body: &str) {
        let path = self.0.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}")).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }

    fn run(&self, action: &[&str], active: &str) -> Output {
        Command::new(env!("CARGO_BIN_EXE_rsdm"))
            .env_clear()
            .env("PATH", &self.0)
            .env("HOME", &self.0)
            .env("RSDM_TEST_LOG", self.0.join("calls"))
            .env("RSDM_TEST_ACTIVE", active)
            .args(["--config"])
            .arg(self.0.join("config.toml"))
            .arg("session")
            .args(action)
            .output()
            .unwrap()
    }

    fn calls(&self) -> String {
        fs::read_to_string(self.0.join("calls")).unwrap_or_default()
    }
}

impl Drop for SessionTools {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn starting_a_second_session_does_not_stop_or_modify_the_first() {
    let tools = SessionTools::new();
    let output = tools.run(&["start", "--", "compositor"], "1");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("already active"));
    let calls = tools.calls();
    assert!(calls.contains("is-active"));
    assert!(!calls.contains("stop"));
    assert!(!calls.contains("dbus-update"));
    assert!(!calls.contains("systemd-run"));
}

#[test]
fn failed_finalize_returns_failure_to_the_caller() {
    let tools = SessionTools::new();
    let output = tools.run(&["finalize"], "0");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("failed to activate"));
    let calls = tools.calls();
    assert!(calls.contains("systemd-run"));
    assert!(!calls.contains("set-environment"));
}

#[test]
fn finalize_does_not_anchor_an_existing_compositor_owned_target() {
    let tools = SessionTools::new();
    let output = tools.run(&["finalize"], "1");
    assert!(output.status.success(), "{:?}", output);
    assert!(!tools.calls().contains("systemd-run"));
}
