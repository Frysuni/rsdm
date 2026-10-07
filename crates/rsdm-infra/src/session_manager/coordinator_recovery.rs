//! Keep the session lease while draining workers and recovering a failed actor.

use std::{sync::atomic::Ordering, time::{Duration, Instant}};

use super::{control::Request, coordinator::{Coordinator, Work}, processes::monotonic_usec};

impl Coordinator {
    pub fn recover(&mut self) {
        self.stopping.store(true, Ordering::SeqCst);
        self.lifecycle.prepare();
        self.xsmp.cancel();
        let hard_deadline = monotonic_usec().unwrap_or(0).saturating_add(5_000_000);
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

        self.drain_workers();
        if let Err(error) = self.runtime.save_session(&self.record) {
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
        // Jobs have bounded bus/start/stop deadlines. In particular, wait for
        // accepted launches before taking the recovery snapshot.
        let deadline = Instant::now() + Duration::from_secs(90);
        while self.workers > 0 && Instant::now() < deadline {
            while let Ok(request) = self.requests.try_recv() {
                reject(request);
            }
            if let Ok(work) = self.work.recv_timeout(Duration::from_millis(100)) {
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
