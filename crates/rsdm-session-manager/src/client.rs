//! CLI requests authenticated against the current coordinated login.

use std::time::Duration;

use rsdm_core::domain::{SessionManagerConfig, ShutdownPolicy};

use super::{
    SessionError, control::{self, LaunchRequest, StopOutcome}, env,
};

#[path = "client_status.rs"]
mod status_query;
pub use status_query::{status, status_details};

pub async fn generation(cfg: &SessionManagerConfig) -> Result<String, SessionError> {
    if !cfg.enabled { return Err(SessionError::State("session manager is disabled".into())); }
    current_generation().await
}

pub async fn is_running(cfg: &SessionManagerConfig) -> Result<bool, SessionError> {
    if !cfg.enabled { return Ok(false); }
    tokio::time::timeout(Duration::from_secs(5), async {
        let connection = zbus::connection::Builder::session()?.method_timeout(Duration::from_secs(5)).build().await?;
        super::attach::control_exists(&connection).await
    }).await.map_err(|_| SessionError::State("coordinator presence query timed out".into()))?
}

async fn current_generation() -> Result<String, SessionError> {
    let connection = zbus::connection::Builder::session()?.method_timeout(Duration::from_secs(5)).build().await?;
    Ok(control::proxy(&connection).await?.call("Handshake", &()).await?)
}

pub async fn run_app(argv: &[String], policy: &ShutdownPolicy) -> Result<i32, SessionError> {
    policy.validate().map_err(|error| SessionError::State(error.into()))?;
    let generation = current_generation().await?;
    let directory = super::runtime::blocking(|| Ok(std::env::current_dir()?)).await?;
    let request = LaunchRequest {
        generation, argv: argv.to_vec(), environment: env::command_environment(),
        directory: directory.to_string_lossy().into_owned(),
        timeout_secs: policy.timeout_secs, on_timeout: policy.on_timeout.to_string(),
        method: policy.method.to_string(), quit_command: policy.quit_command.clone(),
    };
    control::validate_launch(&request).map_err(SessionError::State)?;
    let connection = zbus::connection::Builder::session()?.method_timeout(Duration::from_secs(25)).build().await?;
    let _: String = control::proxy(&connection).await?.call("Launch", &(request,)).await?;
    Ok(0)
}

pub async fn finalize(cfg: &SessionManagerConfig, names: &[String]) -> Result<(), SessionError> {
    let generation = current_generation().await?;
    let pairs = env::collect_present(&cfg.extra_env, names);
    control::validate_finalize(&generation, &pairs).map_err(SessionError::State)?;
    let connection = zbus::connection::Builder::session()?.method_timeout(Duration::from_secs(25)).build().await?;
    control::proxy(&connection).await?.call::<_, _, ()>("Finalize", &(generation, pairs)).await?;
    Ok(())
}

pub async fn stop(action: &str, cfg: &SessionManagerConfig) -> Result<StopOutcome, SessionError> {
    if !cfg.enabled { return Ok(StopOutcome::disabled()); }
    let generation = current_generation().await?;
    let status = status(cfg).await?;
    if status.generation != generation { return Err(SessionError::State("session generation is stale".into())); }
    let longest = status.apps.iter().map(|(_, _, timeout, _)| *timeout).max().unwrap_or(30);
    let connection = zbus::connection::Builder::session()?
        .method_timeout(Duration::from_secs(longest.saturating_add(90))).build().await?;
    Ok(control::proxy(&connection).await?.call("Stop", &(generation, action)).await?)
}

pub async fn cancel(cfg: &SessionManagerConfig) -> Result<StopOutcome, SessionError> {
    if !cfg.enabled { return Ok(StopOutcome::disabled()); }
    let generation = current_generation().await?;
    let connection = zbus::connection::Builder::session()?.method_timeout(Duration::from_secs(5)).build().await?;
    control::proxy(&connection).await?.call::<_, _, ()>("Cancel", &(generation,)).await?;
    Ok(StopOutcome { result: "accepted".into(), forced_units: Vec::new(), message: "shutdown cancellation requested".into() })
}
