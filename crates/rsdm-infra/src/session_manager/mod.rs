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
mod coordinator_readiness;
mod coordinator_requests;
mod coordinator_recovery;
mod coordinator_shutdown;
mod coordinator_startup;
mod deadline;
mod env;
mod identity;
mod lifecycle;
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

use rsdm_core::domain::{SessionManagerConfig, ShutdownPolicy};
use thiserror::Error;

pub use apps::stop_hook;
pub use client::{cancel, generation, status, stop};
pub use cleanup::cleanup;
pub use control::{SessionStatus, StopOutcome};
pub use provider::StartOptions;
pub(crate) use identity::new_generation;
pub(crate) use processes::monotonic_usec;

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("session runtime I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("session manager bus error: {0}")]
    Bus(#[from] zbus::Error),
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

pub fn start(argv: &[String], cfg: &SessionManagerConfig) -> Result<i32, SessionError> {
    start_with_options(argv, cfg, &StartOptions::default())
}

pub fn start_with_options(argv: &[String], cfg: &SessionManagerConfig, options: &StartOptions) -> Result<i32, SessionError> {
    coordinator::start(argv, cfg, options)
}

pub fn run_app(argv: &[String]) -> Result<i32, SessionError> {
    run_app_with_policy(argv, &ShutdownPolicy::default())
}

pub fn run_app_with_policy(argv: &[String], policy: &ShutdownPolicy) -> Result<i32, SessionError> {
    client::run_app(argv, policy)
}

pub fn finalize(cfg: &SessionManagerConfig, names: &[String]) -> Result<(), SessionError> {
    client::finalize(cfg, names)
}
