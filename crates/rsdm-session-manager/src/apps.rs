//! Registration precedes service creation so shutdown can drain accepted launches.

use std::{path::Path, time::Duration};

use rsdm_core::domain::{ShutdownMethod, ShutdownPolicy, TimeoutAction};
use zbus::zvariant::Value;

use super::{
    SessionError, bus::{UnitProperties, UserManager}, control::LaunchRequest, deadline::Deadline, env::valid_environment_name,
    identity::GENERATION_ENV, provider::Provider,
    runtime::{blocking, AppRecord, Runtime}, unit_name::app_unit_name, units::app_properties,
};

pub(super) async fn register(
    runtime: &Runtime, provider: &Provider, request: &LaunchRequest,
) -> Result<AppRecord, SessionError> {
    super::control::validate_launch(request).map_err(SessionError::State)?;
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
        || !matches!(provider.kind, super::provider::ProviderKind::Managed | super::provider::ProviderKind::Niri | super::provider::ProviderKind::Attached))
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
        policy, deadline_usec: None, quit_started: false, dispatch: None, window_close_sent: false,
    };
    let runtime = runtime.clone();
    blocking(move || {
        runtime.save_app(&app)?;
        Ok(app)
    }).await
}

pub(super) async fn launch(
    manager: &UserManager, runtime: &Runtime, anchor: &str, request: &LaunchRequest, mut app: AppRecord,
    config_path: Option<&Path>,
) -> Result<String, SessionError> {
    let properties = match launch_properties(runtime, anchor, request, &app, config_path).await {
        Ok(properties) => properties,
        Err(error) => return Err(rollback_launch(manager, runtime, &app, error).await),
    };
    let started = manager.start_service(&app.unit, &properties).await;
    if let Err(error @ SessionError::StartRejected { .. }) = started {
        return Err(rollback_launch(manager, runtime, &app, error).await);
    }
    let invocation = super::processes::generation_invocation(manager, &app.unit, &runtime.generation).await
        .and_then(|id| id.ok_or_else(|| SessionError::State("application invocation is unavailable after start".into())));
    match invocation {
        Ok(id) => {
            let runtime = runtime.clone();
            let deadline = manager.deadline.clone();
            let unit = app.unit.clone();
            app = blocking(move || {
                let _lease = runtime.app_lease_until(&unit, &deadline)?;
                let mut app = runtime.app(&unit)?;
                app.invocation_id = id;
                runtime.save_app(&app)?;
                Ok(app)
            }).await?;
        }
        Err(error) => {
            if started.is_ok() { return Err(error); }
            // A timed-out start job can still be running. Preserve its record.
            tracing::warn!(unit = app.unit, %error, "could not capture application invocation");
        }
    }
    started?;
    Ok(app.unit)
}

async fn launch_properties(
    runtime: &Runtime, anchor: &str, request: &LaunchRequest, app: &AppRecord,
    config_path: Option<&Path>,
) -> Result<UnitProperties, SessionError> {
    let mut environment = request.environment.clone();
    environment.retain(|(name, _)| name != GENERATION_ENV);
    environment.push((GENERATION_ENV.into(), runtime.generation.clone()));
    let records = runtime.clone();
    let record = blocking(move || records.session()).await?;
    if record.provider == "partial" {
        for (name, value) in &record.exported_environment {
            if environment.iter().any(|(existing, _)| existing == name)
                && !matches!(name.as_str(), "SESSION_MANAGER" | "ICEAUTHORITY" | "RSDM_SESSION_GENERATION" | "XDG_SESSION_ID")
            { continue; }
            environment.retain(|(existing, _)| existing != name);
            environment.push((name.clone(), value.clone()));
        }
    }
    let mut properties = app_properties(&request.argv, &environment, Path::new(&request.directory), Some(anchor)).await?;
    if record.provider == "partial" {
        properties.retain(|(name, _)| !matches!(*name, "PartOf" | "After" | "Requisite"));
    }
    let executable = blocking(|| Ok(std::env::current_exe()?)).await?.to_string_lossy().into_owned();
    let mut stop_argv = vec![executable.clone()];
    if let Some(path) = config_path {
        let path = path.to_str().ok_or_else(|| SessionError::State("hook configuration path is not valid UTF-8".into()))?;
        stop_argv.extend(["--config".into(), path.into()]);
    }
    stop_argv.extend(["session".into(), "app-stop".into(), "--generation".into(), runtime.generation.clone(), "--unit".into(), app.unit.clone()]);
    let stop_command = vec![(executable, stop_argv, vec!["no-env-expand".to_string()])];
    properties.extend([
        ("ExecStopEx", Value::new(stop_command)),
        ("TimeoutStopUSec", Value::from(app.policy.timeout_secs.saturating_add(20).saturating_mul(1_000_000))),
    ]);
    Ok(properties)
}

async fn rollback_launch(manager: &UserManager, runtime: &Runtime, app: &AppRecord, error: SessionError) -> SessionError {
    if matches!(error, SessionError::StartRejected { reference_retained: true, .. }) { return error; }
    let runtime = runtime.clone();
    let app = app.clone();
    let deadline = manager.deadline.clone();
    let rollback = blocking(move || {
        let _lease = runtime.app_lease_until(&app.unit, &deadline)?;
        let current = match runtime.app(&app.unit) {
            Ok(current) => current,
            Err(SessionError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        if !current.invocation_id.is_empty() || current.policy != app.policy {
            return Err(SessionError::State("application registration changed before rollback".into()));
        }
        runtime.remove_app(&app.unit)
    }).await;
    match rollback {
        Ok(()) => error,
        Err(cleanup) => SessionError::State(format!("{error}; could not roll back application registration: {cleanup}")),
    }
}

pub(super) async fn release_closed(manager: &UserManager, runtime: &Runtime) -> Result<(), SessionError> {
    let records = runtime.clone();
    let apps = blocking(move || records.apps()).await?;
    for app in apps {
        if app.invocation_id.is_empty() {
            continue;
        }
        let state = manager.unit_property::<String>(&app.unit, "org.freedesktop.systemd1.Unit", "ActiveState").await;
        let stopped = match state {
            Ok(state) => matches!(state.as_str(), "inactive" | "failed"),
            Err(SessionError::Bus(error)) if super::bus::missing_unit(&error) => true,
            Err(error) => return Err(error),
        };
        if stopped && super::processes::app_processes(manager, &app).await?.is_empty() {
            let _ = manager.unref(&app.unit).await;
            let records = runtime.clone();
            blocking(move || records.remove_app(&app.unit)).await?;
        }
    }
    Ok(())
}

pub(super) async fn reset_preparation(runtime: &Runtime) -> Result<(), SessionError> {
    let runtime = runtime.clone();
    blocking(move || {
        for record in runtime.apps()? {
            let _lease = runtime.app_lease(&record.unit)?;
            let mut app = runtime.app(&record.unit)?;
            app.deadline_usec = None;
            super::app_dispatch::cancel(&mut app);
            runtime.save_app(&app)?;
        }
        Ok(())
    }).await
}

pub async fn stop_hook(generation: &str, unit: &str, actions: super::action_gate::ActionGate) -> Result<(), SessionError> {
    let generation = generation.to_string();
    let Some((runtime, saved, transaction_id)) = blocking(move || {
        let runtime = Runtime::open(&generation)?.ok_or_else(|| SessionError::State("session recovery record is missing".into()))?;
        if runtime.is_disabled()? { return Ok(None); }
        let record = runtime.session()?;
        if record.disabled { return Ok(None); }
        Ok(Some((runtime, record.shutdown_deadline_usec, record.shutdown_transaction.map(|transaction| transaction.id))))
    }).await? else { return Ok(()); };
    let actions = actions.with_generation(runtime.clone());
    let deadline = Deadline::default();
    if let Some(saved) = saved { deadline.set(saved); }
    let manager = match UserManager::connect_guarded(deadline, actions.clone()).await {
        Ok(manager) => manager,
        Err(_) if actions.check().is_err() => return Ok(()),
        Err(error) => return Err(error),
    };
    let mut control = super::app_stop::ShutdownControl::for_manager(&manager);
    control.transaction_id = transaction_id;
    control.noncancelable.store(true, std::sync::atomic::Ordering::SeqCst);
    let result = async {
        pin_pending(&manager, &runtime, unit).await?;
        super::app_stop::prepare_app(&manager, &runtime, unit, &control).await?;
        Ok(())
    }.await;
    if actions.check().is_err() { return Ok(()); }
    result
}

pub(super) async fn pin_pending(manager: &UserManager, runtime: &Runtime, unit: &str) -> Result<bool, SessionError> {
    let records = runtime.clone();
    let app_unit = unit.to_string();
    if !blocking(move || records.app(&app_unit)).await?.invocation_id.is_empty() {
        return Ok(true);
    }
    let Some(id) = super::processes::generation_invocation(manager, unit, &runtime.generation).await? else {
        return Ok(false);
    };
    let runtime = runtime.clone();
    let unit = unit.to_string();
    let deadline = manager.deadline.clone();
    blocking(move || {
        let _lease = runtime.app_lease_until(&unit, &deadline)?;
        let mut app = runtime.app(&unit)?;
        if app.invocation_id.is_empty() {
            app.invocation_id = id;
            runtime.save_app(&app)?;
        }
        Ok(true)
    }).await
}

pub(super) async fn finish(manager: &UserManager, runtime: &Runtime, app: &AppRecord) -> Result<(), SessionError> {
    if pin_pending(manager, runtime, &app.unit).await? {
        let records = runtime.clone();
        let unit = app.unit.clone();
        let current = blocking(move || records.app(&unit)).await?;
        if super::processes::verify_invocation(manager, &current).await? {
            manager.stop(&app.unit, Duration::from_secs(10)).await?;
            manager.unref(&app.unit).await?;
        }
    }
    let runtime = runtime.clone();
    let unit = app.unit.clone();
    blocking(move || runtime.remove_app(&unit)).await
}

#[cfg(test)]
#[path = "apps_launch_tests.rs"]
mod launch_tests;
