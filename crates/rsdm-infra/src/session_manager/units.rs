//! Transient service definitions and executable lookup in the caller's PATH.

use std::{os::unix::fs::PermissionsExt, path::{Path, PathBuf}};

use zbus::zvariant::Value;

use super::{SessionError, bus::UnitProperties, env::DISPLAY_VARS};

pub(super) const APP_SLICE: &str = "app-graphical.slice";
pub(super) const PRE_TARGET: &str = "graphical-session-pre.target";
pub(super) const SESSION_TARGET: &str = "graphical-session.target";
pub(super) const AUTOSTART_TARGET: &str = "xdg-desktop-autostart.target";

pub(super) fn command_properties(
    argv: &[String],
    environment: &[(String, String)],
    directory: &Path,
) -> Result<UnitProperties, SessionError> {
    let program = argv.first().ok_or(SessionError::EmptyCommand)?;
    let executable = resolve_executable(program, environment, directory)?;
    let command = vec![(executable.to_string_lossy().into_owned(), argv.to_vec(), vec!["no-env-expand".to_string()])];
    let values: Vec<String> = environment.iter().map(|(name, value)| format!("{name}={value}")).collect();
    let absent: Vec<String> = DISPLAY_VARS.iter()
        .filter(|name| !environment.iter().any(|(key, _)| key == **name))
        .map(|name| (*name).to_string()).collect();
    Ok(vec![
        ("Type", Value::from("exec")),
        ("TimeoutStartUSec", Value::from(10_000_000_u64)),
        ("ExecStartEx", Value::new(command)),
        ("Environment", Value::new(values)),
        ("UnsetEnvironment", Value::new(absent)),
        ("WorkingDirectory", Value::new(directory.to_string_lossy().into_owned())),
        ("KillMode", Value::from("control-group")),
        ("CollectMode", Value::from("inactive-or-failed")),
        ("AddRef", Value::from(true)),
    ])
}

pub(super) fn compositor_properties(
    argv: &[String], environment: &[(String, String)], directory: &Path, anchor: &str,
) -> Result<UnitProperties, SessionError> {
    let mut properties = command_properties(argv, environment, directory)?;
    properties.extend([
        ("Slice", Value::from("session.slice")),
        ("ExitType", Value::from("main")),
        ("PartOf", Value::new(vec![SESSION_TARGET.to_string(), anchor.to_string()])),
        ("Wants", Value::new(vec![PRE_TARGET.to_string()])),
        ("After", Value::new(vec![PRE_TARGET.to_string()])),
        ("Before", Value::new(vec![SESSION_TARGET.to_string(), AUTOSTART_TARGET.to_string()])),
    ]);
    Ok(properties)
}

pub(super) fn app_properties(
    argv: &[String], environment: &[(String, String)], directory: &Path, anchor: Option<&str>,
) -> Result<UnitProperties, SessionError> {
    let mut properties = command_properties(argv, environment, directory)?;
    let mut lifetime = vec![SESSION_TARGET.to_string()];
    if let Some(anchor) = anchor {
        lifetime.push(anchor.to_string());
    }
    properties.extend([
        ("Slice", Value::from(APP_SLICE)),
        ("ExitType", Value::from("cgroup")),
        ("PartOf", Value::new(lifetime.clone())),
        ("After", Value::new(lifetime.clone())),
        ("Requisite", Value::new(lifetime)),
    ]);
    Ok(properties)
}

pub(super) fn anchor_properties(
    compositor: Option<&str>, environment: &[(String, String)], directory: &Path,
) -> Result<UnitProperties, SessionError> {
    let mut properties = command_properties(&["true".into()], environment, directory)?;
    properties.retain(|(name, _)| *name != "Type");
    let mut after = vec![SESSION_TARGET.to_string()];
    let mut part_of = vec![SESSION_TARGET.to_string()];
    if let Some(unit) = compositor {
        after.push(unit.to_string());
        part_of.push(unit.to_string());
        // Requires/BindsTo could restart a compositor that exited during readiness.
        properties.push(("Requisite", Value::new(vec![unit.to_string()])));
    }
    properties.extend([
        ("Type", Value::from("oneshot")),
        ("RemainAfterExit", Value::from(true)),
        ("PartOf", Value::new(part_of)),
        ("Wants", Value::new(vec![SESSION_TARGET.to_string()])),
        ("After", Value::new(after)),
    ]);
    Ok(properties)
}

fn resolve_executable(program: &str, environment: &[(String, String)], directory: &Path) -> Result<PathBuf, SessionError> {
    let path = Path::new(program);
    let candidates: Vec<PathBuf> = if path.is_absolute() {
        vec![path.to_path_buf()]
    } else if program.contains('/') {
        vec![directory.join(path)]
    } else {
        let search_path = environment.iter().find(|(name, _)| name == "PATH")
            .ok_or_else(|| SessionError::State("the launch environment has no PATH".into()))?;
        std::env::split_paths(&search_path.1).map(|entry| {
            if entry.is_absolute() { entry.join(path) } else { directory.join(entry).join(path) }
        }).collect()
    };
    for candidate in candidates {
        if candidate.metadata().is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0) {
            return Ok(candidate);
        }
    }
    Err(SessionError::State(format!("executable {program:?} was not found in the launch environment")))
}

#[cfg(test)]
#[path = "units_tests.rs"]
mod tests;
