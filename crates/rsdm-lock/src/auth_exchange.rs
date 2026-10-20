//! Cancellable, bounded communication between the PAM helper and Lock UI.

use std::{
    io::{self, Read, Write},
    os::unix::net::UnixStream,
    sync::{Arc, atomic::{AtomicBool, Ordering}, mpsc},
    thread,
    time::{Duration, Instant},
};

use rsdm_core::{domain::PasswordSecret, ports::AuthError};
use zeroize::Zeroizing;

use crate::{auth::AuthEvent, auth_protocol::{self, Frame}};

const POLL_INTERVAL: Duration = Duration::from_millis(20);

pub(super) struct Exchange {
    pub stream: UnixStream,
    pub responses: mpsc::Receiver<Result<PasswordSecret, AuthError>>,
    pub events: mpsc::SyncSender<AuthEvent>,
    pub cancel: Arc<AtomicBool>,
    pub deadline: Instant,
    pub credentials_sent: bool,
}

impl Exchange {
    pub fn run(&mut self, username: &str, service: &str, password: PasswordSecret) -> io::Result<bool> {
        self.stream.set_nonblocking(true)?;
        let start = auth_protocol::encode_start(username, service, password)?;
        self.write_all(&start)?;
        self.credentials_sent = true;
        drop(start);

        let mut buffer = Zeroizing::new(Vec::new());
        loop {
            check_active(&self.cancel, self.deadline)?;
            self.send_response()?;
            self.read_available(&mut buffer)?;
            for frame in auth_protocol::take_frames(&mut buffer)? {
                check_active(&self.cancel, self.deadline)?;
                match frame {
                    Frame::Message(message) => {
                        send_event(&self.events, AuthEvent::Message(message), &self.cancel, self.deadline)?;
                    }
                    Frame::Finished { success, .. } => return Ok(success),
                    _ => return Err(io::Error::new(io::ErrorKind::InvalidData, "unexpected authentication frame")),
                }
            }
            thread::sleep(POLL_INTERVAL.min(self.deadline.saturating_duration_since(Instant::now())));
        }
    }

    fn read_available(&mut self, buffer: &mut Vec<u8>) -> io::Result<()> {
        let mut bytes = [0; 4096];
        match self.stream.read(&mut bytes) {
            Ok(0) => Err(io::Error::new(io::ErrorKind::UnexpectedEof, "authentication helper disconnected")),
            Ok(read) => {
                buffer.extend_from_slice(&bytes[..read]);
                Ok(())
            }
            Err(error) if matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted) => Ok(()),
            Err(error) => Err(error),
        }
    }

    fn send_response(&mut self) -> io::Result<()> {
        let response = match self.responses.try_recv() {
            Ok(Ok(secret)) => secret,
            Ok(Err(_)) | Err(mpsc::TryRecvError::Disconnected) => {
                self.cancel.store(true, Ordering::Release);
                return Err(io::Error::new(io::ErrorKind::Interrupted, "authentication cancelled"));
            }
            Err(mpsc::TryRecvError::Empty) => return Ok(()),
        };
        let mut bytes = Zeroizing::new(Vec::new());
        auth_protocol::write_frame(&mut *bytes, &Frame::Answer(Some(response)))?;
        self.write_all(&bytes)
    }

    fn write_all(&mut self, mut bytes: &[u8]) -> io::Result<()> {
        while !bytes.is_empty() {
            check_active(&self.cancel, self.deadline)?;
            match self.stream.write(bytes) {
                Ok(0) => return Err(io::Error::new(io::ErrorKind::WriteZero, "authentication helper stopped reading")),
                Ok(written) => bytes = &bytes[written..],
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(POLL_INTERVAL.min(self.deadline.saturating_duration_since(Instant::now())));
                }
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
}

pub(super) fn send_event(
    events: &mpsc::SyncSender<AuthEvent>,
    mut event: AuthEvent,
    cancel: &AtomicBool,
    deadline: Instant,
) -> io::Result<()> {
    loop {
        // Try once even at the deadline so a terminal result can reach an
        // available receiver. A stalled UI must never hold the worker alive.
        match events.try_send(event) {
            Ok(()) => return Ok(()),
            Err(mpsc::TrySendError::Full(pending)) => event = pending,
            Err(mpsc::TrySendError::Disconnected(_)) => {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "authentication UI disconnected"));
            }
        }
        check_active(cancel, deadline)?;
        thread::sleep(POLL_INTERVAL.min(deadline.saturating_duration_since(Instant::now())));
    }
}

fn check_active(cancel: &AtomicBool, deadline: Instant) -> io::Result<()> {
    if cancel.load(Ordering::Acquire) {
        return Err(io::Error::new(io::ErrorKind::Interrupted, "authentication cancelled"));
    }
    if Instant::now() >= deadline {
        return Err(io::Error::new(io::ErrorKind::TimedOut, "authentication timed out"));
    }
    Ok(())
}

#[cfg(test)]
#[path = "auth_exchange_tests.rs"]
mod tests;
