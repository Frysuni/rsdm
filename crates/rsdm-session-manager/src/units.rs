//! Transient service definitions and executable lookup in the caller's PATH.

use std::{os::unix::fs::PermissionsExt, path::{Path, PathBuf}};

use zbus::Variant;

use super::{SessionError, bus::UnitProperties, env::DISPLAY_VARS};

pub(super) const APP_SLICE: &str = "app-graphical.slice";
pub(super) const PRE_TARGET: &str = "graphical-session-pre.target";
pub(super) const SESSION_TARGET: &str = "graphical-session.target";
pub(super) const AUTOSTART_TARGET: &str = "xdg-desktop-autostart.target";

pub(super) async fn command_properties(
    argv: &[String],
    environment: &[(String, String)],
    directory: &Path,
) -> Result<UnitProperties, SessionError> {
    let program = argv.first().ok_or(SessionError::EmptyCommand)?.clone();
    let search_environment = environment.to_vec();
    let search_directory = directory.to_path_buf();
    let executable = super::runtime::blocking(move || {
        resolve_executable(&program, &search_environment, &search_directory)
    }).await?;

    let command = vec![(executable.to_string_lossy().into_owned(), argv.to_vec(), vec!["no-env-expand".to_string()])];
    let values: Vec<String> = environment.iter().map(|(name, value)| format!("{name}={value}")).collect();
    let absent: Vec<String> = DISPLAY_VARS.iter()
        .filter(|name| !environment.iter().any(|(key, _)| key == **name))
        .map(|name| (*name).to_string()).collect();
    Ok(vec![
        ("Type", Variant::from("exec")),
        ("TimeoutStartUSec", Variant::from(10_000_000_u64)),
        ("ExecStartEx", Variant::new(command)),
        ("Environment", Variant::new(values)),
        ("UnsetEnvironment", Variant::new(absent)),
        ("WorkingDirectory", Variant::new(directory.to_string_lossy().into_owned())),
        ("KillMode", Variant::from("control-group")),
        ("CollectMode", Variant::from("inactive-or-failed")),
        ("AddRef", Variant::from(true)),
    ])
}

pub(super) async fn compositor_properties(
    argv: &[String], environment: &[(String, String)], directory: &Path, anchor: &str,
) -> Result<UnitProperties, SessionError> {
    let mut properties = command_properties(argv, environment, directory).await?;
    properties.extend([
        ("Slice", Variant::from("session.slice")),
        ("ExitType", Variant::from("main")),
        ("PartOf", Variant::new(vec![SESSION_TARGET.to_string(), anchor.to_string()])),
        ("Wants", Variant::new(vec![PRE_TARGET.to_string()])),
        ("After", Variant::new(vec![PRE_TARGET.to_string()])),
        ("Before", Variant::new(vec![SESSION_TARGET.to_string(), AUTOSTART_TARGET.to_string()])),
    ]);
    Ok(properties)
}

pub(super) async fn app_properties(
    argv: &[String], environment: &[(String, String)], directory: &Path, anchor: Option<&str>,
) -> Result<UnitProperties, SessionError> {
    let mut properties = command_properties(argv, environment, directory).await?;
    let mut lifetime = vec![SESSION_TARGET.to_string()];
    if let Some(anchor) = anchor {
        lifetime.push(anchor.to_string());
    }
    properties.extend([
        ("Slice", Variant::from(APP_SLICE)),
        ("ExitType", Variant::from("cgroup")),
        ("PartOf", Variant::new(lifetime.clone())),
        ("After", Variant::new(lifetime.clone())),
        ("Requisite", Variant::new(lifetime)),
    ]);
    Ok(properties)
}

pub(super) async fn anchor_properties(
    compositor: Option<&str>, environment: &[(String, String)], directory: &Path,
) -> Result<UnitProperties, SessionError> {
    let mut properties = command_properties(&["true".into()], environment, directory).await?;
    properties.retain(|(name, _)| *name != "Type");
    let mut after = vec![SESSION_TARGET.to_string()];
    let mut part_of = vec![SESSION_TARGET.to_string()];
    if let Some(unit) = compositor {
        after.push(unit.to_string());
        part_of.push(unit.to_string());
        // Requires/BindsTo could restart a compositor that exited during readiness.
        properties.push(("Requisite", Variant::new(vec![unit.to_string()])));
    }
    properties.extend([
        ("Type", Variant::from("oneshot")),
        ("RemainAfterExit", Variant::from(true)),
        ("PartOf", Variant::new(part_of)),
        ("Wants", Variant::new(vec![SESSION_TARGET.to_string()])),
        ("After", Variant::new(after)),
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
