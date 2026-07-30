use std::{ffi::CString, path::PathBuf};

use rsdm_core::{
    domain::{Session, SessionExit},
    ports::{ResolvedUser, SessionLaunchError, SessionLaunchRequest, SessionLauncher},
};

use super::command::{PreparedCommand, cstring};

struct SessionLaunchPlan {
    username: String,
    uid: u32,
    gid: u32,
    home: PathBuf,
    session_id: String,
    command: String,
    environment: Vec<(String, String)>,
}

#[derive(Debug, Default)]
pub struct UnixSessionLauncher;

impl UnixSessionLauncher {
    fn plan(
        user: &ResolvedUser,
        session: &Session,
        pam_environment: &[(String, String)],
        vtnr: Option<u32>,
        seat: &str,
    ) -> SessionLaunchPlan {
        SessionLaunchPlan {
            username: user.username.clone(),
            uid: user.uid,
            gid: user.gid,
            home: PathBuf::from(&user.home),
            session_id: session.id.clone(),
            command: session.exec.clone(),
            environment: session_environment(
                user,
                &session.desktop_names,
                pam_environment,
                vtnr,
                seat,
            ),
        }
    }
}

impl SessionLauncher for UnixSessionLauncher {
    fn launch(&self, request: SessionLaunchRequest<'_>) -> Result<SessionExit, SessionLaunchError> {
        refuse_if_output_is_tty()?;

        let plan = Self::plan(
            request.user,
            request.session,
            request.pam_environment,
            request.vtnr,
            request.seat,
        );
        tracing::info!(
            username = %plan.username,
            uid = plan.uid,
            gid = plan.gid,
            session_id = %plan.session_id,
            command = %plan.command,
            wrapper = ?request.wrapper,
            "launching user session"
        );
        tracing::debug!(
            username = %plan.username,
            session_id = %plan.session_id,
            env_names = ?plan.environment.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(),
            "prepared user session environment"
        );
        let command = PreparedCommand::new_wrapped(request.wrapper, &plan.command)?;
        let username = cstring("username", &plan.username)?;
        let home = cstring("home", path_to_str(&plan.home)?)?;

        let env = prepare_environment(&plan.environment)?;

        // SAFETY: fork has no Rust-side memory safety preconditions. The child
        // only calls async-signal-safe-ish libc setup followed by exec/_exit.
        let pid = unsafe { libc::fork() };
        if pid < 0 {
            return Err(last_os_error("fork"));
        }
        if pid == 0 {
            child_exec(&plan, &command, &username, &home, &env);
        }
        tracing::info!(pid, username = %plan.username, session_id = %plan.session_id, "user session child spawned");

        wait_for_child(pid)
    }
}

fn session_environment(
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
        (
            "PATH".to_string(),
            default_session_path(&user.home, &user.username),
        ),
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

    environment
}

fn upsert(environment: &mut Vec<(String, String)>, key: &str, value: &str) {
    if let Some(entry) = environment.iter_mut().find(|(existing, _)| existing == key) {
        entry.1 = value.to_string();
    } else {
        environment.push((key.to_string(), value.to_string()));
    }
}

fn default_session_path(home: &str, username: &str) -> String {
    // pam_env may replace this floor with the system's configured login PATH.
    [
        format!("{home}/.local/bin"),
        "/run/wrappers/bin".to_string(),
        format!("{home}/.nix-profile/bin"),
        "/nix/profile/bin".to_string(),
        format!("{home}/.local/state/nix/profile/bin"),
        format!("/etc/profiles/per-user/{username}/bin"),
        "/nix/var/nix/profiles/default/bin".to_string(),
        "/run/current-system/sw/bin".to_string(),
        "/usr/local/bin".to_string(),
        "/usr/bin".to_string(),
        "/bin".to_string(),
    ]
    .join(":")
}

fn valid_environment_key(key: &str) -> bool {
    !key.is_empty() && !key.contains('=')
}

fn prepare_environment(
    environment: &[(String, String)],
) -> Result<Vec<(CString, CString)>, SessionLaunchError> {
    environment
        .iter()
        .map(|(key, value)| Ok((cstring("env key", key)?, cstring("env value", value)?)))
        .collect()
}

fn refuse_if_output_is_tty() -> Result<(), SessionLaunchError> {
    // SAFETY: isatty only reads the process file descriptor table.
    let stdout_is_tty = unsafe { libc::isatty(libc::STDOUT_FILENO) } == 1;
    // SAFETY: isatty only reads the process file descriptor table.
    let stderr_is_tty = unsafe { libc::isatty(libc::STDERR_FILENO) } == 1;
    if stdout_is_tty || stderr_is_tty {
        Err(SessionLaunchError::Setup(
            "refusing to launch while stdout/stderr are attached to a TTY".to_string(),
        ))
    } else {
        Ok(())
    }
}

fn child_exec(
    plan: &SessionLaunchPlan,
    command: &PreparedCommand,
    username: &CString,
    home: &CString,
    environment: &[(CString, CString)],
) -> ! {
    // SAFETY: all C strings are NUL-terminated and valid in the child.
    unsafe {
        let _ = libc::setsid();
        if libc::clearenv() != 0 {
            libc::_exit(126);
        }
        for (key, value) in environment {
            if libc::setenv(key.as_ptr(), value.as_ptr(), 1) != 0 {
                libc::_exit(126);
            }
        }
        if libc::initgroups(username.as_ptr(), plan.gid) != 0 {
            libc::_exit(126);
        }
        if libc::setgid(plan.gid) != 0 {
            libc::_exit(126);
        }
        if libc::setuid(plan.uid) != 0 {
            libc::_exit(126);
        }
        // Match login(1): a missing home starts in / without rewriting HOME.
        if libc::chdir(home.as_ptr()) != 0 && libc::chdir(c"/".as_ptr()) != 0 {
            libc::_exit(126);
        }
        // The session leader ignored SIGPIPE; that disposition survives exec.
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
        let argv = command.argv_ptrs();
        libc::execvp(command.program.as_ptr(), argv.as_ptr());
        libc::_exit(127);
    }
}

fn wait_for_child(pid: libc::pid_t) -> Result<SessionExit, SessionLaunchError> {
    let mut status = 0;
    loop {
        // SAFETY: waitpid writes to a valid status pointer for the known child pid.
        let waited = unsafe { libc::waitpid(pid, &mut status, 0) };
        if waited == pid {
            let exit = decode_exit(status);
            tracing::info!(pid, ?exit, "user session child exited");
            return Ok(exit);
        }
        if waited < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(SessionLaunchError::Process(format!(
                "waitpid failed: {error}"
            )));
        }
    }
}

fn decode_exit(status: libc::c_int) -> SessionExit {
    if libc::WIFEXITED(status) {
        let code = libc::WEXITSTATUS(status);
        if code == 0 {
            SessionExit::Success
        } else {
            SessionExit::Failed(code)
        }
    } else if libc::WIFSIGNALED(status) {
        SessionExit::Signaled(libc::WTERMSIG(status))
    } else {
        SessionExit::Failed(status)
    }
}

fn path_to_str(path: &std::path::Path) -> Result<&str, SessionLaunchError> {
    path.to_str()
        .ok_or_else(|| SessionLaunchError::Setup("path is not valid UTF-8".to_string()))
}

fn last_os_error(operation: &str) -> SessionLaunchError {
    SessionLaunchError::Process(format!(
        "{operation} failed: {}",
        std::io::Error::last_os_error()
    ))
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
