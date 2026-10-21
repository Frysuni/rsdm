//! Notify the idle parent independently of the emergency-unlock state file.

use std::{io::{self, Write}, os::{fd::{FromRawFd, RawFd}, unix::net::UnixStream}};

pub(super) fn take_sender(fd: RawFd) -> io::Result<UnixStream> {
    if fd <= libc::STDERR_FILENO {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid lock readiness descriptor"));
    }
    let mut kind = 0_i32;
    let mut size = std::mem::size_of_val(&kind) as libc::socklen_t;
    // SAFETY: kind and size point to initialized writable storage.
    if unsafe { libc::getsockopt(fd, libc::SOL_SOCKET, libc::SO_TYPE, (&mut kind as *mut i32).cast(), &mut size) } < 0 {
        return Err(io::Error::last_os_error());
    }
    if kind != libc::SOCK_STREAM {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "lock readiness requires a stream socket"));
    }
    // SAFETY: the inherited descriptor must not reach authentication helpers.
    if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the private CLI option transfers this inherited socket to Lock.
    let sender = unsafe { UnixStream::from_raw_fd(fd) };
    sender.peer_addr()?;
    Ok(sender)
}

pub(super) fn confirm(sender: Option<UnixStream>) {
    if let Some(mut sender) = sender {
        if let Err(error) = sender.write_all(b"R") {
            tracing::warn!(%error, "failed to notify the idle parent of lock readiness");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Read, os::fd::{AsRawFd, IntoRawFd}};

    #[test]
    fn readiness_sender_is_close_on_exec_and_sends_confirmation() {
        let (mut reader, sender) = UnixStream::pair().unwrap();
        let sender = take_sender(sender.into_raw_fd()).unwrap();
        // SAFETY: F_GETFD only reads flags on this owned socket.
        assert_ne!(unsafe { libc::fcntl(sender.as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC, 0);
        confirm(Some(sender));
        let mut message = Vec::new();
        reader.read_to_end(&mut message).unwrap();
        assert_eq!(message, b"R");
    }

    #[test]
    fn invalid_descriptors_are_rejected_before_locking() {
        for fd in [-1, 0, 1, 2] { assert!(take_sender(fd).is_err()); }
        let file = std::fs::File::open("/dev/null").unwrap();
        assert!(take_sender(file.as_raw_fd()).is_err());
        assert!(file.metadata().is_ok());
    }
}
