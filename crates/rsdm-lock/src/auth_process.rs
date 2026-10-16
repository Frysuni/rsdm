//! Own the PAM helper until it has exited, including failed worker startup.

use std::{
    io,
    os::{fd::{AsRawFd, OwnedFd}, unix::{net::UnixStream, process::CommandExt}},
    process::{Child, Command},
    thread,
    time::{Duration, Instant},
};

pub(super) struct HelperProcess {
    pub child: Option<Child>,
    pub deadline: Instant,
}

impl HelperProcess {
    pub fn spawn(deadline: Instant) -> io::Result<(Self, UnixStream)> {
        let (stream, child_stream) = UnixStream::pair()?;
        let child_fd = OwnedFd::from(child_stream);
        let fd = child_fd.as_raw_fd();
        let mut command = Command::new(std::env::current_exe()?);
        command.args(["lock-auth-helper", "--fd", &fd.to_string()]).env_clear();
        // SAFETY: only async-signal-safe calls run before exec; the socket is
        // owned here and kept alive until spawn has completed.
        unsafe {
            command.pre_exec(move || {
                let flags = libc::fcntl(fd, libc::F_GETFD);
                if flags < 0 || libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                    return Err(io::Error::last_os_error());
                }
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = command.spawn()?;
        Ok((Self { child: Some(child), deadline }, stream))
    }
}

impl Drop for HelperProcess {
    fn drop(&mut self) {
        let Some(mut child) = self.child.take() else { return };
        if child.try_wait().ok().flatten().is_some() {
            return;
        }
        // The protocol result is sent after PAM verification and cleanup.
        // Neither a broken exchange nor helper exit handlers may delay Lock.
        if let Err(error) = child.kill() {
            tracing::warn!(%error, "failed to terminate authentication helper");
        }
        let deadline = self.deadline.min(Instant::now() + Duration::from_millis(100));
        loop {
            match child.try_wait() {
                Ok(Some(_)) => return,
                Err(error) => {
                    tracing::warn!(%error, "failed to reap authentication helper");
                    break;
                }
                Ok(None) if Instant::now() >= deadline => break,
                Ok(None) => thread::sleep(Duration::from_millis(5)),
            }
        }
        // SIGKILL can remain pending in uninterruptible kernel sleep. Reaping
        // that process must not block the UI's authentication completion.
        if let Err(error) = thread::Builder::new().name("rsdm-lock-reap".into()).spawn(move || {
            let _ = child.wait();
        }) {
            tracing::warn!(%error, "failed to start authentication helper reaper");
        }
    }
}
