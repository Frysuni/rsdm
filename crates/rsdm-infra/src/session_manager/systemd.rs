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

    for (key, value) in environment {
        if valid_environment_name(key) {
            args.push(format!("--setenv={key}={value}"));
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
mod tests {
    use super::*;

    fn contains_arg(args: &[String], expected: &str) -> bool {
        args.iter().any(|arg| arg == expected)
    }

    fn contains_property(args: &[String], expected: &str) -> bool {
        contains_arg(args, &format!("--property={expected}"))
    }

    fn slice_arg(args: &[String]) -> Option<&str> {
        args.windows(2)
            .find(|window| window[0] == "--slice")
            .map(|window| window[1].as_str())
    }

    #[test]
    fn session_unit_tracks_the_compositor_main_process() {
        let args = systemd_run_args(
            ManagedUnitKind::Session,
            "session-rsdm-test",
            &[("WAYLAND_DISPLAY".to_string(), "wayland-1".to_string())],
        );

        assert!(contains_arg(&args, "--user"));
        assert!(contains_arg(&args, "--collect"));
        assert!(contains_arg(&args, "--same-dir"));
        assert!(contains_arg(&args, "--expand-environment=no"));
        assert!(contains_arg(&args, "--wait"));
        assert!(!contains_arg(&args, "--scope"));
        assert_eq!(slice_arg(&args), Some(SESSION_SLICE));
        assert!(contains_property(&args, "Type=exec"));
        assert!(contains_property(&args, "ExitType=main"));
        assert!(contains_property(&args, "KillMode=control-group"));
        assert!(contains_property(
            &args,
            &format!("PartOf={SESSION_TARGET}")
        ));
        assert!(contains_property(&args, &format!("After={PRE_TARGET}")));
        assert!(contains_property(&args, &format!("Wants={PRE_TARGET}")));
        assert!(contains_arg(&args, "--setenv=WAYLAND_DISPLAY=wayland-1"));
        assert!(contains_property(
            &args,
            "UnsetEnvironment=DISPLAY XAUTHORITY"
        ));
        assert_eq!(args.last().map(String::as_str), Some("--"));
    }

    #[test]
    fn xwayland_app_keeps_its_credentials_without_inheriting_a_wayland_display() {
        let args = systemd_run_args(
            ManagedUnitKind::App,
            "app-rsdm-test",
            &[
                ("DISPLAY".into(), ":2".into()),
                ("XAUTHORITY".into(), "/run/user/1000/xauth".into()),
                ("XCURSOR_THEME".into(), "Adwaita".into()),
                ("XCURSOR_SIZE".into(), "32".into()),
            ],
        );
        assert!(contains_property(&args, "UnsetEnvironment=WAYLAND_DISPLAY"));
        for assignment in [
            "DISPLAY=:2",
            "XAUTHORITY=/run/user/1000/xauth",
            "XCURSOR_THEME=Adwaita",
            "XCURSOR_SIZE=32",
        ] {
            assert!(contains_arg(&args, &format!("--setenv={assignment}")));
        }
    }

    #[test]
    fn app_unit_lives_until_its_cgroup_is_empty() {
        let args = systemd_run_args(
            ManagedUnitKind::App,
            "app-rsdm-test",
            &[
                ("WAYLAND_DISPLAY".to_string(), "wayland-1".to_string()),
                ("bad-name".to_string(), "ignored".to_string()),
            ],
        );

        assert!(contains_arg(&args, "--user"));
        assert!(contains_arg(&args, "--collect"));
        assert!(contains_arg(&args, "--same-dir"));
        assert!(contains_arg(&args, "--expand-environment=no"));
        assert!(!contains_arg(&args, "--wait"));
        assert!(!contains_arg(&args, "--scope"));
        assert_eq!(slice_arg(&args), Some(APP_SLICE));
        assert!(contains_property(&args, "Type=exec"));
        assert!(contains_property(&args, "ExitType=cgroup"));
        assert!(contains_property(&args, "KillMode=control-group"));
        assert!(contains_property(
            &args,
            &format!("PartOf={SESSION_TARGET}")
        ));
        assert!(contains_property(&args, &format!("After={SESSION_TARGET}")));
        assert!(contains_property(
            &args,
            &format!("Requisite={SESSION_TARGET}")
        ));
        assert!(contains_arg(&args, "--setenv=WAYLAND_DISPLAY=wayland-1"));
        assert!(!contains_arg(&args, "--setenv=bad-name=ignored"));
        assert_eq!(args.last().map(String::as_str), Some("--"));
    }

    #[test]
    fn teardown_stops_targets_not_the_app_slice_directly() {
        let mut cfg = SessionManagerConfig::default();
        cfg.extra_env.push("EXTRA_ENV".to_string());
        cfg.extra_env.push("bad-name".to_string());

        let commands = anchored_teardown_commands(&cfg);

        assert!(
            commands
                .iter()
                .any(|command| command == &vec!["stop".to_string(), ANCHOR_UNIT.to_string()])
        );
        assert!(
            commands
                .iter()
                .any(|command| command == &vec!["stop".to_string(), SESSION_TARGET.to_string()])
        );
        assert!(
            commands
                .iter()
                .any(|command| command == &vec!["stop".to_string(), PRE_TARGET.to_string()])
        );
        assert!(
            !commands
                .iter()
                .any(|command| command == &vec!["stop".to_string(), APP_SLICE.to_string()])
        );
        let unset = commands
            .iter()
            .find(|command| command.first().map(String::as_str) == Some("unset-environment"))
            .expect("teardown unsets the exported session environment");
        assert!(unset.iter().any(|arg| arg == "WAYLAND_DISPLAY"));
        assert!(unset.iter().any(|arg| arg == "EXTRA_ENV"));
        assert!(!unset.iter().any(|arg| arg == "bad-name"));
    }

    #[test]
    fn anchor_binds_to_the_compositor_unit_when_known() {
        let args = anchor_args(Some("session-rsdm-niri-1-2-3.service"));
        assert!(contains_property(
            &args,
            "BindsTo=session-rsdm-niri-1-2-3.service"
        ));
        assert!(contains_property(
            &args,
            "After=session-rsdm-niri-1-2-3.service"
        ));
        assert!(contains_property(&args, &format!("Wants={SESSION_TARGET}")));
        assert!(contains_property(
            &args,
            &format!("Wants={AUTOSTART_TARGET}")
        ));
        assert!(contains_property(&args, "RemainAfterExit=yes"));
        assert!(contains_arg(&args, "--collect"));
        assert_eq!(args.last().map(String::as_str), Some("true"));
    }

    #[test]
    fn anchor_without_a_compositor_unit_has_no_binds_to() {
        let args = anchor_args(None);
        assert!(!args.iter().any(|arg| arg.contains("BindsTo=")));
        assert!(contains_property(&args, &format!("Wants={SESSION_TARGET}")));
    }

    #[test]
    fn releasing_the_anchor_never_stops_a_compositor_owned_target() {
        // When the compositor owns graphical-session.target rsdm must only touch
        // its own anchor unit, never the shared targets.
        let commands = release_anchor_commands();
        assert!(
            commands.iter().all(|command| !command
                .iter()
                .any(|arg| arg == SESSION_TARGET || arg == PRE_TARGET || arg == AUTOSTART_TARGET)),
            "anchor release must not stop any graphical-session target"
        );
        assert!(
            commands
                .iter()
                .any(|command| command == &vec!["stop".to_string(), ANCHOR_UNIT.to_string()])
        );
        assert!(commands.iter().any(|command| {
            command == &vec!["reset-failed".to_string(), ANCHOR_UNIT.to_string()]
        }));
    }
}
