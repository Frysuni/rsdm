//! Password verification for the locker: rate-limited PAM `auth` only, never a
//! new session.

use std::sync::{Arc, mpsc};

use rsdm_core::{
    domain::PasswordSecret,
    ports::{
        AuthConversation, AuthError, AuthMessage, AuthMessageStyle, CredentialVerifier,
        LoginAttemptLimiter, VerifyRequest,
    },
};
use rsdm_infra::{pam::PamCredentialVerifier, security::MemoryLoginAttemptLimiter};

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
}

#[derive(Debug)]
struct LockConversation {
    events: mpsc::SyncSender<AuthEvent>,
    responses: mpsc::Receiver<Result<PasswordSecret, AuthError>>,
}

impl AuthConversation for LockConversation {
    fn respond(&mut self, message: AuthMessage) -> Result<Option<PasswordSecret>, AuthError> {
        let needs_answer = matches!(message.style, AuthMessageStyle::Secret | AuthMessageStyle::Visible);
        self.events.send(AuthEvent::Message(message))
            .map_err(|_| AuthError::InvalidCredentials)?;
        if !needs_answer {
            return Ok(None);
        }
        self.responses.recv().map_err(|_| AuthError::InvalidCredentials)?.map(Some)
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
    let conversation = LockConversation {
        events: events_tx.clone(),
        responses: responses_rx,
    };
    std::thread::Builder::new()
        .name("rsdm-lock-auth".to_string())
        .spawn(move || {
            let verifier = PamCredentialVerifier;
            let authenticator = Authenticator {
                verifier: &verifier,
                limiter: limiter.as_ref(),
                username: &username,
                pam_service: &pam_service,
            };
            let outcome = authenticator.attempt(&password, Box::new(conversation));
            drop(password);
            let _ = events_tx.send(AuthEvent::Finished(outcome));
        })?;
    Ok(AuthenticationJob {
        events,
        responses,
        waiting: false,
        cancelled: false,
    })
}
