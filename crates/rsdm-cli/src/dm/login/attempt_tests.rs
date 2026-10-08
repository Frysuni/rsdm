use std::time::Duration;

use rsdm_infra::security::MemoryLoginAttemptLimiter;
use rsdm_core::ports::AuthMessageStyle;

use super::*;

#[derive(Debug)]
struct Conversation;

impl AuthConversation for Conversation {
    fn respond(&mut self, message: AuthMessage) -> Result<Option<PasswordSecret>, AuthError> {
        assert_eq!(message.style, AuthMessageStyle::Info);
        assert_eq!(message.text, "Use your token");
        Ok(None)
    }
}

#[test]
fn failed_pam_aliases_share_the_parent_account_budget() {
    let limiter = MemoryLoginAttemptLimiter::new(2, Duration::from_secs(60));
    let mut ui = Conversation;
    for alias in ["alice@first", "alice@second"] {
        let mut attempt = AttemptConversation::new(alias, &limiter, &mut ui);
        assert!(attempt.check_allowed().is_ok());
        attempt.account_name("alice").unwrap();
        let outcome = super::super::denied_outcome(&attempt, alias.into(), super::super::LeaderReport::AuthFailed);
        assert!(matches!(outcome, super::super::LoginAttemptOutcome::Failure(_)));
    }
    let mut third = AttemptConversation::new("alice@third", &limiter, &mut ui);
    assert!(third.check_allowed().is_ok());
    assert!(matches!(third.account_name("alice"), Err(AuthError::AccountDenied)));
    assert!(third.check_allowed().is_err());
    assert!(limiter.check_allowed("alice").is_err());
}

#[test]
fn repeated_account_reports_count_one_failure_per_identity() {
    let limiter = MemoryLoginAttemptLimiter::new(2, Duration::from_secs(60));
    let mut ui = Conversation;
    let mut attempt = AttemptConversation::new("alice", &limiter, &mut ui);
    attempt.account_name("alice").unwrap();
    attempt.account_name("alice").unwrap();
    super::super::denied_outcome(&attempt, "alice".into(), super::super::LeaderReport::UserDenied);
    assert!(limiter.check_allowed("alice").is_ok());
    attempt.record_failure();
    assert!(limiter.check_allowed("alice").is_err());
}

#[test]
fn successful_authentication_resets_only_its_submitted_and_mapped_identities() {
    let limiter = MemoryLoginAttemptLimiter::new(2, Duration::from_secs(60));
    for username in ["alias", "alice", "bob"] { limiter.record_failure(username); }
    let mut ui = Conversation;
    let mut attempt = AttemptConversation::new("alias", &limiter, &mut ui);
    attempt.account_name("alice").unwrap();
    super::super::denied_outcome(&attempt, "alias".into(), super::super::LeaderReport::SessionLaunchFailed);
    for username in ["alias", "alice", "bob"] { limiter.record_failure(username); }
    assert!(limiter.check_allowed("alias").is_ok());
    assert!(limiter.check_allowed("alice").is_ok());
    assert!(limiter.check_allowed("bob").is_err());
}

#[test]
fn lost_authentication_does_not_erase_the_account_failure_budget() {
    let limiter = MemoryLoginAttemptLimiter::new(2, Duration::from_secs(60));
    limiter.record_failure("alice");
    let mut ui = Conversation;
    let mut attempt = AttemptConversation::new("alias", &limiter, &mut ui);
    attempt.account_name("alice").unwrap();
    super::super::denied_outcome(&attempt, "alias".into(), super::super::LeaderReport::Lost);
    limiter.record_failure("alice");
    assert!(limiter.check_allowed("alice").is_err());
}

#[test]
fn account_tracking_is_bounded_and_prompt_messages_still_reach_the_ui() {
    let limiter = MemoryLoginAttemptLimiter::new(2, Duration::from_secs(60));
    let mut ui = Conversation;
    let mut attempt = AttemptConversation::new("submitted", &limiter, &mut ui);
    for username in ["directory", "authenticated", "account-management"] {
        attempt.account_name(username).unwrap();
    }
    assert!(attempt.account_name("unexpected").is_err());
    let response = attempt.respond(AuthMessage { style: AuthMessageStyle::Info, text: "Use your token".into() });
    assert!(matches!(response, Ok(None)));
}
