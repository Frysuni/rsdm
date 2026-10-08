//! Activate the session anchor only after compositor readiness is established.

use std::{thread, time::Instant};

use rsdm_core::domain::SessionPhase;

use super::{
    SessionError, activation, coordinator::{Coordinator, Work}, processes::monotonic_usec, provider::ProviderKind, units,
};

impl Coordinator {
    pub fn activate(&mut self) {
        if !self.lifecycle.accepts_finalize() || self.ready_busy || self.booting() {
            return;
        }
        if let Err(error) = self.claim_targets() {
            tracing::error!(%error, "cannot persist graphical target ownership");
            return;
        }
        self.ready_busy = true;
        let manager = self.manager.clone();
        let provider = self.provider.clone();
        let record = self.record.clone();
        let environment = self.environment.clone();
        let directory = self.directory.clone();
        let stopping = self.stopping.clone();
        let events = self.events.clone();
        self.workers += 1;
        thread::spawn(move || {
            let result = activation::activate(
                &manager, &provider, &record.anchor_unit, record.compositor_unit.as_deref(),
                &environment, &directory, &stopping,
            );
            let _ = events.send(Work::Ready(result));
        });
    }

    fn claim_targets(&mut self) -> Result<(), SessionError> {
        if self.provider.kind == ProviderKind::Managed && !self.manager.active(units::SESSION_TARGET)? {
            // Persist ownership intent before the start job can pull in a
            // target, including when the anchor's activation later fails.
            self.record.owns_targets = true;
            self.save()?;
        }
        Ok(())
    }

    pub fn boot_completed(&mut self, result: Result<Vec<u8>, SessionError>) -> Result<(), SessionError> {
        if let Err(SessionError::Bus(error)) = &result {
            if super::bus::retryable_error(error) {
                tracing::warn!(%error, "deferring compositor start verification while the user manager is unavailable");
                self.pending_boot = true;
                return Ok(());
            }
        }
        self.pending_boot = false;
        if let Some(process) = &mut self.process {
            process.booting = false;
            match result {
                Ok(id) => {
                    process.invocation = id.clone();
                    self.record.compositor_invocation = id;
                }
                Err(error) => {
                    tracing::error!(%error, "compositor start failed");
                    self.exit_code = 1;
                }
            }
        }
        if self.exit_code != 0 {
            self.begin_stop("compositor-exited", Some(monotonic_usec()?.saturating_add(5_000_000)))?;
        }
        if !self.finalize_replies.is_empty() && self.lifecycle.accepts_finalize() {
            self.activate();
        }
        Ok(())
    }

    pub fn ready_completed(&mut self, result: Result<bool, SessionError>) -> Result<(), SessionError> {
        match result {
            Ok(owns_targets) => {
                self.record.owns_targets |= owns_targets;
                self.pending_ready = true;
                return Ok(());
            }
            Err(error) => {
                self.ready_busy = false;
                for reply in self.finalize_replies.drain(..) {
                    let _ = reply.try_send(Err(error.to_string()));
                }
                tracing::warn!(%error, "session readiness failed");
                self.ready_deadline = Instant::now();
            }
        }
        Ok(())
    }

    pub(super) fn complete_readiness(&mut self, ready: bool) -> Result<(), SessionError> {
        let ready = ready && self.lifecycle.accepts_finalize();
        self.pending_ready = false;
        self.ready_busy = false;
        if ready {
            self.ready_once = true;
            self.lifecycle.ready();
        }
        for reply in self.finalize_replies.drain(..) {
            let result = if ready { Ok(()) } else { Err("session shutdown interrupted activation".into()) };
            let _ = reply.try_send(result);
        }
        if self.lifecycle.phase == SessionPhase::Running {
            while let Some((request, reply, _)) = self.queued.pop_front() {
                self.launch(request, reply)?;
            }
        }
        self.save()
    }
}
