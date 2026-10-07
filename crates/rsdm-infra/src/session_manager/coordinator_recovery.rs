//! Keep the session lease while draining workers and recovering a failed actor.

use std::{sync::atomic::Ordering, time::Duration};

use super::{control::Request, coordinator::{Coordinator, Work}};

impl Coordinator {
    pub fn recover(&mut self) {
        self.stopping.store(true, Ordering::SeqCst);
        self.lifecycle.prepare();
        self.xsmp.cancel();
        self.reject_pending_environment("session coordinator failed");
        if let Some(reply) = self.environment_reply.take() {
            let _ = reply.try_send(Err("session coordinator failed".into()));
        }
        let hard_deadline = super::cleanup::recovery_deadline(
            self.record.shutdown_deadline_usec, self.manager.deadline.get(),
        ).unwrap_or_else(|error| {
            tracing::error!(%error, "cannot establish the recovery deadline");
            1
        });
        self.manager.deadline.set(hard_deadline);
        self.record.shutdown_deadline_usec = Some(hard_deadline);
        if let Some(control) = &self.shutdown {
            control.force(hard_deadline);
            control.recovery.store(true, Ordering::SeqCst);
        }
        for reply in self.finalize_replies.drain(..) {
            let _ = reply.try_send(Err("session coordinator failed".into()));
        }
        while let Some((_, reply, _)) = self.queued.pop_front() {
            let _ = reply.try_send(Err("session coordinator failed".into()));
        }
        self.respond_stop("failed", "session coordinator failed; recovering recorded invocations");

        // Publish the budget before waiting; workers may already have accepted
        // mutations whose invocation will be recovered from the saved intent.
        if let Err(error) = self.save() {
            tracing::warn!(%error, "could not persist recovery deadline");
        }
        self.drain_workers();
        if let Err(error) = self.save() {
            tracing::warn!(%error, "could not persist recovery state");
        }
        if let Err(error) = super::cleanup::recover(&self.manager, &self.runtime) {
            tracing::error!(%error, "session recovery failed");
        }
        if let Some(process) = &mut self.process {
            if let Err(error) = process.stop(&self.manager, &self.provider, &self.runtime.generation) {
                tracing::error!(%error, "session launcher cleanup failed");
            }
        }
    }

    fn drain_workers(&mut self) {
        // Drain accepted work only inside the same budget as recovery. Its
        // ownership intent remains recorded when completion is unconfirmed.
        while self.workers > 0 && self.manager.deadline.remaining(Duration::from_millis(100)).is_ok() {
            while let Ok(request) = self.requests.try_recv() {
                reject(request);
                if self.manager.deadline.remaining(Duration::from_millis(100)).is_err() { break; }
            }
            let Ok(wait) = self.manager.deadline.remaining(Duration::from_millis(100)) else { break; };
            if let Ok(work) = self.work.recv_timeout(wait) {
                self.workers = self.workers.saturating_sub(1);
                match work {
                    Work::Boot(Ok(id)) => self.record.compositor_invocation = id,
                    Work::Ready(Ok(owns)) => self.record.owns_targets |= owns,
                    Work::Launched(_, reply, _) => {
                        let _ = reply.try_send(Err("session coordinator failed".into()));
                    }
                    _ => {}
                }
            }
        }
        if self.workers > 0 {
            tracing::error!(workers = self.workers, "session workers did not drain before recovery");
        }
    }
}

fn reject(request: Request) {
    let message = "session coordinator failed".to_string();
    match request {
        Request::Launch(_, reply) => { let _ = reply.try_send(Err(message)); }
        Request::Finalize { reply, .. } | Request::Cancel { reply, .. } => { let _ = reply.try_send(Err(message)); }
        Request::Stop { reply, .. } => { let _ = reply.try_send(Err(message)); }
        Request::Status(reply) => { let _ = reply.try_send(Err(message)); }
        Request::XsmpPrepare { reply, .. } => { let _ = reply.try_send(Err(message)); }
        Request::StopReplySent(permit) => drop(permit),
    }
}
