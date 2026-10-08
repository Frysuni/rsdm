use std::{ffi::CString, os::fd::OwnedFd, path::PathBuf};

use rsdm_core::{
    domain::{Session, SessionExit},
    ports::{ResolvedUser, RunningSession, SessionLaunchError, SessionLaunchRequest, SessionLauncher},
};

use super::command::{PreparedCommand, cstring};
use super::session_environment::session_environment;
use super::session_start_gate::StartGate;

struct SessionLaunchPlan {
    username: String,
    uid: u32,
    gid: u32,
    home: PathBuf,
    desktop_entry_id: String,
    command: String,
    environment: Vec<(String, String)>,
    generation: Option<String>,
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
        wrapper: &[String],
    ) -> Result<SessionLaunchPlan, SessionLaunchError> {
        let mut environment = session_environment(
            user, &session.desktop_names, pam_environment, vtnr, seat,
        );
        environment.retain(|(name, _)| name != "RSDM_DESKTOP_ENTRY_ID");
        environment.push(("RSDM_DESKTOP_ENTRY_ID".into(), session.id.clone()));
        let coordinated = !wrapper.is_empty() || super::command::is_session_manager_command(&super::command::split_exec(&session.exec)?);
        let generation = if coordinated {
            Some(crate::session_manager::new_generation().map_err(|error| SessionLaunchError::Setup(error.to_string()))?)
        } else { None };
        environment.retain(|(name, _)| name != "RSDM_SESSION_GENERATION");
        if let Some(generation) = &generation { environment.push(("RSDM_SESSION_GENERATION".into(), generation.clone())); }
        Ok(SessionLaunchPlan {
            username: user.username.clone(),
            uid: user.uid,
            gid: user.gid,
            home: PathBuf::from(&user.home),
            desktop_entry_id: session.id.clone(),
            command: session.exec.clone(),
            environment,
            generation,
        })
    }
}

impl SessionLauncher for UnixSessionLauncher {
    fn start(&self, request: SessionLaunchRequest<'_>) -> Result<Box<dyn RunningSession>, SessionLaunchError> {
        refuse_if_output_is_tty()?;

        let plan = Self::plan(
            request.user,
            request.session,
            request.pam_environment,
            request.vtnr,
            request.seat,
            request.wrapper,
        )?;
        tracing::info!(
            username = %plan.username,
            uid = plan.uid,
            gid = plan.gid,
            desktop_entry_id = %plan.desktop_entry_id,
            command = %plan.command,
            wrapper = ?request.wrapper,
            "launching user session"
        );
        tracing::debug!(
            username = %plan.username,
            desktop_entry_id = %plan.desktop_entry_id,
            env_names = ?plan.environment.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(),
            "prepared user session environment"
        );
        let (pid, gate) = spawn_child(&plan, request.wrapper)?;
        tracing::info!(pid, username = %plan.username, desktop_entry_id = %plan.desktop_entry_id, "user session child spawned");

        // The child waits on its start gate while the parent starts the bus
        // executor. No session code runs before this guard attempt completes.
        let shutdown_guard = if plan.generation.is_some() {
            match crate::power::pam_shutdown_guard() {
                Ok(guard) => Some(guard),
                Err(error) => { tracing::error!(%error, "PAM shutdown delay inhibitor unavailable"); None }
            }
        } else { None };
        let session = UnixRunningSession { pid, plan, shutdown_guard, reaped: false };
        gate.release().map_err(|error| SessionLaunchError::Setup(format!("releasing session start gate: {error}")))?;
        Ok(Box::new(session))
    }
}

fn spawn_child(plan: &SessionLaunchPlan, wrapper: &[String]) -> Result<(libc::pid_t, StartGate), SessionLaunchError> {
    let command = PreparedCommand::new_wrapped(wrapper, &plan.command)?;
    let username = cstring("username", &plan.username)?;
    let home = cstring("home", path_to_str(&plan.home)?)?;
    let environment = prepare_environment(&plan.environment)?;
    let gate = StartGate::new().map_err(|error| SessionLaunchError::Setup(format!("creating session start gate: {error}")))?;

    // SAFETY: all command data is prepared before fork. No bus executor has
    // been started; the child first waits for its parent's guard, then sets
    // credentials and executes the session.
    let pid = unsafe { libc::fork() };
    if pid < 0 { return Err(last_os_error("fork")); }
    if pid == 0 {
        if !gate.wait_child() {
            // SAFETY: never unwind or run parent destructors after a failed gate.
            unsafe { libc::_exit(126); }
        }
        child_exec(plan, &command, &username, &home, &environment);
    }
    Ok((pid, gate))
}

struct UnixRunningSession {
    pid: libc::pid_t,
    plan: SessionLaunchPlan,
    shutdown_guard: Option<OwnedFd>,
    reaped: bool,
}

impl RunningSession for UnixRunningSession {
    fn wait(&mut self) -> Result<SessionExit, SessionLaunchError> {
        let exit = wait_for_child(self.pid);
        self.reaped = true;
        let _ = &self.shutdown_guard;
        exit
    }

    fn cleanup(&mut self) -> Result<(), SessionLaunchError> {
        if let Some(generation) = &self.plan.generation {
            super::session_cleanup::cleanup(&self.plan.environment, self.plan.uid, self.plan.gid, generation)?;
        }
        Ok(())
    }
}

impl Drop for UnixRunningSession {
    fn drop(&mut self) {
        if !self.reaped { let _ = wait_for_child(self.pid); }
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
