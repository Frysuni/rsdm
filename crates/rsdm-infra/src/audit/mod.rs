use rsdm_core::{
    domain::{Session, SessionExit},
    ports::{AuditLogger, ResolvedUser},
};

#[derive(Debug, Default)]
pub struct TracingAuditLogger;

impl AuditLogger for TracingAuditLogger {
    fn auth_success(&self, username: &str) {
        tracing::info!(username, "authentication succeeded");
    }

    fn auth_failure(&self, username: &str, reason: &str) {
        tracing::warn!(username, reason, "authentication failed");
    }

    fn session_started(&self, user: &ResolvedUser, session: &Session) {
        tracing::info!(
            username = %user.username,
            uid = user.uid,
            session = %session.id,
            "session started"
        );
    }

    fn session_finished(&self, user: &ResolvedUser, session: &Session, exit: SessionExit) {
        tracing::info!(
            username = %user.username,
            session = %session.id,
            ?exit,
            "session finished"
        );
    }

    fn session_cleanup_finished(&self, user: &ResolvedUser, session: &Session, error: Option<&str>) {
        match error {
            Some(error) => tracing::error!(
                username = %user.username, session = %session.id, error,
                "session cleanup failed"
            ),
            None => tracing::info!(
                username = %user.username, session = %session.id,
                "session cleanup completed"
            ),
        }
    }
}
