//! CLI requests and the basic launch path outside an RSDM-coordinated login.

use std::{path::Path, thread, time::{Duration, Instant}};

use rsdm_core::domain::{SessionManagerConfig, ShutdownPolicy};

use super::{
    SessionError, bus::UserManager, control::{self, LaunchRequest, SessionStatus, StopOutcome}, env,
    identity::{GENERATION_ENV, valid_generation}, unit_name::{ManagedUnitKind, unique_unit_name}, units,
};

pub async fn generation() -> Result<String, SessionError> {
    let generation = std::env::var(GENERATION_ENV)
        .map_err(|_| SessionError::State("this command requires an RSDM-coordinated session".into()))?;
    if !valid_generation(&generation) { return Err(SessionError::State("invalid session generation".into())); }
    Ok(generation)
}

pub async fn run_app(argv: &[String], policy: &ShutdownPolicy) -> Result<i32, SessionError> {
    policy.validate().map_err(|error| SessionError::State(error.into()))?;
    if std::env::var_os(GENERATION_ENV).is_none() {
        let argv = argv.to_vec();
        let policy = policy.clone();
        return tokio::task::spawn_blocking(move || basic_launch(&argv, &policy)).await
            .map_err(|error| SessionError::State(format!("application launch worker failed: {error}")))?;
    }
    let generation = generation().await?;
    let request = LaunchRequest {
        generation, argv: argv.to_vec(), environment: env::command_environment(),
        directory: std::env::current_dir()?.to_string_lossy().into_owned(),
        timeout_secs: policy.timeout_secs, on_timeout: policy.on_timeout.to_string(),
        method: policy.method.to_string(), quit_command: policy.quit_command.clone(),
    };
    control::validate_launch(&request).map_err(SessionError::State)?;
    let connection = zbus::connection::Builder::session()?.method_timeout(Duration::from_secs(25)).build().await?;
    let _: String = control::proxy(&connection).await?.call("Launch", &(request,)).await?;
    Ok(0)
}

pub async fn finalize(cfg: &SessionManagerConfig, names: &[String]) -> Result<(), SessionError> {
    let generation = generation().await?;
    let pairs = env::collect_present(&cfg.extra_env, names);
    control::validate_finalize(&generation, &pairs).map_err(SessionError::State)?;
    let connection = zbus::connection::Builder::session()?.method_timeout(Duration::from_secs(25)).build().await?;
    control::proxy(&connection).await?.call::<_, _, ()>("Finalize", &(generation, pairs)).await?;
    Ok(())
}

pub async fn status() -> Result<SessionStatus, SessionError> {
    let connection = zbus::connection::Builder::session()?.method_timeout(Duration::from_secs(5)).build().await?;
    Ok(control::proxy(&connection).await?.call("Status", &()).await?)
}

pub async fn stop(action: &str) -> Result<StopOutcome, SessionError> {
    let generation = generation().await?;
    let status = status().await?;
    if status.generation != generation { return Err(SessionError::State("session generation is stale".into())); }
    let longest = status.apps.iter().map(|(_, _, timeout, _)| *timeout).max().unwrap_or(30);
    let connection = zbus::connection::Builder::session()?
        .method_timeout(Duration::from_secs(longest.saturating_add(90))).build().await?;
    Ok(control::proxy(&connection).await?.call("Stop", &(generation, action)).await?)
}

pub async fn cancel() -> Result<(), SessionError> {
    let generation = generation().await?;
    let connection = zbus::connection::Builder::session()?.method_timeout(Duration::from_secs(5)).build().await?;
    control::proxy(&connection).await?.call::<_, _, ()>("Cancel", &(generation,)).await?;
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
