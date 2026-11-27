use std::{cell::RefCell, rc::Rc};

use super::*;
use crate::ports::*;

type Events = Rc<RefCell<Vec<&'static str>>>;

struct LaunchContext {
    user: ResolvedUser,
    session: Session,
    environment: Vec<(String, String)>,
    vtnr: Option<u32>,
    seat: String,
    wrapper: Vec<String>,
}

struct Services {
    events: Events,
    cleanup_errors: RefCell<Vec<String>>,
    start_fails: bool,
    wait_fails: bool,
    cleanup_fails: bool,
    close_fails: bool,
    exit: SessionExit,
    canonical: Option<String>,
    limiter_keys: RefCell<Vec<String>>,
    prelaunch: RefCell<Vec<String>>,
    blocked: bool,
    auth_rejected: bool,
    resolver_denied: bool,
    gate_closed: bool,
    mapped_account: Option<String>,
    pam_environment: Vec<(String, String)>,
    launch_context: RefCell<Option<LaunchContext>>,
}

impl Default for Services {
    fn default() -> Self {
        Self {
            events: Events::default(), cleanup_errors: RefCell::default(),
            start_fails: false, wait_fails: false, cleanup_fails: false, close_fails: false,
            exit: SessionExit::Success, canonical: None, limiter_keys: RefCell::default(),
            prelaunch: RefCell::default(), blocked: false, auth_rejected: false,
            resolver_denied: false, gate_closed: false, mapped_account: None,
            pam_environment: Vec::new(), launch_context: RefCell::default(),
        }
    }
}

#[derive(Debug)]
struct Pam { events: Events, close_fails: bool, environment: Vec<(String, String)> }

impl PamSession for Pam {
    fn environment(&self) -> &[(String, String)] { &self.environment }
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
        self.prelaunch.borrow_mut().push(format!("pam:{}", request.username));
        assert_eq!(request.password.expose_secret(), "secret");
        if self.auth_rejected { return Err(AuthError::InvalidCredentials); }
        Ok(AuthenticatedSession {
            outcome: AuthOutcome { username: self.mapped_account.clone().unwrap_or_else(|| request.username.into()) },
            pam_session: Box::new(Pam { events: self.events.clone(), close_fails: self.close_fails,
                environment: self.pam_environment.clone() }),
        })
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
    fn start(&self, request: SessionLaunchRequest<'_>) -> Result<Box<dyn RunningSession>, SessionLaunchError> {
        self.prelaunch.borrow_mut().push("launch".into());
        *self.launch_context.borrow_mut() = Some(LaunchContext {
            user: request.user.clone(), session: request.session.clone(),
            environment: request.pam_environment.to_vec(), vtnr: request.vtnr,
            seat: request.seat.into(), wrapper: request.wrapper.to_vec(),
        });
        self.events.borrow_mut().push("launch");
        if self.start_fails { return Err(SessionLaunchError::Process("launch failed".into())); }
        Ok(Box::new(Running { events: self.events.clone(), wait_fails: self.wait_fails,
            cleanup_fails: self.cleanup_fails, exit: self.exit }))
    }
}

impl UserResolver for Services {
    fn canonical_username(&self, username: &str) -> Result<String, UserResolveError> {
        self.prelaunch.borrow_mut().push(format!("canonical:{username}"));
        Ok(self.canonical.clone().unwrap_or_else(|| username.to_string()))
    }

    fn resolve_user(&self, username: &str) -> Result<ResolvedUser, UserResolveError> {
        self.prelaunch.borrow_mut().push(format!("resolve:{username}"));
        if self.resolver_denied { return Err(UserResolveError::Denied(username.into())); }
        Ok(ResolvedUser { username: username.into(), uid: 1000, gid: 1000, home: "/home/example".into(), shell: "/bin/sh".into() })
    }
}

impl LoginAttemptLimiter for Services {
    fn check_allowed(&self, username: &str) -> Result<(), LoginAttemptLimitError> {
        self.limiter_keys.borrow_mut().push(format!("check:{username}"));
        self.prelaunch.borrow_mut().push(format!("check:{username}"));
        if self.blocked { return Err(LoginAttemptLimitError::RateLimited { username: username.into() }); }
        Ok(())
    }
    fn record_failure(&self, username: &str) {
        self.limiter_keys.borrow_mut().push(format!("failure:{username}"));
        self.prelaunch.borrow_mut().push(format!("failure:{username}"));
    }
    fn record_success(&self, username: &str) {
        self.limiter_keys.borrow_mut().push(format!("success:{username}"));
        self.prelaunch.borrow_mut().push(format!("success:{username}"));
    }
}

impl AuditLogger for Services {
    fn auth_success(&self, username: &str) {
        self.prelaunch.borrow_mut().push(format!("auth-success:{username}"));
    }
    fn auth_failure(&self, username: &str, reason: &str) {
        self.prelaunch.borrow_mut().push(format!("auth-failure:{username}:{reason}"));
    }
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

impl SessionGate for Services {
    fn session_authorized(&self) -> Result<(), SessionLaunchError> {
        self.prelaunch.borrow_mut().push("gate".into());
        if self.gate_closed { return Err(SessionLaunchError::Setup("greeter disconnected".into())); }
        Ok(())
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

#[test]
fn aliases_share_one_limiter_identity_after_directory_canonicalization() {
    let services = Services { canonical: Some("alice".into()), ..Services::default() };
    let mut password = PasswordSecret::new("secret");
    let session = Session::new("example", "Example", "example-session", "/example.desktop");
    let result = LoginUseCase { auth: &services, resolver: &services, launcher: &services,
        limiter: &services, audit: &services, gate: None }.execute(LoginRequest {
            username: "ALICE@example", password: &mut password, pam_service: "test", session: &session,
            tty: "tty1", vtnr: Some(1), seat: "seat0", wrapper: &[], conversation: None,
        });
    assert!(result.is_ok());
    assert_eq!(&*services.limiter_keys.borrow(), &vec![String::from("check:alice"), String::from("success:alice")]);
}


fn execute_gated(services: &Services) -> Result<LoginResult, LoginError> {
    let mut password = PasswordSecret::new("secret");
    let mut session = Session::new("wayland", "Wayland", "compositor", "/wayland.desktop");
    session.desktop_names = vec!["compositor".into()];
    let wrapper = ["rsdm", "session", "start", "--"].map(String::from);
    let result = LoginUseCase { auth: services, resolver: services, launcher: services,
        limiter: services, audit: services, gate: Some(services) }.execute(LoginRequest {
            username: "ALICE@example", password: &mut password, pam_service: "rsdm",
            session: &session, tty: "tty7", vtnr: Some(7), seat: "seat1",
            wrapper: &wrapper, conversation: None,
        });
    assert!(password.expose_secret().is_empty());
    result
}

#[test]
fn rejected_alias_and_rate_limit_never_open_or_launch_a_session() {
    let mut services = Services { canonical: Some("alice".into()), auth_rejected: true, ..Services::default() };
    assert!(matches!(execute_gated(&services), Err(LoginError::Auth(AuthError::InvalidCredentials))));
    assert_eq!(*services.prelaunch.borrow(), [
        "canonical:ALICE@example", "check:alice", "pam:ALICE@example", "failure:alice", "auth-failure:alice:invalid-credentials",
    ]);
    assert_eq!(*services.limiter_keys.borrow(), ["check:alice", "failure:alice"]);
    assert!(services.events.borrow().is_empty());
    services.prelaunch.borrow_mut().clear();
    services.blocked = true;
    services.auth_rejected = false;
    assert!(matches!(execute_gated(&services), Err(LoginError::RateLimited(_))));
    assert_eq!(*services.prelaunch.borrow(), ["canonical:ALICE@example", "check:alice"]);
    assert_eq!(*services.limiter_keys.borrow(), ["check:alice", "failure:alice", "check:alice"]);
    assert!(services.events.borrow().is_empty());
    assert!(services.launch_context.borrow().is_none());
}

#[test]
fn denied_mapped_account_closes_pam_without_releasing_the_greeter() {
    let services = Services { canonical: Some("alice".into()), mapped_account: Some("local-alice".into()),
        resolver_denied: true, ..Services::default() };
    assert!(matches!(execute_gated(&services), Err(LoginError::User(UserResolveError::Denied(name))) if name == "local-alice"));
    assert_eq!(*services.prelaunch.borrow(), [
        "canonical:ALICE@example", "check:alice", "pam:ALICE@example", "success:alice", "auth-success:local-alice", "resolve:local-alice",
    ]);
    assert_eq!(*services.events.borrow(), ["pam-close", "pam-drop"]);
    assert!(services.launch_context.borrow().is_none());
}

#[test]
fn disconnected_greeter_aborts_launch_and_next_login_preserves_pam_context() {
    let mut services = Services {
        canonical: Some("alice".into()), mapped_account: Some("local-alice".into()),
        gate_closed: true,
        pam_environment: vec![("XDG_RUNTIME_DIR".into(), "/run/user/1000".into()),
            ("SSH_AUTH_SOCK".into(), "/run/user/1000/keyring/ssh".into())],
        ..Services::default()
    };
    let before_launch = [
        "canonical:ALICE@example", "check:alice", "pam:ALICE@example", "success:alice", "auth-success:local-alice",
        "resolve:local-alice", "gate",
    ];
    assert!(matches!(execute_gated(&services), Err(LoginError::Session(SessionLaunchError::Setup(_)))));
    assert_eq!(*services.prelaunch.borrow(), before_launch);
    assert_eq!(*services.events.borrow(), ["pam-close", "pam-drop"]);
    assert!(services.launch_context.borrow().is_none());

    services.gate_closed = false;
    services.prelaunch.borrow_mut().clear();
    services.events.borrow_mut().clear();
    assert_eq!(execute_gated(&services).unwrap().exit, SessionExit::Success);
    assert_eq!(*services.prelaunch.borrow(), [before_launch.as_slice(), &["launch"]].concat());
    assert_eq!(*services.events.borrow(), ["launch", "session-started", "wait", "session-finished",
        "cleanup", "pam-close", "cleanup-finished", "pam-drop", "inhibitor-release"]);
    let context = services.launch_context.borrow();
    let context = context.as_ref().unwrap();
    assert_eq!(context.user.username, "local-alice");
    assert_eq!(context.session.id, "wayland");
    assert_eq!(context.session.desktop_names, ["compositor"]);
    assert_eq!(context.environment, services.pam_environment);
    assert_eq!(context.vtnr, Some(7));
    assert_eq!(context.seat, "seat1");
    assert_eq!(context.wrapper, ["rsdm", "session", "start", "--"]);
}
