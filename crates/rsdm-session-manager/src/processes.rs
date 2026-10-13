//! Cgroup membership and signals that cannot hit a reused process ID.

pub(super) use rsdm_system::{clock::monotonic_usec, process_handle::ProcessHandle};

use std::sync::atomic::{AtomicBool, Ordering};

use super::{SessionError, app_stop::ShutdownControl, bus::{UserManager, missing_unit}, runtime::AppRecord};

const SERVICE: &str = "org.freedesktop.systemd1.Service";

pub(super) async fn verify_invocation(manager: &UserManager, app: &AppRecord) -> Result<bool, SessionError> {
    match manager.invocation_id(&app.unit).await {
        Ok(id) => {
            // An unreferenced unit can lose its invocation identity as its
            // final processes exit. Require both stopped state and an empty
            // cgroup before treating that terminal race as a closed app.
            if id.iter().all(|byte| *byte == 0) && !manager.active(&app.unit).await? {
                let mut pids = manager.processes(&app.unit).await?.into_iter().map(|(_, pid, _)| pid).collect();
                super::app_lifetime::exclude_process(app, &mut pids)?;
                if pids.is_empty() { return Ok(false); }
            }
            if app.invocation_id.is_empty() || id != app.invocation_id {
                return Err(SessionError::State(format!("{} invocation changed", app.unit)));
            }
            Ok(true)
        }
        Err(SessionError::Bus(error)) if missing_unit(&error) => Ok(false),
        Err(error) => Err(error),
    }
}

pub(super) async fn app_processes(manager: &UserManager, app: &AppRecord) -> Result<Vec<u32>, SessionError> {
    let mut pids = service_processes(manager, app).await?;
    if let Some(scope) = super::app_scope::registered_scope(app)? {
        pids.extend(super::app_scope::processes(manager, scope).await?);
        pids.sort_unstable();
        pids.dedup();
    }
    super::app_lifetime::exclude_process(app, &mut pids)?;
    if let Some(origin) = &app.original_process {
        if !pids.contains(&origin.pid) {
            if let Some(handle) = ProcessHandle::open(origin.pid)? {
                if handle.id()? == Some(origin.pidfd_id) && handle.alive()? {
                    return Err(SessionError::State("original application process has an unverified cgroup".into()));
                }
            }
        }
    }
    Ok(pids)
}

async fn service_processes(manager: &UserManager, app: &AppRecord) -> Result<Vec<u32>, SessionError> {
    if !verify_invocation(manager, app).await? { return Ok(Vec::new()); }
    let control: u32 = match manager.unit_property(&app.unit, SERVICE, "ControlPID").await {
        Ok(pid) => pid,
        Err(SessionError::Bus(error)) if missing_unit(&error) => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    Ok(manager.processes(&app.unit).await?.into_iter()
        .map(|(_, pid, _)| pid).filter(|pid| *pid != control).collect())
}

pub(super) async fn generation_invocation(
    manager: &UserManager, unit: &str, generation: &str,
) -> Result<Option<Vec<u8>>, SessionError> {
    let result = async {
        let id = manager.invocation_id(unit).await?;
        let environment: Vec<String> = manager.unit_property(unit, SERVICE, "Environment").await?;
        if id.is_empty() || !environment.contains(&format!("{}={generation}", super::identity::GENERATION_ENV))
            || manager.invocation_id(unit).await? != id
        {
            return Err(SessionError::State(format!("{unit} does not belong to this generation")));
        }
        Ok(Some(id))
    }.await;
    match result {
        Err(SessionError::Bus(error)) if missing_unit(&error) => Ok(None),
        other => other,
    }
}

pub(super) async fn signal_app(manager: &UserManager, app: &AppRecord, signal: i32, control: &ShutdownControl) -> Result<bool, SessionError> {
    let sent = AtomicBool::new(false);
    signal_app_tracked(manager, app, signal, control, &sent).await?;
    Ok(sent.load(Ordering::Relaxed))
}

// Keep accepted signals observable when a later ownership check fails.
pub(super) async fn signal_app_tracked(manager: &UserManager, app: &AppRecord, signal: i32, control: &ShutdownControl, sent: &AtomicBool) -> Result<(), SessionError> {
    let processes = app_processes(manager, app).await?;
    if processes.is_empty() || control.cancelled() { return Ok(()); }
    let groups = if app.migrated_scope.is_some() { process_groups(manager, app).await? }
        else { vec![service_group(manager, &app.unit).await?] };
    let lineage = scope_lineage(manager, app).await?;

    // Bound live descriptors even for large browser cgroups. Prepare the
    // owned pidfds before the final invocation and cancellation checks.
    for batch in processes.chunks(32) {
        let batch = batch.to_vec();
        let groups = groups.clone();
        let lineage = lineage.clone();
        let owned = super::runtime::blocking(move || owned_process_handles(&batch, &groups, lineage.as_ref())).await?;
        if control.cancelled() { return Ok(()); }
        let service = verify_invocation(manager, app).await?;
        let scope = if let Some(scope) = &app.migrated_scope { super::app_scope::current(manager, scope).await? } else { false };
        if !service && !scope { return Ok(()); }
        for handle in owned {
            manager.actions.check_generation_async().await?;
            if control.cancelled() { return Ok(()); }
            manager.actions.check()?;
            if handle.signal(signal)? { sent.store(true, Ordering::Relaxed); }
        }
    }
    Ok(())
}

fn owned_process_handles(processes: &[u32], groups: &[String], lineage: Option<&(String, Vec<super::app_origin::OriginalProcess>)>) -> Result<Vec<ProcessHandle>, SessionError> {
    let mut owned = Vec::new();
    for &pid in processes {
        let Some(handle) = ProcessHandle::open(pid)? else { continue; };
        let membership = match std::fs::read_to_string(format!("/proc/{pid}/cgroup")) {
            Ok(membership) => membership,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        if !groups.iter().any(|group| belongs_to_cgroup(&membership, group)) { continue; }
        if let Some((group, origins)) = lineage {
            if belongs_to_cgroup(&membership, group) && !super::app_scope_lineage::owned(pid, &handle, origins)? { continue; }
        }
        owned.push(handle);
    }
    Ok(owned)
}

async fn scope_lineage(manager: &UserManager, app: &AppRecord) -> Result<Option<(String, Vec<super::app_origin::OriginalProcess>)>, SessionError> {
    let Some(scope) = &app.migrated_scope else { return Ok(None); };
    let Some(lineage) = &scope.lineage else { return Ok(None); };
    Ok(super::app_scope::group(manager, scope).await?.map(|group| (group, lineage.clone())))
}

pub(super) async fn process_groups(manager: &UserManager, app: &AppRecord) -> Result<Vec<String>, SessionError> {
    let mut groups = Vec::new();
    if verify_invocation(manager, app).await? {
        groups.push(service_group(manager, &app.unit).await?);
    }
    if let Some(scope) = super::app_scope::registered_scope(app)? {
        if let Some(group) = super::app_scope::group(manager, scope).await? { groups.push(group); }
    }
    Ok(groups)
}

async fn service_group(manager: &UserManager, unit: &str) -> Result<String, SessionError> {
    let group: String = manager.unit_property(unit, SERVICE, "ControlGroup").await?;
    if !group.starts_with('/') || group == "/" {
        return Err(SessionError::State(format!("{unit} has no dedicated cgroup")));
    }
    Ok(group)
}

pub(super) fn belongs_to_cgroup(membership: &str, group: &str) -> bool {
    membership.lines().any(|line| {
        let mut fields = line.splitn(3, ':');
        let (Some(hierarchy), Some(controllers), Some(path)) = (fields.next(), fields.next(), fields.next())
        else { return false; };
        let systemd = (hierarchy == "0" && controllers.is_empty())
            || controllers.split(',').any(|name| name == "name=systemd");
        systemd && (path == group || path.strip_prefix(group).is_some_and(|suffix| suffix.starts_with('/')))
    })
}

pub(super) async fn stop_invocation(manager: &UserManager, unit: &str, expected: &[u8]) -> Result<(), SessionError> {
    if !verify_unit_invocation(manager, unit, expected).await? { return Ok(()); }
    manager.with_owner(unit, expected).stop(unit, std::time::Duration::from_secs(15)).await
}

pub(super) async fn verify_unit_invocation(manager: &UserManager, unit: &str, expected: &[u8]) -> Result<bool, SessionError> {
    manager.verify_owner(unit, expected).await
}

pub(super) async fn terminate_main(manager: &UserManager, app: &AppRecord, control: &ShutdownControl) -> Result<bool, SessionError> {
    if control.cancelled() { return Ok(false); }
    let running = verify_invocation(manager, app).await?;
    let mut main = if running { manager.unit_property::<u32>(&app.unit, SERVICE, "MainPID").await? } else { 0 };
    let original = if main == 0 && app.migrated_scope.is_some() { app.original_process.as_ref() } else { None };
    if let Some(origin) = original { main = origin.pid; }
    if main == 0 {
        return signal_app(manager, app, libc::SIGTERM, control).await;
    }
    let Some(handle) = ProcessHandle::open(main)? else {
        return signal_app(manager, app, libc::SIGTERM, control).await;
    };
    let groups = process_groups(manager, app).await?;
    let lineage = scope_lineage(manager, app).await?;
    let owned = super::runtime::blocking(move || owned_process_handles(&[main], &groups, lineage.as_ref())).await?;
    if owned.is_empty() || !handle.alive()? { return Ok(false); }
    if let Some(origin) = original {
        if handle.id()? != Some(origin.pidfd_id) { return Ok(false); }
    }
    if app_processes(manager, app).await?.contains(&main) {
        manager.actions.check_generation_async().await?;
        if control.cancelled() { return Ok(false); }
        manager.actions.check()?;
        return Ok(handle.signal(libc::SIGTERM)?);
    }
    Ok(false)
}


pub(super) fn require_pidfds() -> Result<(), SessionError> {
    let handle = ProcessHandle::open(std::process::id())?
        .ok_or_else(|| SessionError::State("could not pin the session coordinator process".into()))?;
    handle.signal(0).map(|_| ()).map_err(|error| SessionError::State(format!("Linux pidfd signals are required: {error}")))
}

#[cfg(test)]
#[path = "processes_tests.rs"]
mod invocation_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pidfd_remains_bound_to_the_original_child_after_exit() {
        let mut child = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        let handle = ProcessHandle::open(child.id()).unwrap().unwrap();
        assert!(handle.signal(libc::SIGTERM).unwrap());
        assert!(!child.wait().unwrap().success());
        assert!(!handle.signal(libc::SIGTERM).unwrap());
    }

}
