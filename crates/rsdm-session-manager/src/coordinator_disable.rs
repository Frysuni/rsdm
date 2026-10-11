//! Detach session management without tearing down its live dependants.

use std::{sync::atomic::Ordering, time::{Duration, Instant}};

use super::{SessionError, control::{Request, SessionStatus, StopOutcome}, coordinator::{Coordinator, Work}};

impl Coordinator {
    pub(super) async fn disable(&mut self) -> Result<(), SessionError> {
        self.manager.actions.close();
        self.control_bus.close_admission();
        self.stopping.store(true, Ordering::SeqCst);
        if let Some(control) = &self.shutdown {
            control.noncancelable.store(false, Ordering::SeqCst);
            control.cancelled.store(true, Ordering::SeqCst);
        }
        self.xsmp.cancel();
        self.reject_pending_environment("session manager is disabled");
        if let Some(reply) = self.environment_reply.take() { let _ = reply.try_send(Err("session manager is disabled".into())); }
        for reply in self.finalize_replies.drain(..) { let _ = reply.try_send(Err("session manager is disabled".into())); }
        while let Some((_, reply, _)) = self.queued.pop_front() { let _ = reply.try_send(Err("session manager is disabled".into())); }

        let result = if self.preparing || self.lifecycle.phase == rsdm_core::domain::SessionPhase::StoppingSession { "unknown" } else { "disabled" };
        self.respond_stop(result, "session management disabled; previously accepted actions cannot be recalled");
        self.record.disabled = true;
        self.save().await?;
        self._power_monitor.take();
        self.drain_disabled_workers().await;
        self.save().await?;
        if let Err(error) = self.xsmp.shutdown(&super::deadline::Deadline::default()).await {
            tracing::warn!(%error, "disabled XSMP worker shutdown failed");
        }
        Ok(())
    }

    async fn drain_disabled_workers(&mut self) {
        let limit = Instant::now() + Duration::from_secs(2);
        while self.workers > 0 || self.replies_pending > 0 {
            let remaining = limit.saturating_duration_since(Instant::now());
            if remaining.is_zero() { break; }
            tokio::select! {
                request = self.requests.recv() => match request {
                    Some(Request::StopReplySent(permit)) => {
                        self.replies_pending = self.replies_pending.saturating_sub(1);
                        drop(permit);
                    }
                    Some(request) => reject_disabled(request),
                    None => break,
                },
                work = self.work.recv(), if self.workers > 0 => match work {
                    Some(work) => self.disabled_completed(work),
                    None => break,
                },
                _ = tokio::time::sleep(remaining) => break,
            }
        }
    }

    pub(super) fn disabled_completed(&mut self, work: Work) {
        self.workers = self.workers.saturating_sub(1);
        match work {
            Work::Observed(result) => self.process = result.process,
            Work::Finished(process, _) => self.process = Some(process),
            Work::Boot(Ok(id)) => self.record.compositor_invocation = id,
            Work::Boot(Err(SessionError::StartRejected { reference_retained: false, .. })) => {
                if let Some(process) = &self.process { process.submitted.store(false, Ordering::Release); }
            }
            Work::Launched(_, reply, _) => { let _ = reply.try_send(Err("session manager is disabled".into())); }
            _ => {}
        }
    }
}

fn reject_disabled(request: Request) {
    let message = "session manager is disabled".to_string();
    match request {
        Request::Launch(_, reply) => { let _ = reply.try_send(Err(message)); }
        Request::Finalize { reply, .. } | Request::Cancel { reply, .. } => { let _ = reply.try_send(Err(message)); }
        Request::Stop { reply, .. } => { let _ = reply.try_send(Ok(StopOutcome::disabled())); }
        Request::Status(reply) => { let _ = reply.try_send(Ok(SessionStatus::disabled())); }
        Request::XsmpPrepare { reply, .. } => { let _ = reply.try_send(Err(message)); }
        Request::StopReplySent(permit) => drop(permit),
    }
}
