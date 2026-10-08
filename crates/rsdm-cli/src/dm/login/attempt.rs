//! Keep failure budgets in the Greeter, across forked PAM attempts and aliases.

use rsdm_core::{
    domain::PasswordSecret,
    ports::{AuthConversation, AuthError, AuthMessage, LoginAttemptLimitError, LoginAttemptLimiter},
};

pub(super) struct AttemptConversation<'a> {
    conversation: &'a mut dyn AuthConversation,
    limiter: &'a (dyn LoginAttemptLimiter + Sync),
    identities: Vec<String>,
}

impl<'a> AttemptConversation<'a> {
    pub(super) fn new(
        username: &str, limiter: &'a (dyn LoginAttemptLimiter + Sync),
        conversation: &'a mut dyn AuthConversation,
    ) -> Self {
        Self { conversation, limiter, identities: vec![username.to_string()] }
    }

    pub(super) fn check_allowed(&self) -> Result<(), LoginAttemptLimitError> {
        for username in &self.identities { self.limiter.check_allowed(username)?; }
        Ok(())
    }

    pub(super) fn record_failure(&self) {
        for username in &self.identities { self.limiter.record_failure(username); }
    }

    pub(super) fn record_success(&self) {
        for username in &self.identities { self.limiter.record_success(username); }
    }
}

impl std::fmt::Debug for AttemptConversation<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("AttemptConversation").finish_non_exhaustive()
    }
}

impl AuthConversation for AttemptConversation<'_> {
    fn respond(&mut self, message: AuthMessage) -> Result<Option<PasswordSecret>, AuthError> {
        self.conversation.respond(message)
    }

    fn account_name(&mut self, username: &str) -> Result<(), AuthError> {
        if !self.identities.iter().any(|identity| identity == username) {
            // Only submitted, directory, authenticate, and account-management
            // names are expected. Bound state even for a malformed helper.
            if self.identities.len() >= 4 { return Err(AuthError::AccountDenied); }
            self.identities.push(username.to_string());
        }
        self.check_allowed().map_err(|_| AuthError::AccountDenied)?;
        self.conversation.account_name(username)
    }
}

#[cfg(test)]
#[path = "attempt_tests.rs"]
mod tests;
