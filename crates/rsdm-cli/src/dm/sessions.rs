use anyhow::{Context, Result, bail};
use rsdm_core::{
    domain::{AppConfig, Session},
    ports::SessionDiscoverer,
};
use rsdm_infra::sessions::DesktopSessionDiscoverer;

pub(super) fn resolve_sessions(config: &AppConfig) -> Result<Vec<Session>> {
    let discovered = DesktopSessionDiscoverer::from_config(&config.dm)
        .discover()
        .context("discovering sessions")?;

    if let Some(fixed) = config.dm.fixed_session.as_deref() {
        return Ok(vec![resolve_fixed_session(&discovered, fixed)?]);
    }
    if discovered.is_empty() {
        bail!("no Wayland sessions discovered");
    }
    Ok(discovered)
}

fn resolve_fixed_session(discovered: &[Session], fixed: &str) -> Result<Session> {
    if let Some(session) = discovered.iter().find(|session| session.id == fixed) {
        return Ok(session.clone());
    }

    let mut matches = discovered.iter()
        .filter(|session| session.exec == fixed || session.name == fixed);
    let Some(session) = matches.next() else {
        return Ok(Session::new(fixed, fixed, fixed, "<fixed>"));
    };
    if let Some(other) = matches.next() {
        bail!(
            "dm.fixed_session {fixed:?} matches multiple desktop entries ({}, {}); use a desktop-file id",
            session.id, other.id,
        );
    }
    Ok(session.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_id_precedes_other_entries_names_and_commands() {
        let target = Session::new("target", "Zebra", "target-session", "target.desktop");
        let by_name = Session::new("alias-name", "target", "name-session", "name.desktop");
        let by_exec = Session::new("alias-exec", "Alpha", "target", "exec.desktop");
        for sessions in [
            vec![by_name.clone(), by_exec.clone(), target.clone()],
            vec![target.clone(), by_exec, by_name],
        ] {
            assert_eq!(resolve_fixed_session(&sessions, "target").unwrap(), target);
        }
    }

    #[test]
    fn duplicate_names_require_a_desktop_id() {
        let sessions = vec![
            Session::new("first", "Desktop", "first-session", "first.desktop"),
            Session::new("second", "Desktop", "second-session", "second.desktop"),
        ];
        let error = resolve_fixed_session(&sessions, "Desktop").unwrap_err().to_string();
        assert!(error.contains("use a desktop-file id"));
        assert!(error.contains("first"));
        assert!(error.contains("second"));
        assert_eq!(resolve_fixed_session(&sessions, "second").unwrap(), sessions[1]);
    }

    #[test]
    fn duplicate_exec_and_name_exec_collisions_are_ambiguous() {
        let shared_exec = vec![
            Session::new("first", "First", "shared-session", "first.desktop"),
            Session::new("second", "Second", "shared-session", "second.desktop"),
        ];
        assert!(resolve_fixed_session(&shared_exec, "shared-session").is_err());

        let mixed = vec![
            Session::new("first", "shared-session", "first-session", "first.desktop"),
            shared_exec[1].clone(),
        ];
        assert!(resolve_fixed_session(&mixed, "shared-session").is_err());
    }

    #[test]
    fn unique_alias_preserves_desktop_metadata() {
        let mut session = Session::new("desktop", "Desktop", "desktop-session", "desktop.desktop");
        session.desktop_names = vec!["Example".into()];
        session.comment = Some("Example desktop".into());
        for alias in ["Desktop", "desktop-session"] {
            assert_eq!(resolve_fixed_session(&[session.clone()], alias).unwrap(), session);
        }
    }

    #[test]
    fn unknown_value_remains_a_literal_command() {
        let command = "/usr/bin/compositor --flag";
        assert_eq!(
            resolve_fixed_session(&[], command).unwrap(),
            Session::new(command, command, command, "<fixed>"),
        );
    }
}
