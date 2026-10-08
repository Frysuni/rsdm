//! Disabled DM configuration must stop the service without touching a VT.

use std::{fs, path::PathBuf, process::Command};

struct Configuration(PathBuf);

impl Configuration {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!("rsdm-disabled-dm-{}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        fs::write(directory.join("rsdm.toml"), "[dm]\nenable = false\n[dm.tty]\npath = '/dev/tty63'\n").unwrap();
        Self(directory)
    }

    fn command(&self, action: &str) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_rsdm"));
        command.env_clear().env("PATH", &self.0).env("HOME", &self.0)
            .arg("--config").arg(self.0.join("rsdm.toml")).arg(action);
        command
    }
}

impl Drop for Configuration {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
}

#[test]
fn disabled_dm_exits_with_the_packaged_services_clean_stop_status() {
    let config = Configuration::new();
    let output = config.command("dm").output().unwrap();
    let code = output.status.code().unwrap();
    assert_eq!(code, 78, "{}", String::from_utf8_lossy(&output.stderr));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("COMMAND FAILED"));
    assert!(output.stdout.is_empty());

    let unit = include_str!("../../../packaging/systemd/rsdm.service");
    assert!(unit.lines().any(|line| line == format!("SuccessExitStatus={code}")));
    assert!(unit.lines().any(|line| line == format!("RestartPreventExitStatus={code}")));
    assert!(unit.lines().any(|line| line == "Restart=always"));
    assert!(unit.lines().any(|line| line == "Conflicts=getty@tty1.service autovt@tty1.service"));
    assert!(unit.lines().any(|line| line == "ExecStartPre=/usr/bin/systemctl mask --runtime autovt@tty1.service getty@tty1.service"));
    assert!(unit.lines().any(|line| line == "ExecStopPost=-/usr/bin/systemctl unmask --runtime autovt@tty1.service getty@tty1.service"));
    let template = include_str!("../../../packaging/systemd/rsdm@.service");
    assert!(template.lines().any(|line| line == "TTYPath=/dev/%i"));
    assert!(template.lines().any(|line| line == "Conflicts=getty@%i.service autovt@%i.service"));
    assert!(template.lines().any(|line| line == "ExecStartPre=/usr/bin/systemctl mask --runtime autovt@%i.service getty@%i.service"));
    assert!(config.command("validate-config").output().unwrap().status.success());
}
