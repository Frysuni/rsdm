//! Handover from a failed greeter to a plain TTY login.

use std::{convert::Infallible, ffi::CString, io, os::raw::c_char};

use thiserror::Error;

use super::getty_query::GettyCommand;

#[derive(Debug, Error)]
pub enum FallbackError {
    #[error(transparent)]
    Vt(#[from] super::vt::VtError),
    #[error("fallback command is empty")]
    EmptyCommand,
    #[error("fallback argument contains a NUL byte")]
    NulByte,
    #[error("could not open {path}: {source}")]
    OpenTty {
        path: String,
        #[source]
        source: io::Error,
    },
    #[error("could not attach the fallback to the terminal: {0}")]
    AttachTty(io::Error),
    #[error("every fallback command failed; last error: {0}")]
    Exec(io::Error),
}

/// Reconnect `tty_path` to the standard descriptors, reset it, and `exec` a login
/// program, trying `primary` first and then built-in fallbacks.
///
/// On success this never returns (the process image is replaced). It only returns
/// `Err` when the VT could not be attached or every candidate failed to exec.
pub async fn exec_fallback(tty_path: &str, primary: &[String]) -> Result<Infallible, FallbackError> {
    super::shutdown::install_terminate_handler();
    let _vt = super::vt::acquire(tty_path)?;
    attach_vt(tty_path)?;

    // Resolve the distro's own console login before we exec, so an unconfigured
    // fallback reuses whatever getty systemd runs on this VT.
    let system_getty = super::getty_query::discover_system_getty(tty_path).await;
    let candidates = fallback_candidates(primary, system_getty.as_ref(), tty_path);
    let mut last_error = io::Error::from(io::ErrorKind::NotFound);
    for candidate in &candidates {
        match try_exec(candidate) {
            Ok(infallible) => match infallible {},
            Err(FallbackError::Exec(error)) => {
                tracing::warn!(
                    command = ?candidate,
                    %error,
                    "fallback login could not be exec'd; trying the next candidate"
                );
                last_error = error;
            }
            Err(other) => {
                tracing::warn!(command = ?candidate, error = %other, "skipping malformed fallback command");
            }
        }
    }
    Err(FallbackError::Exec(last_error))
}

/// Attach the calling process to `tty_path`: own it as the controlling terminal,
/// point the standard descriptors at it, and reset it to a sane state.
fn attach_vt(tty_path: &str) -> Result<(), FallbackError> {
    let tty_c = cstring(tty_path)?;
    // SAFETY: tty_c is a valid NUL-terminated path; open returns an fd or -1.
    let fd = unsafe { libc::open(tty_c.as_ptr(), libc::O_RDWR | libc::O_NOCTTY) };
    if fd < 0 {
        return Err(FallbackError::OpenTty {
            path: tty_path.to_string(),
            source: io::Error::last_os_error(),
        });
    }

    if let Err(error) = claim_controlling_terminal(fd) {
        // SAFETY: fd is owned by this function and has not been duplicated.
        unsafe { libc::close(fd) };
        return Err(FallbackError::AttachTty(error));
    }

    // SAFETY: fd is valid; dup2 attaches the standard descriptors to the VT.
    unsafe {
        for target in [libc::STDIN_FILENO, libc::STDOUT_FILENO, libc::STDERR_FILENO] {
            if libc::dup2(fd, target) < 0 {
                let error = io::Error::last_os_error();
                if fd > libc::STDERR_FILENO {
                    libc::close(fd);
                }
                return Err(FallbackError::Exec(error));
            }
        }
        if fd > libc::STDERR_FILENO {
            libc::close(fd);
        }
        // Make our group the foreground so the login program is not stopped by
        // SIGTTIN/SIGTTOU on its first read/write.
        libc::tcsetpgrp(libc::STDIN_FILENO, libc::getpgrp());
    }

    super::terminal::restore_cooked(libc::STDIN_FILENO).map_err(FallbackError::AttachTty)?;
    Ok(())
}

fn claim_controlling_terminal(fd: i32) -> io::Result<()> {
    // SAFETY: fd is an open terminal. A process that is already a session
    // leader can claim it without calling setsid successfully again.
    unsafe {
        if libc::setsid() < 0 && libc::getsid(0) != libc::getpid() {
            return Err(io::Error::last_os_error());
        }
        // Never steal a terminal from another session, including one that
        // claimed it between the ownership check and this ioctl.
        if libc::ioctl(fd, libc::TIOCSCTTY, 0) < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

/// Build the ordered, de-duplicated list of fallback commands to try.
///
/// `system_getty`, when present, is the console login systemd already runs on
/// this VT (discovered via `getty_query`); it is tried right after the operator's
/// configured command and before the hardcoded built-ins.
fn fallback_candidates(
    primary: &[String],
    system_getty: Option<&GettyCommand>,
    tty_path: &str,
) -> Vec<GettyCommand> {
    let tty = tty_path.strip_prefix("/dev/").unwrap_or(tty_path);
    let mut candidates = Vec::new();
    let mut push = |executable: &str, argv: Vec<String>| {
        let command = GettyCommand { executable: executable.to_string(), argv };
        if !command.argv.is_empty() && !candidates.contains(&command) {
            candidates.push(command);
        }
    };

    // The operator's configured fallback wins.
    if let Some(executable) = primary.first() { push(executable, primary.to_vec()); }
    // Otherwise reuse the console login the distro already runs on this VT
    // (agetty, mingetty, busybox getty, ...) so an unconfigured fallback still
    // works on a box that does not ship agetty where we expect it.
    if let Some(system_getty) = system_getty {
        push(&system_getty.executable, system_getty.argv.clone());
    }
    // agetty opens and configures the VT itself, then runs login - the most
    // robust handover. Try it by PATH and the usual absolute locations.
    for agetty in [
        "agetty",
        "/usr/bin/agetty",
        "/sbin/agetty",
        "/usr/sbin/agetty",
    ] {
        push(agetty, vec![
            agetty.to_string(),
            "--noclear".to_string(),
            tty.to_string(),
            "linux".to_string(),
        ]);
    }
    // Plain login as the final resort (it prompts for credentials; we already own
    // the controlling terminal so it will not bounce straight out).
    for login in ["login", "/usr/bin/login", "/bin/login"] {
        push(login, vec![login.to_string()]);
    }
    candidates
}

/// Try to `exec` one candidate. Returns only on failure (exec replaces us).
fn try_exec(command: &GettyCommand) -> Result<Infallible, FallbackError> {
    if command.argv.is_empty() { return Err(FallbackError::EmptyCommand); }
    let program_c = cstring(&command.executable)?;
    let argv = command.argv
        .iter()
        .map(|arg| cstring(arg))
        .collect::<Result<Vec<_>, _>>()?;
    let argv_ptrs: Vec<*const c_char> = argv
        .iter()
        .map(|arg| arg.as_ptr())
        .chain(std::iter::once(std::ptr::null()))
        .collect();

    // SAFETY: program_c and argv_ptrs are valid NUL-terminated C strings with a
    // NULL terminator; execvp searches PATH for bare names.
    unsafe {
        libc::execvp(program_c.as_ptr(), argv_ptrs.as_ptr());
    }
    Err(FallbackError::Exec(io::Error::last_os_error()))
}

fn cstring(value: &str) -> Result<CString, FallbackError> {
    CString::new(value).map_err(|_| FallbackError::NulByte)
}

#[cfg(test)]
mod tests {
    use super::{GettyCommand, fallback_candidates};

    #[test]
    fn candidates_start_with_the_configured_command() {
        let primary = vec![
            "agetty".to_string(),
            "--noclear".to_string(),
            "tty1".to_string(),
            "linux".to_string(),
        ];
        let candidates = fallback_candidates(&primary, None, "/dev/tty1");
        assert_eq!(candidates.first().unwrap().argv, primary);
    }

    #[test]
    fn discovered_getty_follows_the_configured_command() {
        let primary = vec!["login".to_string()];
        let system_getty = GettyCommand { executable: "/sbin/mingetty".into(), argv: vec!["/sbin/mingetty".into(), "tty1".into()] };
        let candidates = fallback_candidates(&primary, Some(&system_getty), "/dev/tty1");
        assert_eq!(candidates.first().unwrap().argv, primary);
        assert_eq!(candidates.get(1), Some(&system_getty));
    }

    #[test]
    fn discovered_getty_leads_when_no_command_is_configured() {
        let system_getty = GettyCommand { executable: "/sbin/mingetty".into(), argv: vec!["/sbin/mingetty".into(), "tty1".into()] };
        let candidates = fallback_candidates(&[], Some(&system_getty), "/dev/tty1");
        assert_eq!(candidates.first(), Some(&system_getty));
    }

    #[test]
    fn identical_arguments_do_not_hide_a_distinct_getty_executable() {
        let primary = vec!["custom-getty".into(), "tty1".into()];
        let system_getty = GettyCommand { executable: "/sbin/mingetty".into(), argv: primary.clone() };
        let candidates = fallback_candidates(&primary, Some(&system_getty), "/dev/tty1");
        assert_eq!(candidates[0].executable, "custom-getty");
        assert_eq!(candidates.get(1), Some(&system_getty));
    }

    #[test]
    fn candidates_are_deduplicated() {
        // The configured command equal to a built-in must not appear twice.
        let primary = vec![
            "agetty".to_string(),
            "--noclear".to_string(),
            "tty1".to_string(),
            "linux".to_string(),
        ];
        let candidates = fallback_candidates(&primary, None, "/dev/tty1");
        let count = candidates
            .iter()
            .filter(|command| command.argv == primary)
            .count();
        assert_eq!(count, 1);
    }

    #[test]
    fn candidates_always_include_a_login_resort() {
        let candidates = fallback_candidates(&[], None, "/dev/tty2");
        assert!(
            candidates
                .iter()
                .any(|command| command.argv == vec!["login".to_string()])
        );
        // The bare tty name (not the device path) is passed to agetty.
        assert!(
            candidates
                .iter()
                .any(|command| command.argv.iter().any(|arg| arg == "tty2"))
        );
        assert!(
            candidates
                .iter()
                .all(|command| command.argv.iter().all(|arg| arg != "/dev/tty2"))
        );
    }
}
