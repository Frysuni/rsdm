//! Environment exported to systemd and D-Bus for the managed session.

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

/// Whether `name` is a valid systemd environment variable name.
pub(super) fn valid_environment_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

pub(super) fn publish(
    manager: &super::bus::UserManager, pairs: &[(String, String)],
) -> Result<(), super::SessionError> {
    if pairs.iter().any(|(name, value)| !valid_environment_name(name) || value.contains('\0')) {
        return Err(super::SessionError::State("invalid session environment".into()));
    }
    manager.set_environment(&pairs.iter().map(|(name, value)| format!("{name}={value}")).collect::<Vec<_>>())?;
    update_activation(manager, pairs)
}

fn update_activation(
    manager: &super::bus::UserManager, pairs: &[(String, String)],
) -> Result<(), super::SessionError> {
    async_io::block_on(async {
        let bus = zbus::Proxy::new(&manager.connection, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus").await?;
        let values: std::collections::HashMap<&str, &str> = pairs.iter().map(|(name, value)| (name.as_str(), value.as_str())).collect();
        bus.call::<_, _, ()>("UpdateActivationEnvironment", &(values,)).await?;
        Ok(())
    })
}

pub(super) fn clear_owned(
    manager: &super::bus::UserManager, pairs: &[(String, String)],
) -> Result<(), super::SessionError> {
    let current = manager.environment()?;
    clear_names(manager, &owned_names(pairs, &current))
}

pub(super) fn clear_names(
    manager: &super::bus::UserManager, names: &[String],
) -> Result<(), super::SessionError> {
    if names.is_empty() {
        return Ok(());
    }
    // D-Bus has no unset operation. Empty values prevent activation with stale
    // display endpoints; clear them before removing the manager ownership data
    // so a failed update can still be retried by generation recovery.
    let empty = names.iter().map(|name| (name.clone(), String::new())).collect::<Vec<_>>();
    update_activation(manager, &empty)?;
    manager.unset_environment(names)
}

fn owned_names(pairs: &[(String, String)], current: &[String]) -> Vec<String> {
    pairs.iter().filter(|(name, value)| {
        !matches!(name.as_str(), "PATH" | "LANG" | "XDG_RUNTIME_DIR")
            && current.contains(&format!("{name}={value}"))
    }).map(|(name, _)| name.clone()).collect()
}

#[cfg(test)]
#[path = "env_transport_tests.rs"]
mod transport_tests;

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
        let pairs: Vec<_> = all_var_names(&[], &[]).into_iter()
            .map(|name| (name, "value".to_string())).collect();
        let current: Vec<_> = pairs.iter().map(|(name, value)| format!("{name}={value}")).collect();
        let command = owned_names(&pairs, &current);
        for name in ["PATH", "LANG", "XDG_RUNTIME_DIR"] {
            assert!(!command.iter().any(|arg| arg == name));
        }
        for name in ["WAYLAND_DISPLAY", "DISPLAY", "XAUTHORITY", "XDG_SESSION_ID"] {
            assert!(command.iter().any(|arg| arg == name));
        }
        assert!(owned_names(&pairs, &["WAYLAND_DISPLAY=new-owner".into()]).is_empty());
    }
}
