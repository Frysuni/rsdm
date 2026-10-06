use std::{
    fmt,
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use rsdm_core::{
    domain::{AppConfig, Session},
    ports::{LoginUi, LoginUiEvent, LoginUiModel, SessionDiscoverer, UserStore},
};
use rsdm_infra::{
    security::MemoryLoginAttemptLimiter,
    sessions::DesktopSessionDiscoverer,
    storage::FileUserStore,
    unix::{VtGuard, acquire_vt, install_terminate_handler, terminate_flag, terminate_requested},
};
use rsdm_tui::RatatuiLoginUi;

mod login;

pub fn run_dm(config: AppConfig) -> Result<()> {
    if !config.dm.enable {
        tracing::info!("display manager is disabled by configuration");
        return Ok(());
    }

    ensure_stdio_not_active_tty(&config)?;
    install_terminate_handler();
    let vt = acquire_vt(&config.dm.tty.path)?;
    let wrapper = session_wrapper(&config);
    let sessions = resolve_sessions(&config)?;
    let store = FileUserStore::new(&config.paths.cache_dir);
    let limiter = MemoryLoginAttemptLimiter::new(
        config.security.max_failed_attempts,
        Duration::from_millis(config.security.failure_delay_ms),
    );

    tracing::info!(
        tty = %config.dm.tty.path,
        seat = %config.dm.tty.seat,
        session_count = sessions.len(),
        session_manager = config.session_manager.enabled,
        "display manager initialized"
    );
    run_greeter_loop(config, vt, wrapper, sessions, store, limiter)
}

fn run_greeter_loop(
    config: AppConfig,
    vt: VtGuard,
    wrapper: Vec<String>,
    sessions: Vec<Session>,
    store: FileUserStore,
    limiter: MemoryLoginAttemptLimiter,
) -> Result<()> {
    let mut ui = RatatuiLoginUi;
    let mut error_message = None;

    loop {
        if terminate_requested() {
            return Ok(());
        }
        vt.reclaim();
        if let Err(error) = rsdm_infra::console_font::publish(&config.dm.tty.path) {
            tracing::warn!(%error, "could not share the Greeter console font with Lock");
        }

        let username = remembered_username(&config, &store);
        let session = remembered_session(&config, &store);
        let mut parked = None;
        let event = ui.run(
            LoginUiModel {
                config: &config,
                sessions: &sessions,
                remembered_username: username.as_deref(),
                remembered_session: session.as_deref(),
                error_message: error_message.as_deref(),
                terminate: terminate_flag(),
            },
            &mut |attempt, conversation| {
                login::begin(
                    &config,
                    &wrapper,
                    &limiter,
                    &sessions,
                    &mut parked,
                    attempt,
                    conversation,
                )
            },
        )?;

        match event {
            LoginUiEvent::Terminated => return Ok(()),
            LoginUiEvent::ExitToTty => return Err(TtyFallbackRequested.into()),
            LoginUiEvent::Reboot => {
                error_message = request_power_action("reboot")?;
            }
            LoginUiEvent::Shutdown => {
                error_message = request_power_action("poweroff")?;
            }
            LoginUiEvent::SessionReady => {
                let Some(parked) = parked else {
                    error_message = Some("Session launch failed".to_string());
                    continue;
                };
                error_message = login::supervise(&store, parked);
            }
        }
    }
}

fn request_power_action(action: &str) -> Result<Option<String>> {
    tracing::warn!(action, "power action requested from greeter");
    let status = match Command::new("systemctl").arg(action).status() {
        Ok(status) => status,
        Err(error) => return Ok(Some(format!("failed to request {action}: {error}"))),
    };
    if !status.success() {
        return Ok(Some(format!(
            "failed to request {action}: systemctl exited with {status}"
        )));
    }

    while !terminate_requested() {
        thread::sleep(Duration::from_millis(200));
    }
    Ok(None)
}

#[derive(Debug)]
pub(crate) struct TtyFallbackRequested;

impl fmt::Display for TtyFallbackRequested {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("user requested TTY fallback")
    }
}

impl std::error::Error for TtyFallbackRequested {}

pub(crate) fn ensure_stdio_not_active_tty(config: &AppConfig) -> Result<()> {
    let tty = Path::new(&config.dm.tty.path);
    for fd in [1, 2] {
        let Some(target) = fd_target(fd) else {
            continue;
        };
        if paths_match(&target, tty) {
            bail!(
                "fd {fd} points at active TTY {}; set StandardOutput=journal and StandardError=journal",
                config.dm.tty.path
            );
        }
    }
    Ok(())
}

fn session_wrapper(config: &AppConfig) -> Vec<String> {
    if !config.session_manager.enabled {
        return Vec::new();
    }

    match std::env::current_exe() {
        Ok(executable) => vec![
            executable.to_string_lossy().into_owned(),
            "session".to_string(),
            "start".to_string(),
            "--".to_string(),
        ],
        Err(error) => {
            tracing::warn!(%error, "cannot resolve rsdm binary; launching session directly");
            Vec::new()
        }
    }
}

fn resolve_sessions(config: &AppConfig) -> Result<Vec<Session>> {
    let discovered = DesktopSessionDiscoverer::from_config(&config.dm)
        .discover()
        .context("discovering sessions")?;

    if let Some(fixed) = config.dm.fixed_session.as_deref() {
        let session = discovered
            .iter()
            .find(|session| session.id == fixed || session.exec == fixed || session.name == fixed)
            .cloned()
            .unwrap_or_else(|| Session::new(fixed, fixed, fixed, "<fixed>"));
        return Ok(vec![session]);
    }
    if discovered.is_empty() {
        bail!("no Wayland sessions discovered");
    }
    Ok(discovered)
}

fn remembered_username(config: &AppConfig, store: &FileUserStore) -> Option<String> {
    config
        .dm
        .remember
        .username
        .then(|| store.load_remembered_username().ok().flatten())
        .flatten()
}

fn remembered_session(config: &AppConfig, store: &FileUserStore) -> Option<String> {
    if !config.dm.uses_picker() || !config.dm.remember.session {
        return None;
    }
    store.load_remembered_session().ok().flatten()
}

fn fd_target(fd: i32) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/self/fd/{fd}")).ok()
}

fn paths_match(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    let Ok(left) = left.canonicalize() else {
        return false;
    };
    let Ok(right) = right.canonicalize() else {
        return false;
    };
    left == right
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_dm_exits_before_touching_the_tty() {
        let mut config = AppConfig::default();
        config.dm.enable = false;
        config.dm.tty.path = "/path/that/must/not/be/opened".to_string();

        assert!(run_dm(config).is_ok());
    }
}
