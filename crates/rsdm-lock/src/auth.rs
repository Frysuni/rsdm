//! Password verification for the locker: rate-limited PAM `auth` only, never a
//! new session.

use std::{
    io,
    sync::{Arc, atomic::{AtomicBool, Ordering}, mpsc},
    time::{Duration, Instant},
};

use rsdm_core::{domain::PasswordSecret, ports::{AuthError, AuthMessage, LoginAttemptLimiter}};
use rsdm_infra::security::MemoryLoginAttemptLimiter;

use crate::{auth_exchange::{Exchange, send_event}, auth_process::HelperProcess};

const AUTH_TIMEOUT: Duration = Duration::from_secs(90);

#[derive(Debug, PartialEq, Eq)]
pub enum Attempt {
    Unlocked,
    /// Wrong password; carries a user-facing, leak-free message.
    Failed(String),
    /// Too many recent failures.
    RateLimited(String),
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

impl Drop for AuthenticationJob {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

pub fn start_authentication(
    password: PasswordSecret,
    username: String,
    pam_service: String,
    limiter: Arc<MemoryLoginAttemptLimiter>,
) -> io::Result<AuthenticationJob> {
    let (events_tx, events) = mpsc::sync_channel(1);
    let (responses, responses_rx) = mpsc::sync_channel(1);
    let cancel = Arc::new(AtomicBool::new(false));
    let job = AuthenticationJob { events, responses, waiting: false, cancelled: false, cancel: Arc::clone(&cancel) };
    if limiter.check_allowed(&username).is_err() {
        let _ = events_tx.try_send(AuthEvent::Finished(Attempt::RateLimited(
            "Too many attempts - wait a moment".to_string(),
        )));
        return Ok(job);
    }

    let deadline = Instant::now() + AUTH_TIMEOUT;
    let (helper, stream) = HelperProcess::spawn(deadline)?;
    std::thread::Builder::new().name("rsdm-lock-auth".to_string()).spawn(move || {
        let mut exchange = Exchange { stream, responses: responses_rx, events: events_tx, cancel, deadline };
        let outcome = run_helper(helper, &mut exchange, &username, &pam_service, password, &limiter);
        let _ = send_event(&exchange.events, AuthEvent::Finished(outcome), &exchange.cancel, deadline);
    })?;
    Ok(job)
}

fn run_helper(
    helper: HelperProcess,
    exchange: &mut Exchange,
    username: &str,
    pam_service: &str,
    password: PasswordSecret,
    limiter: &MemoryLoginAttemptLimiter,
) -> Attempt {
    let result = exchange.run(username, pam_service, password);
    drop(helper);
    match result {
        Ok(true) => {
            limiter.record_success(username);
            Attempt::Unlocked
        }
        Err(error) if error.kind() == io::ErrorKind::Interrupted => {
            Attempt::Failed("Authentication cancelled".to_string())
        }
        result => {
            limiter.record_failure(username);
            let timed_out = matches!(result, Err(error) if error.kind() == io::ErrorKind::TimedOut);
            let message = if timed_out { "Authentication timed out" } else { "Authentication failed" };
            Attempt::Failed(message.to_string())
        }
    }
}

#[cfg(test)]
#[path = "auth_tests.rs"]
mod tests;
