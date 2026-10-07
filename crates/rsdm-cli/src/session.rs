use std::path::Path;

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use rsdm_core::domain::{SessionMode, ShutdownMethod, ShutdownPolicy, TimeoutAction};
use rsdm_infra::{config::load_config, session_manager, unix::split_exec};

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum PowerAction { Reboot, Poweroff }

pub fn power(action: PowerAction) -> Result<()> {
    let action = match action { PowerAction::Reboot => "reboot", PowerAction::Poweroff => "poweroff" };
    print_outcome(rsdm_infra::power::request(action)?)
}

#[derive(Debug, Subcommand)]
pub enum SessionAction {
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
    Finalize { names: Vec<String> },
    /// Close registered applications before stopping the graphical session.
    Stop,
    /// Cancel preparation while the compositor is still running.
    Cancel,
    /// Show the coordinator and its registered applications.
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
    #[arg(long, default_value_t = 30)]
    shutdown_timeout: u64,
    #[arg(long, default_value = "force")]
    on_timeout: TimeoutAction,
    #[arg(long, default_value = "auto")]
    shutdown_method: ShutdownMethod,
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
            session_manager::finalize(&config.session_manager, &names).context("finalizing the graphical session")
        }
        SessionAction::Stop => print_outcome(session_manager::stop("logout")?),
        SessionAction::Cancel => session_manager::cancel().context("cancelling session preparation"),
        SessionAction::Status => {
            let status = session_manager::status()?;
            println!("{}: {} (login {}, desktop {}, generation {})", status.provider, status.phase,
                status.login_session_id, status.desktop_entry_id, status.generation);
            println!("RSDM XSMP: {}", if status.xsmp_available { "available" } else { "unavailable; auto uses the provider's shutdown method" });
            for (unit, method, timeout, state) in status.apps { println!("{unit}: {state}, {method}, {timeout}s"); }
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
    std::process::exit(code);
}

pub fn print_outcome(outcome: session_manager::StopOutcome) -> Result<()> {
    println!("{}: {}", outcome.result, outcome.message);
    for unit in outcome.forced_units { eprintln!("forced shutdown: {unit}"); }
    if matches!(outcome.result.as_str(), "failed" | "cancelled") { bail!("session shutdown {}", outcome.result); }
    Ok(())
}

fn parse_command(command: Option<String>) -> Result<Vec<String>> {
    command.map(|command| split_exec(&command).context("parsing command arguments"))
        .transpose().map(Option::unwrap_or_default)
}
