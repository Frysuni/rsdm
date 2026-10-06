use std::sync::atomic::AtomicBool;

use thiserror::Error;

use crate::domain::{AppConfig, PasswordSecret, Session};
use super::AuthConversation;

pub trait LoginUi {
    /// Drive the login screen until something leaves it.
    ///
    /// The UI stays on screen for the whole conversation: a submitted form is
    /// handed to `attempt` while an "Authenticating..." status is showing, and
    /// a [`LoginAttemptOutcome::Failure`] is rendered inline - the terminal is
    /// never torn down between attempts. Only a
    /// [`LoginAttemptOutcome::SessionReady`] (or a power/exit action) returns,
    /// at which point the caller restores the terminal and lets the parked
    /// session take the VT.
    fn run(
        &mut self,
        model: LoginUiModel<'_>,
        attempt: &mut dyn FnMut(LoginAttempt, &mut dyn AuthConversation) -> LoginAttemptOutcome,
    ) -> Result<LoginUiEvent, UiError>;
}

#[derive(Debug)]
pub struct LoginUiModel<'a> {
    pub config: &'a AppConfig,
    pub sessions: &'a [Session],
    pub remembered_username: Option<&'a str>,
    pub remembered_session: Option<&'a str>,
    pub error_message: Option<&'a str>,
    /// Raised (from a signal handler) when the service is stopping. The UI
    /// returns [`LoginUiEvent::Terminated`] as soon as it notices, so the
    /// terminal is restored and cleared instead of being killed mid-frame.
    pub terminate: &'a AtomicBool,
}

/// A submitted login form, handed to the attempt callback.
#[derive(Debug)]
pub struct LoginAttempt {
    pub username: String,
    pub password: PasswordSecret,
    pub session_id: String,
}

/// What became of a submitted form.
#[derive(Debug)]
pub enum LoginAttemptOutcome {
    /// The attempt ended at the login screen; show the message inline and keep
    /// the form up for another try.
    Failure(String),
    /// Authenticated and authorized: the session is parked waiting for the
    /// terminal. The UI must return [`LoginUiEvent::SessionReady`] now.
    SessionReady,
}

#[derive(Debug)]
pub enum LoginUiEvent {
    /// An attempt succeeded; the caller releases the terminal and resumes the
    /// parked session.
    SessionReady,
    ExitToTty,
    Reboot,
    Shutdown,
    /// The service-stop flag was raised; the caller exits cleanly.
    Terminated,
}

#[derive(Debug, Error)]
pub enum UiError {
    #[error("terminal error: {0}")]
    Terminal(String),
}
