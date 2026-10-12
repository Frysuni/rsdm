//! Capture the original app process while it still belongs to its launch unit.

use serde::{Deserialize, Serialize};

use crate::{SessionError, action_gate::ActionGate, bus::UserManager, deadline::Deadline, processes::{self, ProcessHandle}, runtime::{self, Runtime}};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct OriginalProcess {
    pub pid: u32,
    pub pidfd_id: u64,
}

pub(super) async fn capture(generation: &str, unit: &str, actions: ActionGate) -> Result<(), SessionError> {
    actions.check()?;
    let generation = generation.to_owned();
    let runtime = runtime::blocking(move || Runtime::open(&generation)?.ok_or_else(|| SessionError::State("application generation is missing".into()))).await?;
    let actions = actions.with_generation(runtime.clone());
    actions.check_generation_async().await?;
    let deadline = Deadline::default();
    deadline.set(processes::monotonic_usec()?.saturating_add(2_000_000));
    let manager = UserManager::connect_guarded(deadline, actions).await?;
    capture_process(&manager, &runtime, unit).await
}

async fn capture_process(manager: &UserManager, runtime: &Runtime, unit: &str) -> Result<(), SessionError> {
    let pid = std::process::id();
    let handle = ProcessHandle::open(pid)?.ok_or_else(|| SessionError::State("application process exited".into()))?;
    let Some(pidfd_id) = handle.id()? else { return Ok(()); };
    let invocation = processes::generation_invocation(manager, unit, &runtime.generation).await?
        .ok_or_else(|| SessionError::State("application invocation is missing".into()))?;
    if invocation.iter().all(|byte| *byte == 0) { return Err(SessionError::State("application invocation is unpinned".into())); }
    if manager.unit_property::<u32>(unit, "org.freedesktop.systemd1.Service", "MainPID").await? != pid {
        return Err(SessionError::State("application executor is not the unit's main process".into()));
    }
    let records = runtime.clone();
    let name = unit.to_owned();
    let mut app = runtime::blocking(move || records.app(&name)).await?;
    app.invocation_id = invocation.clone();
    if !processes::app_processes(manager, &app).await?.contains(&pid) || !handle.alive()? {
        return Err(SessionError::State("application executor is outside its invocation".into()));
    }
    save_origin(manager, runtime, unit, invocation, OriginalProcess { pid, pidfd_id }).await
}

async fn save_origin(manager: &UserManager, runtime: &Runtime, unit: &str, invocation: Vec<u8>, origin: OriginalProcess) -> Result<(), SessionError> {
    let records = runtime.clone();
    let name = unit.to_owned();
    let deadline = manager.deadline.clone();
    let actions = manager.actions.clone();
    manager.actions.check_generation_async().await?;
    runtime::blocking(move || {
        let _lease = records.app_lease_until(&name, &deadline)?;
        if records.is_disabled()? { return Err(SessionError::Disabled); }
        let mut app = records.app(&name)?;
        if !app.invocation_id.is_empty() && app.invocation_id != invocation {
            return Err(SessionError::State("application invocation changed before origin capture".into()));
        }
        if app.original_process.as_ref().is_some_and(|original| original != &origin) {
            return Err(SessionError::State("application origin is already captured".into()));
        }
        actions.check()?;
        app.invocation_id = invocation;
        app.original_process = Some(origin);
        records.save_app(&app)
    }).await
}

#[cfg(test)]
#[path = "app_origin_tests.rs"]
mod tests;
