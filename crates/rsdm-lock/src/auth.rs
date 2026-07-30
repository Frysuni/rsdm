//! Password verification for the locker: rate-limited PAM `auth` only, never a
//! new session.

use rsdm_core::{
    domain::PasswordSecret,
    ports::{CredentialVerifier, LoginAttemptLimiter, VerifyRequest},
};

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
    pub fn attempt(&self, password: &PasswordSecret) -> Attempt {
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
                Attempt::Failed("Incorrect password".to_string())
            }
        }
    }
}
