use rsdm_core::ports::ResolvedUser;

pub(super) fn session_environment(
    user: &ResolvedUser,
    desktop_names: &[String],
    pam_environment: &[(String, String)],
    vtnr: Option<u32>,
    seat: &str,
) -> Vec<(String, String)> {
    let mut environment = vec![
        ("HOME".to_string(), user.home.clone()),
        ("USER".to_string(), user.username.clone()),
        ("LOGNAME".to_string(), user.username.clone()),
        ("SHELL".to_string(), user.shell.clone()),
        ("XDG_SESSION_TYPE".to_string(), "wayland".to_string()),
    ];

    // Direct compositor children inherit this before systemd/D-Bus export.
    if !desktop_names.is_empty() {
        environment.push(("XDG_CURRENT_DESKTOP".to_string(), desktop_names.join(":")));
        environment.push(("XDG_SESSION_DESKTOP".to_string(), desktop_names[0].clone()));
    }

    if !seat.is_empty() {
        environment.push(("XDG_SEAT".to_string(), seat.to_string()));
    }
    if let Some(vtnr) = vtnr {
        environment.push(("XDG_VTNR".to_string(), vtnr.to_string()));
    }

    // PAM may derive a conflicting type from PAM_TTY, so identity stays reserved.
    let reserved = [
        "HOME",
        "USER",
        "LOGNAME",
        "SHELL",
        "XDG_SESSION_TYPE",
        "XDG_CURRENT_DESKTOP",
        "XDG_SESSION_DESKTOP",
    ];
    for (key, value) in pam_environment {
        if !reserved.contains(&key.as_str()) && valid_environment_key(key) {
            upsert(&mut environment, key, value);
        }
    }

    if !environment.iter().any(|(name, _)| name == "PATH")
        && let Some(path) = default_session_path()
    {
        environment.push(("PATH".to_string(), path));
    }
    environment
}

fn upsert(environment: &mut Vec<(String, String)>, key: &str, value: &str) {
    if let Some(entry) = environment.iter_mut().find(|(existing, _)| existing == key) {
        entry.1 = value.to_string();
    } else {
        environment.push((key.to_string(), value.to_string()));
    }
}

fn default_session_path() -> Option<String> {
    // Use libc's standard utility path only when PAM did not supply one.
    // Never inherit the root greeter's PATH or guess user profile locations.
    // SAFETY: a null buffer and zero length request the required buffer size.
    let size = unsafe { libc::confstr(libc::_CS_PATH, std::ptr::null_mut(), 0) };
    if size == 0 {
        return None;
    }
    let mut buffer = vec![0_u8; size];
    // SAFETY: buffer contains size writable bytes, including the terminator.
    let written = unsafe { libc::confstr(libc::_CS_PATH, buffer.as_mut_ptr().cast(), size) };
    if written == 0 || written > size {
        return None;
    }
    buffer.truncate(written - 1);
    String::from_utf8(buffer).ok()
}

fn valid_environment_key(key: &str) -> bool {
    !key.is_empty() && !key.contains('=')
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsdm_core::ports::ResolvedUser;

    fn user() -> ResolvedUser {
        ResolvedUser {
            username: "alice".to_string(),
            uid: 1000,
            gid: 100,
            home: "/home/alice".to_string(),
            shell: "/bin/sh".to_string(),
        }
    }

    #[test]
    fn missing_pam_path_uses_the_system_path_without_user_profile_guesses() {
        let environment = session_environment(&user(), &[], &[], None, "");
        let path = value(&environment, "PATH").unwrap();
        assert_eq!(Some(path), default_session_path().as_deref());
        assert!(!path.contains("/home/alice"));
        assert!(!path.contains("/etc/profiles/per-user"));
    }

    fn value<'a>(environment: &'a [(String, String)], key: &str) -> Option<&'a str> {
        environment
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    }

    #[test]
    fn publishes_session_identity_from_desktop_names() {
        let environment = session_environment(
            &user(),
            &["niri".to_string(), "wlroots".to_string()],
            &[],
            Some(1),
            "seat0",
        );
        assert_eq!(value(&environment, "XDG_SESSION_TYPE"), Some("wayland"));
        assert_eq!(
            value(&environment, "XDG_CURRENT_DESKTOP"),
            Some("niri:wlroots")
        );
        assert_eq!(value(&environment, "XDG_SESSION_DESKTOP"), Some("niri"));
        assert_eq!(value(&environment, "XDG_SEAT"), Some("seat0"));
        assert_eq!(value(&environment, "XDG_VTNR"), Some("1"));
    }

    #[test]
    fn omits_desktop_identity_when_the_entry_declares_none() {
        let environment = session_environment(&user(), &[], &[], None, "");
        assert_eq!(value(&environment, "XDG_CURRENT_DESKTOP"), None);
        assert_eq!(value(&environment, "XDG_SESSION_DESKTOP"), None);
        assert_eq!(value(&environment, "XDG_SEAT"), None);
        assert_eq!(value(&environment, "XDG_VTNR"), None);
    }

    #[test]
    fn pam_environment_cannot_clobber_identity_or_account_vars() {
        let pam = vec![
            ("XDG_SESSION_TYPE".to_string(), "tty".to_string()),
            ("XDG_CURRENT_DESKTOP".to_string(), "other".to_string()),
            ("XDG_SESSION_DESKTOP".to_string(), "other".to_string()),
            ("HOME".to_string(), "/root".to_string()),
            ("USER".to_string(), "root".to_string()),
        ];
        let environment =
            session_environment(&user(), &["niri".to_string()], &pam, Some(1), "seat0");
        assert_eq!(value(&environment, "XDG_SESSION_TYPE"), Some("wayland"));
        assert_eq!(value(&environment, "XDG_CURRENT_DESKTOP"), Some("niri"));
        assert_eq!(value(&environment, "XDG_SESSION_DESKTOP"), Some("niri"));
        assert_eq!(value(&environment, "HOME"), Some("/home/alice"));
        assert_eq!(value(&environment, "USER"), Some("alice"));
    }

    #[test]
    fn pam_environment_overrides_the_path_floor_and_adds_its_own_vars() {
        let pam = vec![
            ("PATH".to_string(), "/from/pam".to_string()),
            ("XDG_RUNTIME_DIR".to_string(), "/run/user/1000".to_string()),
            ("BAD=KEY".to_string(), "dropped".to_string()),
        ];
        let environment = session_environment(&user(), &[], &pam, None, "");
        assert_eq!(value(&environment, "PATH"), Some("/from/pam"));
        assert_eq!(
            value(&environment, "XDG_RUNTIME_DIR"),
            Some("/run/user/1000")
        );
        assert!(!environment.iter().any(|(name, _)| name.contains('=')));
    }

    #[test]
    fn never_invents_the_session_bus_address() {
        let pam = vec![("XDG_RUNTIME_DIR".to_string(), "/run/user/1000".to_string())];
        let environment = session_environment(&user(), &[], &pam, None, "");
        assert_eq!(value(&environment, "DBUS_SESSION_BUS_ADDRESS"), None);

        let pam_with_bus = vec![(
            "DBUS_SESSION_BUS_ADDRESS".to_string(),
            "unix:path=/custom/bus".to_string(),
        )];
        let environment = session_environment(&user(), &[], &pam_with_bus, None, "");
        assert_eq!(
            value(&environment, "DBUS_SESSION_BUS_ADDRESS"),
            Some("unix:path=/custom/bus")
        );
    }
}
