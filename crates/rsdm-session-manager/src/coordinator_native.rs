//! Attached native discovery remains cancellable until the actor authorizes dispatch.

use rsdm_core::domain::SessionPhase;

use super::{SessionError, coordinator::{Coordinator, Work}, native_attached::Scoped};

impl Coordinator {
    pub(super) fn prepare_attached_logout(&mut self) {
        self.preparing = true;
        let manager = self.manager.clone();
        let identity = self.record.identity.clone();
        let control = self.shutdown.as_ref().expect("shutdown control").clone();
        let events = self.events.clone();
        self.workers += 1;
        tokio::spawn(async move {
            let result = super::native_attached::prepare(&manager, &identity, &control).await;
            let _ = events.send(Work::AttachedReady(result));
        });
    }

    pub(super) async fn attached_ready(&mut self, result: Result<Option<Scoped>, SessionError>) -> Result<(), SessionError> {
        if self.resume_if_cancelled().await? { return Ok(()); }
        if self.action == "external-shutdown" { return self.begin_finish().await; }
        let target = match result {
            Ok(Some(target)) => target,
            Ok(None) => return self.resume("prepared", "registered applications prepared; no native logout peer belongs to this session").await,
            Err(error) => return self.resume("prepared", &format!("registered applications prepared; native logout unavailable: {error}")).await,
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
