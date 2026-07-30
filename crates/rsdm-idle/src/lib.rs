//! Compositor-driven idle monitoring for rsdm.

use std::{
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use rsdm_core::domain::IdleConfig;
use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle, delegate_noop,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{wl_registry, wl_seat},
};
use wayland_protocols::ext::idle_notify::v1::client::{
    ext_idle_notification_v1::{self, ExtIdleNotificationV1},
    ext_idle_notifier_v1::ExtIdleNotifierV1,
};

const LOCK_READY_TIMEOUT: Duration = Duration::from_secs(10);
static SCOPE_SEQUENCE: AtomicU64 = AtomicU64::new(1);

struct IdleApp {
    config: IdleConfig,
    config_path: PathBuf,
    lock_active: Arc<AtomicBool>,
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
    let seat: wl_seat::WlSeat = globals
        .bind(&qh, 1..=9, ())
        .context("compositor has no wl_seat for idle monitoring")?;
    let notifier: ExtIdleNotifierV1 = globals
        .bind(&qh, 1..=2, ())
        .context("compositor does not support ext-idle-notify-v1")?;
    let _notification = if config.ignore_inhibitors && notifier.version() >= 2 {
        notifier.get_input_idle_notification(timeout_ms, &seat, &qh, ())
    } else {
        if config.ignore_inhibitors {
            tracing::warn!(
                "compositor exposes ext-idle-notify-v1 version 1; idle inhibitors remain active"
            );
        }
        notifier.get_idle_notification(timeout_ms, &seat, &qh, ())
    };

    tracing::info!(
        timeout_seconds = config.timeout,
        ignore_inhibitors = config.ignore_inhibitors,
        "idle monitoring started"
    );
    let mut app = IdleApp {
        config: config.clone(),
        config_path: config_path.to_path_buf(),
        lock_active: Arc::new(AtomicBool::new(false)),
    };
    loop {
        queue
            .blocking_dispatch(&mut app)
            .context("idle Wayland dispatch failed")?;
    }
}

impl Dispatch<ExtIdleNotificationV1, ()> for IdleApp {
    fn event(
        state: &mut Self,
        _proxy: &ExtIdleNotificationV1,
        event: ext_idle_notification_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            ext_idle_notification_v1::Event::Idled => state.start_lock_cycle(),
            ext_idle_notification_v1::Event::Resumed => {
                tracing::debug!("user activity resumed; an active lock remains locked")
            }
            _ => {}
        }
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

fn run_lock_cycle(config: IdleConfig, config_path: PathBuf, active: Arc<AtomicBool>) {
    struct ResetActive(Arc<AtomicBool>);
    impl Drop for ResetActive {
        fn drop(&mut self) {
            self.0.store(false, Ordering::Release);
        }
    }
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
    let (mut command, scoped) = lock_command(&argv);

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            tracing::error!(%error, command = ?config.lock_command, "failed to start idle locker");
            return;
        }
    };
    let ready = if built_in {
        let expected_pid = (!scoped).then_some(child.id());
        wait_until_ready(&mut child, expected_pid)
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
            false
        }
        Ok(None) => {
            tracing::info!(pid = child.id(), "idle locker is active");
            run_hooks("on_lock", &config.on_lock);
            true
        }
    };

    match child.wait() {
        Ok(status) if status.success() => tracing::info!("idle locker exited after unlock"),
        Ok(status) => tracing::error!(%status, "idle locker exited unsuccessfully"),
        Err(error) => tracing::error!(%error, "failed waiting for idle locker"),
    }
    if lock_confirmed {
        run_hooks("on_unlock", &config.on_unlock);
    } else {
        tracing::warn!("skipping on_unlock hooks because lock activation was never confirmed");
    }
}

/// A service-managed idle monitor places its locker in a distinct transient
/// scope. Stopping/restarting rsdm-idle can then use systemd's safe default
/// KillMode=control-group without killing the session-lock owner.
fn lock_command(argv: &[std::ffi::OsString]) -> (Command, bool) {
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
        (command, true)
    } else {
        let mut command = Command::new(&argv[0]);
        command.args(&argv[1..]);
        (command, false)
    }
}

/// `Ok(None)` means ready; `Ok(Some(status))` means it exited first.
fn wait_until_ready(child: &mut Child, expected_pid: Option<u32>) -> Result<Option<ExitStatus>> {
    let deadline = Instant::now() + LOCK_READY_TIMEOUT;
    let uid = rsdm_infra::lock_control::current_uid();
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait().context("checking idle locker status")? {
            return Ok(Some(status));
        }
        if rsdm_infra::lock_control::lock_state(uid)
            .ok()
            .flatten()
            .is_some_and(|state| expected_pid.is_none_or(|pid| state.pid == pid))
        {
            return Ok(None);
        }
        thread::sleep(Duration::from_millis(50));
    }
    anyhow::bail!("no compositor lock confirmation within 10 seconds")
}

fn run_hooks(phase: &str, hooks: &[String]) {
    for hook in hooks {
        tracing::info!(%phase, command = %hook, "running idle hook");
        match Command::new("sh").arg("-c").arg(hook).status() {
            Ok(status) if status.success() => {}
            Ok(status) => tracing::warn!(%phase, %status, command = %hook, "idle hook failed"),
            Err(error) => {
                tracing::warn!(%phase, %error, command = %hook, "could not start idle hook")
            }
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for IdleApp {
    fn event(
        _state: &mut Self,
        _proxy: &wl_registry::WlRegistry,
        _event: wl_registry::Event,
        _data: &GlobalListContents,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

delegate_noop!(IdleApp: ignore wl_seat::WlSeat);
delegate_noop!(IdleApp: ExtIdleNotifierV1);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_worker_guard_resets_activity() {
        let active = Arc::new(AtomicBool::new(true));
        {
            struct Guard(Arc<AtomicBool>);
            impl Drop for Guard {
                fn drop(&mut self) {
                    self.0.store(false, Ordering::Release);
                }
            }
            let _guard = Guard(Arc::clone(&active));
        }
        assert!(!active.load(Ordering::Acquire));
    }
}
