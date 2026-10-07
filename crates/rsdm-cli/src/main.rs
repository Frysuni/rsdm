use std::{
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, ExitCode},
};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use rsdm_core::domain::{AppConfig, FallbackConfig, SecurityConfig};
use rsdm_infra::{
    config::{DEFAULT_CONFIG_PATH, load_config},
    unix::exec_fallback,
};
use tracing::{error, info};

mod dm;
mod logging;
mod output;
mod session;
mod status;
mod unlock;

use session::SessionAction;

#[derive(Debug, Parser)]
#[command(author, version, about)]
struct Cli {
    #[arg(
        long,
        global = true,
        default_value = DEFAULT_CONFIG_PATH,
        env = "RSDM_CONFIG"
    )]
    config: PathBuf,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the display manager greeter on the configured TTY.
    Dm,
    /// Lock the current Wayland session.
    Lock,
    /// Monitor Wayland activity and lock after the idle timeout.
    Idle,
    /// Privileged emergency unlock for an unresponsive rsdm lock screen.
    Unlock {
        #[arg(long)]
        user: Option<String>,
        #[arg(long, hide = true)]
        uid: Option<u32>,
    },
    /// Open rsdm's journal.
    Logs {
        #[arg(short, long)]
        follow: bool,
        #[arg(short = 'n', long, default_value_t = 200)]
        lines: u32,
        #[arg(long, value_enum, default_value_t = LogComponent::All)]
        component: LogComponent,
    },
    /// Show configured features and live lock state.
    Status,
    /// Manage the systemd user graphical session.
    Session {
        #[command(subcommand)]
        action: SessionAction,
    },
    /// Launch an application inside the graphical session.
    App(session::AppOptions),
    /// Prepare the current session and ask logind or the native desktop for power.
    Power {
        #[arg(value_enum)]
        action: session::PowerAction,
    },
    /// Validate the TOML configuration file.
    ValidateConfig,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum LogComponent {
    All,
    Dm,
    Idle,
    Lock,
}

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => return output::usage(error),
    };
    logging::init(&cli.config);

    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            error!(target: "rsdm::cli", error = %format!("{error:#}"), "command failed");
            let _ = output::notice("COMMAND FAILED", format!("error: {error:#}\n"), output::ERROR).stderr();
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Dm => run_dm(&cli.config),
        Command::Lock => run_lock(&cli.config),
        Command::Idle => run_idle(&cli.config),
        Command::Unlock { user, uid } => unlock::run(user.as_deref(), uid),
        Command::Logs {
            follow,
            lines,
            component,
        } => run_logs(follow, lines, component),
        Command::Status => status::run(&cli.config),
        Command::Session { action } => session::run(&cli.config, action),
        Command::App(options) => session::run_app(options),
        Command::Power { action } => session::power(action),
        Command::ValidateConfig => validate_config(&cli.config),
    }
}

fn run_lock(path: &Path) -> Result<()> {
    let config = load_config(path)?;
    rsdm_lock::run(&config, path).context("running the lock screen")
}

fn run_idle(path: &Path) -> Result<()> {
    let config = load_config(path)?;
    rsdm_idle::run(&config.idle, path).context("running idle monitor")
}

fn validate_config(path: &Path) -> Result<()> {
    let config = load_config(path)?;
    print_config_warnings(&config);
    output::notice("CONFIGURATION VALID", format!("configuration is valid: {}\n", path.display()), output::SUCCESS).stdout()?;
    Ok(())
}

fn run_logs(follow: bool, lines: u32, component: LogComponent) -> Result<()> {
    let mut command = ProcessCommand::new("journalctl");
    command
        .arg("--boot")
        .arg("--output=short-precise")
        .arg("--no-hostname")
        .arg("--pager-end")
        .arg(format!("--lines={lines}"));
    if follow {
        command.arg("--follow");
    }

    match component {
        LogComponent::All => {
            command.arg("SYSLOG_IDENTIFIER=rsdm");
        }
        LogComponent::Dm => {
            command.arg("--unit=rsdm.service");
        }
        LogComponent::Idle => {
            command.arg("--user-unit=rsdm-idle.service");
        }
        LogComponent::Lock => {
            command
                .arg("SYSLOG_IDENTIFIER=rsdm")
                .arg("--grep=rsdm_lock|lock screen|session lock|idle locker");
        }
    }

    use std::os::unix::process::CommandExt as _;
    Err(command.exec()).context("opening rsdm journal with journalctl")
}

fn run_dm(path: &Path) -> Result<()> {
    let config = load_config(path).context("loading DM configuration")?;

    info!(tty = %config.dm.tty.path, "starting dm runtime");
    let tty_path = config.dm.tty.path.clone();
    let fallback = config.dm.fallback.clone();
    let security = config.security.clone();
    match dm::run_dm(config, path) {
        Ok(()) => Ok(()),
        Err(error) => fallback_or_error(&fallback, &security, &tty_path, error),
    }
}

fn fallback_or_error(
    fallback: &FallbackConfig,
    security: &SecurityConfig,
    tty_path: &str,
    error: anyhow::Error,
) -> Result<()> {
    if rsdm_infra::unix::terminate_requested() {
        info!("service stop requested; exiting instead of falling back");
        return Ok(());
    }
    if !fallback.enabled {
        return Err(error);
    }
    if !fallback.permitted(security) {
        return Err(error.context(
            "TTY fallback cannot enforce the configured account restrictions",
        ));
    }
    if error.downcast_ref::<rsdm_infra::unix::VtError>().is_some() {
        error!(
            error = %format!("{error:#}"),
            "refusing to start: VT is not available; not running the fallback"
        );
        return Err(error);
    }

    if error.downcast_ref::<dm::TtyFallbackRequested>().is_some() {
        info!("handing over to TTY fallback");
    } else {
        error!(error = %format!("{error:#}"), "greeter failed; handing over to TTY fallback");
    }

    match exec_fallback(tty_path, &fallback.command) {
        Ok(never) => match never {},
        Err(fallback_error) => {
            Err(error.context(format!("TTY fallback also failed: {fallback_error}")))
        }
    }
}

fn print_config_warnings(config: &AppConfig) {
    for warning in config.validation_warnings() {
        tracing::warn!(
            target: "rsdm::cli",
            field = warning.field,
            message = warning.message,
            "config warning"
        );
        let _ = output::notice("CONFIGURATION WARNING", format!("warning: {}: {}\n", warning.field, warning.message), output::WARNING).stderr();
    }
}
