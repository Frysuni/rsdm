use thiserror::Error;
use zeroize::Zeroize;

use crate::{
    domain::{PasswordSecret, Session, SessionExit},
    ports::{
        AuditLogger, AuthConversation, AuthError, AuthProvider, AuthRequest, AuthenticatedSession,
        LoginAttemptLimitError, LoginAttemptLimiter, SessionGate, SessionLaunchError,
        SessionLaunchRequest, SessionLauncher, UserResolveError, UserResolver,
    },
};

pub struct LoginUseCase<'a> {
    pub auth: &'a dyn AuthProvider,
    pub resolver: &'a dyn UserResolver,
    pub launcher: &'a dyn SessionLauncher,
    pub limiter: &'a dyn LoginAttemptLimiter,
    pub audit: &'a dyn AuditLogger,
    /// Parks the login once the user is authenticated and authorized, until the
    /// greeter has released the terminal. None launches immediately.
    pub gate: Option<&'a dyn SessionGate>,
}

impl LoginUseCase<'_> {
    pub fn execute(&self, mut request: LoginRequest<'_>) -> Result<LoginResult, LoginError> {
        let authenticated = AuthenticateUser {
            auth: self.auth,
            limiter: self.limiter,
            audit: self.audit,
        }
        .execute(request.auth_request());
        request.password.zeroize();
        let mut authenticated = authenticated?;

        let session_result = StartUserSession {
            resolver: self.resolver,
            launcher: self.launcher,
            audit: self.audit,
            gate: self.gate,
        }
        .execute(
            &authenticated,
            request.session,
            request.vtnr,
            request.seat,
            request.wrapper,
        );

        let close_result = ReturnToGreeterAfterSessionExit.execute(&mut authenticated);
        let exit = session_result?;
        close_result?;

        Ok(LoginResult { exit })
    }
}

struct AuthenticateUser<'a> {
    auth: &'a dyn AuthProvider,
    limiter: &'a dyn LoginAttemptLimiter,
    audit: &'a dyn AuditLogger,
}

impl AuthenticateUser<'_> {
    fn execute(&self, request: AuthRequest<'_>) -> Result<AuthenticatedSession, LoginError> {
        self.limiter.check_allowed(request.username)?;
        let username = request.username;

        match self.auth.authenticate(request) {
            Ok(session) => {
                self.limiter.record_success(&session.outcome.username);
                self.audit.auth_success(&session.outcome.username);
                Ok(session)
            }
            Err(error) => {
                self.limiter.record_failure(username);
                self.audit.auth_failure(username, error.audit_reason());
                Err(LoginError::Auth(error))
            }
        }
    }
}

struct StartUserSession<'a> {
    resolver: &'a dyn UserResolver,
    launcher: &'a dyn SessionLauncher,
    audit: &'a dyn AuditLogger,
    gate: Option<&'a dyn SessionGate>,
}

impl StartUserSession<'_> {
    fn execute(
        &self,
        authenticated: &AuthenticatedSession,
        session: &Session,
        vtnr: Option<u32>,
        seat: &str,
        wrapper: &[String],
    ) -> Result<SessionExit, LoginError> {
        let user = self
            .resolver
            .resolve_user(&authenticated.outcome.username)?;
        if let Some(gate) = self.gate {
            gate.session_authorized()?;
        }
        self.audit.session_started(&user, session);
        let exit = self.launcher.launch(SessionLaunchRequest {
            session,
            user: &user,
            pam_environment: authenticated.pam_session.environment(),
            vtnr,
            seat,
            wrapper,
        })?;
        self.audit.session_finished(&user, session, exit);
        Ok(exit)
    }
}

struct ReturnToGreeterAfterSessionExit;

impl ReturnToGreeterAfterSessionExit {
    fn execute(&self, authenticated: &mut AuthenticatedSession) -> Result<(), LoginError> {
        authenticated.pam_session.close()?;
        Ok(())
    }
}

pub struct LoginRequest<'a> {
    pub username: &'a str,
    pub password: &'a mut PasswordSecret,
    pub pam_service: &'a str,
    pub session: &'a Session,
    pub tty: &'a str,
    pub vtnr: Option<u32>,
    pub seat: &'a str,
    /// Argv prefix that wraps the session command (session-manager).
    pub wrapper: &'a [String],
    pub conversation: Option<Box<dyn AuthConversation>>,
}

impl<'a> LoginRequest<'a> {
    fn auth_request(&mut self) -> AuthRequest<'_> {
        AuthRequest {
            username: self.username,
            password: self.password,
            pam_service: self.pam_service,
            tty: self.tty,
            vtnr: self.vtnr,
            seat: self.seat,
            session_desktop: self.session.desktop_names.first().map(String::as_str),
            conversation: self.conversation.take(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoginResult {
    pub exit: SessionExit,
}

#[derive(Debug, Error)]
pub enum LoginError {
    #[error(transparent)]
    Auth(#[from] AuthError),
    #[error(transparent)]
    User(#[from] UserResolveError),
    #[error(transparent)]
    Session(#[from] SessionLaunchError),
    #[error(transparent)]
    RateLimited(#[from] LoginAttemptLimitError),
}

trait AuditReason {
    fn audit_reason(&self) -> &'static str;
}

impl AuditReason for AuthError {
    fn audit_reason(&self) -> &'static str {
        match self {
            AuthError::InvalidCredentials => "invalid-credentials",
            AuthError::AccountDenied => "account-denied",
            AuthError::Session(_) => "pam-session",
            AuthError::Backend(_) => "backend",
        }
    }
}
