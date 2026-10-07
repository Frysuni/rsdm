use thiserror::Error;

use crate::domain::{Session, SessionExit};

use super::ResolvedUser;

pub trait SessionDiscoverer {
    fn discover(&self) -> Result<Vec<Session>, SessionDiscoveryError>;
}

pub trait SessionLauncher {
    fn start(&self, request: SessionLaunchRequest<'_>) -> Result<Box<dyn RunningSession>, SessionLaunchError>;
}

/// Owns the child and its shutdown guard through the subsequent PAM close.
pub trait RunningSession {
    /// Wait for the session child without running recovery or closing PAM.
    fn wait(&mut self) -> Result<SessionExit, SessionLaunchError>;
    /// Recover generation-owned resources after the wait, even on a wait error.
    fn cleanup(&mut self) -> Result<(), SessionLaunchError>;
}

/// Parks the login between "the user is authenticated and authorized" and "the
/// terminal is free for the session". The greeter's implementation reports the
/// authorization to its parent process - which is still drawing the login form -
/// and blocks until the parent has torn down its UI and released the VT; only
/// then may the compositor be launched onto it. An error means the launch must
/// be aborted (the greeter is gone).
pub trait SessionGate {
    fn session_authorized(&self) -> Result<(), SessionLaunchError>;
}

#[derive(Debug)]
pub struct SessionLaunchRequest<'a> {
    pub session: &'a Session,
    pub user: &'a ResolvedUser,
    /// Environment published by the greeter's PAM session (`pam_getenvlist`):
    /// XDG_RUNTIME_DIR and the seat/VT vars from pam_systemd, plus any keyring
    /// agent vars (SSH_AUTH_SOCK, GNOME_KEYRING_CONTROL, ...) a keyring module in
    /// the stack exported when it unlocked the keyring. Merged into the session.
    pub pam_environment: &'a [(String, String)],
    /// Virtual terminal number exported as `XDG_VTNR` to the session.
    pub vtnr: Option<u32>,
    /// logind seat exported as `XDG_SEAT` to the session.
    pub seat: &'a str,
    /// Argv prefix prepended to the session command, e.g.
    /// `["/usr/bin/rsdm", "session", "start", "--"]` to wrap the compositor in
    /// the systemd session manager. Empty means launch the command directly.
    pub wrapper: &'a [String],
}

#[derive(Debug, Error)]
pub enum SessionDiscoveryError {
    #[error("session discovery failed: {0}")]
    Backend(String),
}

#[derive(Debug, Error)]
pub enum SessionLaunchError {
    #[error("session setup failed: {0}")]
    Setup(String),
    #[error("session process failed: {0}")]
    Process(String),
}
