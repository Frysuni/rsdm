//! The actor authorizes infrastructure teardown after a cancellable ownership probe.

use rsdm_core::domain::SessionPhase;

use super::{SessionError, coordinator::{Coordinator, Work}, provider::ProviderKind,
    session_process::SessionProcess, status::Capability};

impl Coordinator {
    pub(super) async fn begin_finish(&mut self) -> Result<(), SessionError> {
        if self.provider.kind == ProviderKind::Attached {
            self.respond_stop("prepared", "registered applications prepared; external session infrastructure is retained");
            self.lifecycle.phase = SessionPhase::Closed;
            return self.save().await;
        }
        if self.action != "logout" { return self.begin_teardown().await; }

        self.preparing = true;
        let manager = self.manager.clone();
        let record = self.record.clone();
        let process = self.process.take().expect("session process");
        let events = self.events.clone();
        self.workers += 1;
        tokio::spawn(async move {
            let (_, lifecycle) = super::coordinator_status::lifecycle_capabilities(&manager, &record).await;
            let _ = events.send(Work::LifecycleChecked(process, lifecycle));
        });
        Ok(())
    }

    pub(super) async fn lifecycle_checked(&mut self, process: SessionProcess, capability: Capability) -> Result<(), SessionError> {
        self.process = Some(process);
        if self.resume_if_cancelled().await? { return Ok(()); }
        if self.action != "logout" { return self.begin_finish().await; }
        if capability.state != "available" {
            return self.resume("prepared", &format!("registered applications prepared; session infrastructure retained: {}", capability.reason)).await;
        }
        self.begin_teardown().await
    }
}
