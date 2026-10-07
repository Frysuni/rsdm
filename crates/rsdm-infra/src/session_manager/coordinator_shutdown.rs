//! Application preflight precedes native delegation or owned infrastructure stop.

use std::{path::{Path, PathBuf}, sync::{Arc, atomic::Ordering}, thread, time::{Duration, Instant}};

use rsdm_core::domain::{SessionPhase, ShutdownMethod};

use super::{
    SessionError, app_stop::{self, ShutdownControl}, apps, control::StopOutcome,
    coordinator::{Coordinator, Work}, env,
    provider::Provider, runtime::{Runtime, SessionRecord}, session_process::SessionProcess,
    bus::UserManager, units,
};

impl Coordinator {
    pub fn begin_stop(&mut self, action: &str, hard_deadline: Option<u64>) -> Result<(), SessionError> {
        if self.lifecycle.phase == SessionPhase::StoppingSession { return Ok(()); }
        // Only logind may revise its shutdown budget or revoke its request.
        if self.action == "external-shutdown" && action != "external-shutdown" { return Ok(()); }
        if self.shutdown.is_none() {
            self.forced_units.clear();
            self.action = action.to_string();
            self.shutdown = Some(Arc::new(ShutdownControl::default()));
        }
        if let Some(proposed) = hard_deadline {
            let control = self.shutdown.as_ref().expect("shutdown control");
            let old = control.hard_deadline.load(Ordering::SeqCst);
            control.force(if old == 0 { proposed } else { old.min(proposed) });
            self.action = action.to_string();
        }
        self.stopping.store(true, Ordering::SeqCst);
        self.lifecycle.prepare();
        while let Some((_, reply, _)) = self.queued.pop_front() {
            let _ = reply.try_send(Err("session shutdown started before the application was launched".into()));
        }
        if !self.ready_busy && !self.booting() {
            for reply in self.finalize_replies.drain(..) { let _ = reply.try_send(Err("session shutdown started".into())); }
        }
        self.save()
    }

    pub fn advance(&mut self) -> Result<(), SessionError> {
        if self.lifecycle.phase != SessionPhase::Preparing || self.preparing || self.ready_busy
            || self.booting() || self.lifecycle.pending_launches()
        { return Ok(()); }
        self.preparing = true;
        let manager = self.manager.clone();
        let runtime = self.runtime.clone();
        let mut apps = runtime.apps()?;
        if self.provider.native_desktop() && !self.shutdown.as_ref().expect("shutdown control").noncancelable.load(Ordering::SeqCst) {
            apps.retain(|app| app.policy.method != ShutdownMethod::Auto || !app.policy.quit_command.is_empty());
        }
        let control = self.shutdown.as_ref().expect("shutdown control").clone();
        let events = self.events.clone();
        let action = self.action.clone();
        let native = self.provider.native_desktop();
        self.workers += 1;
        thread::spawn(move || {
            let result = (|| {
                if !native && matches!(action.as_str(), "poweroff" | "reboot") { crate::power::check(&action)?; }
                app_stop::prepare_apps(&manager, &runtime, apps, control)
            })();
            let _ = events.send(Work::Prepared(result));
        });
        Ok(())
    }

    pub fn prepared(&mut self, result: Result<Vec<String>, SessionError>) -> Result<(), SessionError> {
        let control = self.shutdown.as_ref().expect("shutdown control");
        match result {
            Ok(forced) => self.forced_units = forced,
            Err(error) => {
                tracing::error!(%error, "application preflight failed");
                if !control.noncancelable.load(Ordering::SeqCst) {
                    self.resume("failed", &error.to_string())?;
                    return Ok(());
                }
            }
        }
        if control.cancelled.load(Ordering::SeqCst) && !control.noncancelable.load(Ordering::SeqCst) {
            self.resume("cancelled", "shutdown cancelled; applications already closed cannot be restored")?;
            return Ok(());
        }
        if self.provider.native_desktop() && matches!(self.action.as_str(), "logout" | "reboot" | "poweroff") {
            self.lifecycle.phase = SessionPhase::StoppingSession;
            let provider = self.provider.clone();
            let manager = self.manager.clone();
            let action = self.action.clone();
            let events = self.events.clone();
            self.workers += 1;
            thread::spawn(move || { let _ = events.send(Work::Delegated(provider.delegate(&manager, &action))); });
            return Ok(());
        }
        if matches!(self.action.as_str(), "reboot" | "poweroff") {
            self.lifecycle.phase = SessionPhase::StoppingSession;
            let action = self.action.clone();
            let events = self.events.clone();
            self.workers += 1;
            thread::spawn(move || { let _ = events.send(Work::PowerRequested(crate::power::request_direct(&action))); });
            return Ok(());
        }
        self.begin_finish()
    }

    pub fn delegated(&mut self, result: Result<(), SessionError>) -> Result<(), SessionError> {
        if self.record.shutdown_deadline_usec.is_some() { return self.begin_finish(); }
        match result {
            Ok(()) => self.resume("delegated", "native desktop manager owns its shutdown dialogs and completion"),
            Err(error) => self.resume("failed", &error.to_string()),
        }
    }

    pub fn power_requested(&mut self, result: Result<(), SessionError>) -> Result<(), SessionError> {
        match result {
            Ok(()) => self.begin_finish(),
            Err(error) if self.record.shutdown_deadline_usec.is_some() => {
                tracing::warn!(%error, "power request failed while external shutdown is in progress");
                self.begin_finish()
            }
            Err(error) => self.resume("failed", &error.to_string()),
        }
    }

    fn begin_finish(&mut self) -> Result<(), SessionError> {
        self.lifecycle.phase = SessionPhase::StoppingSession;
        self.save()?;
        let manager = self.manager.clone();
        let runtime = self.runtime.clone();
        let record = self.record.clone();
        let provider = self.provider.clone();
        let process = self.process.take().expect("session process");
        let directory = self.directory.clone();
        let events = self.events.clone();
        self.workers += 1;
        thread::spawn(move || { let _ = events.send(Work::Finished(finish(&manager, &runtime, &record, &provider, process, directory))); });
        Ok(())
    }

    fn resume(&mut self, result: &str, message: &str) -> Result<(), SessionError> {
        self.xsmp.cancel();
        self.respond_stop(result, message);
        self.lifecycle.phase = if self.manager.active(&self.record.anchor_unit)? { SessionPhase::Running } else { SessionPhase::Starting };
        self.shutdown = None;
        self.record.shutdown_deadline_usec = None;
        self.action.clear();
        self.preparing = false;
        self.stopping.store(false, Ordering::SeqCst);
        apps::release_closed(&self.manager, &self.runtime)?;
        apps::reset_preparation(&self.runtime)?;
        self.save()
    }

    pub fn respond_stop(&mut self, result: &str, message: &str) {
        for reply in self.stop_replies.drain(..) {
            if reply.try_send(Ok(StopOutcome { result: result.into(), forced_units: self.forced_units.clone(), message: message.into() })).is_ok() {
                self.replies_pending += 1;
            }
        }
    }
}

fn finish(
    manager: &UserManager, runtime: &Runtime, record: &SessionRecord, provider: &Provider, mut process: SessionProcess, directory: PathBuf,
) -> Result<(), SessionError> {
    let mut failure = None;
    for app in runtime.apps()? {
        if let Err(error) = apps::finish(manager, runtime, &app) { failure.get_or_insert(error); }
    }
    if !provider.logout_command.is_empty() {
        if let Err(error) = run_logout_command(manager, record, provider, &directory) { failure.get_or_insert(error); }
    }
    if record.owns_targets {
        for target in [units::AUTOSTART_TARGET, units::SESSION_TARGET, units::PRE_TARGET] {
            if let Err(error) = manager.stop(target, Duration::from_secs(15)) { failure.get_or_insert(error); }
        }
    }
    if let Err(error) = manager.stop(&record.anchor_unit, Duration::from_secs(15)) { failure.get_or_insert(error); }
    let _ = manager.unref(&record.anchor_unit);
    if let Err(error) = process.stop(manager, provider, &runtime.generation) { failure.get_or_insert(error); }
    if let Err(error) = env::clear_owned(manager, &record.exported_environment) { failure.get_or_insert(error); }
    match failure { Some(error) => Err(error), None => Ok(()) }
}

fn run_logout_command(manager: &UserManager, record: &SessionRecord, provider: &Provider, directory: &Path) -> Result<(), SessionError> {
    let unit = format!("rsdm-logout-{}.service", record.identity.generation);
    let environment: Vec<_> = manager.environment()?.into_iter().filter_map(|entry| entry.split_once('=')
        .map(|(name, value)| (name.to_string(), value.to_string()))).collect();
    let mut properties = units::command_properties(&provider.logout_command, &environment, directory)?;
    properties.extend([
        ("ExitType", zbus::zvariant::Value::from("cgroup")),
        ("TimeoutStopUSec", zbus::zvariant::Value::from(1_000_000_u64)),
    ]);
    let result = (|| {
        manager.start_service(&unit, &properties)?;
        let deadline = Instant::now() + Duration::from_secs(15);
        while manager.active(&unit)? {
            if Instant::now() >= deadline { return Err(SessionError::State("logout command timed out".into())); }
            thread::sleep(Duration::from_millis(50));
        }
        let status: i32 = manager.unit_property(&unit, "org.freedesktop.systemd1.Service", "ExecMainStatus")?;
        if status != 0 { return Err(SessionError::State(format!("logout command failed with status {status}"))); }
        Ok(())
    })();
    let _ = manager.stop(&unit, Duration::from_secs(3));
    let _ = manager.unref(&unit);
    result
}

#[cfg(test)]
#[path = "coordinator_shutdown_tests.rs"]
mod tests;
