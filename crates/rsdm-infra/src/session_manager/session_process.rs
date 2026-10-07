//! Observing a managed compositor or an unchanged native session launcher.

use std::{os::unix::process::ExitStatusExt, path::Path, process::{Child, Command, ExitStatus}, time::{Duration, Instant}};

use super::{SessionError, bus::UserManager, deadline::Deadline, processes::ProcessHandle, provider::{Provider, ProviderKind}};

pub(super) struct SessionProcess {
    pub unit: Option<String>,
    pub invocation: Vec<u8>,
    child: Option<Child>,
    pub booting: bool,
    child_exit: Option<i32>,
    native_unit_seen: bool,
    launched: std::time::Instant,
}

fn exit_status_code(status: ExitStatus) -> i32 {
    status.code().or_else(|| status.signal().map(|signal| 128 + signal)).unwrap_or(1)
}

impl SessionProcess {
    pub fn launch(
        argv: &[String], environment: &[(String, String)], directory: &Path,
        provider: &Provider, managed_unit: String,
    ) -> Result<Self, SessionError> {
        if provider.kind == ProviderKind::Managed {
            return Ok(Self { unit: Some(managed_unit), invocation: Vec::new(), child: None, booting: true,
                child_exit: None, native_unit_seen: false, launched: std::time::Instant::now() });
        }
        let child = Command::new(argv.first().ok_or(SessionError::EmptyCommand)?)
            .args(&argv[1..]).env_clear().envs(environment.iter().map(|(key, value)| (key, value)))
            .current_dir(directory).spawn().map_err(SessionError::Spawn)?;
        Ok(Self { unit: provider.native_unit.clone(), invocation: Vec::new(), child: Some(child), booting: false,
            child_exit: None, native_unit_seen: false, launched: std::time::Instant::now() })
    }

    pub fn exited(&mut self, manager: &UserManager) -> Result<Option<i32>, SessionError> {
        if self.booting { return Ok(None); }
        if let Some(child) = &mut self.child {
            if self.child_exit.is_none() { self.child_exit = child.try_wait().map_err(SessionError::Wait)?.map(exit_status_code); }
            let Some(unit) = &self.unit else { return Ok(self.child_exit); };
            if manager.active(unit)? {
                self.native_unit_seen = true;
                if self.invocation.is_empty() { self.invocation = manager.invocation_id(unit)?; }
                return Ok(None);
            }
            if self.native_unit_seen || self.launched.elapsed() > Duration::from_secs(10) {
                return Ok(self.child_exit.or_else(|| self.native_unit_seen.then_some(0)));
            }
            return Ok(None);
        }
        let Some(unit) = &self.unit else { return Ok(None); };
        if manager.active(unit)? { return Ok(None); }
        let code: i32 = manager.unit_property(unit, "org.freedesktop.systemd1.Service", "ExecMainCode")?;
        let status: i32 = manager.unit_property(unit, "org.freedesktop.systemd1.Service", "ExecMainStatus")?;
        Ok(Some(if code == libc::CLD_EXITED { status } else { 128 + status }))
    }

    pub fn stop(&mut self, manager: &UserManager, provider: &Provider, generation: &str) -> Result<(), SessionError> {
        let mut failure = None;
        if let Some(unit) = &self.unit {
            let result = (|| {
                if self.invocation.is_empty() && provider.kind == ProviderKind::Managed {
                    self.invocation = super::processes::generation_invocation(manager, unit, generation)?.unwrap_or_default();
                }
                super::processes::stop_invocation(manager, unit, &self.invocation)?;
                if provider.kind == ProviderKind::Managed { manager.unref(unit)?; }
                Ok(())
            })();
            if let Err(error) = result { failure = Some(error); }
        }
        if let Some(child) = &mut self.child {
            if let Err(error) = async_io::block_on(stop_launcher(child, &manager.deadline)) {
                failure.get_or_insert(error);
            }
        }
        match failure { Some(error) => Err(error), None => Ok(()) }
    }
}

async fn stop_launcher(child: &mut Child, deadline: &Deadline) -> Result<(), SessionError> {
    if child.try_wait()?.is_some() { return Ok(()); }
    // The unreaped child cannot be replaced before this handle is opened.
    // Keep that same handle for escalation, even when the budget expires.
    let handle = ProcessHandle::open(child.id())?;
    let graceful = deadline.bound(async {
        if let Some(handle) = &handle { handle.signal(libc::SIGTERM)?; }
        wait_for_launcher(child, Duration::from_secs(5)).await
    }).await;
    if matches!(graceful, Ok(true)) { return Ok(()); }

    if let Some(handle) = &handle { handle.signal(libc::SIGKILL)?; }
    if let Err(error) = graceful {
        // Reap if already exited, but do not wait beyond the shared deadline.
        // An unconfirmed exit must remain a cleanup failure for recovery.
        let _ = child.try_wait();
        return Err(error);
    }
    if deadline.bound(wait_for_launcher(child, Duration::from_secs(1))).await? { return Ok(()); }
    Err(SessionError::State("session launcher did not exit after SIGKILL".into()))
}

async fn wait_for_launcher(child: &mut Child, timeout: Duration) -> Result<bool, SessionError> {
    let limit = Instant::now() + timeout;
    loop {
        if child.try_wait()?.is_some() { return Ok(true); }
        let now = Instant::now();
        if now >= limit { return Ok(false); }
        async_io::Timer::at((now + Duration::from_millis(50)).min(limit)).await;
    }
}

#[cfg(test)]
#[path = "session_process_tests.rs"]
mod tests;
