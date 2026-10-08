//! Fork children perform only the gate's syscalls and an isolated report write.

use std::{io::Read, os::{fd::{AsRawFd, IntoRawFd}, unix::net::UnixStream}, time::Duration};

use super::*;

struct Child {
    pid: libc::pid_t,
    gate: Option<StartGate>,
    report: UnixStream,
}

impl Child {
    fn new() -> Self {
        let gate = StartGate::new().unwrap();
        let (report, writer) = UnixStream::pair().unwrap();
        report.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        // SAFETY: the child uses only async-signal-safe calls before _exit.
        let pid = unsafe { libc::fork() };
        assert!(pid >= 0);
        if pid == 0 {
            let writer = writer.into_raw_fd();
            // SAFETY: close this child's unused report endpoint.
            unsafe { libc::close(report.as_raw_fd()); }
            let accepted = gate.wait_child();
            let value = u8::from(accepted);
            // SAFETY: write one initialized byte and leave without unwinding.
            unsafe {
                libc::write(writer, (&value as *const u8).cast(), 1);
                libc::_exit(if accepted { 0 } else { 126 });
            }
        }
        drop(writer);
        Self { pid, gate: Some(gate), report }
    }

    fn accepted(&mut self) -> bool {
        let mut result = [0];
        self.report.read_exact(&mut result).unwrap();
        result[0] == 1
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        self.gate.take();
        // SAFETY: this unreaped child pins its PID, even if it already exited.
        unsafe {
            libc::kill(self.pid, libc::SIGKILL);
            let mut status = 0;
            while libc::waitpid(self.pid, &mut status, 0) < 0
                && *libc::__errno_location() == libc::EINTR {}
        }
    }
}

#[test]
fn the_child_cannot_start_before_its_parent_releases_the_gate() {
    let mut child = Child::new();
    let mut poll = libc::pollfd { fd: child.report.as_raw_fd(), events: libc::POLLIN, revents: 0 };
    // SAFETY: poll points to one initialized private descriptor entry.
    assert_eq!(unsafe { libc::poll(&mut poll, 1, 50) }, 0);
    child.gate.take().unwrap().release().unwrap();
    assert!(child.accepted());
}

#[test]
fn loss_of_the_parent_gate_aborts_the_child_instead_of_leaving_it_waiting() {
    let mut child = Child::new();
    drop(child.gate.take());
    assert!(!child.accepted());
}

#[test]
fn an_invalid_start_token_cannot_launch_the_child() {
    let mut child = Child::new();
    let gate = child.gate.take().unwrap();
    drop(gate.reader);
    File::from(gate.writer).write_all(&[0]).unwrap();
    assert!(!child.accepted());
}

#[test]
fn gate_endpoints_are_close_on_exec() {
    let gate = StartGate::new().unwrap();
    for fd in [gate.reader.as_raw_fd(), gate.writer.as_raw_fd()] {
        // SAFETY: F_GETFD only reads flags on these owned descriptors.
        assert_ne!(unsafe { libc::fcntl(fd, libc::F_GETFD) } & libc::FD_CLOEXEC, 0);
    }
}
