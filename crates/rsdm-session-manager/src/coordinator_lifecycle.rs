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
        if self.action != "logout" && !self.provider.native_desktop() { return self.begin_teardown().await; }

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
        if self.action != "logout" && !self.provider.native_desktop() { return self.begin_finish().await; }
        if capability.state != "available" {
            if self.action != "logout" { return self.retain_session(&capability.reason).await; }
            return self.resume("prepared", &format!("registered applications prepared; session infrastructure retained: {}", capability.reason)).await;
        }
        self.begin_teardown().await
    }

    pub(super) async fn retain_session(&mut self, reason: &str) -> Result<(), SessionError> {
        // The owning start call must keep PAM open even after the coordinator
        // releases its inhibitor and control endpoint.
        self.retain_launcher = true;
        self.respond_stop("prepared", &format!("registered applications prepared; session infrastructure retained: {reason}"));
        self.lifecycle.phase = SessionPhase::Closed;
        self.save().await
    }
}

pub(super) async fn wait_retained(
    mut coordinator: Coordinator, launcher: SessionProcess, result: Result<i32, SessionError>,
) -> Result<i32, SessionError> {
    coordinator.manager.actions.close();
    coordinator.control_bus.close_admission();
    if let Err(error) = coordinator.xsmp.shutdown(&coordinator.manager.deadline).await {
        tracing::warn!(%error, "XSMP shutdown failed while retaining the session launcher");
    }
    let process = coordinator.process.take().unwrap_or(launcher);
    let record = coordinator.record.clone();
    let provider = coordinator.provider.clone();
    let connection = coordinator.manager.connection();
    drop(coordinator);

    if process.child.is_none() && !process.submitted.load(std::sync::atomic::Ordering::Acquire) {
        return result;
    }
    let exit = super::session_wait::wait(process, &record, &provider, connection).await;
    match result {
        Err(error) => Err(error),
        Ok(code) if code != 0 => exit.map(|_| code),
        Ok(_) => exit,
    }
}
