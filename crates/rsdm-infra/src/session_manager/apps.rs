//! Registration precedes service creation so shutdown can drain accepted launches.

use std::{path::Path, time::Duration};

use rsdm_core::domain::{ShutdownMethod, ShutdownPolicy, TimeoutAction};
use zbus::zvariant::Value;

use super::{
    SessionError, bus::UserManager, control::LaunchRequest, env::valid_environment_name,
    identity::GENERATION_ENV, provider::Provider,
    runtime::{AppRecord, Runtime}, unit_name::app_unit_name, units::app_properties,
};

pub(super) fn register(
    runtime: &Runtime, provider: &Provider, request: &LaunchRequest,
) -> Result<AppRecord, SessionError> {
    if request.generation != runtime.generation {
        return Err(SessionError::State("session generation mismatch".into()));
    }
    let program = request.argv.first().ok_or(SessionError::EmptyCommand)?;
    if !Path::new(&request.directory).is_absolute() {
        return Err(SessionError::State("application directory must be absolute".into()));
    }
    if request.environment.iter().any(|(name, value)| !valid_environment_name(name) || value.contains('\0')) {
        return Err(SessionError::State("invalid application environment".into()));
    }
    let policy = ShutdownPolicy {
        timeout_secs: request.timeout_secs,
        on_timeout: request.on_timeout.parse().map_err(|error: &'static str| SessionError::State(error.into()))?,
        method: request.method.parse().map_err(|error: &'static str| SessionError::State(error.into()))?,
        quit_command: request.quit_command.clone(),
    };
    policy.validate().map_err(|error| SessionError::State(error.into()))?;
    if policy.method == ShutdownMethod::Xsmp && (!cfg!(feature = "xsmp")
        || !matches!(provider.kind, super::provider::ProviderKind::Managed | super::provider::ProviderKind::Niri))
    {
        return Err(SessionError::State("explicit XSMP requires an XSMP-enabled build and an RSDM-owned XSMP server".into()));
    }
    if provider.native_desktop() && policy.on_timeout == TimeoutAction::Cancel
        && policy.method == ShutdownMethod::Auto && policy.quit_command.is_empty()
    {
        return Err(SessionError::State("native desktop shutdown owns cancellation; use a quit command for an RSDM preflight".into()));
    }
    let app = AppRecord {
        unit: app_unit_name(program, &runtime.generation), invocation_id: Vec::new(),
        policy, deadline_usec: None, quit_started: false,
    };
    runtime.save_app(&app)?;
    Ok(app)
}

pub(super) fn launch(
    manager: &UserManager, runtime: &Runtime, anchor: &str, request: &LaunchRequest, mut app: AppRecord,
) -> Result<String, SessionError> {
    let mut environment = request.environment.clone();
    environment.retain(|(name, _)| name != GENERATION_ENV);
    environment.push((GENERATION_ENV.into(), runtime.generation.clone()));
    let mut properties = app_properties(&request.argv, &environment, Path::new(&request.directory), Some(anchor))?;
    let executable = std::env::current_exe()?.to_string_lossy().into_owned();
    let stop_command = vec![(executable.clone(), vec![executable, "session".into(), "app-stop".into(),
        "--generation".into(), runtime.generation.clone(), "--unit".into(), app.unit.clone()], vec!["no-env-expand".to_string()])];
    properties.extend([
        ("ExecStopEx", Value::new(stop_command)),
        ("TimeoutStopUSec", Value::from(app.policy.timeout_secs.saturating_add(20).saturating_mul(1_000_000))),
    ]);
    let started = manager.start_service(&app.unit, &properties);
    match manager.invocation_id(&app.unit) {
        Ok(id) => {
            let _lease = runtime.app_lease(&app.unit)?;
            app = runtime.app(&app.unit)?;
            app.invocation_id = id;
            runtime.save_app(&app)?;
        }
        Err(error) => {
            if started.is_ok() {
                return Err(error);
            }
            // A timed-out start job can still be running. Preserve its record.
            tracing::warn!(unit = app.unit, %error, "could not capture application invocation");
        }
    }
    started?;
    Ok(app.unit)
}

pub(super) fn release_closed(manager: &UserManager, runtime: &Runtime) -> Result<(), SessionError> {
    for app in runtime.apps()? {
        if app.invocation_id.is_empty() {
            continue;
        }
        let state = manager.unit_property::<String>(&app.unit, "org.freedesktop.systemd1.Unit", "ActiveState");
        let stopped = match state {
            Ok(state) => matches!(state.as_str(), "inactive" | "failed"),
            Err(SessionError::Bus(error)) if super::bus::missing_unit(&error) => true,
            Err(error) => return Err(error),
        };
        if stopped && super::processes::app_processes(manager, &app)?.is_empty() {
            let _ = manager.unref(&app.unit);
            runtime.remove_app(&app.unit)?;
        }
    }
    Ok(())
}

pub(super) fn reset_preparation(runtime: &Runtime) -> Result<(), SessionError> {
    for record in runtime.apps()? {
        let _lease = runtime.app_lease(&record.unit)?;
        let mut app = runtime.app(&record.unit)?;
        app.deadline_usec = None;
        app.quit_started = false;
        runtime.save_app(&app)?;
    }
    Ok(())
}

pub fn stop_hook(generation: &str, unit: &str) -> Result<(), SessionError> {
    let runtime = Runtime::open(generation)?.ok_or_else(|| SessionError::State("session recovery record is missing".into()))?;
    let manager = UserManager::connect()?;
    pin_pending(&manager, &runtime, unit)?;
    let control = super::app_stop::ShutdownControl::default();
    control.noncancelable.store(true, std::sync::atomic::Ordering::SeqCst);
    if let Some(deadline) = runtime.session()?.shutdown_deadline_usec {
        control.force(deadline);
    }
    super::app_stop::prepare_app(&manager, &runtime, unit, &control)?;
    Ok(())
}

pub(super) fn pin_pending(manager: &UserManager, runtime: &Runtime, unit: &str) -> Result<bool, SessionError> {
    let _lease = runtime.app_lease(unit)?;
    let mut app = runtime.app(unit)?;
    if !app.invocation_id.is_empty() {
        return Ok(true);
    }
    let Some(id) = super::processes::generation_invocation(manager, unit, &runtime.generation)? else {
        return Ok(false);
    };
    app.invocation_id = id;
    runtime.save_app(&app)?;
    Ok(true)
}

pub(super) fn finish(manager: &UserManager, runtime: &Runtime, app: &AppRecord) -> Result<(), SessionError> {
    if pin_pending(manager, runtime, &app.unit)?
        && super::processes::verify_invocation(manager, &runtime.app(&app.unit)?)?
    {
        manager.stop(&app.unit, Duration::from_secs(10))?;
        manager.unref(&app.unit)?;
    }
    runtime.remove_app(&app.unit)
}
