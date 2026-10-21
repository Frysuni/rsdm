//! Serialize environment publication while lifecycle decisions stay responsive.

use rsdm_core::domain::SessionPhase;

use super::{SessionError, activation, control::Reply, coordinator::{Coordinator, Work}, env};

impl Coordinator {
    pub(super) async fn finalize_request(
        &mut self, generation: String, mut environment: Vec<(String, String)>, reply: Reply<()>,
    ) -> Result<(), SessionError> {
        if generation != self.runtime.generation || !self.lifecycle.accepts_finalize() {
            let _ = reply.try_send(Err("session is shutting down or generation is stale".into()));
            return Ok(());
        }
        if self.provider.kind == super::provider::ProviderKind::Attached {
            environment.retain(|(name, _)| !matches!(name.as_str(),
                "SESSION_MANAGER" | "ICEAUTHORITY" | "RSDM_SESSION_GENERATION" | "XDG_SESSION_ID"));
        }
        // The endpoint's admission permit bounds queued and active requests.
        self.pending_environment.push_back((environment, reply));
        self.publish_next_environment().await
    }

    pub(super) async fn publish_next_environment(&mut self) -> Result<(), SessionError> {
        if self.environment_reply.is_some() || !self.lifecycle.accepts_finalize() { return Ok(()); }
        let Some((environment, reply)) = self.pending_environment.pop_front() else { return Ok(()); };
        let recorded = async {
            let previous = activation::claim_environment(&mut self.record, &environment)?;
            if self.provider.kind != super::provider::ProviderKind::Attached {
                for pair in previous {
                    if !self.record.previous_environment.contains(&pair) { self.record.previous_environment.push(pair); }
                }
            }
            self.save().await
        }.await;
        if let Err(error) = recorded {
            let _ = reply.try_send(Err(error.to_string()));
            return Ok(());
        }
        if self.provider.kind == super::provider::ProviderKind::Attached {
            for (name, value) in environment {
                self.environment.retain(|(existing, _)| existing != &name);
                self.environment.push((name, value));
            }
            let _ = reply.try_send(Ok(()));
            return Ok(());
        }
        // Record ownership before either manager or activation values can be
        // changed. Only the actor writes the session record, including phase.
        let names = environment.iter().map(|(name, _)| name.clone()).collect();
        self.environment_reply = Some((names, reply));
        let manager = self.manager.clone();
        let events = self.events.clone();
        self.workers += 1;
        tokio::spawn(async move {
            let result = env::publish(&manager, &environment).await;
            let _ = events.send(Work::EnvironmentPublished(result));
        });
        Ok(())
    }

    pub(super) async fn environment_published(&mut self, result: Result<(), SessionError>) -> Result<(), SessionError> {
        let (names, reply) = self.environment_reply.take()
            .ok_or_else(|| SessionError::State("environment publication completed without a request".into()))?;
        if result.is_ok() {
            self.record.previous_environment.retain(|(name, _)| !names.contains(name));
        }
        // No app preparation can start while this publication is outstanding.
        // Honour cancellation before replying, so an accepted startup finalize
        // does not fail and cause its launcher to exit after a cancelled logout.
        self.resume_if_cancelled().await?;
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
        self.publish_next_environment().await
    }

    pub(super) fn reject_pending_environment(&mut self, message: &str) {
        for (_, reply) in self.pending_environment.drain(..) {
            let _ = reply.try_send(Err(message.into()));
        }
    }
}
