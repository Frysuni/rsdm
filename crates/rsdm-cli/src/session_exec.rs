//! Internal DM child entry; it must bypass configuration and logging setup.

use std::process::ExitCode;

use rsdm_core::ports::SessionLaunchError;

pub(super) fn run(request_fd: i32, gate_fd: i32) -> ExitCode {
    match rsdm_infra::unix::exec_session(request_fd, gate_fd) {
        Ok(never) => match never {},
        Err(error) => {
            eprintln!("rsdm session exec: {error}");
            ExitCode::from(match error { SessionLaunchError::Setup(_) => 126, SessionLaunchError::Process(_) => 127 })
        }
    }
}
