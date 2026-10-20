//! Pass a sealed request to a fresh process before it can run session code.

use std::{
    os::{fd::AsRawFd, unix::process::CommandExt},
    process::{Child, Command},
};

use rsdm_core::ports::SessionLaunchError;

use super::{session_exec_request::ExecRequest, session_start_gate::StartGate};

pub(super) fn spawn(request: &ExecRequest) -> Result<(Child, StartGate), SessionLaunchError> {
    let request = request.seal()?;
    let gate = StartGate::new().map_err(|error| SessionLaunchError::Setup(format!("creating session start gate: {error}")))?;
    let inherited = [request.as_raw_fd(), gate.reader.as_raw_fd()];
    // The running inode survives replacement; loader variables stay out of
    // the fresh privileged image until it has validated the sealed request.
    let mut command = Command::new("/proc/self/exe");
    command.env_clear();
    command.args(["session-exec", "--request-fd", &inherited[0].to_string(), "--gate-fd", &inherited[1].to_string()]);
    // SAFETY: only fcntl and errno checks run after fork; both descriptors are
    // owned here and remain alive through spawn's exec handshake.
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
