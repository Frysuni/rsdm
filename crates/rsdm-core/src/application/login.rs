use thiserror::Error;
use zeroize::Zeroize;

use crate::{
    domain::{PasswordSecret, Session, SessionExit},
    ports::{
        AuditLogger, AuthConversation, AuthError, AuthProvider, AuthRequest, AuthenticatedSession,
        LoginAttemptLimitError, LoginAttemptLimiter, ResolvedUser, RunningSession, SessionGate,
        SessionLaunchError, SessionLaunchRequest, SessionLauncher, UserResolveError, UserResolver,
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

        let started = StartUserSession {
            resolver: self.resolver,
            launcher: self.launcher,
            audit: self.audit,
            gate: self.gate,
        }
        .execute(&authenticated, &request);

        let mut running = None;
        let mut launched_user = None;
        let mut cleanup_result = Ok(());
        let session_result = match started {
            Ok((mut handle, user)) => {
                let (exit, cleanup) = self.wait_and_recover(handle.as_mut(), &user, request.session);
                cleanup_result = cleanup;
                launched_user = Some(user);
                running = Some(handle);
                exit
            }
            Err(error) => Err(error),
        };
        let close_result = ReturnToGreeterAfterSessionExit.execute(&mut authenticated);
        self.audit_cleanup(launched_user.as_ref(), request.session, &cleanup_result, &close_result);
        // The shutdown inhibitor belongs to the running handle, including on
        // wait/cleanup errors. Release it only after PAM has closed.
        drop(authenticated);
        drop(running);
        let exit = session_result?;
        cleanup_result?;
        close_result?;

        Ok(LoginResult { exit })
    }

    fn wait_and_recover(
        &self, running: &mut dyn RunningSession, user: &ResolvedUser, session: &Session,
    ) -> (Result<SessionExit, LoginError>, Result<(), LoginError>) {
        let exit = running.wait().map_err(LoginError::from);
        if let Ok(exit) = exit {
            self.audit.session_finished(user, session, exit);
        }
        let cleanup = running.cleanup().map_err(LoginError::from);
        (exit, cleanup)
    }

    fn audit_cleanup(
        &self, user: Option<&ResolvedUser>, session: &Session,
        recovery: &Result<(), LoginError>, pam: &Result<(), LoginError>,
    ) {
        let Some(user) = user else { return; };
        let error = match (recovery, pam) {
            (Err(recovery), Err(pam)) => Some(format!("{recovery}; {pam}")),
            (Err(error), _) | (_, Err(error)) => Some(error.to_string()),
            _ => None,
        };
        self.audit.session_cleanup_finished(user, session, error.as_deref());
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
        request: &LoginRequest<'_>,
    ) -> Result<(Box<dyn RunningSession>, ResolvedUser), LoginError> {
        let user = self
            .resolver
            .resolve_user(&authenticated.outcome.username)?;
        if let Some(gate) = self.gate {
            gate.session_authorized()?;
        }
        let running = self.launcher.start(SessionLaunchRequest {
            session: request.session,
            user: &user,
            pam_environment: authenticated.pam_session.environment(),
            vtnr: request.vtnr,
            seat: request.seat,
            wrapper: request.wrapper,
        })?;
        self.audit.session_started(&user, request.session);
        Ok((running, user))
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

#[cfg(test)]
#[path = "login_tests.rs"]
mod tests;
