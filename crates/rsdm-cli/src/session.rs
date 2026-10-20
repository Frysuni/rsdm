use std::path::Path;

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use rsdm_core::domain::{SessionManagerUpdate, SessionMode, ShutdownMethod, ShutdownPolicy, TimeoutAction};
use rsdm_infra::{config::{load_config_async, watch_session_config}, unix::split_exec};
use rsdm_session_manager as session_manager;

use crate::output;

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum PowerAction { Reboot, Poweroff }

pub async fn power(path: &Path, action: PowerAction) -> Result<()> {
    let action = match action { PowerAction::Reboot => "reboot", PowerAction::Poweroff => "poweroff" };
    let cfg = load_config_async(path).await?.session_manager;
    let has_user_bus = std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some() || std::env::var_os("XDG_RUNTIME_DIR").is_some();
    if cfg.enabled && has_user_bus && session_manager::is_running(&cfg).await? {
        return print_outcome(session_manager::stop(action, &cfg).await?);
    }
    rsdm_system::logind::request_direct(action).await?;
    print_outcome(session_manager::StopOutcome {
        result: "accepted".into(), forced_units: Vec::new(), message: format!("logind accepted {action}"),
    })
}

#[derive(Debug, Subcommand)]
pub enum SessionAction {
    /// Start coordination around the original WM or desktop session command.
    #[command(after_help = "Usage notes:\n  DM wraps the selected session automatically when session management is enabled.\n\nExamples:\n  rsdm session start -- niri-session\n  rsdm session start --mode managed -- sway")]
    Start {
        #[arg(long, default_value = "auto")]
        mode: SessionMode,
        #[arg(long)]
        native_unit: Option<String>,
        #[arg(long)]
        logout_command: Option<String>,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
        compositor: Vec<String>,
    },
    /// Publish the compositor environment and activate the session when ready.
    #[command(after_help = "Examples:\n  rsdm session finalize\n  rsdm session finalize MY_VAR ANOTHER_VAR")]
    Finalize { names: Vec<String> },
    /// Close registered applications before stopping the graphical session.
    #[command(after_help = "Examples:\n  rsdm session stop\n\nUse this command in your WM's logout binding to prepare apps before teardown.")]
    Stop,
    /// Cancel preparation while the compositor is still running.
    #[command(after_help = "Examples:\n  rsdm session cancel\n\nApps that already closed are not restarted.")]
    Cancel,
    /// Show manager scope, capabilities and recent application preparation results.
    #[command(after_help = "Examples:\n  rsdm session status\n  rsdm session status > session-status.txt\n\nReading status never starts the coordinator. Older coordinators use the legacy status response.")]
    Status,
    /// Coordinate applications in the current external graphical login.
    Attach,
    #[command(hide = true)]
    Serve,
    #[command(hide = true)]
    AppStop {
        #[arg(long)]
        generation: String,
        #[arg(long)]
        unit: String,
    },
    #[command(hide = true)]
    AppExec {
        #[arg(long)]
        generation: String,
        #[arg(long)]
        unit: String,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
        argv: Vec<String>,
    },
    #[command(hide = true)]
    AppWatch {
        #[arg(long)]
        generation: String,
        #[arg(long)]
        unit: String,
    },
    #[command(hide = true)]
    Cleanup {
        #[arg(long)]
        generation: String,
        #[arg(long)]
        recorded_config: bool,
    },
}

#[derive(Debug, Args)]
pub struct AppOptions {
    /// Seconds to wait for the app to exit after a quit request.
    #[arg(long, default_value_t = 30)]
    shutdown_timeout: u64,
    /// Timeout policy: force termination or cancel preparation.
    #[arg(long, default_value = "force")]
    on_timeout: TimeoutAction,
    /// Quit method: auto, term or xsmp.
    #[arg(long, default_value = "auto")]
    shutdown_method: ShutdownMethod,
    /// Application quit command, parsed as arguments without a shell.
    #[arg(long)]
    quit_command: Option<String>,
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
    argv: Vec<String>,
}

pub async fn run(path: &Path, action: SessionAction) -> Result<()> {
    let recorded_path = match &action {
        SessionAction::Cleanup { generation, recorded_config: true } => session_manager::recovery_config_path(generation).await?,
        _ => None,
    };
    let path = recorded_path.as_deref().unwrap_or(path);
    let updates = if matches!(&action, SessionAction::Start { .. } | SessionAction::Attach | SessionAction::Serve | SessionAction::Finalize { .. } | SessionAction::AppStop { .. } | SessionAction::AppExec { .. } | SessionAction::AppWatch { .. } | SessionAction::Cleanup { .. }) {
        Some(watch_session_config(path).await?)
    } else { None };
    let cfg = match &updates {
        Some(updates) => updates.borrow().config.clone(),
        None => load_config_async(path).await?.session_manager,
    };
    match action {
        SessionAction::Attach => print_outcome(session_manager::attach_with_config_updates(updates.expect("attach configuration"), path).await?),
        SessionAction::Serve => {
            let code = session_manager::serve_with_config_updates(updates.expect("serve configuration"), path).await?;
            std::process::exit(code);
        }
        SessionAction::Start { mode, native_unit, logout_command, compositor } => {
            let logout_command = if cfg.enabled { parse_command(logout_command)? } else { Vec::new() };
            let options = session_manager::StartOptions { mode, native_unit, logout_command };
            let code = session_manager::start_with_config_updates(&compositor, updates.expect("start configuration"), &options, path).await
                .context("running the graphical session")?;
            std::process::exit(code);
        }
        SessionAction::Finalize { names } => finalize(path, &names, updates.expect("finalize configuration")).await,
        SessionAction::Stop => print_outcome(session_manager::stop("logout", &cfg).await?),
        SessionAction::Cancel => {
            let outcome = session_manager::cancel(&cfg).await.context("cancelling session preparation")?;
            if outcome.result == "disabled" { return print_outcome(outcome); }
            output::notice("CANCELLATION REQUESTED", "Shutdown cancellation requested.\n", output::WARNING).interactive_stdout()?;
            Ok(())
        }
        SessionAction::Status => {
            let details = session_manager::status_details(&cfg).await?;
            output::session_status_details(&details)?;
            Ok(())
        }
        SessionAction::AppStop { generation, unit } => {
            session_manager::stop_hook_with_config_updates(&generation, &unit, updates.expect("hook configuration")).await.context("completing application shutdown")
        }
        SessionAction::AppExec { generation, unit, argv } => {
            session_manager::exec_app_with_config_updates(&generation, &unit, &argv, updates.expect("application configuration")).await.context("starting the application")
        }
        SessionAction::AppWatch { generation, unit } => {
            session_manager::watch_app_with_config_updates(&generation, &unit, updates.expect("application lifetime configuration")).await.context("observing application lifetime")
        }
        SessionAction::Cleanup { generation, .. } => {
            session_manager::cleanup_with_config_updates(&generation, updates.expect("recovery configuration")).await.context("recovering the graphical session")
        }
    }
}

async fn finalize(path: &Path, names: &[String], updates: tokio::sync::watch::Receiver<SessionManagerUpdate>) -> Result<()> {
    if updates.borrow().config.enabled {
        let outcome = session_manager::attach_with_config_updates(updates.clone(), path).await?;
        if outcome.result == "disabled" { return print_outcome(outcome); }
    }
    let current = updates.borrow().config.clone();
    let finalized = session_manager::finalize(&current, names).await;
    if !updates.borrow().config.enabled { return print_outcome(session_manager::StopOutcome::disabled()); }
    finalized.context("finalizing the graphical session")?;
    output::notice("ENVIRONMENT PUBLISHED", "Session environment published.\n", output::SUCCESS).interactive_stdout()?;
    Ok(())
}

pub async fn run_app(path: &Path, options: AppOptions) -> Result<()> {
    let cfg = load_config_async(path).await?.session_manager;
    if !cfg.enabled {
        let code = session_manager::run_app(&options.argv, &cfg).await.context("launching app")?;
        std::process::exit(code);
    }
    let policy = ShutdownPolicy {
        timeout_secs: options.shutdown_timeout, on_timeout: options.on_timeout,
        method: options.shutdown_method, quit_command: parse_command(options.quit_command)?,
    };
    policy.validate().map_err(anyhow::Error::msg)?;
    let updates = watch_session_config(path).await?;
    let attached = session_manager::attach_with_config_updates(updates.clone(), path).await?;
    if attached.result == "disabled" || !updates.borrow().config.enabled {
        let mut disabled = updates.borrow().config.clone();
        disabled.enabled = false;
        let code = session_manager::run_app(&options.argv, &disabled).await.context("launching app")?;
        std::process::exit(code);
    }
    let code = session_manager::run_app_with_policy(&options.argv, &policy, &cfg).await.context("launching app")?;
    if code == 0 {
        output::application(&options.argv, &policy)?;
    }
    std::process::exit(code);
}

pub fn print_outcome(outcome: session_manager::StopOutcome) -> Result<()> {
    output::notice("SESSION REQUEST", format!("{}: {}\n", outcome.result, outcome.message), output::state_color(&outcome.result)).stdout()?;
    for unit in outcome.forced_units {
        output::notice("FORCED SHUTDOWN", format!("forced shutdown: {unit}\n"), output::WARNING).stderr()?;
    }
    if matches!(outcome.result.as_str(), "failed" | "cancelled" | "unknown") { bail!("session shutdown {}", outcome.result); }
    Ok(())
}

fn parse_command(command: Option<String>) -> Result<Vec<String>> {
    command.map(|command| split_exec(&command).context("parsing command arguments"))
        .transpose().map(Option::unwrap_or_default)
}
