//! Environment exported to systemd and D-Bus for the managed session.

use rsdm_core::domain::SessionManagerConfig;

use super::systemd::best_effort_owned;

/// Variables that exist before the compositor and are safe to export up front.
pub(super) const BASE_VARS: &[&str] = &[
    "PATH",
    "XDG_RUNTIME_DIR",
    "XDG_SEAT",
    "XDG_VTNR",
    "XDG_SESSION_ID",
    "XDG_SESSION_TYPE",
    "XDG_SESSION_CLASS",
    "LANG",
];

/// Variables published by the compositor on finalize. Cursor settings remain
/// compositor-owned so Wayland and XWayland cannot diverge.
pub(super) const WAYLAND_VARS: &[&str] = &[
    "WAYLAND_DISPLAY",
    "DISPLAY",
    "XDG_CURRENT_DESKTOP",
    "XDG_SESSION_DESKTOP",
    "DESKTOP_SESSION",
];

/// Push `pairs` into the systemd user manager and D-Bus activation environment.
pub(super) fn export(pairs: &[(String, String)]) {
    if pairs.is_empty() {
        tracing::debug!("no environment variables to export");
        return;
    }
    tracing::debug!(
        names = ?pairs.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(),
        "exporting environment variables"
    );
    let assignments: Vec<String> = pairs
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();

    let mut systemctl: Vec<String> = vec!["set-environment".to_string()];
    systemctl.extend(assignments.iter().cloned());
    best_effort_owned(&systemctl);

    let mut dbus = std::process::Command::new("dbus-update-activation-environment");
    dbus.arg("--systemd").args(&assignments);
    match dbus.status() {
        Ok(status) if status.success() => {}
        Ok(status) => {
            tracing::warn!(%status, "dbus-update-activation-environment failed")
        }
        Err(error) => {
            tracing::debug!(%error, "dbus-update-activation-environment unavailable")
        }
    }
}

/// Names of every variable we manage, for export/unset.
pub(super) fn all_var_names(extra_env: &[String], extra_names: &[String]) -> Vec<String> {
    BASE_VARS
        .iter()
        .chain(WAYLAND_VARS)
        .map(|name| name.to_string())
        .chain(extra_env.iter().cloned())
        .chain(extra_names.iter().cloned())
        .collect()
}

/// `(name, value)` for every managed variable currently present in the env.
pub(super) fn collect_present(
    extra_env: &[String],
    extra_names: &[String],
) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    for name in all_var_names(extra_env, extra_names) {
        if valid_environment_name(&name)
            && let Ok(value) = std::env::var(&name)
            && !pairs.iter().any(|(existing, _)| existing == &name)
        {
            pairs.push((name, value));
        }
    }
    pairs
}

/// Every valid variable in the current process environment, sorted and unique.
pub(super) fn command_environment() -> Vec<(String, String)> {
    let mut environment: Vec<(String, String)> = std::env::vars()
        .filter(|(name, _)| valid_environment_name(name))
        .collect();
    environment.sort_by(|left, right| left.0.cmp(&right.0));
    environment.dedup_by(|left, right| left.0 == right.0);
    environment
}

/// The `systemctl --user unset-environment` argument list for every managed name.
pub(super) fn unset_environment_command(cfg: &SessionManagerConfig) -> Vec<String> {
    let names = all_var_names(&cfg.extra_env, &[])
        .into_iter()
        .filter(|name| valid_environment_name(name));
    let mut unset: Vec<String> = vec!["unset-environment".to_string()];
    unset.extend(names);
    unset
}

/// True when `name` is set in the systemd user manager's environment block -
/// i.e. the compositor has already published it for `graphical-session.target`
/// units (and everything they launch) to inherit.
pub(super) fn manager_env_contains(name: &str) -> bool {
    let prefix = format!("{name}=");
    std::process::Command::new("systemctl")
        .args(["--user", "show-environment"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| {
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .any(|line| line.starts_with(&prefix))
        })
        .unwrap_or(false)
}

/// Whether `name` is a valid systemd environment variable name.
pub(super) fn valid_environment_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn var_names_include_base_wayland_and_extras() {
        let names = all_var_names(&["MY_VAR".to_string()], &["OTHER".to_string()]);
        assert!(names.iter().any(|n| n == "WAYLAND_DISPLAY"));
        assert!(names.iter().any(|n| n == "PATH"));
        assert!(names.iter().any(|n| n == "MY_VAR"));
        assert!(names.iter().any(|n| n == "OTHER"));
    }

    #[test]
    fn environment_names_match_systemd_environment_rules() {
        assert!(valid_environment_name("_RSDM"));
        assert!(valid_environment_name("RSDM_1"));
        assert!(!valid_environment_name(""));
        assert!(!valid_environment_name("1_RSDM"));
        assert!(!valid_environment_name("RSDM-NAME"));
    }
}
