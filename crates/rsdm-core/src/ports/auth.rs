use thiserror::Error;

use crate::domain::PasswordSecret;

pub const MAX_USERNAME_BYTES: usize = 256;
pub const MAX_PASSWORD_BYTES: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMessageStyle {
    Secret,
    Visible,
    Info,
    Error,
}

#[derive(Debug)]
pub struct AuthMessage {
    pub style: AuthMessageStyle,
    pub text: String,
}

pub trait AuthConversation: std::fmt::Debug + Send {
    fn respond(&mut self, message: AuthMessage) -> Result<Option<PasswordSecret>, AuthError>;

    /// Report the backend's account mapping, including after a failed attempt.
    /// The login owner may reject an account whose failure budget is exhausted.
    fn account_name(&mut self, _username: &str) -> Result<(), AuthError> { Ok(()) }
}

#[derive(Debug)]
pub struct AuthRequest<'a> {
    pub username: &'a str,
    pub password: &'a PasswordSecret,
    pub pam_service: &'a str,
    /// VT device name (e.g. `tty1`) handed to PAM as `PAM_TTY` so `pam_systemd`
    /// registers the logind session on the right seat.
    pub tty: &'a str,
    /// Virtual terminal number for `XDG_VTNR`. `None` outside a real VT.
    pub vtnr: Option<u32>,
    /// logind seat, e.g. `seat0`.
    pub seat: &'a str,
    /// First `DesktopNames=` entry of the selected session, handed to PAM as
    /// `XDG_SESSION_DESKTOP` so logind records which desktop the session runs.
    pub session_desktop: Option<&'a str>,
    pub conversation: Option<Box<dyn AuthConversation>>,
}

#[derive(Debug)]
pub struct AuthenticatedSession {
    pub outcome: AuthOutcome,
    pub pam_session: Box<dyn PamSession>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthOutcome {
    pub username: String,
}

pub trait AuthProvider {
    fn authenticate(&self, request: AuthRequest<'_>) -> Result<AuthenticatedSession, AuthError>;
}

/// Verify a password without opening a session. Used by the locker, which only
/// needs to prove the seated user is still present.
pub trait CredentialVerifier {
    fn verify(&self, request: VerifyRequest<'_>) -> Result<(), AuthError>;
}

#[derive(Debug)]
pub struct VerifyRequest<'a> {
    pub username: &'a str,
    pub password: &'a PasswordSecret,
    pub pam_service: &'a str,
    pub conversation: Option<Box<dyn AuthConversation>>,
}

pub trait PamSession: std::fmt::Debug {
    fn environment(&self) -> &[(String, String)];
    fn close(&mut self) -> Result<(), AuthError>;
}

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("authentication failed")]
    InvalidCredentials,
    #[error("account is not permitted to log in")]
    AccountDenied,
    #[error("PAM session failed: {0}")]
    Session(String),
    #[error("authentication backend error: {0}")]
    Backend(String),
}
