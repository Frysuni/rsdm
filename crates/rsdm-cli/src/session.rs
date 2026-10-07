use std::path::Path;

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use rsdm_core::domain::{SessionMode, ShutdownMethod, ShutdownPolicy, TimeoutAction};
use rsdm_infra::{config::load_config, session_manager, unix::split_exec};

use crate::output;

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum PowerAction { Reboot, Poweroff }

pub fn power(action: PowerAction) -> Result<()> {
    let action = match action { PowerAction::Reboot => "reboot", PowerAction::Poweroff => "poweroff" };
    print_outcome(rsdm_infra::power::request(action)?)
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
    /// Show the coordinator and its registered applications.
    #[command(after_help = "Examples:\n  rsdm session status\n  rsdm session status > session-status.txt")]
    Status,
    #[command(hide = true)]
    AppStop {
        #[arg(long)]
        generation: String,
        #[arg(long)]
        unit: String,
    },
    #[command(hide = true)]
    Cleanup {
        #[arg(long)]
        generation: String,
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

pub fn run(path: &Path, action: SessionAction) -> Result<()> {
    match action {
        SessionAction::Start { mode, native_unit, logout_command, compositor } => {
            let config = load_config(path)?;
            let options = session_manager::StartOptions { mode, native_unit, logout_command: parse_command(logout_command)? };
            let code = session_manager::start_with_options(&compositor, &config.session_manager, &options)
                .context("running the graphical session")?;
            std::process::exit(code);
        }
        SessionAction::Finalize { names } => {
            let config = load_config(path)?;
            session_manager::finalize(&config.session_manager, &names).context("finalizing the graphical session")?;
            output::notice("ENVIRONMENT PUBLISHED", "Session environment published.\n", output::SUCCESS).interactive_stdout()?;
            Ok(())
        }
        SessionAction::Stop => print_outcome(session_manager::stop("logout")?),
        SessionAction::Cancel => {
            session_manager::cancel().context("cancelling session preparation")?;
            output::notice("CANCELLATION REQUESTED", "Shutdown cancellation requested.\n", output::WARNING).interactive_stdout()?;
            Ok(())
        }
        SessionAction::Status => {
            let status = session_manager::status()?;
            output::session_status(&status)?;
            Ok(())
        }
        SessionAction::AppStop { generation, unit } => session_manager::stop_hook(&generation, &unit).context("completing application shutdown"),
        SessionAction::Cleanup { generation } => session_manager::cleanup(&generation).context("recovering the graphical session"),
    }
}

pub fn run_app(options: AppOptions) -> Result<()> {
    let policy = ShutdownPolicy {
        timeout_secs: options.shutdown_timeout, on_timeout: options.on_timeout,
        method: options.shutdown_method, quit_command: parse_command(options.quit_command)?,
    };
    let code = session_manager::run_app_with_policy(&options.argv, &policy).context("launching app")?;
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
    if matches!(outcome.result.as_str(), "failed" | "cancelled") { bail!("session shutdown {}", outcome.result); }
    Ok(())
}

fn parse_command(command: Option<String>) -> Result<Vec<String>> {
    command.map(|command| split_exec(&command).context("parsing command arguments"))
        .transpose().map(Option::unwrap_or_default)
}
