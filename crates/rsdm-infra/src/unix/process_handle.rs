//! Linux process identity pinned across exit and numeric PID reuse.

use std::{io, os::fd::{AsRawFd, FromRawFd, OwnedFd}};

pub(crate) struct ProcessHandle(OwnedFd);

impl ProcessHandle {
    pub fn open(pid: u32) -> io::Result<Option<Self>> {
        // SAFETY: pidfd_open takes a process ID and flags, and returns a new FD.
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
        if fd < 0 {
            let error = io::Error::last_os_error();
            return if error.raw_os_error() == Some(libc::ESRCH) { Ok(None) } else { Err(error) };
        }
        // SAFETY: the successful syscall returned a new descriptor we own.
        Ok(Some(Self(unsafe { OwnedFd::from_raw_fd(fd as i32) })))
    }

    pub fn alive(&self) -> io::Result<bool> {
        let mut descriptor = libc::pollfd { fd: self.0.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        // SAFETY: descriptor describes the owned pidfd; the poll never blocks.
        let result = unsafe { libc::poll(&mut descriptor, 1, 0) };
        if result < 0 { return Err(io::Error::last_os_error()); }
        Ok(result == 0)
    }

    pub fn signal(&self, signal: i32) -> io::Result<()> {
        // SAFETY: the owned pidfd identifies the process; no siginfo is supplied.
        let result = unsafe {
            libc::syscall(libc::SYS_pidfd_send_signal, self.0.as_raw_fd(), signal, std::ptr::null::<libc::siginfo_t>(), 0)
        };
        if result < 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) { return Err(error); }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_exited_handle_cannot_signal_a_later_child() {
        let mut child = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        let handle = ProcessHandle::open(child.id()).unwrap().unwrap();
        assert!(handle.alive().unwrap());
        handle.signal(libc::SIGTERM).unwrap();
        child.wait().unwrap();
        assert!(!handle.alive().unwrap());
        let mut replacement = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        handle.signal(libc::SIGKILL).unwrap();
        let untouched = replacement.try_wait().unwrap().is_none();
        replacement.kill().unwrap();
        replacement.wait().unwrap();
        assert!(untouched);
    }
}
