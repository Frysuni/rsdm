use std::{cell::RefCell, rc::Rc};

use super::*;
use crate::ports::*;

type Events = Rc<RefCell<Vec<&'static str>>>;

struct Services {
    events: Events,
    cleanup_errors: RefCell<Vec<String>>,
    start_fails: bool,
    wait_fails: bool,
    cleanup_fails: bool,
    close_fails: bool,
    exit: SessionExit,
}

impl Default for Services {
    fn default() -> Self {
        Self {
            events: Events::default(), cleanup_errors: RefCell::default(),
            start_fails: false, wait_fails: false, cleanup_fails: false, close_fails: false,
            exit: SessionExit::Success,
        }
    }
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

struct Running { events: Events, wait_fails: bool, cleanup_fails: bool, exit: SessionExit }

impl RunningSession for Running {
    fn wait(&mut self) -> Result<SessionExit, SessionLaunchError> {
        self.events.borrow_mut().push("wait");
        if self.wait_fails { return Err(SessionLaunchError::Process("wait failed".into())); }
        Ok(self.exit)
    }

    fn cleanup(&mut self) -> Result<(), SessionLaunchError> {
        self.events.borrow_mut().push("cleanup");
        if self.cleanup_fails { return Err(SessionLaunchError::Process("cleanup failed".into())); }
        Ok(())
    }
}

impl Drop for Running {
    fn drop(&mut self) { self.events.borrow_mut().push("inhibitor-release"); }
}

impl SessionLauncher for Services {
    fn start(&self, _: SessionLaunchRequest<'_>) -> Result<Box<dyn RunningSession>, SessionLaunchError> {
        self.events.borrow_mut().push("launch");
        if self.start_fails { return Err(SessionLaunchError::Process("launch failed".into())); }
        Ok(Box::new(Running { events: self.events.clone(), wait_fails: self.wait_fails,
            cleanup_fails: self.cleanup_fails, exit: self.exit }))
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
    fn session_finished(&self, _: &ResolvedUser, _: &Session, exit: SessionExit) {
        assert_eq!(exit, self.exit);
        self.events.borrow_mut().push("session-finished");
    }
    fn session_cleanup_finished(&self, _: &ResolvedUser, _: &Session, error: Option<&str>) {
        if let Some(error) = error {
            self.cleanup_errors.borrow_mut().push(error.to_string());
            self.events.borrow_mut().push("cleanup-failed");
        } else {
            self.events.borrow_mut().push("cleanup-finished");
        }
    }
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
    let services = Services::default();
    assert_eq!(execute(&services).unwrap().exit, SessionExit::Success);
    assert_eq!(*services.events.borrow(), [
        "launch", "session-started", "wait", "session-finished", "cleanup",
        "pam-close", "cleanup-finished", "pam-drop", "inhibitor-release",
    ]);
}

#[test]
fn cleanup_failure_still_closes_pam_before_releasing_the_inhibitor() {
    let services = Services { cleanup_fails: true, ..Services::default() };
    assert!(matches!(execute(&services), Err(LoginError::Session(_))));
    assert_eq!(*services.events.borrow(), [
        "launch", "session-started", "wait", "session-finished", "cleanup",
        "pam-close", "cleanup-failed", "pam-drop", "inhibitor-release",
    ]);
}

#[test]
fn pam_failure_still_keeps_the_inhibitor_until_pam_is_dropped() {
    let services = Services { close_fails: true, ..Services::default() };
    assert!(matches!(execute(&services), Err(LoginError::Auth(_))));
    assert_eq!(*services.events.borrow(), [
        "launch", "session-started", "wait", "session-finished", "cleanup",
        "pam-close", "cleanup-failed", "pam-drop", "inhibitor-release",
    ]);
}

#[test]
fn launch_failure_does_not_leave_an_open_pam_session() {
    let services = Services { start_fails: true, ..Services::default() };
    assert!(matches!(execute(&services), Err(LoginError::Session(_))));
    assert_eq!(*services.events.borrow(), ["launch", "pam-close", "pam-drop"]);
}

#[test]
fn an_unknown_exit_is_not_reported_as_finished_but_cleanup_still_runs() {
    let services = Services { wait_fails: true, ..Services::default() };
    assert!(matches!(execute(&services), Err(LoginError::Session(_))));
    assert_eq!(*services.events.borrow(), ["launch", "session-started", "wait", "cleanup",
        "pam-close", "cleanup-finished", "pam-drop", "inhibitor-release"]);
}

#[test]
fn failed_and_signaled_exits_are_audited_before_failed_recovery() {
    for exit in [SessionExit::Failed(7), SessionExit::Signaled(9)] {
        let services = Services { cleanup_fails: true, exit, ..Services::default() };
        assert!(matches!(execute(&services), Err(LoginError::Session(_))));
        assert_eq!(*services.events.borrow(), ["launch", "session-started", "wait", "session-finished",
            "cleanup", "pam-close", "cleanup-failed", "pam-drop", "inhibitor-release"]);
    }
}

#[test]
fn combined_recovery_and_pam_errors_are_both_in_the_cleanup_audit() {
    let services = Services { cleanup_fails: true, close_fails: true, ..Services::default() };
    assert!(matches!(execute(&services), Err(LoginError::Session(_))));
    let errors = services.cleanup_errors.borrow();
    assert_eq!(errors.len(), 1);
    assert!(errors[0].contains("cleanup failed"));
    assert!(errors[0].contains("close failed"));
    assert_eq!(*services.events.borrow(), [
        "launch", "session-started", "wait", "session-finished", "cleanup",
        "pam-close", "cleanup-failed", "pam-drop", "inhibitor-release",
    ]);
}
