//! Observing a managed compositor or an unchanged native session launcher.

use std::{os::unix::process::ExitStatusExt, path::Path, process::ExitStatus, sync::Arc, time::Duration};

use tokio::process::{Child, Command};

use super::{SessionError, action_gate::ActionGate, bus::UserManager, deadline::Deadline, processes::ProcessHandle, provider::{Provider, ProviderKind}};

#[derive(Clone)]
pub(super) struct SessionProcess {
    pub unit: Option<String>,
    pub invocation: Vec<u8>,
    pub(super) child: Option<Arc<tokio::sync::Mutex<Child>>>,
    pub(super) compositor: Arc<std::sync::Mutex<Option<Arc<ProcessHandle>>>>,
    pub(super) submitted: Arc<std::sync::atomic::AtomicBool>,
    pub booting: bool,
    child_exit: Option<i32>,
    pub(super) unit_exit: Option<i32>,
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
            return Ok(Self { unit: Some(managed_unit), invocation: Vec::new(), child: None, compositor: Default::default(), submitted: Default::default(), booting: true,
                child_exit: None, unit_exit: None, native_unit_seen: false, launched: std::time::Instant::now() });
        }
        let child = Command::new(argv.first().ok_or(SessionError::EmptyCommand)?)
            .args(&argv[1..]).env_clear().envs(environment.iter().map(|(key, value)| (key, value)))
            .current_dir(directory).spawn().map_err(SessionError::Spawn)?;
        Ok(Self { unit: provider.native_unit.clone(), invocation: Vec::new(), child: Some(Arc::new(tokio::sync::Mutex::new(child))), compositor: Default::default(), submitted: Default::default(), booting: false,
            child_exit: None, unit_exit: None, native_unit_seen: false, launched: std::time::Instant::now() })
    }

    pub async fn exited(&mut self, manager: &UserManager) -> Result<Option<i32>, SessionError> {
        if self.booting { return Ok(None); }
        if self.unit_exit.is_some() { return Ok(self.unit_exit); }
        if let Some(child) = &self.child {
            let mut child = child.lock().await;
            if self.child_exit.is_none() { self.child_exit = child.try_wait().map_err(SessionError::Wait)?.map(exit_status_code); }
        }
        let Some(unit) = &self.unit else { return Ok(self.child_exit); };
        if manager.active(unit).await? {
            if self.child.is_some() && !self.native_unit_seen {
                self.invocation = manager.invocation_id(unit).await?;
                // RefUnit counts calls per peer; never replay an uncertain reply.
                self.native_unit_seen = true;
                manager.retain(unit, &self.invocation).await?;
            }
            self.pin_compositor(manager, unit).await?;
            return Ok(None);
        }
        if self.child.is_some() && !self.native_unit_seen {
            return Ok(if self.launched.elapsed() > Duration::from_secs(10) { self.child_exit } else { None });
        }

        self.unit_exit = manager.read(|| async {
            super::session_exit::read(&manager.connection(), unit, &self.invocation).await
        }).await?.map(|code| if code == 0 { self.child_exit.unwrap_or(0) } else { code });
        Ok(self.unit_exit)
    }

    async fn pin_compositor(&self, manager: &UserManager, unit: &str) -> Result<(), SessionError> {
        if self.compositor.lock().unwrap().is_some() { return Ok(()); }
        let pid: u32 = match manager.unit_property(unit, "org.freedesktop.systemd1.Service", "MainPID").await {
            Ok(pid) => pid,
            Err(SessionError::Bus(zbus::Error::FDO(error))) if matches!(*error,
                zbus::fdo::Error::UnknownProperty(_) | zbus::fdo::Error::UnknownInterface(_)) => return Ok(()),
            Err(error) => return Err(error),
        };
        if pid == 0 { return Ok(()); }
        let handle = ProcessHandle::open(pid)?;
        if manager.invocation_id(unit).await? != self.invocation {
            return Err(SessionError::State("compositor invocation changed before its PID was pinned".into()));
        }
        *self.compositor.lock().unwrap() = handle.map(Arc::new);
        Ok(())
    }

    pub(super) async fn pin_native_compositor(&self, manager: &UserManager, provider: &Provider) -> Result<(), SessionError> {
        if !provider.native_desktop() || self.compositor.lock().unwrap().is_some() { return Ok(()); }
        let handle = manager.read(|| async { super::native_compositor::pin(&manager.connection(), provider).await }).await?;
        *self.compositor.lock().unwrap() = handle;
        Ok(())
    }

    pub async fn stop(&mut self, manager: &UserManager, provider: &Provider, generation: &str) -> Result<(), SessionError> {
        manager.actions.check()?;
        let mut failure = None;
        if let Some(unit) = &self.unit {
            let result = async {
                if self.invocation.is_empty() && provider.kind == ProviderKind::Managed {
                    self.invocation = super::processes::generation_invocation(manager, unit, generation).await?.unwrap_or_default();
                }
                super::processes::stop_invocation(manager, unit, &self.invocation).await?;
                if provider.kind == ProviderKind::Managed || self.native_unit_seen {
                    manager.with_owner(unit, &self.invocation).unref(unit).await?;
                }
                Ok(())
            }.await;
            if let Err(error) = result { failure = Some(error); }
        }
        if let Some(child) = &self.child {
            if let Some(unit) = &self.unit {
                super::processes::verify_unit_invocation(manager, unit, &self.invocation).await?;
            }
            let mut child = manager.actions.guard(child.lock()).await?;
            if let Err(error) = stop_launcher(&mut child, &manager.deadline, manager.actions.clone()).await {
                failure.get_or_insert(error);
            }
        }
        match failure { Some(error) => Err(error), None => Ok(()) }
    }
}

async fn stop_launcher(child: &mut Child, deadline: &Deadline, actions: ActionGate) -> Result<(), SessionError> {
    actions.check()?;
    if child.try_wait()?.is_some() { return Ok(()); }
    // The unreaped child cannot be replaced before this handle is opened.
    // Keep that same handle for escalation, even when the budget expires.
    let Some(pid) = child.id() else { return Ok(()); };
    let handle = ProcessHandle::open(pid)?;
    let graceful = deadline.bound(async {
        if let Some(handle) = &handle {
            actions.check()?;
            handle.signal(libc::SIGTERM)?;
        }
        actions.guard(wait_for_launcher(child, Duration::from_secs(5))).await?
    }).await;
    if matches!(graceful, Ok(true)) { return Ok(()); }

    actions.check()?;
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
    match tokio::time::timeout(timeout, child.wait()).await {
        Ok(result) => {
            result?;
            Ok(true)
        },
        Err(_) => Ok(false),
    }
}

#[cfg(test)]
#[path = "session_process_tests.rs"]
mod tests;
