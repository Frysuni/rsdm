//! Observing a managed compositor or an unchanged native session launcher.

use std::{os::unix::process::ExitStatusExt, path::Path, process::{Child, Command, ExitStatus}, time::Duration};

use super::{SessionError, bus::UserManager, provider::{Provider, ProviderKind}};

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
            if child.try_wait()?.is_none() {
                // The unreaped child and its pidfd keep this signal bound to the
                // original launcher, including launchers without a native unit.
                if let Some(handle) = super::processes::ProcessHandle::open(child.id())? {
                    handle.signal(libc::SIGTERM)?;
                }
                let deadline = std::time::Instant::now() + Duration::from_secs(5);
                while child.try_wait()?.is_none() && std::time::Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(50));
                }
                if child.try_wait()?.is_none() { child.kill()?; }
            }
            child.wait()?;
        }
        match failure { Some(error) => Err(error), None => Ok(()) }
    }
}
