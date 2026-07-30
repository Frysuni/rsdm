use crate::domain::{Session, SessionExit};

use super::ResolvedUser;

pub trait AuditLogger {
    fn auth_success(&self, username: &str);
    fn auth_failure(&self, username: &str, reason: &str);
    fn session_started(&self, user: &ResolvedUser, session: &Session);
    fn session_finished(&self, user: &ResolvedUser, session: &Session, exit: SessionExit);
}
