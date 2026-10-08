//! Keep the forked session child idle until its PAM owner establishes the guard.

use std::{fs::File, io::{self, Write}, os::fd::{FromRawFd, IntoRawFd, OwnedFd}};

const START: u8 = 1;

pub(super) struct StartGate {
    reader: OwnedFd,
    writer: OwnedFd,
}

impl StartGate {
    pub fn new() -> io::Result<Self> {
        let mut fds = [-1; 2];
        // SAFETY: pipe2 fills two initialized entries. CLOEXEC also protects
        // the parent endpoint if a PAM helper executes during guard setup.
        if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: these are new descriptors owned by this gate.
        Ok(unsafe { Self { reader: OwnedFd::from_raw_fd(fds[0]), writer: OwnedFd::from_raw_fd(fds[1]) } })
    }

    /// Called only in the fork child, before credential/environment setup.
    /// This path uses only close/read and errno; it cannot allocate or lock.
    pub fn wait_child(self) -> bool {
        let reader = self.reader.into_raw_fd();
        let writer = self.writer.into_raw_fd();
        let mut start = 0_u8;
        // SAFETY: both descriptors belong to this child. Closing its writer
        // ensures parent failure produces EOF instead of an indefinite wait.
        unsafe {
            libc::close(writer);
            let received = loop {
                let result = libc::read(reader, (&mut start as *mut u8).cast(), 1);
                if result < 0 && *libc::__errno_location() == libc::EINTR { continue; }
                break result == 1 && start == START;
            };
            libc::close(reader);
            received
        }
    }

    pub fn release(self) -> io::Result<()> {
        drop(self.reader);
        File::from(self.writer).write_all(&[START])
    }
}

#[cfg(test)]
#[path = "session_start_gate_tests.rs"]
mod tests;
