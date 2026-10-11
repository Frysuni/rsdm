//! Identity of one login, distinct from the selected desktop entry.

use std::{future::Future, time::Duration};

pub(super) use rsdm_system::generation::new_generation;

use serde::{Deserialize, Serialize};
use zbus::zvariant::OwnedObjectPath;

use super::SessionError;

pub const GENERATION_ENV: &str = "RSDM_SESSION_GENERATION";
pub const DESKTOP_ENTRY_ENV: &str = "RSDM_DESKTOP_ENTRY_ID";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SessionIdentity {
    pub generation: String,
    pub login_session_id: String,
    pub desktop_entry_id: Option<String>,
    pub uid: u32,
}

impl SessionIdentity {
    pub async fn discover() -> Result<Self, SessionError> {
        let generation = match std::env::var(GENERATION_ENV) {
            Ok(generation) if valid_generation(&generation) => generation,
            Ok(_) => return Err(SessionError::State("invalid session generation".into())),
            Err(_) => super::runtime::blocking(|| Ok(new_generation()?)).await?,
        };
        // SAFETY: geteuid has no preconditions.
        let uid = unsafe { libc::geteuid() };
        let timeout = Duration::from_secs(5);
        let connection = async {
            Ok(zbus::connection::Builder::system()?.method_timeout(timeout).build().await?)
        };
        let session_hint = std::env::var("XDG_SESSION_ID").ok().filter(|id| !id.is_empty());
        let login_session_id = verified_login_session(connection, uid, session_hint.as_deref()).await?;
        Ok(Self {
            generation,
            login_session_id,
            desktop_entry_id: std::env::var(DESKTOP_ENTRY_ENV).ok(),
            uid,
        })
    }

    pub fn command_environment(&self, mut environment: Vec<(String, String)>) -> Vec<(String, String)> {
        environment.retain(|(name, _)| name != GENERATION_ENV && name != "XDG_SESSION_ID");
        environment.push((GENERATION_ENV.into(), self.generation.clone()));
        environment.push(("XDG_SESSION_ID".into(), self.login_session_id.clone()));
        environment
    }
}


pub(super) fn valid_generation(generation: &str) -> bool {
    generation.len() == 32
        && generation.bytes().all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

async fn verified_login_session(
    connect: impl Future<Output = Result<zbus::Connection, SessionError>>,
    uid: u32, session_hint: Option<&str>,
) -> Result<String, SessionError> {
    tokio::time::timeout(Duration::from_secs(5), async {
        let connection = connect.await?;
        let manager = zbus::Proxy::new(
            &connection, "org.freedesktop.login1", "/org/freedesktop/login1",
            "org.freedesktop.login1.Manager",
        ).await?;
        let path: OwnedObjectPath = manager.call("GetSessionByPID", &(std::process::id(),)).await?;
        let session = zbus::Proxy::new(
            &connection, "org.freedesktop.login1", path, "org.freedesktop.login1.Session",
        ).await?;
        let (owner, _): (u32, OwnedObjectPath) = session.get_property("User").await?;
        if owner != uid {
            return Err(SessionError::State("logind session belongs to another user".into()));
        }
        let id: String = session.get_property("Id").await?;
        if session_hint.is_some_and(|hint| hint != id) {
            return Err(SessionError::State("logind session hint does not match the current process".into()));
        }
        Ok(id)
    }).await.map_err(|_| {
        SessionError::Bus(zbus::fdo::Error::TimedOut("verifying the logind session timed out".into()).into())
    })?
}

#[cfg(test)]
#[path = "identity_login_tests.rs"]
mod login_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generations_cannot_be_used_as_runtime_path_traversal() {
        assert!(valid_generation("0123456789abcdef0123456789abcdef"));
        for value in ["", "../session", "0123456789ABCDEF0123456789ABCDEF", "0/23456789abcdef0123456789abcdef"] {
            assert!(!valid_generation(value));
        }
    }

    #[test]
    fn each_login_has_a_new_generation() {
        let first = new_generation().unwrap();
        assert!(valid_generation(&first));
        assert_ne!(first, new_generation().unwrap());
    }

    #[test]
    fn prepared_environment_replaces_stale_session_identity() {
        let identity = SessionIdentity {
            generation: "0123456789abcdef0123456789abcdef".into(),
            login_session_id: "42".into(),
            desktop_entry_id: Some("example".into()),
            uid: 1000,
        };
        let environment = identity.command_environment(vec![
            (GENERATION_ENV.into(), "old".into()),
            ("XDG_SESSION_ID".into(), "41".into()),
            ("PATH".into(), "/session/path".into()),
        ]);
        assert_eq!(environment.iter().filter(|(name, _)| name == GENERATION_ENV).count(), 1);
        assert!(environment.contains(&(GENERATION_ENV.into(), identity.generation.clone())));
        assert!(environment.contains(&("XDG_SESSION_ID".into(), "42".into())));
        assert!(environment.contains(&("PATH".into(), "/session/path".into())));
    }
}
