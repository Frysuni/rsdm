//! Compositor-driven idle monitoring for rsdm.

use std::{
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
};

use anyhow::{Context, Result};
use rsdm_core::domain::IdleConfig;
use wayland_client::{
    Connection, Proxy,
    globals::registry_queue_init,
};
use wayland_protocols::ext::idle_notify::v1::client::ext_idle_notifier_v1::ExtIdleNotifierV1;

mod hooks;
mod readiness;
mod seats;
use hooks::run_hooks;

static SCOPE_SEQUENCE: AtomicU64 = AtomicU64::new(1);

struct IdleApp {
    config: IdleConfig,
    config_path: PathBuf,
    lock_active: Arc<AtomicBool>,
    notifier: ExtIdleNotifierV1,
    timeout_ms: u32,
    seats: Vec<seats::SeatNotification>,
}

/// Monitor compositor-reported activity until the Wayland connection closes.
pub fn run(config: &IdleConfig, config_path: &Path) -> Result<()> {
    if !config.enable {
        tracing::info!("idle monitoring is disabled by configuration");
        return Ok(());
    }

    let timeout_ms = u32::try_from(config.timeout.saturating_mul(1000))
        .context("idle timeout does not fit the Wayland protocol")?;
    let conn = Connection::connect_to_env().context("connecting idle monitor to Wayland")?;
    let (globals, mut queue) =
        registry_queue_init(&conn).context("initializing idle Wayland registry")?;
    let qh = queue.handle();
    let notifier: ExtIdleNotifierV1 = globals
        .bind(&qh, 1..=2, ())
        .context("compositor does not support ext-idle-notify-v1")?;
    if config.ignore_inhibitors && notifier.version() < 2 {
        tracing::warn!(
            "compositor exposes ext-idle-notify-v1 version 1; idle inhibitors remain active"
        );
    }

    tracing::info!(
        timeout_seconds = config.timeout,
        ignore_inhibitors = config.ignore_inhibitors,
        "idle monitoring started"
    );
    let mut app = IdleApp {
        config: config.clone(),
        config_path: config_path.to_path_buf(),
        lock_active: Arc::new(AtomicBool::new(false)),
        notifier,
        timeout_ms,
        seats: Vec::new(),
    };
    for global in globals.contents().clone_list() {
        if global.interface == "wl_seat" {
            app.add_seat(globals.registry(), global.name, global.version, &qh);
        }
    }
    anyhow::ensure!(!app.seats.is_empty(), "compositor has no wl_seat for idle monitoring");
    loop {
        queue
            .blocking_dispatch(&mut app)
            .context("idle Wayland dispatch failed")?;
    }
}

impl IdleApp {
    fn start_lock_cycle(&self) {
        if rsdm_infra::lock_control::lock_state(rsdm_infra::lock_control::current_uid())
            .ok()
            .flatten()
            .is_some()
        {
            tracing::debug!("idle threshold reached while rsdm lock is already active");
            return;
        }
        if self
            .lock_active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            tracing::debug!("idle lock cycle is already running");
            return;
        }

        let config = self.config.clone();
        let config_path = self.config_path.clone();
        let active = Arc::clone(&self.lock_active);
        tracing::info!("idle threshold reached; starting lock screen");
        if let Err(error) = thread::Builder::new()
            .name("rsdm-idle-lock".to_string())
            .spawn(move || run_lock_cycle(config, config_path, active))
        {
            self.lock_active.store(false, Ordering::Release);
            tracing::error!(%error, "failed to start idle lock worker");
        }
    }
}

struct ResetActive(Arc<AtomicBool>);

impl Drop for ResetActive {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

fn run_lock_cycle(config: IdleConfig, config_path: PathBuf, active: Arc<AtomicBool>) {
    let _reset = ResetActive(active);

    let built_in = config.lock_command.is_empty();
    let argv = if built_in {
        match std::env::current_exe() {
            Ok(executable) => vec![
                executable.into_os_string(),
                "lock".into(),
                "--config".into(),
                config_path.into_os_string(),
            ],
            Err(error) => {
                tracing::error!(%error, "cannot resolve rsdm executable for idle lock");
                return;
            }
        }
    } else {
        config.lock_command.iter().map(Into::into).collect()
    };
    let (mut command, scope) = lock_command(&argv);

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            tracing::error!(%error, command = ?config.lock_command, "failed to start idle locker");
            return;
        }
    };
    let ready = if built_in {
        readiness::wait_until_ready(&mut child, scope.as_deref())
    } else {
        // A custom locker has no rsdm readiness handshake. Starting it is the
        // strongest guarantee available, and this limitation is documented.
        Ok(None)
    };
    let lock_confirmed = match ready {
        Ok(Some(status)) => {
            tracing::error!(%status, "idle locker exited before securing the session");
            return;
        }
        Err(error) => {
            tracing::error!(%error, "idle locker did not confirm the session lock");
            readiness::terminate_after_timeout(child, scope.as_deref());
            return;
        }
        Ok(None) => {
            tracing::info!(pid = child.id(), "idle locker is active");
            run_hooks("on_lock", &config.on_lock);
            true
        }
    };

    hooks::finish_lock_cycle(child.wait(), lock_confirmed, &config.on_unlock);
}

/// A service-managed idle monitor places its locker in a distinct transient
/// scope. Stopping/restarting rsdm-idle can then use systemd's safe default
/// KillMode=control-group without killing the session-lock owner.
fn lock_command(argv: &[std::ffi::OsString]) -> (Command, Option<String>) {
    if std::env::var_os("INVOCATION_ID").is_some() {
        let sequence = SCOPE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let unit = format!("rsdm-lock-{}-{sequence}.scope", std::process::id());
        let runner = std::env::var_os("RSDM_SYSTEMD_RUN")
            .unwrap_or_else(|| std::ffi::OsString::from("systemd-run"));
        let mut command = Command::new(runner);
        command
            .args(["--user", "--scope", "--quiet", "--collect", "--unit"])
            .arg(&unit)
            .arg("--");
        command.args(argv);
        command.process_group(0);
        (command, Some(unit))
    } else {
        let mut command = Command::new(&argv[0]);
        command.args(&argv[1..]);
        command.process_group(0);
        (command, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_worker_guard_resets_activity() {
        let active = Arc::new(AtomicBool::new(true));
        {
            let _guard = ResetActive(Arc::clone(&active));
        }
        assert!(!active.load(Ordering::Acquire));
    }
}
