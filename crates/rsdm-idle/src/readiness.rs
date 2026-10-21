use std::{
    io::{self, Read},
    os::{fd::AsRawFd, unix::{net::UnixStream, process::CommandExt}},
    process::{Child, Command, ExitStatus},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};

pub(super) fn prepare(command: &mut Command) -> io::Result<UnixStream> {
    let (reader, sender) = UnixStream::pair()?;
    reader.set_nonblocking(true)?;
    command.args(["--ready-fd", &sender.as_raw_fd().to_string()]);
    // SAFETY: only fcntl runs after fork, on the sender kept alive by this closure.
    unsafe {
        command.pre_exec(move || {
            if libc::fcntl(sender.as_raw_fd(), libc::F_SETFD, 0) < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Ok(reader)
}

/// `Ok(None)` means ready; `Ok(Some(status))` means it exited first.
pub(super) fn wait_until_ready(
    child: &mut Child,
    notification: &mut UnixStream,
) -> Result<Option<ExitStatus>> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let mut message = [0];
        let received = notification.read(&mut message);
        if matches!(received, Ok(1)) && message[0] == b'R' {
            return Ok(None);
        }
        if let Some(status) = child.try_wait().context("checking idle locker status")? {
            return Ok(Some(status));
        }
        match received {
            Err(error) if matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted) => {}
            Err(error) => return Err(error).context("reading lock readiness notification"),
            Ok(0) => anyhow::bail!("locker closed its readiness socket without confirmation"),
            Ok(_) => anyhow::bail!("invalid lock readiness notification"),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::{BufRead, BufReader, Write}, os::fd::OwnedFd};

    #[test]
    fn readiness_requires_confirmation_from_the_private_socket() {
        for message in [b"R".as_slice(), b"X".as_slice(), b"".as_slice()] {
            let (mut reader, mut sender) = UnixStream::pair().unwrap();
            reader.set_nonblocking(true).unwrap();
            sender.write_all(message).unwrap();
            drop(sender);
            let mut child = Command::new("sleep").arg("30").spawn().unwrap();
            let result = wait_until_ready(&mut child, &mut reader);
            child.kill().unwrap();
            child.wait().unwrap();
            if message == b"R" { assert!(matches!(result, Ok(None))); }
            else { assert!(result.is_err()); }
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
