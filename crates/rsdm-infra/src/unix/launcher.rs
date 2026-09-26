use std::{ffi::CString, path::PathBuf};

use rsdm_core::{
    domain::{Session, SessionExit},
    ports::{ResolvedUser, SessionLaunchError, SessionLaunchRequest, SessionLauncher},
};

use super::command::{PreparedCommand, cstring};
use super::session_environment::session_environment;

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
