//! Forked owner of the PAM and compositor session.
//!
//! It lives outside the greeter service cgroup and uses a two-phase handshake:
//! authenticate while the UI owns the VT, then launch after the UI releases it.

use std::{
    io,
    os::{fd::BorrowedFd, raw::c_int},
};

use rsdm_core::ports::{AuthConversation, SessionGate, SessionLaunchError};

pub use super::session_report::LeaderReport;
use super::session_report::AUTHORIZED_TAG;
use super::{
    session_conversation::{LeaderConversation, read_auth_report},
    session_pipe::{read_frame, write_frame},
};

pub enum LeaderLaunch {
    Denied(LeaderReport),
    Ready(LeaderHandle),
}

/// The child's side of the two-phase handshake, handed to the session closure.
pub struct LeaderGate {
    report_fd: c_int,
    go_fd: c_int,
}

impl LeaderGate {
    pub fn conversation(&self) -> io::Result<impl AuthConversation + 'static> {
        // SAFETY: these pipe descriptors are valid for the gate's lifetime.
        let report = unsafe { BorrowedFd::borrow_raw(self.report_fd) }.try_clone_to_owned()?;
        let reply = unsafe { BorrowedFd::borrow_raw(self.go_fd) }.try_clone_to_owned()?;
        Ok(LeaderConversation { report, reply })
    }

    /// Report "authorized" to the greeter and park until it answers that the
    /// terminal is released. An error means the greeter is gone or refused;
    /// the session must not be launched.
    pub fn session_authorized(&self) -> io::Result<()> {
        write_frame(self.report_fd, &[AUTHORIZED_TAG, 0, 0, 0, 0])?;
        let mut byte = [0u8; 1];
        loop {
            // SAFETY: read one byte into a valid local buffer.
            let read = unsafe { libc::read(self.go_fd, byte.as_mut_ptr().cast(), 1) };
            if read == 1 {
                return Ok(());
            }
            if read == 0 {
                return Err(io::Error::other(
                    "the greeter exited before releasing the terminal",
                ));
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }
}

impl SessionGate for LeaderGate {
    fn session_authorized(&self) -> Result<(), SessionLaunchError> {
        LeaderGate::session_authorized(self)
            .map_err(|error| SessionLaunchError::Process(format!("session gate: {error}")))
    }
}

/// The parent's handle on a parked, authorized session-leader child.
///
/// Dropping it without [`proceed`](LeaderHandle::proceed) closes the go pipe:
/// the child sees EOF, aborts the launch, closes its PAM session and exits.
pub struct LeaderHandle {
    pid: libc::pid_t,
    report_fd: c_int,
    go_fd: c_int,
    finished: bool,
}

impl LeaderHandle {
    /// Release the parked child onto the (now free) terminal and block until the
    /// session ends, returning its final report. This is the greeter's long wait
    /// for the whole session.
    pub fn proceed(mut self) -> LeaderReport {
        self.finished = true;
        if let Err(error) = write_frame(self.go_fd, &[1]) {
            tracing::warn!(%error, "could not release the session leader; it will abort the launch");
        }
        // SAFETY: both fds are valid and owned by this handle.
        unsafe { libc::close(self.go_fd) };
        let report = read_frame(self.report_fd)
            .map(LeaderReport::decode)
            .unwrap_or(LeaderReport::Lost);
        // SAFETY: report_fd is valid and owned by this handle.
        unsafe { libc::close(self.report_fd) };
        reap(self.pid);
        report
    }
}

impl Drop for LeaderHandle {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        // SAFETY: both fds are valid and owned by this handle. Closing the go
        // pipe makes the parked child's read return EOF, so it aborts cleanly.
        unsafe {
            libc::close(self.go_fd);
            libc::close(self.report_fd);
        }
        reap(self.pid);
    }
}

/// Run the PAM/session lifecycle in a detached child and wait for its first
/// handshake frame.
pub fn spawn_session_leader(
    conversation: &mut dyn AuthConversation,
    session: impl FnOnce(&LeaderGate) -> LeaderReport,
) -> LeaderLaunch {
    let Some((report_read, report_write)) = pipe() else {
        return LeaderLaunch::Denied(LeaderReport::Lost);
    };
    let Some((go_read, go_write)) = pipe() else {
        // SAFETY: the report pipe fds are valid and owned by us.
        unsafe {
            libc::close(report_read);
            libc::close(report_write);
        }
        return LeaderLaunch::Denied(LeaderReport::Lost);
    };

    // SAFETY: fork has no Rust-side preconditions. We fork in a single-threaded
    // context (the TUI event loop is parked in the submit callback), and the
    // child runs only simple libc setup before its own logic and `_exit`.
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        tracing::error!(error = %io::Error::last_os_error(), "fork for the session leader failed");
        // SAFETY: all four fds are valid and owned by us.
        unsafe {
            libc::close(report_read);
            libc::close(report_write);
            libc::close(go_read);
            libc::close(go_write);
        }
        return LeaderLaunch::Denied(LeaderReport::Lost);
    }
    if pid == 0 {
        run_child((report_read, report_write), (go_read, go_write), session);
    }

    // SAFETY: both fds are valid and owned by us.
    unsafe {
        libc::close(report_write);
        libc::close(go_read);
    }
    await_authorization(pid, report_read, go_write, conversation)
}

fn run_child(
    report_pipe: (c_int, c_int),
    go_pipe: (c_int, c_int),
    session: impl FnOnce(&LeaderGate) -> LeaderReport,
) -> ! {
    let (report_read, report_write) = report_pipe;
    let (go_read, go_write) = go_pipe;
    // SAFETY: simple libc calls on valid arguments in the fresh child.
    unsafe {
        libc::close(report_read);
        libc::close(go_write);
        libc::setsid();
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
    let gate = LeaderGate {
        report_fd: report_write,
        go_fd: go_read,
    };
    let report = session(&gate);
    let _ = write_frame(report_write, &report.encode());
    // SAFETY: end the child now; it must never return into the greeter loop.
    unsafe { libc::_exit(0) }
}

fn await_authorization(
    pid: libc::pid_t,
    report_read: c_int,
    go_write: c_int,
    conversation: &mut dyn AuthConversation,
) -> LeaderLaunch {
    match read_auth_report(report_read, go_write, conversation) {
        Some(frame) if frame[0] == AUTHORIZED_TAG => LeaderLaunch::Ready(LeaderHandle {
            pid,
            report_fd: report_read,
            go_fd: go_write,
            finished: false,
        }),
        frame => {
            let report = frame
                .map(LeaderReport::decode)
                .unwrap_or(LeaderReport::Lost);
            // SAFETY: both fds are valid and owned by us.
            unsafe {
                libc::close(report_read);
                libc::close(go_write);
            }
            reap(pid);
            LeaderLaunch::Denied(report)
        }
    }
}

/// `pipe2(O_CLOEXEC)` as `(read, write)`. O_CLOEXEC keeps both handshake pipes
/// from leaking into the compositor the child execs further down.
fn pipe() -> Option<(c_int, c_int)> {
    let mut fds: [c_int; 2] = [0; 2];
    // SAFETY: pipe2 fills a valid two-element array.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        tracing::error!(error = %io::Error::last_os_error(), "pipe2 for the session leader failed");
        return None;
    }
    Some((fds[0], fds[1]))
}

fn reap(pid: libc::pid_t) {
    loop {
        let mut status = 0;
        // SAFETY: waitpid on our own child with a valid status pointer.
        let waited = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
        if waited == pid {
            return;
        }
        if waited < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return;
        }
        // An active session owns its PAM lifecycle outside the greeter cgroup.
        // Let it outlive a stopped DM instead of waiting for desktop logout.
        if super::shutdown::terminate_requested() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}
