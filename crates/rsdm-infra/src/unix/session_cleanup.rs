//! Recovery runs after dropping privileges; root never reads user recovery data.

use std::{
    os::unix::process::CommandExt,
    process::{Child, Command, ExitStatus},
    thread,
    time::{Duration, Instant},
};

use rsdm_core::ports::SessionLaunchError;

pub(super) fn cleanup(environment: &[(String, String)], uid: u32, gid: u32, generation: &str) -> Result<(), SessionLaunchError> {
    let executable = std::env::current_exe().map_err(|error| SessionLaunchError::Setup(error.to_string()))?;
    let mut child = Command::new(executable).args(["session", "cleanup", "--generation", generation])
        .env_clear().envs(environment.iter().map(|(key, value)| (key, value)))
        // Command::uid also clears inherited supplementary groups.
        .uid(uid).gid(gid).current_dir("/").spawn()
        .map_err(|error| SessionLaunchError::Process(format!("session recovery failed: {error}")))?;

    let status = wait_for_recovery(&mut child, Duration::from_secs(90))
        .map_err(|error| SessionLaunchError::Process(format!("session recovery failed: {error}")))?;
    if !status.success() { return Err(SessionLaunchError::Process(format!("session recovery exited with {status}"))); }
    Ok(())
}

fn wait_for_recovery(child: &mut Child, timeout: Duration) -> std::io::Result<ExitStatus> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? { return Ok(status); }
        if Instant::now() >= deadline {
            // A user can stop their recovery helper or hold its file locks. This
            // must not keep the root PAM owner waiting indefinitely. The child
            // remains unreaped, so kill cannot affect a reused process ID.
            child.kill()?;
            child.wait()?;
            return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "session recovery timed out"));
        }
        thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_returns_the_helpers_failure_status() {
        let mut child = Command::new("sh").args(["-c", "exit 7"]).spawn().unwrap();
        let status = wait_for_recovery(&mut child, Duration::from_secs(1)).unwrap();
        assert_eq!(status.code(), Some(7));
    }

    #[test]
    fn a_stopped_helper_cannot_hold_the_pam_owner_forever() {
        let mut child = Command::new("sh").args(["-c", "kill -STOP $$; exit 0"]).spawn().unwrap();
        let error = wait_for_recovery(&mut child, Duration::from_millis(100)).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        assert!(child.try_wait().unwrap().is_some());
    }
}
