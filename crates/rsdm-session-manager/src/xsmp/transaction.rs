//! Save all first phases before phase 2, and all saves before Die.

use rsdm_core::domain::{ShutdownMethod, TimeoutAction};

use super::{Notice, ffi, protocol::{self, Phase}, server::Server};
use crate::{SessionError, apps, processes::{app_processes, monotonic_usec}};

impl Server {
    pub async fn prepare(&mut self, units: &[String], requested: &str, cancellable: bool) -> Result<Vec<String>, SessionError> {
        let records = self.runtime.clone();
        let transaction_id = crate::runtime::blocking(move || Ok(records.session()?.pending_transaction_id().map(str::to_string))).await?;
        if transaction_id.as_deref().unwrap_or("") != requested { return Err(SessionError::State("shutdown transaction is stale".into())); }
        self.transaction_id = transaction_id.clone();
        let mut selected = Vec::new();
        for unit in units {
            if let Some(app) = self.select_application(unit, transaction_id.as_deref()).await? { selected.push(app); }
        }
        self.cancellable = if self.peers.iter().any(|peer| peer.selected) { self.cancellable && cancellable } else { cancellable };
        let records = self.runtime.clone();
        let hard = crate::runtime::blocking(move || Ok(records.session()?.shutdown_deadline_usec)).await?;
        let now = monotonic_usec()?;
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
        self.advance().await?;
        Ok(selected.into_iter().map(|(app, _)| app.unit).collect())
    }

    async fn select_application(&self, unit: &str, transaction_id: Option<&str>) -> Result<Option<(crate::runtime::AppRecord, Vec<usize>)>, SessionError> {
        if !apps::pin_pending(&self.manager, &self.runtime, unit).await? { return Ok(None); }
        let records = self.runtime.clone();
        let app_unit = unit.to_string();
        let app = crate::runtime::blocking(move || records.app(&app_unit)).await?;
        if !app.policy.quit_command.is_empty() || app.policy.method == ShutdownMethod::Term
            || app_processes(&self.manager, &app).await?.is_empty()
        { return Ok(None); }

        let mut peers = Vec::new();
        for (index, peer) in self.peers.iter().enumerate() {
            if peer.unit == unit && peer.verified(&self.manager, &self.runtime).await? { peers.push(index); }
        }
        if peers.is_empty() && app.policy.method == ShutdownMethod::Xsmp {
            return Err(SessionError::State(format!("{unit} has no authenticated XSMP connection")));
        }
        if peers.is_empty() || !crate::app_dispatch::claim(&self.runtime, &app, transaction_id,
            crate::app_dispatch::Method::Xsmp, &self.manager.deadline).await?
        { return Ok(None); }

        let records = self.runtime.clone();
        let app_unit = app.unit.clone();
        let app = crate::runtime::blocking(move || records.app(&app_unit)).await?;
        Ok(Some((app, peers)))
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

    pub async fn advance(&mut self) -> Result<(), SessionError> {
        self.start_pending_saves().await?;
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
        if self.expire_saves().await? { return Ok(()); }
        self.grant_interaction();
        let phases: Vec<_> = self.peers.iter().filter(|peer| peer.selected).map(|peer| peer.phase).collect();
        if protocol::phase2_ready(&phases) {
            for peer in &mut self.peers {
                if peer.selected && peer.phase == Phase::Phase2Waiting {
                    self.manager.actions.check()?;
                    peer.phase = Phase::Phase2Saving;
                    unsafe { ffi::SmsSaveYourselfPhase2(peer.sms); }
                }
            }
        } else if protocol::die_ready(&phases) {
            for peer in &mut self.peers {
                if peer.selected && peer.phase == Phase::Saved {
                    self.manager.actions.check()?;
                    peer.phase = Phase::Dying;
                    unsafe { ffi::SmsDie(peer.sms); }
                }
            }
        }
        Ok(())
    }

    async fn start_pending_saves(&mut self) -> Result<(), SessionError> {
        self.manager.actions.check()?;
        let active = if self.peers.iter().any(|peer| peer.selected) {
            let records = self.runtime.clone();
            let current = crate::runtime::blocking(move || Ok(records.session()?.pending_transaction_id().map(str::to_string))).await?;
            current.as_deref() == self.transaction_id.as_deref()
        } else { false };
        for peer in &mut self.peers {
            self.manager.actions.check()?;
            if matches!(peer.phase, Phase::InitialDone | Phase::CancelledDone) {
                unsafe { ffi::SmsSaveComplete(peer.sms); }
                peer.phase = Phase::Idle;
            }
            if peer.phase == Phase::Idle && peer.selected {
                if !active { peer.selected = false; continue; }
                if !peer.verified(&self.manager, &self.runtime).await? {
                    peer.selected = false;
                    continue;
                }
                let records = self.runtime.clone();
                let unit = peer.unit.clone();
                let app = crate::runtime::blocking(move || records.app(&unit)).await?;
                if app.invocation_id != peer.invocation {
                    peer.selected = false;
                    continue;
                }
                self.manager.actions.check()?;
                peer.phase = Phase::Saving;
                unsafe { ffi::SmsSaveYourself(peer.sms, 2, 1, 2, 0); }
                crate::app_dispatch::finish(&self.runtime, &app, self.transaction_id.as_deref(),
                    crate::app_dispatch::Method::Xsmp, crate::app_dispatch::State::Sent, &self.manager.deadline).await?;
            } else if peer.phase == Phase::Idle && peer.local_save_requested {
                peer.local_save_requested = false;
                peer.phase = Phase::InitialSave;
                unsafe { ffi::SmsSaveYourself(peer.sms, 1, 0, 0, 0); }
            }
        }
        Ok(())
    }

    async fn expire_saves(&mut self) -> Result<bool, SessionError> {
        if !self.peers.iter().any(|peer| peer.selected) { return Ok(false); }
        let records = self.runtime.clone();
        let hard = crate::runtime::blocking(move || Ok(records.session()?.shutdown_deadline_usec)).await?;
        let now = monotonic_usec()?;
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
        if self.manager.actions.check().is_err() { return; }
        if self.peers.iter().any(|peer| peer.interacting) { return; }
        if let Some(peer) = self.peers.iter_mut().find(|peer| peer.selected && peer.interaction_requested && peer.phase.saving()) {
            peer.interaction_requested = false;
            peer.interacting = true;
            unsafe { ffi::SmsInteract(peer.sms); }
        }
    }
}
