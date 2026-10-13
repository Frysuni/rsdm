//! Application preflight precedes native delegation or owned infrastructure stop.

use std::{path::{Path, PathBuf}, sync::{Arc, atomic::Ordering}, time::{Duration, Instant}};

use rsdm_core::domain::SessionPhase;

use super::{
    SessionError, app_stop::ShutdownControl, apps, control::StopOutcome,
    coordinator::{Coordinator, Work}, env,
    provider::Provider, runtime::{Runtime, SessionRecord}, session_process::SessionProcess,
    bus::UserManager, status::Capability, units,
};

impl Coordinator {
    pub async fn begin_stop(&mut self, action: &str, hard_deadline: Option<u64>) -> Result<(), SessionError> {
        if self.lifecycle.phase == SessionPhase::StoppingSession { return Ok(()); }
        // Only logind may revise its shutdown budget or revoke its request.
        if self.action == "external-shutdown" && action != "external-shutdown" { return Ok(()); }
        if self.shutdown.is_none() {
            let transaction = super::shutdown_transaction::ShutdownTransaction::start(&self.runtime, action, hard_deadline).await?;
            self.manager.deadline.set(transaction.deadline_usec);
            self.record.shutdown_deadline_usec = Some(transaction.deadline_usec);
            let transaction_id = transaction.id.clone();
            self.record.shutdown_transaction = Some(transaction);
            self.forced_units.clear();
            self.action = action.to_string();
            let mut control = ShutdownControl::for_manager(&self.manager);
            control.transaction_id = Some(transaction_id);
            self.shutdown = Some(Arc::new(control));
        }
        if let Some(proposed) = hard_deadline {
            let control = self.shutdown.as_ref().expect("shutdown control");
            let old = control.hard_deadline.get();
            control.force(if old == 0 { proposed } else { old.min(proposed) });
            self.record.shutdown_deadline_usec = Some(control.hard_deadline.get());
            if let Some(transaction) = &mut self.record.shutdown_transaction {
                transaction.shorten(control.hard_deadline.get());
            }
            self.action = action.to_string();
        }
        self.stopping.store(true, Ordering::SeqCst);
        self.lifecycle.prepare();
        self.reject_pending_environment("session shutdown started before environment publication");
        while let Some((_, reply, _)) = self.queued.pop_front() {
            let _ = reply.try_send(Err("session shutdown started before the application was launched".into()));
        }
        if !self.ready_busy && !self.booting() {
            for reply in self.finalize_replies.drain(..) { let _ = reply.try_send(Err("session shutdown started".into())); }
        }
        self.save().await
    }

    pub fn advance(&mut self) -> Result<(), SessionError> {
        if self.lifecycle.phase != SessionPhase::Preparing || self.preparing || self.ready_busy
            || self.booting() || self.lifecycle.pending_launches() || self.environment_reply.is_some()
        { return Ok(()); }
        self.preparing = true;
        let manager = self.manager.clone();
        let runtime = self.runtime.clone();
        let control = self.shutdown.as_ref().expect("shutdown control").clone();
        let events = self.events.clone();
        let action = self.action.clone();
        let native = self.provider.native_desktop()
            || (self.provider.kind == super::provider::ProviderKind::Attached && action == "logout");
        let record = self.record.clone();
        self.workers += 1;
        tokio::spawn(async move {
            let capability = if native && !control.noncancelable.load(Ordering::SeqCst) {
                Some(super::coordinator_status::native_logout(&manager, &record).await)
            } else { None };
            let result = super::coordinator_preparation::prepare_applications(
                &manager, &runtime, control, &action, capability.as_ref()).await;
            let _ = events.send(Work::Prepared(result, capability));
        });
        Ok(())
    }

    pub async fn prepared(&mut self, result: Result<Vec<String>, SessionError>, native: Option<Capability>) -> Result<(), SessionError> {
        let control = self.shutdown.as_ref().expect("shutdown control");
        match result {
            Ok(forced) => {
                self.forced_units.extend(forced);
                self.forced_units.sort();
                self.forced_units.dedup();
            }
            Err(error) => {
                tracing::error!(%error, "application preflight failed");
                if !control.noncancelable.load(Ordering::SeqCst) {
                    self.resume("failed", &error.to_string()).await?;
                    return Ok(());
                }
            }
        }
        if self.resume_if_cancelled().await? { return Ok(()); }
        let forced = self.shutdown.as_ref().expect("shutdown control").noncancelable.load(Ordering::SeqCst);
        let native_available = native.as_ref().is_some_and(|capability| capability.state == "available");
        if native_available && forced {
            return self.prepare_native_remainder().await;
        }
        if forced { return self.begin_finish().await; }
        if self.provider.native_desktop() && self.action == "logout" && !native_available {
            let reason = native.as_ref().map_or("Native logout was not verified.", |capability| capability.reason.as_str());
            return self.resume("prepared", &format!("registered applications prepared; native logout unavailable: {reason}")).await;
        }
        if self.provider.native_desktop() && native_available && matches!(self.action.as_str(), "logout" | "reboot" | "poweroff") {
            return self.begin_native_shutdown().await;
        }
        if matches!(self.action.as_str(), "reboot" | "poweroff") {
            self.begin_power_request(native);
            return Ok(());
        }
        if self.provider.kind == super::provider::ProviderKind::Attached && self.action == "logout" {
            self.prepare_attached_logout(native);
            return Ok(());
        }
        self.begin_finish().await
    }

    fn begin_power_request(&mut self, native: Option<Capability>) {
        self.lifecycle.phase = SessionPhase::StoppingSession;
        let action = self.action.clone();
        let events = self.events.clone();
        self.workers += 1;
        let manager = self.manager.clone();
        tokio::spawn(async move {
            let result = manager.actions.guard(manager.deadline.bound(rsdm_system::logind::request_direct(&action))).await
                .and_then(|result| result.map_err(SessionError::from));
            let _ = events.send(Work::PowerRequested(result, native));
        });
    }

    pub async fn delegated(&mut self, result: Result<(), SessionError>) -> Result<(), SessionError> {
        if self.shutdown.as_ref().is_some_and(|control| control.noncancelable.load(Ordering::SeqCst)) {
            return self.prepare_native_remainder().await;
        }
        if self.action == "external-shutdown" { return self.begin_finish().await; }
        match result {
            Ok(()) => self.resume("delegated", "native desktop manager owns its shutdown dialogs and completion").await,
            Err(error) => self.resume("failed", &error.to_string()).await,
        }
    }

    pub async fn power_requested(&mut self, result: Result<(), SessionError>, native: Option<Capability>) -> Result<(), SessionError> {
        match result {
            Ok(()) if native.as_ref().is_some_and(|capability| capability.state != "available") =>
                self.retain_session("logind accepted the power request").await,
            Ok(()) => self.begin_finish().await,
            Err(error) if self.action == "external-shutdown"
                || self.shutdown.as_ref().is_some_and(|control| control.noncancelable.load(Ordering::SeqCst)) => {
                tracing::warn!(%error, "power request failed while external shutdown is in progress");
                self.begin_finish().await
            }
            Err(error) => self.resume("failed", &error.to_string()).await,
        }
    }

    pub(super) async fn begin_teardown(&mut self) -> Result<(), SessionError> {
        self.lifecycle.phase = SessionPhase::StoppingSession;
        self.save().await?;
        let manager = self.manager.clone();
        let runtime = self.runtime.clone();
        let record = self.record.clone();
        let provider = self.provider.clone();
        let mut process = self.process.take().expect("session process");
        let directory = self.directory.clone();
        let events = self.events.clone();
        self.workers += 1;
        tokio::spawn(async move {
            let result = finish(&manager, &runtime, &record, &provider, &mut process, directory).await;
            let _ = events.send(Work::Finished(process, result));
        });
        Ok(())
    }

    pub(super) async fn resume(&mut self, result: &str, message: &str) -> Result<(), SessionError> {
        self.xsmp.cancel();
        self.respond_stop(result, message);
        // Restore the last verified phase. Observation will confirm any real
        // anchor stop once the manager is available again.
        self.lifecycle.phase = if self.ready_once || self.provider.kind == super::provider::ProviderKind::Attached { SessionPhase::Running } else { SessionPhase::Starting };
        self.shutdown = None;
        self.manager.deadline.set(0);
        self.record.shutdown_deadline_usec = None;
        self.action.clear();
        self.preparing = false;
        self.stopping.store(false, Ordering::SeqCst);
        apps::reset_preparation(&self.runtime).await?;
        self.save().await
    }

    pub(super) async fn resume_if_cancelled(&mut self) -> Result<bool, SessionError> {
        let cancelled = self.shutdown.as_ref().is_some_and(|control| {
            control.cancelled.load(Ordering::SeqCst) && !control.noncancelable.load(Ordering::SeqCst)
        });
        if self.lifecycle.phase != SessionPhase::Preparing || !cancelled { return Ok(false); }
        self.resume("cancelled", "shutdown cancelled; applications already closed cannot be restored").await?;
        Ok(true)
    }

    pub fn respond_stop(&mut self, result: &str, message: &str) {
        if let Some(transaction) = &mut self.record.shutdown_transaction {
            transaction.result = Some(result.into());
        }
        for reply in self.stop_replies.drain(..) {
            if reply.try_send(Ok(StopOutcome { result: result.into(), forced_units: self.forced_units.clone(), message: message.into() })).is_ok() {
                self.replies_pending += 1;
            }
        }
    }
}

async fn finish(
    manager: &UserManager, runtime: &Runtime, record: &SessionRecord, provider: &Provider, process: &mut SessionProcess, directory: PathBuf,
) -> Result<(), SessionError> {
    if let Some(unit) = &record.compositor_unit {
        super::processes::verify_unit_invocation(manager, unit, &record.compositor_invocation).await?;
    }

    let mut failure = None;
    let mut cleanup_manager = manager.clone();
    cleanup_manager.deadline = manager.deadline.reserving(Duration::from_secs(1))?;
    let records = runtime.clone();
    for app in super::runtime::blocking(move || records.apps()).await? {
        if let Err(error) = apps::finish(&cleanup_manager, runtime, &app).await { failure.get_or_insert(error); }
    }
    let manager = match &record.compositor_unit {
        Some(unit) => manager.with_owner(unit, &record.compositor_invocation),
        None => manager.clone(),
    };
    if let Some(unit) = &record.compositor_unit {
        cleanup_manager = cleanup_manager.with_owner(unit, &record.compositor_invocation);
    }
    if !provider.logout_command.is_empty() {
        if let Err(error) = run_logout_command(&cleanup_manager, record, provider, &directory).await { failure.get_or_insert(error); }
    }
    if record.owns_targets {
        for target in [units::AUTOSTART_TARGET, units::SESSION_TARGET, units::PRE_TARGET] {
            if let Err(error) = cleanup_manager.stop(target, Duration::from_secs(15)).await { failure.get_or_insert(error); }
        }
    }
    if let Err(error) = cleanup_manager.stop(&record.anchor_unit, Duration::from_secs(15)).await { failure.get_or_insert(error); }
    let _ = cleanup_manager.unref(&record.anchor_unit).await;
    if let Err(error) = process.stop(&manager, provider, &runtime.generation).await { failure.get_or_insert(error); }
    if let Err(error) = env::clear_owned(&manager, &record.exported_environment).await { failure.get_or_insert(error); }
    let records = runtime.clone();
    if let Err(error) = super::runtime::blocking(move || records.reap_closed_app_locks()).await { failure.get_or_insert(error); }
    match failure { Some(error) => Err(error), None => Ok(()) }
}

async fn run_logout_command(manager: &UserManager, record: &SessionRecord, provider: &Provider, directory: &Path) -> Result<(), SessionError> {
    let unit = format!("rsdm-logout-{}.service", record.identity.generation);
    let environment: Vec<_> = manager.environment().await?.into_iter().filter_map(|entry| entry.split_once('=')
        .map(|(name, value)| (name.to_string(), value.to_string()))).collect();
    if !environment.iter().any(|(name, value)| {
        name == super::identity::GENERATION_ENV && value == &record.identity.generation
    }) {
        return Err(SessionError::State("logout command belongs to a stale session generation".into()));
    }
    let mut properties = units::command_properties(&provider.logout_command, &environment, directory).await?;
    // This command may itself stop the graphical target and its anchor. It
    // must survive that stop long enough to return its actual result. Bound
    // its lifetime in systemd as well, including if the coordinator crashes.
    properties.extend([
        ("ExitType", zbus::zvariant::Value::from("cgroup")),
        ("RuntimeMaxUSec", zbus::zvariant::Value::from(15_000_000_u64)),
        ("TimeoutStopUSec", zbus::zvariant::Value::from(1_000_000_u64)),
    ]);
    let result = async {
        manager.start_service(&unit, &properties).await?;
        let deadline = Instant::now() + Duration::from_secs(15);
        while manager.active(&unit).await? {
            if Instant::now() >= deadline { return Err(SessionError::State("logout command timed out".into())); }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let status: i32 = manager.unit_property(&unit, "org.freedesktop.systemd1.Service", "ExecMainStatus").await?;
        if status != 0 { return Err(SessionError::State(format!("logout command failed with status {status}"))); }
        Ok(())
    }.await;
    if !matches!(result, Err(SessionError::StartRejected { .. })) {
        let _ = manager.stop(&unit, Duration::from_secs(3)).await;
        let _ = manager.unref(&unit).await;
    }
    result
}

#[cfg(test)]
#[path = "coordinator_shutdown_tests.rs"]
mod tests;
