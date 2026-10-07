//! Power requests run separately so the lock keeps servicing Wayland events.

use std::{sync::mpsc::{self, Receiver, TryRecvError}, thread};

use rsdm_infra::session_manager::{SessionError, StopOutcome};
use rsdm_ui::LockPending;

use super::App;

pub(super) struct PowerJob {
    result: Receiver<Result<StopOutcome, SessionError>>,
}

pub(super) fn start_capability_query() -> Option<Receiver<rsdm_infra::power::PowerCapabilities>> {
    let (sender, result) = mpsc::channel();
    if let Err(error) = thread::Builder::new().name("rsdm-lock-power-capabilities".into()).spawn(move || {
        match rsdm_infra::power::capabilities() {
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
    fn start(action: LockPending) -> Self {
        let verb = match action {
            LockPending::Reboot => "reboot", LockPending::Shutdown => "poweroff",
            LockPending::Hibernate => "hibernate", LockPending::Sleep => "suspend",
        };
        let (sender, result) = mpsc::channel();
        thread::spawn(move || { let _ = sender.send(rsdm_infra::power::request(verb)); });
        Self { result }
    }
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
        self.power_action = Some(PowerJob::start(action));
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
