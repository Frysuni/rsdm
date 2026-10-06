use std::{
    collections::HashMap,
    sync::{Mutex, MutexGuard},
    time::{Duration, Instant},
};

use rsdm_core::ports::{LoginAttemptLimitError, LoginAttemptLimiter, MAX_USERNAME_BYTES};

const MAX_TRACKED_USERNAMES: usize = 1024;

#[derive(Debug)]
pub struct MemoryLoginAttemptLimiter {
    max_failures: u8,
    delay: Duration,
    attempts: Mutex<HashMap<String, AttemptState>>,
}

impl MemoryLoginAttemptLimiter {
    pub fn new(max_failures: u8, delay: Duration) -> Self {
        Self {
            max_failures,
            delay,
            attempts: Mutex::new(HashMap::new()),
        }
    }
}

impl LoginAttemptLimiter for MemoryLoginAttemptLimiter {
    fn check_allowed(&self, username: &str) -> Result<(), LoginAttemptLimitError> {
        let mut attempts = self.lock_attempts();
        attempts.retain(|_, state| state.last_failure.elapsed() < self.delay);
        if username.len() > MAX_USERNAME_BYTES
            || (!attempts.contains_key(username) && attempts.len() >= MAX_TRACKED_USERNAMES)
        {
            return Err(LoginAttemptLimitError::RateLimited {
                username: username.to_string(),
            });
        }
        let Some(state) = attempts.get(username) else {
            return Ok(());
        };

        if state.failures >= self.max_failures && state.last_failure.elapsed() < self.delay {
            Err(LoginAttemptLimitError::RateLimited {
                username: username.to_string(),
            })
        } else {
            Ok(())
        }
    }

    fn record_failure(&self, username: &str) {
        let mut attempts = self.lock_attempts();
        attempts.retain(|_, state| state.last_failure.elapsed() < self.delay);
        if username.len() > MAX_USERNAME_BYTES
            || (!attempts.contains_key(username) && attempts.len() >= MAX_TRACKED_USERNAMES)
        {
            return;
        }
        let state = attempts.entry(username.to_string()).or_default();
        state.failures = state.failures.saturating_add(1);
        state.last_failure = Instant::now();
    }

    fn record_success(&self, username: &str) {
        self.lock_attempts().remove(username);
    }
}

impl MemoryLoginAttemptLimiter {
    fn lock_attempts(&self) -> MutexGuard<'_, HashMap<String, AttemptState>> {
        self.attempts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[derive(Debug)]
struct AttemptState {
    failures: u8,
    last_failure: Instant,
}

impl Default for AttemptState {
    fn default() -> Self {
        Self {
            failures: 0,
            last_failure: Instant::now(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_until_threshold_then_blocks() {
        let limiter = MemoryLoginAttemptLimiter::new(2, Duration::from_secs(60));
        assert!(limiter.check_allowed("alice").is_ok());
        limiter.record_failure("alice");
        assert!(limiter.check_allowed("alice").is_ok());
        limiter.record_failure("alice");
        assert!(limiter.check_allowed("alice").is_err());
    }

    #[test]
    fn success_resets_the_counter() {
        let limiter = MemoryLoginAttemptLimiter::new(1, Duration::from_secs(60));
        limiter.record_failure("bob");
        assert!(limiter.check_allowed("bob").is_err());
        limiter.record_success("bob");
        assert!(limiter.check_allowed("bob").is_ok());
    }

    #[test]
    fn expired_delay_allows_again() {
        let limiter = MemoryLoginAttemptLimiter::new(1, Duration::from_millis(0));
        limiter.record_failure("carol");
        assert!(limiter.check_allowed("carol").is_ok());
        assert!(limiter.lock_attempts().is_empty());
    }

    #[test]
    fn distinct_usernames_cannot_evict_blocked_accounts_or_grow_without_limit() {
        let limiter = MemoryLoginAttemptLimiter::new(1, Duration::from_secs(60));
        for index in 0..MAX_TRACKED_USERNAMES + 10 {
            limiter.record_failure(&format!("user{index}"));
        }
        assert_eq!(limiter.lock_attempts().len(), MAX_TRACKED_USERNAMES);
        assert!(limiter.check_allowed("user0").is_err());
        assert!(limiter.check_allowed("another-user").is_err());
    }

    #[test]
    fn oversized_usernames_are_not_retained() {
        let limiter = MemoryLoginAttemptLimiter::new(1, Duration::from_secs(60));
        let username = "x".repeat(MAX_USERNAME_BYTES + 1);
        assert!(limiter.check_allowed(&username).is_err());
        limiter.record_failure(&username);
        assert!(limiter.lock_attempts().is_empty());
    }
}
