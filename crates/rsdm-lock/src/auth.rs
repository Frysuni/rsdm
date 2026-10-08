//! Password verification for the locker: rate-limited PAM `auth` only, never a
//! new session.

use std::{
    io::{self, Read, Write},
    os::fd::{AsRawFd, OwnedFd},
    os::unix::net::UnixStream,
    process::{Child, Command},
    sync::{Arc, atomic::{AtomicBool, Ordering}, mpsc},
    time::{Duration, Instant},
};

use rsdm_core::{
    domain::PasswordSecret,
    ports::{
        AuthConversation, AuthError, AuthMessage, CredentialVerifier,
        LoginAttemptLimiter, VerifyRequest,
    },
};
use rsdm_infra::security::MemoryLoginAttemptLimiter;

use crate::auth_protocol::{self, Frame};

const AUTH_TIMEOUT: Duration = Duration::from_secs(90);

#[cfg_attr(not(test), allow(dead_code))]
pub struct Authenticator<'a> {
    pub verifier: &'a dyn CredentialVerifier,
    pub limiter: &'a dyn LoginAttemptLimiter,
    pub username: &'a str,
    pub pam_service: &'a str,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Attempt {
    Unlocked,
    /// Wrong password; carries a user-facing, leak-free message.
    Failed(String),
    /// Too many recent failures.
    RateLimited(String),
}

#[cfg_attr(not(test), allow(dead_code))]
impl Authenticator<'_> {
    pub fn attempt(
        &self,
        password: &PasswordSecret,
        conversation: Box<dyn AuthConversation>,
    ) -> Attempt {
        if self.limiter.check_allowed(self.username).is_err() {
            tracing::warn!(
                username = self.username,
                "lock screen authentication rate limited"
            );
            return Attempt::RateLimited("Too many attempts - wait a moment".to_string());
        }
        tracing::info!(
            username = self.username,
            pam_service = self.pam_service,
            "lock screen authentication submitted"
        );
        match self.verifier.verify(VerifyRequest {
            username: self.username,
            password,
            pam_service: self.pam_service,
            conversation: Some(conversation),
        }) {
            Ok(()) => {
                self.limiter.record_success(self.username);
                tracing::info!(
                    username = self.username,
                    "lock screen authentication succeeded"
                );
                Attempt::Unlocked
            }
            Err(_) => {
                self.limiter.record_failure(self.username);
                tracing::warn!(
                    username = self.username,
                    "lock screen authentication failed"
                );
                Attempt::Failed("Authentication failed".to_string())
            }
        }
    }
}

pub enum AuthEvent {
    Message(AuthMessage),
    Finished(Attempt),
}

pub struct AuthenticationJob {
    pub events: mpsc::Receiver<AuthEvent>,
    pub responses: mpsc::SyncSender<Result<PasswordSecret, AuthError>>,
    pub waiting: bool,
    pub cancelled: bool,
    cancel: Arc<AtomicBool>,
}

impl AuthenticationJob {
    pub fn cancel(&mut self) {
        self.cancelled = true;
        self.cancel.store(true, Ordering::Release);
        let _ = self.responses.try_send(Err(AuthError::InvalidCredentials));
    }
}

pub fn start_authentication(
    password: PasswordSecret,
    username: String,
    pam_service: String,
    limiter: Arc<MemoryLoginAttemptLimiter>,
) -> std::io::Result<AuthenticationJob> {
    let (events_tx, events) = mpsc::sync_channel(1);
    let (responses, responses_rx) = mpsc::sync_channel(1);
    if limiter.check_allowed(&username).is_err() {
        let _ = events_tx.send(AuthEvent::Finished(Attempt::RateLimited(
            "Too many attempts - wait a moment".to_string(),
        )));
        return Ok(AuthenticationJob { events, responses, waiting: false, cancelled: false, cancel: Arc::new(AtomicBool::new(false)) });
    }
    let (parent_stream, child_stream) = UnixStream::pair()?;
    let child_fd = OwnedFd::from(child_stream);
    let child_fd_number = child_fd.as_raw_fd();
    let executable = std::env::current_exe()?;
    let mut command = Command::new(executable);
    command
        .arg("lock-auth-helper")
        .arg("--fd")
        .arg(child_fd_number.to_string())
        .env_clear();
    let child_fd_for_exec = child_fd_number;
    // SAFETY: the closure only clears close-on-exec on the one socket owned by
    // the helper. No Rust code runs in the child before exec beyond fcntl.
    unsafe {
        use std::os::unix::process::CommandExt;
        command.pre_exec(move || {
            let flags = libc::fcntl(child_fd_for_exec, libc::F_GETFD);
            if flags < 0 {
                return Err(io::Error::last_os_error());
            }
            if libc::fcntl(child_fd_for_exec, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                return Err(io::Error::last_os_error());
            }
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = command.spawn()?;
    drop(child_fd);
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = Arc::clone(&cancel);
    let worker_limiter = Arc::clone(&limiter);
    std::thread::Builder::new()
        .name("rsdm-lock-auth".to_string())
        .spawn(move || {
            let outcome = run_helper(
                child,
                parent_stream,
                username,
                pam_service,
                password,
                responses_rx,
                worker_cancel,
                worker_limiter,
                events_tx.clone(),
            );
            let _ = events_tx.send(AuthEvent::Finished(outcome));
        })?;
    Ok(AuthenticationJob {
        events,
        responses,
        waiting: false,
        cancelled: false,
        cancel,
    })
}

fn run_helper(
    mut child: Child,
    mut stream: UnixStream,
    username: String,
    pam_service: String,
    password: PasswordSecret,
    responses: mpsc::Receiver<Result<PasswordSecret, AuthError>>,
    cancel: Arc<AtomicBool>,
    limiter: Arc<MemoryLoginAttemptLimiter>,
    events: mpsc::SyncSender<AuthEvent>,
) -> Attempt {
    let mut start = match auth_protocol::encode_start(&username, &pam_service, &password) {
        Ok(start) => start,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Attempt::Failed(error.to_string());
        }
    };
    let write_result = stream.write_all(&start).and_then(|()| stream.flush());
    start.fill(0);
    if write_result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
        limiter.record_failure(&username);
        return Attempt::Failed("Authentication failed".to_string());
    }
    if stream.set_nonblocking(true).is_err() {
        let _ = child.kill();
        let _ = child.wait();
        limiter.record_failure(&username);
        return Attempt::Failed("Authentication failed".to_string());
    }
    let deadline = Instant::now() + AUTH_TIMEOUT;
    let mut buffer = Vec::new();
    loop {
        if cancel.load(Ordering::Acquire) {
            let _ = child.kill();
            let _ = child.wait();
            return Attempt::Failed("Authentication cancelled".to_string());
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            limiter.record_failure(&username);
            return Attempt::Failed("Authentication timed out".to_string());
        }
        while let Ok(response) = responses.try_recv() {
            match response {
                Ok(secret) => {
                    if auth_protocol::write_frame(&mut stream, &Frame::Answer(Some(secret.expose_secret().to_string()))).is_err() {
                        let _ = child.kill();
                        let _ = child.wait();
                        limiter.record_failure(&username);
                        return Attempt::Failed("Authentication failed".to_string());
                    }
                }
                Err(_) => {
                    cancel.store(true, Ordering::Release);
                    let _ = child.kill();
                    let _ = child.wait();
                    return Attempt::Failed("Authentication cancelled".to_string());
                }
            }
        }
        let mut bytes = [0; 4096];
        match stream.read(&mut bytes) {
            Ok(0) => break,
            Ok(read) => buffer.extend_from_slice(&bytes[..read]),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
        match auth_protocol::take_frames(&mut buffer) {
            Ok(frames) => {
                for frame in frames {
                    match frame {
                        Frame::Message(message) => {
                            if events.send(AuthEvent::Message(message)).is_err() {
                                let _ = child.kill();
                                let _ = child.wait();
                                return Attempt::Failed("Authentication cancelled".to_string());
                            }
                        }
                        Frame::Finished { success, .. } => {
                            let outcome = if success {
                                limiter.record_success(&username);
                                Attempt::Unlocked
                            } else {
                                limiter.record_failure(&username);
                                Attempt::Failed("Authentication failed".to_string())
                            };
                            let _ = child.wait();
                            return outcome;
                        }
                        _ => {
                            let _ = child.kill();
                            let _ = child.wait();
                            limiter.record_failure(&username);
                            return Attempt::Failed("Authentication failed".to_string());
                        }
                    }
                }
            }
            Err(_) => break,
        }
        if child.try_wait().ok().flatten().is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = child.wait();
    limiter.record_failure(&username);
    Attempt::Failed("Authentication failed".to_string())
}

#[cfg(test)]
#[path = "auth_tests.rs"]
mod tests;
