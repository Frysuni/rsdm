//! Save all first phases before phase 2, and all saves before Die.

use rsdm_core::domain::{ShutdownMethod, TimeoutAction};

use super::{Notice, ffi, protocol::{self, Phase}, server::Server};
use crate::session_manager::{SessionError, apps, processes::{app_processes, monotonic_usec}};

impl Server {
    pub fn prepare(&mut self, units: &[String], cancellable: bool) -> Result<Vec<String>, SessionError> {
        let mut selected = Vec::new();
        for unit in units {
            let mut app = self.runtime.app(unit)?;
            if app.invocation_id.is_empty() {
                if !apps::pin_pending(&self.manager, &self.runtime, unit)? { continue; }
                app = self.runtime.app(unit)?;
            }
            if !app.policy.quit_command.is_empty() || app.policy.method == ShutdownMethod::Term
                || app_processes(&self.manager, &app)?.is_empty()
            { continue; }
            let mut peers = Vec::new();
            for (index, peer) in self.peers.iter().enumerate() {
                if peer.unit == *unit && peer.verified(&self.manager, &self.runtime)? { peers.push(index); }
            }
            if peers.is_empty() && app.policy.method == ShutdownMethod::Xsmp {
                return Err(SessionError::State(format!("{unit} has no authenticated XSMP connection")));
            }
            if !peers.is_empty() { selected.push((app, peers)); }
        }
        self.cancellable = if self.peers.iter().any(|peer| peer.selected) { self.cancellable && cancellable } else { cancellable };
        let now = monotonic_usec()?;
        let hard = self.runtime.session()?.shutdown_deadline_usec;
        for (app, peers) in &selected {
            let deadline = app.deadline_usec.unwrap_or_else(|| now.saturating_add(app.policy.timeout_secs * 1_000_000));
            for index in peers {
                let peer = &mut self.peers[*index];
                if !peer.selected {
                    peer.selected = true;
                    peer.deadline = hard.map_or(deadline, |hard| deadline.min(hard));
                    peer.cancel_on_timeout = app.policy.on_timeout == TimeoutAction::Cancel;
                }
            }
        }
        self.advance()?;
        Ok(selected.into_iter().map(|(app, _)| app.unit).collect())
    }

    pub fn cancel(&mut self) {
        for peer in &mut self.peers {
            if peer.selected && !peer.sms.is_null() {
                match peer.phase {
                    Phase::Saving | Phase::Phase2Waiting | Phase::Phase2Saving | Phase::TimedOut => {
                        unsafe { ffi::SmsShutdownCancelled(peer.sms); }
                        peer.phase = Phase::Cancelling;
                    }
                    Phase::Saved | Phase::Failed => {
                        unsafe { ffi::SmsShutdownCancelled(peer.sms); ffi::SmsSaveComplete(peer.sms); }
                        peer.phase = Phase::Idle;
                    }
                    _ => {},
                }
            }
            peer.selected = false;
            peer.interaction_requested = false;
            peer.interacting = false;
            peer.cancellation_requested = false;
        }
        self.cancellable = true;
    }

    pub fn advance(&mut self) -> Result<(), SessionError> {
        self.start_pending_saves();
        if self.peers.iter().any(|peer| peer.selected && (peer.cancellation_requested || peer.phase == Phase::Failed)) {
            let failure = self.peers.iter().find(|peer| peer.selected && peer.phase == Phase::Failed).map(|peer| peer.unit.clone());
            if self.cancellable {
                self.cancel();
                let _ = self.notices.send(failure.map_or(Notice::Cancelled, Notice::Failed));
                return Ok(());
            }
            for peer in &mut self.peers {
                peer.cancellation_requested = false;
                if peer.phase == Phase::Failed { peer.phase = Phase::TimedOut; }
            }
        }
        if self.expire_saves()? { return Ok(()); }
        self.grant_interaction();
        let phases: Vec<_> = self.peers.iter().filter(|peer| peer.selected).map(|peer| peer.phase).collect();
        if protocol::phase2_ready(&phases) {
            for peer in &mut self.peers {
                if peer.selected && peer.phase == Phase::Phase2Waiting {
                    peer.phase = Phase::Phase2Saving;
                    unsafe { ffi::SmsSaveYourselfPhase2(peer.sms); }
                }
            }
        } else if protocol::die_ready(&phases) {
            for peer in &mut self.peers {
                if peer.selected && peer.phase == Phase::Saved {
                    peer.phase = Phase::Dying;
                    unsafe { ffi::SmsDie(peer.sms); }
                }
            }
        }
        Ok(())
    }

    fn start_pending_saves(&mut self) {
        for peer in &mut self.peers {
            if matches!(peer.phase, Phase::InitialDone | Phase::CancelledDone) {
                unsafe { ffi::SmsSaveComplete(peer.sms); }
                peer.phase = Phase::Idle;
            }
            if peer.phase == Phase::Idle && peer.selected {
                peer.phase = Phase::Saving;
                unsafe { ffi::SmsSaveYourself(peer.sms, 2, 1, 2, 0); }
            } else if peer.phase == Phase::Idle && peer.local_save_requested {
                peer.local_save_requested = false;
                peer.phase = Phase::InitialSave;
                unsafe { ffi::SmsSaveYourself(peer.sms, 1, 0, 0, 0); }
            }
        }
    }

    fn expire_saves(&mut self) -> Result<bool, SessionError> {
        if !self.peers.iter().any(|peer| peer.selected) { return Ok(false); }
        let now = monotonic_usec()?;
        let hard = self.runtime.session()?.shutdown_deadline_usec;
        for peer in &mut self.peers {
            if !peer.selected || matches!(peer.phase, Phase::Dying | Phase::TimedOut | Phase::Closed) { continue; }
            let deadline = hard.map_or(peer.deadline, |hard| peer.deadline.min(hard));
            if now >= deadline {
                if peer.cancel_on_timeout && self.cancellable {
                    self.cancel();
                    let _ = self.notices.send(Notice::Cancelled);
                    return Ok(true);
                }
                peer.phase = Phase::TimedOut;
                peer.interacting = false;
                peer.interaction_requested = false;
            }
        }
        Ok(false)
    }

    fn grant_interaction(&mut self) {
        if self.peers.iter().any(|peer| peer.interacting) { return; }
        if let Some(peer) = self.peers.iter_mut().find(|peer| peer.selected && peer.interaction_requested && peer.phase.saving()) {
            peer.interaction_requested = false;
            peer.interacting = true;
            unsafe { ffi::SmsInteract(peer.sms); }
        }
    }
}
