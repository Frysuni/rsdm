//! Register sockets only while waiting; libICE retains its blocking I/O mode.

use std::{io, os::fd::{AsRawFd, BorrowedFd, OwnedFd, RawFd}, task::Poll};

use tokio::io::{Interest, unix::AsyncFd};

struct Descriptor {
    fd: OwnedFd,
    flags: libc::c_int,
}

impl Descriptor {
    fn duplicate(fd: RawFd) -> io::Result<Self> {
        // SAFETY: the server owns the live ICE socket throughout registration.
        let fd = unsafe { BorrowedFd::borrow_raw(fd) }.try_clone_to_owned()?;
        let flags = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFL) };
        if flags < 0 { return Err(io::Error::last_os_error()); }
        let descriptor = Self { fd, flags };
        if unsafe { libc::fcntl(descriptor.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(descriptor)
    }
}

impl AsRawFd for Descriptor {
    fn as_raw_fd(&self) -> RawFd { self.fd.as_raw_fd() }
}

impl Drop for Descriptor {
    fn drop(&mut self) {
        // AsyncFd deregisters before dropping its inner descriptor. Restore
        // the shared flags before the server can enter another FFI call.
        unsafe { libc::fcntl(self.fd.as_raw_fd(), libc::F_SETFL, self.flags); }
    }
}

pub(super) async fn wait(descriptors: &[RawFd]) -> io::Result<usize> {
    let registrations = descriptors.iter().map(|fd| {
        AsyncFd::with_interest(Descriptor::duplicate(*fd)?, Interest::READABLE)
    }).collect::<io::Result<Vec<_>>>()?;
    std::future::poll_fn(|context| {
        for (index, descriptor) in registrations.iter().enumerate() {
            let mut guard = match descriptor.poll_read_ready(context) {
                Poll::Ready(Ok(guard)) => guard,
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Pending => continue,
            };
            // Readiness may be spurious. A zero-time probe prevents passing it
            // to libICE, which would otherwise block on an idle connection.
            let mut probe = libc::pollfd { fd: descriptor.get_ref().as_raw_fd(), events: libc::POLLIN, revents: 0 };
            let ready = unsafe { libc::poll(&mut probe, 1, 0) };
            if ready < 0 { return Poll::Ready(Err(io::Error::last_os_error())); }
            if ready > 0 { return Poll::Ready(Ok(index)); }
            guard.clear_ready();
        }
        Poll::Pending
    }).await
}

#[cfg(test)]
#[path = "readiness_tests.rs"]
mod tests;
