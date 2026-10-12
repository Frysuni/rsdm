//! Save all first phases before phase 2, and all saves before Die.

use std::{sync::{Arc, atomic::Ordering}};

use rsdm_core::domain::{ShutdownMethod, TimeoutAction};

use super::{Notice, ffi, peer::Peer, protocol::{self, Phase}, server::Server};
use crate::{SessionError, app_stop::ShutdownControl, apps, bus::UserManager,
    processes::{app_processes, monotonic_usec}, runtime::Runtime};

impl Server {
    pub async fn prepare(&mut self, units: &[String], requested: &str, cancellable: bool,
        control: Option<Arc<ShutdownControl>>) -> Result<Vec<String>, SessionError> {
        let records = self.runtime.clone();
        let transaction_id = crate::runtime::blocking(move || Ok(records.session()?.pending_transaction_id().map(str::to_string))).await?;
        if transaction_id.as_deref().unwrap_or("") != requested { return Err(SessionError::State("shutdown transaction is stale".into())); }
        self.control = control;
        if self.cancelled() { self.cancel(); return Ok(Vec::new()); }
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
        self.control = None;
    }

    pub async fn advance(&mut self) -> Result<(), SessionError> {
        self.start_pending_saves().await?;
        if self.cancelled() { self.cancel(); return Ok(()); }
        if self.peers.iter().any(|peer| peer.selected && (peer.cancellation_requested || peer.phase == Phase::Failed)) {
            let failure = self.peers.iter().find(|peer| peer.selected && peer.phase == Phase::Failed).map(|peer| peer.unit.clone());
            if self.can_cancel() {
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
                    if self.control.as_ref().is_some_and(|control| control.cancelled()) { break; }
                    self.manager.actions.check()?;
                    peer.phase = Phase::Phase2Saving;
                    unsafe { ffi::SmsSaveYourselfPhase2(peer.sms); }
                }
            }
        } else if protocol::die_ready(&phases) {
            for peer in &mut self.peers {
                if peer.selected && peer.phase == Phase::Saved {
                    if self.control.as_ref().is_some_and(|control| control.cancelled()) { break; }
                    self.manager.actions.check()?;
                    peer.phase = Phase::Dying;
                    unsafe { ffi::SmsDie(peer.sms); }
                }
            }
        }
        if self.cancelled() { self.cancel(); }
        Ok(())
    }

    fn cancelled(&self) -> bool { self.control.as_ref().is_some_and(|control| control.cancelled()) }

    fn can_cancel(&self) -> bool {
        self.cancellable && !self.control.as_ref().is_some_and(|control| control.noncancelable.load(Ordering::SeqCst))
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
                start_save(&self.manager, &self.runtime, peer, self.transaction_id.as_deref(), self.control.as_deref()).await?;
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
        let cancellable = self.can_cancel();
        for peer in &mut self.peers {
            if !peer.selected || matches!(peer.phase, Phase::Dying | Phase::TimedOut | Phase::Closed) { continue; }
            let deadline = hard.map_or(peer.deadline, |hard| peer.deadline.min(hard));
            if now >= deadline {
                if peer.cancel_on_timeout && cancellable {
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
        if self.cancelled() || self.manager.actions.check().is_err() { return; }
        if self.peers.iter().any(|peer| peer.interacting) { return; }
        if let Some(peer) = self.peers.iter_mut().find(|peer| peer.selected && peer.interaction_requested && peer.phase.saving()) {
            peer.interaction_requested = false;
            peer.interacting = true;
            unsafe { ffi::SmsInteract(peer.sms); }
        }
    }
}

async fn start_save(manager: &UserManager, runtime: &Runtime, peer: &mut Peer,
    transaction_id: Option<&str>, control: Option<&ShutdownControl>) -> Result<(), SessionError> {
    if !peer.verified(manager, runtime).await? { peer.selected = false; return Ok(()); }
    let records = runtime.clone();
    let unit = peer.unit.clone();
    let app = crate::runtime::blocking(move || records.app(&unit)).await?;
    if app.invocation_id != peer.invocation { peer.selected = false; return Ok(()); }

    manager.actions.check_generation_async().await?;
    manager.actions.check()?;
    let state = if control.is_some_and(ShutdownControl::cancelled) {
        peer.selected = false;
        crate::app_dispatch::State::NotSent
    } else {
        peer.phase = Phase::Saving;
        unsafe { ffi::SmsSaveYourself(peer.sms, 2, 1, 2, 0); }
        crate::app_dispatch::State::Sent
    };
    crate::app_dispatch::finish(runtime, &app, transaction_id, crate::app_dispatch::Method::Xsmp, state, &manager.deadline).await
}
