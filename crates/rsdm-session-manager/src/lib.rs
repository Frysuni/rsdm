//! Coordinated graphical session lifecycle, with native desktop ownership.

mod activation;
mod app_record_lease;
mod app_stop;
mod apps;
mod bus;
mod client;
mod cleanup;
mod control;
mod coordinator;
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
mod plain_launch;
mod processes;
mod provider;
mod runtime;
mod session_lease;
mod session_process;
mod signals;
mod startup;
mod unit_name;
mod units;
mod xsmp;

#[cfg(test)]
mod helper_start_fixture;

#[cfg(test)]
mod enabled_tests;

use rsdm_core::domain::{SessionManagerConfig, ShutdownPolicy};
use thiserror::Error;

pub use client::{cancel, generation, status, stop};
pub use control::{SessionStatus, StopOutcome};
pub use provider::StartOptions;

#[derive(Debug, Error)]
pub enum SessionError {
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
    coordinator::start(argv, cfg, options).await
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
    apps::stop_hook(generation, unit).await
}

pub async fn cleanup(generation: &str, cfg: &SessionManagerConfig) -> Result<(), SessionError> {
    if !cfg.enabled { return Ok(()); }
    cleanup::cleanup(generation).await
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
