use std::{
    process::{Child, Command, ExitStatus},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};

/// `Ok(None)` means ready; `Ok(Some(status))` means it exited first.
pub(super) fn wait_until_ready(
    child: &mut Child,
    scope: Option<&str>,
) -> Result<Option<ExitStatus>> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let uid = rsdm_infra::lock_control::current_uid();
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait().context("checking idle locker status")? {
            return Ok(Some(status));
        }
        if rsdm_infra::lock_control::lock_state(uid)
            .ok()
            .flatten()
            .is_some_and(|state| owns_lock(state.pid, child.id(), scope))
        {
            return Ok(None);
        }
        thread::sleep(Duration::from_millis(50));
    }
    anyhow::bail!("no compositor lock confirmation within 10 seconds")
}

/// Abort a locker that did not establish the lock protocol in time.
pub(super) fn terminate_after_timeout(mut child: Child, scope: Option<&str>) {
    let deadline = Instant::now() + Duration::from_secs(1);
    if let Some(scope) = scope {
        let result = kill_scope(scope, deadline - Duration::from_millis(500), &mut Command::new("systemctl"));
        match result {
            Ok(status) if status.success() => {},
            Ok(status) => tracing::warn!(%status, %scope, "idle locker scope termination failed"),
            Err(error) => tracing::warn!(%error, %scope, "failed to terminate idle locker scope after readiness timeout"),
        }
    }
    match rsdm_infra::unix::terminate_command_until(&mut child, deadline) {
        Ok(Some(_)) => return,
        Ok(None) => tracing::error!(pid = child.id(), "idle locker did not exit after forced termination"),
        Err(error) => tracing::error!(%error, "failed to terminate idle locker after readiness timeout"),
    }
    thread::spawn(move || {
        if let Err(error) = child.wait() {
            tracing::warn!(%error, "failed to reap idle locker in background");
        }
    });
}

fn kill_scope(scope: &str, deadline: Instant, command: &mut Command) -> std::io::Result<ExitStatus> {
    // A scoped locker may have left the launcher's process group. Target every
    // member through systemd rather than waiting for its normal stop timeout.
    command.args(["--user", "kill", "--signal=SIGKILL", scope]);
    rsdm_infra::unix::run_command_until(command, deadline)
}

fn owns_lock(pid: u32, child_pid: u32, scope: Option<&str>) -> bool {
    let Some(scope) = scope else {
        return pid == child_pid;
    };
    // systemd-run may supervise a different PID. Only accept a lock from the
    // scope we started, never a manual lock that won the startup race.
    std::fs::read_to_string(format!("/proc/{pid}/cgroup"))
        .is_ok_and(|cgroups| belongs_to_scope(&cgroups, scope))
}

fn belongs_to_scope(cgroups: &str, scope: &str) -> bool {
    cgroups.lines().any(|line| {
        let mut fields = line.splitn(3, ':');
        let (Some(hierarchy), Some(controllers), Some(path)) =
            (fields.next(), fields.next(), fields.next())
        else {
            return false;
        };
        let systemd = (hierarchy == "0" && controllers.is_empty())
            || controllers.split(',').any(|name| name == "name=systemd");
        systemd && path.split('/').any(|component| component == scope)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::{BufRead, BufReader, Read}, os::{fd::OwnedFd, unix::{net::UnixStream, process::CommandExt}}};

    #[test]
    fn readiness_rejects_another_lock_process() {
        assert!(owns_lock(42, 42, None));
        assert!(!owns_lock(43, 42, None));
        let scope = "rsdm-lock-42-1.scope";
        assert!(belongs_to_scope(
            "0::/user.slice/user-1000.slice/app.slice/rsdm-lock-42-1.scope",
            scope
        ));
        assert!(belongs_to_scope(
            "1:name=systemd:/user.slice/rsdm-lock-42-1.scope/child",
            scope
        ));
        for cgroups in [
            "0::/user.slice/app-manual-lock.scope",
            "0::/user.slice/rsdm-lock-42-10.scope",
            "0::/user.slice/prefix-rsdm-lock-42-1.scope",
            "2:memory:/user.slice/rsdm-lock-42-1.scope",
            "malformed",
        ] {
            assert!(!belongs_to_scope(cgroups, scope), "{cgroups}");
        }
    }

    #[test]
    fn readiness_timeout_terminates_a_locker() {
        let (reader, writer) = UnixStream::pair().unwrap();
        reader.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let child = Command::new("sh")
            .args(["-c", "sleep 30 & printf 'ready\\n'; kill -STOP $$; wait"])
            .stdout(OwnedFd::from(writer)).process_group(0).spawn().unwrap();
        let mut reader = BufReader::new(reader);
        let mut output = String::new();
        reader.read_line(&mut output).unwrap();
        assert_eq!(output, "ready\n");
        let before = Instant::now();
        terminate_after_timeout(child, None);
        assert!(before.elapsed() < Duration::from_secs(2));
        // Both shell and descendant inherited stdout; EOF proves neither holds
        // the connection after cleanup, even with a stopped group leader.
        reader.read_to_string(&mut output).unwrap();
    }

    #[test]
    fn scope_timeout_uses_a_bounded_forced_kill() {
        let scope = "rsdm-lock-fixture.scope";
        let mut command = Command::new("sh");
        command.args(["-c", "test \"$*\" = '--user kill --signal=SIGKILL rsdm-lock-fixture.scope'", "fixture"]);
        let status = kill_scope(scope, Instant::now() + Duration::from_secs(1), &mut command).unwrap();
        assert!(status.success());

        let mut hung = Command::new("sh");
        hung.args(["-c", "kill -STOP $$", "fixture"]);
        let before = Instant::now();
        let error = kill_scope(scope, before + Duration::from_millis(100), &mut hung).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        assert!(before.elapsed() < Duration::from_secs(1));
    }
}
