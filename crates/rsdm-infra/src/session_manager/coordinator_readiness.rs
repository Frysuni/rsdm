//! Activate the session anchor only after compositor readiness is established.

use std::{thread, time::Instant};

#[cfg(test)]
use std::time::Duration;

use rsdm_core::domain::SessionPhase;

use super::{
    SessionError, activation, coordinator::{Coordinator, Work}, processes::monotonic_usec, provider::ProviderKind, units,
};

impl Coordinator {
    #[cfg(test)]
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

    pub fn verify_boot(&mut self) -> Result<(), SessionError> {
        if !self.pending_boot { return Ok(()); }
        if !self.lifecycle.accepts_finalize() {
            self.pending_boot = false;
            if let Some(process) = &mut self.process { process.booting = false; }
            return Ok(());
        }
        let unit = self.process.as_ref().and_then(|process| process.unit.as_deref())
            .ok_or_else(|| SessionError::State("pending compositor start has no unit".into()))?;
        let job = self.manager.unit_property::<(u32, zbus::zvariant::OwnedObjectPath)>(
            unit, "org.freedesktop.systemd1.Unit", "Job",
        );
        match job {
            Ok((pending, _)) if pending != 0 => return Ok(()),
            Err(SessionError::Bus(error)) if super::bus::retryable_error(&error) => return Ok(()),
            Err(error) => return self.boot_completed(Err(error)),
            _ => {},
        }
        let result = match super::processes::generation_invocation(&self.manager, unit, &self.runtime.generation) {
            Err(SessionError::Bus(error)) if super::bus::retryable_error(&error) => return Ok(()),
            Ok(Some(id)) if id.iter().any(|byte| *byte != 0) => Ok(id),
            Ok(_) => Err(SessionError::State("compositor start did not create an invocation".into())),
            Err(error) => Err(error),
        };
        self.boot_completed(result)
    }

    pub fn ready_completed(&mut self, result: Result<bool, SessionError>) -> Result<(), SessionError> {
        match result {
            Ok(owns_targets) => {
                self.record.owns_targets |= owns_targets;
                self.pending_ready = true;
                return self.verify_readiness();
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

    pub fn verify_readiness(&mut self) -> Result<(), SessionError> {
        if !self.pending_ready {
            return Ok(());
        }
        let ready = if self.lifecycle.accepts_finalize() {
            match self.manager.active(&self.record.anchor_unit) {
                Err(SessionError::Bus(error)) if super::bus::retryable_error(&error) => {
                    tracing::warn!(%error, "deferring activation verification while the user manager is unavailable");
                    return Ok(());
                }
                other => other?,
            }
        } else {
            false
        };
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
