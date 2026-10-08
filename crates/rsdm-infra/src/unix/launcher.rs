use std::{os::{fd::{AsRawFd, OwnedFd}, unix::process::{CommandExt, ExitStatusExt}}, path::PathBuf, process::{Child, Command}};

use rsdm_core::{
    domain::{Session, SessionExit},
    ports::{ResolvedUser, RunningSession, SessionLaunchError, SessionLaunchRequest, SessionLauncher},
};

use super::session_environment::session_environment;
use super::session_exec_request::ExecRequest;
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
        let (child, gate) = spawn_child(&plan, request.wrapper)?;
        let pid = child.id();
        tracing::info!(pid, username = %plan.username, desktop_entry_id = %plan.desktop_entry_id, "user session child spawned");

        // The child waits on its start gate while the parent starts the bus
        // executor. No session code runs before this guard attempt completes.
        let shutdown_guard = if plan.generation.is_some() {
            match crate::power::pam_shutdown_guard() {
                Ok(guard) => Some(guard),
                Err(error) => { tracing::error!(%error, "PAM shutdown delay inhibitor unavailable"); None }
            }
        } else { None };
        let session = UnixRunningSession { child, plan, shutdown_guard, reaped: false };
        gate.release().map_err(|error| SessionLaunchError::Setup(format!("releasing session start gate: {error}")))?;
        Ok(Box::new(session))
    }
}

fn spawn_child(plan: &SessionLaunchPlan, wrapper: &[String]) -> Result<(Child, StartGate), SessionLaunchError> {
    let request = ExecRequest {
        username: plan.username.clone(), uid: plan.uid, gid: plan.gid,
        home: path_to_str(&plan.home)?.to_string(), command: plan.command.clone(),
        wrapper: wrapper.to_vec(), environment: plan.environment.clone(),
    }.seal()?;
    let gate = StartGate::new().map_err(|error| SessionLaunchError::Setup(format!("creating session start gate: {error}")))?;
    let inherited = [request.as_raw_fd(), gate.reader.as_raw_fd()];
    // Re-exec the running inode, including after package replacement/unlink.
    // User/PAM environment travels in the sealed request; only the parent's
    // standard streams reach it. Do not let loader variables from a PAM
    // module influence the fresh privileged image.
    let mut command = Command::new("/proc/self/exe");
    command.env_clear();
    command.args(["session-exec", "--request-fd", &inherited[0].to_string(), "--gate-fd", &inherited[1].to_string()]);
    // SAFETY: this post-fork callback performs only fcntl and errno checks.
    // The request and gate own these descriptors through spawn's exec handshake.
    unsafe {
        command.pre_exec(move || {
            for fd in inherited {
                if libc::fcntl(fd, libc::F_SETFD, 0) < 0 { return Err(std::io::Error::last_os_error()); }
            }
            Ok(())
        });
    }
    let child = command.spawn().map_err(|error| SessionLaunchError::Process(format!("starting session exec helper: {error}")))?;
    Ok((child, gate))
}

struct UnixRunningSession {
    child: Child,
    plan: SessionLaunchPlan,
    shutdown_guard: Option<OwnedFd>,
    reaped: bool,
}

impl RunningSession for UnixRunningSession {
    fn wait(&mut self) -> Result<SessionExit, SessionLaunchError> {
        let exit = wait_for_child(&mut self.child);
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
        if !self.reaped { let _ = wait_for_child(&mut self.child); }
    }
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

fn wait_for_child(child: &mut Child) -> Result<SessionExit, SessionLaunchError> {
    let status = child.wait().map_err(|error| SessionLaunchError::Process(format!("waitpid failed: {error}")))?;
    let exit = decode_exit(status.into_raw());
    tracing::info!(pid = child.id(), ?exit, "user session child exited");
    Ok(exit)
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
