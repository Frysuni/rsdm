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
    // Reserve time for escalation inside the same limit, rather than adding
    // an unbounded wait after the helper has exhausted it.
    let force_at = deadline - (timeout / 2).min(Duration::from_secs(1));
    if let Some(status) = wait_for_exit(child, force_at)? { return Ok(status); }
    // The owned child remains unreaped, so kill cannot target a reused PID.
    child.kill()?;
    let reaped = wait_for_exit(child, deadline)?;
    let message = if reaped.is_some() {
        "session recovery timed out"
    } else {
        "session recovery timed out; helper exit remains unconfirmed after SIGKILL"
    };
    Err(std::io::Error::new(std::io::ErrorKind::TimedOut, message))
}

fn wait_for_exit(child: &mut Child, deadline: Instant) -> std::io::Result<Option<ExitStatus>> {
    loop {
        if let Some(status) = child.try_wait()? { return Ok(Some(status)); }
        let now = Instant::now();
        if now >= deadline { return Ok(None); }
        thread::sleep(Duration::from_millis(50).min(deadline - now));
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
        let before = Instant::now();
        let error = wait_for_recovery(&mut child, Duration::from_millis(100)).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        assert!(child.try_wait().unwrap().is_some());
        assert!(before.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn an_expired_reap_deadline_only_checks_status_without_waiting() {
        let mut child = Command::new("sleep").arg("30").spawn().unwrap();
        let before = Instant::now();
        let pending = wait_for_exit(&mut child, before).unwrap();
        child.kill().unwrap();
        let status = wait_for_exit(&mut child, Instant::now() + Duration::from_secs(1)).unwrap();
        assert!(pending.is_none());
        assert!(status.is_some());
        assert!(before.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn an_already_exited_helper_preserves_its_status_at_an_expired_deadline() {
        let mut child = Command::new("sh").args(["-c", "exit 7"]).spawn().unwrap();
        let expected = wait_for_exit(&mut child, Instant::now() + Duration::from_secs(1)).unwrap().unwrap();
        assert_eq!(wait_for_recovery(&mut child, Duration::ZERO).unwrap(), expected);
    }
}
