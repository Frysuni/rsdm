use std::{cell::RefCell, rc::Rc};

use super::*;
use crate::ports::*;

type Events = Rc<RefCell<Vec<&'static str>>>;

struct Services {
    events: Events,
    start_fails: bool,
    wait_fails: bool,
    close_fails: bool,
}

#[derive(Debug)]
struct Pam { events: Events, close_fails: bool }

impl PamSession for Pam {
    fn environment(&self) -> &[(String, String)] { &[] }
    fn close(&mut self) -> Result<(), AuthError> {
        self.events.borrow_mut().push("pam-close");
        if self.close_fails { return Err(AuthError::Session("close failed".into())); }
        Ok(())
    }
}

impl Drop for Pam {
    fn drop(&mut self) { self.events.borrow_mut().push("pam-drop"); }
}

impl AuthProvider for Services {
    fn authenticate(&self, request: AuthRequest<'_>) -> Result<AuthenticatedSession, AuthError> {
        Ok(AuthenticatedSession { outcome: AuthOutcome { username: request.username.into() },
            pam_session: Box::new(Pam { events: self.events.clone(), close_fails: self.close_fails }) })
    }
}

struct Running { events: Events, fails: bool }

impl RunningSession for Running {
    fn wait(&mut self) -> Result<SessionExit, SessionLaunchError> {
        self.events.borrow_mut().push("cleanup");
        if self.fails { return Err(SessionLaunchError::Process("cleanup failed".into())); }
        Ok(SessionExit::Success)
    }
}

impl Drop for Running {
    fn drop(&mut self) { self.events.borrow_mut().push("inhibitor-release"); }
}

impl SessionLauncher for Services {
    fn start(&self, _: SessionLaunchRequest<'_>) -> Result<Box<dyn RunningSession>, SessionLaunchError> {
        self.events.borrow_mut().push("launch");
        if self.start_fails { return Err(SessionLaunchError::Process("launch failed".into())); }
        Ok(Box::new(Running { events: self.events.clone(), fails: self.wait_fails }))
    }
}

impl UserResolver for Services {
    fn resolve_user(&self, username: &str) -> Result<ResolvedUser, UserResolveError> {
        Ok(ResolvedUser { username: username.into(), uid: 1000, gid: 1000, home: "/home/example".into(), shell: "/bin/sh".into() })
    }
}

impl LoginAttemptLimiter for Services {
    fn check_allowed(&self, _: &str) -> Result<(), LoginAttemptLimitError> { Ok(()) }
    fn record_failure(&self, _: &str) {}
    fn record_success(&self, _: &str) {}
}

impl AuditLogger for Services {
    fn auth_success(&self, _: &str) {}
    fn auth_failure(&self, _: &str, _: &str) {}
    fn session_started(&self, _: &ResolvedUser, _: &Session) {
        self.events.borrow_mut().push("session-started");
    }
    fn session_finished(&self, _: &ResolvedUser, _: &Session, _: SessionExit) {}
}

fn execute(services: &Services) -> Result<LoginResult, LoginError> {
    let mut password = PasswordSecret::new("secret");
    let session = Session::new("example", "Example", "example-session", "/example.desktop");
    let result = LoginUseCase { auth: services, resolver: services, launcher: services, limiter: services,
        audit: services, gate: None }.execute(LoginRequest {
            username: "example", password: &mut password, pam_service: "test", session: &session,
            tty: "tty1", vtnr: Some(1), seat: "seat0", wrapper: &[], conversation: None,
        });
    assert!(password.expose_secret().is_empty());
    result
}

#[test]
fn cleanup_and_pam_end_precede_the_shutdown_inhibitor_release() {
    let services = Services { events: Events::default(), start_fails: false, wait_fails: false, close_fails: false };
    assert_eq!(execute(&services).unwrap().exit, SessionExit::Success);
    assert_eq!(*services.events.borrow(), ["launch", "session-started", "cleanup", "pam-close", "pam-drop", "inhibitor-release"]);
}

#[test]
fn cleanup_failure_still_closes_pam_before_releasing_the_inhibitor() {
    let services = Services { events: Events::default(), start_fails: false, wait_fails: true, close_fails: false };
    assert!(matches!(execute(&services), Err(LoginError::Session(_))));
    assert_eq!(*services.events.borrow(), ["launch", "session-started", "cleanup", "pam-close", "pam-drop", "inhibitor-release"]);
}

#[test]
fn pam_failure_still_keeps_the_inhibitor_until_pam_is_dropped() {
    let services = Services { events: Events::default(), start_fails: false, wait_fails: false, close_fails: true };
    assert!(matches!(execute(&services), Err(LoginError::Auth(_))));
    assert_eq!(*services.events.borrow(), ["launch", "session-started", "cleanup", "pam-close", "pam-drop", "inhibitor-release"]);
}

#[test]
fn launch_failure_does_not_leave_an_open_pam_session() {
    let services = Services { events: Events::default(), start_fails: true, wait_fails: false, close_fails: false };
    assert!(matches!(execute(&services), Err(LoginError::Session(_))));
    assert_eq!(*services.events.borrow(), ["launch", "pam-close", "pam-drop"]);
}
