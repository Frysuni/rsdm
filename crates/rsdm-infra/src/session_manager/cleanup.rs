//! Recovery is restricted to recorded invocations of one login generation.

use std::time::Duration;

use zbus::fdo::RequestNameFlags;

use super::{
    SessionError, app_stop::{self, ShutdownControl}, apps, bus::UserManager,
    control::BUS_NAME, deadline::Deadline, env, identity::GENERATION_ENV,
    runtime::Runtime, units,
};

pub fn cleanup(generation: &str) -> Result<(), SessionError> {
    let Some(runtime) = Runtime::open(generation)? else { return Ok(()); };
    // Validation may have created the private directory before publishing a
    // session record. No service or environment side effect precedes that
    // record, so an empty generation needs no user-manager connection.
    if !runtime.path.join("session.toml").try_exists()? && runtime.apps()?.is_empty() {
        return Ok(());
    }
    let _lease = super::session_lease::SessionLease::acquire()?;
    let deadline = Deadline::default();
    deadline.set(recovery_deadline(runtime.session()?.shutdown_deadline_usec, 0)?);
    let manager = UserManager::connect_until(deadline)?;
    acquire_lease(&manager)?;
    recover(&manager, &runtime)
}

pub(super) fn recover(manager: &UserManager, runtime: &Runtime) -> Result<(), SessionError> {
    let mut record = runtime.session()?;
    let control = std::sync::Arc::new(ShutdownControl::for_manager(manager));
    control.noncancelable.store(true, std::sync::atomic::Ordering::SeqCst);
    control.recovery.store(true, std::sync::atomic::Ordering::SeqCst);
    let deadline = recovery_deadline(record.shutdown_deadline_usec, manager.deadline.get())?;
    control.force(deadline);
    record.shutdown_deadline_usec = Some(deadline);
    let mut failure = None;
    if let Err(error) = runtime.save_session(&record) { failure = Some(error); }
    if let Err(error) = app_stop::prepare_apps(&manager, &runtime, runtime.apps()?, control) {
        failure = Some(error);
    }
    for app in runtime.apps()? {
        if let Err(error) = apps::finish(&manager, &runtime, &app) { failure.get_or_insert(error); }
    }
    let owns_environment = manager.environment()?.contains(&format!("{GENERATION_ENV}={}", runtime.generation));
    if record.owns_targets && owns_environment {
        for target in [units::AUTOSTART_TARGET, units::SESSION_TARGET, units::PRE_TARGET] {
            if let Err(error) = manager.stop(target, Duration::from_secs(15)) { failure.get_or_insert(error); }
        }
    }
    if let Err(error) = manager.stop(&record.anchor_unit, Duration::from_secs(15)) { failure.get_or_insert(error); }
    let _ = manager.unref(&record.anchor_unit);
    if let Some(unit) = &record.compositor_unit {
        let stopped = (|| {
            if record.compositor_invocation.is_empty() && record.provider == "managed" {
                if let Some(id) = super::processes::generation_invocation(manager, unit, &runtime.generation)? {
                    record.compositor_invocation = id;
                } else { return Ok(()); }
            }
            super::processes::stop_invocation(manager, unit, &record.compositor_invocation)
        })();
        if let Err(error) = stopped { failure.get_or_insert(error); }
        if record.provider == "managed" { let _ = manager.unref(unit); }
    }
    if owns_environment {
        if let Err(error) = env::clear_owned(&manager, &record.exported_environment) { failure.get_or_insert(error); }
    }
    if let Err(error) = runtime.reap_closed_app_locks() { failure.get_or_insert(error); }
    if let Some(error) = failure { return Err(error); }
    record.phase = rsdm_core::domain::SessionPhase::Closed;
    runtime.save_session(&record)
}

pub(super) fn recovery_deadline(saved: Option<u64>, current: u64) -> Result<u64, SessionError> {
    let mut deadline = super::processes::monotonic_usec()?.saturating_add(5_000_000);
    if let Some(saved) = saved { deadline = deadline.min(saved); }
    if current != 0 { deadline = deadline.min(current); }
    Ok(deadline)
}

fn acquire_lease(manager: &UserManager) -> Result<(), SessionError> {
    async_io::block_on(manager.deadline.bound(async {
        let lease = async {
            loop {
                match manager.connection().request_name_with_flags(BUS_NAME, RequestNameFlags::DoNotQueue.into()).await {
                    Ok(_) => return Ok(()),
                    Err(zbus::Error::NameTaken) => { async_io::Timer::after(Duration::from_millis(50)).await; }
                    Err(error) => return Err(error.into()),
                }
            }
        };
        futures_lite::future::or(lease, async {
            async_io::Timer::after(Duration::from_secs(2)).await;
            Err(zbus::Error::from(zbus::fdo::Error::TimedOut("acquiring the recovery bus name timed out".into())).into())
        }).await
    }))
}

#[cfg(test)]
#[path = "cleanup_tests.rs"]
mod tests;
