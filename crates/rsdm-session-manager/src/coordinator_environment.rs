//! Serialize environment publication while lifecycle decisions stay responsive.

use std::thread;

use rsdm_core::domain::SessionPhase;

use super::{SessionError, activation, control::Reply, coordinator::{Coordinator, Work}, env};

impl Coordinator {
    pub(super) fn finalize_request(
        &mut self, generation: String, environment: Vec<(String, String)>, reply: Reply<()>,
    ) -> Result<(), SessionError> {
        if generation != self.runtime.generation || !self.lifecycle.accepts_finalize() {
            let _ = reply.try_send(Err("session is shutting down or generation is stale".into()));
            return Ok(());
        }
        // The endpoint's admission permit bounds queued and active requests.
        self.pending_environment.push_back((environment, reply));
        self.publish_next_environment()
    }

    pub(super) fn publish_next_environment(&mut self) -> Result<(), SessionError> {
        if self.environment_reply.is_some() || !self.lifecycle.accepts_finalize() { return Ok(()); }
        let Some((environment, reply)) = self.pending_environment.pop_front() else { return Ok(()); };
        let recorded = activation::claim_environment(&mut self.record, &environment).and_then(|()| self.save());
        if let Err(error) = recorded {
            let _ = reply.try_send(Err(error.to_string()));
            return Ok(());
        }
        // Record ownership before either manager or activation values can be
        // changed. Only the actor writes the session record, including phase.
        self.environment_reply = Some(reply);
        let manager = self.manager.clone();
        let events = self.events.clone();
        self.workers += 1;
        thread::spawn(move || {
            let _ = events.send(Work::EnvironmentPublished(env::publish(&manager, &environment)));
        });
        Ok(())
    }

    pub(super) fn environment_published(&mut self, result: Result<(), SessionError>) -> Result<(), SessionError> {
        let reply = self.environment_reply.take()
            .ok_or_else(|| SessionError::State("environment publication completed without a request".into()))?;
        // No app preparation can start while this publication is outstanding.
        // Honour cancellation before replying, so an accepted startup finalize
        // does not fail and cause its launcher to exit after a cancelled logout.
        self.resume_if_cancelled()?;
        match result {
            Err(error) => { let _ = reply.try_send(Err(error.to_string())); }
            Ok(()) if !self.lifecycle.accepts_finalize() => {
                let _ = reply.try_send(Err("session shutdown interrupted environment publication".into()));
            }
            Ok(()) if self.lifecycle.phase == SessionPhase::Running => { let _ = reply.try_send(Ok(())); }
            Ok(()) => {
                self.finalize_replies.push(reply);
                self.activate();
            }
        }
        self.publish_next_environment()
    }

    pub(super) fn reject_pending_environment(&mut self, message: &str) {
        for (_, reply) in self.pending_environment.drain(..) {
            let _ = reply.try_send(Err(message.into()));
        }
    }
}
