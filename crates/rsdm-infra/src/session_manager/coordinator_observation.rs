//! Apply asynchronous observations to the current session lifecycle.

use std::{thread, time::{Duration, Instant}};

use rsdm_core::domain::SessionPhase;

use super::{
    SessionError,
    coordinator::{Coordinator, Work},
    coordinator_probe::{ObservationResult, BootCheck, observe_external},
    processes::monotonic_usec,
    provider::ProviderKind,
};

impl Coordinator {
    #[cfg(test)]
    pub fn observe(&mut self) -> Result<(), SessionError> {
        let process = self.process.take();
        let result = observe_external(
            &self.manager,
            &self.runtime,
            &self.provider,
            process,
            self.lifecycle.phase == SessionPhase::Running
                && self.last_reap.elapsed() >= Duration::from_secs(1),
            self.lifecycle.phase,
            self.ready_once,
            self.ready_busy,
            self.ready_deadline,
            self.pending_boot,
            self.pending_ready,
            &self.record.anchor_unit,
        );
        self.observation_completed(result)
    }

    pub fn start_observation(&mut self) {
        if self.observation_busy {
            return;
        }
        if super::signals::requested() && self.lifecycle.accepts_launch() {
            if let Err(error) = self.begin_stop("logout", None) {
                tracing::error!(%error, "cannot begin signal-triggered session stop");
            }
        }

        // Keep the process available until the original start worker reports.
        // A retry is observed only after that worker has already completed.
        if self.process.as_ref().is_some_and(|process| process.booting) && !self.pending_boot {
            return;
        }
        // Finish outstanding startup verification, then leave the process with
        // the actor so shutdown preparation can advance and cleanup can own it.
        if !self.lifecycle.accepts_launch() && !self.pending_boot && !self.pending_ready {
            return;
        }

        let reaped_apps = self.lifecycle.phase == SessionPhase::Running
            && self.last_reap.elapsed() >= Duration::from_secs(1);
        let process = self.process.take();
        let manager = self.manager.clone();
        let runtime = self.runtime.clone();
        let provider = self.provider.clone();
        let phase = self.lifecycle.phase;
        let ready_once = self.ready_once;
        let ready_busy = self.ready_busy;
        let ready_deadline = self.ready_deadline;
        let pending_boot = self.pending_boot;
        let pending_ready = self.pending_ready;
        let anchor_unit = self.record.anchor_unit.clone();
        let events = self.events.clone();
        self.observation_busy = true;
        self.workers += 1;

        thread::spawn(move || {
            let result = observe_external(
                &manager,
                &runtime,
                &provider,
                process,
                reaped_apps,
                phase,
                ready_once,
                ready_busy,
                ready_deadline,
                pending_boot,
                pending_ready,
                &anchor_unit,
            );
            let _ = events.send(Work::Observed(result));
        });
    }

    pub fn observation_completed(
        &mut self,
        result: ObservationResult,
    ) -> Result<(), SessionError> {
        self.observation_busy = false;
        self.process = result.process;
        let observation = match result.result {
            Ok(observation) => observation,
            Err(SessionError::Bus(error)) if super::bus::retryable_error(&error) => {
                tracing::warn!(%error, "deferring session observation while the user manager is unavailable");
                return Ok(());
            }
            Err(error) => return Err(error),
        };

        if let Some(boot) = observation.boot {
            match boot {
                BootCheck::Pending => {}
                BootCheck::Discard => {
                    self.pending_boot = false;
                    if let Some(process) = &mut self.process {
                        process.booting = false;
                    }
                }
                BootCheck::Completed(result) => self.boot_completed(result)?,
            }
        }
        if let Some(result) = observation.readiness_check {
            self.complete_readiness(result?)?;
        }

        if observation.reaped_apps {
            self.last_reap = Instant::now();
        }
        if let Some(process) = &self.process
            && self.record.compositor_invocation != process.invocation
        {
            self.record.compositor_invocation = process.invocation.clone();
            self.save()?;
        }
        if let Some(exit) = observation.process_exit
            && !exit.native_running
            && !exit.native_starting
        {
            self.exit_code = exit.code;
            if self.lifecycle.phase != SessionPhase::StoppingSession {
                self.begin_stop(
                    "compositor-exited",
                    Some(monotonic_usec()?.saturating_add(5_000_000)),
                )?;
            }
        }
        if self.ready_once
            && self.lifecycle.phase != SessionPhase::StoppingSession
            && let (Some(anchor_active), Some(session_target_active)) = (
                observation.anchor_active,
                observation.session_target_active,
            )
            && (!anchor_active || !session_target_active)
        {
            self.begin_stop(
                "external-stop",
                Some(monotonic_usec()?.saturating_add(5_000_000)),
            )?;
        }
        if let Some(readiness) = observation.readiness
            && self.lifecycle.phase == SessionPhase::Starting
            && Instant::now() < self.ready_deadline
        {
            if readiness.display_published {
                self.display_since.get_or_insert_with(Instant::now);
            } else {
                self.display_since = None;
            }
            let display_ready = self.display_since.is_some_and(|time| {
                time.elapsed() >= Duration::from_secs(2)
            });
            let native = self.provider.kind != ProviderKind::Managed;
            let ready = if native {
                readiness.target_active && readiness.provider_ready.unwrap_or(false)
            } else {
                readiness.target_active || display_ready
            };
            if ready {
                self.activate();
            }
        }

        // Explicit finalize may have published its environment while this
        // worker owned the process. Retry that deferred activation now.
        if self.lifecycle.phase == SessionPhase::Starting && !self.finalize_replies.is_empty() {
            self.activate();
        }

        while self.queued.front().is_some_and(|(_, _, accepted)| {
            accepted.elapsed() >= Duration::from_secs(10)
        }) {
            let (_, reply, _) = self.queued.pop_front().expect("queued launch");
            let _ = reply.try_send(Err("graphical session did not become ready".into()));
        }
        Ok(())
    }
}
