use std::{cell::Cell, time::Duration};

use super::*;

struct Verifier {
    calls: Cell<usize>,
    accepts: bool,
}

impl CredentialVerifier for Verifier {
    fn verify(&self, request: VerifyRequest<'_>) -> Result<(), AuthError> {
        self.calls.set(self.calls.get() + 1);
        assert!(request.password.expose_secret().is_empty());
        assert!(request.conversation.is_some());
        if self.accepts { Ok(()) } else { Err(AuthError::InvalidCredentials) }
    }
}

#[derive(Debug)]
struct Conversation;

impl AuthConversation for Conversation {
    fn respond(&mut self, _: AuthMessage) -> Result<Option<PasswordSecret>, AuthError> {
        unreachable!("the test verifier does not request additional credentials")
    }
}

#[test]
fn empty_initial_password_is_verified_by_the_backend() {
    let verifier = Verifier { calls: Cell::new(0), accepts: true };
    let limiter = MemoryLoginAttemptLimiter::new(1, Duration::from_secs(60));
    let authenticator = Authenticator {
        verifier: &verifier, limiter: &limiter, username: "alice", pam_service: "test",
    };
    assert_eq!(authenticator.attempt(&PasswordSecret::new(""), Box::new(Conversation)), Attempt::Unlocked);
    assert_eq!(verifier.calls.get(), 1);
}

#[test]
fn rejected_empty_passwords_are_rate_limited() {
    let verifier = Verifier { calls: Cell::new(0), accepts: false };
    let limiter = MemoryLoginAttemptLimiter::new(1, Duration::from_secs(60));
    let authenticator = Authenticator {
        verifier: &verifier, limiter: &limiter, username: "alice", pam_service: "test",
    };
    let password = PasswordSecret::new("");
    assert!(matches!(authenticator.attempt(&password, Box::new(Conversation)), Attempt::Failed(_)));
    assert!(matches!(authenticator.attempt(&password, Box::new(Conversation)), Attempt::RateLimited(_)));
    assert_eq!(verifier.calls.get(), 1);
}
