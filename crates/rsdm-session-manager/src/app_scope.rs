//! Adopt a scope only while its original app process proves the migration.

use serde::{Deserialize, Serialize};

use crate::{SessionError, bus::{UserManager, missing_unit}, processes::{self, ProcessHandle}, runtime::{self, AppRecord, Runtime}};

const UNIT: &str = "org.freedesktop.systemd1.Unit";
const SCOPE: &str = "org.freedesktop.systemd1.Scope";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MigratedScope {
    pub unit: String,
    pub invocation_id: Vec<u8>,
}

pub(super) fn registered_scope(app: &AppRecord) -> Result<Option<&MigratedScope>, SessionError> {
    let Some(scope) = &app.migrated_scope else { return Ok(None); };
    if app.original_process.as_ref().is_none_or(|origin| origin.pid == 0 || origin.pidfd_id == 0)
        || !scope.unit.ends_with(".scope") || scope.invocation_id.is_empty() || scope.invocation_id.iter().all(|byte| *byte == 0)
    {
        return Err(SessionError::State("application scope has no authenticated origin".into()));
    }
    Ok(Some(scope))
}

pub(super) async fn refresh(manager: &UserManager, records: &Runtime, mut app: AppRecord) -> Result<AppRecord, SessionError> {
    if registered_scope(&app)?.is_some() || app.original_process.is_none() { return Ok(app); }
    let Some(scope) = discover(manager, &app).await? else { return Ok(app); };
    let runtime = records.clone();
    let deadline = manager.deadline.clone();
    let actions = manager.actions.clone();
    manager.actions.check_generation_async().await?;
    app = runtime::blocking(move || {
        let _lease = runtime.app_lease_until(&app.unit, &deadline)?;
        if runtime.is_disabled()? { return Err(SessionError::Disabled); }
        let mut current = runtime.app(&app.unit)?;
        if current.invocation_id != app.invocation_id || current.original_process != app.original_process {
            return Err(SessionError::State("application changed before scope adoption".into()));
        }
        if current.migrated_scope.as_ref().is_some_and(|saved| saved != &scope) {
            return Err(SessionError::State("application scope is already captured".into()));
        }
        actions.check()?;
        current.migrated_scope = Some(scope);
        runtime.save_app(&current)?;
        Ok(current)
    }).await?;
    Ok(app)
}

async fn discover(manager: &UserManager, app: &AppRecord) -> Result<Option<MigratedScope>, SessionError> {
    let Some(origin) = &app.original_process else { return Ok(None); };
    let Some(handle) = ProcessHandle::open(origin.pid)? else { return Ok(None); };
    if origin.pidfd_id == 0 || handle.id()? != Some(origin.pidfd_id) || !handle.alive()?
        || !processes::verify_invocation(manager, app).await?
    { return Ok(None); }
    let main: u32 = manager.unit_property(&app.unit, "org.freedesktop.systemd1.Service", "MainPID").await?;
    if main != origin.pid { return Ok(None); }
    let unit = manager.unit_for_pid(origin.pid).await?;
    if !unit.ends_with(".scope") || unit == app.unit { return Ok(None); }
    if !manager.unit_property::<bool>(&unit, UNIT, "Transient").await? { return Ok(None); }
    let invocation_id = manager.invocation_id(&unit).await?;
    if invocation_id.is_empty() || invocation_id.iter().all(|byte| *byte == 0) { return Ok(None); }
    let group: String = manager.unit_property(&unit, SCOPE, "ControlGroup").await?;
    let mut pids = manager.processes(&unit).await?.into_iter().map(|(_, pid, _)| pid).collect::<Vec<_>>();
    pids.sort_unstable();
    let initial = pids.clone();
    let origin = origin.clone();
    let Some(owned) = runtime::blocking(move || super::app_scope_lineage::pin_owned(&pids, &group, &origin)).await? else { return Ok(None); };
    let mut current = manager.processes(&unit).await?.into_iter().map(|(_, pid, _)| pid).collect::<Vec<_>>();
    current.sort_unstable();
    if initial != current || manager.unit_for_pid(main).await? != unit || manager.invocation_id(&unit).await? != invocation_id
        || !processes::verify_invocation(manager, app).await? || !handle.alive()?
    { return Ok(None); }
    for handle in owned { if !handle.alive()? { return Ok(None); } }
    Ok(Some(MigratedScope { unit, invocation_id }))
}

pub(super) async fn processes(manager: &UserManager, scope: &MigratedScope) -> Result<Vec<u32>, SessionError> {
    let Some(_) = group(manager, scope).await? else { return Ok(Vec::new()); };
    let records = match manager.processes(&scope.unit).await {
        Ok(records) => records,
        Err(SessionError::Bus(error)) if missing_unit(&error) => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let pids = records.into_iter().map(|(_, pid, _)| pid).collect();
    if !current(manager, scope).await? { return Ok(Vec::new()); }
    Ok(pids)
}

pub(super) async fn group(manager: &UserManager, scope: &MigratedScope) -> Result<Option<String>, SessionError> {
    if !current(manager, scope).await? { return Ok(None); }
    let group: String = match manager.unit_property(&scope.unit, SCOPE, "ControlGroup").await {
        Ok(group) => group,
        Err(SessionError::Bus(error)) if missing_unit(&error) => return Ok(None),
        Err(error) => return Err(error),
    };
    if !group.starts_with('/') || group == "/" {
        return Err(SessionError::State("application scope has no dedicated cgroup".into()));
    }
    Ok(Some(group))
}

pub(super) async fn current(manager: &UserManager, scope: &MigratedScope) -> Result<bool, SessionError> {
    if !scope.unit.ends_with(".scope") { return Err(SessionError::State("invalid migrated application scope".into())); }
    let checked = async {
        let id = manager.invocation_id(&scope.unit).await?;
        if !scope.invocation_id.is_empty() && scope.invocation_id.iter().any(|byte| *byte != 0) && id == scope.invocation_id {
            return Ok(!stopped(manager, scope).await?);
        }
        if id.iter().all(|byte| *byte == 0) && stopped(manager, scope).await? { return Ok(false); }
        Err(SessionError::State("application scope invocation changed".into()))
    }.await;
    match checked {
        Err(SessionError::Bus(error)) if missing_unit(&error) => Ok(false),
        result => result,
    }
}

async fn stopped(manager: &UserManager, scope: &MigratedScope) -> Result<bool, SessionError> {
    let state: String = manager.unit_property(&scope.unit, UNIT, "ActiveState").await?;
    if !matches!(state.as_str(), "inactive" | "failed") { return Ok(false); }
    if !manager.processes(&scope.unit).await?.is_empty() {
        return Err(SessionError::State("inactive application scope still has processes".into()));
    }
    Ok(true)
}

pub(super) async fn closed(manager: &UserManager, app: &AppRecord) -> Result<bool, SessionError> {
    let Some(scope) = registered_scope(app)? else { return Ok(true); };
    Ok(!current(manager, scope).await?)
}

pub(super) async fn wait_closed(manager: &UserManager, app: &AppRecord) -> Result<(), SessionError> {
    if app.migrated_scope.is_none() { return Ok(()); }
    manager.deadline.bound(async {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while !closed(manager, app).await? { tokio::time::sleep(std::time::Duration::from_millis(20)).await; }
            Ok::<_, SessionError>(())
        }).await.map_err(|_| SessionError::State("application scope did not close after preparation".into()))?
    }).await
}

#[cfg(feature = "xsmp")]
pub(super) async fn peer_app(manager: &UserManager, runtime: &Runtime, unit: &str, pid: u32, process: &ProcessHandle) -> Result<AppRecord, SessionError> {
    let records = runtime.clone();
    let apps = runtime::blocking(move || records.apps()).await?;
    let identity = process.id()?;
    let app = apps.into_iter().find(|app| {
        app.migrated_scope.as_ref().is_some_and(|scope| scope.unit == unit)
            || app.original_process.as_ref().is_some_and(|origin| origin.pid == pid && identity == Some(origin.pidfd_id))
    }).ok_or_else(|| SessionError::State("ICE scope is not a registered application".into()))?;
    let app = refresh(manager, runtime, app).await?;
    if !registered_scope(&app)?.is_some_and(|scope| scope.unit == unit) {
        return Err(SessionError::State("ICE scope has no authenticated application origin".into()));
    }
    Ok(app)
}

#[cfg(test)]
#[path = "app_scope_tests.rs"]
mod tests;
