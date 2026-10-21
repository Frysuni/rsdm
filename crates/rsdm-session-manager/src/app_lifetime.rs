//! Keep the launch unit alive while its application has escaped to a scope.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::{SessionError, action_gate::ActionGate, app_lifetime_group::{self, Group},
    deadline::Deadline, processes::ProcessHandle, runtime::{self, AppRecord, Runtime}};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LifetimeProcess {
    pub pid: u32,
    pub pidfd_id: u64,
}

pub(super) fn owns_process(app: &AppRecord, pid: u32, handle: &ProcessHandle) -> std::io::Result<bool> {
    let Some(lifetime) = &app.lifetime_process else { return Ok(false); };
    Ok(lifetime.pid == pid && lifetime.pidfd_id != 0 && handle.id()? == Some(lifetime.pidfd_id))
}

pub(super) fn exclude_process(app: &AppRecord, pids: &mut Vec<u32>) -> Result<(), SessionError> {
    let Some(lifetime) = &app.lifetime_process else { return Ok(()); };
    if !pids.contains(&lifetime.pid) { return Ok(()); }
    if let Some(handle) = ProcessHandle::open(lifetime.pid)? {
        if owns_process(app, lifetime.pid, &handle)? { pids.retain(|pid| *pid != lifetime.pid); }
    }
    Ok(())
}

pub(super) async fn start(generation: &str, unit: &str, actions: ActionGate) -> Result<(), SessionError> {
    let generation = generation.to_owned();
    let records = runtime::blocking(move || Runtime::open(&generation)?.ok_or_else(|| SessionError::State("application generation is missing".into()))).await?;
    let runtime = records.clone();
    let name = unit.to_owned();
    let (app, config_path) = runtime::blocking(move || Ok((runtime.app(&name)?, runtime.session()?.config_path))).await?;
    if app.original_process.is_none() { return Ok(()); }
    let actions = actions.with_generation(records.clone());
    actions.check_generation_async().await?;
    // Keep the running image reachable after a package update replaces its path.
    let mut command = tokio::process::Command::new("/proc/self/exe");
    if let Some(path) = config_path { command.arg("--config").arg(path); }
    command.args(["session", "app-watch", "--generation", &records.generation, "--unit", unit]);
    command.stdin(std::process::Stdio::null());
    // Each guard schedules one observation; avoid a worker per CPU per app.
    command.env("TOKIO_WORKER_THREADS", "1");
    actions.check()?;
    let mut child = command.spawn()?;
    let pid = child.id().ok_or_else(|| SessionError::State("application lifetime process did not start".into()))?;
    let ready = match tokio::time::timeout(Duration::from_secs(2), wait_ready(&records, &app, pid, &mut child, &actions)).await {
        Ok(result) => result,
        Err(_) => Err(SessionError::State("application lifetime process did not become ready".into())),
    };
    if ready.is_err() {
        let _ = child.start_kill();
        let _ = tokio::time::timeout(Duration::from_secs(1), child.wait()).await;
    }
    ready
}

async fn wait_ready(records: &Runtime, app: &AppRecord, pid: u32, child: &mut tokio::process::Child, actions: &ActionGate) -> Result<(), SessionError> {
    loop {
        actions.check_generation_async().await?;
        let runtime = records.clone();
        let name = app.unit.clone();
        let saved = runtime::blocking(move || runtime.app(&name)).await?;
        if saved.original_process != app.original_process || saved.invocation_id != app.invocation_id {
            return Err(SessionError::State("application changed before lifetime observation".into()));
        }
        if let Some(handle) = ProcessHandle::open(pid)? {
            if owns_process(&saved, pid, &handle)? && handle.alive()? { return Ok(()); }
        }
        if child.try_wait()?.is_some() { return Err(SessionError::State("application lifetime process exited before readiness".into())); }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

pub(super) async fn watch(generation: &str, unit: &str, actions: ActionGate) -> Result<(), SessionError> {
    let generation = generation.to_owned();
    let runtime = runtime::blocking(move || Runtime::open(&generation)?.ok_or_else(|| SessionError::State("application generation is missing".into()))).await?;
    let actions = actions.with_generation(runtime.clone());
    actions.check_generation_async().await?;
    let name = unit.to_owned();
    let mut lifetime = runtime::blocking(move || initialize(runtime, name, actions)).await?;
    // Once admitted, this process is a passive lifecycle dependency. It does
    // not connect to backends or dispatch actions, including after disable.
    loop {
        let (next, running) = runtime::blocking(move || {
            let running = lifetime.running()?;
            Ok((lifetime, running))
        }).await?;
        if !running { return Ok(()); }
        lifetime = next;
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

struct Lifetime {
    runtime: Runtime,
    app: AppRecord,
    original: ProcessHandle,
    root: Group,
    migrated: Option<Group>,
}

fn initialize(runtime: Runtime, unit: String, actions: ActionGate) -> Result<Lifetime, SessionError> {
    let deadline = Deadline::default();
    deadline.set(crate::processes::monotonic_usec()?.saturating_add(2_000_000));
    let _lease = runtime.app_lease_until(&unit, &deadline)?;
    actions.check_generation()?;
    let mut app = runtime.app(&unit)?;
    let origin = app.original_process.as_ref().ok_or_else(|| SessionError::State("application origin is missing".into()))?;
    let original = ProcessHandle::open(origin.pid)?.ok_or_else(|| SessionError::State("original application process exited".into()))?;
    if original.id()? != Some(origin.pidfd_id) || !original.alive()? {
        return Err(SessionError::State("original application process changed".into()));
    }
    let root = Group::open(&app_lifetime_group::process_group(std::process::id())?)?;
    if app_lifetime_group::process_group(origin.pid)? != root.identity.path || !original.alive()? {
        return Err(SessionError::State("lifetime observer is outside the application's launch cgroup".into()));
    }
    let handle = ProcessHandle::open(std::process::id())?.ok_or_else(|| SessionError::State("lifetime observer exited".into()))?;
    let pidfd_id = handle.id()?.ok_or_else(|| SessionError::State("lifetime observer has no stable kernel identity".into()))?;
    if app.lifetime_process.is_some() { return Err(SessionError::State("application already has a lifetime observer".into())); }
    actions.check_generation()?;
    app.lifetime_process = Some(LifetimeProcess { pid: std::process::id(), pidfd_id });
    runtime.save_app(&app)?;
    Ok(Lifetime { runtime, app, original, root, migrated: None })
}

impl Lifetime {
    fn running(&mut self) -> Result<bool, SessionError> {
        let original_alive = self.original.alive()?;
        let current = match self.runtime.app(&self.app.unit) {
            Ok(current) => current,
            Err(SessionError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => return Ok(original_alive),
            Err(error) => return Err(error),
        };
        if current.original_process != self.app.original_process || current.invocation_id != self.app.invocation_id {
            return Err(SessionError::State("application changed during lifetime observation".into()));
        }
        if self.migrated.is_none() {
            if let Some(identity) = current.migrated_scope.as_ref().and_then(|scope| scope.lifetime_group.as_ref()) {
                self.migrated = Group::restore(identity)?;
            }
        }
        if original_alive {
            self.pin_migration()?;
            return Ok(true);
        }
        Ok(self.root.contains_other_processes(std::process::id())?
            || self.migrated.as_ref().map_or(Ok(false), Group::populated)?)
    }

    fn pin_migration(&mut self) -> Result<(), SessionError> {
        if self.migrated.is_some() { return Ok(()); }
        let origin = self.app.original_process.as_ref().expect("captured original process");
        let path = match app_lifetime_group::process_group(origin.pid) {
            Ok(path) => path,
            Err(SessionError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        if path == self.root.identity.path { return Ok(()); }
        let group = match Group::open(&path) {
            Ok(group) => group,
            Err(SessionError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        let current = match app_lifetime_group::process_group(origin.pid) {
            Ok(current) => current,
            Err(SessionError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        if self.original.alive()? && current == path {
            self.migrated = Some(group);
        }
        Ok(())
    }
}
