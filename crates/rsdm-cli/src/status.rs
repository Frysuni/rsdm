use std::{
    io::{self, Read},
    os::{fd::OwnedFd, unix::net::UnixStream},
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use anyhow::Result;
use rsdm_infra::config::load_config;

use crate::output::{ACCENT, ERROR, MUTED, Report, SUCCESS, state_color};

pub fn run(path: &Path) -> Result<()> {
    let config = load_config(path)?;
    let uid = std::env::var("SUDO_UID")
        .ok()
        .and_then(|uid| uid.parse().ok())
        .unwrap_or_else(rsdm_infra::lock_control::current_uid);
    let mut report = Report::new("SYSTEM STATUS", ACCENT);
    report.field("config", path.display().to_string(), ACCENT);
    report.section("COMPONENTS");
    report.field("dm", enabled(config.dm.enable), state_color(enabled(config.dm.enable)));
    report.field("lock", enabled(config.lock.enable), state_color(enabled(config.lock.enable)));
    let service = user_unit_state("rsdm-idle.service");
    let idle = format!("{} (service: {}, timeout: {}s, inhibitors: {})",
        enabled(config.idle.enable), service, config.idle.timeout,
        if config.idle.ignore_inhibitors { "ignored" } else { "honored" });
    report.field("idle", idle, if config.idle.enable { state_color(&service) } else { MUTED });
    report.section("CURRENT LOGIN");
    let (lock, color) = match rsdm_infra::lock_control::lock_state(uid) {
        Ok(Some(state)) => (format!("pid {} (uid {})", state.pid, state.uid), SUCCESS),
        Ok(None) => (format!("none for uid {uid}"), MUTED),
        Err(error) => (format!("invalid runtime state: {error}"), ERROR),
    };
    report.field("active lock", lock, color);
    report.field("wayland", std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "unavailable".into()), ACCENT);
    report.stdout()?;
    Ok(())
}

fn user_unit_state(unit: &str) -> String {
    let Ok(state) = query_user_unit_state(unit) else { return "unavailable".to_string(); };
    if state.is_empty() { "unknown".to_string() } else { state }
}

fn query_user_unit_state(unit: &str) -> io::Result<String> {
    let (reader, writer) = UnixStream::pair()?;
    reader.set_nonblocking(true)?;
    let mut command = Command::new("systemctl");
    command.args(["--user", "is-active", unit])
        .stdin(Stdio::null()).stdout(OwnedFd::from(writer)).stderr(Stdio::null());
    rsdm_infra::unix::run_command_until(&mut command, Instant::now() + Duration::from_secs(5))?;
    drop(command);

    // ActiveState is a short scalar. Inherited stdout must not hold the report
    // open after systemctl exits, and unexpected output must stay bounded.
    let mut bytes = Vec::new();
    if let Err(error) = reader.take(256).read_to_end(&mut bytes) {
        if error.kind() != io::ErrorKind::WouldBlock { return Err(error); }
    }
    Ok(String::from_utf8_lossy(&bytes).trim().to_string())
}

const fn enabled(value: bool) -> &'static str {
    if value { "enabled" } else { "disabled" }
}
