//! Activate the session anchor only after compositor readiness is established.

use std::{thread, time::{Duration, Instant}};

use rsdm_core::domain::SessionPhase;

use super::{
    SessionError, activation, coordinator::{Coordinator, Work}, provider::ProviderKind, units,
};

impl Coordinator {
    pub fn observe_readiness(&mut self) -> Result<(), SessionError> {
        if self.lifecycle.phase != SessionPhase::Starting || self.ready_busy || self.booting()
            || Instant::now() >= self.ready_deadline
        {
            return Ok(());
        }
        let target = self.manager.active(units::SESSION_TARGET)?;
        let native = self.provider.kind != ProviderKind::Managed;
        if activation::published_display(&self.manager)? {
            self.display_since.get_or_insert_with(Instant::now);
        } else {
            self.display_since = None;
        }
        let display_ready = self.display_since.is_some_and(|time| {
            time.elapsed() >= Duration::from_secs(2)
        });
        let ready = if native {
            target && self.provider.ready(&self.manager)?
        } else {
            target || display_ready
        };
        if ready {
            self.activate();
        }
        Ok(())
    }

    pub fn activate(&mut self) {
        if self.ready_busy || self.booting() {
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
            self.begin_stop("compositor-exited", true)?;
        }
        if !self.finalize_replies.is_empty() && self.lifecycle.accepts_finalize() {
            self.activate();
        }
        Ok(())
    }

    pub fn ready_completed(&mut self, result: Result<bool, SessionError>) -> Result<(), SessionError> {
        self.ready_busy = false;
        match result {
            Ok(owns_targets) => {
                self.record.owns_targets |= owns_targets;
                let ready = self.manager.active(&self.record.anchor_unit)? && self.lifecycle.accepts_finalize();
                if ready {
                    self.ready_once = true;
                    self.lifecycle.ready();
                }
                for reply in self.finalize_replies.drain(..) {
                    let result = if ready {
                        Ok(())
                    } else {
                        Err("session shutdown interrupted activation".into())
                    };
                    let _ = reply.try_send(result);
                }
            }
            Err(error) => {
                for reply in self.finalize_replies.drain(..) {
                    let _ = reply.try_send(Err(error.to_string()));
                }
                tracing::warn!(%error, "session readiness failed");
                self.ready_deadline = Instant::now();
            }
        }
        if self.lifecycle.phase == SessionPhase::Running {
            while let Some((request, reply, _)) = self.queued.pop_front() {
                self.launch(request, reply)?;
            }
        }
        Ok(())
    }
}
