//! Coordinated graphical session lifecycle, with native desktop ownership.

mod activation;
mod action_gate;
mod app_record_lease;
mod app_stop;
mod app_dispatch;
mod app_result;
mod attach;
mod attach_bootstrap;
mod apps;
mod bus;
mod client;
mod cleanup;
mod control;
mod control_caller;
mod control_handshake;
mod coordinator;
mod coordinator_disable;
mod coordinator_environment;
mod coordinator_notifications;
mod coordinator_observation;
mod coordinator_probe;
mod coordinator_readiness;
mod coordinator_requests;
mod coordinator_recovery;
mod coordinator_shutdown;
mod coordinator_startup;
mod deadline;
mod env;
mod identity;
mod lifecycle;
mod native_compositor;
mod niri;
mod window_close;
mod plain_launch;
mod partial;
mod processes;
mod provider;
mod runtime;
mod runtime_deadline;
mod session_lease;
mod session_process;
mod session_wait;
mod shutdown_transaction;
mod signals;
mod startup;
mod unit_name;
mod units;
mod xsmp;

#[cfg(test)]
mod helper_start_fixture;

#[cfg(test)]
mod control_test_authority;

#[cfg(test)]
mod enabled_tests;

#[cfg(test)]
mod disabled_generation_tests;

use rsdm_core::domain::{SessionManagerConfig, SessionManagerUpdate, ShutdownPolicy};
use thiserror::Error;

pub use client::{cancel, generation, is_running, status, stop};
pub use control::{SessionStatus, StopOutcome};
pub use provider::StartOptions;

pub async fn attach_with_config_updates(updates: tokio::sync::watch::Receiver<SessionManagerUpdate>, path: &std::path::Path) -> Result<StopOutcome, SessionError> {
    let cfg = updates.borrow().config.clone();
    if !cfg.enabled { return Ok(StopOutcome::disabled()); }
    let actions = action_gate::ActionGate::new(Some(updates));
    let result = attach::attach(&cfg, path, actions.clone()).await;
    if actions.check().is_err() { return Ok(StopOutcome::disabled()); }
    let generation = result?;
    Ok(StopOutcome { result: "attached".into(), forced_units: Vec::new(), message: format!("joined session {generation}") })
}

pub async fn serve_with_config_updates(updates: tokio::sync::watch::Receiver<SessionManagerUpdate>, path: &std::path::Path) -> Result<i32, SessionError> {
    let cfg = updates.borrow().config.clone();
    partial::serve(&cfg, action_gate::ActionGate::new(Some(updates)), path).await
}

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("session manager is disabled")]
    Disabled,
    #[error("session runtime I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("session manager bus error: {0}")]
    Bus(#[from] zbus::Error),
    #[error("{error}")]
    StartRejected {
        #[source]
        error: Box<SessionError>,
        // A replacement peer may have restored ownership during the rejected call.
        reference_retained: bool,
    },
    #[error("{0}")]
    State(String),
    #[error("no compositor or application command was given")]
    EmptyCommand,
    #[error("a graphical session is already active; refusing to replace it")]
    SessionAlreadyActive,
    #[error("failed to spawn compositor: {0}")]
    Spawn(std::io::Error),
    #[error("failed to wait for compositor: {0}")]
    Wait(std::io::Error),
    #[error("graphical-session.target is not active; `rsdm app` must run inside a graphical session")]
    NoGraphicalSession,
}

pub async fn start(argv: &[String], cfg: &SessionManagerConfig) -> Result<i32, SessionError> {
    start_with_options(argv, cfg, &StartOptions::default()).await
}

pub async fn start_with_options(argv: &[String], cfg: &SessionManagerConfig, options: &StartOptions) -> Result<i32, SessionError> {
    if !cfg.enabled { return plain_launch::run(argv).await; }
    coordinator::start(argv, cfg, options, action_gate::ActionGate::default(), None).await
}

pub async fn start_with_config_updates(
    argv: &[String], mut updates: tokio::sync::watch::Receiver<SessionManagerUpdate>,
    options: &StartOptions, config_path: &std::path::Path,
) -> Result<i32, SessionError> {
    let cfg = updates.borrow_and_update().config.clone();
    if !cfg.enabled {
        drop(updates);
        return plain_launch::run(argv).await;
    }
    let path = config_path.to_path_buf();
    let path = runtime::blocking(move || Ok(std::path::absolute(path)?)).await?;
    coordinator::start(argv, &cfg, options, action_gate::ActionGate::new(Some(updates)), Some(path)).await
}

pub async fn run_app(argv: &[String], cfg: &SessionManagerConfig) -> Result<i32, SessionError> {
    run_app_with_policy(argv, &ShutdownPolicy::default(), cfg).await
}

pub async fn run_app_with_policy(argv: &[String], policy: &ShutdownPolicy, cfg: &SessionManagerConfig) -> Result<i32, SessionError> {
    if !cfg.enabled { return plain_launch::run(argv).await; }
    client::run_app(argv, policy).await
}

pub async fn finalize(cfg: &SessionManagerConfig, names: &[String]) -> Result<(), SessionError> {
    if !cfg.enabled { return Ok(()); }
    client::finalize(cfg, names).await
}

pub async fn stop_hook(generation: &str, unit: &str, cfg: &SessionManagerConfig) -> Result<(), SessionError> {
    if !cfg.enabled { return Ok(()); }
    apps::stop_hook(generation, unit, action_gate::ActionGate::default()).await
}

pub async fn cleanup(generation: &str, cfg: &SessionManagerConfig) -> Result<(), SessionError> {
    if !cfg.enabled { return Ok(()); }
    cleanup::cleanup(generation, action_gate::ActionGate::default()).await
}

pub async fn stop_hook_with_config_updates(generation: &str, unit: &str, updates: tokio::sync::watch::Receiver<SessionManagerUpdate>) -> Result<(), SessionError> {
    if !updates.borrow().config.enabled { return Ok(()); }
    apps::stop_hook(generation, unit, action_gate::ActionGate::new(Some(updates))).await
}

pub async fn cleanup_with_config_updates(generation: &str, updates: tokio::sync::watch::Receiver<SessionManagerUpdate>) -> Result<(), SessionError> {
    if !updates.borrow().config.enabled { return Ok(()); }
    cleanup::cleanup(generation, action_gate::ActionGate::new(Some(updates))).await
}

impl From<rsdm_system::logind::LogindError> for SessionError {
    fn from(error: rsdm_system::logind::LogindError) -> Self {
        match error {
            rsdm_system::logind::LogindError::Io(error) => Self::Io(error),
            rsdm_system::logind::LogindError::Bus(error) => Self::Bus(error),
            rsdm_system::logind::LogindError::State(message) => Self::State(message),
        }
    }
}
