//! Linux process identity pinned across exit and numeric PID reuse.

use std::{io, os::fd::{AsRawFd, FromRawFd, OwnedFd}};

pub struct ProcessHandle(OwnedFd);

const PID_FS_MAGIC: libc::c_long = 0x50494446;

impl ProcessHandle {
    /// Pin a Unix socket's authenticated peer without reopening a numeric PID.
    pub fn socket_peer(socket: &impl AsRawFd) -> io::Result<Self> {
        let mut descriptor = -1;
        let mut length = std::mem::size_of_val(&descriptor) as libc::socklen_t;
        // SAFETY: both output pointers are valid; success returns an owned FD.
        let result = unsafe {
            libc::getsockopt(socket.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERPIDFD,
                (&mut descriptor as *mut i32).cast(), &mut length)
        };
        if result < 0 { return Err(io::Error::last_os_error()); }
        // SAFETY: SO_PEERPIDFD returned a new descriptor owned by this caller.
        Ok(Self(unsafe { OwnedFd::from_raw_fd(descriptor) }))
    }

    /// Adopt a pidfd received from a trusted process-credential provider.
    pub fn from_pidfd(descriptor: OwnedFd) -> Self { Self(descriptor) }

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

    /// Stable process identity on kernels with pidfs; anonymous pidfds have none.
    pub fn id(&self) -> io::Result<Option<u64>> {
        let mut filesystem = std::mem::MaybeUninit::<libc::statfs>::uninit();
        // SAFETY: filesystem points to writable storage for the owned FD.
        if unsafe { libc::fstatfs(self.0.as_raw_fd(), filesystem.as_mut_ptr()) } < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: fstatfs initialized the output on success.
        if unsafe { filesystem.assume_init() }.f_type != PID_FS_MAGIC { return Ok(None); }
        let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: metadata points to writable storage for the owned FD.
        if unsafe { libc::fstat(self.0.as_raw_fd(), metadata.as_mut_ptr()) } < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: fstat initialized the output on success. Pidfs assigns one
        // inode per process, shared by separate pidfd_open calls and exec.
        Ok(Some(unsafe { metadata.assume_init() }.st_ino))
    }

    pub fn alive(&self) -> io::Result<bool> {
        let mut descriptor = libc::pollfd { fd: self.0.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        // SAFETY: descriptor describes the owned pidfd; the poll never blocks.
        let result = unsafe { libc::poll(&mut descriptor, 1, 0) };
        if result < 0 { return Err(io::Error::last_os_error()); }
        Ok(result == 0)
    }

    /// Return whether the kernel accepted the signal for the pinned process.
    pub fn signal(&self, signal: i32) -> io::Result<bool> {
        // SAFETY: the owned pidfd identifies the process; no siginfo is supplied.
        let result = unsafe {
            libc::syscall(libc::SYS_pidfd_send_signal, self.0.as_raw_fd(), signal, std::ptr::null::<libc::siginfo_t>(), 0)
        };
        if result < 0 {
            let error = io::Error::last_os_error();
            return if error.raw_os_error() == Some(libc::ESRCH) { Ok(false) } else { Err(error) };
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_credentials_provide_a_live_peer_pidfd() {
        let (socket, _peer) = std::os::unix::net::UnixStream::pair().unwrap();
        let handle = ProcessHandle::socket_peer(&socket).unwrap();
        assert!(handle.alive().unwrap());
        assert!(handle.signal(0).unwrap());
    }

    #[test]
    fn pidfs_ids_match_separate_handles_to_one_process_and_distinguish_children() {
        let first = ProcessHandle::open(std::process::id()).unwrap().unwrap();
        let second = ProcessHandle::open(std::process::id()).unwrap().unwrap();
        assert_eq!(first.id().unwrap(), second.id().unwrap());
        let mut child = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        let handle = ProcessHandle::open(child.id()).unwrap().unwrap();
        if let Some(identity) = first.id().unwrap() {
            assert_ne!(identity, 0);
            assert_ne!(Some(identity), handle.id().unwrap());
        } else {
            assert_eq!(handle.id().unwrap(), None);
        }
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn an_exited_handle_cannot_signal_a_later_child() {
        let mut child = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        let handle = ProcessHandle::open(child.id()).unwrap().unwrap();
        assert!(handle.alive().unwrap());
        assert!(handle.signal(libc::SIGTERM).unwrap());
        child.wait().unwrap();
        assert!(!handle.alive().unwrap());
        let mut replacement = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        assert!(!handle.signal(libc::SIGKILL).unwrap());
        let untouched = replacement.try_wait().unwrap().is_none();
        replacement.kill().unwrap();
        replacement.wait().unwrap();
        assert!(untouched);
    }
}
