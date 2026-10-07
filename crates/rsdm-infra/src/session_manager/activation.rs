//! Readiness and graphical target ownership for a single session generation.

use std::{path::Path, sync::atomic::{AtomicBool, Ordering}};

use super::{SessionError, bus::UserManager, env, provider::{Provider, ProviderKind}, runtime::{Runtime, SessionRecord}, units};

pub(super) fn activate(
    manager: &UserManager, provider: &Provider, anchor: &str, compositor: Option<&str>,
    environment: &[(String, String)], directory: &Path, stopping: &AtomicBool,
) -> Result<bool, SessionError> {
    if stopping.load(Ordering::SeqCst) { return Ok(false); }
    let external_target = manager.active(units::SESSION_TARGET)?;
    if provider.kind != ProviderKind::Managed && (!external_target || !provider.ready(manager)?) {
        return Err(SessionError::State("native session manager is not ready".into()));
    }
    let mut properties = units::anchor_properties(compositor, environment, directory)?;
    if provider.kind != ProviderKind::Managed {
        // A native target can stop between readiness and the anchor start job.
        // Requisite refuses that race; Wants would restart someone else's session.
        properties.retain(|(name, _)| !matches!(*name, "Wants" | "Requisite"));
        let mut required = vec![units::SESSION_TARGET.to_string()];
        if let Some(unit) = compositor {
            required.push(unit.to_string());
        }
        properties.push(("Requisite", zbus::zvariant::Value::new(required)));
    }
    if !external_target && provider.kind == ProviderKind::Managed {
        properties.retain(|(name, _)| *name != "Wants");
        properties.push(("Wants", zbus::zvariant::Value::new(vec![
            units::SESSION_TARGET.to_string(), units::AUTOSTART_TARGET.to_string(),
        ])));
    }
    if stopping.load(Ordering::SeqCst) { return Ok(false); }
    manager.start_service(anchor, &properties)?;
    Ok(!external_target && provider.kind == ProviderKind::Managed)
}

pub(super) fn published_display(manager: &UserManager) -> Result<bool, SessionError> {
    let current = manager.environment()?.into_iter().find(|entry| entry.starts_with("WAYLAND_DISPLAY="));
    // Startup clears the previous display before the compositor is launched.
    Ok(current.is_some_and(|value| value != "WAYLAND_DISPLAY="))
}

pub(super) fn export(
    manager: &UserManager, runtime: &Runtime, record: &mut SessionRecord, pairs: Vec<(String, String)>,
) -> Result<(), SessionError> {
    if pairs.iter().any(|(name, value)| !env::valid_environment_name(name) || value.contains('\0')) {
        return Err(SessionError::State("invalid session environment".into()));
    }
    for (name, value) in &pairs {
        if let Some((_, old)) = record.exported_environment.iter_mut().find(|(key, _)| key == name) { *old = value.clone(); }
        else { record.exported_environment.push((name.clone(), value.clone())); }
    }
    // SetEnvironment can succeed before activation-environment publication fails.
    runtime.save_session(record)?;
    env::publish(manager, &pairs)
}
