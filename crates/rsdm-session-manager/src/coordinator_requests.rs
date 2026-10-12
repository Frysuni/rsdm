//! Requests admitted by the coordinator's current lifecycle state.

use std::{sync::atomic::Ordering, time::Instant};

use rsdm_core::domain::SessionPhase;

use super::{
    SessionError, apps,
    control::{LaunchRequest, Reply, Request, StopOutcome},
    coordinator::{Coordinator, Work, MAX_COORDINATOR_WORKERS},
};

impl Coordinator {
    pub async fn request(&mut self, request: Request) -> Result<(), SessionError> {
        match request {
            Request::Launch(request, reply) => self.launch(request, reply).await?,
            Request::Finalize { generation, environment, reply } => {
                self.finalize_request(generation, environment, reply).await?;
            }
            Request::Stop { generation, action, reply } => {
                self.stop_request(generation, action, reply).await?;
            }
            Request::Cancel { generation, reply } => {
                let _ = reply.try_send(self.cancel_request(&generation));
            }
            Request::Status(reply) => {
                let _ = reply.try_send(Ok(self.status().await?));
            }
            Request::StatusDetails(reply) => self.request_status_details(reply),
            Request::StopReplySent(permit) => {
                self.replies_pending = self.replies_pending.saturating_sub(1);
                drop(permit);
            }
            Request::XsmpPrepare { generation, transaction_id, units, cancellable, reply } => {
                if generation != self.runtime.generation || self.lifecycle.phase == SessionPhase::Closed {
                    let _ = reply.try_send(Err("session generation is stale or closed".into()));
                } else {
                    let allowed = cancellable && self.lifecycle.phase == SessionPhase::Preparing
                        && self.shutdown.as_ref().is_some_and(|control| {
                            !control.noncancelable.load(Ordering::SeqCst)
                        });
                    let current = self.record.pending_transaction_id().unwrap_or("");
                    if current != transaction_id {
                        let _ = reply.try_send(Err("shutdown transaction is stale".into()));
                    } else if self.shutdown.as_ref().is_some_and(|control| control.cancelled()) {
                        let _ = reply.try_send(Ok(Vec::new()));
                    } else {
                        self.xsmp.prepare(units, transaction_id, allowed, self.shutdown.clone(), reply);
                    }
                }
            }
        }
        Ok(())
    }

    async fn stop_request(
        &mut self,
        generation: String,
        action: String,
        reply: Reply<StopOutcome>,
    ) -> Result<(), SessionError> {
        if generation != self.runtime.generation
            || !matches!(action.as_str(), "logout" | "reboot" | "poweroff")
        {
            let _ = reply.try_send(Err("invalid session generation or shutdown action".into()));
            return Ok(());
        }
        if self.lifecycle.phase == SessionPhase::Closed {
            let _ = reply.try_send(Err("session has already stopped".into()));
        } else if !self.action.is_empty() && self.action != action {
            let _ = reply.try_send(Err("another shutdown action is already in progress".into()));
        } else {
            self.stop_replies.push(reply);
            self.begin_stop(&action, None).await?;
        }
        Ok(())
    }

    fn cancel_request(&mut self, generation: &str) -> Result<(), String> {
        if generation != self.runtime.generation {
            return Err("session generation is stale".into());
        }
        if self.lifecycle.phase != SessionPhase::Preparing {
            return Err("session is not in cancellable preparation".into());
        }
        let control = self.shutdown.as_ref()
            .ok_or_else(|| "session preparation has not started".to_string())?;
        if control.noncancelable.load(Ordering::SeqCst) {
            return Err("external shutdown cannot be cancelled".into());
        }

        control.cancelled.store(true, Ordering::SeqCst);
        self.xsmp.cancel();
        Ok(())
    }

    pub async fn launch(&mut self, request: LaunchRequest, reply: Reply<String>) -> Result<(), SessionError> {
        if request.generation != self.runtime.generation || !self.lifecycle.accepts_launch() {
            let _ = reply.try_send(Err("session is shutting down or generation is stale".into()));
            return Ok(());
        }
        if self.workers >= MAX_COORDINATOR_WORKERS {
            let _ = reply.try_send(Err("session coordinator is busy; retry the launch".into()));
            return Ok(());
        }
        if self.lifecycle.phase == SessionPhase::Starting {
            self.queued.push_back((request, reply, Instant::now()));
            return Ok(());
        }
        let app = match apps::register(&self.runtime, &self.provider, &request).await {
            Ok(app) => app,
            Err(error) => {
                let _ = reply.try_send(Err(error.to_string()));
                return Ok(());
            }
        };
        self.lifecycle.register_launch(app.unit.clone());
        let manager = self.manager.clone();
        let runtime = self.runtime.clone();
        let anchor = self.record.anchor_unit.clone();
        let config_path = self.record.config_path.clone();
        let events = self.events.clone();
        self.workers += 1;
        tokio::spawn(async move {
            let unit = app.unit.clone();
            let result = apps::launch(&manager, &runtime, &anchor, &request, app, config_path.as_deref()).await;
            let _ = events.send(Work::Launched(unit, reply, result));
        });
        Ok(())
    }
}
