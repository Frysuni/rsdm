//! Environment exported to systemd and D-Bus for the managed session.

use rsdm_core::domain::SessionManagerConfig;

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

/// Display identity published by the live compositor. Appearance settings are
/// imported only when explicitly requested through extra_env or finalize.
pub(super) const WAYLAND_VARS: &[&str] = &[
    "WAYLAND_DISPLAY",
    "DISPLAY",
    "XAUTHORITY",
    "XDG_CURRENT_DESKTOP",
    "XDG_SESSION_DESKTOP",
    "DESKTOP_SESSION",
];

/// Display endpoints must not come from a previous login in the user manager.
pub(super) const DISPLAY_VARS: &[&str] = &["WAYLAND_DISPLAY", "DISPLAY", "XAUTHORITY"];

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
    let names: Vec<&str> = pairs.iter().map(|(key, _)| key.as_str()).collect();

    let mut dbus = std::process::Command::new("dbus-update-activation-environment");
    dbus.arg("--systemd")
        .args(&names)
        .envs(pairs.iter().map(|(key, value)| (key, value)));
    match dbus.status() {
        Ok(status) if status.success() => return,
        Ok(status) => {
            tracing::warn!(%status, "dbus-update-activation-environment failed")
        }
        Err(error) => {
            tracing::debug!(%error, "dbus-update-activation-environment unavailable")
        }
    }
    // --systemd already updates both environments; use systemctl only when the
    // combined update is unavailable or fails.
    let result = std::process::Command::new("systemctl")
        .args(["--user", "import-environment"])
        .args(&names)
        .envs(pairs.iter().map(|(key, value)| (key, value)))
        .status();
    match result {
        Ok(status) if status.success() => {}
        Ok(status) => tracing::warn!(%status, "systemctl import-environment failed"),
        Err(error) => tracing::debug!(%error, "systemctl unavailable"),
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
    collect_command_environment(std::env::vars_os())
}

fn collect_command_environment(
    variables: impl IntoIterator<Item = (std::ffi::OsString, std::ffi::OsString)>,
) -> Vec<(String, String)> {
    let mut environment: Vec<(String, String)> = variables
        .into_iter()
        .filter_map(|(name, value)| Some((name.into_string().ok()?, value.into_string().ok()?)))
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
        // These belong to the user's login environment, not to one compositor.
        .filter(|name| !matches!(name.as_str(), "PATH" | "LANG" | "XDG_RUNTIME_DIR"))
        .filter(|name| valid_environment_name(name));
    let mut unset: Vec<String> = vec!["unset-environment".to_string()];
    unset.extend(names);
    unset
}

/// Return the serialized display assignment for change detection. Its presence
/// alone is not readiness: the user manager can outlive a graphical session.
pub(super) fn manager_wayland_display() -> Option<String> {
    std::process::Command::new("systemctl")
        .args(["--user", "show-environment"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| {
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .find(|line| {
                    line.strip_prefix("WAYLAND_DISPLAY=")
                        .is_some_and(|value| !matches!(value, "" | "\"\"" | "''"))
                })
                .map(str::to_owned)
        })
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
    fn non_utf8_environment_does_not_crash_session_launch() {
        use std::os::unix::ffi::OsStringExt;
        let invalid = std::ffi::OsString::from_vec(vec![0xff]);
        let environment = collect_command_environment([
            ("PATH".into(), "/bin".into()),
            ("INVALID_VALUE".into(), invalid.clone()),
            (invalid, "value".into()),
            ("bad-name".into(), "value".into()),
        ]);
        assert_eq!(environment, [("PATH".into(), "/bin".into())]);
    }

    #[test]
    fn var_names_include_base_wayland_and_extras() {
        let names = all_var_names(&["MY_VAR".to_string()], &["OTHER".to_string()]);
        assert!(names.iter().any(|n| n == "WAYLAND_DISPLAY"));
        assert!(names.iter().any(|n| n == "PATH"));
        assert!(names.iter().any(|n| n == "MY_VAR"));
        assert!(names.iter().any(|n| n == "OTHER"));
        assert!(names.iter().any(|n| n == "XAUTHORITY"));
        assert!(!names.iter().any(|n| n == "XCURSOR_THEME"));
    }

    #[test]
    fn environment_names_match_systemd_environment_rules() {
        assert!(valid_environment_name("_RSDM"));
        assert!(valid_environment_name("RSDM_1"));
        assert!(!valid_environment_name(""));
        assert!(!valid_environment_name("1_RSDM"));
        assert!(!valid_environment_name("RSDM-NAME"));
    }

    #[test]
    fn logout_keeps_the_user_managers_base_environment() {
        let command = unset_environment_command(&SessionManagerConfig::default());
        for name in ["PATH", "LANG", "XDG_RUNTIME_DIR"] {
            assert!(!command.iter().any(|arg| arg == name));
        }
        for name in ["WAYLAND_DISPLAY", "DISPLAY", "XAUTHORITY", "XDG_SESSION_ID"] {
            assert!(command.iter().any(|arg| arg == name));
        }
    }
}
