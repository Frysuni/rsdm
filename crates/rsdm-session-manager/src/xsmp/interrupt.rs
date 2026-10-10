//! Shutdown wakes both asynchronous waits and blocking libICE socket I/O.

use std::{os::fd::{AsRawFd, BorrowedFd, OwnedFd, RawFd}, sync::{Arc, Mutex, Weak}};

use tokio::sync::watch;

use crate::SessionError;

pub(super) struct Interrupt {
    stopped: watch::Sender<bool>,
    sockets: Mutex<Vec<Weak<OwnedFd>>>,
}

impl Default for Interrupt {
    fn default() -> Self {
        Self { stopped: watch::channel(false).0, sockets: Mutex::new(Vec::new()) }
    }
}

impl Interrupt {
    pub fn register(&self, fd: RawFd) -> Result<Arc<OwnedFd>, SessionError> {
        // SAFETY: the caller owns the live ICE socket during this duplication.
        let descriptor = Arc::new(unsafe { BorrowedFd::borrow_raw(fd) }.try_clone_to_owned()?);
        let mut sockets = self.sockets.lock().unwrap_or_else(|error| error.into_inner());
        sockets.retain(|socket| socket.strong_count() > 0);
        if *self.stopped.borrow() {
            shutdown(&descriptor);
            return Err(SessionError::State("XSMP server is stopping".into()));
        }
        sockets.push(Arc::downgrade(&descriptor));
        Ok(descriptor)
    }

    pub fn stop(&self) {
        self.stopped.send_replace(true);
        let sockets = self.sockets.lock().unwrap_or_else(|error| error.into_inner());
        for socket in sockets.iter().filter_map(Weak::upgrade) { shutdown(&socket); }
    }

    pub async fn stopped(&self) {
        let mut receiver = self.stopped.subscribe();
        loop {
            if *receiver.borrow_and_update() { return; }
            if receiver.changed().await.is_err() { return; }
        }
    }
}

fn shutdown(socket: &OwnedFd) {
    // SAFETY: the duplicate keeps this socket alive even if libICE has closed
    // its descriptor. No FFI object is accessed from the requesting thread.
    unsafe { libc::shutdown(socket.as_raw_fd(), libc::SHUT_RDWR); }
}

#[cfg(test)]
#[path = "interrupt_tests.rs"]
mod tests;
