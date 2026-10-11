//! Power requests run separately so the lock keeps servicing Wayland events.

use std::{path::{Path, PathBuf}, sync::mpsc::{self, Receiver, TryRecvError}, thread};

use rsdm_session_manager::{SessionError, StopOutcome};
use rsdm_ui::LockPending;

use super::App;

pub(super) struct PowerJob {
    result: Receiver<Result<StopOutcome, SessionError>>,
}

pub(super) fn start_capability_query() -> Option<Receiver<rsdm_infra::power::PowerCapabilities>> {
    let (sender, result) = mpsc::channel();
    if let Err(error) = thread::Builder::new().name("rsdm-lock-power-capabilities".into()).spawn(move || {
        let capabilities = tokio::runtime::Builder::new_current_thread().enable_all().build()
            .map_err(rsdm_system::logind::LogindError::from)
            .and_then(|runtime| runtime.block_on(rsdm_system::logind::capabilities()));
        match capabilities {
            Ok(capabilities) => { let _ = sender.send(capabilities); }
            Err(error) => tracing::warn!(%error, "cannot determine logind sleep capabilities"),
        }
    }) {
        tracing::warn!(%error, "cannot start logind capability query");
        return None;
    }
    Some(result)
}

impl PowerJob {
    fn start(action: LockPending, config_path: PathBuf) -> Self {
        let verb = match action {
            LockPending::Reboot => "reboot", LockPending::Shutdown => "poweroff",
            LockPending::Hibernate => "hibernate", LockPending::Sleep => "suspend",
        };
        let (sender, result) = mpsc::channel();
        thread::spawn(move || { let _ = sender.send(request(verb, &config_path)); });
        Self { result }
    }
}

fn request(verb: &str, config_path: &Path) -> Result<StopOutcome, SessionError> {
    // Read the current policy on this worker before creating its runtime.
    let cfg = if matches!(verb, "reboot" | "poweroff") && std::env::var_os("RSDM_SESSION_GENERATION").is_some() {
        Some(rsdm_infra::config::load_config(config_path)
            .map_err(|error| SessionError::State(error.to_string()))?.session_manager)
    } else { None };
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    runtime.block_on(async {
        if let Some(cfg) = cfg.filter(|cfg| cfg.enabled) {
            return rsdm_session_manager::stop(verb, &cfg).await;
        }
        rsdm_system::logind::request_direct(verb).await?;
        Ok(StopOutcome {
            result: "accepted".into(), forced_units: Vec::new(), message: format!("logind accepted {verb}"),
        })
    })
}

impl App {
    pub(super) fn process_power_capabilities(&mut self) {
        let Some(result) = &self.power_capabilities else { return; };
        match result.try_recv() {
            Ok(capabilities) => {
                self.ctx.hibernate_available = capabilities.hibernate;
                self.ctx.suspend_available = capabilities.suspend;
                self.needs_redraw = true;
            }
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {},
        }
        self.power_capabilities = None;
    }

    pub(super) fn start_power_action(&mut self, action: LockPending) {
        if self.power_action.is_some() { return; }
        self.power_action = Some(PowerJob::start(action, self.ctx.config_path.clone()));
        self.needs_redraw = true;
    }

    pub(super) fn process_power_action(&mut self) {
        let Some(job) = &self.power_action else { return; };
        let result = match job.result.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err(SessionError::State("power request worker stopped".into())),
        };
        self.power_action = None;
        match result {
            Ok(outcome) if !matches!(outcome.result.as_str(), "failed" | "cancelled") => {},
            Ok(outcome) => self.model.set_error(outcome.message),
            Err(error) => {
                tracing::error!(%error, "power request failed");
                self.model.set_error(format!("Power request failed: {error}"));
            }
        }
        // Only successful authentication or emergency unlock can release the lock.
        self.needs_redraw = true;
    }
}
