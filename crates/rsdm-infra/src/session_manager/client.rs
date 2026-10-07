//! CLI requests and the basic launch path outside an RSDM-coordinated login.

use std::{path::Path, thread, time::{Duration, Instant}};

use rsdm_core::domain::{SessionManagerConfig, ShutdownPolicy};

use super::{
    SessionError, bus::UserManager, control::{self, LaunchRequest, SessionStatus, StopOutcome}, env,
    identity::{GENERATION_ENV, valid_generation}, unit_name::{ManagedUnitKind, unique_unit_name}, units,
};

pub fn generation() -> Result<String, SessionError> {
    let generation = std::env::var(GENERATION_ENV)
        .map_err(|_| SessionError::State("this command requires an RSDM-coordinated session".into()))?;
    if !valid_generation(&generation) { return Err(SessionError::State("invalid session generation".into())); }
    Ok(generation)
}

pub fn run_app(argv: &[String], policy: &ShutdownPolicy) -> Result<i32, SessionError> {
    policy.validate().map_err(|error| SessionError::State(error.into()))?;
    if std::env::var_os(GENERATION_ENV).is_none() { return basic_launch(argv, policy); }
    let generation = generation()?;
    let request = LaunchRequest {
        generation, argv: argv.to_vec(), environment: env::command_environment(),
        directory: std::env::current_dir()?.to_string_lossy().into_owned(),
        timeout_secs: policy.timeout_secs, on_timeout: policy.on_timeout.to_string(),
        method: policy.method.to_string(), quit_command: policy.quit_command.clone(),
    };
    control::validate_launch(&request).map_err(SessionError::State)?;
    let connection = zbus::blocking::connection::Builder::session()?.method_timeout(Duration::from_secs(25)).build()?;
    let _: String = control::proxy(&connection)?.call("Launch", &(request,))?;
    Ok(0)
}

pub fn finalize(cfg: &SessionManagerConfig, names: &[String]) -> Result<(), SessionError> {
    let generation = generation()?;
    let pairs = env::collect_present(&cfg.extra_env, names);
    control::validate_finalize(&generation, &pairs).map_err(SessionError::State)?;
    let connection = zbus::blocking::connection::Builder::session()?.method_timeout(Duration::from_secs(25)).build()?;
    control::proxy(&connection)?.call::<_, _, ()>("Finalize", &(generation, pairs))?;
    Ok(())
}

pub fn status() -> Result<SessionStatus, SessionError> {
    let connection = zbus::blocking::connection::Builder::session()?.method_timeout(Duration::from_secs(5)).build()?;
    Ok(control::proxy(&connection)?.call("Status", &())?)
}

pub fn stop(action: &str) -> Result<StopOutcome, SessionError> {
    let generation = generation()?;
    let status = status()?;
    if status.generation != generation { return Err(SessionError::State("session generation is stale".into())); }
    let longest = status.apps.iter().map(|(_, _, timeout, _)| *timeout).max().unwrap_or(30);
    let connection = zbus::blocking::connection::Builder::session()?
        .method_timeout(Duration::from_secs(longest.saturating_add(90))).build()?;
    Ok(control::proxy(&connection)?.call("Stop", &(generation, action))?)
}

pub fn cancel() -> Result<(), SessionError> {
    let generation = generation()?;
    let connection = zbus::blocking::connection::Builder::session()?.method_timeout(Duration::from_secs(5)).build()?;
    control::proxy(&connection)?.call::<_, _, ()>("Cancel", &(generation,))?;
    Ok(())
}

fn basic_launch(argv: &[String], policy: &ShutdownPolicy) -> Result<i32, SessionError> {
    if *policy != ShutdownPolicy::default() {
        return Err(SessionError::State("shutdown policy requires an RSDM-coordinated session".into()));
    }
    let program = argv.first().ok_or(SessionError::EmptyCommand)?;
    let manager = UserManager::connect()?;
    let deadline = Instant::now() + Duration::from_secs(10);
    while !manager.active(units::SESSION_TARGET)? {
        if Instant::now() >= deadline { return Err(SessionError::NoGraphicalSession); }
        thread::sleep(Duration::from_millis(200));
    }
    let unit = format!("{}.service", unique_unit_name(ManagedUnitKind::App, program));
    let mut properties = units::app_properties(argv, &env::command_environment(), Path::new(&std::env::current_dir()?), None)?;
    properties.retain(|(name, _)| *name != "AddRef");
    manager.start_service(&unit, &properties)?;
    Ok(0)
}
