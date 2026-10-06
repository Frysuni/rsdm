//! systemd command construction for managed sessions and applications.

use std::{
    os::unix::process::ExitStatusExt,
    process::{Command, ExitStatus},
};

use rsdm_core::domain::SessionManagerConfig;

use super::env::{DISPLAY_VARS, unset_environment_command, valid_environment_name};

pub(super) const APP_SLICE: &str = "app-graphical.slice";
pub(super) const SESSION_SLICE: &str = "session.slice";
pub(super) const PRE_TARGET: &str = "graphical-session-pre.target";
pub(super) const SESSION_TARGET: &str = "graphical-session.target";
pub(super) const AUTOSTART_TARGET: &str = "xdg-desktop-autostart.target";

/// Fixed transient unit that keeps `graphical-session.target` wanted.
pub(super) const ANCHOR_UNIT: &str = "rsdm-graphical-session.service";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ManagedUnitKind {
    Session,
    App,
}

pub(super) fn systemd_run_args(
    kind: ManagedUnitKind,
    unit: &str,
    environment: &[(String, String)],
) -> Vec<String> {
    let (slice, exit_type) = match kind {
        ManagedUnitKind::Session => (SESSION_SLICE, "main"),
        ManagedUnitKind::App => (APP_SLICE, "cgroup"),
    };

    let mut args = vec![
        "--user".to_string(),
        "--quiet".to_string(),
        "--collect".to_string(),
        "--same-dir".to_string(),
        "--expand-environment=no".to_string(),
        format!("--unit={unit}"),
        "--slice".to_string(),
        slice.to_string(),
        "--property=Type=exec".to_string(),
        format!("--property=ExitType={exit_type}"),
        "--property=KillMode=control-group".to_string(),
        format!("--property=PartOf={SESSION_TARGET}"),
    ];

    match kind {
        ManagedUnitKind::Session => {
            args.push("--wait".to_string());
            // Pull graphical-session-pre.target in (it refuses a manual start)
            // and order the compositor after it.
            args.push(format!("--property=Wants={PRE_TARGET}"));
            args.push(format!("--property=After={PRE_TARGET}"));
        }
        ManagedUnitKind::App => {
            args.push(format!("--property=After={SESSION_TARGET}"));
            args.push(format!("--property=Requisite={SESSION_TARGET}"));
        }
    }

    // systemd-run reads values from its own environment when only a name is
    // passed. Keep credentials out of the publicly visible command line.
    for (key, _) in environment {
        if valid_environment_name(key) {
            args.push(format!("--setenv={key}"));
        }
    }

    // --setenv only overrides present names; absent display variables would
    // otherwise leak in from systemd's shared environment (including rsdm app).
    let absent: Vec<_> = DISPLAY_VARS
        .iter()
        .copied()
        .filter(|name| !environment.iter().any(|(key, _)| key == name))
        .collect();
    if !absent.is_empty() {
        args.push(format!("--property=UnsetEnvironment={}", absent.join(" ")));
    }
    args.push("--".to_string());
    args
}

/// Build the anchor command, optionally binding it to the compositor unit.
pub(super) fn anchor_args(bind_to: Option<&str>) -> Vec<String> {
    let mut args = vec![
        "--user".to_string(),
        "--quiet".to_string(),
        "--collect".to_string(),
        format!("--unit={ANCHOR_UNIT}"),
        "--property=Type=oneshot".to_string(),
        "--property=RemainAfterExit=yes".to_string(),
        format!("--property=Wants={SESSION_TARGET}"),
        format!("--property=After={SESSION_TARGET}"),
        format!("--property=Wants={AUTOSTART_TARGET}"),
        format!("--property=After={AUTOSTART_TARGET}"),
    ];
    if let Some(unit) = bind_to {
        args.push(format!("--property=BindsTo={unit}"));
        args.push(format!("--property=After={unit}"));
    }
    args.push("--".to_string());
    args.push("true".to_string());
    args
}

pub(super) fn anchor_graphical_session(bind_to: Option<&str>) -> bool {
    if unit_is_active(ANCHOR_UNIT) {
        tracing::debug!(unit = ANCHOR_UNIT, "graphical session already anchored");
        return true;
    }
    best_effort_owned(&["reset-failed".to_string(), ANCHOR_UNIT.to_string()]);
    let status = Command::new("systemd-run")
        .args(anchor_args(bind_to))
        .status();
    match status {
        Ok(status) if status.success() => {
            tracing::info!(unit = ANCHOR_UNIT, "graphical session anchored");
            true
        }
        Ok(status) => {
            tracing::warn!(unit = ANCHOR_UNIT, %status, "failed to anchor graphical session");
            false
        }
        Err(error) => {
            tracing::warn!(%error, "systemd-run unavailable to anchor graphical session");
            false
        }
    }
}

pub(super) fn unit_is_active(unit: &str) -> bool {
    Command::new("systemctl")
        .args(["--user", "is-active", "--quiet", unit])
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Stop and forget the anchor unit, releasing its hold on the targets without
/// touching a target the compositor may own.
pub(super) fn release_anchor_commands() -> Vec<Vec<String>> {
    vec![
        vec!["stop".to_string(), ANCHOR_UNIT.to_string()],
        vec!["reset-failed".to_string(), ANCHOR_UNIT.to_string()],
    ]
}

/// Best-effort release of a lingering anchor, used before a session starts.
pub(super) fn release_anchor() {
    for args in release_anchor_commands() {
        best_effort_owned(&args);
    }
}

pub(super) fn anchored_teardown_commands(cfg: &SessionManagerConfig) -> Vec<Vec<String>> {
    vec![
        vec!["stop".to_string(), ANCHOR_UNIT.to_string()],
        vec!["stop".to_string(), SESSION_TARGET.to_string()],
        vec!["stop".to_string(), PRE_TARGET.to_string()],
        unset_environment_command(cfg),
    ]
}

pub(super) fn best_effort_owned(args: &[String]) {
    let status = Command::new("systemctl").arg("--user").args(args).status();
    match status {
        Ok(status) if status.success() => {
            tracing::debug!(?args, "systemctl --user command succeeded")
        }
        Ok(status) => tracing::warn!(?args, %status, "systemctl --user command failed"),
        Err(error) => tracing::debug!(%error, ?args, "systemctl --user unavailable"),
    }
}

pub(super) fn stop_unit(unit: &str) {
    best_effort_owned(&["stop".to_string(), unit.to_string()]);
}

pub(super) fn exit_status_code(status: ExitStatus) -> i32 {
    status
        .code()
        .or_else(|| status.signal().map(|signal| 128 + signal))
        .unwrap_or(1)
}

#[cfg(test)]
#[path = "systemd_tests.rs"]
mod tests;
