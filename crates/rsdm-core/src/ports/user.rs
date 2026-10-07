use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedUser {
    pub username: String,
    pub uid: u32,
    pub gid: u32,
    pub home: String,
    pub shell: String,
}

pub trait UserResolver {
    fn resolve_user(&self, username: &str) -> Result<ResolvedUser, UserResolveError>;

    /// Return the account name that should identify this login attempt.
    /// Implementations may map directory/realm aliases; failures are handled
    /// by the login flow using the submitted name so authentication semantics
    /// remain unchanged.
    fn canonical_username(&self, username: &str) -> Result<String, UserResolveError> {
        Ok(username.to_string())
    }
}

#[derive(Debug, Error)]
pub enum UserResolveError {
    #[error("user not found: {0}")]
    NotFound(String),
    #[error("user is not allowed: {0}")]
    Denied(String),
    #[error("user resolver failed: {0}")]
    Backend(String),
}
