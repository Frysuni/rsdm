//! Attached native discovery remains cancellable until the actor authorizes dispatch.

use rsdm_core::domain::SessionPhase;

use super::{SessionError, coordinator::{Coordinator, Work}, native_attached::Scoped, status::Capability};

impl Coordinator {
    pub(super) async fn begin_native_shutdown(&mut self) -> Result<(), SessionError> {
        self.lifecycle.phase = SessionPhase::StoppingSession;
        self.save().await?;
        let provider = self.provider.clone();
        let identity = self.record.identity.clone();
        let manager = self.manager.clone();
        let action = self.action.clone();
        let events = self.events.clone();
        self.workers += 1;
        tokio::spawn(async move {
            let _ = events.send(Work::Delegated(provider.delegate(&manager, &action, &identity).await));
        });
        Ok(())
    }

    pub(super) fn prepare_attached_logout(&mut self, native: Option<Capability>) {
        self.preparing = true;
        let manager = self.manager.clone();
        let identity = self.record.identity.clone();
        let control = self.shutdown.as_ref().expect("shutdown control").clone();
        let events = self.events.clone();
        self.workers += 1;
        tokio::spawn(async move {
            let result = super::native_attached::prepare(&manager, &identity, &control).await;
            let _ = events.send(Work::AttachedReady(result, native));
        });
    }

    pub(super) async fn attached_ready(&mut self, result: Result<Option<Scoped>, SessionError>, native: Option<Capability>) -> Result<(), SessionError> {
        if self.resume_if_cancelled().await? { return Ok(()); }
        let native_preparation = native.as_ref().is_some_and(|capability| capability.state == "available");
        if self.action == "external-shutdown" {
            return if native_preparation { self.prepare_native_remainder().await } else { self.begin_finish().await };
        }
        let unavailable_result = if native_preparation { "failed" } else { "prepared" };
        let unavailable_reason = if native_preparation {
            "native logout ownership changed before delegation; auto applications were left to the desktop"
        } else { "registered applications prepared; no native logout peer belongs to this session" };
        let target = match result {
            Ok(Some(target)) => target,
            Ok(None) => return self.resume(unavailable_result, unavailable_reason).await,
            Err(error) => return self.resume(unavailable_result, &format!("{unavailable_reason}: {error}")).await,
        };
        self.lifecycle.phase = SessionPhase::StoppingSession;
        self.save().await?;
        let manager = self.manager.clone();
        let identity = self.record.identity.clone();
        let runtime = self.runtime.clone();
        let control = self.shutdown.as_ref().expect("shutdown control").clone();
        let events = self.events.clone();
        self.workers += 1;
        tokio::spawn(async move { let _ = events.send(Work::Delegated(target.logout(&manager, &runtime, &identity, &control).await)); });
        Ok(())
    }
}
