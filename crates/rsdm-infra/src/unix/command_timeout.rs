//! Bound foreground commands and terminate their owned process group on timeout.

use std::{io, os::unix::process::CommandExt, process::{Command, ExitStatus}, time::{Duration, Instant}};

use super::child_wait::wait_for_exit;

pub fn run_command_until(command: &mut Command, deadline: Instant) -> io::Result<ExitStatus> {
    let remaining = deadline.checked_duration_since(Instant::now())
        .filter(|duration| !duration.is_zero())
        .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "command deadline expired before launch"))?;
    let force_at = deadline - (remaining / 2).min(Duration::from_secs(1));
    let mut child = command.process_group(0).spawn()?;
    if let Some(status) = wait_for_exit(&mut child, force_at)? { return Ok(status); }

    // The group leader remains our unreaped child, so this process-group ID
    // cannot be reused before signaling. Kill descendants before reaping it.
    let group = child.id() as libc::pid_t;
    // SAFETY: spawn established a distinct group whose ID is its owned child's PID.
    let group_error = if unsafe { libc::kill(-group, libc::SIGKILL) } != 0 {
        let error = io::Error::last_os_error();
        (error.raw_os_error() != Some(libc::ESRCH)).then_some(error)
    } else { None };
    // Also terminate the owned leader if the command moved itself out of the
    // original group. It is still unreaped, so its PID cannot be replaced.
    let killed = child.kill();
    let reaped = wait_for_exit(&mut child, deadline)?;
    if let Some(error) = group_error { return Err(error); }
    killed?;
    let message = if reaped.is_some() {
        "command timed out; its process group was killed"
    } else {
        "command timed out; child exit remains unconfirmed after SIGKILL"
    };
    Err(io::Error::new(io::ErrorKind::TimedOut, message))
}

#[cfg(test)]
#[path = "command_timeout_tests.rs"]
mod tests;
