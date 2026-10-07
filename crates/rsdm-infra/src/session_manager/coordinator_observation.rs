//! Observe compositor identity and external stops without changing ownership.

use std::time::{Duration, Instant};

use rsdm_core::domain::SessionPhase;

use super::{SessionError, apps, coordinator::Coordinator, provider::ProviderKind, units};

impl Coordinator {
    pub fn observe(&mut self) -> Result<(), SessionError> {
        if self.lifecycle.phase == SessionPhase::Running
            && self.last_reap.elapsed() >= Duration::from_secs(1)
        {
            apps::release_closed(&self.manager, &self.runtime)?;
            self.last_reap = Instant::now();
        }
        if super::signals::requested() && self.lifecycle.accepts_launch() {
            self.begin_stop("logout", false)?;
        }

        self.observe_process()?;
        if self.ready_once && self.lifecycle.phase != SessionPhase::StoppingSession
            && (!self.manager.active(&self.record.anchor_unit)?
                || !self.manager.active(units::SESSION_TARGET)?)
        {
            self.begin_stop("external-stop", true)?;
        }
        self.observe_readiness()?;

        while self.queued.front().is_some_and(|(_, _, accepted)| {
            accepted.elapsed() >= Duration::from_secs(10)
        }) {
            let (_, reply, _) = self.queued.pop_front().expect("queued launch");
            let _ = reply.try_send(Err("graphical session did not become ready".into()));
        }
        Ok(())
    }

    fn observe_process(&mut self) -> Result<(), SessionError> {
        let exited = self.process.as_mut()
            .map(|process| process.exited(&self.manager)).transpose()?.flatten();
        if let Some(code) = exited {
            let detached = self.provider.native_desktop()
                || (self.provider.kind == ProviderKind::External && self.provider.native_unit.is_none());
            let native_running = detached && code == 0 && self.provider.ready(&self.manager)?;
            let native_starting = detached && code == 0
                && self.lifecycle.phase == SessionPhase::Starting
                && Instant::now() < self.ready_deadline;
            if !native_running && !native_starting {
                self.exit_code = code;
                if self.lifecycle.phase != SessionPhase::StoppingSession {
                    self.begin_stop("compositor-exited", true)?;
                }
            }
        }
        if let Some(process) = &self.process {
            if self.record.compositor_invocation != process.invocation {
                self.record.compositor_invocation = process.invocation.clone();
                self.save()?;
            }
        }
        Ok(())
    }
}
