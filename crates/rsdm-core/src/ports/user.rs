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
