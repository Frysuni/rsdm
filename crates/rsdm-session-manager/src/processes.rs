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
            if id.iter().all(|byte| *byte == 0) && !manager.active(&app.unit).await?
                && manager.processes(&app.unit).await?.is_empty()
            { return Ok(false); }
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
    let group: String = manager.unit_property(&app.unit, SERVICE, "ControlGroup").await?;
    if !group.starts_with('/') || group == "/" {
        return Err(SessionError::State(format!("{} has no dedicated cgroup", app.unit)));
    }

    // Bound live descriptors even for large browser cgroups. Prepare the
    // owned pidfds before the final invocation and cancellation checks.
    for batch in processes.chunks(32) {
        let batch = batch.to_vec();
        let group = group.clone();
        let owned = super::runtime::blocking(move || owned_process_handles(&batch, &group)).await?;
        if control.cancelled() { return Ok(()); }
        if !verify_invocation(manager, app).await? { return Ok(()); }
        for handle in owned {
            manager.actions.check_generation_async().await?;
            if control.cancelled() { return Ok(()); }
            manager.actions.check()?;
            if handle.signal(signal)? { sent.store(true, Ordering::Relaxed); }
        }
    }
    Ok(())
}

fn owned_process_handles(processes: &[u32], group: &str) -> Result<Vec<ProcessHandle>, SessionError> {
    let mut owned = Vec::new();
    for &pid in processes {
        let Some(handle) = ProcessHandle::open(pid)? else { continue; };
        let membership = match std::fs::read_to_string(format!("/proc/{pid}/cgroup")) {
            Ok(membership) => membership,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        if belongs_to_cgroup(&membership, group) { owned.push(handle); }
    }
    Ok(owned)
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
    match manager.invocation_id(unit).await {
        Ok(id) if !expected.is_empty() && id == expected => manager.stop(unit, std::time::Duration::from_secs(15)).await,
        Ok(id) if id.iter().all(|byte| *byte == 0) && !manager.active(unit).await?
            && manager.processes(unit).await?.is_empty() => Ok(()),
        Ok(_) => Err(SessionError::State(format!("{unit} invocation changed or was never verified"))),
        Err(SessionError::Bus(error)) if missing_unit(&error) => Ok(()),
        Err(error) => Err(error),
    }
}

pub(super) async fn terminate_main(manager: &UserManager, app: &AppRecord, control: &ShutdownControl) -> Result<bool, SessionError> {
    if control.cancelled() || !verify_invocation(manager, app).await? {
        return Ok(false);
    }
    let main: u32 = manager.unit_property(&app.unit, SERVICE, "MainPID").await?;
    if main == 0 {
        return signal_app(manager, app, libc::SIGTERM, control).await;
    }
    let Some(handle) = ProcessHandle::open(main)? else {
        return signal_app(manager, app, libc::SIGTERM, control).await;
    };
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
