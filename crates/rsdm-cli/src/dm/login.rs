use anyhow::Result;
use rsdm_core::{
    application::login::{LoginError, LoginRequest, LoginUseCase},
    domain::{AppConfig, PasswordSecret, Session, SessionExit},
    ports::{
        LoginAttempt, LoginAttemptLimitError, LoginAttemptLimiter, LoginAttemptOutcome, UserStore,
    },
};
use rsdm_infra::{
    audit::TracingAuditLogger,
    pam::PamAuthProvider,
    storage::FileUserStore,
    unix::{
        LeaderGate, LeaderHandle, LeaderLaunch, LeaderReport, UnixSessionLauncher,
        UnixUserResolver, spawn_session_leader, terminate_requested,
    },
};

pub(super) struct ParkedSession {
    handle: LeaderHandle,
    username: String,
    session_id: String,
}

pub(super) fn begin(
    config: &AppConfig,
    wrapper: &[String],
    limiter: &dyn LoginAttemptLimiter,
    sessions: &[Session],
    parked: &mut Option<ParkedSession>,
    attempt: LoginAttempt,
) -> LoginAttemptOutcome {
    let LoginAttempt {
        username,
        mut password,
        session_id,
    } = attempt;
    let Some(session) = sessions.iter().find(|session| session.id == session_id) else {
        tracing::warn!(%username, %session_id, "selected session disappeared");
        return LoginAttemptOutcome::Failure("Selected session is unavailable".to_string());
    };

    if limiter.check_allowed(&username).is_err() {
        tracing::warn!(%username, "login attempt rate limited");
        return LoginAttemptOutcome::Failure("Too many failed attempts; please wait".to_string());
    }

    tracing::info!(
        %username,
        session_id = %session.id,
        session_name = %session.name,
        "login submitted"
    );
    let launch = spawn_session_leader(|gate| {
        child_login(config, wrapper, &username, &mut password, session, gate)
    });
    drop(password);
    match launch {
        LeaderLaunch::Ready(handle) => {
            limiter.record_success(&username);
            *parked = Some(ParkedSession {
                handle,
                username,
                session_id: session.id.clone(),
            });
            LoginAttemptOutcome::SessionReady
        }
        LeaderLaunch::Denied(report) => denied_outcome(limiter, username, report),
    }
}

fn denied_outcome(
    limiter: &dyn LoginAttemptLimiter,
    username: String,
    report: LeaderReport,
) -> LoginAttemptOutcome {
    let message = match report {
        LeaderReport::AuthFailed => {
            limiter.record_failure(&username);
            "Authentication failed"
        }
        LeaderReport::UserDenied => {
            limiter.record_success(&username);
            "User is not permitted to log in"
        }
        LeaderReport::SessionLaunchFailed => {
            limiter.record_success(&username);
            "Session launch failed"
        }
        _ => "The login process exited before reporting its result",
    };
    tracing::warn!(%username, ?report, "login failed");
    LoginAttemptOutcome::Failure(message.to_string())
}

pub(super) fn supervise(store: &FileUserStore, parked: ParkedSession) -> Option<String> {
    let ParkedSession {
        handle,
        username,
        session_id,
    } = parked;
    let report = handle.proceed();
    if terminate_requested() && report == LeaderReport::Lost {
        tracing::info!(%username, "service stop requested while supervising the session");
        return None;
    }

    match report {
        LeaderReport::SessionSuccess => {
            tracing::info!(%username, %session_id, "session completed successfully");
            persist_remembered(store, &username, &session_id);
            None
        }
        LeaderReport::SessionFailed(code) => {
            tracing::warn!(%username, %session_id, code, "session exited unexpectedly");
            persist_remembered(store, &username, &session_id);
            Some(format!("Session exited: Failed({code})"))
        }
        LeaderReport::SessionSignaled(signal) => {
            tracing::warn!(%username, %session_id, signal, "session killed by a signal");
            persist_remembered(store, &username, &session_id);
            Some(format!("Session exited: Signaled({signal})"))
        }
        LeaderReport::Lost => {
            tracing::warn!(%username, %session_id, "the session process exited before reporting");
            Some("The session process exited before reporting its result".to_string())
        }
        report => {
            tracing::error!(%username, %session_id, ?report, "unexpected post-launch report");
            Some("Session launch failed".to_string())
        }
    }
}

fn child_login(
    config: &AppConfig,
    wrapper: &[String],
    username: &str,
    password: &mut PasswordSecret,
    session: &Session,
    gate: &LeaderGate,
) -> LeaderReport {
    let auth = PamAuthProvider;
    let resolver = UnixUserResolver::with_allowed_groups(
        config.security.deny_root,
        config.security.allowed_groups.clone(),
    );
    let launcher = UnixSessionLauncher;
    let audit = TracingAuditLogger;
    let limiter = AllowAllLimiter;

    let result = LoginUseCase {
        auth: &auth,
        resolver: &resolver,
        launcher: &launcher,
        limiter: &limiter,
        audit: &audit,
        gate: Some(gate),
    }
    .execute(LoginRequest {
        username,
        password,
        pam_service: &config.dm.pam_service,
        session,
        tty: tty_name(&config.dm.tty.path),
        vtnr: config.dm.tty.vtnr(),
        seat: &config.dm.tty.seat,
        wrapper,
    });

    match result {
        Ok(login) => match login.exit {
            SessionExit::Success => LeaderReport::SessionSuccess,
            SessionExit::Failed(code) => LeaderReport::SessionFailed(code),
            SessionExit::Signaled(signal) => LeaderReport::SessionSignaled(signal),
        },
        Err(LoginError::Auth(_)) => LeaderReport::AuthFailed,
        Err(LoginError::User(_)) => LeaderReport::UserDenied,
        Err(LoginError::Session(_)) | Err(LoginError::RateLimited(_)) => {
            LeaderReport::SessionLaunchFailed
        }
    }
}

fn persist_remembered(store: &FileUserStore, username: &str, session_id: &str) {
    if let Err(error) = store.store_remembered_username(username) {
        tracing::warn!(%error, "could not persist remembered username");
    }
    if let Err(error) = store.store_remembered_session(session_id) {
        tracing::warn!(%error, "could not persist remembered session");
    }
}

struct AllowAllLimiter;

impl LoginAttemptLimiter for AllowAllLimiter {
    fn check_allowed(&self, _username: &str) -> Result<(), LoginAttemptLimitError> {
        Ok(())
    }

    fn record_failure(&self, _username: &str) {}

    fn record_success(&self, _username: &str) {}
}

fn tty_name(path: &str) -> &str {
    path.strip_prefix("/dev/").unwrap_or(path)
}
