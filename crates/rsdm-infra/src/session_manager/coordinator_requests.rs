//! Requests admitted by the coordinator's current lifecycle state.

use std::{sync::atomic::Ordering, thread, time::Instant};

use rsdm_core::domain::{SessionPhase, ShutdownMethod};

use super::{
    SessionError, apps,
    control::{LaunchRequest, Reply, Request, SessionStatus, StopOutcome},
    coordinator::{Coordinator, Work},
    runtime::AppRecord,
};

impl Coordinator {
    pub fn request(&mut self, request: Request) -> Result<(), SessionError> {
        match request {
            Request::Launch(request, reply) => self.launch(request, reply)?,
            Request::Finalize { generation, environment, reply } => {
                self.finalize_request(generation, environment, reply)?;
            }
            Request::Stop { generation, action, reply } => {
                self.stop_request(generation, action, reply)?;
            }
            Request::Cancel { generation, reply } => {
                let _ = reply.try_send(self.cancel_request(&generation));
            }
            Request::Status(reply) => {
                let _ = reply.try_send(Ok(self.status()?));
            }
            Request::StopReplySent(permit) => {
                self.replies_pending = self.replies_pending.saturating_sub(1);
                drop(permit);
            }
            Request::XsmpPrepare { generation, units, cancellable, reply } => {
                if generation != self.runtime.generation || self.lifecycle.phase == SessionPhase::Closed {
                    let _ = reply.try_send(Err("session generation is stale or closed".into()));
                } else {
                    let allowed = cancellable && self.lifecycle.phase == SessionPhase::Preparing
                        && self.shutdown.as_ref().is_some_and(|control| {
                            !control.noncancelable.load(Ordering::SeqCst)
                        });
                    self.xsmp.prepare(units, allowed, reply);
                }
            }
        }
        Ok(())
    }

    fn stop_request(
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
            self.begin_stop(&action, None)?;
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

    fn status(&self) -> Result<SessionStatus, SessionError> {
        let apps = self.runtime.apps()?.into_iter().map(|app| {
            let method = self.app_method(&app);
            let state = if app.quit_started { "quitting" } else { "registered" };
            (app.unit, method, app.policy.timeout_secs, state.to_string())
        }).collect();
        Ok(SessionStatus {
            generation: self.runtime.generation.clone(),
            login_session_id: self.record.identity.login_session_id.clone(),
            desktop_entry_id: self.record.identity.desktop_entry_id.clone().unwrap_or_default(),
            provider: self.provider.name().into(),
            phase: self.lifecycle.phase.to_string(),
            xsmp_available: self.xsmp.available(),
            apps,
        })
    }

    fn app_method(&self, app: &AppRecord) -> String {
        if !app.policy.quit_command.is_empty() {
            return "command".into();
        }
        if app.policy.method != ShutdownMethod::Auto {
            return app.policy.method.to_string();
        }
        if self.provider.native_desktop() {
            return "native".into();
        }
        if self.xsmp.connected.contains(&app.unit) {
            return "xsmp".into();
        }
        "term".into()
    }

    pub fn launch(&mut self, request: LaunchRequest, reply: Reply<String>) -> Result<(), SessionError> {
        if request.generation != self.runtime.generation || !self.lifecycle.accepts_launch() {
            let _ = reply.try_send(Err("session is shutting down or generation is stale".into()));
            return Ok(());
        }
        if self.lifecycle.phase == SessionPhase::Starting {
            self.queued.push_back((request, reply, Instant::now()));
            return Ok(());
        }
        let app = match apps::register(&self.runtime, &self.provider, &request) {
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
        let events = self.events.clone();
        self.workers += 1;
        thread::spawn(move || {
            let unit = app.unit.clone();
            let result = apps::launch(&manager, &runtime, &anchor, &request, app);
            let _ = events.send(Work::Launched(unit, reply, result));
        });
        Ok(())
    }
}
