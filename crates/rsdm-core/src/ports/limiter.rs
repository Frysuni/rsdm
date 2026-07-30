use thiserror::Error;

pub trait LoginAttemptLimiter {
    fn check_allowed(&self, username: &str) -> Result<(), LoginAttemptLimitError>;
    fn record_failure(&self, username: &str);
    fn record_success(&self, username: &str);
}

#[derive(Debug, Error)]
pub enum LoginAttemptLimitError {
    #[error("too many failed login attempts for {username}")]
    RateLimited { username: String },
}
